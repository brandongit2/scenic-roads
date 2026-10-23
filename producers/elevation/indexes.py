#!/usr/bin/env python3
"""The elevation datasets' tile indexes (docs/inputs.md §6.8, `[data] index`), from each source's own
listing: a GeoJSON FeatureCollection, a feature per file, its `url` and its footprint (a polygon in
the index's CRS), which the app reads as the file's box (min x <= x < max x, min y < y <= max y).

  hrdem   NRCan's HRDEM 2 m mosaic: the tiles the bucket has at 2 m (its hrdem-mosaic-2m/ keys,
          listed; or --tiles, a listing of them) and their 500 km squares from NRCan's tile index
          (--squares, EPSG:3979, as NRCan publishes it: hrdem-squares.geojson here).
  3dep    USGS 3DEP 1/3 arc-second: every USGS_13_<tile>.tif under the bucket's current/ folder
          (listed), its footprint the 1° square it's named by (its upper-left corner).
  fabdem  FABDEM v1-2: every tile inside Bristol's 10° zips (each zip's central directory read by
          range requests), as <zip URL>#<member>, its footprint the 1° square it's named by (its
          south-west corner). The URL only names the tile: FABDEM is read from the store
          (sources/fetched/fabdem/), which `scenic-build fabdem-fetch` fills from Hugging Face's
          mirror (docs/inputs.md §6.8); builds never download it. Bristol's server now resets
          requests, so this listing may not run again; the accepted index stands.
  glo30   Copernicus DEM GLO-30 (the 3D terrain's north of 60°N): every tile in the bucket's
          tileList.txt (or --tiles, a copy of it), its DEM by `url` and its fill mask (FLM, in its
          AUXFILES/) by `mask`, its footprint the 1° square it's named by (its south-west corner).

Drop the index beside its declaration in inputs/elevation/; the input check reads it.

usage: indexes.py hrdem [--tiles <listing.txt>] --squares <index.geojson> --out <file>
       indexes.py 3dep --out <file>
       indexes.py fabdem --out <file>
       indexes.py glo30 [--tiles <tileList.txt>] --out <file>
"""
import argparse
import json
import re
import struct
import sys
import urllib.parse
import urllib.request

NRCAN = "https://canelevation-dem.s3.ca-central-1.amazonaws.com"
USGS = "https://prd-tnm.s3.amazonaws.com"
USGS_PREFIX = "StagedProducts/Elevation/13/TIFF/current/"
BRISTOL = "https://data.bris.ac.uk/datasets/s5hqmjcdj8yo2ibzi9b4ew3sn"
GLO30 = "https://copernicus-dem-30m.s3.amazonaws.com"
UA = {"User-Agent": "scenic-roads/0.1 (personal offline map)"}
# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): the buckets' listings and published files.
FETCHES = "api, data"


def get(url, rng=None):
    h = dict(UA)
    if rng:
        h["Range"] = "bytes=%d-%d" % rng
    with urllib.request.urlopen(urllib.request.Request(url, headers=h), timeout=120) as r:
        return r.read()


