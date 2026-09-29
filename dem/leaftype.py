#!/usr/bin/env python3
"""Dominant leaf type of forests, on the canopy files' 10° squares (for the tree cover layer).

Classes (u8): 0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data. Written per 10° square
of the Meta canopy cache (data/cache/chm10) that touches our regions, at 0.0005° (~50 m):
data/trees/leaf/lat<top>_lon<left>.tif.

Sources:
  Europe         Copernicus HRL Dominant Leaf Type 2018, 10 m (broadleaf / coniferous), read at
                 0.0005° from the EEA's public image service (exportImage, nearest neighbour).
  North America  NALCMS 2020 land cover, 30 m (CEC; NRCan, USGS, INEGI…): needleleaf forest →
                 conifer, broadleaf deciduous → broadleaf, mixed forest → mixed. The GeoTIFF is
                 streamed out of CEC's 3.9 GB zip by byte range (only the TIFF is kept, and only
                 while squares are made).
  Hong Kong      none (no data).

usage: leaftype.py [eu] [na] [--keep-nalcms]
"""
from __future__ import annotations

import io
import json
import re
import struct
import subprocess
import sys
import time
import zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import rasterio
from rasterio.transform import from_origin
from rasterio.warp import Resampling, reproject
from shapely.geometry import Polygon, box
from shapely.ops import unary_union

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "data" / "cache" / "chm10"
T = ROOT / "data" / "trees"
OUT = T / "leaf"
UA = "road-elevations/0.1 (personal offline map)"
RES = 0.0005
N = int(round(10 / RES))

EEA = "https://image.discomap.eea.europa.eu/arcgis/rest/services/GioLandPublic/HRL_DominantLeafType2018/ImageServer/exportImage"
MAX_W, MAX_H = 7500, 4100
NALCMS_ZIP = "https://www.cec.org/files/atlas_layers/1_terrestrial_ecosystems/1_01_0_land_cover_2020_30m/land_cover_2020v2_30m_tif.zip"
NALCMS_MEMBER_OFFSET = 38952  # local header of …/data/NA_NALCMS_landcover_2020v2_30m.tif
NALCMS_COMPRESSED = 2668629085
NALCMS_TIF = T / "nalcms-2020.tif"
# NALCMS classes → ours.
NA_MAP = np.zeros(256, np.uint8)
NA_MAP[[1, 2]] = 2
NA_MAP[[3, 4, 5]] = 1
NA_MAP[6] = 3
NA_MAP[[0, 127, 255]] = 255


def read_poly(path: Path):
    """Geofabrik .poly → shapely geometry (outer rings minus '!' holes)."""
    outer, holes, cur, name = [], [], None, None
    for line in path.read_text().splitlines()[1:]:
        s = line.strip()
        if not s:
            continue
        if cur is None:
            if s == "END":
                break
            name, cur = s, []
        elif s == "END":
            (holes if name.startswith("!") else outer).append(Polygon(cur))
            cur = None
        else:
            x, y = s.split()[:2]
            cur.append((float(x), float(y)))
    g = unary_union([p.buffer(0) for p in outer])
    return g.difference(unary_union([p.buffer(0) for p in holes])) if holes else g


def regions():
    """Union of the regions' outlines (regions.json): Geofabrik's .poly (data/trees/poly, fetched by
    regionpolys.py), or the bbox of regions taken from Overpass."""
    cfg = json.loads((ROOT / "regions.json").read_text())["regions"]
    polys = []
    for r in cfg:
        p = T / "poly" / f"{r['id']}.poly"
        if p.exists():
            polys.append(read_poly(p))
        elif "bbox" in r:
            polys.append(box(*r["bbox"]))
    return unary_union(polys)


def squares(region):
    """(top, left) of the canopy cache's 10° squares that touch the regions."""
    out = []
    for f in sorted(CACHE.glob("*_cover5m.tif")):
        m = re.search(r"lat=(-?[\d.]+)_lon=(-?[\d.]+)_", f.name)
        top, left = float(m[1]), float(m[2])
        if region.intersects(box(left, top - 10, left + 10, top)):
            out.append((int(top), int(left)))
    return out


def save(top: int, left: int, a: np.ndarray, source: str):
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / f"lat{top}_lon{left}.tif"
    tmp = path.with_suffix(".tmp.tif")
    with rasterio.open(tmp, "w", driver="GTiff", width=N, height=N, count=1, dtype="uint8", crs="EPSG:4326",
                       transform=from_origin(left, top, RES, RES), nodata=255, compress="deflate", tiled=True,
                       blockxsize=512, blockysize=512) as d:
        d.write(a, 1)
        d.update_tags(source=source, classes="0 not forest, 1 broadleaf, 2 conifer, 3 mixed, 255 no data")
    tmp.rename(path)
    counts = np.bincount(a.ravel(), minlength=256)
    print(f"  lat{top}_lon{left}: broadleaf {counts[1] / a.size:.1%}, conifer {counts[2] / a.size:.1%}, mixed {counts[3] / a.size:.1%}, no data {counts[255] / a.size:.1%}")


