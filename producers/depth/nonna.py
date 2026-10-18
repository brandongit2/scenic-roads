#!/usr/bin/env python3
"""Canada's depth sources' grids (docs/inputs.md §6.10): CHS's NONNA-100 (100 m) and NONNA-10
(10 m), from CHS's keyless coverage service (WCS 2.0.1, EPSG:3857 subsets), heights above chart
datum (negative below it; drying heights positive). Its licence (accepted by the owner) asks every
product made from it to carry its notice; the declaration carries it, never redistributed.

- `--box` is fetched in blocks (`--block` degrees, 0.5 × 0.3 for NONNA-10 as the service's own
  index cells are) into `--cache`, after the host's robots.txt, one request at a time; a block with
  no data (the service sends it as nodata) makes no grid, and a failed request stops the run. `--from <tif>…` reads blocks already fetched.
- Chart datum → MSL: the stations' mean water level above chart datum (stations.py chs), a harmonic
  surface over the water, subtracted; the same is the chart's low water below MSL (`low_file`).
- No per-cell flags: every cell is the grid's own zoom (NONNA-100 a z10 grid, NONNA-10 z13).

usage: nonna.py 10|100 --box w,s,e,n --stations <csv> --out <dir> --index <file>
                (--cache <dir> | --from <tif> …) [--block 0.5,0.3]
"""
from __future__ import annotations

import argparse
from pathlib import Path

from grid import Writer, fetch, lonlat_to_merc
from datums import from_table
from raster import Spec, produce

WCS = "https://nonna-geoserver.data.chs-shc.ca/geoserver/wcs"
# (The coverages' ids hold spaces, sent encoded.)
COVERAGE = {"10": "nonna__NONNA%2010%20Coverage", "100": "nonna__NONNA%20100%20Coverage"}
ZOOM = {"10": 13, "100": 10}


def block_url(cov: str, w: float, s: float, e: float, n: float) -> str:
    x0, y0 = lonlat_to_merc(w, s)
    x1, y1 = lonlat_to_merc(e, n)
    return merc_url(cov, x0, y0, x1, y1)


def merc_url(cov: str, x0: float, y0: float, x1: float, y1: float) -> str:
    """The GetCoverage URL for a subset given in EPSG:3857 metres (a block, or a quarter of one)."""
    return (f"{WCS}?service=WCS&version=2.0.1&request=GetCoverage&coverageId={cov}&format=image/tiff"
            f"&subset=X({float(x0):.2f},{float(x1):.2f})&subset=Y({float(y0):.2f},{float(y1):.2f})&subsettingCrs=http://www.opengis.net/def/crs/EPSG/0/3857")


def spec(which: str, surf) -> Spec:
    """How a block becomes grids, `surf` the stations' mean water level above chart datum. (Nodata:
    the files' own, float32's max.)"""
    return Spec(zoom=ZOOM[which], piece_zoom=None if which == "100" else 8, offset=surf, low=surf)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("which", choices=["10", "100"])
    ap.add_argument("--box", required=True)
    ap.add_argument("--stations", required=True)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", required=True, type=Path)
    ap.add_argument("--cache", type=Path)
    ap.add_argument("--from", dest="src", nargs="+", type=Path)
    ap.add_argument("--block", default="0.5,0.3")
    a = ap.parse_args()
    box = [float(v) for v in a.box.split(",")]
    table = (a.stations, "mwl_above_cd_m", frozenset(), "id")
    w = Writer(a.out)
    if a.src:
        files = [(p, f"{WCS}#{COVERAGE[a.which]}") for p in a.src]
    else:
        if a.cache is None:
            raise SystemExit("--cache or --from")
        bw, bh = (float(v) for v in a.block.split(","))
        files = []
        lat = box[1]
        while lat < box[3]:
            lon = box[0]
            while lon < box[2]:
                b = (lon, lat, min(lon + bw, box[2]), min(lat + bh, box[3]))
                u = block_url(COVERAGE[a.which], *b)
                p = a.cache / f"nonna{a.which}_{b[0]:.2f}_{b[1]:.2f}.tif"
                # (No data comes as a TIFF of nodata, making no grid; a failure stops the run.)
                files.append((fetch(u, p), u))
                lon += bw
            lat += bh
    # (The datum surface once, over the water of every block: chart datum below MSL is both what's
    # subtracted and the low water.)
    s = spec(a.which, from_table(table, [p for p, _ in files]))
    for p, u in files:
        produce(p, s, w, u, f"nonna{a.which}-{p.stem}")
    w.write_index(a.index)


if __name__ == "__main__":
    main()
