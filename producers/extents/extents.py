#!/usr/bin/env python3
"""The source extents' pieces (docs/inputs.md §6.11): for each declaration given, the GeoJSON of
its pieces, read as its `[[piece]]` blocks say, written beside a copy of it in --out/<theme>/.

A reader per kind of service, none per source:
  arcgis   an ArcGIS REST layer's features (`url`: the layer; `where`; paged by `page`)
  stac     a STAC API collection's items (`url`: the collection; paged by its `next` links)
  wfs      a WFS feature type (`url`, `type`; `crs` where the answer doesn't name its own)
  file     an index file fetched whole (`url`; GeoJSON, or a zipped shapefile)
  listing  a listing of georeferenced names: `url` an S3 bucket (`prefix`), a page of links or a
           text file (`list` = "s3", "links", "lines"); `pattern`, a regular expression with the
           groups `x` and `y` (and `ew`, `ns`: W and S negate); `unit`, the CRS units of one of the
           name's; `size`, a tile's side(s) in CRS units; `corner`, the corner named ("sw", "nw");
           `crs`
  raster   each file's own header and an overview, by range request (`urls`, or `stac` and `asset`:
           a STAC collection's items' files): its bounds and where it has data, at the coarsest
           overview, or with `mask_m` at the coarsest whose pixels are no wider than that
  none     no extent (`why`): the piece is listed, with a null geometry and the class `unknown`

Every request goes through one fetcher: the app's User-Agent, one at a time with a pause, and no
more than --max-gb (5) read for one source, its answers together (the run stops, naming it):
headers, indexes and overviews, never a source's data but a small one's.

usage: extents.py <declaration.toml>... --out <folder> [--cache <folder>] [--max-gb 5] [--pause 0.3]
"""
import argparse
import datetime
import hashlib
import io
import json
import math
import os
import re
import shutil
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

import pyproj
import shapely
import shapely.affinity
import shapely.geometry as sg

UA = "scenic-roads/0.1 (personal offline map)"
# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): documented services and published files.
FETCHES = "api, data"
CLASSES = ("mask", "files", "index", "service", "claimed", "unknown")
AVAILABILITY = ("open", "login", "paid", "closed")
# The class a method's extents are; None: the declaration says (`class`).
METHOD_CLASS = {"arcgis": None, "stac": "index", "wfs": None, "file": None, "listing": "index", "raster": "files", "none": "unknown"}
M_PER_DEG = 111_320.0


