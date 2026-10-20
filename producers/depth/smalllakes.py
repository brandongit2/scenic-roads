#!/usr/bin/env python3
"""Estimated floors of the small lakes (docs/inputs.md §6.10): every lake or pond of the map's own
basemap (OpenStreetMap's outlines, its z14 water) under 10 ha that HydroLAKES lacks (so GLOBathy has
no depth for it), its maximum depth estimated from the slope of the land around it, drawn as a bowl.
Estimates throughout (the declaration's `estimated`, gaps only: `datum_exact = false`), so a survey
and GLOBathy win where they have a value.

- The estimate: the depth lab's fit (its validation.md §3, `params_depth.json`: 357 measured lakes
  of Massachusetts and Minnesota, checked on Québec's; on the 85 measured lakes under 10 ha of the
  three, median error 48 %, against 53 % for the area rule):
  ln Dmax = a + c ln S + b ln L + d (ln L)², ln L held at its peak beyond it (`BETA`), S the mean
  land slope (m/m) in a ring 30–100 m from the shore (the map's z12 terrain, other lakes and ponds
  and terrain at or below 0.3 m left out), L the largest distance to shore (m, the largest inscribed
  circle's radius; islands are shore). Where the ring has under 3 land pixels or S < 0.003 (flat or
  missing terrain): the area rule Dmax = e^a · A^b (`AREA_BETA`, A in ha).
- The bowl: globathy.py's (D = Dmax · (l / L)^0.75, the distance to shore smoothed by L/8), here on
  the z14 grid (`ZOOM`, the basemap's own; ~7 m at 45°N).
- The lakes: the basemap's z14 water polygons of class lake or pond, merged by OSM id across tiles,
  under 10 ha (ground), not intermittent (seasonal water is left out on purpose), not touching the
  sea (bays drawn as lakes: the sea's sources), and not HydroLAKES': a lake a fifth or more of whose
  area a HydroLAKES v1.0 polygon covers is GLOBathy's (`--hydrolakes`, the owner's zip).
- Read from the app's server (`--server`, this machine's: /tiles/base and /tiles/terrain), nothing
  fetched from the internet.

A grid a z10 tile (`PIECE_ZOOM`, 4096² at z14, cropped to its data); a z10 tile's lakes are read
from its z14 tiles and a ring of one more (a lake reaching past that ring is skipped and counted).
Several `--box`es a run, each owning the tiles whose north-west corner is in it (globathy.owned's
rule), one index for all, written after each box. A run resumes: the tiles finished are listed in
`<index>.done` and skipped when their grid is in the index. The server's answers are counted and
logged a box (a 404 reads as no tile); a box whose basemap answers were all 404s fails.

usage: smalllakes.py --hydrolakes <zip> --box w,s,e,n [--box …] --out <dir> --index <file> [--server URL]
"""
from __future__ import annotations

import argparse
import collections
import gzip
import math
import time
import urllib.error
import urllib.request
import zipfile
from functools import lru_cache
from pathlib import Path

import numpy as np

import globathy
from grid import HALF, TILE, UA, Window, Writer, log, merc_to_lonlat, px_size

SOURCE_URL = "https://doi.org/10.1371/journal.pone.0025764"  # (Hollister et al. 2011, the method)
ZOOM = 14
PIECE_ZOOM = 10
TERRAIN_Z = 12
MAX_HA = 10.0
# (The depth lab's params_depth.json, model kSLq on S30_100, and its area rule.)
BETA = (-3.820468659540713, 0.36259624867415013, 1.983300273194609, -0.13239733333469395)
AREA_BETA = (1.1112663099708497, 0.22753622975506627)
S_MIN = 0.003
RING = (30.0, 100.0)
HYDROLAKES_SHARE = 0.2
SEA_M = 3.0


def dmax_estimate(S: float | None, L: float, area_ha: float) -> tuple[float, bool]:
    """(The estimated maximum depth in metres, whether the land slope gave it rather than the area
    rule.)"""
    if S is not None and math.isfinite(S) and S >= S_MIN:
        a, c, b, d = BETA
        lnL = math.log(max(L, 1.0))
        if d < 0:
            lnL = min(lnL, -b / (2 * d))
        return math.exp(a + c * math.log(S) + b * lnL + d * lnL * lnL), True
    a, b = AREA_BETA
    return math.exp(a + b * math.log(max(area_ha, 1e-3))), False


