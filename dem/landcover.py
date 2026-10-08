#!/usr/bin/env python3
"""Land cover on the z11 analysis grid (grid.idx) from ESA WorldCover 2021 (10 m). Incremental:
tiles done in the last run are copied (see main).

Reads WorldCover's 1/4 overview (~40 m) over each grid tile and samples it at the cell
centres (nearest). Classes are collapsed to roadcore::grid::class:
  1 trees (+ mangroves)   2 shrub   3 open (grass, crop, bare, moss/lichen)
  4 built-up   5 water   6 herbaceous wetland   7 snow/ice   0 no data

With --only (the unit builds, scenic-build unit): classifies just the grid slots listed there
(u32, grid.idx order; the tiles the packs lack) and keeps the rest of grid.class.u8 as staged. No
cross-run cache then: each unit has its own folder.

usage: landcover.py <build_dir> [--workers N] [--only <slots.u32>]
"""
from __future__ import annotations

import argparse
import math
import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

UA = "scenic-roads/0.1 (personal offline map)"
os.environ.update(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_HTTP_MAX_RETRY="10", GDAL_HTTP_RETRY_DELAY="2", VSI_CACHE="FALSE", GDAL_HTTP_USERAGENT=UA)

import numpy as np  # noqa: E402
import rasterio  # noqa: E402
from rasterio.windows import Window  # noqa: E402
from tqdm import tqdm  # noqa: E402

URL = "/vsicurl/https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_{t}_Map.tif"
WORLD = 2**11 * 256
LUT = np.zeros(256, np.uint8)
for wc, c in {10: 1, 95: 1, 20: 2, 30: 3, 40: 3, 60: 3, 100: 3, 50: 4, 80: 5, 90: 6, 70: 7}.items():
    LUT[wc] = c

_tls = threading.local()


def ds(name: str):
    cache = getattr(_tls, "ds", None)
    if cache is None:
        cache = _tls.ds = {}
    if name not in cache:
        url = URL.format(t=name)
        try:
            cache[name] = rasterio.open(url, overview_level=1)
        except rasterio.errors.RasterioIOError:
            if not absent(url):
                raise
            cache[name] = None  # ocean / outside coverage
    return cache[name]


def absent(url: str) -> bool:
    """Whether a WorldCover tile that wouldn't open isn't there at all (S3 answers 404, or 403 for a
    key that doesn't exist): open sea. A timeout, a 5xx, or a tile that's there raises instead, so
    the job fails and is tried again rather than keeping water for good (the units keep their
    results between runs)."""
    import urllib.error
    import urllib.request

    plain = url.replace("/vsicurl/", "")
    last: Exception | None = None
    for attempt in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(plain, method="HEAD", headers={"User-Agent": UA}), timeout=60):
                return False
        except urllib.error.HTTPError as e:
            if e.code in (403, 404):
                return True
            last = e
        except OSError as e:
            last = e
        time.sleep(5 * (attempt + 1))
    raise RuntimeError(f"WorldCover {plain}: {last}")


def wc_name(lat0: int, lon0: int) -> str:
    return f"{'N' if lat0 >= 0 else 'S'}{abs(lat0):02d}{'E' if lon0 >= 0 else 'W'}{abs(lon0):03d}"