class Fetcher:
    """Every request of a run: counted, paced, bounded, and kept (--cache) so a run made again asks
    for nothing it has."""

    def __init__(self, cache, max_bytes, pause):
        self.cache, self.max_bytes, self.pause = cache, max_bytes, pause
        # What a source's reads have come to so far (`source()` starts a source): its limit is
        # --max-gb, its cached answers counted as fetched ones.
        self.spent = 0
        self.requests = self.bytes = 0
        self.lengths = {}
        self.last = 0.0
        # The newest date an answer read since `since()` was fetched on (a cached one's: its file's).
        self.fetched = None

    def since(self):
        self.fetched = None

    def source(self):
        self.spent = 0

    def left(self):
        return self.max_bytes - self.spent

    def mark(self):
        return self.requests, self.bytes

    def get(self, url, rng=None):
        key = hashlib.sha1((url + (f"#{rng[0]}-{rng[1]}" if rng else "")).encode()).hexdigest()
        path = os.path.join(self.cache, key) if self.cache else None
        if path and os.path.exists(path):
            with open(path, "rb") as f:
                body = f.read()
            self.requests += 1
            self.bytes += len(body)
            self.spend(url, len(body))
            self.saw(datetime.date.fromtimestamp(os.path.getmtime(path)).isoformat())
            return body
        headers = {"User-Agent": UA}
        if rng:
            headers["Range"] = f"bytes={rng[0]}-{rng[1]}"
        for attempt in range(4):
            wait = self.last + self.pause - time.monotonic()
            if wait > 0:
                time.sleep(wait)
            try:
                with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=180) as r:
                    n = r.headers.get("Content-Length")
                    if n and int(n) > self.left():
                        sys.exit(f"{url}: {int(n) / 1e6:.0f} MB, over what --max-gb leaves this source: not fetched")
                    # (In blocks: read(n) would set aside n bytes first.)
                    parts, got = [], 0
                    while got <= self.left():
                        part = r.read(1 << 20)
                        if not part:
                            break
                        parts.append(part)
                        got += len(part)
                    body = b"".join(parts)
                    total = (r.headers.get("Content-Range") or "").rpartition("/")[2]
                    if len(body) > self.left():
                        sys.exit(f"{url}: over what --max-gb leaves this source: stopped")
                break
            except urllib.error.HTTPError as e:
                if e.code not in (429, 500, 502, 503, 504) or attempt == 3:
                    raise
                time.sleep(5 * (attempt + 1))
            except (urllib.error.URLError, TimeoutError):
                if attempt == 3:
                    raise
                time.sleep(5 * (attempt + 1))
            finally:
                self.last = time.monotonic()
        self.requests += 1
        self.bytes += len(body)
        self.spend(url, len(body))
        self.saw(datetime.date.today().isoformat())
        if rng and total.isdigit():
            self.lengths[url] = int(total)
        if path:
            if rng and total.isdigit():
                with open(os.path.join(self.cache, hashlib.sha1(url.encode()).hexdigest() + ".len"), "w") as f:
                    f.write(total)
            with open(path + ".part", "wb") as f:
                f.write(body)
            os.replace(path + ".part", path)
        return body

    def spend(self, url, n):
        self.spent += n
        if self.spent > self.max_bytes:
            sys.exit(f"{url}: this source has read {self.spent / 1e9:.2f} GB, over --max-gb: stopped")

    def saw(self, day):
        self.fetched = max(self.fetched or day, day)

    def length(self, url):
        """A file's whole length, as a ranged answer for it said."""
        if url not in self.lengths and self.cache:
            with open(os.path.join(self.cache, hashlib.sha1(url.encode()).hexdigest() + ".len")) as f:
                self.lengths[url] = int(f.read())
        return self.lengths[url]

    def json(self, url):
        return json.loads(self.get(url))


class RangeFile:
    """A remote file read a block at a time through the fetcher, for rasterio's `opener`."""

    BLOCK = 1 << 16

    def __init__(self, net, url):
        # rasterio hands the address on as a path: one slash after the scheme, and a probe of its
        # own first ("test/test"), to be refused as a missing file.
        url = re.sub(r"^(https?):/+", r"\1://", str(url))
        if not url.startswith("http"):
            raise FileNotFoundError(url)
        self.net, self.url, self.pos, self.blocks = net, url, 0, {}
        self._block(0)
        self.size = net.length(url)

    def _block(self, k):
        if k not in self.blocks:
            self.blocks[k] = self.net.get(self.url, (k * self.BLOCK, (k + 1) * self.BLOCK - 1))
        return self.blocks[k]

    def read(self, n=-1):
        out = b""
        while (n < 0 or len(out) < n) and self.pos < self.size:
            b = self._block(self.pos // self.BLOCK)[self.pos % self.BLOCK:]
            b = b if n < 0 else b[:n - len(out)]
            out += b
            self.pos += len(b)
        return out

    def seek(self, off, whence=0):
        self.pos = off + (0, self.pos, self.size)[whence]
        return self.pos

    def tell(self):
        return self.pos

    def close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *_):
        pass


# ---- readers: each yields (geometry in `crs`, properties), and says its CRS ----

def read_arcgis(net, b, tol_deg):
    """An ArcGIS REST layer, as GeoJSON in WGS 84, generalised by the service."""
    layer = net.json(b["url"].rstrip("/") + "?f=json")
    oid = layer.get("objectIdField") or "OBJECTID"
    fields = sorted(wanted_fields(b)) or [oid]
    page, off = int(b.get("page", 500)), 0
    while True:
        # (Ordered by the object id, so the pages hold the same features in the same order each run.)
        q = {"where": b.get("where", "1=1"), "outFields": ",".join(fields), "outSR": "4326", "f": "geojson",
             "returnGeometry": "true", "geometryPrecision": "6", "maxAllowableOffset": f"{tol_deg / 4:.7f}",
             "orderByFields": oid, "resultOffset": str(off), "resultRecordCount": str(page)}
        d = net.json(b["url"].rstrip("/") + "/query?" + urllib.parse.urlencode(q))
        if "error" in d:
            sys.exit(f"{b['url']}: {d['error']}")
        feats = d.get("features") or []
        for f in feats:
            if f.get("geometry"):
                yield sg.shape(f["geometry"]), f.get("properties") or {}
        off += len(feats)
        more = d.get("exceededTransferLimit") or (d.get("properties") or {}).get("exceededTransferLimit")
        if not feats or (len(feats) < page and not more):
            return


