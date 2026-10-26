#!/usr/bin/env python3
"""A depth source's grids from rasters it serves as files (docs/inputs.md §6.10): the generic
producer the per-source ones call, and runnable on its own for any source that is a few GeoTIFFs
(or anything GDAL reads) in one datum.

Each raster is warped onto the Web Mercator grid of its own zoom (the coarsest whose pixels are no
larger than 1.5 × its cells at its middle latitude, or `--zoom`), averaged; its values made metres
below the water surface (× `--scale`: −1 for depths positive down, 0.3048 for feet), then brought to
mean sea level by a datum surface solved from tide stations (`--offset`: the value of MSL above the
raster's datum, subtracted), a chart's low water below MSL kept beside it for the "dries" flag
(`--low`), and cells a flag raster marks capped at a coarser zoom (`--flag`). Cut by z6 tile when
`--piece-zoom` says so.

usage: raster.py --src <raster> [--src …] --out <dir> --index <file> --url <origin>
                 [--band 1] [--scale 1] [--valid lo,hi] [--zoom z] [--piece-zoom 6]
                 [--flag <raster>:<band>:<v1,v2,…>:<zoom>] [--flag-nonzero <raster>:<band>:<zoom>]
                 [--offset <stations.csv>:<column>[:<skip ids>[:<id column>]]] [--low …, the same]
                 [--low-raster <raster>] [--name <prefix>]
"""
from __future__ import annotations

import argparse
import math
from pathlib import Path

import numpy as np

from datums import from_table
from grid import Window, Writer, log, native_zoom, tiles_of, warp_onto


def bounds_merc(ds) -> tuple[float, float, float, float]:
    from rasterio.warp import transform_bounds

    return transform_bounds(ds.crs, "EPSG:3857", *ds.bounds, densify_pts=21)


def cell_m(ds) -> float:
    """A raster's mean cell size in ground metres."""
    if ds.crs.is_geographic:
        lat = (ds.bounds.top + ds.bounds.bottom) / 2
        return math.sqrt(abs(ds.res[0]) * 111320 * math.cos(math.radians(lat)) * abs(ds.res[1]) * 110574)
    if ds.crs.to_epsg() == 3857:
        lat = math.degrees(2 * math.atan(math.exp((ds.bounds.top + ds.bounds.bottom) / 2 / 6378137)) - math.pi / 2)
        return math.sqrt(abs(ds.res[0] * ds.res[1])) * math.cos(math.radians(lat))
    return math.sqrt(abs(ds.res[0] * ds.res[1]))


def mid_lat(ds) -> float:
    from rasterio.warp import transform_bounds

    b = transform_bounds(ds.crs, "EPSG:4326", *ds.bounds)
    return (b[1] + b[3]) / 2


class Spec:
    """How one source's rasters become depth grids."""

    def __init__(self, band=1, scale=1.0, valid=None, zoom=None, piece_zoom=None, flags=(), offset=None, low=None, nodata=None, resampling=None, low_raster=None, top=None):
        self.band, self.scale, self.valid, self.zoom, self.piece_zoom = band, scale, valid, zoom, piece_zoom
        # Each flag: (raster or None for the source itself, band, a function of its values → bool, zoom).
        self.flags = list(flags)
        # Datum surfaces (datums.Surface): MSL above the raster's datum; a chart's low water below MSL.
        self.offset, self.low, self.nodata, self.resampling = offset, low, nodata, resampling
        # Values above `top` once brought to MSL left out (`valid` is on the raster's own datum).
        self.top = top
        # Or the low water as a raster of its own (the chart's datum below MSL, m ≥ 0: a source's own
        # separation, its MSL grid minus its chart-datum grid).
        self.low_raster = low_raster


def produce(src: Path, spec: Spec, writer: Writer, url: str, name: str) -> int:
    """`src` as depth grids into `writer`; how many it wrote."""
    import rasterio

    with rasterio.open(src) as ds:
        z = spec.zoom if spec.zoom is not None else native_zoom(cell_m(ds), mid_lat(ds))
        whole = Window.around(z, bounds_merc(ds))
        rs = spec.resampling or "average"
        n = 0
        pieces = list(tiles_of(whole, spec.piece_zoom)) if spec.piece_zoom is not None else [(None, None, whole)]
        for tx, ty, win in pieces:
            v = warp_onto(ds, spec.band, win, rs, src_nodata=spec.nodata)
            if spec.valid is not None:
                v[(v < spec.valid[0]) | (v > spec.valid[1])] = np.nan
            if not np.isfinite(v).any():
                continue
            v = v * spec.scale
            if spec.offset is not None:
                v = v - spec.offset.on(win)
            if spec.top is not None:
                v[v > spec.top] = np.nan
                if not np.isfinite(v).any():
                    continue
            low = spec.low.on(win) if spec.low is not None else None
            if spec.low_raster is not None:
                from scipy.ndimage import distance_transform_edt

                low = warp_onto(spec.low_raster, 1, win, "bilinear")
                gap = ~np.isfinite(low)
                if gap.all():
                    low = None
                elif gap.any():
                    low = low[tuple(distance_transform_edt(gap, return_distances=False, return_indices=True))]
            zoom = None
            for fpath, fband, flagged, fz in spec.flags:
                f = warp_onto(ds if fpath is None else fpath, fband, win, "nearest")
                if zoom is None:
                    zoom = np.full(v.shape, z, "uint8")
                zoom[flagged(f) & np.isfinite(v)] = fz
            piece = name if tx is None else f"{name}-{spec.piece_zoom}-{tx}-{ty}"
            if writer.put(piece, url, win, v, zoom, low):
                n += 1
                log(f"{piece}: z{z}, {np.isfinite(v).sum():,} cells, {np.nanmin(v):.1f} to {np.nanmax(v):.1f} m")
        return n


def parse_table(s: str | None):
    if not s:
        return None
    parts = s.split(":") + ["", ""]
    return (parts[0], parts[1], frozenset(x for x in parts[2].split(",") if x), parts[3] or "id")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--src", action="append", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", required=True, type=Path)
    ap.add_argument("--url", required=True)
    ap.add_argument("--band", type=int, default=1)
    ap.add_argument("--scale", type=float, default=1.0)
    ap.add_argument("--valid")
    ap.add_argument("--zoom", type=int)
    ap.add_argument("--piece-zoom", type=int)
    ap.add_argument("--flag", action="append", default=[])
    ap.add_argument("--flag-nonzero", action="append", default=[])
    ap.add_argument("--offset")
    ap.add_argument("--low")
    ap.add_argument("--low-raster", type=Path)
    ap.add_argument("--name")
    a = ap.parse_args()
    flags = []
    for f in a.flag:
        p, b, vals, z = f.rsplit(":", 3)
        vs = np.asarray([float(x) for x in vals.split(",")], "float32")
        flags.append((p, int(b), lambda arr, vs=vs: np.isin(arr, vs), int(z)))
    for f in a.flag_nonzero:
        p, b, z = f.rsplit(":", 2)
        flags.append((p, int(b), lambda arr: np.isfinite(arr) & (arr != 0), int(z)))
    spec = Spec(band=a.band, scale=a.scale, valid=[float(x) for x in a.valid.split(",")] if a.valid else None, zoom=a.zoom, piece_zoom=a.piece_zoom,
                flags=flags, offset=from_table(parse_table(a.offset), a.src), low=from_table(parse_table(a.low), a.src), low_raster=a.low_raster)
    w = Writer(a.out)
    for s in a.src:
        produce(s, spec, w, a.url, a.name or s.stem)
    w.write_index(a.index)


if __name__ == "__main__":
    main()
