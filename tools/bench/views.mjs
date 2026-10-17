// Heavy views, each timed still and while panning and orbiting (bench.mjs per view), as one table.
//
// Each view lies in the map's coverage and its camera looks over the landscape: bench.mjs fails a
// view whose centre leaves the covered regions (the server's /api/coverage), or whose camera is under
// 200 m over the ground under it or over the highest ground in view, at the start or along a run.
//
//   node tools/bench/views.mjs [--base http://localhost:5173/] [--only tokyo,alps] [--runs static,pan,orbit]
//                              [--secs 5] [--query bldfield=off] [--out results.json] [--port 9333]
//                              [--set "js run in the page first, as bench.mjs's"] [--cold] [--timeout 240] [--native]
//
// Needs the Chrome bench.mjs describes (real GPU, uncapped frame rate). Window 1440×900 at 2×.
// Per view and run: frame interval p50/p95 (ms), and the GPU time of frames (ms, p50/p95; for the
// gestures, frames that moved the camera). --query adds URL switches before the hash (e.g.
// fskip=, fdbg=, bldfield=off) to attribute cost to a layer. --cold: each view loaded with the browser's
// cache emptied (its load timed: to idle, long tasks), the gestures then fetching what they reach. A
// view that hasn't finished in --timeout seconds (default 240) is reported as hung, its tab closed.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
    return acc;
  }, []),
);

/** name, map= (zoom/lat/lng/bearing/pitch), what makes it heavy. */
export const VIEWS = [
  ['tokyo', '14/35.68120/139.76710/0/60', 'dense 3D city, tilted: meshes and field'],
  ['manhattan', '13.5/40.75800/-73.98500/30/60', 'dense 3D city, tilted, far field to the horizon'],
  ['hongkong', '14/22.28500/114.16000/320/65', 'towers on steep terrain by the water'],
  // (Every view in the map's coverage, its camera over the landscape: bench.mjs checks both, still and
  // along the runs. At the terrain's 3× exaggeration Yosemite's first camera, 12.5/…/70, was 90 m
  // over the valley floor with its walls 3 km over it.)
  ['japanalps', '11/36.30000/137.64000/20/55', 'high mountains: terrain, hill-shading, trees'],
  ['yosemite', '11.5/37.73000/-119.60000/80/60', 'valley terrain with trees'],
  ['pyrenees', '8/42.70000/0.50000/0/75', 'wide tilted view to the horizon'],
  ['lochs', '11/56.70000/-5.25000/45/65', 'coast and sea lochs: water shading on terrain'],
  ['sfbay', '11.5/37.80000/-122.40000/300/65', 'city by the bay: water, buildings, roads'],
];

const PORT = String(args.port ?? 9333);
const base = String(args.base ?? 'http://localhost:5173/');
const runs = String(args.runs ?? 'static,pan,orbit');
const only = args.only ? String(args.only).split(',') : null;
const bench = fileURLToPath(new URL('./bench.mjs', import.meta.url));
const results = {};
for (const [name, map] of VIEWS) {
  if (only && !only.includes(name)) continue;
  const url = `${base}${args.query ? `?${args.query}` : ''}#map=${map}`;
  const a = ['--port', PORT, '--url', url, '--w', '1440', '--h', '900', '--dpr', '2', '--runs', runs, '--secs', String(args.secs ?? 5), '--label', name];
  if (args.set) a.push('--set', String(args.set));
  if (args.cold) a.push('--cold');
  // --native: the headed window's own size and pixel ratio, paced by the display (bench.mjs).
  if (args.native) a.push('--native');
  let r;
  try {
    r = JSON.parse(execFileSync('node', [bench, ...a], { encoding: 'utf8', maxBuffer: 1 << 26, stdio: ['ignore', 'pipe', 'inherit'], timeout: Number(args.timeout ?? 240) * 1000 }));
  } catch (e) {
    console.error(name, 'failed:', String(e).slice(0, 300));
    results[name] = { failed: String(e).slice(0, 300) };
    // (a killed run leaves its tab: closed, so it doesn't draw on behind the next)
    for (const t of await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()) if (t.type === 'page' && t.url !== 'about:blank') await fetch(`http://127.0.0.1:${PORT}/json/close/${t.id}`);
    console.log(`${name.padEnd(10)} | hung or failed`);
    continue;
  }
  results[name] = r;
  const l = r.load;
  const row = [name.padEnd(10), `load: idle ${(l.navToIdleMs / 1000).toFixed(1)} s, ${l.longTasks} long tasks ${l.longTaskMs} ms, longest ${l.topTasks.slice(0, 3).join('/')}`];
  for (const [k, v] of Object.entries(r.runs)) {
    const g = k.startsWith('static') ? v.gpuMs : v.moved.gpu;
    row.push(`${k}: frame ${v.frameMs.p50}/${v.frameMs.p95} gpu ${g.p50}/${g.p95} cpu ${v.renderCpuMs.p50}/${v.renderCpuMs.p95} longest ${v.longTasks.top.slice(0, 3).join('/') || '-'}`);
  }
  console.log(row.join(' | '));
}
if (args.out) fs.writeFileSync(String(args.out), JSON.stringify(results, null, 1));