def read_stac(net, b, _tol):
    url = b["url"].rstrip("/") + "/items?limit=" + str(b.get("page", 500))
    while url:
        d = net.json(url)
        for f in d.get("features") or []:
            if f.get("geometry"):
                yield sg.shape(f["geometry"]), {**(f.get("properties") or {}), "id": f.get("id"), "collection": f.get("collection")}
        url = next((l["href"] for l in d.get("links") or [] if l.get("rel") == "next"), None)


def read_wfs(net, b, _tol):
    page, off = int(b.get("page", 2000)), 0
    while True:
        q = {"service": "WFS", "version": "2.0.0", "request": "GetFeature", "typeNames": b["type"],
             "outputFormat": b.get("format", "application/json"), "count": str(page), "startIndex": str(off)}
        if b.get("fields"):
            q["propertyName"] = ",".join(b["fields"])
        d = net.json(b["url"] + ("&" if "?" in b["url"] else "?") + urllib.parse.urlencode(q))
        if answer_crs(d) and "crs" not in b:
            b["crs"] = answer_crs(d)
        feats = d.get("features") or []
        for f in feats:
            if f.get("geometry"):
                yield xy(sg.shape(f["geometry"]), b), f.get("properties") or {}
        off += len(feats)
        if len(feats) < page:
            return


def answer_crs(d):
    """The CRS a GeoJSON answer names (its legacy `crs` member), if any."""
    return ((d.get("crs") or {}).get("properties") or {}).get("name")


def xy(g, b):
    """A geometry east first, as the rest reads it: swapped where the declaration says its
    answers are latitude first (`latfirst`). Not guessed from the CRS an answer names: data.geopf.fr
    names urn:ogc:def:crs:EPSG::4326 and answers longitude first."""
    return shapely.transform(g, lambda c: c[:, ::-1]) if b.get("latfirst") else g


def read_file(net, b, _tol):
    body = net.get(b["url"])
    if body[:2] == b"PK":
        import shapefile
        z = zipfile.ZipFile(io.BytesIO(body))
        shp = next(n for n in z.namelist() if n.lower().endswith(".shp") and (not b.get("member") or b["member"] in n))
        stem = shp[:-4]
        opened = {ext: io.BytesIO(z.read(next(n for n in z.namelist() if n.lower() == (stem + "." + ext).lower()))) for ext in ("shp", "shx", "dbf")}
        prj = next((n for n in z.namelist() if n.lower() == (stem + ".prj").lower()), None)
        if prj and "crs" not in b:
            b["crs"] = z.read(prj).decode("utf-8", "replace")
        r = shapefile.Reader(**opened)
        names = [f[0] for f in r.fields[1:]]
        for sr in r.iterShapeRecords():
            if sr.shape.shapeType:
                yield sg.shape(sr.shape.__geo_interface__), dict(zip(names, sr.record))
        return
    d = json.loads(body)
    if answer_crs(d) and "crs" not in b:
        b["crs"] = answer_crs(d)
    for f in d["features"]:
        if f.get("geometry"):
            yield xy(sg.shape(f["geometry"]), b), f.get("properties") or {}


