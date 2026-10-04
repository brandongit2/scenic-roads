#!/usr/bin/env python3
"""Map labels by importance: places, provinces and states, water and parks as label points, each
with the zoom from which it may show (mz), for the label density control (web: Layers → Map).

A label shows from the zoom where its isolation, the distance to the nearest label of its kind that
matters more (interest.py isolation), spans the spacing set in the app: mz + log2(spacing in px),
as the stops & sights' names do. The most important for miles around shows first, and labels are
as dense at every zoom: a hamlet alone in the north shows zoomed out, one beside a town only close
in. (The basemap's own labels came by its class zooms and collisions alone.)

What matters more, per kind (k; class c):
  place  city > town > village > suburb > hamlet, quarter > neighbourhood > locality, isolated
         dwelling; within a class, population (a capital first)
  state  provinces and states, by population
  water  oceans and seas, then by area: lakes, reservoirs, bays and straits alike (a bay or strait
         mapped as a point has none: as one of 1 km², 10 km² with a Wikipedia article; not by
         Wikidata item, which most Canadian bays have from the national names database)
  park   national parks, other protected areas and nature reserves by area, a national park as one
         ten times its size
Isolation alone would hide a big place beside a bigger one (Laval beside Montréal, Yokohama
beside Tokyo) until close in, so a place also shows from a zoom by its population (18 − 2 log10
population at the default spacing: a million people from zoom 6, 100,000 from 8), and an area once
it spans BIG_PX on screen (√area), whichever comes first: mz is the earlier, both moving with the
spacing; collisions decide between neighbours, the most important first. An area's name also waits
until the area spans SIZE_PX (ms: the zoom it does), however far the next one is: no pond named
zoomed out for want of a lake near. Water and parks of the same name within DEDUPE_KM of a more
important one are left out (a lake mapped twice).
Rivers keep the basemap's labels (along their lines).

Output: data/build/labels.tiles (roadcore archive of gzip'd Mapbox vector tiles, layer "l",
z0–12, z12 overzoomed beyond): a label is in the tiles from the zoom where it could show at
MIN_PX, and in every z12 tile. Properties: n (name), en (English: its own name:en, else names.py's
table), k, c, mz, ms (areas), s (importance, the placement order).

usage: labels.py   (reads data/names/named.osm.pbf: names.py filter)
       labels.py --src <pbf> --out <labels.tiles> --work <dir> --own-english
                  (the new pipeline: the OSM pass's labels set; English only the places' own, as
                  the server attaches the translations when serving)
"""
from __future__ import annotations

import gzip
import json
import math
import struct
import subprocess
import sys
import time
from collections import defaultdict
from pathlib import Path

import numpy as np
import osmium
from shapely import wkb as swkb

from interest import isolation, min_zoom
from names import differs, english_at

ROOT = Path(__file__).resolve().parent.parent
N = ROOT / "data" / "names"
SRC = N / "named.osm.pbf"
NODES = N / "labels-nodes.osm.pbf"
AREAS = N / "labels-areas.osm.pbf"
OUT = ROOT / "data" / "build" / "labels.tiles"
MAXZ = 12
# The smallest spacing the app allows (px): a label is in the tiles from the zoom it could show at it.
MIN_PX = 16
EXTENT = 4096

PLACE_RANK = {"city": 7, "town": 6, "village": 5, "suburb": 4, "hamlet": 3, "quarter": 3, "neighbourhood": 2,
              "locality": 1, "isolated_dwelling": 0}
# Seas and oceans above any water area (whose scores are log10 km² + 4: up to ~10).
SEA_SCORE = 20.0
# An area's name shows once the area spans SIZE_PX (√area), whatever the spacing (the app scales
# it by the square root of the kind's spacing over DEFAULT_PX), and from BIG_PX however near a more
# important one is.
SIZE_PX = 20
BIG_PX = 120
# The app's default label spacing (web/src/state.ts DEFAULT_DENSITY): mz is relative to it.
DEFAULT_PX = 90
# A place with a population shows from zoom POP_ZOOM[0] − POP_ZOOM[1] log10(population) at the
# default spacing (a capital half a zoom sooner) however near a more important one is.
POP_ZOOM = (18.0, 2.0)
DEDUPE_KM = 30.0
# natural=water that isn't a lake: a river's or canal's water area (named along its waterway line).
NOT_LAKE = {"river", "stream", "canal", "ditch", "drain", "wastewater", "riverbank", "moat", "fountain", "rapids",
            "fish_pass", "lock", "basin"}
# Label points (nodes alone), and areas (with their outlines' ways and nodes).
NODE_FILTERS = ["n/place=" + ",".join([*PLACE_RANK, "state", "province", "ocean", "sea"]), "n/natural=bay,strait"]
AREA_FILTERS = ["wr/natural=water,bay,strait", "wr/boundary=national_park,protected_area", "wr/leisure=nature_reserve"]


