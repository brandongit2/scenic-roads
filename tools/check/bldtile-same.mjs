// A 3D buildings' z8 area as a page's task (docs/buildings3d.md §3.6–3.7): its task folder cut from
// real work files (`scenic-build bldtile-task`, as a bldtiles job cuts it), then the `bldtile`
// program run over a copy natively (on each thread count given) and over another as WebAssembly
// under Node's WASI (tools/check/wasi-run.mjs, as a page runs it), their outputs (area.tiles,
// area.json) compared byte for byte. Each run's time and peak memory said. Exits 1 on any
// difference.
//
//   node tools/check/bldtile-same.mjs --root <build root> --area <6/x/y | 8/x/y> --work <dir>
//        [--bin target/release] [--wasm target/wasm32-wasip1/release] [--threads 1,8]
//        [--pass d] [--regions dir]
//
// A z6 tile: its area with the most buildings. --root: a root with the tile's (and its
// neighbours') work files in its manifest, its regions and the pass's outlines; only --work is
// written.
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const args = process.argv.slice(2);
const arg = (k, d) => (args.includes(k) ? args[args.indexOf(k) + 1] : d);
const root = arg("--root"), area = arg("--area"), work = arg("--work") && resolve(arg("--work"));
if (!root || !area || !work) {
  console.error("usage: bldtile-same.mjs --root <dir> --area <6/x/y | 8/x/y> --work <dir> [--bin dir] [--wasm dir] [--threads 1,8] [--pass d] [--regions dir]");
  process.exit(2);
}
const bin = resolve(arg("--bin", join(repo, "target/release")));
const wasm = resolve(arg("--wasm", join(repo, "target/wasm32-wasip1/release")));
const threads = arg("--threads", "1,0").split(",").map(Number);

// A command under /usr/bin/time -l: its wall time, peak memory (the footprint macOS gives), and
// its stderr's last lines.
function timed(cmd, argv) {
  const t0 = performance.now();
  const r = spawnSync("/usr/bin/time", ["-l", cmd, ...argv], { encoding: "utf8", maxBuffer: 1 << 26 });
  const s = (performance.now() - t0) / 1000;
  const peak = Number((r.stderr.match(/(\d+)\s+peak memory footprint/) || r.stderr.match(/(\d+)\s+maximum resident set size/) || [0, 0])[1]);
  if (r.status !== 0) {
    console.error(r.stderr.slice(-3000));
    throw new Error(`${cmd} failed (${r.status})`);
  }
  return { s, peakMb: peak / 2 ** 20, err: r.stderr };
}

rmSync(work, { recursive: true, force: true });
mkdirSync(work, { recursive: true });
const task = join(work, "task");
const extra = ["--pass", "--regions"].flatMap((k) => (arg(k) ? [k, arg(k)] : []));
const cutOut = execFileSync(join(bin, "scenic-build"), ["bldtile-task", area, "--root", root, "--scratch", join(work, "scratch"), "--out", task, ...extra], { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
const cut = JSON.parse(cutOut.trim().split("\n").pop());
console.log(`cut ${cut.area}: ${(cut.bytes / 2 ** 20).toFixed(1)} MB of files, ${cut.records} records, predicted ${cut.mem_mb} MB, in ${cut.secs.toFixed(1)} s`);

const runs = [];
for (const n of threads) {
  const d = join(work, `native-${n || "all"}`);
  cpSync(task, d, { recursive: true });
  const r = timed(join(bin, "bldtile"), [d, cut.area, ...(n ? ["--workers", String(n)] : [])]);
  runs.push({ name: `native, ${n || "all"} thread${n === 1 ? "" : "s"}`, d, ...r });
}
{
  const d = join(work, "wasm");
  cpSync(task, d, { recursive: true });
  const r = timed(process.execPath, [join(here, "wasi-run.mjs"), join(wasm, "bldtile.wasm"), "{}", d, cut.area]);
  const m = r.err.match(/run (\d+) exit (\d+) mem (\d+)/);
  runs.push({ name: "WebAssembly (Node's WASI)", d, ...r, s: m ? Number(m[1]) / 1000 : r.s, wasmMb: m ? Number(m[3]) : null });
}

let bad = 0;
const files = ["area.tiles", "area.json"];
const first = runs[0];
for (const r of runs) {
  const same = files.every((f) => existsSync(join(r.d, f)) && Buffer.compare(readFileSync(join(r.d, f)), readFileSync(join(first.d, f))) === 0);
  if (!same) bad++;
  const mem = r.wasmMb != null ? `its memory ${r.wasmMb} MB, the process's peak ${r.peakMb.toFixed(0)} MB` : `peak ${r.peakMb.toFixed(0)} MB`;
  console.log(`${r.name}: ${r.s.toFixed(1)} s, ${mem}: ${same ? "the same bytes" : "DIFFERENT"}`);
}
const sum = JSON.parse(readFileSync(join(first.d, "area.json"), "utf8"));
console.log(`area ${cut.area}: ${sum.buildings} buildings, ${sum.parts} parts, ${sum.tiles.reduce((a, b) => a + b, 0)} tiles, ${(readFileSync(join(first.d, "area.tiles")).length / 2 ** 20).toFixed(1)} MB`);
process.exit(bad ? 1 : 0);
