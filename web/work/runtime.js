// Runs the build's WebAssembly programs (wasm32-wasip1) over an in-memory filesystem, in a web
// worker (docs/workers.md §7). The WASI layer is browser_wasi_shim (vendor/). Here its files grow by
// doubling (a copy per write is quadratic for a file written in small pieces) and know whether a
// program wrote them, and each program runs in a memory capped at what the task may use: one that
// needs more fails cleanly, where an uncapped one could take the whole tab down.
import { WASI, File, OpenFile, OpenDirectory, PreopenDirectory, Directory, wasi } from "./vendor/browser_wasi_shim/index.js";
import { Fd, Inode } from "./vendor/browser_wasi_shim/fd.js";

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

// Files read where they lie (docs/workers.md §3, Read where they lie): a task's `/net`, the build's
// data on the NAS (`nas/<path>`) and the outside data servers' files (`web/<host>/<path>`, laid out
// as crate::fetch's mirror folders are), read through the coordinator a block at a time as a
// program reads them: a program opens them as files, so what it reads isn't sent ahead of it, nor
// known ahead. Read-only. Synchronous requests (a Web Worker may make them): a WASI call can't wait.
const BLOCK = 1 << 20;
// (The blocks kept, the most recently read: up to this many bytes.)
const KEEP = 64 << 20;

export class Net {
  // `base`: where the coordinator answers for the task ("/work/net/<lease>"); `headers`: its token.
  constructor(base, headers, request = xhr) {
    this.base = base;
    this.headers = headers;
    this.request = request;
    this.probes = new Map();
    this.blocks = new Map();
    this.held = 0;
    this.got = 0;
  }
  url(path, query) {
    return `${this.base}/${path.split("/").map(encodeURIComponent).join("/")}${query ? `?${query}` : ""}`;
  }
  // What's at `path`: { kind: "file", size } or { kind: "dir" }; null when nothing is.
  probe(path) {
    if (!this.probes.has(path)) {
      const r = this.request(this.url(path, "probe"), this.headers, null);
      if (r.status === 404) this.probes.set(path, null);
      else if (r.status === 200) this.probes.set(path, JSON.parse(new TextDecoder().decode(r.body)));
      else throw new Error(`${path}: ${r.status}`);
    }
    return this.probes.get(path);
  }
  // A folder's entries: [[name, "file" | "dir", size]].
  list(path) {
    const r = this.request(this.url(path, "list"), this.headers, null);
    if (r.status !== 200) throw new Error(`${path}: ${r.status}`);
    return JSON.parse(new TextDecoder().decode(r.body)).entries;
  }
  // `len` bytes of `path` (`size` bytes long) from `off`.
  read(path, size, off, len) {
    const out = new Uint8Array(len);
    for (let o = 0; o < len; ) {
      const at = off + o, b = Math.floor(at / BLOCK), key = `${b} ${path}`;
      let data = this.blocks.get(key);
      if (data) {
        this.blocks.delete(key);
      } else {
        const s = b * BLOCK, e = Math.min(size, s + BLOCK);
        const r = this.request(this.url(path), this.headers, [s, e]);
        if ((r.status !== 206 && r.status !== 200) || r.body.byteLength !== e - s) throw new Error(`${path}: bytes ${s}-${e}: ${r.status}, ${r.body.byteLength} bytes`);
        data = r.body;
        this.held += data.byteLength;
        this.got += data.byteLength;
        for (const [k, v] of this.blocks) {
          if (this.held <= KEEP) break;
          this.blocks.delete(k);
          this.held -= v.byteLength;
        }
      }
      this.blocks.set(key, data);
      const within = at - b * BLOCK, n = Math.min(len - o, data.byteLength - within);
      out.set(data.subarray(within, within + n), o);
      o += n;
    }
    return out;
  }
}

// A synchronous request: { status, body (Uint8Array) }.
function xhr(url, headers, range) {
  const x = new XMLHttpRequest();
  x.open("GET", url, false);
  x.responseType = "arraybuffer";
  for (const [k, v] of Object.entries(headers)) x.setRequestHeader(k, v);
  if (range) x.setRequestHeader("Range", `bytes=${range[0]}-${range[1] - 1}`);
  x.send();
  return { status: x.status, body: new Uint8Array(x.response || new ArrayBuffer(0)) };
}

const join = (a, b) => (a ? `${a}/${b}` : b);

class NetFile extends Inode {
  constructor(net, path, size) {
    super();
    this.net = net;
    this.path = path;
    this.len = size;
  }
  path_open(oflags, rights) {
    if (oflags & (wasi.OFLAGS_CREAT | wasi.OFLAGS_TRUNC) || BigInt(rights) & BigInt(wasi.RIGHTS_FD_WRITE)) return { ret: wasi.ERRNO_ROFS, fd_obj: null };
    return { ret: wasi.ERRNO_SUCCESS, fd_obj: new NetOpen(this) };
  }
  stat() {
    return new wasi.Filestat(this.ino, wasi.FILETYPE_REGULAR_FILE, BigInt(this.len));
  }
}

