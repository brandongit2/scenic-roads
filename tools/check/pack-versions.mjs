// A one-pack publish re-requests only that pack's tiles (task #253): an open page's road tiles are
// counted from Chrome's network log before and after a catalog that changed one z6 (hi) pack.
// Driven through the Chrome DevTools Protocol.
//
// Start a Chrome with remote debugging and a scratch profile, and the map server over a test root
// of your own (--root, --home, --port: never the live server's) whose next catalog `--publish`
// writes (and the server then takes: a server over --root reads its catalog at start, so the
// command restarts it):
//   "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --remote-debugging-port=18229 \
//     --user-data-dir=<scratch>/chrome about:blank
//   node tools/check/pack-versions.mjs --port 18229 --url 'http://127.0.0.1:18153/?checks' \
//     --at -73.125,45.24,9.5 --pack 6/18/22 --publish '<command writing the next catalog>'
//
// It frames `--at` (lng,lat,zoom: best a corner of four z6 packs), waits for the road tiles to
// settle, runs `--publish`, waits for the server to take the new catalog (/api/catalog), has the
// page ask (as it does every minute), and waits again. Passes when every road tile asked for after
// the switch lies under `--pack`, and at least one did.
import { execSync } from 'node:child_process';

const args = Object.fromEntries(process.argv.slice(2).reduce((acc, a, i, all) => {
  if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
  return acc;
}, []));
const PORT = Number(args.port ?? 18229);
const URL_ = String(args.url ?? 'http://127.0.0.1:18153/?checks');
const [lng, lat, zoom] = String(args.at ?? '-73.125,45.24,9.5').split(',').map(Number);
const PACK = String(args.pack ?? '6/18/22');
const origin = new URL(URL_).origin;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const target = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: 'PUT' })).json();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let seq = 0;
const pending = new Map();
const tiles = []; // [time, url]
const seen = new Set();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  // Each worker (the road tiles are fetched from workers): its requests too.
  if (m.method === 'Target.attachedToTarget') {
    const sessionId = m.params.sessionId;
    ws.send(JSON.stringify({ id: ++seq, sessionId, method: 'Network.enable', params: {} }));
    ws.send(JSON.stringify({ id: ++seq, sessionId, method: 'Target.setAutoAttach', params: { autoAttach: true, waitForDebuggerOnStart: true, flatten: true } }));
    ws.send(JSON.stringify({ id: ++seq, sessionId, method: 'Runtime.runIfWaitingForDebugger', params: {} }));
  }
  // (Once each: a worker's request can be reported by its page too.)
  if (m.method === 'Network.requestWillBeSent' && m.params.request.url.includes('/tiles/roads/') && !seen.has(m.params.requestId)) {
    seen.add(m.params.requestId);
    tiles.push([Date.now(), m.params.request.url]);
  }
});
const send = (method, params = {}) => new Promise((res, rej) => {
  const id = ++seq;
  pending.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
  ws.send(JSON.stringify({ id, method, params }));
});
const evaluate = async (expr) => {
  const r = await send('Runtime.evaluate', { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(`page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
  return r.result.value;
};
/** Waits until no road tile was asked for in `quiet` ms (at most `max`). */
async function settle(quiet = 4000, max = 60000) {
  const t0 = Date.now();
  await sleep(quiet);
  while (Date.now() - t0 < max && tiles.length && Date.now() - tiles[tiles.length - 1][0] < quiet) await sleep(500);
}
const zxy = (u) => new URL(u).pathname.split('/').slice(-3).map(Number);
const packOf = ([z, x, y]) => (z <= 2 ? 'root' : z <= 8 ? `3/${x >> (z - 3)}/${y >> (z - 3)}` : `6/${x >> (z - 6)}/${y >> (z - 6)}`);
const catalogN = async () => (await (await fetch(`${origin}/api/catalog`)).json()).n;
const catalogNow = () => catalogN().catch(() => undefined);

await send('Network.enable');
await send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true });
await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false });
await send('Page.navigate', { url: URL_ });
for (let i = 0; i < 120; i++) {
  if (await evaluate('return !!(window.__map && window.__map.loaded())').catch(() => false)) break;
  await sleep(500);
}
await evaluate(`window.__map.jumpTo({ center: [${lng}, ${lat}], zoom: ${zoom}, pitch: 0, bearing: 0 }); return 1;`);
await settle();
const before = tiles.splice(0);
const n0 = await catalogN();
console.log(`catalog ${n0}: ${before.length} road tile requests at ${lng},${lat} z${zoom}; under ${PACK}: ${before.filter(([, u]) => packOf(zxy(u)) === PACK).length}`);

execSync(String(args.publish), { stdio: 'inherit' });
let n1 = n0;
for (let i = 0; i < 90 && (n1 === n0 || n1 === undefined); i++) {
  await sleep(1000);
  n1 = await catalogNow();
}
if (n1 === n0 || n1 === undefined) throw new Error('the server never took the new catalog');
// The page asks again when it comes back into view (catalog.ts), as it would within the minute.
await evaluate(`document.dispatchEvent(new Event('visibilitychange')); return 1;`);
await settle(6000);
const after = tiles.splice(0);
const outside = after.filter(([, u]) => packOf(zxy(u)) !== PACK);
console.log(`catalog ${n1}: ${after.length} road tile requests; outside ${PACK}: ${outside.length}`);
for (const [, u] of after) console.log(`  ${new URL(u).pathname}${new URL(u).search}  (${packOf(zxy(u))})`);
await fetch(`http://127.0.0.1:${PORT}/json/close/${target.id}`).catch(() => {});
ws.close();
const ok = after.length > 0 && outside.length === 0;
console.log(ok ? 'PASS' : 'FAIL');
process.exit(ok ? 0 : 1);
