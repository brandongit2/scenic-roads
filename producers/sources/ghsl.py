#!/usr/bin/env python3
"""The building-heights source's index (docs/inputs.md §6.9, the `building-heights` role): GHSL's
GHS-BUILT-H R2023A tiles (average net building height, 3 arc-seconds, EPSG:4326), from its tiles
folder's listing. A GeoJSON FeatureCollection, a feature per tile zip, its `url` and its footprint.

The tiles' grid is read here, never in the app: a zip named `<product>_R<row>_C<col>.zip` is the
10° square `GRID_DEG` × (row − 1) below `GRID_TOP` and × (col − 1) right of `GRID_LEFT` (R1_C1's
corner, read from R1_C8's GeoTIFF); squares wholly beyond the poles are left out.

Add the index beside its declaration in inputs/sources/; the input check reads it.

usage: ghsl.py --out <file>
"""
import argparse
import json
import re
import urllib.request

FOLDER = "https://jeodpp.jrc.ec.europa.eu/ftp/jrc-opendata/GHSL/GHS_BUILT_H_GLOBE_R2023A/GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_4326_3ss/V1-0/tiles/"
PRODUCT = "GHS_BUILT_H_ANBH_E2018_GLOBE_R2023A_4326_3ss_V1_0"
GRID_TOP, GRID_LEFT, GRID_DEG = 89.09958317764332, -180.00791620856731, 10.0
UA = {"User-Agent": "scenic-roads/0.1 (personal offline map)"}
# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): the release's published tile folder.
FETCHES = "data"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    with urllib.request.urlopen(urllib.request.Request(FOLDER, headers=UA), timeout=120) as r:
        page = r.read().decode("utf-8", "replace")
    names = sorted(set(re.findall(rf'href="({PRODUCT}_R(\d+)_C(\d+)\.zip)"', page)))
    if not names:
        raise SystemExit(f"no tiles listed at {FOLDER}")
    feats = []
    for name, r, c in names:
        top = GRID_TOP - GRID_DEG * (int(r) - 1)
        left = GRID_LEFT + GRID_DEG * (int(c) - 1)
        if top - GRID_DEG < -90 or top > 90.5:
            continue
        b = [left, top - GRID_DEG, left + GRID_DEG, top]
        ring = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]], [b[0], b[1]]]
        feats.append({"type": "Feature", "properties": {"url": FOLDER + name}, "geometry": {"type": "Polygon", "coordinates": [ring]}})
    with open(a.out, "w") as f:
        json.dump({"type": "FeatureCollection", "features": feats}, f, separators=(",", ":"))
        f.write("\n")
    print(f"{len(feats)} tiles")


if __name__ == "__main__":
    main()