def population(v: str | None) -> float:
    if not v:
        return 0.0
    try:
        return max(0.0, float(v.replace(",", "").replace(" ", "").split(";")[0]))
    except ValueError:
        return 0.0


class Points(osmium.SimpleHandler):
    def __init__(self, rows: list):
        super().__init__()
        # kind, class, name, lon, lat, score, own English (name:en), area (km²), zoom by population
        self.rows = rows

    def node(self, n):
        t = n.tags
        name = t.get("name")
        if not name or not n.location.valid():
            return
        lon, lat, p, en = n.location.lon, n.location.lat, t.get("place"), t.get("name:en")
        if p in PLACE_RANK:
            cap = 0.95 if t.get("capital") in ("yes", "2", "3", "4") else 0.0
            pop = min(8.9, math.log10(population(t.get("population")) + 1))
            self.rows.append(("place", p, name, lon, lat, PLACE_RANK[p] * 10 + pop + cap, en, None,
                              POP_ZOOM[0] - POP_ZOOM[1] * pop - (0.5 if cap else 0) if pop > 0 else None))
        elif p in ("state", "province"):
            self.rows.append(("state", p, name, lon, lat, math.log10(population(t.get("population")) + 1), en, None, None))
        elif p in ("ocean", "sea"):
            self.rows.append(("water", p, name, lon, lat, SEA_SCORE, en, None, None))
        elif t.get("natural") in ("bay", "strait"):
            km2 = 10.0 if t.get("wikipedia") else 1.0
            self.rows.append(("water", t["natural"], name, lon, lat, math.log10(km2) + 4, en, km2, None))


class Areas(osmium.SimpleHandler):
    """Areas alone: no node callback, so the outlines' nodes never reach Python."""

    def __init__(self, rows: list):
        super().__init__()
        self.rows = rows
        self.wkb = osmium.geom.WKBFactory()

    def area(self, a):
        t = a.tags
        name = t.get("name")
        if not name:
            return
        nat = t.get("natural")
        if nat == "water" and t.get("water", "lake") not in NOT_LAKE:
            kind, cls = "water", "lake"
        elif nat in ("bay", "strait"):
            kind, cls = "water", nat
        elif t.get("boundary") == "national_park":
            kind, cls = "park", "national_park"
        elif t.get("boundary") == "protected_area" or t.get("leisure") == "nature_reserve":
            kind, cls = "park", "protected_area"
        else:
            return
        try:
            g = swkb.loads(self.wkb.create_multipolygon(a), hex=True)
        except Exception:
            return
        if g.is_empty:
            return
        pt = g.representative_point()  # inside it, not a centroid off in a bay
        km2 = g.area * 111.32 * 110.57 * math.cos(math.radians(pt.y))
        score = math.log10(km2 + 1e-4) + 4 + (1 if cls == "national_park" else 0)
        self.rows.append((kind, cls, name, pt.x, pt.y, score, t.get("name:en"), km2, None))


def dedupe(rows: list, score: np.ndarray, lon: np.ndarray, lat: np.ndarray) -> np.ndarray:
    """Which to keep: water and parks but those within DEDUPE_KM of one of the same kind and name
    that matters more."""
    keep = np.ones(len(rows), dtype=bool)
    groups: dict[tuple[str, str], list[int]] = defaultdict(list)
    for i, r in enumerate(rows):
        if r[0] in ("water", "park"):
            groups[(r[0], r[2])].append(i)
    for idx in groups.values():
        if len(idx) < 2:
            continue
        kept: list[tuple[float, float]] = []
        for i in sorted(idx, key=lambda i: -score[i]):
            x, y = lon[i] * 111.32 * math.cos(math.radians(lat[i])), lat[i] * 110.57
            if kept:
                k = np.array(kept)
                if np.hypot(k[:, 0] - x, k[:, 1] - y).min() < DEDUPE_KM:
                    keep[i] = False
                    continue
            kept.append((x, y))
    return keep


# ---- Mapbox vector tiles (points, one layer): as web/src/mvt.ts --------------------------------

def varint(v: int) -> bytes:
    out = bytearray()
    while v > 0x7F:
        out.append((v & 0x7F) | 0x80)
        v >>= 7
    out.append(v)
    return bytes(out)


def field(num: int, wire: int) -> bytes:
    return varint((num << 3) | wire)


def ld(num: int, b: bytes) -> bytes:
    return field(num, 2) + varint(len(b)) + b


zz = lambda n: (n << 1) if n >= 0 else (-n << 1) - 1


