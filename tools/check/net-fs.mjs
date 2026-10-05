// The worker page's `/net` (web/work/runtime.js: files read where they lie, through the
// coordinator), against a coordinator stand-in: found, read across blocks, listed, a data server's
// missing file as crate::fetch's `.none`, and nothing written.
//   node tools/check/net-fs.mjs
import { Net, NetPreopen } from "../../web/work/runtime.js";
import { wasi } from "../../web/work/vendor/browser_wasi_shim/index.js";
import assert from "node:assert/strict";

const SIZE = 2_500_000;
const byte = (i) => (i * 7 + 3) % 251;
let requests = 0;
function request(url, headers, range) {
  requests++;
  assert.equal(headers.Authorization, "Bearer t");
  const [path, query] = decodeURIComponent(url.slice("/work/net/9/".length)).split("?");
  const json = (v) => ({ status: 200, body: new TextEncoder().encode(JSON.stringify(v)) });
  const files = { "web/data.example/dem/a.tif": SIZE, "nas/sources/canopy/sq1.tif": 10 };
  if (query === "probe") return path in files ? json({ kind: "file", size: files[path] }) : path === "nas/sources/canopy" ? json({ kind: "dir" }) : { status: 404, body: new Uint8Array(0) };
  if (query === "list") return json({ entries: [["sq1.tif", "file", 10], ["more", "dir", 0]] });
  assert.ok(range && path in files, `read ${path}`);
  const body = new Uint8Array(range[1] - range[0]);
  for (let i = 0; i < body.length; i++) body[i] = byte(range[0] + i);
  return { status: 206, body };
}
const net = new Net("/work/net/9", { Authorization: "Bearer t" }, request);
const pre = new NetPreopen(net);
const READ = BigInt(1 << 1);

// A file: its size, then bytes across a block's end, read with seeks and preads.
const f = pre.path_open(0, "web/data.example/dem/a.tif", 0, READ, 0n, 0);
assert.equal(f.ret, 0);
assert.equal(f.fd_obj.fd_filestat_get().filestat.size, BigInt(SIZE));
const want = (o, n) => Uint8Array.from({ length: n }, (_, i) => byte(o + i));
assert.deepEqual(f.fd_obj.fd_pread(4000n, BigInt((1 << 20) - 2000)).data, want((1 << 20) - 2000, 4000));
f.fd_obj.fd_seek(BigInt(SIZE - 10), wasi.WHENCE_SET);
assert.deepEqual(f.fd_obj.fd_read(100n).data, want(SIZE - 10, 10), "a read stops at the end");
const before = requests;
assert.deepEqual(f.fd_obj.fd_pread(10n, 5n).data, want(5, 10));
assert.equal(requests, before, "a block read once is kept");

// A data server's file it doesn't have: `<path>.none` is there, `.ranges` never is.
assert.equal(pre.path_filestat_get(0, "web/data.example/dem/missing.tif").ret, wasi.ERRNO_NOENT);
assert.equal(pre.path_filestat_get(0, "web/data.example/dem/missing.tif.none").ret, 0);
assert.equal(pre.path_filestat_get(0, "web/data.example/dem/a.tif.none").ret, wasi.ERRNO_NOENT);
assert.equal(pre.path_filestat_get(0, "web/data.example/dem/a.tif.ranges").ret, wasi.ERRNO_NOENT);

// A NAS folder, listed as a program reads it.
const d = pre.path_open(0, "nas/sources/canopy", wasi.OFLAGS_DIRECTORY, READ, 0n, 0);
assert.equal(d.ret, 0);
const names = [];
for (let c = 2n; ; c++) {
  const r = d.fd_obj.fd_readdir_single(c);
  if (!r.dirent) break;
  names.push(new TextDecoder().decode(r.dirent.dir_name));
}
assert.deepEqual(names, ["sq1.tif", "more"]);

// Nothing written: not a new file, nor an old one opened to write.
assert.equal(pre.path_open(0, "nas/sources/canopy/new.tif", wasi.OFLAGS_CREAT, READ, 0n, 0).ret, wasi.ERRNO_ROFS);
assert.equal(pre.path_open(0, "nas/sources/canopy/sq1.tif", 0, BigInt(wasi.RIGHTS_FD_WRITE), 0n, 0).ret, wasi.ERRNO_ROFS);
console.log(`net-fs: ok (${requests} requests)`);
