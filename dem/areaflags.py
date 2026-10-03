#!/usr/bin/env python3
"""A unit's area flags (grid.areas.u8: PARK, HERITAGE, SPECIAL_AREA, INDIGENOUS bits), rasterised
onto its own z11 grid (grid.idx) from the heritage job's flagged polygons (heritage.py --cover's
area-shapes.geojsonseq), as heritage.py rasterises them for a whole build (docs/phase5.md
"Heritage and area flags"). Only the polygons meeting the grid's extent are read.

usage: areaflags.py <unit_dir> <area-shapes.geojsonseq>
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np
from shapely.geometry import box, shape
from shapely.ops import transform as shp_transform

import heritage


def main() -> None:
    d, src = Path(sys.argv[1]), Path(sys.argv[2])
    tiles = np.fromfile(d / "grid.idx", dtype=np.uint32).reshape(-1, 2)
    if len(tiles) == 0:
        return
    lon = lambda x: x / 2048 * 360 - 180
    lat = lambda y: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y / 2048))))
    extent = box(lon(tiles[:, 0].min()), lat(tiles[:, 1].max() + 1), lon(tiles[:, 0].max() + 1), lat(tiles[:, 1].min()))
    shapes = []
    with open(src, encoding="utf-8") as f:
        for line in f:
            feat = json.loads(line)
            g = shape(feat["geometry"])
            if g.is_empty or not g.intersects(extent):
                continue
            shapes.append((shp_transform(heritage.to_merc, g), int(feat["properties"]["bit"])))
    print(f"areaflags: {len(shapes)} polygons over {len(tiles)} grid tiles", file=sys.stderr)
    heritage.rasterise(d, shapes)


if __name__ == "__main__":
    main()