def tile_classes(tx: int, ty: int) -> np.ndarray:
    g = np.arange(256) + 0.5
    lon = (tx * 256 + g) / WORLD * 360 - 180
    lat = np.degrees(np.arctan(np.sinh(np.pi * (1 - 2 * (ty * 256 + g) / WORLD))))
    LON, LAT = np.meshgrid(lon, lat)
    out = np.zeros((256, 256), np.uint8)
    # WorldCover tiles are 3° × 3°, named by their south-west corner.
    lat0s = np.floor(LAT / 3).astype(int) * 3
    lon0s = np.floor(LON / 3).astype(int) * 3
    for la in np.unique(lat0s):
        for lo in np.unique(lon0s):
            m = (lat0s == la) & (lon0s == lo)
            if not m.any():
                continue
            d = ds(wc_name(int(la), int(lo)))
            if d is None:
                continue
            t = d.transform
            cols = ((LON[m] - t.c) / t.a).astype(np.int64)
            rows = ((LAT[m] - t.f) / t.e).astype(np.int64)
            c0, c1 = max(0, cols.min()), min(d.width - 1, cols.max())
            r0, r1 = max(0, rows.min()), min(d.height - 1, rows.max())
            if c1 < c0 or r1 < r0:
                continue
            a = d.read(1, window=Window(c0, r0, c1 - c0 + 1, r1 - r0 + 1))
            cc = np.clip(cols - c0, 0, a.shape[1] - 1)
            rr = np.clip(rows - r0, 0, a.shape[0] - 1)
            out[m] = LUT[a[rr, cc]]
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("build", type=Path)
    ap.add_argument("--workers", type=int, default=32)
    ap.add_argument("--only", type=Path)
    args = ap.parse_args()
    b: Path = args.build
    tiles = np.fromfile(b / "grid.idx", dtype=np.uint32).reshape(-1, 2)
    if args.only is not None:
        only(b, tiles, np.fromfile(args.only, dtype=np.uint32), args.workers)
        return
    out = np.memmap(b / "grid.class.u8.tmp", dtype=np.uint8, mode="w+", shape=(len(tiles), 256, 256))
    # Tiles classified in the last run are copied from its grid.class.u8 (still in place; its
    # tile order is in data/cache/steps/landcover.tiles); only new tiles are read from WorldCover.
    cache = b.parent / "cache" / "steps" / "landcover.tiles"
    cache.parent.mkdir(parents=True, exist_ok=True)
    todo = list(range(len(tiles)))
    if cache.exists() and (b / "grid.class.u8").exists():
        prev = np.fromfile(cache, dtype=np.uint32).reshape(-1, 2)
        old = np.memmap(b / "grid.class.u8", dtype=np.uint8, mode="r")
        if old.size == len(prev) * 65536:
            old = old.reshape(-1, 256, 256)
            slot = {(int(x), int(y)): i for i, (x, y) in enumerate(prev)}
            todo = []
            for i, (x, y) in enumerate(tiles):
                j = slot.get((int(x), int(y)))
                if j is None:
                    todo.append(i)
                else:
                    out[i] = old[j]
        del old
    print(f"land cover: {len(tiles)} grid tiles, {len(tiles) - len(todo)} from the last run, {len(todo)} to classify")
    with ThreadPoolExecutor(args.workers) as pool:
        futs = {pool.submit(tile_classes, int(tiles[i][0]), int(tiles[i][1])): i for i in todo}
        for f in tqdm(as_completed(futs), total=len(futs), desc="WorldCover → z11 grid", unit="tile"):
            out[futs[f]] = f.result()
    out.flush()
    counts = np.bincount(np.asarray(out).ravel(), minlength=8)
    del out
    os.replace(b / "grid.class.u8.tmp", b / "grid.class.u8")
    tiles.tofile(cache)
    report(counts)


def only(b: Path, tiles: np.ndarray, slots: np.ndarray, workers: int):
    """Classifies the listed slots into a copy of the staged grid.class.u8."""
    src = np.fromfile(b / "grid.class.u8", dtype=np.uint8)
    if src.size != len(tiles) * 65536:
        raise SystemExit(f"grid.class.u8 has {src.size} cells for {len(tiles)} grid tiles")
    if slots.size and int(slots.max()) >= len(tiles):
        raise SystemExit(f"slot {int(slots.max())} beyond the {len(tiles)} grid tiles")
    out = src.reshape(-1, 256, 256)
    print(f"land cover: {len(tiles)} grid tiles, {len(tiles) - len(slots)} from the packs, {len(slots)} to classify")
    with ThreadPoolExecutor(workers) as pool:
        futs = {pool.submit(tile_classes, int(tiles[i][0]), int(tiles[i][1])): int(i) for i in slots}
        for f in tqdm(as_completed(futs), total=len(futs), desc="WorldCover → z11 grid", unit="tile"):
            out[futs[f]] = f.result()
    out.tofile(b / "grid.class.u8.tmp")
    os.replace(b / "grid.class.u8.tmp", b / "grid.class.u8")
    report(np.bincount(out[slots].ravel(), minlength=8) if slots.size else np.zeros(8, np.int64))


def report(counts: np.ndarray):
    names = ["none", "trees", "shrub", "open", "built", "water", "wetland", "snow"]
    tot = max(int(counts.sum()), 1)
    print("classes:", {n: f"{c / tot * 100:.1f} %" for n, c in zip(names, counts)})


if __name__ == "__main__":
    main()
