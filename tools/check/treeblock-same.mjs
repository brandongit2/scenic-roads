// A row of a tree cover piece's z8 blocks as a page's task (docs/plan.md §6 Trees, docs/workers.md
// §3): its task folder cut from the NAS's coverage (`scenic-build treeblock-task`, as a piece's run
// cuts it), then the `trees` program's `--blocks` run natively (on each thread count given), each
// block alone natively (`--block`), as WebAssembly under Node's WASI (tools/check/wasi-run.mjs),
// and as WebAssembly under the page's own runtime (web/work/runtime.js: its in-memory files, its
// memory cap, the squares read through its `/net`, here served from the NAS's files, a range at a
// time). Every run's blocks the same bytes; their tiles the same as the NAS's packs and mid have
// them. Each run's time and memory said, and the bytes the page's run read. Exits 1 on any
// difference.
//
//   node tools/check/treeblock-same.mjs --root <NAS project> --piece 6/x/y --row y --work <dir>
//        [--bin target/release] [--wasm target/wasm32-wasip1/release] [--threads 1,0]
//        [--pass d] [--regions dir]
//
// Only --work is written.
import { execFileSync, spawnSync } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { filesystem, run, Net, walk } from "../../web/work/runtime.js";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const args = process.argv.slice(2);
const arg = (k, d) => (args.includes(k) ? args[args.indexOf(k) + 1] : d);
const root = arg("--root") && resolve(arg("--root")), piece = arg("--piece"), rowY = arg("--row"), work = arg("--work") && resolve(arg("--work"));
if (!root || !piece || !rowY || !work) {
  console.error("usage: treeblock-same.mjs --root <dir> --piece 6/x/y --row y --work <dir> [--bin dir] [--wasm dir] [--threads 1,0] [--pass d] [--regions dir]");
  process.exit(2);
}
const bin = resolve(arg("--bin", join(repo, "target/release")));
const wasm = resolve(arg("--wasm", join(repo, "target/wasm32-wasip1/release")));
const threads = arg("--threads", "1,0").split(",").map(Number);
const LAYERS = ["cover", "height", "leaf"];
const FILES = [...LAYERS.map((l) => `trees-${l}.tiles`), "trees-tops.bin"];

// A command under /usr/bin/time -l: its wall time, peak memory and stderr.
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

// An archive's tiles (roadcore's RDTILES: its index at the offset in its header, 24 bytes an entry):
// "z/x/y" → bytes.
function tiles(p) {
  const b = readFileSync(p);
  const off = Number(b.readBigUInt64LE(8)), n = Number(b.readBigUInt64LE(16));
  const out = new Map();
  for (let i = 0; i < n; i++) {
    const e = off + 24 * i;
    const k = b.readBigUInt64LE(e), o = Number(b.readBigUInt64LE(e + 8)), len = b.readUInt32LE(e + 16);
    const z = Number(k >> 58n), x = Number((k >> 29n) & ((1n << 29n) - 1n)), y = Number(k & ((1n << 29n) - 1n));
    out.set(`${z}/${x}/${y}`, b.subarray(o, o + len));
  }
  return out;
}