def ground_scale(y_merc: float) -> float:
    """Ground metres per Mercator metre at Mercator y."""
    return math.cos(math.radians(float(merc_to_lonlat(0.0, y_merc)[1])))


def largest_distance(g, scale: float) -> float:
    """L: the radius of the largest circle inside the outline (ground metres; islands are shore)."""
    import shapely

    best = 0.0
    for p in shapely.get_parts(g):
        if p.geom_type != "Polygon" or p.area <= 0:
            continue
        c = shapely.maximum_inscribed_circle(p, max(0.5, math.sqrt(p.area) / 2000))
        best = max(best, c.length)
    return best * scale


def land_slope(g, others, terrain, scale: float) -> float | None:
    """S: the mean slope (m/m) of the land 30–100 m (ground) from the outline `g` (Mercator), on the
    z12 terrain (`terrain(c0, r0, w, h)`: metres, NaN where missing); `others` (the other lakes and
    ponds) and terrain at or below 0.3 m left out. None under 3 land pixels."""
    from rasterio import features
    from scipy import ndimage

    r = px_size(TERRAIN_Z)
    margin = (RING[1] + 60) / scale
    minx, miny, maxx, maxy = g.bounds
    c0 = int(math.floor((minx - margin + HALF) / r))
    c1 = int(math.ceil((maxx + margin + HALF) / r))
    r0 = int(math.floor((HALF - maxy - margin) / r))
    r1 = int(math.ceil((HALF - miny + margin) / r))
    win = Window(TERRAIN_Z, c0, r0, c1 - c0, r1 - r0)
    dem = terrain(c0, r0, win.w, win.h)
    if not np.isfinite(dem).any():
        return None
    tr = win.transform()
    me = features.rasterize([(g, 1)], out_shape=(win.h, win.w), transform=tr, fill=0, dtype="uint8", all_touched=True).astype(bool)
    wat = np.zeros_like(me)
    if others:
        wat = features.rasterize([(o, 1) for o in others], out_shape=(win.h, win.w), transform=tr, fill=0, dtype="uint8").astype(bool)
    gy, gx = np.gradient(dem.astype("float64"), r * scale)
    slope = np.hypot(gx, gy)
    dist = ndimage.distance_transform_edt(~me) * r * scale
    m = ~me & ~wat & (dem > 0.3) & np.isfinite(slope) & (dist > RING[0]) & (dist <= RING[1])
    if m.sum() < 3:
        return None
    return float(slope[m].mean())


class Server:
    """The app's tiles over HTTP (this machine's server): the basemap's MVT and the terrain's
    Terrarium PNGs. `answers` counts them by kind ("base 404", "base ok", …): a 404 is read as no
    tile, so a box whose every basemap answer is a 404 (a server without that area) fails (`check`)
    rather than giving an empty source."""

    def __init__(self, url: str, pause: float = 2.0):
        self.url = url.rstrip("/")
        self.pause = pause
        self.answers: collections.Counter = collections.Counter()

    def _get(self, kind: str, path: str) -> bytes:
        req = urllib.request.Request(f"{self.url}{path}", headers={"User-Agent": UA})
        for attempt in range(5):
            try:
                with urllib.request.urlopen(req, timeout=60) as r:
                    self.answers[f"{kind} ok"] += 1
                    return r.read() if r.status != 204 else b""
            except urllib.error.HTTPError as e:
                e.close()
                if e.code == 404:
                    self.answers[f"{kind} 404"] += 1
                    return b""
                if attempt == 4:
                    raise
            except OSError:
                if attempt == 4:
                    raise
            time.sleep(self.pause * 2 ** attempt)  # (2, 4, 8, 16 s)
        return b""

    def check(self, box):
        """Fails `box` when every basemap tile asked for it was a 404."""
        if self.answers["base 404"] and not self.answers["base ok"]:
            raise SystemExit(f"{box}: every basemap tile was a 404 ({self.answers['base 404']}): the server at {self.url} has no basemap there")

    @lru_cache(maxsize=2048)
    def base(self, z: int, x: int, y: int) -> bytes:
        return self._get("base", f"/tiles/base/{z}/{x}/{y}")

    @lru_cache(maxsize=256)
    def terrain(self, z: int, x: int, y: int):
        import warnings

        from rasterio.errors import NotGeoreferencedWarning
        from rasterio.io import MemoryFile

        b = self._get("terrain", f"/tiles/terrain/{z}/{x}/{y}")
        if not b:
            return None
        with warnings.catch_warnings(), MemoryFile(b) as f:
            warnings.simplefilter("ignore", NotGeoreferencedWarning)
            with f.open() as ds:
                rgb = ds.read([1, 2, 3]).astype("float32")
        return rgb[0] * 256.0 + rgb[1] + rgb[2] / 256.0 - 32768.0


