// How far a ray in the open gets before a wall, measured on the footprints, against the field's
// random-set model: node openrun.mjs <z> <x> <y> …   (level-z tiles; the z14 mesh tiles under each)
// Footprints rasterized at 8192² over the tile (z12: 1.2 m), the extruded layer's buildings (no
// outlines with parts, no copies); from random open points, random directions, marched to the first
// building (capped at 300 m): the share still in the open at 2, 4, 8, 16 m, and the mean. The model:
// open runs exponential with mean π (1 − F) ÷ L (F the coverage, L the walls' length a m²: from the
// field tile's f, w, v, as the shader takes them).
import zlib from 'node:zlib';
const NM = '/Users/brandontsang/repos/scenic-roads/web/node_modules';
const { VectorTile } = await import(`${NM}/@mapbox/vector-tile/index.js`);
const Pbf = (await import(`${NM}/pbf/index.js`)).PbfReader;
const M = 8192;
for (const key of process.argv.slice(2)) {
  const [Z, X, Y] = key.split('/').map(Number);
  const k = 14 - Z;
  const mask = new Uint8Array(M * M);
  for (let j = 0; j < 1 << k; j++) for (let i = 0; i < 1 << k; i++) {
    const r = await fetch(`http://127.0.0.1:5290/bld/14/${(X << k) + i}/${(Y << k) + j}`);
    if (r.status !== 200) continue;
    let buf = Buffer.from(await r.arrayBuffer());
    try { buf = zlib.gunzipSync(buf); } catch {}
    const vt = new VectorTile(new Pbf(buf));
    for (const ln of Object.keys(vt.layers)) {
      const L = vt.layers[ln];
      const sx = M / (1 << k) / L.extent, ox = i * (M >> k), oy = j * (M >> k);
      for (let f = 0; f < L.length; f++) {
        const ft = L.feature(f);
        if (ft.type !== 3) continue;
        const p = ft.properties;
        if (p.k === 2 || p.o || !p.h) continue;
        const edges = [];
        let y0 = Infinity, y1 = -Infinity;
        for (const ring of ft.loadGeometry()) for (let q = 0; q < ring.length; q++) {
          const a = ring[q], b = ring[(q + 1) % ring.length];
          const ax = ox + a.x * sx, ay = oy + a.y * sx, bx = ox + b.x * sx, by = oy + b.y * sx;
          if (ay === by) continue;
          edges.push([ax, ay, bx, by]);
          y0 = Math.min(y0, ay, by); y1 = Math.max(y1, ay, by);
        }
        for (let yy = Math.max(0, Math.ceil(y0 - 0.5)); yy <= Math.min(M - 1, Math.floor(y1 - 0.5)); yy++) {
          const yc = yy + 0.5, xs = [];
          for (const [ax, ay, bx, by] of edges) if ((ay <= yc && by > yc) || (by <= yc && ay > yc)) xs.push(ax + ((yc - ay) / (by - ay)) * (bx - ax));
          xs.sort((p2, q2) => p2 - q2);
          for (let q = 0; q + 1 < xs.length; q += 2) for (let xx = Math.max(0, Math.ceil(xs[q] - 0.5)); xx <= Math.min(M - 1, Math.floor(xs[q + 1] - 0.5)); xx++) mask[yy * M + xx] = 1;
        }
      }
    }
  }
  const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (Y + 0.5)) / 2 ** Z)));
  const mpp = (40075016.686 * Math.cos(lat)) / 2 ** Z / M;   // metres a sample
  // the field's figures for the tile: F, L = Σ w ÷ H
  const r = await fetch(`http://127.0.0.1:5290/field/${Z}/${X}/${Y}.bin`);
  const u = new Uint16Array(zlib.gunzipSync(Buffer.from(await r.arrayBuffer())).buffer.slice(0));
  const N = 512 * 512;
  let F = 0, LA = 0;
  for (let c = 0; c < 5; c++) for (let i = 0; i < N; i++) {
    const f = u[c * N + i] / 65535;
    if (f < 1e-5) continue;
    const w = 2 ** ((u[(5 + c) * N + i] / 65535) * 12) - 1, H = Math.max(u[(10 + c) * N + i] / 64 / f, 0.5);
    F += f; LA += w / H;
  }
  F /= N; LA /= N;
  const lModel = (Math.PI * (1 - F)) / LA;
  // the runs
  let cover = 0;
  for (let i = 0; i < M * M; i += 97) cover += mask[i];
  cover /= Math.ceil((M * M) / 97);
  const ds = [2, 4, 8, 16], alive = [0, 0, 0, 0];
  let sum = 0, n = 0;
  let seed = 12345;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) >>> 0) / 4294967296);
  const capPx = 300 / mpp;
  while (n < 200000) {
    const x = 1000 + rnd() * (M - 2000), y = 1000 + rnd() * (M - 2000);
    if (mask[Math.floor(y) * M + Math.floor(x)]) continue;
    const a = rnd() * 2 * Math.PI, dx = Math.cos(a), dy = Math.sin(a);
    let s = 0;
    while (s < capPx) {
      s += 0.5;
      const px = Math.floor(x + dx * s), py = Math.floor(y + dy * s);
      if (px < 0 || py < 0 || px >= M || py >= M) break;
      if (mask[py * M + px]) break;
    }
    const sm = s * mpp;
    sum += sm; n++;
    ds.forEach((d, i) => { if (sm > d) alive[i]++; });
  }
  const lMeas = sum / n;
  console.log(`${key}: coverage ${cover.toFixed(3)} (field F ${F.toFixed(3)}), L ${LA.toFixed(3)}/m; open runs: mean ${lMeas.toFixed(1)} m (model ${lModel.toFixed(1)} m)`);
  console.log('  still open after  ' + ds.map((d, i) => `${d} m: ${(alive[i] / n).toFixed(3)} (model ${Math.exp(-d / lModel).toFixed(3)}; hazard ×${(-Math.log(alive[i] / n) / (d / lModel)).toFixed(2)})`).join('   '));
}
