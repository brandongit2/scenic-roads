#!/usr/bin/env python3
"""EMODnet's DTM 2024, MSL version (docs/inputs.md §6.10): its ESRI ASCII tiles, one at a time, from
a tile list (a CSV with `dtm_tile`, `product_format`, `download_url`; the rows "ESRI ASCII Mean Sea
Level" read): the zip fetched (grid.fetch), its .asc taken out, given a WGS 84 .prj (the ASCII grids
carry no CRS; EMODnet's DTM is lon/lat on WGS 84), produced (raster.py: zoom by its cells, cut by z6
tile, the grid's own NODATA_value; land, > 0, left out), and the zip and grid deleted.

usage: emodnet.py <tiles.csv> <work dir>   (grids into <work>/out, the index <work>/emodnet-dtm-grids.geojson)
"""
from __future__ import annotations

import csv
import sys
import zipfile
from pathlib import Path

from grid import Writer, fetch, log
from raster import Spec, produce

WGS84 = 'GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]'


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    with open(sys.argv[1], newline="") as f:
        rows = [r for r in csv.DictReader(f) if r["product_format"] == "ESRI ASCII Mean Sea Level"]
    work = Path(sys.argv[2])
    tmp = work / "tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    w = Writer(work / "out")
    log(f"{len(rows)} tiles")
    for r in rows:
        u, tile = r["download_url"], r["dtm_tile"]
        z = fetch(u, tmp / u.rsplit("/", 1)[1], pause=1.0)
        with zipfile.ZipFile(z) as zf:
            m = next(n for n in zf.namelist() if n.lower().endswith(".asc"))
            asc = tmp / Path(m).name
            with zf.open(m) as s, open(asc, "wb") as o:
                while b := s.read(1 << 22):
                    o.write(b)
        prj = asc.with_suffix(".prj")
        prj.write_text(WGS84)
        n = produce(asc, Spec(valid=(-12000.0, 0.0), piece_zoom=6), w, u, f"emodnet-{tile}")
        log(f"{tile}: {n} grids")
        for p in (asc, prj, z):
            p.unlink()
    w.write_index(work / "emodnet-dtm-grids.geojson")


if __name__ == "__main__":
    main()