def water_polygons(mvt: bytes, z: int, x: int, y: int):
    """A basemap tile's water: [(class, (osm id, 'w'/'r'/''), intermittent, shapely polygon in
    Mercator)], tunnels left out."""
    import mapbox_vector_tile
    import shapely

    if not mvt:
        return []
    if mvt[:2] == b"\x1f\x8b":
        mvt = gzip.decompress(mvt)
    wl = mapbox_vector_tile.decode(mvt, default_options={"y_coord_down": True}).get("water")
    if not wl:
        return []
    tsz = 2 * HALF / 2 ** z
    ox, oy, k = -HALF + x * tsz, HALF - y * tsz, tsz / wl["extent"]
    out = []
    for f in wl["features"]:
        p = f["properties"]
        if p.get("brunnel") == "tunnel":
            continue
        g = f["geometry"]
        polys = g["coordinates"] if g["type"] == "MultiPolygon" else [g["coordinates"]] if g["type"] == "Polygon" else []
        fid = int(f.get("id") or 0)
        key = (int(p.get("id") or 0), {2: "w", 3: "r"}.get(fid % 10, "") if fid else "")
        for poly in polys:
            rings = [[(ox + a * k, oy - b * k) for a, b in r] for r in poly if len(r) >= 4]
            if not rings:
                continue
            pg = shapely.Polygon(rings[0], rings[1:])
            if not pg.is_valid:
                pg = shapely.make_valid(pg)
            out.append((p.get("class", "lake"), key, int(p.get("intermittent") or 0), pg))
    return out


def lakes_of(tiles, x0: int, y0: int, x1: int, y1: int):
    """The lakes and ponds of z14 tiles x0..x1 × y0..y1, merged by OSM id (id-less ones where they
    touch): [(key, intermittent, geometry)], and the sea's polygons."""
    import shapely

    pieces, inter, noid, sea = collections.defaultdict(list), {}, [], []
    # (A piece at the map's edge asks for its neighbours past it, x or y -1 or 2^ZOOM: there are
    # none, and the server answers 400. A lake cut by the antimeridian keeps only its part on
    # this side.)
    n = 2**ZOOM
    for x in range(max(x0, 0), min(x1, n - 1) + 1):
        for y in range(max(y0, 0), min(y1, n - 1) + 1):
            for cls, key, it, pg in water_polygons(tiles.base(ZOOM, x, y), ZOOM, x, y):
                if cls == "ocean":
                    sea.append(pg)
                elif cls in ("lake", "pond"):
                    if key[0] == 0:
                        noid.append(pg)
                    else:
                        pieces[key].append(pg)
                        inter[key] = inter.get(key, 0) or it
    out = []
    for key, ps in pieces.items():
        g = shapely.make_valid(shapely.union_all(ps) if len(ps) > 1 else ps[0])
        if g.geom_type == "GeometryCollection":
            g = shapely.union_all([p for p in shapely.get_parts(g) if p.geom_type in ("Polygon", "MultiPolygon")])
        if not g.is_empty:
            out.append((key, inter[key], g))
    if noid:
        for i, g in enumerate(shapely.get_parts(shapely.union_all(noid))):
            out.append(((-(i + 1), ""), 0, g))
    return out, sea


def read_hydrolakes(hydrolakes_zip: Path, box):
    """HydroLAKES v1.0's polygons meeting `box` (lon/lat), in Mercator."""
    import shapefile
    import shapely

    from grid import lonlat_to_merc

    w, s, e, n = box
    out = []
    with zipfile.ZipFile(hydrolakes_zip) as z:
        names = z.namelist()
        member = lambda ext: next(m for m in names if m.lower().endswith("hydrolakes_polys_v10" + ext))
        with z.open(member(".shp")) as shp, z.open(member(".dbf")) as dbf:
            r = shapefile.Reader(shp=shp, dbf=dbf)
            for sh in r.iterShapes(bbox=[w, s, e, n]):
                if not sh.points:
                    continue
                parts = list(sh.parts) + [len(sh.points)]
                rings = []
                for i in range(len(parts) - 1):
                    a = np.asarray(sh.points[parts[i]:parts[i + 1]], dtype="float64")
                    x, y = lonlat_to_merc(a[:, 0], a[:, 1])
                    rings.append(np.column_stack([x, y]))
                pg = shapely.Polygon(rings[0], rings[1:])
                out.append(pg if pg.is_valid else shapely.make_valid(pg))
    return out