def listed_names(net, b):
    kind = b.get("list", "s3")
    if kind == "s3":
        token = None
        while True:
            q = {"list-type": "2", "prefix": b.get("prefix", "")}
            if b.get("delimiter"):
                q["delimiter"] = b["delimiter"]
            if token:
                q["continuation-token"] = token
            x = ET.fromstring(net.get(b["url"].rstrip("/") + "/?" + urllib.parse.urlencode(q)))
            ns = x.tag[:x.tag.index("}") + 1] if x.tag.startswith("{") else ""
            for e in x.iter(ns + "Key"):
                yield e.text
            for e in x.iter(ns + "Prefix"):
                if e.text != b.get("prefix", ""):
                    yield e.text
            token = x.findtext(ns + "NextContinuationToken")
            if not token:
                return
    elif kind == "links":
        yield from re.findall(r'href="([^"]+)"', net.get(b["url"]).decode("utf-8", "replace"))
    elif kind == "lines":
        yield from net.get(b["url"]).decode("utf-8", "replace").split()
    else:
        sys.exit(f"unknown list {kind!r}")


def read_listing(net, b, _tol):
    pat = re.compile(b["pattern"])
    unit = float(b.get("unit", 1))
    size = b["size"] if isinstance(b["size"], list) else [b["size"], b["size"]]
    corner = b.get("corner", "sw")
    geographic = pyproj.CRS.from_user_input(b.get("crs", "OGC:CRS84")).is_geographic
    b["grain_m"] = round(max(size) * (M_PER_DEG if geographic else 1), 1)
    seen = set()
    for name in listed_names(net, b):
        m = pat.search(name)
        if not m:
            continue
        g = m.groupdict()
        x, y = float(g["x"]) * unit, float(g["y"]) * unit
        if g.get("ew") in ("W", "w"):
            x = -x
        if g.get("ns") in ("S", "s"):
            y = -y
        if corner == "nw":
            y -= size[1]
        if (x, y) in seen:
            continue
        seen.add((x, y))
        yield sg.box(x, y, x + size[0], y + size[1]), {k: v for k, v in g.items() if k not in ("x", "y", "ew", "ns")}


def raster_files(net, b):
    """A raster piece's files: its `urls`, or a STAC collection's (`stac`) items' asset (`asset`)."""
    if "stac" not in b:
        return b["urls"]
    url, out = b["stac"].rstrip("/") + "/items?limit=500", []
    while url:
        d = net.json(url)
        out += [f["assets"][b["asset"]]["href"] for f in d.get("features") or [] if b["asset"] in f.get("assets", {})]
        url = next((l["href"] for l in d.get("links") or [] if l.get("rel") == "next"), None)
    return sorted(out)


def read_raster(net, b, _tol):
    """Each file's bounds and valid-data mask at its coarsest overview. A file's CRS goes with its
    shape (they may differ): yielded already in WGS 84."""
    import numpy as np
    import rasterio
    import rasterio.features
    import rasterio.warp
    b["crs"] = "OGC:CRS84"
    with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_PAM_ENABLED="NO"):
        for url in raster_files(net, b):
            with rasterio.open(url, opener=lambda p, mode="rb", net=net: RangeFile(net, p)) as ds:
                px = max(abs(ds.transform.a), abs(ds.transform.e)) * (M_PER_DEG if ds.crs.is_geographic else 1)
                levels = ds.overviews(1) or [1]
                fine = [f for f in levels if f * px <= float(b.get("mask_m", 0))]
                # (No overview as fine as `mask_m`: the finest there is, never the full resolution,
                # which would be a whole download; the grain recorded says how fine it was.)
                f = fine[-1] if fine else levels[0 if "mask_m" in b else -1]
                h, w = max(1, math.ceil(ds.height / f)), max(1, math.ceil(ds.width / f))
                mask = ds.dataset_mask(out_shape=(h, w))
                tr = ds.transform * ds.transform.scale(ds.width / w, ds.height / h)
                shapes = [sg.shape(s) for s, v in rasterio.features.shapes((mask > 0).astype(np.uint8), mask=mask > 0, transform=tr) if v]
                geom = shapely.union_all(shapes)
                res = max(abs(ds.transform.a), abs(ds.transform.e))
                if ds.crs.is_geographic:
                    mid = math.radians((ds.bounds.top + ds.bounds.bottom) / 2)
                    res = max(abs(ds.transform.e) * M_PER_DEG, abs(ds.transform.a) * M_PER_DEG * math.cos(mid))
                b["grain_m"] = max(b.get("grain_m", 0), round(f * px, 1))
                yield to_wgs84(geom, ds.crs.to_wkt(), abs(tr.a)), {"res_m": round(res, 4)}


