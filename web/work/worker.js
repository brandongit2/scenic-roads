// A slot of the web worker page (docs/workers.md §7): runs the tasks the page gives it, one at a
// time. A task's programs (the build's WebAssembly builds) run in order over an in-memory
// filesystem holding its inputs; what they write in the unit's folder goes back to the
// coordinator, then the task is done. Its memory is what the coordinator predicted for it: the
// files take theirs, a program gets the rest, and one that needs more fails as out of memory (the
// coordinator then gives it to a worker that spares more).
import { filesystem, run, bytes, changes, Net } from "./runtime.js";

let worker = "";
const modules = new Map();

function headers(extra = {}) {
  return { "X-Worker": worker, ...extra };
}

async function post(path, body) {
  const r = await fetch(path, { method: "POST", headers: headers({ "Content-Type": "application/json" }), body: JSON.stringify(body) });
  if (!r.ok && r.status !== 410) throw new Error(`${path}: ${r.status} ${(await r.text()).slice(0, 200)}`);
  return r.status;
}

async function program(prog, version) {
  const key = `${prog}@${version}`;
  if (!modules.has(key)) {
    const r = await fetch(`/work/prog/${prog}.wasm?v=${encodeURIComponent(version)}`, { headers: headers() });
    if (!r.ok) throw new Error(`${prog}.wasm: ${r.status}`);
    modules.set(key, await WebAssembly.compile(await r.arrayBuffer()));
  }
  return modules.get(key);
}

// A run's arguments and environment with the task's places filled in; null when it names a place
// this worker doesn't have (its environment variable is then left out, as natively).
// A 64-bit hash of a file's bytes (two lanes of MurmurHash3's mixing), to tell an output written
// back unchanged: it isn't sent, the unit's folder has it. (No crypto.subtle: the page needn't be
// a secure context.)
function digest(b) {
  const n = b.byteLength >>> 2;
  const w = b.byteOffset % 4 === 0 ? new Uint32Array(b.buffer, b.byteOffset, n) : new Uint32Array(b.slice(0, n * 4).buffer);
  let h1 = 0x9747b28c ^ b.byteLength, h2 = 0x5bd1e995 ^ Math.floor(b.byteLength / 4294967296);
  const mix = (h, k, c1, c2, r) => {
    k = Math.imul(k, c1);
    k = (k << 15) | (k >>> 17);
    h ^= Math.imul(k, c2);
    h = (h << r) | (h >>> (32 - r));
    return (Math.imul(h, 5) + 0xe6546b64) | 0;
  };
  for (let i = 0; i < n; i++) {
    h1 = mix(h1, w[i], 0xcc9e2d51, 0x1b873593, 13);
    h2 = mix(h2, w[i], 0x85ebca6b, 0xc2b2ae35, 17);
  }
  for (let i = n * 4; i < b.byteLength; i++) {
    h1 = mix(h1, b[i], 0xcc9e2d51, 0x1b873593, 13);
    h2 = mix(h2, b[i] ^ 0xff, 0x85ebca6b, 0xc2b2ae35, 17);
  }
  const fin = (h) => {
    h = Math.imul(h ^ (h >>> 16), 0x85ebca6b);
    h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35);
    return (h ^ (h >>> 16)) >>> 0;
  };
  return `${fin(h1).toString(16)}.${fin(h2).toString(16)}`;
}

function fill(v, places) {
  let out = v;
  for (const [k, p] of Object.entries(places)) if (p !== null) out = out.split(k).join(p);
  return /\{[a-z]+\}/.test(out) ? null : out;
}

