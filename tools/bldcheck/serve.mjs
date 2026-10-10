// Prototype server (task #102): the field tiles (<field>/<pack>/<z>/<x>/<y>.bin, merged) and the
// buildings' mesh tiles out of local copies of the packs, with CORS.
//   node serve.mjs <port> <fielddir> <packdir>
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const [port, fieldDir, packDir] = process.argv.slice(2);
const fields = fs.readdirSync(fieldDir).filter((d) => fs.statSync(path.join(fieldDir, d)).isDirectory()).map((d) => path.join(fieldDir, d));
const index = {};
for (const f of fields) {
  for (const z of fs.readdirSync(f)) {
    if (!/^\d+$/.test(z)) continue;
    for (const x of fs.readdirSync(path.join(f, z))) for (const y of fs.readdirSync(path.join(f, z, x))) (index[z] ??= []).push(`${x}/${y.replace('.bin', '')}`);
  }
}
const packs = new Map();
for (const p of fs.readdirSync(packDir)) {
  const fd = fs.openSync(path.join(packDir, p), 'r');
  const head = Buffer.alloc(36);
  fs.readSync(fd, head, 0, 36, 0);
  const idxOff = Number(head.readBigUInt64LE(16)), n = Number(head.readBigUInt64LE(24));
  const idx = Buffer.alloc(n * 32);
  fs.readSync(fd, idx, 0, n * 32, idxOff);
  const m = new Map();
  for (let i = 0; i < n; i++) m.set(idx.readBigUInt64LE(i * 32), [Number(idx.readBigUInt64LE(i * 32 + 8)), idx.readUInt32LE(i * 32 + 16)]);
  const [, x, y] = p.split('.')[0].split('-').map(Number);
  packs.set(`${x}/${y}`, { fd, m });
}
const key = (z, x, y) => (BigInt(z) << 58n) | (BigInt(x) << 29n) | BigInt(y);
let served = 0, bytes = 0;
http.createServer((req, res) => {
  res.setHeader('Access-Control-Allow-Origin', '*');
  const u = new URL(req.url, 'http://x');
  let m;
  if (u.pathname === '/field/index.json') {
    res.setHeader('Content-Type', 'application/json');
    return res.end(JSON.stringify(index));
  }
  if ((m = u.pathname.match(/^\/field\/(\d+)\/(\d+)\/(\d+)\.bin$/))) {
    for (const f of fields) {
      const p = path.join(f, m[1], m[2], `${m[3]}.bin`);
      if (fs.existsSync(p)) {
        const b = fs.readFileSync(p);
        served++;
        bytes += b.length;
        res.setHeader('Content-Type', 'application/octet-stream');
        res.setHeader('Content-Length', b.length);
        return res.end(b);
      }
    }
    res.statusCode = 404;
    return res.end();
  }
  if ((m = u.pathname.match(/^\/bld\/(\d+)\/(\d+)\/(\d+)$/))) {
    const [z, x, y] = m.slice(1).map(Number);
    const pk = packs.get(`${x >> (z - 6)}/${y >> (z - 6)}`);
    const e = pk?.m.get(key(z, x, y));
    if (!e) {
      res.statusCode = 204;
      return res.end();
    }
    const b = Buffer.alloc(e[1]);
    fs.readSync(pk.fd, b, 0, e[1], e[0]);
    res.setHeader('Content-Type', 'application/x-protobuf');
    res.setHeader('Content-Encoding', 'gzip');
    res.setHeader('Content-Length', b.length);
    return res.end(b);
  }
  if (u.pathname === '/stats') return res.end(JSON.stringify({ served, bytes }));
  res.statusCode = 404;
  res.end();
}).listen(Number(port), '127.0.0.1', () => console.log(`listening ${port}; ${fields.length} fields, ${packs.size} packs`));