READERS = {"arcgis": read_arcgis, "stac": read_stac, "wfs": read_wfs, "file": read_file, "listing": read_listing, "raster": read_raster}


# ---- from features to pieces ----

def wanted_fields(b):
    out = set(b.get("by", []))
    if b.get("name"):
        out.add(b["name"])
    for m in (b.get("metrics") or {}).values():
        if isinstance(m, dict):
            out.add(m["field"])
    return out


def metric(spec, props):
    """(value or None, the publisher's class or None) of a metric's declaration on a feature."""
    if not isinstance(spec, dict):
        return spec, None
    v = props.get(spec["field"])
    if "index" in spec and isinstance(v, list):
        v = v[spec["index"]]
    if v is None or v == "":
        return None, None
    if "map" in spec:
        return spec["map"].get(str(v)), str(v)
    if "date" in spec:
        how = spec["date"]
        if how == "epoch-ms":
            return datetime.datetime.fromtimestamp(float(v) / 1000, datetime.UTC).year, None
        # A year alone or opening a compact date (20230930).
        years = [int(y) for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?:[01]\d[0-3]\d)?(?!\d)", str(v))]
        if not years:
            return None, None
        return (years[0] if how == "iso" else max(years)), None
    if "text" in spec:
        # A number written with its unit ("2 metre"): the unit must be the one declared.
        m = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*(\w+)\s*", str(v))
        if not m or not m.group(2).lower().startswith(spec["text"].lower()):
            return None, str(v)
        v = m.group(1)
    try:
        v = float(v)
    except (TypeError, ValueError):
        return None, None
    if not math.isfinite(v) or v <= 0:
        return None, None
    if spec.get("density"):
        v = 1 / math.sqrt(v)
    v *= float(spec.get("scale", 1))
    return round(v, 4), None


def unwrapped(xs):
    """Longitudes of a ring made continuous, and its turns round the axis (0, or ±1 round a pole)."""
    out, add = [xs[0]], 0.0
    for a, b in zip(xs, xs[1:]):
        d = b - a
        if d > 180:
            add -= 360
        elif d < -180:
            add += 360
        out.append(b + add)
    return out, round((out[-1] - out[0]) / 360)


def ring_region(lons, lats):
    """A ring's region in the longitude-latitude plane, within -180..180: cut at the antimeridian,
    and closed along 90° where it goes round a pole."""
    xs, turns = unwrapped(lons)
    pts = list(zip(xs, lats))
    if turns:
        pole = 90.0 if sum(lats) > 0 else -90.0
        pts = pts[:-1] + [(xs[-1], lats[-1]), (xs[-1], pole), (xs[0], pole), (xs[0], lats[0])]
    poly = shapely.make_valid(sg.Polygon(pts))
    parts = []
    for k in (-2, -1, 0, 1, 2):
        cut = poly.intersection(sg.box(-180 + 360 * k, -90, 180 + 360 * k, 90))
        if not cut.is_empty:
            parts.append(shapely.affinity.translate(cut, xoff=-360 * k))
    return polygons(shapely.union_all(parts))


def polygons(g):
    """A geometry's polygons as one (Multi)Polygon: lines and points a repair or a cut left dropped."""
    if g.geom_type in ("Polygon", "MultiPolygon"):
        return g
    return shapely.union_all([p for p in getattr(g, "geoms", []) if p.geom_type in ("Polygon", "MultiPolygon")])


def to_wgs84(geom, crs, step):
    """A geometry of `crs` in longitude and latitude (-180..180), its edges densified to `step`
    (the CRS's units) first so a straight edge there stays where it was."""
    c = pyproj.CRS.from_user_input(crs)
    geographic = c.is_geographic
    tr = None if c.equals(pyproj.CRS.from_epsg(4326)) or c.equals(pyproj.CRS.from_user_input("OGC:CRS84")) else pyproj.Transformer.from_crs(c, "OGC:CRS84", always_xy=True)
    if tr is None:
        b = geom.bounds
        if b[0] >= -180 and b[2] <= 180:
            return geom
    out = []
    for poly in getattr(geom, "geoms", [geom]):
        if poly.geom_type != "Polygon" or poly.is_empty:
            continue
        if not geographic:
            poly = shapely.segmentize(poly, step)
        regions = []
        for ring in [poly.exterior, *poly.interiors]:
            xs, ys = ring.coords.xy
            if tr:
                xs, ys = tr.transform(xs, ys)
            regions.append(ring_region(list(xs), list(ys)))
        shell = regions[0]
        for hole in regions[1:]:
            shell = shell.difference(hole)
        out.append(shell)
    return polygons(shapely.union_all(out))



