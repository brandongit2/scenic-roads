// Serves a synthetic town (synth.mjs) as the field and the meshes: node synthserve.mjs <port> <dir>
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
const [port, dir] = process.argv.slice(2);
http.createServer((req, res) => {
  res.setHeader('Access-Control-Allow-Origin', '*');
  const u = new URL(req.url, 'http://x');
  let m;
  if (u.pathname === '/field/index.json') return res.end(fs.readFileSync(path.join(dir, 'field', 'index.json')));
  if ((m = u.pathname.match(/^\/field\/(\d+)\/(\d+)\/(\d+)\.bin$/))) {
    const p = path.join(dir, 'field', m[1], m[2], `${m[3]}.bin`);
    if (!fs.existsSync(p)) { res.statusCode = 404; return res.end(); }
    return res.end(fs.readFileSync(p));
  }
  if ((m = u.pathname.match(/^\/bld\/(\d+)\/(\d+)\/(\d+)$/))) {
    const p = path.join(dir, 'bld', m[2], `${m[3]}.pbf.gz`);
    if (m[1] !== '14' || !fs.existsSync(p)) { res.statusCode = 204; return res.end(); }
    res.setHeader('Content-Type', 'application/x-protobuf');
    res.setHeader('Content-Encoding', 'gzip');
    return res.end(fs.readFileSync(p));
  }
  res.statusCode = 404;
  res.end();
}).listen(Number(port), '127.0.0.1');
