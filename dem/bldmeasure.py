#!/usr/bin/env python3
"""The 3D buildings' data measured (docs/buildings3d.md §2, phase B0): the coverage's buildings in the
downloaded Overture files (dem/bldfetch.py), their heights and floors by country and where they come
from, the storey heights fitted, the fill's errors on a held-out tenth of the measured heights
(§2.3), and the building tiles' counts and sizes at z12-14 as §3.4 would encode them.

Phases, each kept in --work and skipped where done (a run again goes on from where one stopped):
  scan    each Overture file's row groups that meet the coverage grown by 1 km: their bytes copied
          from the NAS into a sparse local copy of the file (reads of up to 16 MB, a file ahead),
          decoded by --jobs processes (pyarrow, shapely) into a record per building and part whose
          centroid is in the coverage + 1 km: centroid, footprint area, region, height, floors,
          kind, roof, the datasets of its footprint and of its height, and its geometry's encoded
          size at z12-14 (§3.4: in the tile of its centroid, quantized to the 4096 grid, repeated
          points and collapsed rings dropped, simplified to a grid unit at z12-13). Into
          work/spool/6-<x>-<y>/<file>.parquet by the z6 tile of the centroid; a building with a
          height or floors within 310 m of its tile's edge is also written to the neighbour's
          folder (for the neighbours' rule). GHSL's tiles are copied to work/ghsl/.
  fit     the storey height per country: height = a × floors + b, fitted by least absolute
          deviations over the buildings with both (the hold-out left out; the global fit for a
          country with fewer than 500), from the measured heights (ESTIMATES, Microsoft's, left
          out) and, to compare, from every height; each floor count's median height; the size
          classes' median heights. Into work/fit.json.
  fill    per z6 tile: GHSL sampled at each centroid; every building's height filled by §2.3's
          rules as B0 set them (0 measured, 1 floors, 2 Microsoft's estimate, 3 neighbours, 4 GHSL
          in its 20 m cells, 5 size and kind fitted), and by the first order (measured with
          Microsoft's, floors, neighbours, GHSL, size at the first defaults) for its shares; the
          tiles' counts at z12-14 by B0's heights (z14 every building and part, z13 the 20 m or
          2,000 m² ones, z12 the 40 m ones) with their estimated encoded size; and the hold-out:
          the buildings with a height whose id hashes to 0 mod 10, each estimated by every rule as
          if unmeasured (the neighbours' rule without the held-out heights), with the training
          tenth (1 mod 10) the report fits GHSL's factor on. Into work/fill/6-<x>-<y>.pkl.
  tiles   the fullest and heaviest tiles (by count and by the estimate), §4.6's test places' and
          a sample, encoded as §3.4 says (MVT 2.1, layer b, gzip level 6), their geometries read
          again from the NAS: raw and gzip'd sizes, the estimate calibrated by them, and the
          heaviest z14 tiles simplified to a grid unit or with fewer properties.
  report  the tables, as Markdown on stdout and in work/report.md and report.json: the rules
          against the measured heights and against every height, by country and true height.

The NAS is only read, never written. Runs pause between units (a file, a tile) while the build
agent has a job (--agent-status, read only). Python decodes and measures here; it isn't the build's
code (§3.7): numbers are for the plan, not for a tile.

usage: bldmeasure.py --root <NAS project folder> --work <local folder>
                     [--regions regions.geojson | --server http://localhost:8080] [--jobs 8]
                     [--fill-jobs 4] [--phases scan,fit,fill,tiles,report] [--box w,s,e,n]
                     [--agent-status <status.json>]
"""
from __future__ import annotations

import argparse
import gzip
import json
import math
import os
import pickle
import shutil
import sys
import time
import urllib.request
from collections import Counter
from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.parquet as pq
import shapely

OVERTURE = "sources/overture/2026-09-23-1"
GHSL_DIR = "sources/ghsl/R2023A"
GHSL_PRODUCT = "GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_4326_3ss_V1_0"
# The GHSL tiles' grid (as dem/bldfetch.py has it): R1_C1's corner, 10° tiles.
GHSL_TOP, GHSL_LEFT, GHSL_DEG = 89.09958317764332, -180.00791620856731, 10.0
EQ = 40075016.68557849             # the equator (WGS84), m: a Web Mercator world unit there
DEG_M = 6371008.8 * math.pi / 180  # a degree on the mean sphere, m
EXTENT = 4096
COVER_M = 1000.0                   # the coverage's buffer (plan.md §5)
EDGE_M = 310.0                     # the neighbours' 300 m, and some
H_MIN, H_MAX = 2.0, 700.0          # a measured height taken (§2.3, rule 0)
# Heights that are estimates, not measurements: left out of rule 1's fit, and scored apart.
ESTIMATES = {"Microsoft ML Buildings"}
GHSL_TALL = 20.0                   # B0's rule 4: GHSL only where its cell is this tall
F_MAX = 200                        # floors taken: 1-200
UA = "scenic-roads/0.1 (personal offline map)"

# A region's country: its parent's code, else its own (ISO 3166), but for these.
SPECIAL = {"french-guiana": "GF", "hong-kong": "HK", "puerto-rico": "PR", "saint-pierre-et-miquelon": "PM"}

# §2.3's rule 4: size and kind.
SHEDS = {"shed", "garage", "garages", "carport", "hut"}
CHURCHES = {"church", "cathedral"}
HOUSES = {"house", "detached", "semidetached_house", "terrace", "residential", "apartments", "bungalow",
          "dwelling_house", "farm", "cabin", "static_caravan", "houseboat", "stilt_house", "trullo",
          "allotment_house", "dormitory", "ger"}
SIZE_BINS = ["church or cathedral", "shed, garage or under 30 m²", "house or under 250 m²", "250-2,000 m²", "over 2,000 m²"]
SIZE_DEFAULT = np.array([15.0, 3.0, 6.5, 9.0, 10.0], np.float32)
# §3.4's `c`, from Overture's subtype.
KIND = {"residential": 1, "outbuilding": 2, "commercial": 3, "industrial": 4, "religious": 5, "civic": 6,
        "education": 6, "medical": 6, "agricultural": 7, "transportation": 8, "service": 9,
        "entertainment": 9, "military": 9}

# Places for §2.2's list: the buildings within the radius of the point.
PLACES = [
    ("Manhattan", -73.985, 40.758, 2), ("Chicago", -87.632, 41.882, 2), ("Los Angeles", -118.243, 34.052, 2),
    ("rural Kansas", -98.5, 38.6, 15), ("rural Vermont", -72.6, 44.0, 15), ("Toronto", -79.383, 43.653, 2),
    ("Montréal", -73.567, 45.509, 2), ("Vancouver", -123.121, 49.283, 2), ("rural Québec", -71.0, 46.4, 15),
    ("Paris", 2.349, 48.853, 2), ("rural France", 2.4, 46.8, 15), ("London", -0.128, 51.507, 2),
    ("rural England", -2.0, 51.2, 15), ("Dublin", -6.26, 53.35, 2), ("Madrid", -3.704, 40.417, 2),
    ("rural Spain", -4.5, 40.9, 15), ("Lisbon", -9.139, 38.722, 2), ("Tokyo", 139.767, 35.681, 2),
    ("Osaka", 135.502, 34.694, 2), ("rural Japan", 138.2, 36.4, 15), ("Taipei", 121.565, 25.033, 2),
    ("Hong Kong", 114.17, 22.30, 2), ("Singapore", 103.851, 1.29, 2), ("San Juan", -66.106, 18.466, 2),
    ("Honolulu", -157.858, 21.307, 2),
]

# §4.6's places for the iPad's measurements.
TEST_PLACES = [("Shinjuku", 139.700, 35.690), ("Midtown Manhattan", -73.985, 40.758), ("Paris (Châtelet)", 2.347, 48.858),
               ("Hong Kong, the Mid-Levels", 114.150, 22.279), ("Monaco", 7.420, 43.737), ("Woodstock, Vermont", -72.519, 43.624),
               ("Takayama", 137.252, 36.141)]

B_COLS = ["id", "sources", "height", "num_floors", "min_height", "min_floor", "class", "subtype",
          "roof_shape", "roof_color", "has_parts", "is_underground", "geometry"]
P_COLS = ["id", "sources", "height", "num_floors", "min_height", "min_floor", "roof_shape", "roof_color",
          "is_underground", "geometry"]
# The spool's columns. flags: 1 has parts, 2 underground, 4 roof colour, 8 a part, 16 only within the
# coverage's 1 km.
SPOOL = [("t6", pa.uint16()), ("own", pa.bool_()), ("fi", pa.uint8()), ("rg", pa.uint16()), ("row", pa.uint32()),
         ("lon7", pa.int32()), ("lat7", pa.int32()), ("area", pa.float32()), ("nv", pa.uint32()), ("nr", pa.uint16()),
         ("gb14", pa.uint32()), ("gb13", pa.uint32()), ("gb12", pa.uint32()), ("h", pa.float32()), ("nf", pa.int16()),
         ("mh", pa.float32()), ("mf", pa.int16()), ("cls", pa.uint8()), ("sub", pa.uint8()), ("roof", pa.uint8()),
         ("flags", pa.uint8()), ("hsrc", pa.uint8()), ("gsrc", pa.uint8()), ("reg", pa.uint8()), ("hid", pa.uint32())]
NP = {pa.bool_(): np.bool_, pa.uint8(): np.uint8, pa.uint16(): np.uint16, pa.uint32(): np.uint32, pa.int16(): np.int16,
      pa.int32(): np.int32, pa.float32(): np.float32}
DICTS = ("datasets", "classes", "subtypes", "roofs")


def log(msg: str) -> None:
    print(f"{time.strftime('%Y-%m-%d %H:%M:%S')} {msg}", flush=True)


def world(lon, lat):
    """Web Mercator, in world units (0-1, y down)."""
    s = np.sin(np.radians(np.clip(lat, -85.05112878, 85.05112878)))
    return (np.asarray(lon) + 180.0) / 360.0, 0.5 - np.log((1 + s) / (1 - s)) / (4 * np.pi)


def wait_agent(path: str | None) -> None:
    """Waits while the build agent has a job (its status.json, read only)."""
    if not path:
        return
    said = False
    while True:
        try:
            d = json.loads(Path(path).read_text())
            busy = d.get("job") or d.get("beside")
        except (OSError, ValueError):
            busy = None
        if not busy:
            if said:
                log("the agent's job is done; going on")
            return
        if not said:
            log(f"the agent has a job ({json.dumps(busy)[:120]}); paused")
            said = True
        time.sleep(60)


# ---- the coverage -----------------------------------------------------------------------------------

def load_regions(args, work: Path):
    """The regions' names, outlines (shapely, one per region) and countries."""
    path = work / "regions.geojson"
    if not path.exists():
        if args.regions:
            shutil.copyfile(args.regions, path)
        else:
            server = args.server.rstrip("/")

            def get(url):
                req = urllib.request.Request(url, headers={"User-Agent": UA})
                with urllib.request.urlopen(req, timeout=120) as r:
                    return json.loads(r.read())

            feats = []
            for r in get(f"{server}/api/regions")["regions"]:
                for entry in r["outline"]:
                    kind, _, arg = entry.partition(":")
                    if kind != "osm":
                        raise SystemExit(f"{r['id']}: an outline entry this doesn't read: {entry}")
                    d = get(f"{server}/api/areas/{arg}")
                    p = d["properties"]
                    feats.append({"type": "Feature", "geometry": d["geometry"],
                                  "properties": {"region": r["id"], "area": int(arg), "iso": p.get("iso"), "in": p.get("in")}})
            path.write_text(json.dumps({"type": "FeatureCollection", "features": feats}))
    d = json.loads(path.read_text())
    parts, country = {}, {}
    for f in d["features"]:
        p = f["properties"]
        parts.setdefault(p["region"], []).append(shapely.make_valid(shapely.geometry.shape(f["geometry"])))
        country[p["region"]] = SPECIAL.get(p["region"]) or p.get("in") or (p.get("iso") or "??")[:2]
    names = sorted(parts)
    return names, [shapely.union_all(parts[k]) for k in names], [country[k] for k in names]


def build_grid(geoms) -> dict:
    """Per 1° cell: [(region code, the region's piece in the cell, the piece grown by 1 km, whether
    it fills the cell)], the regions coded from 1. The 1 km in a frame scaled by the cell's cos(lat)."""
    grid: dict[tuple[int, int], list] = {}
    for code, g in enumerate(geoms, 1):
        w, s, e, n = g.bounds
        for iy in range(max(-90, math.floor(s - 0.1)), min(89, math.floor(n + 0.1)) + 1):
            c = math.cos(math.radians(min(85.0, iy + 0.5 if iy >= 0 else -(iy + 0.5))))
            hi = math.cos(math.radians(min(85.0, max(abs(iy), abs(iy + 1)))))
            gx, gy = 1.3 * COVER_M / (DEG_M * hi), 1.3 * COVER_M / DEG_M
            for ix in range(math.floor(w - 0.1), math.floor(e + 0.1) + 1):
                grown = shapely.clip_by_rect(g, ix - gx, iy - gy, ix + 1 + gx, iy + 1 + gy)
                if grown.is_empty:
                    continue
                piece = shapely.clip_by_rect(grown, ix, iy, ix + 1, iy + 1)
                if not piece.is_empty and piece.area >= 1.0 - 1e-9:
                    grid.setdefault((ix, iy), []).insert(0, (code, None, None, True))
                    continue
                scaled = shapely.transform(grown, lambda xy: xy * np.array([c, 1.0]))
                buf = shapely.transform(scaled.buffer(COVER_M / DEG_M, quad_segs=4), lambda xy: xy / np.array([c, 1.0]))
                buf = shapely.clip_by_rect(buf, ix, iy, ix + 1, iy + 1)
                if buf.is_empty:
                    continue
                grid.setdefault((ix, iy), []).append((code, None if piece.is_empty else piece, buf, False))
    return grid


