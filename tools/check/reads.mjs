// Which files a unit's tail steps open (the lists in pipeline::unit::tail's `reads`, which say what
// a task sends a worker), traced under the web worker page's own runtime (web/work/runtime.js) in
// Node, and whether their outputs are the native run's. Takes a folder the unit-snap command made:
//   node tools/check/reads.mjs <snap dir> <buildings dir> <wasm dir> <SCENIC_OWN> [first step]
// e.g. node tools/check/reads.mjs data/snap/snap data/snap/6-20-22-buildings \
//        target/wasm32-wasip1/release -675000000,450890356,-618750000,489224993 view
import { filesystem, run, bytes, changes } from "../../web/work/runtime.js";
import { OpenDirectory } from "../../web/work/vendor/browser_wasi_shim/fs_mem.js";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const [, , snap, bdir, wasmDir, own, first = "view"] = process.argv;
let opened = new Map();
const open = OpenDirectory.prototype.path_open;
OpenDirectory.prototype.path_open = function (dirflags, path, ...rest) {
  const r = open.call(this, dirflags, path, ...rest);
  if (!opened.has(path)) opened.set(path, r.ret);
  return r;
};
function load(dir, prefix, out) {
  for (const n of readdirSync(dir)) {
    const p = join(dir, n);
    if (statSync(p).isDirectory()) load(p, `${prefix}/${n}`, out);
    else out.push([`${prefix}/${n}`, new Uint8Array(readFileSync(p))]);
  }
  return out;
}
const steps = readdirSync(snap).filter((n) => / before$/.test(n)).sort();
const start = steps.findIndex((n) => n.includes(`scenic ${first}`));
if (start < 0) throw new Error(`no "scenic ${first}" snapshot in ${snap}`);
const files = load(join(snap, steps[start]), "/u", []);
load(bdir, "/b", files);
const root = filesystem(files);
const sizes = new Map(files.map(([p, b]) => [p.slice(1), b.byteLength]));
console.log(`${files.length} files, ${(bytes(root) / 1048576).toFixed(0)} MB`);
const mod = await WebAssembly.compile(readFileSync(join(wasmDir, "scenic-metrics.wasm")));
const env = { SCENIC_CACHE: "/cache", SCENIC_SCACHE: "/u/scache" };
for (const name of steps.slice(start)) {
  const step = name.match(/scenic (\w+)/)?.[1];
  if (!step) continue;
  const args = ["scenic-metrics", "/u", step, ...(step === "buildings" ? ["/b"] : [])];
  opened = new Map();
  const r = run(mod, args, step === "view" ? { ...env, SCENIC_OWN: own } : env, root, 3800);
  const read = [...opened].filter(([p, ret]) => ret === 0 && sizes.has(p)).map(([p]) => p);
  const mb = read.reduce((s, p) => s + sizes.get(p), 0) / 1048576;
  console.log(`${step}: exit ${r.code} in ${(r.ms / 1000).toFixed(1)} s, ${r.mb.toFixed(0)} MB of memory; read ${mb.toFixed(0)} MB: ${read.filter((p) => !p.startsWith("b/")).join(" ")}${read.some((p) => p.startsWith("b/")) ? " b/*" : ""}`);
  if (r.code !== 0) process.exit(1);
}
const last = steps[steps.length - 1].replace(/ before$/, " after");
const { written } = changes(root, "u", [...sizes.keys()]);
const differ = written.filter(([p, d]) => { try { return Buffer.compare(Buffer.from(d), readFileSync(join(snap, last, p.slice(2)))) !== 0; } catch { return true; } });
console.log(`${written.length} files written, ${written.length - differ.length} as natively${differ.length ? `; differ: ${differ.map(([p]) => p).join(" ")}` : ""}`);
process.exit(differ.length ? 1 : 0);
