#!/usr/bin/env python3
"""GLOBathy's estimated lake floors (docs/inputs.md §6.10): every HydroLAKES v1.0 lake (10 ha and
more) with GLOBathy's suggested maximum depth (`Dmax_use_m`), drawn as a bowl on the lake's outline,
depth below the lake's surface. Estimates throughout (the declaration's `estimated`): the build
classes them so and draws them only where no survey has a value (`datum_exact = false`).

- GLOBathy's maximum depths: its basic-parameters zip (CC0, figshare), fetched into `--cache` after
  the host's robots.txt (or `--params <zip>`); its `(ALL_LAKES).csv`, rows by `Hylak_id`.
- The outlines: HydroLAKES v1.0's polygons (`HydroLAKES_polys_v10_shp.zip`, the owner's download,
  `--hydrolakes`), read from the zip record by record, lakes meeting `--box`.
- The bowl (the depth lab's fit against 436 surveyed lakes, nRMSE 0.21): D = Dmax · (l / L)^0.75,
  l the distance to the nearest shore (islands are shore), smoothed by a Gaussian of L/8 (land
  counting as 0, so the medial axis's crease doesn't show), L its largest; GLOBathy's own rasters
  are the same bowl with p = 1 (`P`).
- On the z11 grid (`ZOOM`, ~40 m at 50°N: GLOBathy's 1″ rasters held coarser, as estimates), cut
  along z8 tiles (one grid a z8 tile with lake). A lake is computed once, painted into every tile
  it meets; HydroLAKES' lakes don't overlap.

Boxes are made one at a time, each holding its tiles in memory (16 MB each): give several
`--box`es of a few degrees side by side (each owns the tiles whose corner is in it, `owned`, so
none is written twice), one index for all.

usage: globathy.py --hydrolakes <zip> (--cache <dir> | --params <zip>) --box w,s,e,n [--box …] --out <dir> --index <file>
"""
from __future__ import annotations

import argparse
import csv
import io
import math
import zipfile
from pathlib import Path

import numpy as np

from grid import TILE, Window, Writer, fetch, lonlat_to_merc, log, px_size

PARAMS_URL = "https://ndownloader.figshare.com/files/28919991"
SOURCE_URL = "https://doi.org/10.6084/m9.figshare.c.5243309.v1"
ZOOM = 11
PIECE_ZOOM = 8
P = 0.75
SIGMA = 1 / 8


def read_dmax(params_zip: Path, box) -> dict[int, float]:
    """GLOBathy's `Dmax_use_m` (m, > 0) by Hylak_id for the lakes whose pour point is within 1° of
    `box` (a lake's outline reaches past its pour point)."""
    w, s, e, n = box
    out = {}
    with zipfile.ZipFile(params_zip) as z:
        name = next(m for m in z.namelist() if m.endswith("(ALL_LAKES).csv"))
        with z.open(name) as f:
            for r in csv.DictReader(io.TextIOWrapper(f, "utf-8", newline="")):
                lon, lat = float(r["Pour_long"]), float(r["Pour_lat"])
                if not (w - 1 <= lon <= e + 1 and s - 1 <= lat <= n + 1):
                    continue
                try:
                    d = float(r["Dmax_use_m"])
                except ValueError:
                    continue
                if math.isfinite(d) and d > 0:
                    out[int(float(r["Hylak_id"]))] = d
    return out


def read_lakes(hydrolakes_zip: Path, box, dmax: dict[int, float]):
    """(Hylak_id, Dmax, the outline's rings in lon/lat) of each lake meeting `box` with a Dmax."""
    import shapefile

    w, s, e, n = box
    with zipfile.ZipFile(hydrolakes_zip) as z:
        names = z.namelist()
        member = lambda ext: next(m for m in names if m.lower().endswith("hydrolakes_polys_v10" + ext))
        with z.open(member(".shp")) as shp, z.open(member(".dbf")) as dbf:
            r = shapefile.Reader(shp=shp, dbf=dbf)
            fields = [f[0] for f in r.fields[1:]]
            k = fields.index("Hylak_id")
            for sr in r.iterShapeRecords(bbox=[w, s, e, n]):
                hid = int(sr.record[k])
                if hid not in dmax or not sr.shape.points:
                    continue
                pts, parts = sr.shape.points, list(sr.shape.parts) + [len(sr.shape.points)]
                rings = [pts[parts[i]:parts[i + 1]] for i in range(len(parts) - 1)]
                yield hid, dmax[hid], rings


