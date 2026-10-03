// x and Math.log10(x) as hex bits: every distinct ia of today's stops & sights (as the worker reads
// them: f32, at least 0.05), then a million random doubles over every exponent.
import fs from 'node:fs';
const B = process.argv[2] ?? 'http://localhost:8092';
const hex = (v) => { const b = new DataView(new ArrayBuffer(8)); b.setFloat64(0, v); return b.getBigUint64(0).toString(16); };
const out = [];
const seen = new Set();
for (const k of ['viewpoint', 'peak', 'waterfall', 'lighthouse', 'covered_bridge', 'rest', 'trailhead']) {
  const fc = await (await fetch(`${B}/api/layer/pois-${k}`)).json();
  for (const f of fc.features) {
    const ia = f.properties.ia == null ? 20000 : Number(f.properties.ia);
    const x = Math.max(0.05, Math.fround(ia));
    if (!seen.has(x)) { seen.add(x); out.push(`${hex(x)} ${hex(Math.log10(x))}`); }
  }
}
const ias = out.length;
const dv = new DataView(new ArrayBuffer(8));
for (let i = 0; i < 1_000_000; i++) {
  dv.setUint32(0, (Math.random() * 0x7ff00000) >>> 0);
  dv.setUint32(4, (Math.random() * 2 ** 32) >>> 0);
  const x = dv.getFloat64(0);
  out.push(`${hex(x)} ${hex(Math.log10(x))}`);
}
for (const x of [1, 10, 100, 0.1, 0.05, 2, 1e-310, 5e-324, 20000, 0.5]) out.push(`${hex(x)} ${hex(Math.log10(x))}`);
fs.writeFileSync(process.argv[3], out.join('\n') + '\n');
console.log(`${ias} distinct ia values, ${out.length} in all`);
