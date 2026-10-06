#!/usr/bin/env python3
"""The 3D buildings' sources for one z6 tile T, decoded (docs/buildings3d.md §3.1, `bldprep`):
`scenic-build bldprep` runs this and reads its stdout (pipeline::bld::prep), which computes every
number that ends up in a file (§3.7). Python only decodes: pyarrow reads the parquet, rasterio the
GeoTIFFs; the geometry goes on as Overture's WKB, the heights as doubles.

Read (from the NAS, never written):
  - sources/overture/<release>/: the downloaded files (buildings.json) and the row groups of each
    whose box meets T (footers.json.gz), buildings and building parts; of their rows, those whose
    box meets T (parts: T grown by PART_MARGIN_DEG, so an outline in T finds its parts);
  - sources/ghsl/R2023A/: the window under T of each GHSL tile meeting it (index.json).

Written (stdout, little-endian): b"BLDP1\\n", then frames, each a u8 kind, a u32 header length, the
header (JSON: its own fields, and "cols": [[name, bytes], ...]), then each column's bytes in order:
  3  a GHSL window: "name", "transform" (the tile's GDAL geotransform), "col_off", "row_off",
     "width", "height"; column data: f32 rows, north first (0 where the tile has no value);
  1  a row group's buildings: "n", "src", "k", "of", "dicts" ({class, subtype, roof, dataset}:
     the codes' strings); columns id (36 ASCII bytes a row), geom_off (u64, n + 1) and geom (WKB),
     height and min_height (f64, NaN none), num_floors and min_floor (i32, -2^31 none), class,
     subtype and roof (i32 codes, -1 none), has_parts and is_underground (u8), fds and hds (i32: the
     footprint's and the height's dataset, the first source with property "" or none, and with
     "/properties/height"; -1 none), rid_off (u64, n + 1) and rid (the footprint source's record_id);
  2  a row group's parts: as buildings but without class, subtype and has_parts, with parent (36
     bytes a row: building_id);
  9  the end: "release", "tile", "files" ([name, etag, [row groups]]), "ghsl" (tile names), "rows".

usage: bldprep.py --root <NAS project folder> --tile 6/x/y [--release 2026-09-23.1] [--jobs 4]
"""
from __future__ import annotations

import argparse
import gzip
import json
import math
import struct
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.parquet as pq

MAGIC = b"BLDP1\n"
K_BUILDINGS, K_PARTS, K_GHSL, K_END = 1, 2, 3, 9
GHSL_DIR = "sources/ghsl/R2023A"
# Parts are read this far (degrees) around T: an outline whose centroid is in T has its parts.
PART_MARGIN_DEG = 0.02
B_COLS = ["id", "sources", "height", "num_floors", "min_height", "min_floor", "class", "subtype", "roof_shape", "has_parts",
          "is_underground", "geometry", "bbox"]
P_COLS = ["id", "sources", "height", "num_floors", "min_height", "min_floor", "roof_shape", "is_underground", "geometry",
          "building_id", "bbox"]
I32_NONE = -(2 ** 31)


def _str(t) -> bool:
    return pa.types.is_string(t) or pa.types.is_large_string(t) or (pa.types.is_dictionary(t) and _str(t.value_type))


def _struct(**fields):
    def ok(t) -> bool:
        if not pa.types.is_struct(t):
            return False
        have = {t.field(k).name: t.field(k).type for k in range(t.num_fields)}
        return all(k in have and f(have[k]) for k, f in fields.items())
    return ok


# What each column read must be: Overture's types, or what reads them the same way (a float
# column read as integers would be truncated, a string one as numbers fail late).
TYPES = {
    "id": _str, "building_id": _str, "class": _str, "subtype": _str, "roof_shape": _str,
    "height": pa.types.is_floating, "min_height": pa.types.is_floating,
    "num_floors": pa.types.is_integer, "min_floor": pa.types.is_integer,
    "has_parts": pa.types.is_boolean, "is_underground": pa.types.is_boolean,
    "geometry": lambda t: pa.types.is_binary(t) or pa.types.is_large_binary(t),
    "bbox": _struct(xmin=pa.types.is_floating, xmax=pa.types.is_floating, ymin=pa.types.is_floating, ymax=pa.types.is_floating),
    "sources": lambda t: (pa.types.is_list(t) or pa.types.is_large_list(t)) and _struct(property=_str, dataset=_str, record_id=_str)(t.value_type),
}