def tidy(geom, tol_deg):
    """Dissolved features as a piece's outline: hairline gaps between neighbours closed, simplified
    with the topology kept, holes and specks under the tolerance squared dropped."""
    g = polygons(shapely.make_valid(geom))
    g = g.buffer(tol_deg / 4, join_style="mitre").buffer(-tol_deg / 4, join_style="mitre")
    g = polygons(shapely.make_valid(g.simplify(tol_deg, preserve_topology=True)))
    least = tol_deg * tol_deg
    kept = []
    for p in getattr(g, "geoms", [g]):
        if p.is_empty or p.area < least:
            continue
        kept.append(sg.Polygon(p.exterior, [h for h in p.interiors if sg.Polygon(h).area >= least]))
    g = shapely.set_precision(sg.MultiPolygon(kept), 1e-5)
    g = polygons(shapely.make_valid(g))
    if g.is_empty:
        return None
    # RFC 7946's winding: exteriors counter-clockwise, holes clockwise.
    g = shapely.orient_polygons(g, exterior_cw=False)
    return g if g.geom_type == "MultiPolygon" else sg.MultiPolygon([g])


def block_pieces(net, decl, b, today):
    """A piece block's features and what was read for them."""
    method = b["method"]
    cls = METHOD_CLASS[method] or b.get("class")
    if cls not in CLASSES or (METHOD_CLASS[method] and b.get("class") not in (None, cls, "claimed")):
        sys.exit(f"{decl['id']}/{b['id']}: class {b.get('class')!r} isn't one a {method} piece can be")
    avail = b.get("availability", decl.get("availability"))
    if avail not in AVAILABILITY:
        sys.exit(f"{decl['id']}/{b['id']}: availability {avail!r}")
    if method != "none" and (b.get("class") == "claimed" or avail != "open"):
        cls = "claimed"
    specs = b.get("metrics") or {}
    base = {"extent": cls, "availability": avail}
    if method == "none":
        vals = {k: metric(s, {})[0] for k, s in specs.items()}
        props = {"piece": b["id"], **base, **{k: v for k, v in vals.items() if v is not None}}
        if b.get("label"):
            props["name"] = b["label"]
        yield {"type": "Feature", "properties": props, "geometry": None}, {"piece": b["id"], "method": "none", "why": b["why"], "date": today}
        return
    if b.get("name") and b["name"] not in b.get("by", []):
        sys.exit(f"{decl['id']}/{b['id']}: name {b['name']!r} isn't one of `by`: a piece's features would name it apart")
    tol_m = float(b.get("tolerance_m", 50))
    tol_deg = tol_m / M_PER_DEG
    r0, b0 = net.mark()
    net.since()
    groups, n, flt = {}, 0, b.get("filter") or {}
    for geom, props in READERS[method](net, b, tol_deg):
        if any(str(props.get(k)) not in [str(x) for x in want] for k, want in flt.items()):
            continue
        n += 1
        by = tuple(str(props.get(k)) for k in b.get("by", []))
        ms = {k: metric(s, props) for k, s in specs.items()}
        varying = tuple((k, ms[k][0]) for k in sorted(specs) if isinstance(specs[k], dict))
        g = groups.setdefault((by, varying), {"geoms": [], "ms": ms, "name": props.get(b["name"]) if b.get("name") else b.get("label")})
        g["geoms"].append(geom)
    crs = b.get("crs", "OGC:CRS84")
    read = {"piece": b["id"], "method": method, "url": b.get("url") or b.get("stac") or b["urls"][0], "features": n,
            "date": net.fetched or today, "tolerance_m": tol_m}
    if "grain_m" in b:
        read["grain_m"] = b["grain_m"]
    repaired = 0
    feats = []
    shared = {}
    for by, _ in groups:
        shared[by] = shared.get(by, 0) + 1
    for (by, varying), g in sorted(groups.items(), key=lambda kv: str(kv[0])):
        bad = [x for x in g["geoms"] if not x.is_valid]
        repaired += len(bad)
        native = shapely.union_all([polygons(shapely.make_valid(x)) for x in g["geoms"]])
        c = pyproj.CRS.from_user_input(crs)
        step = tol_deg if c.is_geographic else tol_m * 20
        geom = tidy(to_wgs84(native, crs, step), tol_deg)
        if geom is None:
            continue
        pid = "/".join([b["id"], *by])
        if shared[by] > 1:
            pid = "/".join([pid, *(f"{k}={v}" for k, v in varying if v is not None)])
        props = {"piece": pid}
        if g["name"]:
            props["name"] = str(g["name"])
        props.update(base)
        stated = {}
        for k, (v, cl) in g["ms"].items():
            if v is not None:
                props[k] = v
            if cl is not None:
                stated[k] = cl
        if stated:
            props["stated"] = stated
        feats.append({"type": "Feature", "properties": props, "geometry": sg.mapping(geom)})
    r1, b1 = net.mark()
    read.update(requests=r1 - r0, bytes=b1 - b0)
    if repaired:
        read["repaired"] = repaired
    ids = [f["properties"]["piece"] for f in feats]
    if len(set(ids)) != len(ids):
        sys.exit(f"{decl['id']}/{b['id']}: two pieces of one id: name them apart with `by`")
    yield from ((f, None) for f in feats)
    yield None, read


