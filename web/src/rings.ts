// An outline's rings nested into polygons with their holes (regions.ts reads the coverage and the
// areas' outlines through it). Pure, without imports: web/test/rings.test.ts runs it under node.

type Ring = GeoJSON.Position[];
type Box = [number, number, number, number];

const ringBox = (r: Ring): Box => {
  let w = Infinity, s = Infinity, e = -Infinity, n = -Infinity;
  for (const [x, y] of r) {
    if (x < w) w = x;
    if (y < s) s = y;
    if (x > e) e = x;
    if (y > n) n = y;
  }
  return [w, s, e, n];
};
const inRing = (r: Ring, [x, y]: GeoJSON.Position) => {
  let inside = false;
  for (let i = 0, j = r.length - 1; i < r.length; j = i++) {
    if (r[i][1] > y !== r[j][1] > y && x < ((r[j][0] - r[i][0]) * (y - r[i][1])) / (r[j][1] - r[i][1]) + r[i][0]) inside = !inside;
  }
  return inside;
};

/**
 * An outline's rings as polygons with their holes. Outlines from passes before ring kinds were
 * recorded come with every ring as a polygon of its own, read even–odd (a ring inside another is a
 * hole in it, an island in that hole a ring again), which MapLibre would fill solid, an enclave over
 * its surroundings; newer ones come nested. Either way the rings are nested again by containment,
 * each hole with the smallest ring around it.
 *
 * The rings around a ring are looked up in a grid of the outline's extent (each ring listed in the
 * cells its box covers; a ring's first point's cell lists every ring that can hold it): an Arctic
 * outline has over 100 000 rings, and comparing every pair froze the page (#564).
 */
export function nested(f: GeoJSON.Feature<GeoJSON.MultiPolygon>): GeoJSON.Feature<GeoJSON.MultiPolygon> {
  const rings = f.geometry.coordinates.flat(1);
  const n = rings.length;
  if (n < 2) return f;
  const box = rings.map(ringBox);
  let w = Infinity, s = Infinity, e = -Infinity, nn = -Infinity;
  for (const b of box) {
    if (b[0] < w) w = b[0];
    if (b[1] < s) s = b[1];
    if (b[2] > e) e = b[2];
    if (b[3] > nn) nn = b[3];
  }
  const g = Math.max(1, Math.min(1024, Math.ceil(Math.sqrt(n))));
  const cw = (e - w) / g || 1, ch = (nn - s) / g || 1;
  const col = (x: number) => Math.min(g - 1, Math.max(0, Math.floor((x - w) / cw)));
  const row = (y: number) => Math.min(g - 1, Math.max(0, Math.floor((y - s) / ch)));
  const cells: number[][] = Array.from({ length: g * g }, () => []);
  box.forEach((b, i) => {
    for (let y = row(b[1]), y1 = row(b[3]); y <= y1; y++) for (let x = col(b[0]), x1 = col(b[2]); x <= x1; x++) cells[y * g + x].push(i);
  });
  const around = rings.map((r, i) => {
    const bi = box[i], p = r[0];
    return cells[row(p[1]) * g + col(p[0])].filter((j) => {
      const bj = box[j];
      return j !== i && bj[0] <= bi[0] && bj[1] <= bi[1] && bj[2] >= bi[2] && bj[3] >= bi[3] && inRing(rings[j], p);
    });
  });
  const depth = around.map((a) => a.length);
  const size = box.map((b) => (b[2] - b[0]) * (b[3] - b[1]));
  const polys = new Map<number, Ring[]>();
  rings.forEach((r, i) => depth[i] % 2 === 0 && polys.set(i, [r]));
  rings.forEach((r, i) => {
    if (depth[i] % 2 === 0) return;
    const outer = around[i].filter((j) => depth[j] === depth[i] - 1).sort((a, b) => size[a] - size[b])[0];
    if (outer === undefined) polys.set(i, [r]);
    else polys.get(outer)!.push(r);
  });
  return { ...f, geometry: { type: 'MultiPolygon', coordinates: [...polys.values()] } };
}
