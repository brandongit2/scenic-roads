// A synthetic town for the field's darkening test (task #102): a regular grid of identical flat-roofed
// blocks over open water, as the meshes' z14 vector tiles and as field tiles (levels 4–14), both from
// the same geometry, exact:   node synth.mjs <outdir> [block units] [period units] [height m]
// Units are the z14 tiles' (extent 4096); the area: lon 139.88–140.02, lat 35.40–35.48 (Tokyo Bay).
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const [OUT, B0 = '25', P0 = '50', H0 = '12'] = process.argv.slice(2);
const BLK = Number(B0), PER = Number(P0), H = Number(H0);
const lonlat2t = (lon, lat, z) => {
  const n = 2 ** z;
  const x = ((lon + 180) / 360) * n;
  const y = ((1 - Math.log(Math.tan((lat * Math.PI) / 180) + 1 / Math.cos((lat * Math.PI) / 180)) / Math.PI) / 2) * n;
  return [x, y];
};
const [gx0, gy0] = lonlat2t(139.88, 35.48, 14), [gx1, gy1] = lonlat2t(140.02, 35.40, 14);
const tx0 = Math.floor(gx0), tx1 = Math.floor(gx1), ty0 = Math.floor(gy0), ty1 = Math.floor(gy1);
// the blocks in global z14 units: X = tx * 4096 + x; the grid inside [gx0, gx1] × [gy0, gy1]
const X0 = Math.ceil((gx0 * 4096) / PER) * PER, X1 = gx1 * 4096, Y0 = Math.ceil((gy0 * 4096) / PER) * PER, Y1 = gy1 * 4096;
const lat = 35.44;
const mPerUnit = (40075016.686 * Math.cos((lat * Math.PI) / 180)) / 2 ** 14 / 4096;
console.log(`tiles z14 x ${tx0}–${tx1}, y ${ty0}–${ty1}; block ${(BLK * mPerUnit).toFixed(1)} m, period ${(PER * mPerUnit).toFixed(1)} m, H ${H} m, coverage ${((BLK / PER) ** 2).toFixed(3)}`);

// ---- the meshes: a minimal MVT encoder
const varint = (n, a) => { n = n >>> 0; while (n > 127) { a.push((n & 127) | 128); n >>>= 7; } a.push(n); };
const zz = (n) => (n << 1) ^ (n >> 31);
const field = (num, wire, a) => varint((num << 3) | wire, a);
const bytes = (num, payload, a) => { field(num, 2, a); varint(payload.length, a); for (const b of payload) a.push(b); };
fs.mkdirSync(path.join(OUT, 'bld'), { recursive: true });
for (let tx = tx0; tx <= tx1; tx++) for (let ty = ty0; ty <= ty1; ty++) {
  const feats = [];
  for (let X = X0; X < X1; X += PER) {
    const lx = X - tx * 4096;
    if (lx + BLK <= -64 || lx >= 4096 + 64) continue;
    for (let Y = Y0; Y < Y1; Y += PER) {
      const ly = Y - ty * 4096;
      if (ly + BLK <= -64 || ly >= 4096 + 64) continue;
      // (a block crossing the tile's edge: only in the tile holding its corner, whole, as the tiles'
      // own buildings stand in one tile; MapLibre draws it from there)
      if (lx < 0 || ly < 0 || lx >= 4096 || ly >= 4096) continue;
      const g = [];
      varint(9, g); varint(zz(lx), g); varint(zz(ly), g);
      varint(26, g); varint(zz(BLK), g); varint(zz(0), g); varint(zz(0), g); varint(zz(BLK), g); varint(zz(-BLK), g); varint(zz(0), g);
      varint(15, g);
      const f = [];
      field(2, 2, f); const tags = []; varint(0, tags); varint(0, tags); varint(tags.length, f); f.push(...tags);
      field(3, 0, f); varint(3, f);
      bytes(4, g, f);
      feats.push(f);
    }
  }
  if (!feats.length) continue;
  const L = [];
  field(15, 0, L); varint(2, L);
  bytes(1, Buffer.from('b'), L);
  for (const f of feats) bytes(2, f, L);
  bytes(3, Buffer.from('h'), L);
  const v = []; field(5, 0, v); varint(Math.round(H * 10), v);
  bytes(4, v, L);
  field(5, 0, L); varint(4096, L);
  const T = [];
  bytes(3, L, T);
  fs.mkdirSync(path.join(OUT, 'bld', String(tx)), { recursive: true });
  fs.writeFileSync(path.join(OUT, 'bld', String(tx), `${ty}.pbf.gz`), zlib.gzipSync(Buffer.from(T)));
}

