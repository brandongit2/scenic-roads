#!/usr/bin/env python3
"""The canopy source's index (docs/inputs.md §6.9, the `canopy` role): Meta & WRI's global canopy
height, 10° squares, from its bucket's listing. A GeoJSON FeatureCollection, a feature per file the
map reads (its `kind`: `cover5m`, the share taller than 5 m; `p95` and `median`, the heights), its
`url` and its footprint, the 10° square it's named by (its north-west corner).

The bucket's naming quirk is read here, never in the app: most of the equator's row is named
`lat=-0.0` (a few squares' files `lat=0.0`, and at some squares the kinds split between the two), so
a square's corner is the name's latitude as a number (-0.0 is 0); where one file is under both
names, the `lat=0.0` one is listed (as the app asked for it first before the index).

Drop the index beside its declaration in inputs/sources/; the input check reads it.

usage: canopy.py --out <file>
"""
import argparse
import json
import re
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

BUCKET = "https://dataforgood-fb-data.s3.amazonaws.com"
PREFIX = "forests/v1/alsgedi_global_v6_float_epsg4326_v3_10deg/"
KINDS = ("cover5m", "p95", "median")
UA = {"User-Agent": "scenic-roads/0.1 (personal offline map)"}
NAME = re.compile(r"meta_chm_lat=(-?\d+(?:\.\d+)?)_lon=(-?\d+(?:\.\d+)?)_([a-z0-9]+)\.tif$")
NS = "{http://s3.amazonaws.com/doc/2006-03-01/}"


def keys():
    token = None
    while True:
        q = {"list-type": "2", "prefix": PREFIX}
        if token:
            q["continuation-token"] = token
        url = BUCKET + "/?" + urllib.parse.urlencode(q)
        with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=120) as r:
            x = ET.fromstring(r.read())
        for c in x.iter(NS + "Contents"):
            yield c.find(NS + "Key").text
        if x.findtext(NS + "IsTruncated") != "true":
            return
        token = x.findtext(NS + "NextContinuationToken")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    files = {}
    for k in keys():
        m = NAME.search(k)
        if not m or m.group(3) not in KINDS:
            continue
        lat, lon, kind = m.group(1), m.group(2), m.group(3)
        top, left = int(round(float(lat))), int(round(float(lon)))
        at = (top, left, kind)
        # (One file under both spellings of the equator: `lat=0.0`'s.)
        if at in files and lat.startswith("-"):
            continue
        files[at] = f"{BUCKET}/{k}"
    feats = []
    for (top, left, kind), url in sorted(files.items(), key=lambda kv: (-kv[0][0], kv[0][1], kv[0][2])):
        sq = [[left, top - 10], [left + 10, top - 10], [left + 10, top], [left, top], [left, top - 10]]
        feats.append({"type": "Feature", "properties": {"url": url, "kind": kind}, "geometry": {"type": "Polygon", "coordinates": [sq]}})
    with open(a.out, "w") as f:
        json.dump({"type": "FeatureCollection", "features": feats}, f, separators=(",", ":"))
        f.write("\n")
    print(f"{len(feats)} files in {len({(t, l) for t, l, _ in files})} squares")


if __name__ == "__main__":
    main()