def log(msg: str) -> None:
    print(f"{time.strftime('%Y-%m-%d %H:%M:%S')} bldprep: {msg}", file=sys.stderr, flush=True)


def tile_box(z: int, x: int, y: int) -> list[float]:
    """Tile z/x/y's box in degrees (w, s, e, n)."""
    n = 2 ** z
    lon = lambda v: v / n * 360.0 - 180.0
    lat = lambda v: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * v / n))))
    return [lon(x), lat(y + 1), lon(x + 1), lat(y)]


def grown(b: list[float], d: float) -> list[float]:
    return [b[0] - d, b[1] - d, b[2] + d, b[3] + d]


def meets(a, b) -> bool:
    return a[0] <= b[2] and a[2] >= b[0] and a[1] <= b[3] and a[3] >= b[1]


class Out:
    """The frames, written in order."""

    def __init__(self, f):
        self.f = f
        self.f.write(MAGIC)

    def frame(self, kind: int, header: dict, cols: list[tuple[str, object]]) -> None:
        header = dict(header)
        header["cols"] = [[name, memoryview(b).nbytes] for name, b in cols]
        h = json.dumps(header, separators=(",", ":")).encode()
        self.f.write(struct.pack("<BI", kind, len(h)))
        self.f.write(h)
        for _, b in cols:
            self.f.write(b)


def ghsl(root: Path, box: list[float], out: Out) -> list[str]:
    """The window under `box` of each GHSL tile meeting it, as frames; the tiles' names."""
    import rasterio
    from rasterio.windows import Window
    index = json.loads((root / GHSL_DIR / "index.json").read_text())["tiles"]
    used = []
    for name in sorted(index):
        if not meets(index[name]["bbox"], box):
            continue
        stem = name.removesuffix(".zip")
        with rasterio.open(f"/vsizip/{root / GHSL_DIR / name}/{stem}.tif") as src:
            gt = src.transform.to_gdal()  # (left, pixel width, 0, top, 0, pixel height < 0)
            c0 = max(0, math.floor((box[0] - gt[0]) / gt[1]))
            c1 = min(src.width, math.floor((box[2] - gt[0]) / gt[1]) + 1)
            r0 = max(0, math.floor((box[3] - gt[3]) / gt[5]))
            r1 = min(src.height, math.floor((box[1] - gt[3]) / gt[5]) + 1)
            if c1 <= c0 or r1 <= r0:
                continue
            w = src.read(1, window=Window(c0, r0, c1 - c0, r1 - r0)).astype("<f4")
            if src.nodata is not None and math.isfinite(src.nodata):
                w[w == np.float32(src.nodata)] = 0
            w[~np.isfinite(w)] = 0
            out.frame(K_GHSL, {"name": name, "transform": list(gt), "col_off": c0, "row_off": r0, "width": c1 - c0, "height": r1 - r0},
                      [("data", w.tobytes())])
            used.append(name)
            log(f"GHSL {stem[-8:]}: {c1 - c0} × {r1 - r0} pixels ({src.dtypes[0]}, nodata {src.nodata})")
    return used


def codes(col) -> tuple[np.ndarray, list]:
    """A string column as i32 codes (-1 none) and their strings."""
    d = pc.dictionary_encode(col)
    if isinstance(d, pa.ChunkedArray):
        d = d.combine_chunks()
    return pc.fill_null(d.indices, -1).to_numpy(zero_copy_only=False).astype("<i4"), d.dictionary.to_pylist()


