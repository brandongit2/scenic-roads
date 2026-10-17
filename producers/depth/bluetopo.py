#!/usr/bin/env python3
"""The US coasts' depth source's grids (docs/inputs.md §6.10): NOAA's BlueTopo, 4–16 m tiles in UTM
(band 1 elevation above NAVD88, negative below; band 3 the contributing survey, explained by each
tile's raster attribute table), from its public bucket by its tile scheme.

- The scheme (a GeoPackage: each tile's footprint, links, delivered date and SHA-256) is fetched
  (`--scheme <url>`) or read (`--scheme <file>`); the tiles meeting `--box` are fetched into
  `--cache` (after the host's robots.txt, at most one request at a time) and checked against their
  checksums. `--from <dir>` reads tiles already downloaded (their names as the bucket has them).
- Heights → MSL by the tide stations' MSL − NAVD88 (`--offset`, a harmonic surface over the water);
  the chart's low water (MLLW) below MSL beside it (`--low`).
- Cells whose contributor is interpolated (the attribute table's survey id ends `.interpolated`,
  or holds `_interp`, or is value 0) are capped at zoom 12.
- Each tile is a grid at its own zoom (4 m → 15 here, as the owner chose; 8 m → 14; 16 m → 13).

usage: bluetopo.py --scheme <gpkg or url> --box w,s,e,n --out <dir> --index <file>
                   (--cache <dir> | --from <dir>) [--offset <csv>:<column>] [--low <csv>:<column>]
"""
from __future__ import annotations

import argparse
import hashlib
import sqlite3
import xml.etree.ElementTree as ET
from pathlib import Path

import numpy as np

from grid import Writer, fetch, log
from datums import from_table
from raster import Spec, parse_table, produce

INTERPOLATED_ZOOM = 12


def scheme_tiles(gpkg: Path, box) -> list[dict]:
    con = sqlite3.connect(f"file:{gpkg}?mode=ro", uri=True)
    (table,) = con.execute("select table_name from gpkg_contents where data_type = 'features'").fetchone()
    rows = con.execute(
        f'select t.tile, t."GeoTIFF_Link", t."RAT_Link", t."Delivered_Date", t."GeoTIFF_SHA256_Checksum" from "{table}" t '
        f'join "rtree_{table}_geom" r on r.id = t.fid where r.maxx >= ? and r.minx <= ? and r.maxy >= ? and r.miny <= ? and t."GeoTIFF_Link" is not null',
        (box[0], box[2], box[1], box[3])).fetchall()
    return [{"tile": r[0], "tif": r[1], "rat": r[2], "date": r[3], "sha": r[4]} for r in rows]


def interpolated_values(aux: bytes) -> list[float]:
    """The contributor values a tile's raster attribute table marks interpolated."""
    root = ET.fromstring(aux)
    rat = root.find(".//GDALRasterAttributeTable")
    if rat is None:
        return []
    names = [fd.find("Name").text for fd in rat.findall("FieldDefn")]
    out = []
    for r in rat.findall("Row"):
        d = dict(zip(names, [x.text for x in r.findall("F")]))
        sid = d.get("source_survey_id") or ""
        if d.get("value") == "0" or sid.endswith(".interpolated") or "_interp" in sid:
            out.append(float(d["value"]))
    return out


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
        tif, aux = here / name, here / (name + ".aux.xml")
        if a.src is None:
            fetch(t["tif"], tif)
            fetch(t["rat"], aux)
            if t["sha"] and sha256(tif) != t["sha"].lower():
                tif.unlink()
                raise SystemExit(f"{name}: its checksum isn't the scheme's")
        if not tif.exists():
            log(f"{name}: not here, left out")
            continue
        have.append((t, tif, aux, name))
    # (The datum surfaces once, over the water of every tile.)
    tifs = [h[1] for h in have]
    offset, low = from_table(parse_table(a.offset), tifs), from_table(parse_table(a.low), tifs)
    for t, tif, aux, name in have:
        interp = np.asarray(interpolated_values(aux.read_bytes()) if aux.exists() else [], "float32")
        spec = Spec(band=1, flags=[(None, 3, lambda c, v=interp: np.isin(c, v), INTERPOLATED_ZOOM)], offset=offset, low=low)
        produce(tif, spec, w, t["tif"], name.removesuffix(".tiff"))
    w.write_index(a.index)


if __name__ == "__main__":
    main()
