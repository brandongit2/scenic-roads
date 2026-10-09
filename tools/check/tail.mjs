// A unit's tail as a browser's task runs it, from its elevations on: the task's own steps and places
// (scenic-build tail-spec), run under the web worker page's runtime (web/work/runtime.js) in Node,
// with `/net` answered as the coordinator answers it (crate::coord's `net`): the NAS's files where
// they lie, and the data servers' from a mirror folder the native run recorded (SCENIC_FETCH_RECORD)
// or, for bytes it lacks, the network (curl, with the build's User-Agent). Each step's reads of the
// unit's folder are traced (the lists in pipeline::unit::tail's `reads`: what a task sends a worker),
// and what the steps wrote is compared with the native run's. Takes a folder unit-snap made:
//   node tools/check/tail.mjs <unit> <snap dir> <buildings dir> <wasm dir> <NAS root> <mirror dir> [first step]
// With ONLY_LISTED=1 the steps get only the files their lists name, as a task's worker does: the
// outputs still the native run's says nothing they look for (without opening it) is missing.
// e.g. node tools/check/tail.mjs 6/20/22 data/snap/snap data/snap/6-20-22-buildings \
//        target/wasm32-wasip1/release /Volumes/personal/projects/scenic-roads data/snap/mirror
import { filesystem, run, bytes, changes, walk, Net } from "../../web/work/runtime.js";
import { OpenDirectory } from "../../web/work/vendor/browser_wasi_shim/fs_mem.js";
import { closeSync, existsSync, openSync, readFileSync, readSync, readdirSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";

const [, , unit, snap, bdir, wasmDir, nas, mirror, first = "elevations (elev)"] = process.argv;
const spec = JSON.parse(execFileSync("target/release/scenic-build", ["tail-spec", unit], { encoding: "utf8" }));
// What crate::coord's `net` serves: the NAS's paths a task reads, and the data servers.
const NAS_PATHS = ["sources/"];
const WEB_HOSTS = spec.web_hosts;
const UA = "scenic-roads/0.1 (personal offline map)";

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

// `/net`, as the coordinator answers it.
const json = (v) => ({ status: 200, body: new TextEncoder().encode(JSON.stringify(v)) });
const none = { status: 404, body: new Uint8Array(0) };
const counts = { nas: 0, mirror: 0, network: 0, bytes: 0 };
function readAt(file, s, e) {
  const fd = openSync(file, "r");
  try {
    const b = Buffer.alloc(e - s);
    let o = 0;
    while (o < b.length) {
      const n = readSync(fd, b, o, b.length - o, s + o);
      if (n === 0) break;
      o += n;
    }
    return new Uint8Array(b.buffer, b.byteOffset, o);
  } finally {
    closeSync(fd);
  }
}
const ranges = (p) => (existsSync(`${p}.ranges`) ? readFileSync(`${p}.ranges`, "utf8").trim().split("\n").map((l) => l.split(" ").map(Number)) : null);
const covered = (rs, s, e) => {
  // (Recorded ranges merged, then whether one holds [s, e).)
  const m = [];
  for (const [a, b] of [...rs].sort((x, y) => x[0] - y[0])) {
    if (m.length && a <= m[m.length - 1][1]) m[m.length - 1][1] = Math.max(m[m.length - 1][1], b);
    else m.push([a, b]);
  }
  return m.some(([a, b]) => a <= s && b >= e);
};
function curl(url, args) {
  counts.network++;
  return execFileSync("curl", ["-sS", "--fail-with-body", "-A", UA, "--retry", "3", ...args, url], { maxBuffer: 1 << 28 });
}
const lengths = new Map();
function remoteLength(url) {
  if (!lengths.has(url)) {
    // A ranged request for its first byte: its Content-Range tells the length (S3's and Bristol's
    // servers answer one); 403/404/410: none.
    let head;
    try {
      head = execFileSync("curl", ["-sS", "-A", UA, "-r", "0-0", "-o", "/dev/null", "-D", "-", "-w", "%{http_code}", url], { encoding: "utf8" });
    } catch (e) {
      throw new Error(`${url}: ${e.message}`);
    }
    counts.network++;
    const code = Number(head.trim().split("\n").pop());
    const m = head.match(/content-range:\s*bytes \d+-\d+\/(\d+)/i);
    lengths.set(url, [403, 404, 410].includes(code) ? null : m ? Number(m[1]) : Number(head.match(/content-length:\s*(\d+)/i)?.[1]));
  }
  return lengths.get(url);
}
function request(url, _headers, range) {
  const [path, query] = decodeURIComponent(url.slice("/work/net/1/".length)).split("?");
  if (path.startsWith("nas/")) {
    const rel = path.slice(4);
    if (!NAS_PATHS.some((n) => rel.startsWith(n) || rel === n.replace(/\/$/, ""))) return { status: 403, body: new Uint8Array(0) };
    const p = join(nas, rel);
    let st;
    try {
      st = statSync(p);
    } catch {
      return none;
    }
    counts.nas++;
    if (query === "probe") return json(st.isDirectory() ? { kind: "dir" } : { kind: "file", size: st.size });
    if (query === "list") return json({ entries: readdirSync(p).map((n) => { const s = statSync(join(p, n)); return [n, s.isDirectory() ? "dir" : "file", s.size]; }).sort() });
    const body = readAt(p, range[0], range[1]);
    counts.bytes += body.byteLength;
    return { status: 206, body };
  }
  const m = path.match(/^web\/([^/]+)\/(.+)$/);
  if (!m) return none;
  if (!WEB_HOSTS.includes(m[1])) return { status: 403, body: new Uint8Array(0) };
  const u = `https://${m[1]}/${m[2]}`;
  const local = join(mirror, m[1], m[2]);
  if (existsSync(`${local}.none`)) return none;
  const have = existsSync(local) ? ranges(local) : undefined;
  const size = have !== undefined ? statSync(local).size : remoteLength(u);
  if (size === null) return none;
  if (query === "probe") return json({ kind: "file", size });
  const [s, e] = range;
  let body;
  if (have === null || (have && covered(have, s, e))) {
    counts.mirror++;
    body = readAt(local, s, e);
  } else {
    body = new Uint8Array(curl(u, ["-r", `${s}-${e - 1}`]));
  }
  counts.bytes += body.byteLength;
  return { status: 206, body };
}

const steps = readdirSync(snap).filter((n) => / before$/.test(n)).sort();
const start = steps.findIndex((n) => n.slice(3) === `${first} before`);
if (start < 0) throw new Error(`no "${first}" snapshot in ${snap}`);
let files = load(join(snap, steps[start]), "/u", []);
load(bdir, "/b", files);
if (process.env.ONLY_LISTED) {
  // (As crate::offload's `matching`: a name, or `<folder>/<start>*`.)
  const pats = spec.runs.slice(spec.runs.findIndex((r) => r.what === first)).flatMap((r) => r.reads || []).map((p) => p.replace("{dir}/", "/u/").replace("{buildings}/", "/b/"));
  const listed = (p) => pats.some((pat) => (pat.endsWith("*") ? p.startsWith(pat.slice(0, -1)) && !p.slice(pat.length - 1).includes("/") : p === pat));
  const all = files.length;
  files = files.filter(([p]) => listed(p) && !p.endsWith(".tmp"));
  console.log(`only the listed files: ${files.length} of ${all}`);
}
const root = filesystem(files);
const sizes = new Map(files.map(([p, b]) => [p.slice(1), b.byteLength]));
console.log(`${files.length} files, ${(bytes(root) / 1048576).toFixed(0)} MB`);
const fill = (v) => {
  let out = v;
  for (const [k, p] of Object.entries(spec.places)) if (p !== null) out = out.split(k).join(p);
  return /\{[a-z]+\}/.test(out) ? null : out;
};
const firstRun = spec.runs.findIndex((r) => r.what === first);
const modules = new Map();
let failed = false;
for (const r of spec.runs.slice(firstRun)) {
  if (!steps.some((n) => n.slice(3) === `${r.what} before`)) {
    console.log(`${r.what}: not in the snapshot (passed over)`);
    continue;
  }
  if (!modules.has(r.prog)) modules.set(r.prog, await WebAssembly.compile(readFileSync(join(wasmDir, `${r.prog}.wasm`))));
  const args = [r.prog, ...r.args.map(fill)];
  const env = Object.fromEntries(r.env.map(([k, v]) => [k, fill(v)]).filter(([, v]) => v !== null));
  const net = new Net("/work/net/1", {}, request);
  // (What's there as the step starts: a file it opens is one it reads, unless it made it; `+`, one
  // an earlier step of the task made.)
  const present = new Map(walk(root).map(([p, f]) => [p, Number(f.size ?? f.data.byteLength)]));
  opened = new Map();
  const before = { ...counts };
  const res = run(modules.get(r.prog), args, env, root, 3800, net);
  const read = [...opened].filter(([p, ret]) => ret === 0 && present.has(p)).map(([p]) => p);
  const mb = read.reduce((s, p) => s + present.get(p), 0) / 1048576;
  const listed = (r.reads || []).length ? ` (listed: ${r.reads.join(" ")})` : " (none listed)";
  const unlisted = read.filter((p) => p.startsWith("u/") && !(r.reads || []).some((pat) => {
    const rel = pat.replace("{dir}/", "u/");
    return rel.endsWith("*") ? p.startsWith(rel.slice(0, -1)) : p === rel;
  }));
  const name = (p) => (sizes.has(p) ? p : `+${p}`);
  console.log(`${r.what}: exit ${res.code} in ${(res.ms / 1000).toFixed(1)} s, ${res.mb.toFixed(0)} MB of memory; read ${mb.toFixed(1)} MB: ${read.filter((p) => !p.startsWith("b/")).map(name).join(" ")}${read.some((p) => p.startsWith("b/")) ? " b/*" : ""}${listed}`);
  if (unlisted.length) console.log(`  NOT LISTED: ${unlisted.join(" ")}`);
  console.log(`  /net: ${counts.nas - before.nas} NAS requests, ${counts.mirror - before.mirror} from the mirror, ${counts.network - before.network} over the network, ${((counts.bytes - before.bytes) / 1048576).toFixed(1)} MB`);
  if (res.code !== 0) {
    console.log(res.log);
    failed = true;
    break;
  }
}
const last = steps[steps.length - 1].replace(/ before$/, " after");
const { written } = changes(root, "u", [...sizes.keys()]);
// (dem-stats.json says how long its run took: the rest of it is compared.)
const timeless = (p, b) => (p.endsWith("dem-stats.json") ? Buffer.from(JSON.stringify({ ...JSON.parse(Buffer.from(b).toString()), seconds: 0 })) : Buffer.from(b));
const differ = written.filter(([p, d]) => {
  try {
    return Buffer.compare(timeless(p, d), timeless(p, readFileSync(join(snap, last, p.slice(2))))) !== 0;
  } catch {
    return true;
  }
});
console.log(`${written.length} files written, ${written.length - differ.length} as natively${differ.length ? `; differ: ${differ.map(([p]) => p).join(" ")}` : ""}`);
process.exit(failed || differ.length ? 1 : 0);