async function exec({ lease, task, mem_mb }) {
  const say = (state, frac) => postMessage({ state: `${task.unit}: ${state}`, frac });
  const t0 = performance.now();
  // Its inputs, a few at a time.
  const files = [];
  const sent = new Map();
  let got = 0;
  const total = task.inputs.reduce((s, [, n]) => s + n, 0) || 1;
  const queue = task.inputs.slice();
  await Promise.all(
    Array.from({ length: 4 }, async () => {
      while (queue.length) {
        const [path, size] = queue.shift();
        const r = await fetch(`/work/in/${lease}/${path}`, { headers: headers() });
        // (Not an input of this lease any more: the job took its task back and ended it.)
        if (r.status === 404 || r.status === 410) throw new Error("taken back");
        if (!r.ok) throw new Error(`input ${path}: ${r.status}`);
        const b = new Uint8Array(await r.arrayBuffer());
        if (b.byteLength !== size) throw new Error(`input ${path}: ${b.byteLength} bytes, not ${size}`);
        sent.set(path, digest(b));
        files.push([`/${path}`, b]);
        got += size;
        say(`fetching (${Math.round(got / 1048576)} of ${Math.round(total / 1048576)} MB)`, (0.15 * got) / total);
      }
    }),
  );
  const fetchS = (performance.now() - t0) / 1000;
  const root = filesystem(files);
  files.length = 0;
  // What it reads where it lies (`/net`: the NAS's data and the data servers' files, through the
  // coordinator), when a place names it.
  const net = Object.values(task.places || {}).some((p) => typeof p === "string" && p.startsWith("/net")) ? new Net(`/work/net/${lease}`, headers()) : null;
  let peak = 0;
  const t1 = performance.now();
  for (const [i, r] of task.runs.entries()) {
    say(r.what, 0.15 + (0.75 * i) / task.runs.length);
    const args = [r.prog, ...r.args.map((a) => fill(a, task.places))];
    if (args.includes(null)) throw new Error(`${r.what}: names a place this worker doesn't have`);
    const env = Object.fromEntries(r.env.map(([k, v]) => [k, fill(v, task.places)]).filter(([, v]) => v !== null));
    const held = bytes(root) / 1048576;
    const res = run(await program(r.prog, task.version), args, env, root, mem_mb - held, net);
    peak = Math.max(peak, res.mb + held);
    if (res.oom) {
      const e = new Error(`${r.what} ran out of memory at ${Math.round(res.mb + held)} MB`);
      e.oom = Math.round(res.mb + held);
      throw e;
    }
    if (res.code !== 0) throw new Error(`${r.what} failed (${res.code}): ${res.log.slice(-600)}`);
  }
  const runS = (performance.now() - t1) / 1000;
  // What they wrote in the unit's folder (and any input they removed), back to the coordinator:
  // not what's as it was sent.
  const { written: all, removed } = changes(root, "u", task.inputs.map(([p]) => p));
  const written = all.filter(([path, data]) => !(sent.has(path) && sent.get(path) === digest(data)));
  const outputs = [];
  for (const [i, [path, data]] of written.entries()) {
    say(`sending ${path}`, 0.9 + (0.1 * i) / written.length);
    const r = await fetch(`/work/out/${lease}/${path}`, { method: "PUT", headers: headers({ "Content-Type": "application/octet-stream" }), body: data });
    if (r.status === 410) throw new Error("taken back");
    if (!r.ok) throw new Error(`sending ${path}: ${r.status}`);
    outputs.push({ path, size: data.byteLength });
  }
  const status = await post("/work/done", { worker, lease, outputs, removed, secs: runS, fetch_s: fetchS, peak_mb: Math.round(peak) });
  if (status === 410) throw new Error("taken back");
  return { peak: Math.round(peak), runS };
}

onmessage = async (e) => {
  if (e.data.init) {
    ({ worker } = e.data.init);
    return;
  }
  const job = e.data.run;
  try {
    const r = await exec(job);
    postMessage({ done: job.lease, ok: true, peak_mb: r.peak });
  } catch (err) {
    if (err.message !== "taken back") {
      await post("/work/fail", { worker, lease: job.lease, error: String(err.message).slice(0, 3000), oom_mb: err.oom ?? null }).catch(() => {});
    }
    postMessage({ done: job.lease, ok: false, error: err.message, oom: err.oom ?? null });
  }
};
