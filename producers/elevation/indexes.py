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
  glo30   Copernicus DEM GLO-30 (the worldwide dataset): every tile in the bucket's
          tileList.txt (or --tiles, a copy of it), its DEM by `url` and its fill mask (FLM, in its
          AUXFILES/) by `mask`, its footprint the 1° square it's named by (its south-west corner).
  rema    PGC's REMA v2.0 32 m mosaic (EPSG:3031, Antarctica): every tile of the bucket's
          rema/mosaics/v2.0/32m/ (listed), its DEM by `url`, its footprint its STAC item's
          `proj:bbox`; with --south-of <lat> only the tiles wholly south of that latitude (their
          corner farthest from the pole), so the dataset meets no Web Mercator tile (they end at
          85.05° S) and its coverage changes no existing key (docs/polar.md §2).

Drop the index beside its declaration in inputs/elevation/; the input check reads it.

usage: indexes.py hrdem [--res 1|2] [--tiles <listing.txt>] --squares <index.geojson> --out <file>
       indexes.py 3dep --out <file>
       indexes.py 3dep-1m --bbox <w,s,e,n> --out <file with {zone}>
       indexes.py glo30 [--tiles <tileList.txt>] --out <file>
       indexes.py rema [--south-of <lat>] --out <file>
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
GLO30 = "https://copernicus-dem-30m.s3.amazonaws.com"
PGC = "https://pgc-opendata-dems.s3.us-west-2.amazonaws.com"
REMA_PREFIX = "rema/mosaics/v2.0/32m/"
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


def rema_lat(x, y):
    """The latitude of EPSG:3031's (x, y): polar stereographic variant B on WGS 84, standard
    parallel 71° S (Snyder 21-33, 21-34; roadcore::polar::inverse)."""
    import math
    a, f = 6378137.0, 1 / 298.257223563
    e = math.sqrt(f * (2 - f))
    pc = math.radians(71.0)
    tc = math.tan(math.pi / 4 - pc / 2) / ((1 - e * math.sin(pc)) / (1 + e * math.sin(pc))) ** (e / 2)
    mc = math.cos(pc) / math.sqrt(1 - (e * math.sin(pc)) ** 2)
    t = math.hypot(x, y) * tc / (a * mc)
    phi = math.pi / 2 - 2 * math.atan(t)
    for _ in range(20):
        es = e * math.sin(phi)
        phi = math.pi / 2 - 2 * math.atan(t * ((1 - es) / (1 + es)) ** (e / 2))
    return -math.degrees(phi)


def rema(a):
    tiles = sorted({m.group(1) for k in s3_keys(PGC, REMA_PREFIX) for m in [re.fullmatch(re.escape(REMA_PREFIX) + r"(\d+_\d+)\.json", k)] if m})
    feats = []
    for t in tiles:
        item = json.loads(get(f"{PGC}/{REMA_PREFIX}{t}/{t}_32m_v2.0.json"))
        x0, y0, x1, y1 = item["properties"]["proj:bbox"]
        if a.south_of is not None:
            far = max((abs(x0), abs(x1))), max((abs(y0), abs(y1)))
            if not rema_lat(*far) < a.south_of:
                continue
        feats.append(feature(item["assets"]["dem"]["href"], square(x0, y0, x1, y1)))
    return {"type": "FeatureCollection", "crs": {"type": "name", "properties": {"name": "urn:ogc:def:crs:EPSG::3031"}}, "features": feats}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("source", choices=["hrdem", "3dep", "3dep-1m", "glo30", "rema"])
    ap.add_argument("--tiles")
    ap.add_argument("--squares")
    ap.add_argument("--res", type=int, choices=[1, 2], default=2)
    ap.add_argument("--bbox")
    ap.add_argument("--south-of", type=float)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    if a.source == "3dep-1m":
        if not a.bbox or "{zone}" not in a.out:
            ap.error("3dep-1m needs --bbox and an --out naming {zone}")
        docs = {a.out.format(zone=c - 26900): d for c, d in usgs_1m(a).items()}
    else:
        docs = {a.out: {"hrdem": hrdem, "3dep": usgs, "glo30": glo30, "rema": rema}[a.source](a)}
    for out, doc in docs.items():
        with open(out, "w") as f:
            json.dump(doc, f, separators=(",", ":"))
            f.write("\n")
        print(f"{out}: {len(doc['features'])} files", file=sys.stderr)


if __name__ == "__main__":
    main()
