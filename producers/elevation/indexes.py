#!/usr/bin/env python3
"""The elevation datasets' tile indexes (docs/inputs.md §6.8, `[data] index`), from each source's own
listing: a GeoJSON FeatureCollection, a feature per file, its `url` and its footprint (a polygon in
the index's CRS), which the app reads as the file's box (min x <= x < max x, min y < y <= max y).

  hrdem   NRCan's HRDEM mosaic at --res metres (2, the default, or 1): the tiles the bucket has at
          that resolution (its hrdem-mosaic-<res>m/ keys, listed; or --tiles, a listing of them) and
          their 500 km squares from NRCan's tile index (--squares, EPSG:3979, as NRCan publishes it:
          hrdem-squares.geojson here).
  3dep    USGS 3DEP 1/3 arc-second: every USGS_13_<tile>.tif under the bucket's current/ folder
          (listed), its footprint the 1° square it's named by (its upper-left corner).
  3dep-1m USGS 3DEP 1 m: every tile The National Map's products API lists meeting --bbox (w,s,e,n,
          degrees), an index a UTM zone (--out names the files, `{zone}` in it: each zone's in
          NAD83 / UTM, EPSG:269<zone>); its footprint the 10 km square it's named by (x<e>y<n>: e,
          its west edge, and n, its north edge, in 10 km); its zone from its name
          (USGS_1M_<zone>_…), else from its GeoTIFF's header (the older USGS_one_meter_…, USGS_1m_…:
          their project names' zones aren't always theirs). Newest first (`publicationDate`), as
          several projects can cover a tile and a point takes the first file with data there.
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

usage: indexes.py hrdem [--res 1|2] [--tiles <listing.txt>] --squares <index.geojson> --out <file>
       indexes.py 3dep --out <file>
       indexes.py 3dep-1m --bbox <w,s,e,n> --out <file with {zone}>
       indexes.py fabdem --out <file>
       indexes.py glo30 [--tiles <tileList.txt>] --out <file>
"""
import argparse
import json
import re
import struct
import sys
import time
import urllib.error
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
        lines = list(s3_keys(NRCAN, f"hrdem-mosaic-{a.res}m/"))
    for line in lines:
        m = re.fullmatch(r"(?:.*/)?(\d+_\d+)-mosaic-%dm-dtm\.tif" % a.res, line)
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
        feats.append(feature(f"{NRCAN}/hrdem-mosaic-{a.res}m/{tid}-mosaic-{a.res}m-dtm.tif", square(min(xs), min(ys), max(xs), max(ys))))
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


TNM_PRODUCTS = "https://tnmaccess.nationalmap.gov/api/v1/products"
TNM_1M = "Digital Elevation Model (DEM) 1 meter"


def get_patiently(url, rng=None, tries=7):
    """`get`, retried with back-off (a 429's `Retry-After` honoured): the products API has its bad
    moments."""
    for k in range(tries):
        try:
            return get(url, rng)
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            http = e.code if isinstance(e, urllib.error.HTTPError) else None
            if k == tries - 1 or http not in (None, 429, 500, 502, 503, 504):
                raise
            wait = 5 * 2 ** k
            after = e.headers.get("Retry-After", "") if http else ""
            if after.isdigit():
                wait = max(wait, int(after))
            print(f"{url}: {e}; again in {wait} s", file=sys.stderr)
            time.sleep(wait)


def tnm_items(dataset, bbox):
    """Every GeoTIFF product of `dataset` the products API lists meeting `bbox` (w, s, e, n)."""
    out, off = [], 0
    while True:
        q = {"datasets": dataset, "bbox": ",".join(map(str, bbox)), "prodFormats": "GeoTIFF",
             "max": "1000", "offset": str(off), "outputFormat": "JSON"}
        d = json.loads(get_patiently(TNM_PRODUCTS + "?" + urllib.parse.urlencode(q)))
        got = d.get("items") or []
        out += got
        off += len(got)
        print(f"products: {off} of {d.get('total')}", file=sys.stderr)
        if not got or off >= d.get("total", 0):
            return out
        time.sleep(1)


