#!/usr/bin/env python3
"""The US coasts' depth source's grids (docs/inputs.md §6.10): NOAA's BlueTopo, 4–16 m tiles in UTM
(band 1 elevation above NAVD88, negative below), from its public bucket by its tile scheme.

- The scheme (a GeoPackage: each tile's footprint, links, delivered date and SHA-256) is fetched
  (`--scheme <url>`) or read (`--scheme <file>`); the tiles meeting `--box` are fetched into
  `--cache` (at most one request at a time) and checked against their
  checksums. `--from <dir>` reads tiles already downloaded (their names as the bucket has them).
- Heights → MSL by the tide stations' MSL − NAVD88 (`--offset`, a harmonic surface over the water);
  the chart's low water (MLLW) below MSL beside it (`--low`).
- Each tile is a grid at its own zoom (4 m → 15 here, as the owner chose; 8 m → 14; 16 m → 13).

usage: bluetopo.py --scheme <gpkg or url> --box w,s,e,n --out <dir> --index <file>
                   (--cache <dir> | --from <dir>) [--offset <csv>:<column>] [--low <csv>:<column>]
"""
from __future__ import annotations

import argparse
import hashlib
import sqlite3
from pathlib import Path

from grid import Writer, fetch, log
from datums import from_table
from raster import Spec, parse_table, produce

# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): BlueTopo's published scheme and tiles.
FETCHES = "data"


def scheme_tiles(gpkg: Path, box) -> list[dict]:
    con = sqlite3.connect(f"file:{gpkg}?mode=ro", uri=True)
    (table,) = con.execute("select table_name from gpkg_contents where data_type = 'features'").fetchone()
    rows = con.execute(
        f'select t.tile, t."GeoTIFF_Link", t."Delivered_Date", t."GeoTIFF_SHA256_Checksum" from "{table}" t '
        f'join "rtree_{table}_geom" r on r.id = t.fid where r.maxx >= ? and r.minx <= ? and r.maxy >= ? and r.miny <= ? and t."GeoTIFF_Link" is not null',
        (box[0], box[2], box[1], box[3])).fetchall()
    return [{"tile": r[0], "tif": r[1], "date": r[2], "sha": r[3]} for r in rows]


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scheme", required=True)
    ap.add_argument("--box", required=True)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--index", required=True, type=Path)
    ap.add_argument("--cache", type=Path)
    ap.add_argument("--from", dest="src", type=Path)
    ap.add_argument("--offset")
    ap.add_argument("--low")
    a = ap.parse_args()
    box = [float(v) for v in a.box.split(",")]
    here = a.src or a.cache
    if here is None:
        raise SystemExit("--cache or --from")
    gpkg = Path(a.scheme) if not a.scheme.startswith("http") else fetch(a.scheme, here / a.scheme.rsplit("/", 1)[1])
    tiles = scheme_tiles(gpkg, box)
    log(f"{len(tiles)} tiles meet the box")
    w = Writer(a.out)
    have = []
    for t in tiles:
        name = t["tif"].rsplit("/", 1)[1]
        tif = here / name
        if a.src is None:
            fetch(t["tif"], tif)
            if t["sha"] and sha256(tif) != t["sha"].lower():
                tif.unlink()
                raise SystemExit(f"{name}: its checksum isn't the scheme's")
        if not tif.exists():
            log(f"{name}: not here, left out")
            continue
        have.append((t, tif, name))
    # (The datum surfaces once, over the water of every tile.)
    tifs = [h[1] for h in have]
    offset, low = from_table(parse_table(a.offset), tifs), from_table(parse_table(a.low), tifs)
    for t, tif, name in have:
        spec = Spec(band=1, offset=offset, low=low)
        produce(tif, spec, w, t["tif"], name.removesuffix(".tiff"))
    w.write_index(a.index)


if __name__ == "__main__":
    main()