def bowl(rings, dmax: float):
    """The lake's bowl on the z11 grid: (its window, depth: negative below the surface, NaN off it)."""
    from rasterio import features
    from scipy import ndimage

    merc = [[lonlat_to_merc(x, y) for x, y in r] for r in rings]
    xs = [p[0] for r in merc for p in r]
    ys = [p[1] for r in merc for p in r]
    win = Window.around(ZOOM, (min(xs), min(ys), max(xs), max(ys)), pad=2)
    geom = {"type": "Polygon", "coordinates": [[(float(x), float(y)) for x, y in r] for r in merc]}
    inside = features.rasterize([(geom, 1)], out_shape=(win.h, win.w), transform=win.transform(), fill=0, dtype="uint8").astype(bool)
    if not inside.any():
        return win, np.full((win.h, win.w), np.nan, "float32")
    # (Ground metres a pixel, at the lake's middle latitude.)
    lat = math.degrees(2 * math.atan(math.exp((sum(ys) / len(ys)) / 6378137.0)) - math.pi / 2)
    m = px_size(ZOOM) * math.cos(math.radians(lat))
    l = np.maximum(ndimage.distance_transform_edt(inside) - 0.5, 0.25) * m
    big = float(l[inside].max())
    sig = big * SIGMA / m
    if sig > 0.05:
        l = ndimage.gaussian_filter(np.where(inside, l, 0.0), sig, mode="constant")
    top = float(l[inside].max())
    d = np.where(inside, -dmax * np.power(np.clip(l / top, 0.0, 1.0), P), np.nan) if top > 0 else np.full(l.shape, np.nan)
    return win, d.astype("float32")


def owned(box, tx: int, ty: int) -> bool:
    """Whether z8 tile (tx, ty) is `box`'s: its north-west corner in it (w ≤ lon < e, s < lat ≤ n),
    so boxes side by side share no tile and a run per box writes each tile once, whole."""
    w, s, e, n = box
    span = TILE * 2 ** (ZOOM - PIECE_ZOOM)
    b = Window(ZOOM, tx * span, ty * span, 1, 1).bounds_lonlat()
    x, y = b[0], b[3]
    return w <= x < e and s < y <= n


def produce(lakes, writer: Writer, url: str, box) -> int:
    """Each lake's bowl painted into the z8 tiles of `box` (`owned`) it meets; a grid a tile. The
    tiles' grids."""
    span = TILE * 2 ** (ZOOM - PIECE_ZOOM)
    tiles: dict[tuple[int, int], np.ndarray] = {}
    for _hid, dmax, rings in lakes:
        win, d = bowl(rings, dmax)
        ok = np.isfinite(d)
        if not ok.any():
            continue
        for ty in range(win.r0 // span, (win.r0 + win.h - 1) // span + 1):
            for tx in range(win.c0 // span, (win.c0 + win.w - 1) // span + 1):
                c0, r0 = max(win.c0, tx * span), max(win.r0, ty * span)
                c1, r1 = min(win.c0 + win.w, (tx + 1) * span), min(win.r0 + win.h, (ty + 1) * span)
                if not owned(box, tx, ty):
                    continue
                sub = d[r0 - win.r0:r1 - win.r0, c0 - win.c0:c1 - win.c0]
                if not np.isfinite(sub).any():
                    continue
                t = tiles.setdefault((tx, ty), np.full((span, span), np.nan, "float32"))
                dst = t[r0 - ty * span:r1 - ty * span, c0 - tx * span:c1 - tx * span]
                np.copyto(dst, sub, where=np.isfinite(sub))
    n = 0
    for (tx, ty), t in sorted(tiles.items()):
        n += writer.put(f"globathy-{PIECE_ZOOM}-{tx}-{ty}", url, Window(ZOOM, tx * span, ty * span, span, span), t)
    return n


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--hydrolakes", required=True, type=Path)
    ap.add_argument("--cache", type=Path)
    ap.add_argument("--params", type=Path)
    ap.add_argument("--box", required=True, action="append")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", required=True, type=Path)
    a = ap.parse_args()
    boxes = [[float(v) for v in b.split(",")] for b in a.box]
    if a.params is None:
        if a.cache is None:
            raise SystemExit("--cache or --params")
        a.params = fetch(PARAMS_URL, a.cache / "GLOBathy_basic_parameters.zip")
    w = Writer(a.out)
    for box in boxes:
        # (The lakes meeting the box's tiles, which reach past it by up to a z8 tile.)
        reach = [box[0], box[1] - 1.5, box[2] + 1.5, box[3]]
        dmax = read_dmax(a.params, reach)
        n = produce(read_lakes(a.hydrolakes, reach, dmax), w, SOURCE_URL, box)
        log(f"{box}: {len(dmax)} GLOBathy lakes near it, {n} grids")
    w.write_index(a.index)


if __name__ == "__main__":
    main()
