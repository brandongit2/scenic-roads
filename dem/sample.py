#!/usr/bin/env python3
"""Sample national DEMs at every road vertex.

Priority per vertex (first source with valid data wins):
  1. NRCan HRDEM 2 m lidar mosaic, read at its 8 m overview  (Canada, where lidar exists)
  2. USGS 3DEP 1/3 arc-second (~10 m)                         (United States)
  3. NRCan MRDEM 30 m                                         (Canada + border fallback)

Only the COG blocks that contain road vertices are fetched (HTTP range requests); nothing
is stored except the per-vertex results.

Incremental: after a run, a sorted (vertex → elevation, source) cache is kept. Densified
geometry is deterministic, so unchanged roads reproduce identical vertices and are served
from the cache; only new or edited roads trigger DEM downloads.

Outputs are written to *.tmp memory-maps (checkpointed per DEM file, so an interrupted run
resumes) and renamed into place at the end, so a running server is never disturbed.

usage: sample.py <build_dir> [--workers N] [--cache DIR] [--no-cache]
"""
from __future__ import annotations

import argparse
import json
import math
import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

os.environ.update(
    GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR",
    CPL_VSIL_CURL_ALLOWED_EXTENSIONS=".tif",
    GDAL_HTTP_MAX_RETRY="10",
    GDAL_HTTP_RETRY_DELAY="2",
    GDAL_HTTP_TIMEOUT="90",
    VSI_CACHE="FALSE",
    GDAL_CACHEMAX="256",
)

import numpy as np  # noqa: E402
import rasterio  # noqa: E402
from pyproj import Transformer  # noqa: E402
from rasterio.windows import Window  # noqa: E402
from tqdm import tqdm  # noqa: E402

HERE = Path(__file__).resolve().parent
NRCAN = "https://canelevation-dem.s3.ca-central-1.amazonaws.com"
MRDEM = f"/vsicurl/{NRCAN}/mrdem-30/mrdem-30-dtm.tif"
USGS = "/vsicurl/https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/13/TIFF/current/{t}/USGS_13_{t}.tif"

SRC_HRDEM, SRC_3DEP, SRC_MRDEM = 1, 2, 3
NODATA_BELOW = -1000.0  # all three sources use large negative nodata values

_tls = threading.local()


def open_ds(url: str, level: int | None):
    """Thread-local dataset handles (GDAL handles are not thread-safe)."""
    cache = getattr(_tls, "ds", None)
    if cache is None:
        cache = _tls.ds = {}
    key = (url, level)
    ds = cache.get(key)
    if ds is None:
        ds = rasterio.open(url, overview_level=level) if level is not None else rasterio.open(url)
        cache[key] = ds
    return ds


def bilinear(a: np.ndarray, c: np.ndarray, r: np.ndarray) -> np.ndarray:
    """Bilinear sample of block `a` at pixel-centre coords (c, r); edge pixels clamp.
    Falls back to the nearest pixel if any of the four neighbours is nodata."""
    h, w = a.shape
    c0 = np.floor(c).astype(np.int32)
    r0 = np.floor(r).astype(np.int32)
    fx = (c - c0).astype(np.float32)
    fy = (r - r0).astype(np.float32)
    c0c, c1 = np.clip(c0, 0, w - 1), np.clip(c0 + 1, 0, w - 1)
    r0c, r1 = np.clip(r0, 0, h - 1), np.clip(r0 + 1, 0, h - 1)
    v00, v01, v10, v11 = a[r0c, c0c], a[r0c, c1], a[r1, c0c], a[r1, c1]
    val = (v00 * (1 - fx) + v01 * fx) * (1 - fy) + (v10 * (1 - fx) + v11 * fx) * fy
    bad = (v00 < NODATA_BELOW) | (v01 < NODATA_BELOW) | (v10 < NODATA_BELOW) | (v11 < NODATA_BELOW)
    bad |= ~np.isfinite(val)
    if bad.any():
        rn = np.clip(np.rint(r[bad]).astype(np.int32), 0, h - 1)
        cn = np.clip(np.rint(c[bad]).astype(np.int32), 0, w - 1)
        vn = a[rn, cn]
        vn = np.where((vn < NODATA_BELOW) | ~np.isfinite(vn), np.nan, vn)
        val[bad] = vn
    return val