def eea_chunk(w, s, e, n, width, height) -> np.ndarray | None:
    """One exportImage request; a request that keeps failing is split in four."""
    a = eea_request(w, s, e, n, width, height)
    if a is not None or width < 1000 or height < 1000:
        return a
    hw, hh = width // 2, height // 2
    xm, ym = w + hw * RES, n - hh * RES
    out = np.full((height, width), 255, np.uint8)
    for (r0, c0, h, wd, bb) in [(0, 0, hh, hw, (w, ym, xm, n)), (0, hw, hh, width - hw, (xm, ym, e, n)),
                                (hh, 0, height - hh, hw, (w, s, xm, ym)), (hh, hw, height - hh, width - hw, (xm, s, e, ym))]:
        part = eea_chunk(*bb, wd, h)
        if part is not None:
            out[r0:r0 + h, c0:c0 + wd] = part
    return out


def eea_request(w, s, e, n, width, height) -> np.ndarray | None:
    url = (f"{EEA}?bbox={w},{s},{e},{n}&bboxSR=4326&imageSR=4326&size={width},{height}&format=tiff&pixelType=U8"
           f"&interpolation=RSP_NearestNeighbor&f=image")
    for attempt in range(3):
        r = subprocess.run(["curl", "-sS", "--fail", "-m", "240", "-A", UA, url], capture_output=True)
        if r.returncode == 0 and r.stdout[:2] in (b"II", b"MM"):
            with rasterio.MemoryFile(r.stdout) as mf, mf.open() as d:
                a = d.read(1)
            if a.shape == (height, width):
                return a
        time.sleep(10 * (attempt + 1))
    return None


def europe(region):
    eu = region.intersection(box(-40, 20, 40, 75))
    for top, left in squares(region):
        if left < -40 or left >= 40 or (OUT / f"lat{top}_lon{left}.tif").exists():
            continue
        a = np.full((N, N), 255, np.uint8)
        jobs = []
        for r0 in range(0, N, MAX_H):
            for c0 in range(0, N, MAX_W):
                h, w = min(MAX_H, N - r0), min(MAX_W, N - c0)
                bb = (left + c0 * RES, top - (r0 + h) * RES, left + (c0 + w) * RES, top - r0 * RES)
                if eu.intersects(box(*bb)):
                    jobs.append((r0, c0, h, w, bb))
        print(f"lat{top}_lon{left}: {len(jobs)} EEA requests", flush=True)
        with ThreadPoolExecutor(2) as ex:
            for (r0, c0, h, w, bb), chunk in zip(jobs, ex.map(lambda j: eea_chunk(*j[4], j[3], j[2]), jobs)):
                if chunk is None:
                    print(f"  failed chunk {bb}", file=sys.stderr)
                    continue
                lut = np.full(256, 255, np.uint8)
                lut[[0, 1, 2]] = [0, 1, 2]
                a[r0:r0 + h, c0:c0 + w] = lut[chunk]
        save(top, left, a, "Copernicus HRL Dominant Leaf Type 2018 (EEA), 10 m, read at 0.0005°")


def fetch_nalcms():
    if NALCMS_TIF.exists():
        return
    T.mkdir(parents=True, exist_ok=True)
    head = subprocess.run(["curl", "-sS", "--fail", "-A", UA, "-r", f"{NALCMS_MEMBER_OFFSET}-{NALCMS_MEMBER_OFFSET + 511}", NALCMS_ZIP],
                          capture_output=True, check=True).stdout
    assert head[:4] == b"PK\x03\x04", "unexpected zip layout"
    nlen, elen = struct.unpack("<HH", head[26:30])
    start = NALCMS_MEMBER_OFFSET + 30 + nlen + elen
    tmp = NALCMS_TIF.with_suffix(".part")
    print(f"streaming NALCMS GeoTIFF ({NALCMS_COMPRESSED / 1e9:.1f} GB compressed)…")
    p = subprocess.Popen(["curl", "-sS", "--fail", "-A", UA, "-r", f"{start}-{start + NALCMS_COMPRESSED - 1}", NALCMS_ZIP], stdout=subprocess.PIPE)
    dec = zlib.decompressobj(-15)
    got = 0
    with tmp.open("wb") as f:
        while chunk := p.stdout.read(8 << 20):
            f.write(dec.decompress(chunk))
            got += len(chunk)
            print(f"\r  {got / NALCMS_COMPRESSED:.0%}", end="", flush=True)
        f.write(dec.flush())
    print()
    if p.wait() != 0:
        raise SystemExit("NALCMS download failed")
    tmp.rename(NALCMS_TIF)


def north_america(region, keep: bool):
    todo = [(t, l) for t, l in squares(region) if l < -40 and not (OUT / f"lat{t}_lon{l}.tif").exists()]
    if not todo:
        return
    fetch_nalcms()
    with rasterio.open(NALCMS_TIF) as src:
        for top, left in todo:
            t0 = time.time()
            dst = np.full((N, N), 255, np.uint8)
            # NALCMS has no class 0: it is the background outside the continent (no data).
            reproject(rasterio.band(src, 1), dst, dst_transform=from_origin(left, top, RES, RES), dst_crs="EPSG:4326",
                      resampling=Resampling.nearest, src_nodata=0, dst_nodata=255, num_threads=4)
            save(top, left, NA_MAP[dst], "NALCMS 2020 land cover 30 m (CEC), resampled to 0.0005°")
            print(f"    ({time.time() - t0:.0f} s)")
    if not keep:
        NALCMS_TIF.unlink()


def main():
    args = set(sys.argv[1:])
    region = regions()
    if "eu" in args or not args - {"--keep-nalcms"}:
        europe(region)
    if "na" in args or not args - {"--keep-nalcms"}:
        north_america(region, "--keep-nalcms" in args)


if __name__ == "__main__":
    main()
