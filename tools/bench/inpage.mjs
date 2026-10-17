// Runs the in-page self-benchmark (`#bench`, web/src/selfbench.ts) in Safari or Chrome on this Mac,
// in a normal visible window paced by the display, and collects its results.
//
//   node tools/bench/inpage.mjs --browser safari|chrome [--app http://localhost:5199] [--port 5300]
//        [--views tokyo,manhattan] [--runs static,pan,orbit] [--secs 5] [--cold] [--label name]
//        [--out dir] [--timeout 180] [--w 1440 --h 900 (the window)] [--wd-port 4445] [--prof (selfbench.ts's prof=1)]
//        [--query "fcache=0&glcheck=1" (the page's own switches)]
//
// --app is the map as served (Vite's dev server or the map server). The page is opened on
// http://localhost:<port>/, a small proxy to --app that also takes the page's results
// (POST /api/bench, as the map server would, which saves them under <home>/bench/): each is written
// to --out (default $TMPDIR/scenic-bench/) and printed as a row. --cold: view i opened on port + i, an
// origin whose HTTP cache is empty, so every view loads cold (no browser data is cleared); the
// proxy listens on each. Names under localhost (roads.localhost …, web/src/hosts.ts) reach the proxy
// too. Safari: a WebDriver session (safaridriver: Develop > Allow Remote Automation, once
// `safaridriver --enable`), a window of its own without the owner's extensions, closed at the end.
// Chrome: a window of its own (a fresh profile in the scratch directory,
// GPU and display pacing as the owner's), quit by its PID at the end.
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
    return acc;
  }, []),
);
const browser = String(args.browser ?? 'safari');
const app = new URL(String(args.app ?? 'http://localhost:5199'));
const PORT = Number(args.port ?? 5300);
const VIEWS = JSON.parse(fs.readFileSync(new URL('../../web/src/data/benchviews.json', import.meta.url), 'utf8')).map((v) => v[0]);
const views = args.views ? String(args.views).split(',') : VIEWS;
const outDir = String(args.out ?? path.join(os.tmpdir(), 'scenic-bench'));
const label = String(args.label ?? browser);
const sid = `${new Date().toISOString().slice(0, 16).replace(/[-:T]/g, '')}${args.cold ? 'c' : ''}`;
fs.mkdirSync(outDir, { recursive: true });

// The load on this Mac as the run starts (rerun anything measured on a loaded machine).
const load = execFileSync('ps', ['-Ao', 'pid,%cpu,comm', '-r'], { encoding: 'utf8' }).split('\n').slice(1, 6).map((l) => l.trim().replace(/\s+/g, ' ').slice(0, 90));
console.error(`load: ${load.join(' | ')}`);

const got = new Map();
let wake = () => {};
const proxy = (req, res) => {
  if (req.method === 'POST' && req.url.startsWith('/api/bench')) {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      const r = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      r.machineLoad = load;
      fs.writeFileSync(path.join(outDir, `${r.sid}-${r.label}-${r.view}.json`), JSON.stringify(r, null, 1));
      got.set(r.view, r);
      res.writeHead(200, { 'content-type': 'application/json' }).end('{"ok":true}');
      wake();
    });
    return;
  }
  const up = http.request({ host: app.hostname, port: app.port, path: req.url, method: req.method, headers: { ...req.headers, host: req.headers.host?.replace(/:\d+$/, `:${app.port}`) } }, (r) => {
    res.writeHead(r.statusCode, r.headers);
    r.pipe(res);
  });
  up.on('error', () => res.writeHead(502).end());
  req.pipe(up);
};
const servers = [];
for (let i = 0; i < (args.cold ? views.length : 1); i++) {
  const s = http.createServer(proxy);
  s.keepAliveTimeout = 5000;
  await new Promise((r) => s.listen(PORT + i, r));
  servers.push(s);
}

const hash = new URLSearchParams({ views: views.join(','), runs: String(args.runs ?? 'static,pan,orbit'), secs: String(args.secs ?? 5), label, sid, ...(args.cold ? { cold: String(PORT) } : {}), ...(args.prof ? { prof: '1' } : {}) });
const url = `http://localhost:${PORT}/?bench=0${args.query ? `&${args.query}` : ''}#bench&${hash}`;
let child = null, session = null;
const WD = `http://127.0.0.1:${args['wd-port'] ?? 4445}`;
const wd = async (method, p, body) => (await (await fetch(`${WD}${p}`, { method, headers: { 'content-type': 'application/json' }, body: body && JSON.stringify(body) })).json()).value;
if (browser === 'safari') {
  // A WebDriver session (safaridriver): a window of its own, without the owner's extensions or data.
  child = spawn('/usr/bin/safaridriver', ['-p', String(args['wd-port'] ?? 4445)], { stdio: 'ignore' });
  for (let i = 0; i < 50 && !session; i++) {
    await new Promise((r) => setTimeout(r, 200));
    session = await wd('POST', '/session', { capabilities: { alwaysMatch: { browserName: 'safari' } } }).then((v) => v.sessionId, () => null);
  }
  if (!session) throw new Error('no Safari WebDriver session (Develop > Allow Remote Automation, safaridriver --enable)');
  await wd('POST', `/session/${session}/window/rect`, { x: 0, y: 0, width: Number(args.w ?? 1440), height: Number(args.h ?? 900) });
  console.error(`safaridriver pid ${child.pid}, session ${session}`);
  // (navigation answers once the page has loaded; the benchmark runs on by itself)
  void wd('POST', `/session/${session}/url`, { url });
} else {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR ?? os.tmpdir(), 'bench-chrome-'));
  child = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    `--user-data-dir=${dir}`, '--no-first-run', '--no-default-browser-check', '--disable-extensions',
    `--window-size=${args.w ?? 1440},${args.h ?? 900}`, '--window-position=0,0', '--new-window', url,
  ], { stdio: 'ignore' });
  console.error(`chrome pid ${child.pid}, profile ${dir}`);
}

const fmt = (s) => (s ? `${s.p50}/${s.p95}` : '-');
const timeout = Number(args.timeout ?? 180) * 1000;
try {
  for (const v of views) {
    const t0 = Date.now();
    while (!got.has(v) && Date.now() - t0 < timeout) await new Promise((r) => { wake = r; setTimeout(r, 1000); });
    const r = got.get(v);
    if (!r) {
      console.log(`${v.padEnd(10)} | no result in ${timeout / 1000} s`);
      break;
    }
    const l = r.load;
    const row = [v.padEnd(10), `${r.viewport} load ${(l.navToSettledMs / 1000).toFixed(1)} s frame ${fmt(l.frameMs)} longest ${l.longestFrames.slice(0, 3).join('/')}`];
    for (const [k, x] of Object.entries(r.runs)) row.push(`${k}: ${x.fps} fps frame ${fmt(x.frameMs)} cpu ${fmt(x.renderCpuMs)}${x.gpuMs ? ` gpu ${fmt(x.gpuMs)}` : ''} longest ${x.longestFrames.slice(0, 3).join('/')}`);
    if (r.invalid) row.push(`INVALID: ${r.invalid}`);
    console.log(row.join(' | '));
  }
} finally {
  for (const s of servers) s.close();
  servers.forEach((s) => s.closeAllConnections?.());
  if (session) await wd('DELETE', `/session/${session}`).catch(() => {});
  if (child) child.kill('SIGTERM');
}
fs.writeFileSync(path.join(outDir, `${sid}-${label}.json`), JSON.stringify(Object.fromEntries(got), null, 1));
console.error(`results: ${path.join(outDir, `${sid}-${label}*.json`)}`);