def tiff_projected_crs(url):
    """The ProjectedCSType GeoKey of a GeoTIFF's first image (its header read by range requests: its
    first 16 KB, and whatever of its first IFD and GeoKeys lies past them)."""
    head = get_patiently(url, (0, 16383))

    def read(off, n):
        return head[off:off + n] if off + n <= len(head) else get_patiently(url, (off, off + n - 1))

    e = "<" if head[:2] == b"II" else ">"
    big = struct.unpack(e + "H", head[2:4])[0] == 43
    word, w = ("Q", 8) if big else ("I", 4)
    ifd = struct.unpack(e + word, head[8:16] if big else head[4:8])[0]
    count = struct.unpack(e + ("Q" if big else "H"), read(ifd, 8 if big else 2))[0]
    entry = 12 + 2 * (w - 4)
    entries = read(ifd + (8 if big else 2), count * entry)
    for i in range(count):
        q = entries[i * entry:(i + 1) * entry]
        if struct.unpack(e + "H", q[:2])[0] != 34735:
            continue
        n = struct.unpack(e + word, q[4:4 + w])[0]
        raw = q[4 + w:4 + 2 * w][:2 * n] if 2 * n <= w else read(struct.unpack(e + word, q[4 + w:4 + 2 * w])[0], 2 * n)
        keys = struct.unpack(e + "H" * n, raw)
        for k in range(4, len(keys), 4):
            if keys[k] == 3072:
                return keys[k + 3]
    raise SystemExit(f"{url}: no ProjectedCSType")


def usgs_1m(a):
    items = {i["downloadURL"]: i for i in tnm_items(TNM_1M, [float(v) for v in a.bbox.split(",")])}
    zones = {}
    for url, i in sorted(items.items(), key=lambda kv: (kv[1]["publicationDate"], kv[0]), reverse=True):
        name = url.rsplit("/", 1)[-1]
        m = re.search(r"_x(\d+)y(\d+)_", name)
        if not url.endswith(".tif") or not m:
            print(f"{url}: not a named tile, left out", file=sys.stderr)
            continue
        z = re.match(r"USGS_1M_(\d+)_", name)
        crs = 26900 + int(z.group(1)) if z else tiff_projected_crs(url)
        if not 26901 <= crs <= 26923:
            raise SystemExit(f"{url}: EPSG:{crs} isn't a NAD83 / UTM zone")
        x, y = int(m.group(1)), int(m.group(2))
        zones.setdefault(crs, []).append(feature(url, square(x * 10000, (y - 1) * 10000, (x + 1) * 10000, y * 10000)))
    return {
        c: {"type": "FeatureCollection", "crs": {"type": "name", "properties": {"name": f"urn:ogc:def:crs:EPSG::{c}"}},
            "features": f}
        for c, f in sorted(zones.items())
    }


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
    ap.add_argument("source", choices=["hrdem", "3dep", "3dep-1m", "fabdem", "glo30"])
    ap.add_argument("--tiles")
    ap.add_argument("--squares")
    ap.add_argument("--res", type=int, choices=[1, 2], default=2)
    ap.add_argument("--bbox")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    if a.source == "3dep-1m":
        if not a.bbox or "{zone}" not in a.out:
            ap.error("3dep-1m needs --bbox and an --out naming {zone}")
        docs = {a.out.format(zone=c - 26900): d for c, d in usgs_1m(a).items()}
    else:
        docs = {a.out: {"hrdem": hrdem, "3dep": usgs, "fabdem": fabdem, "glo30": glo30}[a.source](a)}
    for out, doc in docs.items():
        with open(out, "w") as f:
            json.dump(doc, f, separators=(",", ":"))
            f.write("\n")
        print(f"{out}: {len(doc['features'])} files", file=sys.stderr)


if __name__ == "__main__":
    main()