GRID: dict = {}


def init_worker(grid_path: str) -> None:
    """A scan worker: the coverage's grid, prepared."""
    global GRID
    GRID = pickle.loads(Path(grid_path).read_bytes())
    for entries in GRID.values():
        for _, piece, buf, _ in entries:
            if piece is not None:
                shapely.prepare(piece)
            if buf is not None:
                shapely.prepare(buf)


def assign(lon: np.ndarray, lat: np.ndarray):
    """Each point's region code (0: outside the coverage + 1 km) and whether it's only in the 1 km."""
    n = len(lon)
    reg, inbuf = np.zeros(n, np.uint8), np.zeros(n, bool)
    ok = np.flatnonzero(np.isfinite(lon) & np.isfinite(lat))
    if not len(ok):
        return reg, inbuf
    ix, iy = np.floor(lon[ok]).astype(np.int64), np.floor(lat[ok]).astype(np.int64)
    key = (ix + 1000) * 1000 + (iy + 500)
    order = np.argsort(key, kind="stable")
    cuts = np.flatnonzero(np.diff(key[order])) + 1
    for part in np.split(order, cuts):
        entries = GRID.get((int(ix[part[0]]), int(iy[part[0]])))
        if not entries:
            continue
        sel = ok[part]
        if entries[0][3]:
            reg[sel] = entries[0][0]
            continue
        x, y = lon[sel], lat[sel]
        left = np.ones(len(sel), bool)
        for stage in (1, 2):
            for code, piece, buf, _ in entries:
                g = piece if stage == 1 else buf
                if g is None:
                    continue
                li = np.flatnonzero(left)
                if not len(li):
                    break
                hit = li[shapely.intersects_xy(g, x[li], y[li])]
                reg[sel[hit]] = code
                inbuf[sel[hit]] = stage == 2
                left[hit] = False
    return reg, inbuf


def rg_meets(grid: dict, b, box) -> bool:
    """Whether a row group's box (w, s, e, n) meets the coverage + 1 km (and --box)."""
    w, s, e, n = b
    if box and (e < box[0] or w > box[2] or n < box[1] or s > box[3]):
        return False
    rect = shapely.box(w, s, e, n)
    for ix in range(math.floor(w), math.floor(e) + 1):
        for iy in range(math.floor(s), math.floor(n) + 1):
            for _, _, buf, full in grid.get((ix, iy), ()):
                if full or buf.intersects(rect):
                    return True
    return False


# ---- geometry as the tiles have it ------------------------------------------------------------------

def vlen(u):
    """The bytes of each unsigned varint."""
    return 1 + (u >= 1 << 7).astype(np.int64) + (u >= 1 << 14) + (u >= 1 << 21) + (u >= 1 << 28)


def zz(d):
    return (d << 1) ^ (d >> 63)


def quantize(geoms, wx, wy, z: int, simplify: bool):
    """Each geometry's rings as a zoom-z tile of §3.4 has them: in the tile of its centroid (wx, wy:
    world units), on the tile's 4096 grid, each ring's closing point and repeated points dropped, a
    ring of fewer than 3 points dropped (and a hole whose outer ring is); simplified to one grid
    unit first when `simplify`. Returns the points (X, Y: tile coordinates; their feature and ring,
    and which are kept) and the rings (feature, exterior, kept, points kept)."""
    scale = float(1 << z) * EXTENT
    tx = np.floor(np.asarray(wx) * (1 << z)).astype(np.int64)
    ty = np.floor(np.asarray(wy) * (1 << z)).astype(np.int64)

    def to_px(c):
        x, y = world(c[:, 0], c[:, 1])
        return np.column_stack([x * scale, y * scale])

    g = shapely.transform(geoms, to_px)
    if simplify:
        g = shapely.simplify(g, 1.0, preserve_topology=False)
    parts, pi = shapely.get_parts(g, return_index=True)
    rings, ri = shapely.get_rings(parts, return_index=True)
    xy, ci = shapely.get_coordinates(rings, return_index=True)
    rfeat = pi[ri]
    rext = np.ones(len(ri), bool)
    rext[1:] = ri[1:] != ri[:-1]
    pf = rfeat[ci]
    X = np.rint(xy[:, 0]).astype(np.int64) - tx[pf] * EXTENT
    Y = np.rint(xy[:, 1]).astype(np.int64) - ty[pf] * EXTENT
    m = len(ci)
    keep = np.ones(m, bool)
    if m:
        keep[np.r_[ci[1:] != ci[:-1], True]] = False
        keep[1:] &= ~((ci[1:] == ci[:-1]) & (X[1:] == X[:-1]) & (Y[1:] == Y[:-1]))
        k = np.flatnonzero(keep)
        if len(k):
            kr = ci[k]
            first = k[np.r_[True, kr[1:] != kr[:-1]]]
            last = k[np.r_[kr[1:] != kr[:-1], True]]
            same = (first != last) & (X[first] == X[last]) & (Y[first] == Y[last])
            keep[last[same]] = False
    cnt = np.bincount(ci[keep], minlength=len(rings))
    ext_of = np.maximum.accumulate(np.where(rext, np.arange(len(rings)), 0)) if len(rings) else np.zeros(0, np.int64)
    rok = (cnt >= 3) & (cnt[ext_of] >= 3)
    keep &= rok[ci] if m else keep
    return X, Y, pf, ci, keep, rfeat, rext, rok, cnt


def geom_bytes(geoms, wx, wy, z: int, simplify: bool):
    """Each geometry's MVT command bytes at zoom z (packed varints, as a tile would hold them), its
    points and its rings."""
    n = len(geoms)
    X, Y, pf, ci, keep, rfeat, rext, rok, cnt = quantize(geoms, wx, wy, z, simplify)
    b = np.zeros(n, np.int64)
    k = np.flatnonzero(keep)
    if len(k):
        Xk, Yk, fk = X[k], Y[k], pf[k]
        dX, dY = np.diff(Xk, prepend=0), np.diff(Yk, prepend=0)
        fs = np.r_[True, fk[1:] != fk[:-1]]
        dX[fs], dY[fs] = Xk[fs], Yk[fs]  # each feature's cursor starts at (0, 0)
        b += np.bincount(fk, weights=vlen(zz(dX)) + vlen(zz(dY)), minlength=n).astype(np.int64)
        rb = 2 + vlen(((cnt.astype(np.int64) - 1) << 3) | 2)  # MoveTo, LineTo(n), ClosePath
        b += np.bincount(rfeat[rok], weights=rb[rok], minlength=n).astype(np.int64)
    return b, np.bincount(pf[keep], minlength=n), np.bincount(rfeat[rok], minlength=n)


# ---- scan --------------------------------------------------------------------------------------------

_PF: dict = {}
HEX = np.zeros(256, np.uint32)
for _i, _c in enumerate(b"0123456789abcdef"):
    HEX[_c] = _i
for _i, _c in enumerate(b"ABCDEF"):
    HEX[_c] = 10 + _i


def _codes(arr) -> tuple[np.ndarray, list]:
    """A string column as codes (-1 null) and its dictionary."""
    d = pc.dictionary_encode(arr)
    if isinstance(d, pa.ChunkedArray):
        d = d.combine_chunks()
    return pc.fill_null(d.indices, -1).to_numpy(zero_copy_only=False).astype(np.int32), d.dictionary.to_pylist()


def scan_rg(job):
    """One row group of a local copy: its buildings (or parts) in the coverage + 1 km, as spool
    columns (local dictionary codes, mapped by the caller), with the copies for the neighbouring
    tiles' margins."""
    path, fi, rg, kind, box = job
    pf = _PF.get(path)
    if pf is None:
        _PF.clear()
        pf = _PF[path] = pq.ParquetFile(path)
    t = pf.read_row_group(rg, columns=B_COLS if kind == "building" else P_COLS)
    n0 = t.num_rows
    geom = shapely.from_wkb(t.column("geometry").to_numpy(zero_copy_only=False), on_invalid="ignore")
    cen = shapely.centroid(geom)
    lon, lat = shapely.get_x(cen), shapely.get_y(cen)
    reg, inbuf = assign(lon, lat)
    keep = reg > 0
    if box:
        keep &= (lon >= box[0]) & (lon <= box[2]) & (lat >= box[1]) & (lat <= box[3])
    keep &= ~shapely.is_empty(geom)
    rows = np.flatnonzero(keep)
    stats = {"rows": n0, "kept": len(rows)}
    if not len(rows):
        return None, stats
    t = t.take(pa.array(rows))
    geom, lon, lat, reg, inbuf = geom[rows], lon[rows], lat[rows], reg[rows], inbuf[rows]
    n = len(rows)
    wx, wy = world(lon, lat)
    area = shapely.area(geom) * DEG_M * DEG_M * np.cos(np.radians(lat))
    gb14, nv, nr = geom_bytes(geom, wx, wy, 14, False)
    gb13 = geom_bytes(geom, wx, wy, 13, True)[0]
    gb12 = geom_bytes(geom, wx, wy, 12, True)[0]

    def num(name, fill):
        return pc.fill_null(t.column(name), fill).to_numpy(zero_copy_only=False)

    h = t.column("height").to_numpy(zero_copy_only=False).astype(np.float32)
    mh = t.column("min_height").to_numpy(zero_copy_only=False).astype(np.float32)
    nf = np.clip(num("num_floors", 0), -32768, 32767).astype(np.int16)
    mf = np.clip(num("min_floor", 0), -32768, 32767).astype(np.int16)
    flags = np.where(num("is_underground", False), 2, 0) | np.where(pc.is_valid(t.column("roof_color")).to_numpy(zero_copy_only=False), 4, 0)
    flags |= np.where(inbuf, 16, 0)
    if kind == "building":
        flags |= np.where(num("has_parts", False), 1, 0)
        cls = _codes(t.column("class"))
        sub = _codes(t.column("subtype"))
    else:
        flags |= 8
        cls = sub = (np.full(n, -1, np.int32), [])
    roof = _codes(t.column("roof_shape"))
    # Sources: the footprint's (property "" or none) and the height's ("/properties/height", else
    # the footprint's).
    s = t.column("sources").combine_chunks()
    flat = pc.list_flatten(s)
    parent = pc.list_parent_indices(s).to_numpy()
    prop = flat.field("property")
    di, dd = _codes(flat.field("dataset"))
    is_h = pc.fill_null(pc.equal(prop, "/properties/height"), False).to_numpy(zero_copy_only=False)
    is_p = pc.fill_null(pc.or_kleene(pc.is_null(prop), pc.equal(prop, "")), False).to_numpy(zero_copy_only=False)
    gsrc = np.full(n, -1, np.int32)
    gsrc[parent[is_p][::-1]] = di[is_p][::-1]
    hs = np.full(n, -1, np.int32)
    hs[parent[is_h][::-1]] = di[is_h][::-1]
    hsrc = np.where(np.isnan(h), -1, np.where(hs >= 0, hs, gsrc))
    vc = pc.value_counts(prop)
    stats["props"] = Counter({str(k): int(v) for k, v in zip(vc.field("values").to_pylist(), vc.field("counts").to_pylist())})
    ids = pc.utf8_slice_codeunits(t.column("id"), -8).to_numpy(zero_copy_only=False).astype("S8")
    hx = HEX[np.frombuffer(ids.tobytes(), np.uint8).reshape(-1, 8)]
    hid = (hx << np.arange(28, -1, -4, dtype=np.uint32)).sum(axis=1, dtype=np.uint64).astype(np.uint32)

    # The z6 tile, and the copies for the neighbours within EDGE_M of its edges.
    x6, y6 = np.floor(wx * 64).astype(np.int64), np.floor(wy * 64).astype(np.int64)
    mpu = EQ * np.cos(np.radians(lat))  # metres a world unit, here
    dw, de = (wx - x6 / 64) * mpu, ((x6 + 1) / 64 - wx) * mpu
    dn, ds = (wy - y6 / 64) * mpu, ((y6 + 1) / 64 - wy) * mpu
    data = ((h >= H_MIN) & (h <= H_MAX)) | (nf >= 1)
    idx, t6, own = [np.arange(n)], [x6 * 64 + y6], [np.ones(n, bool)]
    for ox, oy, near in ((-1, 0, dw < EDGE_M), (1, 0, de < EDGE_M), (0, -1, dn < EDGE_M), (0, 1, ds < EDGE_M),
                         (-1, -1, (dw < EDGE_M) & (dn < EDGE_M)), (1, -1, (de < EDGE_M) & (dn < EDGE_M)),
                         (-1, 1, (dw < EDGE_M) & (ds < EDGE_M)), (1, 1, (de < EDGE_M) & (ds < EDGE_M))):
        tx, ty = x6 + ox, y6 + oy
        sel = np.flatnonzero(near & data & (tx >= 0) & (tx < 64) & (ty >= 0) & (ty < 64))
        idx.append(sel)
        t6.append(tx[sel] * 64 + ty[sel])
        own.append(np.zeros(len(sel), bool))
    idx = np.concatenate(idx)
    stats["margin"] = int(len(idx) - n)
    out = {
        "t6": np.concatenate(t6).astype(np.uint16), "own": np.concatenate(own), "fi": np.full(len(idx), fi, np.uint8),
        "rg": np.full(len(idx), rg, np.uint16), "row": rows[idx].astype(np.uint32),
        "lon7": np.rint(lon[idx] * 1e7).astype(np.int32), "lat7": np.rint(lat[idx] * 1e7).astype(np.int32),
        "area": area[idx].astype(np.float32), "nv": nv[idx].astype(np.uint32), "nr": np.minimum(nr[idx], 65535).astype(np.uint16),
        "gb14": gb14[idx].astype(np.uint32), "gb13": gb13[idx].astype(np.uint32), "gb12": gb12[idx].astype(np.uint32),
        "h": h[idx], "nf": nf[idx], "mh": mh[idx], "mf": mf[idx], "flags": flags[idx].astype(np.uint8),
        "reg": reg[idx], "hid": hid[idx],
        # local codes, mapped by the caller
        "cls": cls[0][idx], "sub": sub[0][idx], "roof": roof[0][idx], "hsrc": hsrc[idx], "gsrc": gsrc[idx],
    }
    return (out, {"classes": cls[1], "subtypes": sub[1], "roofs": roof[1], "datasets": dd}), stats