def encode(points: list[tuple[int, int, int, dict]]) -> bytes:
    keys: dict[str, int] = {}
    vals: dict[tuple, int] = {}
    vmsgs: list[bytes] = []

    def val(v) -> int:
        k = (type(v).__name__, v)
        if k in vals:
            return vals[k]
        if isinstance(v, str):
            m = ld(1, v.encode())
        elif isinstance(v, int):
            m = field(6, 0) + varint(zz(v))
        else:
            m = field(3, 1) + struct.pack("<d", float(v))
        vals[k] = len(vmsgs)
        vmsgs.append(m)
        return vals[k]

    layer = field(15, 0) + varint(2) + ld(1, b"l")
    for x, y, fid, props in points:
        tags = bytearray()
        for k, v in props.items():
            if v is None:
                continue
            if k not in keys:
                keys[k] = len(keys)
            tags += varint(keys[k]) + varint(val(v))
        f = field(1, 0) + varint(fid) + ld(2, bytes(tags)) + field(3, 0) + varint(1)
        f += ld(4, varint(9) + varint(zz(x)) + varint(zz(y)))
        layer += ld(2, f)
    for k in keys:
        layer += ld(3, k.encode())
    for m in vmsgs:
        layer += ld(4, m)
    layer += field(5, 0) + varint(EXTENT)
    return ld(3, layer)


class Writer:
    """roadcore::archive format (as trees.py's): magic, index offset, count, metadata JSON, blobs, index."""

    def __init__(self, path: Path, meta: str):
        self.path, self.tmp = path, path.with_suffix(".tiles.tmp")
        self.f = self.tmp.open("wb")
        mb = meta.encode()
        self.f.write(b"RDTILES1" + bytes(16) + struct.pack("<I", len(mb)) + mb)
        self.pos = 28 + len(mb)
        self.index = []

    def add(self, z: int, x: int, y: int, blob: bytes, raw: int):
        self.f.write(blob)
        self.index.append(((z << 58) | (x << 29) | y, self.pos, len(blob), raw))
        self.pos += len(blob)

    def finish(self) -> int:
        self.index.sort()
        pad = (8 - self.pos % 8) % 8
        self.f.write(bytes(pad))
        off = self.pos + pad
        for e in self.index:
            self.f.write(struct.pack("<QQII", *e))
        self.f.seek(8)
        self.f.write(struct.pack("<QQ", off, len(self.index)))
        self.f.close()
        self.tmp.rename(self.path)
        return len(self.index)


def args() -> None:
    """--src, --out, --work, --own-english: where to read and write (else the legacy paths)."""
    global SRC, NODES, AREAS, OUT, OWN_ENGLISH
    a = sys.argv[1:]
    opt = lambda k: a[a.index(k) + 1] if k in a and a.index(k) + 1 < len(a) else None  # noqa: E731
    if opt("--src"):
        SRC = Path(opt("--src"))
    if opt("--work"):
        w = Path(opt("--work"))
        w.mkdir(parents=True, exist_ok=True)
        NODES, AREAS = w / "labels-nodes.osm.pbf", w / "labels-areas.osm.pbf"
    if opt("--out"):
        OUT = Path(opt("--out"))
    OWN_ENGLISH = "--own-english" in a


OWN_ENGLISH = False


def progress(done: int, total: int, unit: str) -> None:
    """A line the build agent shows as this job's progress (crates/pipeline/src/agent/jobs.rs)."""
    print(f"progress: {done}/{total} {unit}", file=sys.stderr, flush=True)