def rounded(o):
    """A geometry's mapping with coordinates to 5 decimals, as lists."""
    if isinstance(o, (list, tuple)):
        return [rounded(x) for x in o]
    if isinstance(o, float):
        return round(o, 5)
    if isinstance(o, dict):
        return {k: rounded(v) for k, v in o.items()}
    return o


def produce(net, path, out, today):
    with open(path, "rb") as f:
        decl = tomllib.load(f)
    decl["id"] = os.path.basename(path)[:-len(".toml")]
    for k in ("name", "credit", "licence", "theme"):
        if not decl.get(k):
            sys.exit(f"{path}: no {k}")
    feats, reads = [], []
    net.source()
    for b in decl.get("piece") or []:
        for f, read in block_pieces(net, decl, dict(b), today):
            if f:
                f["geometry"] = rounded(f["geometry"])
                feats.append(f)
            if read:
                reads.append(read)
    folder = os.path.join(out, decl["theme"])
    os.makedirs(folder, exist_ok=True)
    shutil.copyfile(path, os.path.join(folder, decl["id"] + ".toml"))
    target = os.path.join(folder, decl["id"] + ".geojson")
    with open(target, "w") as f:
        json.dump({"type": "FeatureCollection", "source": decl["id"], "theme": decl["theme"], "read": reads, "features": feats},
                  f, ensure_ascii=False, separators=(",", ":"))
        f.write("\n")
    by_class = {}
    for x in feats:
        by_class[x["properties"]["extent"]] = by_class.get(x["properties"]["extent"], 0) + 1
    print(f"{decl['id']}: {len(feats)} pieces ({', '.join(f'{n} {c}' for c, n in sorted(by_class.items()))}), "
          f"{os.path.getsize(target) / 1e3:.0f} KB; read {sum(r.get('requests', 0) for r in reads)} requests, "
          f"{sum(r.get('bytes', 0) for r in reads) / 1e6:.2f} MB, {sum(r.get('features', 0) for r in reads)} features", flush=True)


def main():
    ap = argparse.ArgumentParser(usage=__doc__)
    ap.add_argument("declarations", nargs="+")
    ap.add_argument("--out", required=True)
    ap.add_argument("--cache")
    ap.add_argument("--max-gb", type=float, default=5)
    ap.add_argument("--pause", type=float, default=0.3)
    a = ap.parse_args()
    if a.cache:
        os.makedirs(a.cache, exist_ok=True)
    net = Fetcher(a.cache, int(a.max_gb * 1e9), a.pause)
    today = datetime.date.today().isoformat()
    for path in a.declarations:
        produce(net, path, a.out, today)


if __name__ == "__main__":
    main()