class NetOpen extends Fd {
  constructor(file) {
    super();
    this.file = file;
    this.pos = 0n;
  }
  bytes(size, offset) {
    const off = Number(offset), n = Math.max(0, Math.min(Number(size), this.file.len - off));
    return n ? this.file.net.read(this.file.path, this.file.len, off, n) : new Uint8Array(0);
  }
  fd_fdstat_get() {
    return { ret: 0, fdstat: new wasi.Fdstat(wasi.FILETYPE_REGULAR_FILE, 0) };
  }
  fd_filestat_get() {
    return { ret: 0, filestat: this.file.stat() };
  }
  fd_read(size) {
    try {
      const data = this.bytes(size, this.pos);
      this.pos += BigInt(data.byteLength);
      return { ret: 0, data };
    } catch {
      return { ret: wasi.ERRNO_IO, data: new Uint8Array(0) };
    }
  }
  fd_pread(size, offset) {
    try {
      return { ret: 0, data: this.bytes(size, offset) };
    } catch {
      return { ret: wasi.ERRNO_IO, data: new Uint8Array(0) };
    }
  }
  fd_seek(offset, whence) {
    const to = whence === wasi.WHENCE_SET ? offset : whence === wasi.WHENCE_CUR ? this.pos + offset : whence === wasi.WHENCE_END ? BigInt(this.file.len) + offset : -1n;
    if (to < 0n) return { ret: wasi.ERRNO_INVAL, offset: 0n };
    this.pos = to;
    return { ret: 0, offset: to };
  }
  fd_tell() {
    return { ret: 0, offset: this.pos };
  }
}

// A folder of `/net`: what's under it found as a program asks for it (a path at a time, one request
// each, the answer kept); listed when a program reads the folder. A data server's file it doesn't
// have is crate::fetch's `<path>.none`; there are no `.ranges` (every file's bytes are there).
class NetDir extends Directory {
  constructor(net, path) {
    super(new Map());
    this.net = net;
    this.path = path;
    this.listed = false;
  }
  get contents() {
    if (this.net && !this.listed) {
      this.listed = true;
      for (const [name, kind, size] of this.net.list(this.path)) this.held.set(name, kind === "dir" ? new NetDir(this.net, join(this.path, name)) : new NetFile(this.net, join(this.path, name), size));
    }
    return this.held;
  }
  set contents(v) {
    this.held = v;
  }
  get_entry_for_path(path) {
    if (!path.parts.length) return { ret: wasi.ERRNO_SUCCESS, entry: this };
    const rel = join(this.path, path.parts.join("/"));
    try {
      if (rel.startsWith("web/") && rel.endsWith(".ranges")) return { ret: wasi.ERRNO_NOENT, entry: null };
      if (rel.startsWith("web/") && rel.endsWith(".none")) {
        return this.net.probe(rel.slice(0, -5)) ? { ret: wasi.ERRNO_NOENT, entry: null } : { ret: wasi.ERRNO_SUCCESS, entry: new File(new ArrayBuffer(0), { readonly: true }) };
      }
      const p = this.net.probe(rel);
      if (!p) return { ret: wasi.ERRNO_NOENT, entry: null };
      if (p.kind === "dir") return { ret: wasi.ERRNO_SUCCESS, entry: new NetDir(this.net, rel) };
      if (path.is_dir) return { ret: wasi.ERRNO_NOTDIR, entry: null };
      return { ret: wasi.ERRNO_SUCCESS, entry: new NetFile(this.net, rel, p.size) };
    } catch {
      return { ret: wasi.ERRNO_IO, entry: null };
    }
  }
  get_parent_dir_and_entry_for_path() {
    return { ret: wasi.ERRNO_ROFS, parent_entry: null, filename: null, entry: null };
  }
  create_entry_for_path() {
    return { ret: wasi.ERRNO_ROFS, entry: null };
  }
}

export class NetPreopen extends OpenDirectory {
  constructor(net) {
    super(new NetDir(net, ""));
  }
  fd_prestat_get() {
    return { ret: 0, prestat: wasi.Prestat.dir("/net") };
  }
}

const PAGE = 65536;

// Runs `module` with `args` and `env` over `root` (and `/net`, with `net`: what's read where it
// lies), in at most `maxMb` of memory: { code, ms, mb (its memory at the end: it only grows), oom,
// log }.
export function run(module, args, env, root, maxMb, net = null) {
  const log = [];
  let logged = 0;
  const sink = (b) => {
    log.push(b.slice());
    logged += b.byteLength;
    while (logged > 65536 && log.length > 1) logged -= log.shift().byteLength;
  };
  const fds = [new OpenFile(new File(new Uint8Array(0))), new Out(sink), new Out(sink), new PreopenDirectory("/", root.contents)];
  if (net) fds.push(new NetPreopen(net));
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