def sample_raster(url, level, idx, px, py, elev, src, code, pool, desc, block=512):
    """Sample one raster at points `idx` (projected coords px, py in the raster's CRS).
    Writes valid samples into elev/src. Returns number of valid samples."""
    ds = open_ds(url, level)
    t = ds.transform
    W, H = ds.width, ds.height
    cf = (px - t.c) / t.a - 0.5
    rf = (py - t.f) / t.e - 0.5
    inside = (cf > -0.5) & (rf > -0.5) & (cf < W - 0.5) & (rf < H - 0.5)
    idx, cf, rf = idx[inside], cf[inside], rf[inside]
    if idx.size == 0:
        return 0
    bx = np.clip(np.floor(cf).astype(np.int64), 0, W - 1) // block
    by = np.clip(np.floor(rf).astype(np.int64), 0, H - 1) // block
    key = by * ((W + block - 1) // block) + bx
    order = np.argsort(key, kind="stable")
    key, idx, cf, rf = key[order], idx[order], cf[order], rf[order]
    starts = np.flatnonzero(np.r_[True, key[1:] != key[:-1]])
    ends = np.r_[starts[1:], key.size]
    nbx = (W + block - 1) // block

    def work(s, e):
        k = int(key[s])
        bxi, byi = k % nbx, k // nbx
        x0, y0 = bxi * block, byi * block
        w, h = min(block, W - x0), min(block, H - y0)
        a = open_ds(url, level).read(1, window=Window(x0, y0, w, h), out_dtype="float32")
        return s, e, bilinear(a, cf[s:e] - x0, rf[s:e] - y0)

    good = 0
    futs = [pool.submit(work, s, e) for s, e in zip(starts, ends)]
    for f in tqdm(as_completed(futs), total=len(futs), desc=desc, unit="blk", leave=False, mininterval=0.5):
        s, e, v = f.result()
        ok = np.isfinite(v)
        ii = idx[s:e][ok]
        elev[ii] = v[ok]
        src[ii] = code
        good += int(ok.sum())
    return good


def usgs_groups(lon, lat, idx):
    """Group point indices by USGS 1° tile name (nXXwYYY = upper-left corner)."""
    if idx.size == 0:
        return {}
    tkey = np.ceil(lat[idx]).astype(np.int64) * 1000 + np.ceil(-lon[idx]).astype(np.int64)
    order = np.argsort(tkey, kind="stable")
    tk = tkey[order]
    starts = np.flatnonzero(np.r_[True, tk[1:] != tk[:-1]])
    groups = {}
    for s, e in zip(starts, np.r_[starts[1:], tk.size]):
        k = int(tk[s])
        groups[f"n{k // 1000:02d}w{k % 1000:03d}"] = idx[order[s:e]]
    return groups


def pack(v: np.ndarray) -> np.ndarray:
    """Vertex (lon_e7, lat_e7) → sortable uint64 key."""
    return ((v[:, 0].astype(np.int64) + 2**31).astype(np.uint64) << np.uint64(32)) | (v[:, 1].astype(np.int64) + 2**31).astype(np.uint64)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build", type=Path)
    ap.add_argument("--workers", type=int, default=48)
    ap.add_argument("--cache", type=Path, default=None)
    ap.add_argument("--no-cache", action="store_true", help="ignore cached elevations (e.g. after a DEM update)")
    args = ap.parse_args()
    b: Path = args.build
    cache_dir: Path = args.cache or (b.parent / "cache")
    t_start = time.time()

    verts = np.memmap(b / "verts.bin", dtype=np.int32, mode="r").reshape(-1, 2)
    n = verts.shape[0]
    stamp = f"{n}:{(b / 'verts.bin').stat().st_mtime_ns}"
    print(f"{n:,} vertices")

    ckpt_path = b / "dem-progress.json"
    ckpt = json.loads(ckpt_path.read_text()) if ckpt_path.exists() else {}
    resume = ckpt.get("stamp") == stamp and (b / "elev.f32.tmp").exists() and (b / "src.u8.tmp").exists()
    done = set(ckpt.get("done", [])) if resume else set()
    elev = np.memmap(b / "elev.f32.tmp", dtype=np.float32, mode="r+" if resume else "w+", shape=(n,))
    src = np.memmap(b / "src.u8.tmp", dtype=np.uint8, mode="r+" if resume else "w+", shape=(n,))

    def mark(name):
        elev.flush()
        src.flush()
        done.add(name)
        ckpt_path.write_text(json.dumps({"stamp": stamp, "done": sorted(done)}))

    if not resume:
        elev[:] = np.nan
        src[:] = 0
        keys = pack(verts)
        ck = cache_dir / "dem-cache.keys.u64"
        if not args.no_cache and ck.exists():
            ckeys = np.memmap(ck, dtype=np.uint64, mode="r")
            celev = np.memmap(cache_dir / "dem-cache.elev.f32", dtype=np.float32, mode="r")
            csrc = np.memmap(cache_dir / "dem-cache.src.u8", dtype=np.uint8, mode="r")
            hit_total = 0
            for i in tqdm(range(0, n, 10_000_000), desc="reuse cached elevations", unit="chunk"):
                k = keys[i : i + 10_000_000]
                pos = np.searchsorted(ckeys, k)
                pos_c = np.minimum(pos, ckeys.size - 1)
                hit = (pos < ckeys.size) & (ckeys[pos_c] == k)
                elev[i : i + 10_000_000][hit] = celev[pos_c[hit]]
                src[i : i + 10_000_000][hit] = csrc[pos_c[hit]]
                hit_total += int(hit.sum())
            print(f"cache: reused {hit_total:,} of {n:,} vertices ({hit_total / max(n, 1) * 100:.1f} %)")
        del keys
        mark("cache")

    miss = np.flatnonzero(np.isnan(elev))
    print(f"{miss.size:,} vertices need DEM sampling")
    lon = verts[miss, 0].astype(np.float64) * 1e-7
    lat = verts[miss, 1].astype(np.float64) * 1e-7
    # Local arrays are indexed by position in `miss`; results are scattered back via `miss`.
    loc_elev = np.full(miss.size, np.nan, np.float32)
    loc_src = np.zeros(miss.size, np.uint8)

    def scatter():
        ok = np.isfinite(loc_elev)
        elev[miss[ok]] = loc_elev[ok]
        src[miss[ok]] = loc_src[ok]

    # Project to EPSG:3979 (Canada Atlas Lambert, used by HRDEM and MRDEM).
    tr = Transformer.from_crs("EPSG:4326", "EPSG:3979", always_xy=True)
    x = np.empty(miss.size, np.float64)
    y = np.empty(miss.size, np.float64)
    step = 5_000_000
    for i in tqdm(range(0, miss.size, step), desc="project → EPSG:3979", unit="chunk"):
        x[i : i + step], y[i : i + step] = tr.transform(lon[i : i + step], lat[i : i + step])

    pool = ThreadPoolExecutor(max_workers=args.workers)
    local = np.arange(miss.size)

    # ---- 1. HRDEM lidar (8 m overview of the 2 m mosaic) ----------------------------
    index = json.loads((HERE / "hrdem_tile_index.geojson").read_text())
    have = {line.split("/")[-1].split("-")[0] for line in (HERE / "hrdem_2m_tiles.txt").read_text().split()}
    tiles = []
    for f in index["features"]:
        tid = f["properties"]["id"]
        if tid not in have:
            continue
        xs = [p[0] for p in f["geometry"]["coordinates"][0]]
        ys = [p[1] for p in f["geometry"]["coordinates"][0]]
        sel = local[(x >= min(xs)) & (x < max(xs)) & (y > min(ys)) & (y <= max(ys))]
        if sel.size:
            tiles.append((tid, sel))
    tiles.sort(key=lambda t: -t[1].size)
    print(f"HRDEM: {len(tiles)} mosaic tiles contain roads to sample")
    for tid, sel in tqdm(tiles, desc="HRDEM lidar tiles", unit="tile"):
        name = f"hrdem:{tid}"
        if name in done:
            continue
        url = f"/vsicurl/{NRCAN}/hrdem-mosaic-2m/{tid}-mosaic-2m-dtm.tif"
        got = sample_raster(url, 1, sel, x[sel], y[sel], loc_elev, loc_src, SRC_HRDEM, pool, f"  {tid} ({sel.size:,} pts)")
        tqdm.write(f"  HRDEM {tid}: {got:,}/{sel.size:,} vertices with lidar")
        scatter()
        mark(name)

    # ---- 2. USGS 3DEP 1/3" -----------------------------------------------------------
    left = local[np.isnan(loc_elev)]
    groups = usgs_groups(lon, lat, left)
    print(f"3DEP: {left.size:,} vertices left, {len(groups)} candidate 1° tiles")
    for tname, sel in tqdm(sorted(groups.items(), key=lambda kv: -kv[1].size), desc="USGS 3DEP tiles", unit="tile"):
        name = f"3dep:{tname}"
        if name in done:
            continue
        try:
            got = sample_raster(USGS.format(t=tname), None, sel, lon[sel], lat[sel], loc_elev, loc_src, SRC_3DEP, pool, f"  {tname} ({sel.size:,} pts)")
        except rasterio.errors.RasterioIOError:
            got = 0  # no 3DEP tile here (Canada / ocean)
        if got:
            tqdm.write(f"  3DEP {tname}: {got:,}/{sel.size:,}")
        scatter()
        mark(name)

    # ---- 3. MRDEM 30 m fallback ---------------------------------------------------------
    if "mrdem" not in done:
        left = local[np.isnan(loc_elev)]
        print(f"MRDEM: {left.size:,} vertices left")
        if left.size:
            got = sample_raster(MRDEM, None, left, x[left], y[left], loc_elev, loc_src, SRC_MRDEM, pool, "MRDEM 30 m blocks")
            print(f"  MRDEM: {got:,}/{left.size:,}")
        scatter()
        mark("mrdem")
    pool.shutdown()

    # ---- finish: stats, atomic rename, refresh cache ---------------------------------
    elev.flush()
    src.flush()
    counts = np.bincount(src, minlength=4)
    stats = {
        "vertices": int(n),
        "hrdem": int(counts[1]),
        "usgs3dep": int(counts[2]),
        "mrdem": int(counts[3]),
        "missing": int(counts[0]),
        "sampled_this_run": int(miss.size),
        "seconds": round(time.time() - t_start, 1),
    }
    del elev, src
    os.replace(b / "elev.f32.tmp", b / "elev.f32")
    os.replace(b / "src.u8.tmp", b / "src.u8")
    (b / "dem-stats.json").write_text(json.dumps(stats, indent=2))
    ckpt_path.unlink(missing_ok=True)

    print("updating elevation cache")
    cache_dir.mkdir(parents=True, exist_ok=True)
    keys = pack(verts)
    order = np.argsort(keys, kind="stable")
    ev = np.memmap(b / "elev.f32", dtype=np.float32, mode="r")
    sv = np.memmap(b / "src.u8", dtype=np.uint8, mode="r")
    for name, arr in [("keys.u64", keys[order]), ("elev.f32", ev[order]), ("src.u8", sv[order])]:
        tmp = cache_dir / f"dem-cache.{name}.tmp"
        arr.tofile(tmp)
        os.replace(tmp, cache_dir / f"dem-cache.{name}")
    print(json.dumps(stats, indent=2))


if __name__ == "__main__":
    main()