def main() -> None:
    args()
    t0 = time.time()
    progress(0, 6, "steps (the label points)")
    subprocess.run(["osmium", "tags-filter", "--overwrite", "-R", str(SRC), *NODE_FILTERS, "-o", str(NODES)], check=True)
    # Only named areas become labels: the unnamed (most lakes and ponds) go before their nodes are
    # read. Members of a named relation stay (as its references), named or not.
    all_areas = AREAS.with_name(AREAS.name.replace(".osm.pbf", "-all.osm.pbf"))
    subprocess.run(["osmium", "tags-filter", "--overwrite", str(SRC), *AREA_FILTERS, "-o", str(all_areas)], check=True)
    subprocess.run(["osmium", "tags-filter", "--overwrite", str(all_areas), "wr/name", "-o", str(AREAS)], check=True)
    all_areas.unlink(missing_ok=True)
    rows: list[tuple[str, str, str, float, float, float, str | None, float | None, float | None]] = []
    progress(1, 6, "steps (reading the points)")
    Points(rows).apply_file(str(NODES))
    print(f"{len(rows)} label points ({time.time() - t0:.0f} s)", file=sys.stderr)
    # Node locations in a sparse index on disk (16 bytes a node): pyosmium's default switches to a
    # dense array as big as the highest node id (about 100 GB for the planet's ids).
    idx = AREAS.with_name("labels-nodes.idx")
    idx.unlink(missing_ok=True)
    progress(2, 6, "steps (reading the named areas)")
    Areas(rows).apply_file(str(AREAS), locations=True, idx=f"sparse_file_array,{idx}")
    idx.unlink(missing_ok=True)
    print(f"{len(rows)} labels read ({time.time() - t0:.0f} s)", file=sys.stderr)
    keep = dedupe(rows, np.array([r[5] for r in rows]), np.array([r[3] for r in rows]), np.array([r[4] for r in rows]))
    rows = [r for r, k in zip(rows, keep) if k]
    print(f"{len(keep) - len(rows)} duplicates left out ({time.time() - t0:.0f} s)", file=sys.stderr)
    kinds = np.array([r[0] for r in rows])
    lon = np.array([r[3] for r in rows])
    lat = np.array([r[4] for r in rows])
    score = np.array([r[5] for r in rows])
    # Areas: the zoom where √area spans one pixel; ms where it spans SIZE_PX.
    zs = np.array([min_zoom(r[4], math.sqrt(r[7])) if r[7] else np.nan for r in rows])
    ms = zs + math.log2(SIZE_PX)
    # The zoom by size or population, relative to the default spacing as mz is.
    absz = np.fmin(zs + math.log2(BIG_PX), np.array([r[8] if r[8] is not None else np.nan for r in rows])) - math.log2(DEFAULT_PX)
    mz = np.zeros(len(rows))
    progress(3, 6, "steps (each label's isolation)")
    for k in ("place", "state", "water", "park"):
        idx = np.nonzero(kinds == k)[0]
        if not len(idx):
            continue
        ia = isolation(lon[idx], lat[idx], score[idx])
        mz[idx] = [min_zoom(float(lat[i]), float(d)) for i, d in zip(idx, ia)]
        mz[idx] = np.fmin(mz[idx], absz[idx])
        print(f"  {k}: {len(idx)}, isolation done ({time.time() - t0:.0f} s)", file=sys.stderr)
    lat_c = np.clip(lat, -85.05, 85.05)
    tx = (lon + 180) / 360
    ty = 0.5 - np.log(np.tan(np.pi / 4 + np.radians(lat_c) / 2)) / (2 * np.pi)
    # (ms shifts by half the spacing's log2: up to this much sooner at MIN_PX.)
    first = np.clip(np.floor(np.fmax(mz + math.log2(MIN_PX), ms - 0.5 * math.log2(DEFAULT_PX / MIN_PX))), 0, MAXZ).astype(int)
    tiles: dict[tuple[int, int, int], list[int]] = defaultdict(list)
    progress(4, 6, "steps (the tiles each label shows in)")
    for i in range(len(rows)):
        for z in range(first[i], MAXZ + 1):
            n = 1 << z
            tiles[(z, min(n - 1, int(tx[i] * n)), min(n - 1, int(ty[i] * n)))].append(i)
    print(f"{len(tiles)} tiles ({time.time() - t0:.0f} s)", file=sys.stderr)
    en_cache: dict[int, str | None] = {}

    def english(i: int) -> str | None:
        if i not in en_cache:
            if OWN_ENGLISH:
                own = rows[i][6]
                en_cache[i] = own.strip() if own and differs(rows[i][2], own) else None
            else:
                en_cache[i] = english_at(rows[i][2], (lon[i], lat[i]), rows[i][6])
        return en_cache[i]

    OUT.parent.mkdir(parents=True, exist_ok=True)
    w = Writer(OUT, json.dumps({"format": "pbf", "layer": "l", "maxzoom": MAXZ}))
    step = max(1, len(tiles) // 100)
    for k, (z, x, y) in enumerate(sorted(tiles)):
        if k % step == 0:
            progress(k, len(tiles), "label tiles written")
        n = 1 << z
        ids = sorted(tiles[(z, x, y)], key=lambda i: -score[i])
        pts = []
        for i in ids:
            kind, cls, name = rows[i][0], rows[i][1], rows[i][2]
            pts.append((round((tx[i] * n - x) * EXTENT), round((ty[i] * n - y) * EXTENT), i,
                        {"n": name, "en": english(i), "k": kind, "c": cls, "mz": round(float(mz[i]), 2),
                         "ms": None if math.isnan(ms[i]) else round(float(ms[i]), 2), "s": round(float(score[i]), 2)}))
        raw = encode(pts)
        w.add(z, x, y, gzip.compress(raw, 6), len(raw))
    count = w.finish()
    print(f"labels.tiles: {len(rows)} labels in {count} tiles, {OUT.stat().st_size / 1e6:.0f} MB ({time.time() - t0:.0f} s)", file=sys.stderr)


if __name__ == "__main__":
    main()
