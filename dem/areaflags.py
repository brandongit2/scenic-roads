#!/usr/bin/env python3
"""A unit's area flags (grid.areas.u8: PARK, HERITAGE, SPECIAL_AREA, INDIGENOUS bits), rasterised
onto its own z11 grid (grid.idx) from the heritage-sites job's flagged polygons near it (the
units' slices of heritage.py --tiles' area-shapes.geojsonseq), as heritage.py rasterises them for a
whole build (docs/phase5.md "Heritage and area flags"). Each tile is the union of its polygons'
burns, so units agree wherever their grids overlap.

Polygons are chosen by bounding box: Web Mercator is monotone on each axis, so a polygon whose box
misses the grid's misses it in Mercator too (an exact intersection in degrees could drop one that
touches an edge tile in Mercator).

usage: areaflags.py <unit_dir> <area-shapes.geojsonseq>
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np
from shapely.geometry import shape
from shapely.ops import transform as shp_transform

import heritage


def main() -> None:
    d, src = Path(sys.argv[1]), Path(sys.argv[2])
    tiles = np.fromfile(d / "grid.idx", dtype=np.uint32).reshape(-1, 2)
    if len(tiles) == 0:
        return
    lon = lambda x: x / 2048 * 360 - 180
    lat = lambda y: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y / 2048))))
    w, e = lon(int(tiles[:, 0].min())), lon(int(tiles[:, 0].max()) + 1)
    s, n = lat(int(tiles[:, 1].max()) + 1), lat(int(tiles[:, 1].min()))
    shapes = []
    with open(src, encoding="utf-8") as f:
        for line in f:
            feat = json.loads(line)
            g = shape(feat["geometry"])
            if g.is_empty:
                continue
            x0, y0, x1, y1 = g.bounds
            if x1 < w or x0 > e or y1 < s or y0 > n:
                continue
            shapes.append((shp_transform(heritage.to_merc, g), int(feat["properties"]["bit"])))
    print(f"areaflags: {len(shapes)} polygons over {len(tiles)} grid tiles", file=sys.stderr)
    heritage.rasterise(d, shapes)


if __name__ == "__main__":
    main()
