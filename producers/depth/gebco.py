#!/usr/bin/env python3
"""The worldwide coarse depth source's grids (docs/inputs.md §6.10): GEBCO's yearly global grid
(15 arc-seconds, heights above mean sea level, negative below) and its type identifier grid (TID:
how each cell was got), from the two zips the owner downloads by hand, read where they lie on the NAS.

Each zip's netCDF is copied out once to `--tmp` (GDAL can't read netCDF-4 inside a zip on macOS);
then each z3 tile with sea is warped onto the zoom-8 Web Mercator grid (averaged), with a zoom per
cell: 8 where the TID says measured (10–17 direct, 70 a pre-made regional grid), 7 where it says
interpolated (41–43, 71) or estimated (40, 44–48, 72). Land (TID 0) has none. The grids are cut by
z3 tile; the index lists them.

usage: gebco.py --grid <GEBCO zip> --tid <TID zip> --tmp <dir> --out <dir> --index <file>
                [--box w,s,e,n] [--keep-tmp]
"""
from __future__ import annotations

import argparse
import shutil
import zipfile
from pathlib import Path

import numpy as np

from grid import Window, Writer, log, tiles_of

ZOOM = 8
FLAGGED_ZOOM = 7
# TID codes (GEBCO's documentation): measured; everything else under the sea is capped.
MEASURED = set(range(10, 18)) | {70}
SOURCE_URL = "https://www.gebco.net/data-products/gridded-bathymetry-data"
CELLS = 240  # a degree's cells


def extract(zip_path: Path, tmp: Path) -> Path:
    with zipfile.ZipFile(zip_path) as z:
        member = next(n for n in z.namelist() if n.lower().endswith(".nc"))
        dest = tmp / member
        if dest.exists() and dest.stat().st_size == z.getinfo(member).file_size:
            return dest
        log(f"copying {member} out of {zip_path.name} ({z.getinfo(member).file_size / 1e9:.1f} GB)")
        tmp.mkdir(parents=True, exist_ok=True)
        with z.open(member) as src, open(str(dest) + ".part", "wb") as out:
            shutil.copyfileobj(src, out, 1 << 22)
        Path(str(dest) + ".part").replace(dest)
        return dest


def read_rows(ds, west: float, south: float, east: float, north: float) -> tuple[np.ndarray, float, float]:
    """The grid's cells in a lon/lat box (rows from the north), with its west and north edges. The
    netCDF's first row is the south (latitude ascending), as GDAL may or may not flip it: read by
    its own transform."""
    from rasterio.windows import Window as RW

    t = ds.transform
    flip = t.e > 0
    c0 = max(0, int(np.floor((west + 180) * CELLS)))
    c1 = min(ds.width, int(np.ceil((east + 180) * CELLS)))
    if flip:
        r0 = max(0, int(np.floor((south + 90) * CELLS)))
        r1 = min(ds.height, int(np.ceil((north + 90) * CELLS)))
        a = ds.read(1, window=RW(c0, r0, c1 - c0, r1 - r0))[::-1]
        return a, -180 + c0 / CELLS, -90 + r1 / CELLS
    r0 = max(0, int(np.floor((90 - north) * CELLS)))
    r1 = min(ds.height, int(np.ceil((90 - south) * CELLS)))
    return ds.read(1, window=RW(c0, r0, c1 - c0, r1 - r0)), -180 + c0 / CELLS, 90 - r0 / CELLS


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--grid", required=True, type=Path)
    ap.add_argument("--tid", required=True, type=Path)
    ap.add_argument("--tmp", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", required=True, type=Path)
    ap.add_argument("--box", help="only this lon/lat box (w,s,e,n)")
    ap.add_argument("--keep-tmp", action="store_true")
    a = ap.parse_args()
    import rasterio
    from rasterio.transform import from_origin

    from grid import lonlat_to_merc, warp_onto

    grid_nc, tid_nc = extract(a.grid, a.tmp), extract(a.tid, a.tmp)
    box = [float(v) for v in a.box.split(",")] if a.box else [-180.0, -85.0511, 180.0, 85.0511]
    x0, y0 = lonlat_to_merc(box[0], box[1])
    x1, y1 = lonlat_to_merc(box[2], box[3])
    whole = Window.around(ZOOM, (float(x0), float(y0), float(x1), float(y1)))
    w = Writer(a.out)
    with rasterio.open(f"netcdf:{grid_nc}:elevation") as g, rasterio.open(f"netcdf:{tid_nc}:tid") as t:
        for tx, ty, win in tiles_of(whole, 3):
            wb, sb, eb, nb = win.bounds_lonlat()
            # A cell's margin around the tile, so averaging at its edge sees its neighbours.
            m = 2 / CELLS
            h, west, north = read_rows(g, wb - m, sb - m, eb + m, nb + m)
            tid, _, _ = read_rows(t, wb - m, sb - m, eb + m, nb + m)
            sea = (tid != 0) & (h < 50)
            if not sea.any():
                continue
            tr = from_origin(west, north, 1 / CELLS, 1 / CELLS)
            hv = np.where(sea, h, np.nan).astype("float32")
            depth = warp_onto((hv, tr, "EPSG:4326"), 1, win, "average", src_nodata=np.nan)
            measured = np.isin(tid, list(MEASURED)).astype("float32")
            frac = warp_onto((np.where(sea, measured, np.nan).astype("float32"), tr, "EPSG:4326"), 1, win, "average", src_nodata=np.nan)
            zoom = np.where(frac >= 0.5, ZOOM, FLAGGED_ZOOM).astype("uint8")
            if w.put(f"3-{tx}-{ty}", SOURCE_URL, win, depth, zoom):
                log(f"z3 tile {tx}/{ty}: {np.isfinite(depth).mean() * 100:.0f} % sea")
    w.write_index(a.index)
    if not a.keep_tmp:
        for p in (grid_nc, tid_nc):
            p.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