def tile_lakes(tiles, hl_tree, hl, tx: int, ty: int, counts: collections.Counter):
    """The small lakes meeting z10 tile (tx, ty): [(geometry in Mercator, Dmax)]."""
    import shapely

    f = 2 ** (ZOOM - PIECE_ZOOM)
    x0, y0, x1, y1 = tx * f - 1, ty * f - 1, (tx + 1) * f, (ty + 1) * f
    lakes, sea = lakes_of(tiles, x0, y0, x1, y1)
    tsz = 2 * HALF / 2 ** ZOOM
    fetched = shapely.box(-HALF + x0 * tsz, HALF - (y1 + 1) * tsz, -HALF + (x1 + 1) * tsz, HALF - y0 * tsz).exterior
    span = 2 * HALF / 2 ** PIECE_ZOOM
    piece = shapely.box(-HALF + tx * span, HALF - (ty + 1) * span, -HALF + (tx + 1) * span, HALF - ty * span)
    geoms = [g for _, _, g in lakes]
    tree = shapely.STRtree(geoms)
    sea_tree = shapely.STRtree(sea) if sea else None
    def terrain(c0, r0, w, h):
        a = np.full((h, w), np.nan, dtype="float32")
        for y in range(r0 // TILE, (r0 + h - 1) // TILE + 1):
            for x in range(c0 // TILE, (c0 + w - 1) // TILE + 1):
                t = tiles.terrain(TERRAIN_Z, x, y)
                if t is None:
                    continue
                oy, ox = y * TILE - r0, x * TILE - c0
                ys, xs = slice(max(0, oy), min(h, oy + TILE)), slice(max(0, ox), min(w, ox + TILE))
                a[ys, xs] = t[ys.start - oy:ys.stop - oy, xs.start - ox:xs.stop - ox]
        return a

    out = []
    for i, (key, it, g) in enumerate(lakes):
        if not g.intersects(piece):
            continue
        sc = ground_scale(g.centroid.y)
        ha = g.area * sc * sc / 1e4
        if ha >= MAX_HA:
            continue
        if it:
            counts["intermittent"] += 1
            continue
        if g.distance(fetched) < tsz / 256:
            counts["reaching past the tiles read"] += 1
            continue
        if sea_tree is not None and len(sea_tree.query(g, predicate="dwithin", distance=SEA_M / sc)):
            counts["touching the sea"] += 1
            continue
        if hl_tree is not None:
            cov = sum(g.intersection(hl[j]).area for j in hl_tree.query(g, predicate="intersects"))
            if cov >= HYDROLAKES_SHARE * g.area:
                counts["in HydroLAKES"] += 1
                continue
        others = [geoms[j] for j in tree.query(g.buffer((RING[1] + 60) / sc)) if j != i]
        S = land_slope(g, others, terrain, sc)
        L = largest_distance(g, sc)
        if L <= 0:
            continue
        d, by_slope = dmax_estimate(S, L, ha)
        counts["by the land slope" if by_slope else "by the area rule"] += 1
        out.append((g, d))
    return out


def merc_rings(g):
    """A polygon's (or a multipolygon's parts') rings in lon/lat, for globathy.bowl."""
    import shapely

    for p in shapely.get_parts(g):
        if p.geom_type != "Polygon" or p.is_empty:
            continue
        rings = [np.asarray(p.exterior.coords)] + [np.asarray(r.coords) for r in p.interiors]
        yield [list(zip(*merc_to_lonlat(r[:, 0], r[:, 1]))) for r in rings]


def paint(lakes, tx: int, ty: int) -> np.ndarray:
    """The z10 tile's z14 grid: each lake's bowl (each part of a multipolygon its own, to the same
    Dmax), NaN off them."""
    span = TILE * 2 ** (ZOOM - PIECE_ZOOM)
    t = np.full((span, span), np.nan, "float32")
    for g, dmax in lakes:
        for rings in merc_rings(g):
            win, d = globathy.bowl(rings, dmax, zoom=ZOOM)
            c0, r0 = max(win.c0, tx * span), max(win.r0, ty * span)
            c1, r1 = min(win.c0 + win.w, (tx + 1) * span), min(win.r0 + win.h, (ty + 1) * span)
            if c0 >= c1 or r0 >= r1:
                continue
            sub = d[r0 - win.r0:r1 - win.r0, c0 - win.c0:c1 - win.c0]
            dst = t[r0 - ty * span:r1 - ty * span, c0 - tx * span:c1 - tx * span]
            np.copyto(dst, sub, where=np.isfinite(sub))
    return t


def owned(box, tx: int, ty: int) -> bool:
    w, s, e, n = box
    b = Window(PIECE_ZOOM, tx * TILE, ty * TILE, 1, 1).bounds_lonlat()
    return w <= b[0] < e and s < b[3] <= n


def piece_range(box):
    from grid import lonlat_to_merc

    w, s, e, n = box
    span = 2 * HALF / 2 ** PIECE_ZOOM
    x0, y1 = lonlat_to_merc(w, s)
    x1, y0 = lonlat_to_merc(e, n)
    return (int(math.floor((float(x0) + HALF) / span)) - 1, int(math.floor((HALF - float(y0)) / span)) - 1,
            int(math.floor((float(x1) + HALF) / span)) + 1, int(math.floor((HALF - float(y1)) / span)) + 1)


def produce(tiles, hl, writer: Writer, box, counts: collections.Counter, done: Path | None = None) -> int:
    """Each owned z10 tile of `box`: its grid when it has small lakes. With `done` (a file of the
    tiles finished, "tx ty file-or-dash" a line, appended as each one ends), the tiles listed there
    are skipped when their grid is in the writer's index (or they had none), so a stopped run
    resumes."""
    import shapely

    hl_tree = shapely.STRtree(hl) if hl else None
    finished = set()
    if done is not None and done.exists():
        have = {f["properties"]["file"] for f in writer.features}
        for line in done.read_text().split("\n"):
            p = line.split()
            if len(p) == 3 and (p[2] == "-" or p[2] in have):
                finished.add((int(p[0]), int(p[1])))
    n = 0
    span = TILE * 2 ** (ZOOM - PIECE_ZOOM)
    tx0, ty0, tx1, ty1 = piece_range(box)
    for ty in range(ty0, ty1 + 1):
        for tx in range(tx0, tx1 + 1):
            if not owned(box, tx, ty):
                continue
            if (tx, ty) in finished:
                counts["tiles done before"] += 1
                continue
            ans = getattr(tiles, "answers", collections.Counter())
            before = (ans["base ok"], ans["base 404"])
            lakes = tile_lakes(tiles, hl_tree, hl, tx, ty, counts)
            # (A tile whose basemap reads were all 404s isn't recorded as done: a server without
            # the area mustn't make a rerun skip it.)
            all_404 = ans["base ok"] == before[0] and ans["base 404"] > before[1]
            name = f"smalllakes-{PIECE_ZOOM}-{tx}-{ty}"
            wrote = bool(lakes) and writer.put(name, SOURCE_URL, Window(ZOOM, tx * span, ty * span, span, span), paint(lakes, tx, ty))
            n += wrote
            if done is not None and not all_404:
                with open(done, "a") as f:
                    f.write(f"{tx} {ty} {name + '.tif' if wrote else '-'}\n")
    return n


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--hydrolakes", required=True, type=Path)
    ap.add_argument("--server", default="http://127.0.0.1:7311")
    ap.add_argument("--box", required=True, action="append")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", required=True, type=Path)
    a = ap.parse_args()
    tiles = Server(a.server)
    w = Writer(a.out)
    w.resume(a.index)
    done = a.index.with_name(a.index.name + ".done")
    for b in a.box:
        box = [float(v) for v in b.split(",")]
        counts = collections.Counter()
        tiles.answers.clear()
        # (HydroLAKES' lakes meeting the box's tiles and their ring, which reach past it.)
        hl = read_hydrolakes(a.hydrolakes, [box[0] - 0.5, box[1] - 0.5, box[2] + 0.5, box[3] + 0.5])
        n = produce(tiles, hl, w, box, counts, done)
        log(f"{box}: {len(hl)} HydroLAKES lakes near it, {n} grids; lakes {dict(counts)}; server answers {dict(tiles.answers)}")
        w.write_index(a.index)
        tiles.check(box)


if __name__ == "__main__":
    main()