def square(x0, y0, x1, y1):
    return {"type": "Polygon", "coordinates": [[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]}


def feature(url, geom):
    return {"type": "Feature", "properties": {"url": url}, "geometry": geom}


def hrdem(a):
    have = set()
    if a.tiles:
        lines = [l.strip() for l in open(a.tiles)]
    else:
        lines = list(s3_keys(NRCAN, "hrdem-mosaic-2m/"))
    for line in lines:
        m = re.fullmatch(r"(?:.*/)?(\d+_\d+)-mosaic-2m-dtm\.tif", line)
        if m:
            have.add(m.group(1))
    src = json.load(open(a.squares))
    feats = []
    for f in src["features"]:
        tid = f["properties"]["id"]
        if tid not in have:
            continue
        ring = f["geometry"]["coordinates"][0]
        xs, ys = [p[0] for p in ring], [p[1] for p in ring]
        feats.append(feature(f"{NRCAN}/hrdem-mosaic-2m/{tid}-mosaic-2m-dtm.tif", square(min(xs), min(ys), max(xs), max(ys))))
    return {"type": "FeatureCollection", "crs": {"type": "name", "properties": {"name": "urn:ogc:def:crs:EPSG::3979"}}, "features": feats}


def s3_keys(base, prefix):
    token = None
    while True:
        q = {"list-type": "2", "prefix": prefix, "max-keys": "1000"}
        if token:
            q["continuation-token"] = token
        x = get(base + "/?" + urllib.parse.urlencode(q)).decode()
        yield from re.findall(r"<Key>([^<]*)</Key>", x)
        m = re.search(r"<NextContinuationToken>([^<]*)</NextContinuationToken>", x)
        if not m:
            return
        token = m.group(1)


def usgs(a):
    feats = []
    for k in sorted(s3_keys(USGS, USGS_PREFIX)):
        m = re.fullmatch(re.escape(USGS_PREFIX) + r"([ns])(\d\d)([ew])(\d\d\d)/USGS_13_\1\2\3\4\.tif", k)
        if not m:
            continue
        n = int(m.group(2)) * (1 if m.group(1) == "n" else -1)
        w = int(m.group(4)) * (-1 if m.group(3) == "w" else 1)
        feats.append(feature(f"{USGS}/{k}", square(w, n - 1, w + 1, n)))
    return {"type": "FeatureCollection", "features": feats}


def zip_members(url):
    head = urllib.request.urlopen(urllib.request.Request(url, method="HEAD", headers=UA), timeout=120)
    size = int(head.headers["Content-Length"])
    tail = get(url, (max(0, size - 65558), size - 1))
    i = tail.rfind(b"PK\x05\x06")
    if i < 0:
        raise SystemExit(f"{url}: no end of central directory")
    n, cd_size, cd_off = struct.unpack("<HII", tail[i + 10:i + 20])
    if cd_off == 0xFFFFFFFF or n == 0xFFFF:
        j = tail.rfind(b"PK\x06\x06")
        n, cd_size, cd_off = struct.unpack("<QQQ", tail[j + 32:j + 56])
    cd = get(url, (cd_off, cd_off + cd_size - 1))
    p, out = 0, []
    while p + 46 <= len(cd) and cd[p:p + 4] == b"PK\x01\x02":
        ln, le, lc = struct.unpack("<HHH", cd[p + 28:p + 34])
        out.append(cd[p + 46:p + 46 + ln].decode())
        p += 46 + ln + le + lc
    return out


def fabdem(a):
    page = get(BRISTOL + "/").decode()
    zips = sorted(set(re.findall(r'href="([^"]*_FABDEM_V1-2\.zip)"', page)))
    feats = []
    for i, z in enumerate(zips):
        url = urllib.parse.urljoin(BRISTOL + "/", z)
        for mem in zip_members(url):
            m = re.fullmatch(r"([NS])(\d\d)([EW])(\d\d\d)_FABDEM_V1-2\.tif", mem.rsplit("/", 1)[-1])
            if not m:
                continue
            s = int(m.group(2)) * (1 if m.group(1) == "N" else -1)
            w = int(m.group(4)) * (1 if m.group(3) == "E" else -1)
            feats.append(feature(f"{url}#{mem}", square(w, s, w + 1, s + 1)))
        print(f"{i + 1}/{len(zips)} zips, {len(feats)} tiles", file=sys.stderr)
    feats.sort(key=lambda f: f["properties"]["url"])
    return {"type": "FeatureCollection", "features": feats}


def glo30(a):
    text = open(a.tiles).read() if a.tiles else get(GLO30 + "/tileList.txt").decode()
    feats = []
    for line in text.split():
        m = re.fullmatch(r"(Copernicus_DSM_COG_10_([NS])(\d\d)_00_([EW])(\d\d\d)_00)_DEM", line)
        if not m:
            continue
        stem = m.group(1)
        s = int(m.group(3)) * (1 if m.group(2) == "N" else -1)
        w = int(m.group(5)) * (1 if m.group(4) == "E" else -1)
        f = feature(f"{GLO30}/{stem}_DEM/{stem}_DEM.tif", square(w, s, w + 1, s + 1))
        f["properties"]["mask"] = f"{GLO30}/{stem}_DEM/AUXFILES/{stem}_FLM.tif"
        feats.append(f)
    feats.sort(key=lambda f: f["properties"]["url"])
    return {"type": "FeatureCollection", "features": feats}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("source", choices=["hrdem", "3dep", "fabdem", "glo30"])
    ap.add_argument("--tiles")
    ap.add_argument("--squares")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    doc = {"hrdem": hrdem, "3dep": usgs, "fabdem": fabdem, "glo30": glo30}[a.source](a)
    with open(a.out, "w") as f:
        json.dump(doc, f, separators=(",", ":"))
        f.write("\n")
    print(f"{a.out}: {len(doc['features'])} files", file=sys.stderr)


if __name__ == "__main__":
    main()