def copy_rows(src: Path, dst: Path, md, rgs: list[int]) -> int:
    """The row groups `rgs` of parquet file `src` (and its footer) into a sparse local copy `dst`,
    in sequential reads of up to 16 MB. Returns the bytes read."""
    spans = []
    for i in rgs:
        g = md.row_group(i)
        lo = hi = None
        for c in range(g.num_columns):
            col = g.column(c)
            start = col.dictionary_page_offset if col.has_dictionary_page and col.dictionary_page_offset else col.data_page_offset
            lo = start if lo is None else min(lo, start)
            hi = start + col.total_compressed_size if hi is None else max(hi, start + col.total_compressed_size)
        spans.append([lo, hi])
    spans.sort()
    merged = []
    for lo, hi in spans:
        if merged and lo <= merged[-1][1] + (1 << 20):
            merged[-1][1] = max(merged[-1][1], hi)
        else:
            merged.append([lo, hi])
    size = src.stat().st_size
    read = 0
    tmp = dst.with_name(dst.name + ".tmp")
    with src.open("rb", buffering=0) as f, tmp.open("wb") as o:
        o.truncate(size)
        f.seek(size - 8)
        tail = f.read(8)
        foot = int.from_bytes(tail[:4], "little") + 8
        merged.append([0, 4])
        merged.append([size - foot, size])
        for lo, hi in merged:
            f.seek(lo)
            o.seek(lo)
            left = hi - lo
            while left:
                b = f.read(min(16 << 20, left))
                if not b:
                    raise OSError(f"{src}: short read at {lo}")
                o.write(b)
                left -= len(b)
                read += len(b)
    tmp.rename(dst)
    return read


def phase_scan(args, work: Path, root: Path, names, countries) -> None:
    spool = work / "spool"
    (spool / ".done").mkdir(parents=True, exist_ok=True)
    cache = work / "cache"
    cache.mkdir(exist_ok=True)
    for f in cache.iterdir():
        f.unlink()
    # GHSL's tiles, copied whole.
    gdir = work / "ghsl"
    gdir.mkdir(exist_ok=True)
    t0 = time.time()
    box = [float(v) for v in args.box.split(",")] if args.box else None
    index = json.loads((root / GHSL_DIR / "index.json").read_text())["tiles"]
    for name, e in sorted(index.items()):
        b = e["bbox"]
        if box and (b[2] < box[0] or b[0] > box[2] or b[3] < box[1] or b[1] > box[3]):
            continue
        z = root / GHSL_DIR / name
        dst = gdir / z.name
        if not dst.exists() or dst.stat().st_size != z.stat().st_size:
            shutil.copyfile(z, dst.with_name(dst.name + ".tmp"))
            (dst.with_name(dst.name + ".tmp")).rename(dst)
    log(f"ghsl: {len(list(gdir.glob('*.zip')))} tiles here ({time.time() - t0:.0f} s)")

    grid_path = work / "grid.pkl"
    if not grid_path.exists():
        t0 = time.time()
        _, geoms, _ = load_regions(args, work)
        grid = build_grid(geoms)
        grid_path.write_bytes(pickle.dumps(grid))
        log(f"coverage grid: {len(grid)} cells ({time.time() - t0:.0f} s)")
    grid = pickle.loads(grid_path.read_bytes())

    base = root / OVERTURE
    listed = json.loads((base / "buildings.json").read_text())["files"]
    files = sorted(listed)
    dicts_path = work / "dicts.json"
    dicts = json.loads(dicts_path.read_text()) if dicts_path.exists() else {k: [None] for k in DICTS}
    scan_path = work / "scan.json"
    scan = json.loads(scan_path.read_text()) if scan_path.exists() else {"files": {}}
    todo = [(fi, f) for fi, f in enumerate(files) if not (spool / ".done" / f"{fi:03d}").exists()]
    log(f"scan: {len(files)} files, {len(todo)} to scan")
    if not todo:
        return

    def prepare(item):
        fi, rel = item
        src = base / rel
        md = pq.read_metadata(src)
        rgs = []
        for g in range(md.num_row_groups):
            st = {}
            rgm = md.row_group(g)
            for c in range(rgm.num_columns):
                p = rgm.column(c).path_in_schema
                if p in ("bbox.xmin", "bbox.ymin", "bbox.xmax", "bbox.ymax"):
                    s = rgm.column(c).statistics
                    st[p] = (s.min, s.max) if s is not None and s.has_min_max else None
            b = (-180.0, -90.0, 180.0, 90.0) if None in st.values() or len(st) < 4 else \
                (st["bbox.xmin"][0], st["bbox.ymin"][0], st["bbox.xmax"][1], st["bbox.ymax"][1])
            if rg_meets(grid, b, box):
                rgs.append(g)
        t = time.time()
        local = cache / f"{fi:03d}.parquet"
        nbytes = copy_rows(src, local, md, rgs) if rgs else 0
        return fi, rel, local, rgs, md.num_row_groups, nbytes, time.time() - t

    t_all = time.time()
    with ProcessPoolExecutor(args.jobs, initializer=init_worker, initargs=(str(grid_path),)) as pool, ThreadPoolExecutor(1) as copier:
        ahead = copier.submit(prepare, todo[0])
        for k in range(len(todo)):
            fi, rel, local, rgs, nrg, nbytes, dt_copy = ahead.result()
            if k + 1 < len(todo):
                wait_agent(args.agent_status)
                ahead = copier.submit(prepare, todo[k + 1])
            t1 = time.time()
            kind = "building_part" if "type=building_part/" in rel else "building"
            outs, st = [], Counter()
            props = Counter()
            if rgs:
                for res, s in pool.map(scan_rg, [(str(local), fi, g, "building" if kind == "building" else "part", box) for g in rgs], chunksize=2):
                    props.update(s.pop("props", {}))
                    st.update(s)
                    if res is not None:
                        outs.append(res)
            # Local dictionary codes to the global ones (0: none).
            cols = {}
            for name, _ in SPOOL:
                if name in ("cls", "sub", "roof", "hsrc", "gsrc"):
                    continue
                cols[name] = np.concatenate([o[0][name] for o in outs]) if outs else np.zeros(0)
            for name, dname, kname in (("cls", "classes", "classes"), ("sub", "subtypes", "subtypes"),
                                       ("roof", "roofs", "roofs"), ("hsrc", "datasets", "datasets"), ("gsrc", "datasets", "datasets")):
                parts = []
                for o, ds in outs:
                    local_names = ds[kname]
                    table = dicts[dname]
                    m = np.zeros(len(local_names) + 1, np.uint8)
                    for i, s_ in enumerate(local_names):
                        if s_ not in table:
                            table.append(s_)
                        if table.index(s_) >= 255:
                            raise SystemExit(f"more than 254 {dname}")
                        m[i] = table.index(s_)
                    parts.append(m[o[name]])  # -1 is the last entry: 0
                cols[name] = np.concatenate(parts) if parts else np.zeros(0, np.uint8)
            dicts_path.write_text(json.dumps(dicts))
            # Into the spool by z6 tile.
            if outs:
                order = np.argsort(cols["t6"], kind="stable")
                t6s = cols["t6"][order]
                cuts = np.flatnonzero(np.diff(t6s)) + 1
                for sel in np.split(order, cuts):
                    t6 = int(cols["t6"][sel[0]])
                    d = spool / f"6-{t6 // 64}-{t6 % 64}"
                    d.mkdir(exist_ok=True)
                    table = pa.table({name: pa.array(cols[name][sel].astype(NP[typ]), type=typ) for name, typ in SPOOL})
                    tmp = d / f"{fi:03d}.parquet.tmp"
                    pq.write_table(table, tmp, compression="zstd", compression_level=3, use_dictionary=False,
                                   column_encoding={"row": "DELTA_BINARY_PACKED", "lon7": "DELTA_BINARY_PACKED", "lat7": "DELTA_BINARY_PACKED"})
                    tmp.rename(d / f"{fi:03d}.parquet")
            local.unlink(missing_ok=True)
            scan["files"][rel] = {"fi": fi, "row_groups": nrg, "read": len(rgs), "bytes": nbytes, "copy_s": round(dt_copy, 1),
                                  "decode_s": round(time.time() - t1, 1), **{k: int(v) for k, v in st.items()}}
            scan.setdefault("props", {})
            for p_, v in props.items():
                scan["props"][p_] = scan["props"].get(p_, 0) + v
            scan_path.write_text(json.dumps(scan, indent=1))
            (spool / ".done" / f"{fi:03d}").touch()
            log(f"scan {k + 1}/{len(todo)} {rel.rsplit('/', 1)[-1][:10]}: {len(rgs)}/{nrg} row groups, {nbytes / 1e6:.0f} MB "
                f"in {dt_copy:.0f} s, {st['kept']:,} of {st['rows']:,} kept (+{st['margin']:,} margin) in {time.time() - t1:.0f} s")
    scan["seconds"] = scan.get("seconds", 0) + time.time() - t_all
    scan_path.write_text(json.dumps(scan, indent=1))


# ---- fit ---------------------------------------------------------------------------------------------

def context(work: Path):
    """What the fill and the report need: regions, countries, codes."""
    names, _, countries = load_regions(argparse.Namespace(regions=None, server=None), work)
    clist = sorted(set(countries))
    regc = np.array([0] + [clist.index(c) for c in countries], np.int64)
    dicts = json.loads((work / "dicts.json").read_text())
    return names, countries, clist, regc, dicts


def size_bins(area, cls, sub, dicts):
    """§2.3 rule 4's class of each building (SIZE_BINS)."""
    classes, subtypes = dicts["classes"], dicts["subtypes"]
    church = np.isin(cls, [i for i, c in enumerate(classes) if c in CHURCHES])
    shed = np.isin(cls, [i for i, c in enumerate(classes) if c in SHEDS])
    house = np.isin(cls, [i for i, c in enumerate(classes) if c in HOUSES]) | np.isin(sub, [i for i, c in enumerate(subtypes) if c == "residential"])
    return np.where(church, 0, np.where(shed | (area < 30), 1, np.where(house | (area < 250), 2, np.where(area < 2000, 3, 4)))).astype(np.uint8)


def lad(h: np.ndarray, f: np.ndarray):
    """h ≈ a·f + b by least absolute deviations: a on a grid (then finer), b the median residual."""
    if len(h) > 3_000_000:
        sel = np.random.default_rng(0).choice(len(h), 3_000_000, replace=False)
        h, f = h[sel], f[sel]

    def cost(a):
        r = h - a * f
        b = float(np.median(r))
        return float(np.mean(np.abs(r - b))), b

    grid = np.arange(0.5, 6.0001, 0.05)
    best = min(grid, key=lambda a: cost(a)[0])
    fine = np.arange(best - 0.05, best + 0.0501, 0.005)
    a = float(min(fine, key=lambda a: cost(a)[0]))
    return a, cost(a)[1], cost(a)[0]


def estimates(dicts) -> list[int]:
    """The dataset codes whose heights are estimates (ESTIMATES), not measurements."""
    return [i for i, d in enumerate(dicts["datasets"]) if d in ESTIMATES]


