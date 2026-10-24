// A test server (task #102) for the buildings out of a build root's packs, with CORS: the field's
// tiles (`field3-webp`, layers/buildings-field/{hi,lo}/) at /field/{z}/{x}/{y} and the meshes
// (layers/buildings/hi/) at /bld/{z}/{x}/{y}, as the server answers them (204 where a pack has no
// such tile).
//   node serve.mjs <port> <root> [mesh root]
// (mesh root: the meshes' hi packs read from another root's manifest instead, read only: the
// project folder's, whose z14 tiles are the same bytes)
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const [port, root, meshRoot] = process.argv.slice(2);
// (A root's manifest as the build's readers have it: the current term's snapshot, else the newest of a
// term before it, else, before the pool, state/build/manifest.json; docs/pool.md §6.2.)
const manifest = (r) => {
  const b = path.join(r, 'state/build');
  const json = (p) => JSON.parse(fs.readFileSync(path.join(b, p), 'utf8'));
  let e = 0;
  try { e = json('lead.json').term; } catch { e = Math.max(0, ...(fs.existsSync(path.join(b, 'terms')) ? fs.readdirSync(path.join(b, 'terms')) : []).map((n) => parseInt(n, 10) || 0)); }
  while (fs.existsSync(path.join(b, `terms/${e + 1}.json`))) e++;
  for (let t = e; t >= 1; t--) {
    for (const f of [`term/${t}/records.json`, ...(t === 1 ? ['term/1/first.json'] : [])]) {
      if (fs.existsSync(path.join(b, f))) return json(f).manifest;
    }
  }
  return json('manifest.json');
};
const read = (r) => Object.entries(manifest(r)).map(([l, c]) => [l, path.join(r, c)]);
const entries = meshRoot ? [...read(root).filter(([l]) => !l.startsWith('layers/buildings/')), ...read(meshRoot).filter(([l]) => l.startsWith('layers/buildings/'))] : read(root);
/** layer → "scope z/x/y" → { fd, entries, gzip } */
const packs = new Map();
for (const [logical, file] of entries) {
  const m = logical.match(/^layers\/(buildings(?:-field)?)\/(hi|lo)\/(\d+)-(\d+)-(\d+)$/);
  if (!m) continue;
  if (!fs.existsSync(file)) continue;
  const fd = fs.openSync(file, 'r');
  const head = Buffer.alloc(36);
  fs.readSync(fd, head, 0, 36, 0);
  const flags = head.readUInt32LE(12);
  const idxOff = Number(head.readBigUInt64LE(16)), n = Number(head.readBigUInt64LE(24));
  const idx = Buffer.alloc(n * 32);
  fs.readSync(fd, idx, 0, n * 32, idxOff);
  const e = new Map();
  for (let i = 0; i < n; i++) e.set(idx.readBigUInt64LE(i * 32), [Number(idx.readBigUInt64LE(i * 32 + 8)), idx.readUInt32LE(i * 32 + 16)]);
  if (!packs.has(m[1])) packs.set(m[1], new Map());
  packs.get(m[1]).set(`${m[2]} ${m[3]}/${m[4]}/${m[5]}`, { fd, e, gzip: (flags & 1) !== 0 });
}
const key = (z, x, y) => (BigInt(z) << 58n) | (BigInt(x) << 29n) | BigInt(y);
let served = 0, bytes = 0;
http.createServer((req, res) => {
  res.setHeader('Access-Control-Allow-Origin', '*');
  const u = new URL(req.url, 'http://x');
  const m = u.pathname.match(/^\/(field|bld)\/(\d+)\/(\d+)\/(\d+)$/);
  if (u.pathname === '/stats') return res.end(JSON.stringify({ served, bytes }));
  if (!m) {
    res.statusCode = 404;
    return res.end();
  }
  const [z, x, y] = m.slice(2).map(Number);
  const layer = packs.get(m[1] === 'field' ? 'buildings-field' : 'buildings');
  // (As the server: zooms 3–8 from a z3 tile's lo pack, deeper from a z6 tile's hi pack.)
  const pk = z <= 8 ? layer?.get(`lo 3/${x >> (z - 3)}/${y >> (z - 3)}`) : layer?.get(`hi 6/${x >> (z - 6)}/${y >> (z - 6)}`);
  // (The meshes z14 alone, as bldtiles makes them now.)
  const e = m[1] === 'bld' && z !== 14 ? undefined : pk?.e.get(key(z, x, y));
  if (!e) {
    res.statusCode = 204;
    return res.end();
  }
  const b = Buffer.alloc(e[1]);
  fs.readSync(pk.fd, b, 0, e[1], e[0]);
  served++;
  bytes += b.length;
  res.setHeader('Content-Type', m[1] === 'field' ? 'application/octet-stream' : 'application/x-protobuf');
  if (pk.gzip) res.setHeader('Content-Encoding', 'gzip');
  res.setHeader('Content-Length', b.length);
  res.end(b);
}).listen(Number(port), '127.0.0.1', () => console.log(`listening ${port}; ${[...packs].map(([l, p]) => `${l}: ${p.size} packs`).join(', ')}`));
