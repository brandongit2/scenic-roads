// Runs the build's WebAssembly programs (wasm32-wasip1) over an in-memory filesystem, in a web
// worker (docs/workers.md §7). The WASI layer is browser_wasi_shim (vendor/). Here its files grow by
// doubling (a copy per write is quadratic for a file written in small pieces) and know whether a
// program wrote them, and each program runs in a memory capped at what the task may use: one that
// needs more fails cleanly, where an uncapped one could take the whole tab down.
import { WASI, File, OpenFile, PreopenDirectory, Directory } from "./vendor/browser_wasi_shim/index.js";
import { Fd } from "./vendor/browser_wasi_shim/fd.js";

class GrowFile extends File {
  constructor(data, dirty = false) {
    super(new ArrayBuffer(0));
    this.buf = data;
    this.len = data.byteLength;
    this.dirty = dirty;
  }
  get data() {
    return this.buf.subarray(0, this.len);
  }
  set data(d) {
    // (The shim sets it to truncate, resize or replace a file: a change either way.)
    this.buf = d;
    this.len = d.byteLength;
    this.dirty = true;
  }
  get size() {
    return BigInt(this.len);
  }
  ensure(n) {
    if (n <= this.buf.byteLength) return;
    const nb = new Uint8Array(Math.max(n, this.buf.byteLength * 2, 1 << 16));
    nb.set(this.buf.subarray(0, this.len));
    this.buf = nb;
  }
}

const write = OpenFile.prototype.fd_write;
const pwrite = OpenFile.prototype.fd_pwrite;
function put(f, data, at) {
  const end = at + data.byteLength;
  f.ensure(end);
  f.buf.set(data, at);
  if (end > f.len) f.len = end;
  f.dirty = true;
  return end;
}
OpenFile.prototype.fd_write = function (data) {
  if (!(this.file instanceof GrowFile)) return write.call(this, data);
  this.file_pos = BigInt(put(this.file, data, Number(this.file_pos)));
  return { ret: 0, nwritten: data.byteLength };
};
OpenFile.prototype.fd_pwrite = function (data, offset) {
  if (!(this.file instanceof GrowFile)) return pwrite.call(this, data, offset);
  put(this.file, data, Number(offset));
  return { ret: 0, nwritten: data.byteLength };
};
// Files a program creates are GrowFiles too, written from the start.
const create = Directory.prototype.create_entry_for_path;
Directory.prototype.create_entry_for_path = function (path, isDir) {
  const r = create.call(this, path, isDir);
  if (r.entry && !isDir && !(r.entry instanceof GrowFile)) {
    const parts = path.split("/").filter((p) => p && p !== ".");
    let dir = this;
    for (const p of parts.slice(0, -1)) dir = p === ".." ? dir.parent : dir.contents.get(p);
    const g = new GrowFile(new Uint8Array(0), true);
    dir.contents.set(parts[parts.length - 1], g);
    r.entry = g;
  }
  return r;
};

// A program's stdout and stderr: kept (its log, for a failure), the last 64 kB.
class Out extends Fd {
  constructor(sink) {
    super();
    this.sink = sink;
  }
  fd_write(data) {
    this.sink(data);
    return { ret: 0, nwritten: data.byteLength };
  }
  fd_fdstat_get() {
    return { ret: 0, fdstat: { fs_filetype: 2, fs_flags: 0, fs_rights_base: 0n, fs_rights_inherited: 0n, write_bytes(v, p) { v.setUint8(p, 2); v.setUint16(p + 2, 0, true); v.setBigUint64(p + 8, 0n, true); v.setBigUint64(p + 16, 0n, true); } } };
  }
}