// ---- the field: level 14 exact (coverage, wall area, v), class 1 (8–20 m) or the class of H
const cls = H < 8 ? 0 : H < 20 ? 1 : H < 40 ? 2 : H < 100 ? 3 : 4;
const N = 512, TU = 4096 / N;   // a texel's side in units
const tiles14 = new Map();
const overlap = (a0, a1, b0, b1) => Math.max(0, Math.min(a1, b1) - Math.max(a0, b0));
for (let tx = tx0; tx <= tx1; tx++) for (let ty = ty0; ty <= ty1; ty++) {
  const F = new Float32Array(N * N), W = new Float32Array(N * N);
  const ux0 = tx * 4096, uy0 = ty * 4096;
  // the blocks' x and y intervals meeting the tile
  const xs = [], ys = [];
  for (let X = X0; X < X1; X += PER) if (X + BLK > ux0 && X < ux0 + 4096) xs.push(X);
  for (let Y = Y0; Y < Y1; Y += PER) if (Y + BLK > uy0 && Y < uy0 + 4096) ys.push(Y);
  for (let j = 0; j < N; j++) {
    const t0 = uy0 + j * TU, t1 = t0 + TU;
    let fy = 0, ey = 0;   // covered length in y; vertical edges' share: the y length of blocks met
    const yOv = ys.map((Y) => overlap(Y, Y + BLK, t0, t1));
    for (const o of yOv) fy += o;
    // horizontal edges (y = Y, Y + BLK) inside this texel row
    const hEdges = ys.reduce((n, Y) => n + (Y >= t0 && Y < t1 ? 1 : 0) + (Y + BLK >= t0 && Y + BLK < t1 ? 1 : 0), 0);
    for (let i = 0; i < N; i++) {
      const s0 = ux0 + i * TU, s1 = s0 + TU;
      let fx = 0, vEdges = 0;
      for (const X of xs) {
        fx += overlap(X, X + BLK, s0, s1);
        vEdges += (X >= s0 && X < s1 ? 1 : 0) + (X + BLK >= s0 && X + BLK < s1 ? 1 : 0);
      }
      const k = j * N + i;
      F[k] = (fx / TU) * (fy / TU);
      // wall length in the texel (units): vertical edges × the covered y length, horizontal edges ×
      // the covered x length
      const len = vEdges * fy + hEdges * fx;
      W[k] = (len * mPerUnit * H) / (TU * mPerUnit) ** 2;
    }
  }
  tiles14.set(`${tx}/${ty}`, { F, W });
}
// the levels: each the mean of its four children (densities)
const levels = { 14: tiles14 };
for (let z = 13; z >= 4; z--) {
  const m = new Map();
  for (const [key, t] of levels[z + 1]) {
    const [cx, cy] = key.split('/').map(Number);
    const px = cx >> 1, py = cy >> 1, pk = `${px}/${py}`;
    let p = m.get(pk);
    if (!p) m.set(pk, (p = { F: new Float32Array(N * N), W: new Float32Array(N * N) }));
    const ox = (cx & 1) * (N / 2), oy = (cy & 1) * (N / 2);
    for (let j = 0; j < N / 2; j++) for (let i = 0; i < N / 2; i++) {
      let f = 0, w = 0;
      for (const [di, dj] of [[0, 0], [1, 0], [0, 1], [1, 1]]) { const k = (2 * j + dj) * N + 2 * i + di; f += t.F[k]; w += t.W[k]; }
      p.F[(oy + j) * N + ox + i] += f / 4;
      p.W[(oy + j) * N + ox + i] += w / 4;
    }
  }
  levels[z] = m;
}
const index = {};
for (let z = 4; z <= 14; z++) {
  for (const [key, t] of levels[z]) {
    const u = new Uint16Array(15 * N * N);
    for (let k = 0; k < N * N; k++) {
      const f = Math.min(1, t.F[k]);
      u[cls * N * N + k] = Math.round(f * 65535);
      u[(5 + cls) * N * N + k] = Math.round((Math.log2(1 + t.W[k]) / 12) * 65535);
      u[(10 + cls) * N * N + k] = Math.min(65535, Math.round(f * H * 64));
    }
    const [x, y] = key.split('/');
    fs.mkdirSync(path.join(OUT, 'field', String(z), x), { recursive: true });
    fs.writeFileSync(path.join(OUT, 'field', String(z), x, `${y}.bin`), zlib.gzipSync(Buffer.from(u.buffer)));
    (index[z] ??= []).push(key);
  }
}
fs.writeFileSync(path.join(OUT, 'field', 'index.json'), JSON.stringify(index));
console.log('levels', Object.entries(index).map(([z, l]) => `${z}:${l.length}`).join(' '));