rmSync(work, { recursive: true, force: true });
mkdirSync(work, { recursive: true });
const extra = ["--pass", "--regions"].flatMap((k) => (arg(k) ? [k, arg(k)] : []));
const cutOut = execFileSync(join(bin, "scenic-build"), ["treeblock-task", piece, "--row", rowY, "--root", root, "--scratch", join(work, "scratch"), "--out", work, ...extra], { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
const cut = JSON.parse(cutOut.trim().split("\n").pop());
const blocks = cut.blocks.split(",");
console.log(`row ${cut.blocks} of ${cut.piece}: squares ${cut.squares || "none"}, its coverage ${(cut.coverage_bytes / 1024).toFixed(0)} kB`);
const cov = join(work, "task/u/coverage.json");
const chm = join(root, "sources/canopy"), leaf = join(root, "sources/trees/leaf");
const common = (out) => ["--blocks", cut.blocks, "--coverage", cov, "--chm", chm, "--leaf", leaf, "--squares", cut.squares, "--out", out];
const dirOf = (out, b) => join(out, b.replaceAll("/", "-"));

const runs = [];
for (const n of threads) {
  const d = join(work, `native-${n || "all"}`);
  const r = timed(join(bin, "trees"), [...common(d), ...(n ? ["--workers", String(n)] : [])]);
  runs.push({ name: `native, ${n || "all"} thread${n === 1 ? "" : "s"}`, d, ...r });
}
{
  // Each block alone (`--block`), as the piece's run makes the blocks it keeps.
  const d = join(work, "alone");
  let s = 0, peak = 0;
  for (const b of blocks) {
    const r = timed(join(bin, "trees"), ["--block", b, "--coverage", cov, "--chm", chm, "--leaf", leaf, "--out", dirOf(d, b)]);
    s += r.s;
    peak = Math.max(peak, r.peakMb);
  }
  runs.push({ name: "native, each block alone", d, s, peakMb: peak });
}
{
  const d = join(work, "wasi");
  const r = timed(process.execPath, [join(here, "wasi-run.mjs"), join(wasm, "trees.wasm"), "{}", ...common(d)]);
  const m = r.err.match(/run (\d+) exit (\d+) mem (\d+)/);
  runs.push({ name: "WebAssembly (Node's WASI)", d, ...r, s: m ? Number(m[1]) / 1000 : r.s, wasmMb: m ? Number(m[3]) : null });
}
{
  // As a page runs it: the task's folder in memory, the squares through `/net` a 1 MB block at a
  // time (here from the NAS's files, as the coordinator serves them), its memory capped.
  const fds = new Map();
  const request = (url, _headers, range) => {
    const [path, query] = decodeURIComponent(url.slice("/work/net/1/".length)).split("?");
    const json = (v) => ({ status: 200, body: new TextEncoder().encode(JSON.stringify(v)) });
    if (!path.startsWith("nas/sources/")) return { status: 403, body: new Uint8Array(0) };
    const p = join(root, path.slice(4));
    if (query === "probe") {
      if (!existsSync(p)) return { status: 404, body: new Uint8Array(0) };
      const st = statSync(p);
      return json(st.isDirectory() ? { kind: "dir" } : { kind: "file", size: st.size });
    }
    if (query === "list") return json({ entries: readdirSync(p).sort().map((n) => [n, statSync(join(p, n)).isDirectory() ? "dir" : "file", statSync(join(p, n)).size]) });
    if (!fds.has(p)) fds.set(p, openSync(p, "r"));
    const body = new Uint8Array(range[1] - range[0]);
    readSync(fds.get(p), body, 0, body.length, range[0]);
    return { status: 206, body };
  };
  const net = new Net("/work/net/1", {}, request);
  const fs = filesystem([["/u/coverage.json", new Uint8Array(readFileSync(cov))]]);
  const module = await WebAssembly.compile(readFileSync(join(wasm, "trees.wasm")));
  const page = ["--blocks", cut.blocks, "--coverage", "/u/coverage.json", "--chm", "/net/nas/sources/canopy", "--leaf", "/net/nas/sources/trees/leaf", "--squares", cut.squares, "--out", "/u"];
  const cap = Number(arg("--page-mb", "3000"));
  const res = run(module, ["trees", ...page], {}, fs, cap, net);
  for (const f of fds.values()) closeSync(f);
  if (res.code !== 0) {
    console.error(res.log);
    throw new Error(`the page's run failed (${res.code}${res.oom ? ", out of memory" : ""})`);
  }
  const d = join(work, "page");
  let written = 0;
  for (const [path, f] of walk(fs.contents.get("u"), "")) {
    if (path === "coverage.json") continue;
    mkdirSync(dirname(join(d, path)), { recursive: true });
    writeFileSync(join(d, path), f.data);
    written += f.data.byteLength;
  }
  runs.push({ name: "WebAssembly (the page's runtime)", d, s: res.ms / 1000, wasmMb: res.mb, netMb: net.got / 2 ** 20, writtenMb: written / 2 ** 20 });
}

let bad = 0;
const first = runs[0];
for (const r of runs) {
  const same = blocks.every((b) => FILES.every((f) => existsSync(join(dirOf(r.d, b), f)) && Buffer.compare(readFileSync(join(dirOf(r.d, b), f)), readFileSync(join(dirOf(first.d, b), f))) === 0));
  if (!same) bad++;
  const mem = r.wasmMb != null ? `its memory ${r.wasmMb.toFixed(0)} MB${r.peakMb ? `, the process's peak ${r.peakMb.toFixed(0)} MB` : ""}` : `peak ${r.peakMb.toFixed(0)} MB`;
  const more = r.netMb != null ? `, ${r.netMb.toFixed(0)} MB read through /net, ${r.writtenMb.toFixed(0)} MB written` : "";
  console.log(`${r.name}: ${r.s.toFixed(1)} s, ${mem}${more}: ${same ? "the same bytes" : "DIFFERENT"}`);
}
// The NAS's packs and mid: the same tiles and values.
let n = 0;
for (const b of blocks) {
  for (const l of LAYERS) {
    const mine = tiles(join(dirOf(first.d, b), `trees-${l}.tiles`)), nas = tiles(join(dirOf(join(work, "packs"), b), `trees-${l}.tiles`));
    n += mine.size;
    const same = mine.size === nas.size && [...mine].every(([k, v]) => nas.has(k) && Buffer.compare(v, nas.get(k)) === 0);
    if (!same) {
      bad++;
      console.log(`${b} ${l}: ${mine.size} tiles made, the NAS has ${nas.size}: DIFFERENT`);
    }
  }
  if (Buffer.compare(readFileSync(join(dirOf(first.d, b), "trees-tops.bin")), readFileSync(join(dirOf(join(work, "packs"), b), "trees-tops.bin"))) !== 0) {
    bad++;
    console.log(`${b}: its zoom-8 values differ from the NAS's mid`);
  }
}
console.log(`${blocks.length} blocks, ${n} tiles: ${bad ? "DIFFERENCES" : "all the same as the NAS's packs and mid"}`);
process.exit(bad ? 1 : 0);