// A filesystem from [[path ("/u/x"), Uint8Array]]: its root directory.
export function filesystem(files) {
  const root = new Directory(new Map());
  for (const [path, bytes] of files) {
    const parts = path.split("/").filter(Boolean);
    let d = root;
    for (const p of parts.slice(0, -1)) {
      let c = d.contents.get(p);
      if (!c) {
        c = new Directory(new Map());
        c.parent = d;
        d.contents.set(p, c);
      }
      d = c;
    }
    d.contents.set(parts[parts.length - 1], new GrowFile(bytes));
  }
  return root;
}

const PAGE = 65536;

// Runs `module` with `args` and `env` over `root`, in at most `maxMb` of memory: { code, ms, mb
// (its memory at the end: it only grows), oom, log }.
export function run(module, args, env, root, maxMb) {
  const log = [];
  let logged = 0;
  const sink = (b) => {
    log.push(b.slice());
    logged += b.byteLength;
    while (logged > 65536 && log.length > 1) logged -= log.shift().byteLength;
  };
  const fds = [new OpenFile(new File(new Uint8Array(0))), new Out(sink), new Out(sink), new PreopenDirectory("/", root.contents)];
  const wasi = new WASI(args, Object.entries(env).map(([k, v]) => `${k}=${v}`), fds, { debug: false });
  const imports = { wasi_snapshot_preview1: wasi.wasiImport };
  const wantsMemory = WebAssembly.Module.imports(module).some((i) => i.module === "env" && i.name === "memory" && i.kind === "memory");
  const maximum = Math.max(64, Math.floor(maxMb)) * (1048576 / PAGE);
  let memory = null, inst = null;
  // (The module's own minimum isn't known here: start at 64 MB, more if it asks.)
  for (let initial = 1024; !inst; initial *= 2) {
    if (wantsMemory) {
      memory = new WebAssembly.Memory({ initial: Math.min(initial, maximum), maximum });
      imports.env = { memory };
    }
    try {
      inst = new WebAssembly.Instance(module, imports);
    } catch (e) {
      if (!wantsMemory || !(e instanceof WebAssembly.LinkError) || initial >= maximum) throw e;
    }
  }
  memory = memory || inst.exports.memory;
  const t = performance.now();
  let code = 0;
  try {
    code = wasi.start({ exports: Object.assign({}, inst.exports, { memory }) });
  } catch (e) {
    code = -1;
    sink(new TextEncoder().encode(`\n${e}`));
  }
  const text = new TextDecoder().decode(concat(log));
  const mb = memory.buffer.byteLength / 1048576;
  // Out of memory: the allocator said so, or a trap at the cap.
  const oom = code !== 0 && (/memory allocation of \d+ bytes failed|out of memory/i.test(text) || (wantsMemory && memory.buffer.byteLength + 64 * PAGE >= maximum * PAGE));
  return { code, ms: performance.now() - t, mb, oom, log: text.slice(-4000) };
}

function concat(parts) {
  const n = parts.reduce((s, p) => s + p.byteLength, 0);
  const out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.byteLength;
  }
  return out;
}

// Every file under a directory: [[path relative to it, the file]].
export function walk(dir, prefix = "") {
  const out = [];
  for (const [name, e] of dir.contents) {
    if (e instanceof Directory) out.push(...walk(e, prefix + name + "/"));
    else out.push([prefix + name, e]);
  }
  return out;
}

// The bytes the filesystem holds.
export function bytes(dir) {
  return walk(dir).reduce((s, [, f]) => s + (f instanceof GrowFile ? f.len : f.data.byteLength), 0);
}

// What the programs did under `prefix` ("u"): the files written ([path, Uint8Array]) and the
// paths of inputs `had` that are gone.
export function changes(root, prefix, had) {
  const dir = root.contents.get(prefix);
  const now = dir ? walk(dir, prefix + "/") : [];
  const written = now.filter(([, f]) => f.dirty).map(([p, f]) => [p, f.data]);
  const present = new Set(now.map(([p]) => p));
  const removed = had.filter((p) => p.startsWith(prefix + "/") && !present.has(p));
  return { written, removed };
}