def phase_fit(work: Path) -> None:
    """Rule 1's storey height per country, from the heights measured (Microsoft's estimates left
    out: they grow ~1.5 m a floor where OSM's and lidar's grow ~3), and as the plan first had it
    (every height); the median height of each floor count; rule 4's classes' median heights."""
    names, countries, clist, regc, dicts = context(work)
    est = estimates(dicts)
    hs, fs, cs, srcs = [], [], [], []
    hist = np.zeros((2, len(clist), 5, 7001), np.int64)
    files = sorted(work.glob("spool/6-*/*.parquet"))
    t0 = time.time()
    for f in files:
        t = pq.read_table(f, columns=["own", "flags", "h", "nf", "reg", "hid", "area", "cls", "sub", "hsrc"])
        c = {k: t.column(k).to_numpy() for k in t.column_names}
        ok = c["own"] & (c["flags"] & 10 == 0) & (c["h"] >= H_MIN) & (c["h"] <= H_MAX) & (c["hid"] % 10 != 0)
        both = ok & (c["nf"] >= 1) & (c["nf"] <= F_MAX)
        hs.append(c["h"][both])
        fs.append(c["nf"][both].astype(np.float32))
        cs.append(regc[c["reg"][both]])
        srcs.append(c["hsrc"][both])
        cc = regc[c["reg"][ok]]
        b = size_bins(c["area"][ok], c["cls"][ok], c["sub"][ok], dicts)
        hd = np.rint(c["h"][ok] * 10).astype(np.int64)
        meas = ~np.isin(c["hsrc"][ok], est)
        hist[0] += np.bincount((cc * 5 + b) * 7001 + hd, minlength=hist[0].size).reshape(hist[0].shape)
        hist[1] += np.bincount((cc[meas] * 5 + b[meas]) * 7001 + hd[meas], minlength=hist[1].size).reshape(hist[1].shape)
    h, f, cc, src = np.concatenate(hs), np.concatenate(fs), np.concatenate(cs), np.concatenate(srcs)
    meas = ~np.isin(src, est)
    log(f"fit: {len(h):,} buildings with a height and floors, {meas.sum():,} of them measured ({time.time() - t0:.0f} s)")

    def per_floor(hh, ff):
        out = {}
        for label, lo, hi in (("1", 1, 1), ("2", 2, 2), ("3-4", 3, 4), ("5-9", 5, 9), ("10+", 10, F_MAX)):
            m = (ff >= lo) & (ff <= hi)
            out[label] = [int(m.sum()), round(float(np.median(hh[m] / ff[m])), 2) if m.any() else None]
        return out

    def table(hh, ff):
        """The median height of the buildings of 1, 2, … 12 floors (None under 30 of them)."""
        return [round(float(np.median(hh[ff == k])), 2) if (ff == k).sum() >= 30 else None for k in range(1, 13)]

    def fits(sel):
        a, b, mae = lad(h[sel], f[sel])
        out = {"*": {"n": int(sel.sum()), "a": round(a, 3), "b": round(b, 2), "mae": round(mae, 2), "floors": per_floor(h[sel], f[sel]),
                     "table": table(h[sel], f[sel])}}
        for i, c_ in enumerate(clist):
            m = sel & (cc == i)
            e = {"n": int(m.sum()), "floors": per_floor(h[m], f[m]), "table": table(h[m], f[m])}
            if m.sum() >= 500:
                a, b, mae = lad(h[m], f[m])
                e.update(a=round(a, 3), b=round(b, 2), mae=round(mae, 2))
            out[c_] = e
        return out

    fit, fit_all = fits(meas), fits(np.ones(len(h), bool))
    by_src = {}
    for i, c_ in enumerate(["*"] + clist):
        for s_ in np.unique(src):
            m = (src == s_) & ((cc == i - 1) if i else True)
            if m.sum() >= 1000:
                a, b, mae = lad(h[m], f[m])
                by_src.setdefault(c_, {})[dicts["datasets"][s_] if s_ else "none"] = {
                    "n": int(m.sum()), "a": round(a, 3), "b": round(b, 2), "mae": round(mae, 2), "floors": per_floor(h[m], f[m])}

    def med(hh):
        n = hh.sum()
        return None if n == 0 else round(float(np.searchsorted(np.cumsum(hh), (n + 1) // 2)) / 10, 1)

    def sizes(H):
        out = {"*": {"n": [int(H[:, k].sum()) for k in range(5)], "median": [med(H[:, k].sum(axis=0)) for k in range(5)]}}
        for i, c_ in enumerate(clist):
            out[c_] = {"n": [int(H[i, k].sum()) for k in range(5)], "median": [med(H[i, k]) for k in range(5)]}
        return out

    (work / "fit.json").write_text(json.dumps({"estimates": sorted(ESTIMATES), "storey": fit, "storey_all": fit_all,
                                              "storey_by_source": by_src, "size": sizes(hist[1]), "size_all": sizes(hist[0])}, indent=1))
    log(f"fit: {fit['*']['a']} m a floor + {fit['*']['b']} m from the measured heights "
        f"({fit_all['*']['a']} m + {fit_all['*']['b']} m from all); written ({time.time() - t0:.0f} s)")


# ---- fill --------------------------------------------------------------------------------------------

def ghsl_sample(lon, lat, gdir: Path) -> np.ndarray:
    """GHSL's ANBH (m; 0 where the cell has no buildings or there's no tile) at each point, read in
    1° windows."""
    import rasterio
    from rasterio.windows import Window
    out = np.zeros(len(lon), np.float32)
    if not len(lon):
        return out
    R = np.floor((GHSL_TOP - lat) / GHSL_DEG).astype(np.int64) + 1
    C = np.floor((lon - GHSL_LEFT) / GHSL_DEG).astype(np.int64) + 1
    for k in np.unique(R * 100 + C):
        r, c = divmod(int(k), 100)
        sel = np.flatnonzero((R == r) & (C == c))
        z = gdir / f"{GHSL_PRODUCT}_R{r}_C{c}.zip"
        if not z.exists():
            continue
        with rasterio.open(f"/vsizip/{z}/{GHSL_PRODUCT}_R{r}_C{c}.tif") as src:
            T = src.transform
            col = np.clip(np.floor((lon[sel] - T.c) / T.a).astype(np.int64), 0, src.width - 1)
            row = np.clip(np.floor((lat[sel] - T.f) / T.e).astype(np.int64), 0, src.height - 1)
            blk = (row // 1200) * 100 + col // 1200
            for bk in np.unique(blk):
                s2 = np.flatnonzero(blk == bk)
                r0, c0 = int(row[s2].min()), int(col[s2].min())
                r1, c1 = int(row[s2].max()) + 1, int(col[s2].max()) + 1
                w = src.read(1, window=Window(c0, r0, c1 - c0, r1 - r0))
                out[sel[s2]] = w[row[s2] - r0, col[s2] - c0]
    return np.where(np.isfinite(out) & (out > 0), out, 0).astype(np.float32)


def grouped_median(i, h, n: int):
    """Per group 0..n-1: the count of its h, and their median (the lower middle; h < 16384)."""
    cnt, med = np.zeros(n, np.int64), np.zeros(n, np.int64)
    if len(i):
        key = (i.astype(np.int64) << 14) | h.astype(np.int64)
        key.sort()
        g = key >> 14
        st = np.flatnonzero(np.r_[True, g[1:] != g[:-1]])
        k = np.diff(np.r_[st, len(key)])
        cnt[g[st]] = k
        med[g[st]] = key[st + (k - 1) // 2] & 0x3FFF
    return cnt, med


def near_pairs(tree, qX, qY, qcos, radius: float):
    """(query, data point, metres) for the data points within `radius` metres of each query: in
    Mercator metres (EQ a world unit), a ground metre being cos(lat) of one."""
    from scipy.spatial import cKDTree
    qt = cKDTree(np.column_stack([qX, qY]))
    m = qt.sparse_distance_matrix(tree, radius / max(float(qcos.min()), 0.02), output_type="ndarray")
    i, j = m["i"], m["j"]
    d = m["v"] * qcos[i]
    ok = d <= radius
    return i[ok], j[ok], d[ok]


def rule2(q, d, held: bool):
    """§2.3's rule 2 for the queries q from the data d (dicts of X, Y, cos, area, h (dm), id).
    Returns per query: the height (dm, 0 none), the stage (1: 150 m and similar footprints, 2: 300 m),
    the 150 m count, and the variant without the footprints' filter (150 m, 5 or more)."""
    from scipy.spatial import cKDTree
    nq = len(q["X"])
    out = {"p": np.zeros(nq, np.int64), "st": np.zeros(nq, np.uint8), "nA": np.zeros(nq, np.int64), "pv": np.zeros(nq, np.int64)}
    if not nq or not len(d["X"]):
        return out
    tree = cKDTree(np.column_stack([d["X"], d["Y"]]))
    start, size, pairs_total = 0, 5000, 0
    while start < nq:
        sl = slice(start, min(nq, start + size))
        i, j, dist = near_pairs(tree, q["X"][sl], q["Y"][sl], q["cos"][sl], 300.0 if held else 150.0)
        if held:
            ok = d["id"][j] != q["id"][sl][i]
            i, j, dist = i[ok], j[ok], dist[ok]
        qa, da, h = q["area"][sl][i], d["area"][j], d["h"][j]
        a = (dist <= 150) & (da >= 0.5 * qa) & (da <= 2 * qa)
        n = sl.stop - sl.start
        nA, mA = grouped_median(i[a], h[a], n)
        out["nA"][sl] = nA
        okA = nA >= 5
        out["p"][sl] = np.where(okA, mA, 0)
        out["st"][sl] = np.where(okA, 1, 0)
        if held:
            nB, mB = grouped_median(i, h, n)
            v = dist <= 150
            nV, mV = grouped_median(i[v], h[v], n)
            out["pv"][sl] = np.where(nV >= 5, mV, 0)
        else:
            # The 300 m stage only for the queries the first didn't answer.
            rest = np.flatnonzero(~okA)
            nB, mB = np.zeros(n, np.int64), np.zeros(n, np.int64)
            if len(rest):
                i2, j2, _ = near_pairs(tree, q["X"][sl][rest], q["Y"][sl][rest], q["cos"][sl][rest], 300.0)
                nb, mb = grouped_median(i2, d["h"][j2], len(rest))
                nB[rest], mB[rest] = nb, mb
                pairs_total += len(i2)
        okB = ~okA & (nB >= 8)
        out["p"][sl] = np.where(okB, mB, out["p"][sl])
        out["st"][sl] = np.where(okB, 2, out["st"][sl])
        pairs_total += len(i)
        start = sl.stop
        size = int(min(100_000, max(1000, 6e6 / max(1.0, len(i) / n))))
    out["pairs"] = pairs_total
    return out


def morton(wx, wy, bits: int = 24):
    def spread(v):
        v = v.astype(np.uint64) & np.uint64(0xFFFFFFFF)
        for s, m in ((16, 0x0000FFFF0000FFFF), (8, 0x00FF00FF00FF00FF), (4, 0x0F0F0F0F0F0F0F0F), (2, 0x3333333333333333), (1, 0x5555555555555555)):
            v = (v | (v << np.uint64(s))) & np.uint64(m)
        return v
    s = float(1 << bits)
    return spread(np.floor(np.asarray(wx) * s)) | (spread(np.floor(np.asarray(wy) * s)) << np.uint64(1))


KEEP_TOP, THR = 12, {14: 150_000, 13: 100_000, 12: 60_000}


def fill_tile(job):
    """One z6 tile's fill, hold-out and tile counts (see the module's docstring)."""
    key, files, ctx = job
    t_start = time.time()
    regc, fit_a, fit_b, size_fit, dicts, gdir, want = ctx["regc"], ctx["a"], ctx["b"], ctx["size"], ctx["dicts"], Path(ctx["ghsl"]), ctx.get("want")
    t = pa.concat_tables([pq.read_table(f) for f in files])
    c = {k: t.column(k).to_numpy() for k in t.column_names}
    del t
    nreg = len(regc)
    own, flags = c["own"], c["flags"]
    under, part = (flags & 2) > 0, (flags & 8) > 0
    bld = ~under & ~part
    lon, lat = c["lon7"] / 1e7, c["lat7"] / 1e7
    h, nf, area, reg, hid = c["h"], c["nf"].astype(np.int64), c["area"].astype(np.float64), c["reg"].astype(np.int64), c["hid"]
    cc = regc[reg]
    m0 = (h >= H_MIN) & (h <= H_MAX) & ~under
    f1 = (nf >= 1) & (nf <= F_MAX) & ~under
    h1 = np.clip(fit_a[cc] * nf + fit_b[cc], H_MIN, H_MAX)
    ho = m0 & bld & (hid % 10 == 0)
    tr = m0 & bld & (hid % 10 == 1)
    wx, wy = world(lon, lat)
    X, Y, cosl = wx * EQ, wy * EQ, np.cos(np.radians(lat))
    mo = morton(wx, wy)
    hdm = lambda v: np.clip(np.rint(np.asarray(v) * 10), 0, 16383).astype(np.int64)

    def subset(mask, hv):
        idx = np.flatnonzero(mask)
        idx = idx[np.argsort(mo[idx], kind="stable")]
        return idx, {"X": X[idx], "Y": Y[idx], "cos": cosl[idx], "area": area[idx], "h": hdm(hv[idx]) if hv is not None else None, "id": idx}

    # The hold-out: the held-out heights left out, a held-out building with floors kept by them.
    qi, q = subset(own & ho, None)
    di, d = subset(bld & ((m0 & ~ho) | f1), np.where(m0 & ~ho, h, h1))
    r_ho = rule2(q, d, True)
    del q, d, di
    # The map: every building without a height or floors, from all heights.
    mi, mq = subset(own & bld & ~m0 & ~f1, None)
    di2, d2 = subset(bld & (m0 | f1), np.where(m0, h, h1))
    r_map = rule2(mq, d2, False)
    del mq, d2, di2, X, Y, cosl, mo

    # GHSL and size, for the tile's own buildings.
    oi = np.flatnonzero(own & bld)
    g = np.zeros(len(h), np.float32)
    g[oi] = ghsl_sample(lon[oi], lat[oi], gdir)
    p3 = np.where(g > 0, np.where(area < 60, np.minimum(g, 4.0), g), np.nan)
    bins = size_bins(area, c["cls"], c["sub"], dicts)
    p4 = SIZE_DEFAULT[bins]

    # The map's heights and their sources.
    p2 = np.zeros(len(h))
    p2[mi] = r_map["p"] / 10.0
    s = np.full(len(h), 4, np.uint8)
    hf = p4.astype(np.float64).copy()
    has3 = np.isfinite(p3)
    hf[has3], s[has3] = p3[has3], 3
    has2 = p2 > 0
    hf[has2], s[has2] = p2[has2], 2
    hf[f1], s[f1] = h1[f1], 1
    hf[m0], s[m0] = h[m0], 0
    # Parts: measured, else from floors, else 6.5 m.
    hpart = np.where(m0, h, np.where(f1, h1, 6.5))
    hf = np.where(part, hpart, hf)
    s = np.where(part, np.where(m0, 0, np.where(f1, 1, 4)), s).astype(np.uint8)
    # B0's recommendation (docs/buildings3d.md §2.3), whose heights the tiles here are counted by:
    # 0 measured, 1 floors, 2 Microsoft's estimate, 3 neighbours, 4 GHSL where its cell is 20 m or
    # more, 5 size and kind with the defaults fitted per country.
    ml = m0 & np.isin(c["hsrc"], ctx["est"])
    p4r = size_fit[cc, bins]
    sr = np.full(len(h), 5, np.uint8)
    hr = p4r.astype(np.float64).copy()
    tall3 = has3 & (g >= GHSL_TALL)
    hr[tall3], sr[tall3] = p3[tall3], 4
    hr[has2], sr[has2] = p2[has2], 3
    hr[ml], sr[ml] = h[ml], 2
    hr[f1], sr[f1] = h1[f1], 1
    m0m = m0 & ~ml
    hr[m0m], sr[m0m] = h[m0m], 0
    hr = np.where(part, hpart, hr)
    sr = np.where(part, np.where(m0, 0, np.where(f1, 1, 5)), sr).astype(np.uint8)
    hf_plan, s_plan = hf, s
    hf, s = hr, sr

    out = {"key": key, "n": int(own.sum()), "pairs": [r_ho.get("pairs", 0), r_map.get("pairs", 0)]}
    # Counts per region.
    feat = own & ~under
    ob = own & bld
    def rc(mask):
        return np.bincount(reg[mask], minlength=nreg).astype(np.int64)
    out["count"] = {
        "n": rc(ob), "meas": rc(ob & m0), "floors": rc(ob & f1), "both": rc(ob & m0 & f1), "either": rc(ob & (m0 | f1)),
        "h_out": rc(ob & ~np.isnan(h) & ~m0), "roof": rc(ob & (c["roof"] > 0)), "roofc": rc(ob & ((flags & 4) > 0)),
        "has_parts": rc(ob & ((flags & 1) > 0)), "inbuf": rc(ob & ((flags & 16) > 0)), "under": rc(own & under),
        "parts": rc(own & part & ~under), "parts_h": rc(own & part & m0), "parts_base": rc(own & part & (c["mh"] > 0)),
        "nv": np.bincount(reg[ob], weights=c["nv"][ob], minlength=nreg).astype(np.int64),
    }
    out["s"] = np.zeros((nreg, 5), np.int64)
    np.add.at(out["s"], (reg[ob], s_plan[ob]), 1)
    out["s_rec"] = np.zeros((nreg, 6), np.int64)
    np.add.at(out["s_rec"], (reg[ob], s[ob]), 1)
    out["map_stage"] = np.bincount(r_map["st"], minlength=3)
    nds = 256
    out["hsrc"] = np.bincount(reg[ob & m0] * nds + c["hsrc"][ob & m0], minlength=nreg * nds).reshape(nreg, nds)
    out["gsrc"] = np.bincount(reg[ob] * nds + c["gsrc"][ob], minlength=nreg * nds).reshape(nreg, nds)
    hk, hn = np.unique(cc[ob & m0] * 8192 + hdm(h[ob & m0]), return_counts=True)
    out["hhist"] = (hk, hn)
    fk, fn = np.unique(cc[ob & f1] * 256 + np.minimum(nf[ob & f1], 255), return_counts=True)
    out["fhist"] = (fk, fn)
    # The hold-out's predictions (own held-out buildings).
    o = own[qi]
    qq = qi[o]
    p1 = np.where(f1[qq], h1[qq], np.nan)
    out["ho"] = {"reg": reg[qq].astype(np.uint8), "hsrc": c["hsrc"][qq], "h": h[qq], "nf": nf[qq].astype(np.int16),
                 "area": area[qq].astype(np.float32), "bin": bins[qq], "g": g[qq], "p1": p1.astype(np.float32),
                 "p2": np.where(r_ho["p"][o] > 0, r_ho["p"][o] / 10.0, np.nan).astype(np.float32), "st": r_ho["st"][o],
                 "nA": np.minimum(r_ho["nA"][o], 65535).astype(np.uint16),
                 "pv": np.where(r_ho["pv"][o] > 0, r_ho["pv"][o] / 10.0, np.nan).astype(np.float32),
                 "p3": p3[qq].astype(np.float32), "p4": p4[qq]}
    ti = np.flatnonzero(own & tr)
    out["tr"] = {"reg": reg[ti].astype(np.uint8), "h": h[ti], "nf": nf[ti].astype(np.int16), "area": area[ti].astype(np.float32),
                 "bin": bins[ti], "g": g[ti], "hsrc": c["hsrc"][ti]}

    # The tiles at z12-14.
    x14, y14 = np.floor(wx * (1 << 14)).astype(np.int64), np.floor(wy * (1 << 14)).astype(np.int64)
    fv = np.flatnonzero(feat)
    hfd = hdm(hf)
    kk = np.where(part, 1, np.where((flags & 1) > 0, 2, 0))
    nprops = 3 + (s == 1) + (part & (c["mh"] > 0)) + (kk > 0)
    inc = {14: np.ones(len(h), bool), 13: (hf >= 20) | (area >= 2000), 12: hf >= 40}
    out["tiles"], out["dense"] = {}, {}
    for z in (14, 13, 12):
        sel = fv[inc[z][fv]]
        gb = c[f"gb{z}"][sel].astype(np.int64)
        ovh = 1 + 2 + 2 + (2 * nprops[sel] + 1) + 2 + 1 + vlen(gb)
        tk = ((x14[sel] >> (14 - z)) << 32) | (y14[sel] >> (14 - z))
        u, inv = np.unique(tk, return_inverse=True)
        nfeat = np.bincount(inv, minlength=len(u))
        gsum = np.bincount(inv, weights=gb, minlength=len(u))
        osum = np.bincount(inv, weights=ovh, minlength=len(u))
        uv = np.unique(inv.astype(np.int64) * 16384 + hfd[sel])
        nvals = np.bincount(uv // 16384, minlength=len(u)) + 30
        est = gsum + osum + 5 * nvals + 40
        nv = np.bincount(inv, weights=c["nv"][sel], minlength=len(u))
        out["tiles"][z] = {"key": u, "n": nfeat, "gb": gsum, "ovh": osum, "vals": nvals, "est": est, "nv": nv}
        # The records of the tiles the tiles phase may encode.
        pick = set(np.argsort(-est)[:KEEP_TOP].tolist()) | set(np.argsort(-nfeat)[:KEEP_TOP].tolist())
        pick |= set(np.flatnonzero(est >= THR[z]).tolist())
        tx_, ty_ = u >> 32, u & 0xFFFFFFFF
        pick |= set(np.flatnonzero(((tx_ * 73856093) ^ (ty_ * 19349663)) % {14: 2003, 13: 211, 12: 23}[z] == 0).tolist())
        if want is not None:
            pick = {i for i in range(len(u)) if (z, int(u[i])) in want}
        pick = np.array(sorted(pick), np.int64)
        rs = np.isin(inv, pick)
        r = sel[rs]
        out["dense"][z] = {"tile": u[inv[rs]], "fi": c["fi"][r], "rg": c["rg"][r], "row": c["row"][r], "part": part[r],
                           "h": hfd[r], "m": hdm(np.where(part[r] & (c["mh"][r] > 0), c["mh"][r], 0)), "s": s[r],
                           "f": np.where(s[r] == 1, nf[r], 0), "c": np.array([KIND.get(dicts["subtypes"][v] if v else "", 0) for v in c["sub"][r]], np.uint8),
                           "k": kk[r], "hid": hid[r], "lon7": c["lon7"][r], "lat7": c["lat7"][r]}
    # Places.
    out["places"] = {}
    for name, plon, plat, km in PLACES:
        if not len(lon) or not (lon.min() - 0.3 < plon < lon.max() + 0.3 and lat.min() - 0.3 < plat < lat.max() + 0.3):
            continue
        dx = (lon - plon) * DEG_M * math.cos(math.radians(plat))
        dy = (lat - plat) * DEG_M
        m = ob & (dx * dx + dy * dy <= (km * 1000.0) ** 2)
        if m.any():
            out["places"][name] = [int(m.sum()), int((m & m0).sum()), int((m & f1).sum()), int((m & (m0 | f1)).sum())]
    out["seconds"] = round(time.time() - t_start, 1)
    return out


def size_table(fit: dict, clist: list) -> np.ndarray:
    """Rule 4's fitted defaults per country and class: the measured median where there are 200,
    else the coverage's, else the plan's default."""
    sm = fit["size"]
    return np.array([[sm[c_]["median"][k] if sm[c_]["n"][k] >= 200 and sm[c_]["median"][k] else
                      (sm["*"]["median"][k] if sm["*"]["median"][k] else float(SIZE_DEFAULT[k])) for k in range(5)] for c_ in clist])


def phase_fill(args, work: Path) -> None:
    names, countries, clist, regc, dicts = context(work)
    fit = json.loads((work / "fit.json").read_text())
    st = fit["storey"]
    a = np.array([st[c].get("a", st["*"]["a"]) for c in clist], np.float64)
    b = np.array([st[c].get("b", st["*"]["b"]) for c in clist], np.float64)
    ctx = {"regc": regc, "a": a, "b": b, "size": size_table(fit, clist), "est": estimates(dicts), "dicts": dicts, "ghsl": str(work / "ghsl")}
    out_dir = work / "fill"
    out_dir.mkdir(exist_ok=True)
    tiles = []
    for d in sorted((work / "spool").glob("6-*")):
        files = sorted(d.glob("*.parquet"))
        if files and not (out_dir / f"{d.name}.pkl").exists():
            tiles.append((sum(pq.read_metadata(f).num_rows for f in files), d.name, [str(f) for f in files]))
    tiles.sort(reverse=True)
    log(f"fill: {len(tiles)} tiles to fill, {sum(t[0] for t in tiles):,} records")
    t0 = time.time()

    def mem(rows):  # a tile's peak memory, roughly (measured: ~450 B a record)
        return 0.6e9 + 450 * rows

    with ProcessPoolExecutor(args.fill_jobs) as pool:
        running = {}
        queue = list(tiles)
        while queue or running:
            # The largest tile that fits in --fill-mem-gb beside those running (alone if none fits).
            while queue and len(running) < args.fill_jobs:
                used = sum(mem(r) for _, r in running.values())
                pick = next((k for k, t_ in enumerate(queue) if used + mem(t_[0]) <= args.fill_mem_gb * 1e9), None)
                if pick is None:
                    if running:
                        break
                    pick = 0
                wait_agent(args.agent_status)
                rows, key, files = queue.pop(pick)
                running[pool.submit(fill_tile, (key, files, ctx))] = (key, rows)
            done = next(iter(f for f in list(running) if f.done()), None)
            if done is None:
                time.sleep(1)
                continue
            key, _ = running.pop(done)
            res = done.result()
            tmp = out_dir / f"{key}.pkl.tmp"
            tmp.write_bytes(pickle.dumps(res, protocol=5))
            tmp.rename(out_dir / f"{key}.pkl")
            log(f"fill {key}: {res['n']:,} buildings, {res['pairs'][0] / 1e6:.0f} M + {res['pairs'][1] / 1e6:.0f} M pairs, "
                f"{res['seconds']:.0f} s ({len(queue)} left, {len(running)} running, {time.time() - t0:.0f} s)")
    log(f"fill: done in {time.time() - t0:.0f} s")


# ---- tiles -------------------------------------------------------------------------------------------

def varints(u) -> bytes:
    u = np.asarray(u, np.uint64)
    n = vlen(u.astype(np.int64))
    out = np.zeros((len(u), 5), np.uint8)
    for k in range(5):
        out[:, k] = ((u >> np.uint64(7 * k)) & np.uint64(0x7F)).astype(np.uint8) | ((n > k + 1).astype(np.uint8) << 7)
    return out[np.arange(5)[None, :] < n[:, None]].tobytes()


def pb_len(field: int, payload: bytes) -> bytes:
    return varints([field << 3 | 2]) + varints([len(payload)]) + payload


def encode_tile(z: int, tx: int, ty: int, geoms, rec, simplify: bool | None = None, props: str = "hmsfck") -> tuple[int, int, int]:
    """Tile z/tx/ty as §3.4 has it (MVT 2.1, layer `b`, a feature a building or part, whole, sorted by
    the centroid's Morton code then id; properties h, m, s, f, c, k): its raw and gzip'd bytes, and
    its features. `simplify` (default: below z14) and `props` vary it, to measure the alternatives."""
    wx, wy = world(rec["lon7"] / 1e7, rec["lat7"] / 1e7)
    order = np.lexsort((rec["hid"], morton(wx * (1 << z) - tx, wy * (1 << z) - ty, 12)))
    geoms = geoms[order]
    r = {k: v[order] for k, v in rec.items()}
    wx, wy = wx[order], wy[order]
    X, Y, pf, ci, keep, rfeat, rext, rok, cnt = quantize(geoms, wx, wy, z, z < 14 if simplify is None else simplify)
    k = np.flatnonzero(keep)
    Xk, Yk, ck = X[k], Y[k], ci[k]
    starts = np.r_[0, np.flatnonzero(ck[1:] != ck[:-1]) + 1, len(ck)]
    ring_pts = {int(ck[starts[i]]): (starts[i], starts[i + 1]) for i in range(len(starts) - 1)}
    keys = ["h", "m", "s", "f", "c", "k"]
    values: dict[int, int] = {}

    def vi(v):
        v = int(v)
        if v not in values:
            values[v] = len(values)
        return values[v]

    rings_of: dict[int, list[int]] = {}
    for ri in np.flatnonzero(rok):
        rings_of.setdefault(int(rfeat[ri]), []).append(int(ri))
    body = bytearray()
    nfeat = 0
    for fi in range(len(geoms)):
        rl = rings_of.get(fi)
        if not rl:
            continue
        cmds = []
        cx = cy = 0
        for ri in rl:
            a, b = ring_pts[ri]
            px, py = Xk[a:b], Yk[a:b]
            area2 = int(np.sum(px * np.roll(py, -1) - np.roll(px, -1) * py))
            if (area2 < 0) == bool(rext[ri]):  # exterior positive, holes negative (y down)
                px, py = np.r_[px[:1], px[1:][::-1]], np.r_[py[:1], py[1:][::-1]]
            dx, dy = np.diff(px, prepend=cx), np.diff(py, prepend=cy)
            cmds.append(np.array([9, zz(np.int64(dx[0])), zz(np.int64(dy[0])), 2 | (len(px) - 1) << 3], np.int64))
            cmds.append(np.column_stack([zz(dx[1:]), zz(dy[1:])]).ravel())
            cmds.append(np.array([15], np.int64))
            cx, cy = int(px[-1]), int(py[-1])
        tags = [0, vi(r["h"][fi])]
        if r["m"][fi] > 0 and "m" in props:
            tags += [1, vi(r["m"][fi])]
        if "s" in props:
            tags += [2, vi(r["s"][fi])]
        if r["s"][fi] == 1 and "f" in props:
            tags += [3, vi(r["f"][fi])]
        if "c" in props:
            tags += [4, vi(r["c"][fi])]
        if r["k"][fi] > 0 and "k" in props:
            tags += [5, vi(r["k"][fi])]
        f = pb_len(2, varints(tags)) + varints([3 << 3 | 0, 3]) + pb_len(4, varints(np.concatenate(cmds)))
        body += pb_len(2, f)
        nfeat += 1
    layer = pb_len(1, b"b") + bytes(body)
    for kname in keys:
        layer += pb_len(3, kname.encode())
    for v in values:
        layer += pb_len(4, varints([5 << 3 | 0, v]))
    layer += varints([5 << 3, EXTENT, 15 << 3, 2])
    raw = pb_len(3, layer)
    return len(raw), len(gzip.compress(raw, compresslevel=6, mtime=0)), nfeat


def phase_tiles(args, work: Path, root: Path) -> None:
    names, countries, clist, regc, dicts = context(work)
    files = sorted(json.loads((root / OVERTURE / "buildings.json").read_text())["files"])
    fills = sorted((work / "fill").glob("6-*.pkl"))
    cols = {z: {"est": [], "n": [], "key": [], "z6": []} for z in (14, 13, 12)}
    for k, p in enumerate(fills):
        r = pickle.loads(p.read_bytes())
        for z in (14, 13, 12):
            t = r["tiles"][z]
            cols[z]["est"].append(t["est"])
            cols[z]["n"].append(t["n"])
            cols[z]["key"].append(t["key"])
            cols[z]["z6"].append(np.full(len(t["key"]), k))
    rows = {z: {k: np.concatenate(v) for k, v in cols[z].items()} for z in cols}
    # Candidates: the 30 (z14) or 12 largest by estimate and the 6 most crowded per zoom, and a sample.
    want, est_of = {}, {}
    top14 = {(14, int(rows[14]["key"][i])) for i in np.argsort(-rows[14]["est"])[:12]}
    # The z14 tiles of §4.6's test places, for the iPad's estimate.
    for name, plon, plat in TEST_PLACES:
        wx, wy = world(np.array([plon]), np.array([plat]))
        key = (int(wx[0] * (1 << 14)) << 32) | int(wy[0] * (1 << 14))
        i = np.flatnonzero(rows[14]["key"] == key)
        if len(i):
            want[(14, key)] = fills[int(rows[14]["z6"][i[0]])].stem
            est_of[(14, key)] = float(rows[14]["est"][i[0]])
    for z in (14, 13, 12):
        r = rows[z]
        tx_, ty_ = r["key"] >> 32, r["key"] & 0xFFFFFFFF
        sample = np.flatnonzero((((tx_ * 73856093) ^ (ty_ * 19349663)) % {14: 2003, 13: 211, 12: 23}[z] == 0) & (r["n"] >= 20))
        sample = np.random.default_rng(z).choice(sample, min(40, len(sample)), replace=False) if len(sample) else sample
        for i in np.r_[np.argsort(-r["est"])[:30 if z == 14 else 12], np.argsort(-r["n"])[:6], sample].astype(np.int64):
            want[(z, int(r["key"][i]))] = fills[int(r["z6"][i])].stem
            est_of[(z, int(r["key"][i]))] = float(r["est"][i])
    log(f"tiles: {len(want)} tiles to encode")
    # Their records, from the fill outputs (a tile whose records weren't kept is filled again).
    recs = {}
    for p in fills:
        need = {k for k, v in want.items() if v == p.stem}
        if not need:
            continue
        r = pickle.loads(p.read_bytes())
        missing = set(need)
        for z in (14, 13, 12):
            d = r["dense"][z]
            for (zz_, key) in need:
                if zz_ != z:
                    continue
                m = d["tile"] == key
                if m.any():
                    recs[(z, key)] = {k: v[m] for k, v in d.items() if k != "tile"}
                    missing.discard((z, key))
        if missing:
            log(f"tiles: {p.stem} filled again for {len(missing)} tiles")
            fit = json.loads((work / "fit.json").read_text())["storey"]
            fj = json.loads((work / "fit.json").read_text())
            ctx = {"regc": regc, "a": np.array([fit[c].get("a", fit["*"]["a"]) for c in clist]),
                   "b": np.array([fit[c].get("b", fit["*"]["b"]) for c in clist]), "size": size_table(fj, clist),
                   "est": estimates(dicts), "dicts": dicts, "ghsl": str(work / "ghsl"), "want": missing}
            r2 = fill_tile((p.stem, [str(f) for f in sorted((work / "spool" / p.stem).glob("*.parquet"))], ctx))
            for (z, key) in missing:
                d = r2["dense"][z]
                m = d["tile"] == key
                recs[(z, key)] = {k: v[m] for k, v in d.items() if k != "tile"}
    # Their geometries, read again from the NAS a row group at a time.
    need = {}
    for tk, rec in recs.items():
        for fi, rg in set(zip(rec["fi"].tolist(), rec["rg"].tolist())):
            need.setdefault((fi, rg), set()).add(tk)
    log(f"tiles: {len(need)} row groups to read")
    geo: dict = {}
    t0 = time.time()
    nbytes = 0
    for (fi, rg) in sorted(need):
        wait_agent(args.agent_status)
        pf = pq.ParquetFile(root / OVERTURE / files[fi])
        col = pf.read_row_group(rg, columns=["geometry"]).column("geometry")
        nbytes += pf.metadata.row_group(rg).total_byte_size
        for tk in need[(fi, rg)]:
            rec = recs[tk]
            m = (rec["fi"] == fi) & (rec["rg"] == rg)
            rows_ = rec["row"][m]
            wkb = col.take(pa.array(rows_.astype(np.int64))).to_numpy(zero_copy_only=False)
            geo.setdefault(tk, {})[(fi, rg)] = (np.flatnonzero(m), shapely.from_wkb(wkb))
    log(f"tiles: geometries read ({time.time() - t0:.0f} s)")
    res = []
    for tk, rec in recs.items():
        z, key = tk
        gs = np.empty(len(rec["fi"]), object)
        for idx, gg in geo[tk].values():
            gs[idx] = gg
        raw, gz, nfeat = encode_tile(z, key >> 32, key & 0xFFFFFFFF, gs, rec)
        est = est_of[tk]
        res.append({"z": z, "x": key >> 32, "y": key & 0xFFFFFFFF, "n": int(len(rec["fi"])), "features": nfeat,
                    "raw": raw, "gz": gz, "est": est, "z6": want[tk]})
        if z == 14 and tk in top14:
            # What would bring the heaviest under ~300 KB: simplified to a grid unit (0.6 m at the
            # equator) as z12-13 are; only h and k kept; both.
            res[-1]["gz_simplified"] = encode_tile(z, key >> 32, key & 0xFFFFFFFF, gs, rec, simplify=True)[1]
            res[-1]["gz_hk"] = encode_tile(z, key >> 32, key & 0xFFFFFFFF, gs, rec, props="hk")[1]
            res[-1]["gz_both"] = encode_tile(z, key >> 32, key & 0xFFFFFFFF, gs, rec, simplify=True, props="hk")[1]
    (work / "tiles.json").write_text(json.dumps(res, indent=1))
    log(f"tiles: {len(res)} encoded")


# ---- report ------------------------------------------------------------------------------------------

def q(v, p):
    return float(np.percentile(v, p)) if len(v) else float("nan")


def phase_report(args, work: Path, root: Path) -> None:
    names, countries, clist, regc, dicts = context(work)
    fit = json.loads((work / "fit.json").read_text())
    scan = json.loads((work / "scan.json").read_text())
    fills = sorted((work / "fill").glob("6-*.pkl"))
    nreg = len(names) + 1
    cnt = None
    s = np.zeros((nreg, 5), np.int64)
    sr = np.zeros((nreg, 6), np.int64)
    hsrc = np.zeros((nreg, 256), np.int64)
    gsrc = np.zeros((nreg, 256), np.int64)
    hh = Counter()
    fh = Counter()
    ho, tr = [], []
    tiles = {14: [], 13: [], 12: []}
    places = {}
    per6 = {}
    map_stage = np.zeros(3, np.int64)
    pairs = [0, 0]
    secs = 0.0
    for p in fills:
        r = pickle.loads(p.read_bytes())
        if cnt is None:
            cnt = {k: v.copy() for k, v in r["count"].items()}
        else:
            for k, v in r["count"].items():
                cnt[k] += v
        s += r["s"]
        sr += r.get("s_rec", np.zeros((nreg, 6), np.int64))
        hsrc += r["hsrc"]
        gsrc += r["gsrc"]
        hh.update(dict(zip(r["hhist"][0].tolist(), r["hhist"][1].tolist())))
        fh.update(dict(zip(r["fhist"][0].tolist(), r["fhist"][1].tolist())))
        ho.append(r["ho"])
        tr.append(r["tr"])
        map_stage += r["map_stage"]
        pairs[0] += r["pairs"][0]
        pairs[1] += r["pairs"][1]
        secs += r["seconds"]
        for z in (14, 13, 12):
            t = r["tiles"][z]
            tiles[z].append(np.column_stack([t["key"], t["n"], t["est"], t["gb"], t["nv"]]))
        for name, v in r["places"].items():
            places[name] = [a + b for a, b in zip(places.get(name, [0, 0, 0, 0]), v)]
        per6[p.stem] = int(r["n"])
    ho = {k: np.concatenate([x[k] for x in ho]) for k in ho[0]}
    tr = {k: np.concatenate([x[k] for x in tr]) for k in tr[0]}
    rep = {"buildings": int(cnt["n"].sum()), "fill_cpu_s": round(secs), "pairs": pairs, "scan_s": round(scan.get("seconds", 0)),
           "scan_bytes": sum(f.get("bytes", 0) for f in scan["files"].values())}
    md = []
    pr = md.append
    ds = dicts["datasets"]
    creg = [regc[i] for i in range(1, nreg)]

    def by_country(a):
        out = np.zeros((len(clist),) + a.shape[1:], np.int64)
        for i in range(1, nreg):
            out[regc[i]] += a[i]
        return out

    C = {k: by_country(v) for k, v in cnt.items()}
    Cs, Ch, Cg, Cr = by_country(s), by_country(hsrc), by_country(gsrc), by_country(sr)
    order = [i for i in np.argsort(-C["n"]) if C["n"][i] > 0]
    pct = lambda a, b: f"{100 * a / b:.1f} %" if b else "–"

    pr(f"## Coverage: {rep['buildings']:,} buildings (centroid in the coverage + 1 km; {int(cnt['parts'].sum()):,} parts; "
       f"{int(cnt['under'].sum()):,} underground left out)\n")
    pr("| country | buildings | height | floors | either | neither | both | roof shape | roof colour | parts |")
    pr("|---|---|---|---|---|---|---|---|---|---|")
    tot = {k: v.sum(axis=0) for k, v in C.items()}
    for i in list(order) + [None]:
        g = (lambda k: tot[k]) if i is None else (lambda k, i=i: C[k][i])
        n = g("n")
        pr(f"| {'all' if i is None else clist[i]} | {n / 1e6:.2f} M | {pct(g('meas'), n)} | {pct(g('floors'), n)} | {pct(g('either'), n)} | "
           f"{pct(n - g('either'), n)} | {pct(g('both'), n)} | {pct(g('roof'), n)} | {pct(g('roofc'), n)} | {g('parts'):,} |")
    rep["coverage"] = {clist[i]: {k: int(C[k][i]) for k in C} for i in range(len(clist))}

    def srcs(M, i, k=4):
        row = M[i] if i is not None else M.sum(axis=0)
        n = row.sum()
        top = np.argsort(-row)[:k]
        return ", ".join(f"{ds[j] if j else 'none'} {100 * row[j] / n:.0f} %" for j in top if row[j] > 0 and 100 * row[j] / n >= 0.5) if n else "–"

    pr("\n## Where the heights and footprints come from\n")
    pr("| country | measured heights' datasets | footprints' datasets |")
    pr("|---|---|---|")
    for i in list(order) + [None]:
        pr(f"| {'all' if i is None else clist[i]} | {srcs(Ch, i)} | {srcs(Cg, i)} |")

    pr("\n## Measured heights (rule 0's) by country\n")
    pr("| country | median | p90 | p99 | 20 m or more | 40 m or more |")
    pr("|---|---|---|---|---|---|")
    hist = np.zeros((len(clist), 8192), np.int64)
    for k, v in hh.items():
        hist[k // 8192, k % 8192] += v
    def hq(row, p):
        n = row.sum()
        return np.searchsorted(np.cumsum(row), p * n) / 10 if n else float("nan")
    for i in list(order) + [None]:
        row = hist[i] if i is not None else hist.sum(axis=0)
        n = row.sum()
        pr(f"| {'all' if i is None else clist[i]} | {hq(row, .5):.1f} m | {hq(row, .9):.1f} m | {hq(row, .99):.1f} m | "
           f"{pct(row[200:].sum(), n)} | {pct(row[400:].sum(), n)} |")

    est_codes = estimates(dicts)
    pr("\n## Storey heights (rule 1): height = a × floors + b, least absolute deviations, the hold-out left out\n")
    pr("Fitted from the measured heights (Microsoft's ML estimates left out), and, for comparison, from every height.\n")
    pr("| country | measured, with floors | a (m a floor) | b (m) | mean abs. dev. | every height: n | a | b | median h/floor, measured: 1 | 2 | 3-4 | 5-9 | 10+ |")
    pr("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    st, sta = fit["storey"], fit["storey_all"]
    for c_ in [clist[i] for i in order] + ["*"]:
        e, ea = st[c_], sta[c_]
        fl = e["floors"]
        pr(f"| {'all' if c_ == '*' else c_} | {e['n']:,} | {e.get('a', '(all)')} | {e.get('b', '')} | {e.get('mae', '')} | {ea['n']:,} | "
           f"{ea.get('a', '(all)')} | {ea.get('b', '')} | " + " | ".join(f"{fl[k][1]}" if fl[k][1] is not None else "–" for k in ("1", "2", "3-4", "5-9", "10+")) + " |")
    pr("\nBy the height's dataset: a, b (n); median h/floor at 1 | 2 | 3-4 | 5-9 | 10+ floors\n")
    pr("| country | dataset | n | a | b | 1 | 2 | 3-4 | 5-9 | 10+ |")
    pr("|---|---|---|---|---|---|---|---|---|---|")
    for c_, row in fit["storey_by_source"].items():
        for d_, v in sorted(row.items(), key=lambda kv: -kv[1]["n"]):
            fl = v["floors"]
            pr(f"| {'all' if c_ == '*' else c_} | {d_} | {v['n']:,} | {v['a']} | {v['b']} | "
               + " | ".join(f"{fl[k][1]}" if fl[k][1] is not None else "–" for k in ("1", "2", "3-4", "5-9", "10+")) + " |")
    pr("\nMedian height by floors, measured: " + "; ".join(
        f"{'all' if c_ == '*' else c_} " + ", ".join(f"{k + 1}: {v}" for k, v in enumerate(st[c_]["table"]) if v is not None)
        for c_ in ["*"] + [clist[i] for i in order[:8]]))

    # The hold-out, against two truths: the measured heights, and every height (as the plan put it).
    hc = regc[ho["reg"]]
    truth = ho["h"].astype(np.float64)
    is_est = np.isin(ho["hsrc"], est_codes)
    trc = regc[tr["reg"]]
    tr_meas = ~np.isin(tr["hsrc"], est_codes)

    p4f = size_table(fit, clist)[hc, ho["bin"]]
    # GHSL scaled per country: the median of measured / GHSL over the training tenth's measured heights.
    kg = np.ones(len(clist))
    mg = tr_meas & (tr["g"] > 0)
    kall = float(np.median(tr["h"][mg] / tr["g"][mg])) if mg.any() else 1.0
    for i in range(len(clist)):
        m = mg & (trc == i)
        kg[i] = float(np.median(tr["h"][m] / tr["g"][m])) if m.sum() >= 200 else kall
    p3k = np.where(np.isfinite(ho["p3"]), np.where(ho["area"] < 60, np.minimum(ho["g"] * kg[hc], 4.0), ho["g"] * kg[hc]), np.nan)
    # Rule 1 by a table of each floor count's median height (12 floors and under), else a × floors + b.
    tab = np.array([[v if v is not None else np.nan for v in (st[c_]["table"])] for c_ in clist])
    gtab = np.array([v if v is not None else np.nan for v in st["*"]["table"]])
    nfh = ho["nf"].astype(np.int64)
    a_ = np.array([st[c_].get("a", st["*"]["a"]) for c_ in clist])
    b_ = np.array([st[c_].get("b", st["*"]["b"]) for c_ in clist])
    lin = np.clip(a_[hc] * nfh + b_[hc], H_MIN, H_MAX)
    k = np.clip(nfh, 1, 12) - 1
    p1t = np.where(nfh <= 12, np.where(np.isfinite(tab[hc, k]), tab[hc, k], np.where(np.isfinite(gtab[k]), gtab[k], lin)), lin)
    p1t = np.where(np.isfinite(ho["p1"]), p1t, np.nan)
    p2a = np.where(ho["st"] == 1, ho["p2"], np.nan)
    p2b = np.where(ho["st"] == 2, ho["p2"], np.nan)
    rules = {"1 floors": ho["p1"], "1' floors table": p1t, "2 neighbours": ho["p2"], "2a 150 m stage": p2a, "2b 300 m stage": p2b,
             "2' 150 m, any footprint": ho["pv"], "3 GHSL": ho["p3"], "3' GHSL × k": p3k, "4 size and kind": ho["p4"],
             "4' fitted defaults": p4f}

    def chain(*ps):
        v = np.full(len(truth), np.nan)
        for p in ps:
            m = np.isnan(v) & np.isfinite(p)
            v[m] = p[m]
        return v

    rules["chain 1-2-3-4"] = chain(ho["p1"], ho["p2"], ho["p3"], ho["p4"])
    rules["chain 1-3-2-4"] = chain(ho["p1"], ho["p3"], ho["p2"], ho["p4"])
    rules["chain 2-1-3-4"] = chain(ho["p2"], ho["p1"], ho["p3"], ho["p4"])
    rules["chain 1-2-3'-4'"] = chain(ho["p1"], ho["p2"], p3k, p4f)
    rules["chain 1-2-4'"] = chain(ho["p1"], ho["p2"], p4f)
    rules["chain 1'-2-3'-4'"] = chain(p1t, ho["p2"], p3k, p4f)
    rules["chain 1-2a-3'-2b-4'"] = chain(ho["p1"], p2a, p3k, p2b, p4f)
    p3t = np.where(ho["g"] >= GHSL_TALL, ho["p3"], np.nan)
    rules["3 GHSL, 20 m cells"] = p3t
    rules["chain 1-2-3(20 m)-4'"] = chain(ho["p1"], ho["p2"], p3t, p4f)
    rules["3(20 m)-4' without 1-2"] = chain(p3t, p4f)
    rep["ghsl_k"] = {clist[i]: round(kg[i], 3) for i in range(len(clist))}
    rep["holdout"] = {}
    rep["duel"] = {}
    short = ["1 floors", "1' floors table", "2 neighbours", "3 GHSL", "3' GHSL × k", "4 size and kind", "4' fitted defaults",
             "3(20 m)-4' without 1-2", "chain 1-2-3-4", "chain 1-2-3(20 m)-4'"]
    duel = [("1 floors", "2 neighbours"), ("1' floors table", "2 neighbours"), ("2a 150 m stage", "3' GHSL × k"),
            ("2b 300 m stage", "3' GHSL × k"), ("3 GHSL", "4 size and kind"), ("3' GHSL × k", "4' fitted defaults")]
    chains = [k_ for k_ in rules if k_.startswith("chain")]
    for tname, tmask in (("measured", ~is_est), ("every height", np.ones(len(truth), bool))):
        rep["holdout"][tname], rep["duel"][tname] = {}, {}
        pr(f"\n## The fill's hold-out against {'the measured heights (lidar, OSM, Esri, cities; Microsoft' + chr(39) + 's estimates left out)' if tname == 'measured' else 'every height (Microsoft' + chr(39) + 's estimates too, as the plan put it)'}\n")
        pr(f"{tmask.sum():,} held-out buildings (id hash 0 mod 10), each rule as if unmeasured: the median / p90 of "
           "|estimate − truth| in metres, (the share it answers).\n")
        pr("| country | held out | " + " | ".join(short) + " |")
        pr("|---|---|" + "---|" * len(short))
        for i in list(order) + [None]:
            m = tmask & (np.ones(len(truth), bool) if i is None else hc == i)
            if m.sum() < 200:
                continue
            name = "all" if i is None else clist[i]
            rep["holdout"][tname][name] = row = {}
            for label, p in rules.items():
                ok = m & np.isfinite(p)
                e = p[ok] - truth[ok]
                ae = np.abs(e)
                row[label] = {"n": int(ok.sum()), "share": round(ok.sum() / m.sum(), 4), "med": round(q(ae, 50), 2),
                              "p90": round(q(ae, 90), 2), "bias": round(q(e, 50), 2)}
            pr(f"| {name} | {m.sum():,} | " + " | ".join(
                f"{row[l]['med']:.1f} / {row[l]['p90']:.1f} ({100 * row[l]['share']:.0f} %)" if row[l]["n"] >= 20 else "–" for l in short) + " |")
        pr("\nBias (the median of estimate − truth):\n")
        pr("| country | " + " | ".join(short) + " |")
        pr("|---|" + "---|" * len(short))
        for name, row in rep["holdout"][tname].items():
            pr(f"| {name} | " + " | ".join(f"{row[l]['bias']:+.1f}" if row[l]["n"] >= 20 else "–" for l in short) + " |")
        pr("\nThe chains (median / p90):\n")
        pr("| country | " + " | ".join(c_[6:] for c_ in chains) + " |")
        pr("|---|" + "---|" * len(chains))
        for name, row in rep["holdout"][tname].items():
            pr(f"| {name} | " + " | ".join(f"{row[c_]['med']:.1f} / {row[c_]['p90']:.1f}" for c_ in chains) + " |")
        pr("\nWhere two rules both answer, the median error of each on the same buildings (n):\n")
        pr("| country | " + " | ".join(f"{a} / {b}" for a, b in duel) + " |")
        pr("|---|" + "---|" * len(duel))
        for i in list(order) + [None]:
            m = tmask & (np.ones(len(truth), bool) if i is None else hc == i)
            if m.sum() < 200:
                continue
            name = "all" if i is None else clist[i]
            rep["duel"][tname][name] = {}
            cells = []
            for a, b in duel:
                ok = m & np.isfinite(rules[a]) & np.isfinite(rules[b])
                if ok.sum() < 50:
                    cells.append("–")
                    continue
                ea, eb = q(np.abs(rules[a][ok] - truth[ok]), 50), q(np.abs(rules[b][ok] - truth[ok]), 50)
                rep["duel"][tname][name][f"{a} / {b}"] = [round(ea, 2), round(eb, 2), int(ok.sum())]
                cells.append(f"{ea:.1f} / {eb:.1f} ({ok.sum():,})")
            pr(f"| {name} | " + " | ".join(cells) + " |")
    # By the true height: the skyline (z12-13) is the tall buildings, which the medians hide.
    classes = [("under 6 m", 0, 6), ("6-10 m", 6, 10), ("10-20 m", 10, 20), ("20-40 m", 20, 40), ("40 m or more", 40, 1e9)]
    cols = ["1 floors", "1' floors table", "2 neighbours", "3 GHSL", "3' GHSL × k", "4 size and kind", "4' fitted defaults",
            "3(20 m)-4' without 1-2", "chain 1-2-3-4", "chain 1-2-3(20 m)-4'"]
    rep["by_height"] = {}
    for tname, tmask in (("measured", ~is_est), ("every height", np.ones(len(truth), bool))):
        pr(f"\n**By the true height** ({tname}; all countries; JP and US apart): median |error| / bias (m), (the share answered)\n")
        pr("| | held out | " + " | ".join(cols) + " |")
        pr("|---|---|" + "---|" * len(cols))
        rep["by_height"][tname] = {}
        for cname, cm in (("all", np.ones(len(truth), bool)), ("JP", hc == clist.index("JP") if "JP" in clist else np.zeros(len(truth), bool)),
                          ("US", hc == clist.index("US") if "US" in clist else np.zeros(len(truth), bool))):
            for label, lo, hi in classes:
                m = tmask & cm & (truth >= lo) & (truth < hi)
                if m.sum() < 100:
                    continue
                cells = []
                for c_ in cols:
                    ok = m & np.isfinite(rules[c_])
                    e = rules[c_][ok] - truth[ok]
                    rep["by_height"][tname][f"{cname} {label} {c_}"] = [round(q(np.abs(e), 50), 2), round(q(e, 50), 2), int(ok.sum())]
                    cells.append(f"{q(np.abs(e), 50):.1f} / {q(e, 50):+.1f} ({100 * ok.sum() / m.sum():.0f} %)" if ok.sum() >= 20 else "–")
                pr(f"| {cname} {label} | {m.sum():,} | " + " | ".join(cells) + " |")
    pr("\n**The chain 1-2-3-4 by the truth's dataset** (all countries)\n")
    pr("| dataset | held out | median | p90 | bias |")
    pr("|---|---|---|---|---|")
    p = rules["chain 1-2-3-4"]
    for j in np.argsort(-np.bincount(ho["hsrc"], minlength=256))[:6]:
        m = ho["hsrc"] == j
        if m.sum() < 100:
            continue
        e = p[m] - truth[m]
        pr(f"| {ds[j] if j else 'none'} | {m.sum():,} | {q(np.abs(e), 50):.1f} m | {q(np.abs(e), 90):.1f} m | {q(e, 50):+.1f} m |")
    st2 = ho["st"]
    pr(f"\nRule 2 on the hold-out: {pct((st2 == 1).sum(), len(st2))} by 150 m and similar footprints, "
       f"{pct((st2 == 2).sum(), len(st2))} by the 300 m fallback; on the map: {pct(map_stage[1], map_stage.sum())} and "
       f"{pct(map_stage[2], map_stage.sum())} of the buildings without a height or floors.")

    pr("\n## The map's heights by rule (every building in the coverage)\n")
    pr("| country | 0 measured | 0 of them Microsoft's | 1 floors | 2 neighbours | 3 GHSL | 4 size |")
    pr("|---|---|---|---|---|---|---|")
    est_n = Ch[:, est_codes].sum(axis=1) if est_codes else np.zeros(len(clist))
    for i in list(order) + [None]:
        row = Cs[i] if i is not None else Cs.sum(axis=0)
        n = row.sum()
        en = est_n[i] if i is not None else est_n.sum()
        pr(f"| {'all' if i is None else clist[i]} | {pct(row[0], n)} | {pct(en, n)} | " + " | ".join(pct(row[k], n) for k in range(1, 5)) + " |")
    rep["fill_shares"] = {clist[i]: [int(v) for v in Cs[i]] + [int(est_n[i])] for i in range(len(clist))}
    pr("\n**As B0 recommends** (§2.3: floors before Microsoft's estimates, GHSL only in 20 m cells, size and kind fitted):\n")
    pr("| country | 0 measured | 1 floors | 2 Microsoft's estimate | 3 neighbours | 4 GHSL, 20 m cells | 5 size and kind |")
    pr("|---|---|---|---|---|---|---|")
    for i in list(order) + [None]:
        row = Cr[i] if i is not None else Cr.sum(axis=0)
        n = row.sum()
        pr(f"| {'all' if i is None else clist[i]} | " + " | ".join(pct(row[k], n) for k in range(6)) + " |")
    rep["fill_shares_rec"] = {clist[i]: [int(v) for v in Cr[i]] for i in range(len(clist))}

    pr("\n## Size and kind (rule 4): measured heights' medians by class (the hold-out left out)\n")
    pr("| country | " + " | ".join(SIZE_BINS) + " |")
    pr("|---|---|---|---|---|---|")
    for key, label in (("size", "measured"), ("size_all", "every height")):
        for c_ in [clist[i] for i in order] + ["*"]:
            e = fit[key][c_]
            if sum(e["n"]) < 200:
                continue
            pr(f"| {'all' if c_ == '*' else c_} ({label}) | " + " | ".join(f"{m} m ({n:,})" if m is not None else "–" for m, n in zip(e["median"], e["n"])) + " |")
    pr("GHSL's factor per country (median measured / GHSL, training tenth): " + ", ".join(f"{clist[i]} {kg[i]:.2f}" for i in order))

    # Tiles.
    pr("\n## Tiles\n")
    enc = json.loads((work / "tiles.json").read_text()) if (work / "tiles.json").exists() else []
    rep["tiles"] = {}
    ratio = {}
    for z in (14, 13, 12):
        e = [t for t in enc if t["z"] == z]
        if e:
            rr = np.array([t["gz"] / t["raw"] for t in e])
            er = np.array([t["raw"] / t["est"] for t in e])
            ratio[z] = (float(np.median(rr)), float(np.median(er)))
    for z in (14, 13, 12):
        a = np.concatenate(tiles[z]) if tiles[z] else np.zeros((0, 5))
        n = a[:, 1]
        gzr, er = ratio.get(z, (0.6, 1.0))
        gz_est = a[:, 2] * er * gzr
        top = np.argsort(-a[:, 2])[:10]
        rep["tiles"][z] = {"tiles": int(len(a)), "features": int(n.sum()), "max_n": int(n.max()) if len(n) else 0,
                           "p99_n": q(n, 99), "p50_n": q(n, 50), "est_gz_total": float(gz_est.sum()), "gz_ratio": gzr, "est_ratio": er,
                           "over_300k": int((gz_est > 300_000).sum()), "max_gz_est": float(gz_est.max()) if len(a) else 0}
        pr(f"**z{z}:** {len(a):,} tiles, {int(n.sum()):,} features; per tile median {q(n, 50):.0f}, p99 {q(n, 99):.0f}, "
           f"max {int(n.max()) if len(n) else 0:,}; estimated {gz_est.sum() / 1e9:.2f} GB gzip'd in all "
           f"(raw/estimate {er:.3f}, gzip/raw {gzr:.3f} from the encoded tiles); {int((gz_est > 300_000).sum())} tiles over 300 KB.\n")
        pr("| tile | features | estimated (gzip) | encoded raw | encoded gzip |")
        pr("|---|---|---|---|---|")
        encd = {(t["x"], t["y"]): t for t in enc if t["z"] == z}
        for i in top:
            key = int(a[i, 0])
            x, y = key >> 32, key & 0xFFFFFFFF
            t = encd.get((x, y))
            pr(f"| {z}/{x}/{y} | {int(a[i, 1]):,} | {gz_est[i] / 1e3:.0f} KB | {t['raw'] / 1e3:.0f} KB | {t['gz'] / 1e3:.0f} KB |" if t else
               f"| {z}/{x}/{y} | {int(a[i, 1]):,} | {gz_est[i] / 1e3:.0f} KB | – | – |")
    # Packs: a z6 tile's z12-14.
    packs = Counter()
    for z in (14, 13, 12):
        gzr, er = ratio.get(z, (0.6, 1.0))
        a = np.concatenate(tiles[z]) if tiles[z] else np.zeros((0, 5))
        key = a[:, 0].astype(np.int64)
        k6 = ((key >> 32) >> (z - 6)) * 64 + ((key & 0xFFFFFFFF) >> (z - 6))
        u, inv = np.unique(k6, return_inverse=True)
        for k, v in zip(u.tolist(), np.bincount(inv, weights=a[:, 2] * er * gzr).tolist()):
            packs[(k // 64, k % 64)] += v
    big = packs.most_common(8)
    rep["packs"] = {"total": float(sum(packs.values())), "largest": [[f"6/{k[0]}/{k[1]}", float(v)] for k, v in big]}
    pr(f"\n**Packs** (a z6 tile's z12-14, gzip'd, estimated): {sum(packs.values()) / 1e9:.2f} GB in all; the largest "
       + ", ".join(f"6/{k[0]}/{k[1]} {v / 1e6:.0f} MB" for k, v in big))
    over = [t for t in enc if t["z"] == 14 and t["gz"] > 300_000]
    rep["z14_encoded_over_300k"] = [[f"14/{t['x']}/{t['y']}", t["features"], t["gz"]] for t in sorted(over, key=lambda t: -t["gz"])]
    pr(f"\n**Encoded z14 tiles over 300 KB:** {len(over)} of the {len([t for t in enc if t['z'] == 14])} encoded (the 30 largest by "
       "the estimate among them): " + ", ".join(f"14/{t['x']}/{t['y']} {t['gz'] / 1e3:.0f} KB ({t['features']:,})" for t in sorted(over, key=lambda t: -t["gz"])))
    # The heaviest z14 tiles' alternatives.
    alt = [t for t in enc if "gz_simplified" in t]
    if alt:
        pr("\n**The heaviest z14 tiles, encoded otherwise** (gzip'd KB): as §3.4 / simplified to a grid unit / only h and k / both\n")
        for t in sorted(alt, key=lambda t: -t["gz"]):
            pr(f"- 14/{t['x']}/{t['y']} ({t['features']:,} features): {t['gz'] / 1e3:.0f} / {t['gz_simplified'] / 1e3:.0f} / "
               f"{t['gz_hk'] / 1e3:.0f} / {t['gz_both'] / 1e3:.0f}")
    # §4.6's places.
    pr("\n**§4.6's test places' z14 tiles:** " + "; ".join(
        f"{name} 14/{x}/{y} {t['features']:,} features, {t['gz'] / 1e3:.0f} KB"
        for name, plon, plat in TEST_PLACES
        for x, y in [(int(world(np.array([plon]), np.array([plat]))[0][0] * (1 << 14)), int(world(np.array([plon]), np.array([plat]))[1][0] * (1 << 14)))]
        for t in [next((t for t in enc if t["z"] == 14 and t["x"] == x and t["y"] == y), None)] if t))
    # Calibration.
    if enc:
        pr("\n**Encoded tiles** (the estimate's calibration):\n")
        for z in (14, 13, 12):
            e = [t for t in enc if t["z"] == z]
            if e:
                er = np.array([t["raw"] / t["est"] for t in e])
                bpf = np.array([t["gz"] / max(1, t["features"]) for t in e])
                pr(f"- z{z}: {len(e)} tiles; raw / estimate median {np.median(er):.3f} (range {er.min():.3f}-{er.max():.3f}); "
                   f"gzip'd bytes a feature median {np.median(bpf):.1f} (range {bpf.min():.1f}-{bpf.max():.1f})")
    # z6 tiles.
    big6 = sorted(per6.items(), key=lambda kv: -kv[1])
    rep["z6"] = {"tiles": len(per6), "over_5M": [[k, v] for k, v in big6 if v > 5_000_000]}
    pr(f"\n**z6 tiles** with buildings: {len(per6)}; over 5 M: " + ", ".join(f"{k.replace('-', '/')} {v / 1e6:.1f} M" for k, v in big6 if v > 5_000_000))

    pr("\n## Places (the buildings within 2 km of a city's centre, 15 km of a rural point)\n")
    pr("| place | buildings | height | floors | either |")
    pr("|---|---|---|---|---|")
    for name, *_ in PLACES:
        v = places.get(name)
        if v:
            pr(f"| {name} | {v[0]:,} | {pct(v[1], v[0])} | {pct(v[2], v[0])} | {pct(v[3], v[0])} |")
    rep["places"] = places
    pr(f"\nScan: {rep['scan_bytes'] / 1e9:.1f} GB read from the NAS in {rep['scan_s'] / 60:.0f} min; fill: {rep['fill_cpu_s'] / 60:.0f} CPU-min, "
       f"{pairs[0] / 1e9:.2f} G + {pairs[1] / 1e9:.2f} G neighbour pairs.")
    text = "\n".join(md)
    print(text)
    (work / "report.md").write_text(text)
    (work / "report.json").write_text(json.dumps(rep, indent=1, default=float))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", required=True, help="the NAS's project folder (read only)")
    ap.add_argument("--work", required=True, help="a local folder for the spool and the results")
    ap.add_argument("--regions", help="the regions' outlines as GeoJSON (else from --server)")
    ap.add_argument("--server", default="http://localhost:8080", help="the map's server, for the regions")
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--fill-jobs", type=int, default=4)
    ap.add_argument("--fill-mem-gb", type=float, default=16.0, help="the fill's tiles in memory at once, roughly")
    ap.add_argument("--phases", default="scan,fit,fill,tiles,report")
    ap.add_argument("--box", help="w,s,e,n: only the buildings in this box (a test)")
    ap.add_argument("--agent-status", default=str(Path.home() / "Library/Application Support/scenic/agent/status.json"))
    args = ap.parse_args()
    root, work = Path(args.root), Path(args.work)
    if not (root / OVERTURE / "buildings.json").exists():
        raise SystemExit(f"{root}: no {OVERTURE}/buildings.json")
    work.mkdir(parents=True, exist_ok=True)
    phases = args.phases.split(",")
    names, _, countries = load_regions(args, work)
    log(f"{len(names)} regions in {len(set(countries))} countries")
    if "scan" in phases:
        phase_scan(args, work, root, names, countries)
    if "fit" in phases:
        phase_fit(work)
    if "fill" in phases:
        phase_fill(args, work)
    if "tiles" in phases:
        phase_tiles(args, work, root)
    if "report" in phases:
        phase_report(args, work, root)


if __name__ == "__main__":
    main()