def strings(values: list) -> tuple[np.ndarray, bytes]:
    """Strings as u64 offsets and their UTF-8 bytes."""
    enc = [(v or "").encode() for v in values]
    off = np.zeros(len(enc) + 1, "<u8")
    np.cumsum([len(e) for e in enc], out=off[1:])
    return off, b"".join(enc)


def ids36(col) -> bytes:
    """A column of UUIDs as 36 ASCII bytes a row (none: zeros, which aren't a UUID)."""
    v = pc.fill_null(col, "0" * 36).to_pylist()
    bad = [s for s in v if len(s) != 36]
    if bad:
        raise SystemExit(f"ids aren't UUIDs: {bad[:3]}")
    return "".join(v).encode()


_local = threading.local()


def read_rg(job):
    """One row group's rows whose box meets the job's box, as a frame's header and columns."""
    path, rg, part, box, src = job
    pf = getattr(_local, "pf", None)
    if pf is None or _local.path != path:
        _local.pf, _local.path = pq.ParquetFile(path), path
        pf = _local.pf
    t = pf.read_row_group(rg, columns=P_COLS if part else B_COLS)
    names = set(t.column_names)
    want = set(P_COLS if part else B_COLS)
    if want - names:
        raise SystemExit(f"{src}: Overture's columns changed: no {sorted(want - names)}")
    retyped = [f"{c} ({t.schema.field(c).type})" for c in sorted(want) if not TYPES[c](t.schema.field(c).type)]
    if retyped:
        raise SystemExit(f"{src}: Overture's columns changed type: {', '.join(retyped)}")
    bb = t.column("bbox")
    f = lambda k: pc.struct_field(bb, k)
    keep = pc.and_(pc.and_(pc.less_equal(f("xmin"), box[2]), pc.greater_equal(f("xmax"), box[0])),
                   pc.and_(pc.less_equal(f("ymin"), box[3]), pc.greater_equal(f("ymax"), box[1])))
    t = t.filter(pc.fill_null(keep, False))
    n = t.num_rows
    if not n:
        return None
    g = t.column("geometry").combine_chunks()
    if pa.types.is_large_binary(g.type):
        offs = np.frombuffer(g.buffers()[1], "<i8")[g.offset:g.offset + n + 1]
    else:
        offs = np.frombuffer(g.buffers()[1], "<i4")[g.offset:g.offset + n + 1].astype("<i8")
    data = memoryview(g.buffers()[2])[offs[0]:offs[-1]] if g.buffers()[2] is not None else b""
    geom_off = (offs - offs[0]).astype("<u8")
    num = lambda c, v, dt: pc.fill_null(t.column(c), v).to_numpy(zero_copy_only=False).astype(dt)
    # Sources: the footprint's (property "" or none) and the height's, the first of each.
    s = t.column("sources").combine_chunks()
    flat = pc.list_flatten(s)
    parent = pc.list_parent_indices(s).to_numpy()
    prop = flat.field("property")
    di, dsets = codes(flat.field("dataset"))
    rec = flat.field("record_id").to_pylist()
    is_h = pc.fill_null(pc.equal(prop, "/properties/height"), False).to_numpy(zero_copy_only=False)
    is_p = pc.fill_null(pc.or_kleene(pc.is_null(prop), pc.equal(prop, "")), False).to_numpy(zero_copy_only=False)
    fds = np.full(n, -1, "<i4")
    hds = np.full(n, -1, "<i4")
    rid = [""] * n
    # Each row's first footprint source and first height source (np.unique: first occurrences).
    pi = np.flatnonzero(is_p)
    rows_p, first_p = np.unique(parent[pi], return_index=True)
    fds[rows_p] = di[pi[first_p]]
    for r, k in zip(rows_p.tolist(), pi[first_p].tolist()):
        rid[r] = rec[k] or ""
    hi = np.flatnonzero(is_h)
    rows_h, first_h = np.unique(parent[hi], return_index=True)
    hds[rows_h] = di[hi[first_h]]
    rid_off, rid_b = strings(rid)
    roof, roofs = codes(t.column("roof_shape"))
    cols = [
        ("id", ids36(t.column("id"))),
        ("geom_off", geom_off.tobytes()), ("geom", data),
        ("height", num("height", float("nan"), "<f8").tobytes()), ("min_height", num("min_height", float("nan"), "<f8").tobytes()),
        ("num_floors", num("num_floors", I32_NONE, "<i4").tobytes()), ("min_floor", num("min_floor", I32_NONE, "<i4").tobytes()),
        ("roof", roof.tobytes()), ("is_underground", num("is_underground", False, "u1").tobytes()),
        ("fds", fds.tobytes()), ("hds", hds.tobytes()), ("rid_off", rid_off.tobytes()), ("rid", rid_b),
    ]
    dicts = {"roof": roofs, "dataset": dsets, "class": [], "subtype": []}
    if part:
        cols.append(("parent", ids36(t.column("building_id"))))
    else:
        cls, classes = codes(t.column("class"))
        sub, subtypes = codes(t.column("subtype"))
        cols += [("class", cls.tobytes()), ("subtype", sub.tobytes()), ("has_parts", num("has_parts", False, "u1").tobytes())]
        dicts.update({"class": classes, "subtype": subtypes})
    return {"n": n, "src": src, "dicts": dicts}, cols


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", required=True, help="the NAS project folder (read only)")
    ap.add_argument("--tile", required=True, help="the z6 tile, 6/x/y")
    ap.add_argument("--release", default="2026-09-23.1")
    ap.add_argument("--jobs", type=int, default=4, help="row groups read at once")
    a = ap.parse_args()
    z, x, y = (int(v) for v in a.tile.replace("-", "/").split("/"))
    if z != 6:
        raise SystemExit(f"{a.tile}: not a z6 tile")
    root = Path(a.root)
    base = root / "sources/overture" / a.release.replace(".", "-")
    box = tile_box(z, x, y)
    pbox = grown(box, PART_MARGIN_DEG)
    t0 = time.time()
    out = Out(open(sys.stdout.fileno(), "wb", buffering=8 << 20, closefd=False))
    listed = json.loads((base / "buildings.json").read_text())["files"]
    footers = json.loads(gzip.decompress((base / "footers.json.gz").read_bytes()))
    # The row groups meeting T, file by file in name order.
    jobs, files = [], []
    for name in sorted(listed):
        part = "type=building_part/" in name
        foot = footers.get(f"release/{a.release}/{name}")
        if foot is None or foot.get("etag") != listed[name]["etag"]:
            raise SystemExit(f"{name}: footers.json.gz doesn't list it as downloaded (etag)")
        b = pbox if part else box
        rgs = [k for k, g in enumerate(foot["rgs"]) if meets(g, b)]
        if rgs:
            files.append([name, listed[name]["etag"], rgs])
            jobs += [(str(base / name), k, part, b, f"{name.rsplit('/', 1)[-1][:10]}#{k}") for k in rgs]
    log(f"{a.tile}: {len(jobs)} row groups in {len(files)} files")
    used = ghsl(root, box, out)
    rows = 0
    with ThreadPoolExecutor(a.jobs) as ex:
        ahead = 2 * a.jobs
        futs = [ex.submit(read_rg, j) for j in jobs[:ahead]]
        for k in range(len(jobs)):
            r = futs[k].result()
            futs[k] = None
            if k + ahead < len(jobs):
                futs.append(ex.submit(read_rg, jobs[k + ahead]))
            if r is None:
                continue
            header, cols = r
            header.update(k=k, of=len(jobs))
            rows += header["n"]
            out.frame(K_PARTS if jobs[k][2] else K_BUILDINGS, header, cols)
            if (k + 1) % 50 == 0:
                log(f"{k + 1}/{len(jobs)} row groups, {rows:,} rows ({time.time() - t0:.0f} s)")
    out.frame(K_END, {"release": a.release, "tile": a.tile, "files": files, "ghsl": used, "rows": rows}, [])
    out.f.flush()
    log(f"{a.tile}: {rows:,} rows from {len(jobs)} row groups in {time.time() - t0:.0f} s")


if __name__ == "__main__":
    main()
