#!/usr/bin/env python3
"""The source extents' pieces (docs/inputs.md §6.11): for each declaration given, the GeoJSON of
its pieces, read as its `[[piece]]` blocks say, written beside a copy of it in --out/<theme>/.

A reader per kind of service, none per source:
  arcgis   an ArcGIS REST layer's features, or an ImageServer's catalogue (`url`: the layer;
           `where`; paged by `page`), as GeoJSON or Esri JSON, whichever the layer answers
  stac     a STAC API collection's items (`url`: the collection; paged by its `next` links), each
           item's footprint or (`geometry`) the GeoJSON file of its asset of that name
  atom     an ATOM download feed (`url`; `follow` the feeds it links): its files' `bbox`es
           (`latfirst` where they're south west north east), or its entries' GeoRSS polygons and boxes
  wfs      a WFS feature type (`url`, `type`; `crs` where the answer doesn't name its own), as
           GeoJSON or GML (`format`; `version`)
  file     an index file fetched whole (`url`): GeoJSON, GML or a GeoPackage (`layer`), or a zip
           of one of them or of a shapefile (`member`; `encoding`, its text's, else its .cpg's or UTF-8)
  listing  a listing of georeferenced names (`url`, or several in turn: `urls`): an S3 bucket
           (`prefix`), a page of links, a text file, a zip's members (its central directory by
           range request) or a spreadsheet's rows, cells joined by ";" (`sheet`) (`list` = "s3", "links",
           "lines", "zip", "xlsx"); `pattern`, a regular expression with the
           groups `x` and `y` (and `ew`, `ns`: W and S negate); `unit`, the CRS units of one of the
           name's; `size`, a tile's side(s) in CRS units; `corner`, the corner named ("sw", "nw");
           `crs`; or the group `ref`, a national grid's lettered reference (`grid`: "osgb",
           "irish"), its square and side read from it; or, for a nested sheet nomenclature,
           `digits` (each a group's steps) and `origin`
  raster   each file's own header and an overview, by range request (`urls`; `list` and `pattern`,
           a listing's names as `listing` reads one; or `stac` and `asset`, a STAC collection's
           items' files): its bounds and where it has data, at the coarsest overview, or with
           `mask_m` at the coarsest whose pixels are no wider than that (a file with none stops it)
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
import ssl
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

import certifi
import pyproj
import shapely
import shapely.affinity
import shapely.geometry as sg

UA = "scenic-roads/0.1 (personal offline map)"
# Mozilla's trust store (certifi's), not the system's file, which lacks roots some publishers chain to
# (Quebec's: Sectigo's R46); verified as strictly.
TLS = ssl.create_default_context(cafile=certifi.where())
# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): documented services and published files.
FETCHES = "api, data"
CLASSES = ("mask", "files", "index", "service", "claimed", "unknown")
AVAILABILITY = ("open", "login", "paid", "closed")
# The class a method's extents are; None: the declaration says (`class`).
METHOD_CLASS = {"arcgis": None, "stac": "index", "wfs": None, "file": None, "listing": "index", "raster": "files", "atom": "index", "none": "unknown"}
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
                with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=180, context=TLS) as r:
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
    """An ArcGIS REST layer's features (a FeatureServer or MapServer layer, or an ImageServer's
    catalogue), in WGS 84, generalised by the service: as GeoJSON where the layer offers it, as
    Esri JSON otherwise (an ImageServer offers only that)."""
    layer = net.json(b["url"].rstrip("/") + "?f=json")
    oid = layer.get("objectIdField") or next((f["name"] for f in layer.get("fields") or [] if f.get("type") == "esriFieldTypeOID"), "OBJECTID")
    esri = "geojson" not in (layer.get("supportedQueryFormats") or "").lower()
    fields = sorted(wanted_fields(b) | {oid})
    base = b["url"].rstrip("/") + "/query?"
    # (The features the filter holds, counted by the service first: the pages must come to as many,
    # so none was skipped or read twice at a page's edge.)
    count = net.json(base + urllib.parse.urlencode({"where": b.get("where", "1=1"), "returnCountOnly": "true", "f": "json"}))
    if "error" in count or not isinstance(count.get("count"), int):
        sys.exit(f"{b['url']}: no count of its features: {count.get('error', count)}")
    page, last, seen = int(b.get("page", 500)), None, 0
    while True:
        # (Paged by the object id, each page after the last one's: the same features in the same
        # order each run, and no deep offsets, which a catalogue of 100,000 items times out on.)
        where = b.get("where", "1=1") if last is None else f"({b.get('where', '1=1')}) AND {oid} > {last}"
        q = {"where": where, "outFields": ",".join(fields), "outSR": "4326", "f": "json" if esri else "geojson",
             "returnGeometry": "true", "geometryPrecision": "6", "maxAllowableOffset": f"{tol_deg / 4:.7f}",
             "orderByFields": oid, "resultRecordCount": str(page)}
        d = net.json(base + urllib.parse.urlencode(q))
        if "error" in d:
            sys.exit(f"{b['url']}: {d['error']}")
        feats = d.get("features") or []
        ids = [(f.get("attributes" if esri else "properties") or {}).get(oid, f.get("id")) for f in feats]
        if None in ids or len(set(ids)) < len(ids) or (last is not None and any(i <= last for i in ids)):
            # (A joined layer's rows share their table's id: paging by it would skip rows.)
            sys.exit(f"{b['url']}: its features' {oid} isn't one each: can't page by it")
        for f in feats:
            props = f.get("attributes" if esri else "properties") or {}
            g = esri_shape(f.get("geometry")) if esri else (flat_shape(f["geometry"]) if f.get("geometry") else None)
            if g is not None:
                yield g, props
        seen += len(feats)
        more = d.get("exceededTransferLimit") or (d.get("properties") or {}).get("exceededTransferLimit")
        if not feats or (len(feats) < page and not more):
            break
        last = max(ids)
    if seen != count["count"]:
        sys.exit(f"{b['url']}: {seen} features read in pages, where the service counts {count['count']}: "
                 f"its {oid} doesn't page (rows sharing one, or the layer changed while read)")


def flat_shape(g):
    """A GeoJSON geometry as a shapely one from its positions' first two numbers: a service's z and m
    dropped (an ArcGIS layer's m may be null, which no geometry takes)."""
    def trim(c):
        return c[:2] if c and not isinstance(c[0], list) else [trim(x) for x in c]
    return sg.shape({**g, "coordinates": trim(g["coordinates"])} if "coordinates" in g else g)


def esri_shape(g):
    """An Esri JSON polygon as a shapely one: its rings by the even-odd rule (each ring flips what it
    encloses), so exteriors and holes need no telling apart by their winding."""
    if not g or not g.get("rings"):
        return None
    out = None
    for r in g["rings"]:
        if len(r) < 4:
            continue
        p = shapely.make_valid(sg.Polygon([c[:2] for c in r]))
        out = p if out is None else out.symmetric_difference(p)
    return None if out is None or out.is_empty else polygons(out)


def stac_items(net, url, page=500):
    """A STAC or OGC API Features collection's items, `page` a request, paged by its `next` links up
    to the first page with no items. A next link without the page size asked for gets it back: a
    server whose links drop it (Luzern's) answers the next page of its default size, so the pages
    would skip or repeat."""
    url, seen = url.rstrip("/") + "/items?limit=" + str(page), set()
    while url:
        if url in seen:
            sys.exit(f"{url}: a `next` link back to a page already read")
        seen.add(url)
        d = net.json(url)
        if not d.get("features"):
            # (Some OGC API servers link a `next` from an empty page: Ellipsis Drive's, back to the first.)
            return
        yield from d["features"]
        url = next((l["href"] for l in d.get("links") or [] if l.get("rel") == "next"), None)
        if url and "limit" not in urllib.parse.parse_qs(urllib.parse.urlsplit(url).query):
            url += ("&" if urllib.parse.urlsplit(url).query else "?") + "limit=" + str(page)


def read_stac(net, b, _tol):
    """A STAC collection's items: each item's footprint, or with `geometry` the GeoJSON file its
    asset of that name is (a finer outline than the item's own, where a publisher gives one)."""
    for f in stac_items(net, b["url"], b.get("page", 500)):
        props = {**(f.get("properties") or {}), "id": f.get("id"), "collection": f.get("collection")}
        asset = (f.get("assets") or {}).get(b["geometry"]) if b.get("geometry") else None
        if asset:
            g = json.loads(net.get(asset["href"]))
            parts = [sg.shape(x["geometry"]) for x in (g.get("features") or [g]) if x.get("geometry")]
            if parts:
                # (In WGS 84 like the items' own footprints, whatever CRS the file names.)
                shape = shapely.union_all([polygons(shapely.make_valid(x)) for x in parts])
                yield (to_wgs84(shape, answer_crs(g), 20.0) if answer_crs(g) else shape), props
        elif f.get("geometry"):
            yield sg.shape(f["geometry"]), props


def read_wfs(net, b, _tol):
    """A WFS feature type's features, paged (`page`, by `startIndex`), as GeoJSON or GML, whichever
    the server answers in the `format` asked for; `version` 2.0.0 (the default) or a 1.x's
    (`typeName`, `maxFeatures`)."""
    page, off = int(b.get("page", 2000)), 0
    version = b.get("version", "2.0.0")
    old = version.startswith("1.")
    while True:
        q = {"service": "WFS", "version": version, "request": "GetFeature", ("typeName" if old else "typeNames"): b["type"],
             "outputFormat": b.get("format", "application/json"), ("maxFeatures" if old else "count"): str(page),
             "startIndex": str(off)}
        if b.get("fields"):
            q["propertyName"] = ",".join(b["fields"])
        body = net.get(b["url"] + ("&" if "?" in b["url"] else "?") + urllib.parse.urlencode(q))
        if b"ExceptionReport" in body[:1000]:
            sys.exit(f"{b['url']}: {body[:1000].decode('utf-8', 'replace')}")
        # (Every feature, those without geometry too, so a page's count is the server's.)
        feats = list(index_features(body, b))
        yield from feats
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


INDEX_KINDS = (".shp", ".gpkg", ".geojson", ".json", ".gml", ".xml")


def read_file(net, b, _tol):
    """An index file fetched whole: GeoJSON, GML, a GeoPackage, or a zip holding one of them or a
    shapefile (`member`: a part of the name of the one to read, where it holds several)."""
    body = net.get(b["url"])
    if body[:2] == b"PK":
        z = zipfile.ZipFile(io.BytesIO(body))
        names = [n for n in z.namelist() if n.lower().endswith(INDEX_KINDS) and (not b.get("member") or b["member"] in n)]
        if not names:
            sys.exit(f"{b['url']}: no index file in the zip{' named ' + repr(b['member']) if b.get('member') else ''}")
        # (A shapefile first: its .xml is its metadata.)
        name = next((n for n in names if n.lower().endswith(".shp")), names[0])
        if name.lower().endswith(".shp"):
            yield from shapefile_features(z, name[:-4], b)
            return
        body = z.read(name)
    yield from index_features(body, b)


def index_features(body, b):
    """An index file's features, by what it is: a GeoPackage, GML, or GeoJSON."""
    if body[:16] == b"SQLite format 3\0":
        yield from geopackage_features(body, b)
    elif body.lstrip()[:1] == b"<":
        yield from gml_features(body, b)
    else:
        d = json.loads(body)
        if answer_crs(d) and "crs" not in b:
            b["crs"] = answer_crs(d)
        for f in d["features"]:
            # (One without geometry is still a feature: a WFS page's count is all of them.)
            yield (lambda f=f: xy(flat_shape(f["geometry"]), b) if f.get("geometry") else None), f.get("properties") or {}


def shapefile_features(z, stem, b):
    import shapefile
    opened = {ext: io.BytesIO(z.read(next(n for n in z.namelist() if n.lower() == (stem + "." + ext).lower()))) for ext in ("shp", "shx", "dbf")}
    prj = next((n for n in z.namelist() if n.lower() == (stem + ".prj").lower()), None)
    if prj and "crs" not in b:
        b["crs"] = z.read(prj).decode("utf-8", "replace")
    # (Its text's encoding: the declaration's `encoding`, else the one its .cpg names, else UTF-8.)
    cpg = next((n for n in z.namelist() if n.lower() == (stem + ".cpg").lower()), None)
    encoding = b.get("encoding") or (z.read(cpg).decode("ascii", "replace").strip() if cpg else "utf-8")
    if encoding.isdigit():
        # (A code page by its number alone, as Esri writes it: "1252".)
        encoding = "cp" + encoding
    r = shapefile.Reader(**opened, encoding=encoding)
    names = [f[0] for f in r.fields[1:]]
    def shape(k):
        sh = r.shape(k)
        return sg.shape(sh.__geo_interface__) if sh.shapeType else None
    for k, rec in enumerate(r.iterRecords()):
        yield (lambda k=k: shape(k)), dict(zip(names, rec))


def geopackage_features(body, b):
    """A GeoPackage's features table (`layer`, where it has several), its CRS from the package's own
    table of them; each row's columns its properties, its geometry the GeoPackage blob's WKB."""
    import sqlite3
    import tempfile
    with tempfile.NamedTemporaryFile(suffix=".gpkg") as tmp:
        tmp.write(body)
        tmp.flush()
        db = sqlite3.connect(f"file:{tmp.name}?mode=ro", uri=True)
        try:
            layers = db.execute("SELECT c.table_name, g.column_name, s.organization, s.organization_coordsys_id, s.definition "
                                "FROM gpkg_contents c JOIN gpkg_geometry_columns g ON g.table_name = c.table_name "
                                "JOIN gpkg_spatial_ref_sys s ON s.srs_id = g.srs_id WHERE c.data_type = 'features'").fetchall()
            pick = [x for x in layers if not b.get("layer") or x[0] == b["layer"]]
            if len(pick) != 1:
                sys.exit(f"{b['url']}: its feature tables are {[x[0] for x in layers]}: name one with `layer`")
            table, col, org, code, wkt = pick[0]
            if "crs" not in b:
                b["crs"] = f"{org}:{code}" if org and org.upper() == "EPSG" else wkt
            cur = db.execute(f'SELECT * FROM "{table}"')
            names = [d[0] for d in cur.description]
            for row in cur:
                props = dict(zip(names, row))
                blob = props.pop(col)
                yield (lambda blob=blob: xy(gpkg_geometry(blob), b) if blob else None), props
        finally:
            db.close()


def gpkg_geometry(blob):
    """A GeoPackage geometry blob's shape: its header ("GP", a version, flags saying the envelope's
    size and the byte order, the SRS id, the envelope), then WKB."""
    if blob[:2] != b"GP":
        sys.exit("a GeoPackage geometry without its GP header")
    flags = blob[3]
    if flags & 0x10:
        return None
    envelope = (0, 32, 48, 48, 64)[(flags >> 1) & 7]
    return shapely.from_wkb(bytes(blob[8 + envelope:]))


GML = ("http://www.opengis.net/gml", "http://www.opengis.net/gml/3.2")
GML_SHAPES = ("Polygon", "Surface", "MultiSurface", "MultiPolygon", "Envelope")


def gml_features(body, b):
    """A GML feature collection's features (a WFS's answer, or a file), each its simple child
    elements' text as properties and its first polygon, surface or envelope as its geometry, in
    the CRS its `srsName` names (where the declaration names none)."""
    root = ET.fromstring(body)
    for feat in gml_members(root):
        props, geom = {}, None
        for child in feat:
            if len(child) == 0:
                if child.text and child.text.strip():
                    props[child.tag.rpartition("}")[2]] = child.text.strip()
            elif geom is None:
                geom = next((e for e in child.iter() if e.tag.rpartition("}")[2] in GML_SHAPES and e.tag[1:].partition("}")[0] in GML), None)
        if geom is None:
            yield (lambda: None), props
            continue
        srs = geom.get("srsName") or next((e.get("srsName") for e in geom.iter() if e.get("srsName")), None)
        if srs and "crs" not in b:
            b["crs"] = srs
        yield (lambda geom=geom: xy(gml_shape(geom), b)), props


def gml_members(root):
    """A GML collection's features: the children of its member elements (wfs:member,
    gml:featureMember, gml:featureMembers)."""
    for el in root.iter():
        if el.tag.rpartition("}")[2] in ("member", "featureMember", "featureMembers"):
            yield from el


def gml_shape(el):
    """A GML polygon, surface, multi-surface or envelope as a shapely geometry, in its own axes'
    order."""
    tag = el.tag.rpartition("}")[2]
    if tag == "Envelope":
        lo = [float(v) for v in el.findtext("{*}lowerCorner").split()]
        hi = [float(v) for v in el.findtext("{*}upperCorner").split()]
        return sg.box(lo[0], lo[1], hi[0], hi[1])
    polys = []
    for p in el.iter():
        if p.tag.rpartition("}")[2] not in ("Polygon", "PolygonPatch"):
            continue
        rings = {"exterior": [], "interior": []}
        for r in p:
            kind = r.tag.rpartition("}")[2]
            kind = {"outerBoundaryIs": "exterior", "innerBoundaryIs": "interior"}.get(kind, kind)
            if kind in rings:
                ring = next((x for x in r.iter() if x.tag.rpartition("}")[2] == "LinearRing"), None)
                if ring is not None:
                    rings[kind].append(gml_ring(ring))
        if rings["exterior"]:
            polys.append(shapely.make_valid(sg.Polygon(rings["exterior"][0], rings["interior"])))
    return polygons(shapely.union_all(polys)) if polys else None


def gml_ring(ring):
    pos = ring.find("{*}posList")
    if pos is not None:
        dim = int(pos.get("srsDimension") or ring.get("srsDimension") or 2)
        v = [float(x) for x in pos.text.split()]
        return list(zip(v[0::dim], v[1::dim]))
    coords = ring.findtext("{*}coordinates")
    if coords is not None:
        return [tuple(float(x) for x in c.split(",")[:2]) for c in coords.split()]
    return [tuple(float(x) for x in p.text.split()[:2]) for p in ring.iterfind("{*}pos")]


ATOM = "{http://www.w3.org/2005/Atom}"
GEORSS = "{http://www.georss.org/georss}"


def read_atom(net, b, _tol):
    """An ATOM download feed (an INSPIRE download service's): each file it links (`rel="section"`,
    or any non-feed link) with its own `bbox` attribute (west south east north), else each entry's
    GeoRSS polygon or box (latitude first, as GeoRSS writes them; `latfirst = true` where a feed
    writes its `bbox`es latitude first too, Brussels's). `follow = true` reads the
    feeds its entries link (`type="application/atom+xml"`, `rel="alternate"`) too, each once. A file's properties: its
    entry's `title`, `updated` and `id`, and its own `href`, `title` and `length`."""
    todo, seen = [b["url"]], set()
    while todo:
        url = todo.pop(0)
        if url in seen:
            continue
        seen.add(url)
        feed = ET.fromstring(net.get(url))
        for e in feed.iter(ATOM + "entry"):
            entry = {k: e.findtext(ATOM + k) for k in ("title", "updated", "id")}
            boxed = False
            for l in e.iter(ATOM + "link"):
                kind = l.get("type") or ""
                if "atom" in kind:
                    # (An entry's own feed, `alternate`: never `up` or `self`, which lead back to the
                    # service's top, every dataset it has.)
                    if b.get("follow") and l.get("rel", "alternate") == "alternate":
                        todo.append(l.get("href"))
                    continue
                if l.get("bbox"):
                    w, s, east, n = (float(v) for v in l.get("bbox").split())
                    boxed = True
                    yield xy(sg.box(w, s, east, n), b), {**entry, "href": l.get("href"), "link_title": l.get("title"), "length": l.get("length")}
            if boxed:
                continue
            for p in e.iter(GEORSS + "polygon"):
                v = [float(x) for x in p.text.split()]
                yield sg.Polygon(list(zip(v[1::2], v[0::2]))), entry
            for p in e.iter(GEORSS + "box"):
                s, w, n, east = (float(x) for x in p.text.split())
                yield sg.box(w, s, east, n), entry


def listed_names(net, b):
    """A listing's names: of `url`, or of each of `urls` in turn (a set split into several
    archives or pages)."""
    for url in b.get("urls") or [b["url"]]:
        yield from names_at(net, b, url)


def names_at(net, b, url):
    kind = b.get("list", "s3")
    if kind == "s3":
        token = None
        while True:
            q = {"list-type": "2", "prefix": b.get("prefix", "")}
            if b.get("delimiter"):
                q["delimiter"] = b["delimiter"]
            if token:
                q["continuation-token"] = token
            x = ET.fromstring(net.get(url.rstrip("/") + "/?" + urllib.parse.urlencode(q)))
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
        # (An attribute's name in any case: an IIS directory listing writes HREF.)
        yield from re.findall(r'href="([^"]+)"', net.get(url).decode("utf-8", "replace"), re.I)
    elif kind == "lines":
        yield from net.get(url).decode("utf-8", "replace").split()
    elif kind == "zip":
        # (A zip's central directory, by range request: its members' names, none of its data.)
        yield from zipfile.ZipFile(RangeFile(net, url)).namelist()
    elif kind == "xlsx":
        yield from sheet_rows(net.get(url), b.get("sheet"))
    else:
        sys.exit(f"unknown list {kind!r}")


def grid_ref(ref, grid):
    """A national grid's lettered reference: its square's south-west corner and side, in the
    grid's metres. `grid` "osgb": the British National Grid's (EPSG:27700) two letters, its 500 km
    and 100 km squares; "irish": the Irish Grid's (EPSG:29903) one, its 100 km square. Then
    digits, the eastings' half first (NS16: 10 km, NG2004: 1 km), and a quadrant (NS16NE: 5 km)."""
    m = re.fullmatch(r"([A-HJ-Z]{1,2})(\d*)(NE|NW|SE|SW)?", ref.upper())
    if not m or len(m.group(1)) != {"osgb": 2, "irish": 1}[grid] or len(m.group(2)) % 2:
        sys.exit(f"{ref!r} isn't a {grid} grid reference")
    letters, digits, quad = m.groups()
    # (A 5 × 5 grid of letters, A at the top left, I left out.)
    cells = [ord(c) - 65 - (c > "I") for c in letters]
    x, y = ((cells[0] % 5 - 2) * 500_000, (3 - cells[0] // 5) * 500_000) if grid == "osgb" else (0, 0)
    x, y = x + cells[-1] % 5 * 100_000, y + (4 - cells[-1] // 5) * 100_000
    k = len(digits) // 2
    side = 10 ** (5 - k)
    if k:
        x, y = x + int(digits[:k]) * side, y + int(digits[k:]) * side
    if quad:
        side //= 2
        x, y = x + side * (quad[1] == "E"), y + side * (quad[0] == "N")
    return x, y, side


XLSX = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"


def sheet_rows(body, sheet=None):
    """A spreadsheet's (xlsx) rows, the first sheet's or the one named: each its cells' text joined
    by ";", a cell left empty as nothing between two, a number formatted as a date as its ISO date
    (the workbook's count of days from 1899-12-30)."""
    z = zipfile.ZipFile(io.BytesIO(body))
    names = z.namelist()
    strings = ["".join(t.text or "" for t in si.iter(XLSX + "t")) for si in ET.fromstring(z.read("xl/sharedStrings.xml"))] if "xl/sharedStrings.xml" in names else []
    dated = []
    if "xl/styles.xml" in names:
        styles = ET.fromstring(z.read("xl/styles.xml"))
        custom = {f.get("numFmtId"): re.sub(r'"[^"]*"|\[[^]]*\]', "", f.get("formatCode", "")) for f in styles.iter(XLSX + "numFmt")}
        # (Excel's own date formats are 14 to 22; a workbook's own, a code writing a year or a day.)
        xfs = styles.find(XLSX + "cellXfs")
        dated = [14 <= int(x.get("numFmtId", 0)) <= 22 or bool(re.search(r"[yd]", custom.get(x.get("numFmtId"), ""), re.I))
                 for x in (xfs if xfs is not None else [])]
    rel = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"
    sheets = [(s.get("name"), s.get(rel)) for s in ET.fromstring(z.read("xl/workbook.xml")).iter(XLSX + "sheet")]
    pick = next((r for n, r in sheets if sheet in (None, n)), None)
    if pick is None:
        sys.exit(f"no sheet {sheet!r} in the workbook: its sheets are {[n for n, _ in sheets]}")
    target = next(x.get("Target") for x in ET.fromstring(z.read("xl/_rels/workbook.xml.rels")) if x.get("Id") == pick)
    for row in ET.fromstring(z.read("xl/" + target.lstrip("/").removeprefix("xl/"))).iter(XLSX + "row"):
        cells = {}
        for c in row.iter(XLSX + "c"):
            col = sum((ord(ch) - 64) * 26 ** i for i, ch in enumerate(reversed(re.match(r"[A-Z]+", c.get("r")).group())))
            v, kind = c.findtext(XLSX + "v"), c.get("t")
            if kind == "s":
                v = strings[int(v)]
            elif kind == "inlineStr":
                v = "".join(t.text or "" for t in c.iter(XLSX + "t"))
            elif v is not None and kind in (None, "n") and int(c.get("s", 0)) < len(dated) and dated[int(c.get("s", 0))]:
                v = (datetime.date(1899, 12, 30) + datetime.timedelta(days=int(float(v)))).isoformat()
            cells[col] = v or ""
        if cells:
            yield ";".join(cells.get(k, "") for k in range(1, max(cells) + 1))


def read_listing(net, b, _tol):
    pat = re.compile(b["pattern"])
    unit = float(b.get("unit", 1))
    size = b.get("size")
    if size is not None and not isinstance(size, list):
        size = [size, size]
    corner = b.get("corner", "sw")
    geographic = pyproj.CRS.from_user_input(b.get("crs", "OGC:CRS84")).is_geographic
    digits = b.get("digits") or []
    where = {"x", "y", "ew", "ns", "ref"} | {d["group"] for d in digits}
    seen, sides = set(), set()
    for name in listed_names(net, b):
        m = pat.search(name)
        if not m:
            continue
        g = m.groupdict()
        if "ref" in g:
            x, y, w = grid_ref(g["ref"], b["grid"])
            h = w
        else:
            x, y = sheet_corner(g, b) if digits else (float(g["x"]) * unit, float(g["y"]) * unit)
            w, h = size
            if g.get("ew") in ("W", "w"):
                x = -x
            if g.get("ns") in ("S", "s"):
                y = -y
            if corner == "nw":
                y -= h
        props = {k: v for k, v in g.items() if k not in where}
        # (A name listed twice is one tile; a tile's two files of different readings, a survey and
        # its successor, are two.)
        key = (x, y, w, tuple(sorted(props.items())))
        if key in seen:
            continue
        seen.add(key)
        sides.add(max(w, h))
        yield sg.box(x, y, x + w, y + h), props
    # (The extent is good to a tile, the largest read: its grain.)
    b["grain_m"] = round(max(sides | ({max(size)} if size else set()), default=0) * (M_PER_DEG if geographic else 1), 1)


def sheet_corner(g, b):
    """A sheet's corner from its name in a nested nomenclature (`digits`): `origin`, plus for each
    digit group its number from `base` (1) times its steps, `x` and `y`; a digit naming a column and
    a row at once says how many columns, `cols` (a quadrant's 1 and 2 its southern row, 3 and 4 its
    northern: `cols = 2`)."""
    x, y = (float(v) for v in b.get("origin", [0, 0]))
    for d in b["digits"]:
        k = int(g[d["group"]]) - int(d.get("base", 1))
        cols = int(d.get("cols", 0))
        col, row = (k % cols, k // cols) if cols else (k, k)
        x += float(d.get("x", 0)) * col
        y += float(d.get("y", 0)) * row
    return x, y


def raster_files(net, b):
    """A raster piece's files: its `urls`, a listing's names (`list`, as `listing` reads one) that
    `pattern` matches, each taken relative to `url` (a bucket's keys to the bucket), or a STAC
    collection's (`stac`) items' asset (`asset`)."""
    if "list" in b:
        base = b["url"].rstrip("/") + "/"
        return sorted({urllib.parse.urljoin(base, n) for n in listed_names(net, b) if re.search(b["pattern"], n)})
    if "stac" not in b:
        return b["urls"]
    return sorted(f["assets"][b["asset"]]["href"] for f in stac_items(net, b["stac"]) if b["asset"] in f.get("assets", {}))


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
                levels = ds.overviews(1)
                if not levels:
                    # Its mask would be the whole file read (every block); a declaration's
                    # `pattern` leaves such files out.
                    sys.exit(f"{url}: no overviews, so its mask is a whole download")
                fine = [f for f in levels if f * px <= float(b.get("mask_m", 0))]
                # (No overview as fine as `mask_m`: the finest there is, never the full resolution,
                # which would be a whole download; the grain recorded says how fine it was.)
                f = fine[-1] if fine else levels[0 if "mask_m" in b else -1]
                h, w = max(1, math.ceil(ds.height / f)), max(1, math.ceil(ds.width / f))
                mask = ds.dataset_mask(out_shape=(h, w))
                tr = ds.transform @ ds.transform.scale(ds.width / w, ds.height / h)
                shapes = [sg.shape(s) for s, v in rasterio.features.shapes((mask > 0).astype(np.uint8), mask=mask > 0, transform=tr) if v]
                geom = shapely.union_all(shapes)
                res = max(abs(ds.transform.a), abs(ds.transform.e))
                if ds.crs.is_geographic:
                    mid = math.radians((ds.bounds.top + ds.bounds.bottom) / 2)
                    res = max(abs(ds.transform.e) * M_PER_DEG, abs(ds.transform.a) * M_PER_DEG * math.cos(mid))
                b["grain_m"] = max(b.get("grain_m", 0), round(f * px, 1))
                yield to_wgs84(geom, ds.crs.to_wkt(), abs(tr.a)), {"res_m": round(res, 4)}


READERS = {"arcgis": read_arcgis, "stac": read_stac, "wfs": read_wfs, "file": read_file, "listing": read_listing, "raster": read_raster, "atom": read_atom}


# ---- from features to pieces ----

def latest_versions(feats, spec):
    """Of the features naming one tile (the match of `pattern` in their `field`), only the one
    greatest in `order`: a collection keeping each tile's earlier versions beside its newest
    (swissALTI3D's swissalti3d_2019_2600-1199 and _2025_2600-1199). A feature naming none is kept."""
    pat, best = re.compile(spec["pattern"]), {}
    for geom, props in feats:
        m = pat.search(str(props.get(spec["field"], "")))
        if not m:
            yield geom, props
        elif m.group(0) not in best or str(props.get(spec["order"])) > best[m.group(0)][0]:
            best[m.group(0)] = (str(props.get(spec["order"])), geom, props)
    for _, geom, props in best.values():
        yield geom, props


def wanted_fields(b):
    out = set(b.get("by", []))
    if b.get("name"):
        out.add(b["name"])
    for m in (b.get("metrics") or {}).values():
        for r in m if isinstance(m, list) else [m]:
            if isinstance(r, dict):
                out.add(r["field"])
    return out


def metric(spec, props):
    """(value or None, the publisher's class or None) of a metric's declaration on a feature: a
    constant, a reading, or a list of readings, the first giving a value (a field the publisher
    fills only sometimes, then another)."""
    if isinstance(spec, list):
        stated = None
        for s in spec:
            v, cl = metric(s, props)
            if v is not None:
                return v, cl
            stated = stated or cl
        return None, stated
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
        if how == "yymmdd":
            # Six-digit dates of the 2000s, one or several ("090317 - 100526"): the last year.
            years = [2000 + int(d[:2]) for d in re.findall(r"(?<!\d)\d{6}(?!\d)", str(v))]
            return (max(years) if years else None), None
        # A year alone or opening a compact date (20230930).
        years = [int(y) for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?:[01]\d[0-3]\d)?(?!\d)", str(v))]
        if how == "text":
            # (A span's end written with two digits, "2011-12": 2012; a date's month, "2011-12-05", isn't one.)
            years += [int(y[:2] + e) for y, e in re.findall(r"(?<!\d)((?:19|20)\d\d)-(\d\d)(?![\d-])", str(v)) if e > y[2:]]
        if not years:
            return None, None
        return (years[0] if how == "iso" else max(years)), None
    if "text" in spec:
        # A number written with its unit ("2 metre", "10 pts/m2", "0.35m (spec)": a remark in
        # brackets after it kept out): the unit must be the one declared.
        m = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*([^\s(]+)\s*(?:\([^)]*\))?\s*", str(v))
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
    groups, n, flt, exc = {}, 0, b.get("filter") or {}, b.get("exclude") or {}
    repaired, quarter = 0, {}
    feats = READERS[method](net, b, tol_deg)
    for geom, props in (latest_versions(feats, b["latest"]) if b.get("latest") else feats):
        if any(str(props.get(k)) not in [str(x) for x in want] for k, want in flt.items()):
            continue
        # (`exclude`, the filter's counterpart: a feature whose field is one of these is left out,
        # "" for a field empty or missing.)
        if any(("" if props.get(k) is None else str(props.get(k))) in [str(x) for x in drop] for k, drop in exc.items()):
            continue
        if callable(geom):
            # (A reader's geometry built only for the features the filter keeps: a national file's
            # thousands of areas for the one a claim names.)
            geom = geom()
            if geom is None:
                continue
        if not geom.is_valid:
            repaired += 1
            geom = shapely.make_valid(geom)
        # (Each feature generalised to a quarter of the tolerance before they're dissolved, as
        # `arcgis` asks its service to: a source mask's pixel staircases, RGE ALTI's 290,932 polygons
        # of some 120 million positions, took hours to dissolve and tidy unsimplified.)
        crs = b.get("crs", "OGC:CRS84")
        if crs not in quarter:
            quarter[crs] = (tol_deg if pyproj.CRS.from_user_input(crs).is_geographic else tol_m) / 4
        geom = geom.simplify(quarter[crs], preserve_topology=True)
        n += 1
        by = tuple(str(props.get(k)) for k in b.get("by", []))
        ms = {k: metric(s, props) for k, s in specs.items()}
        # (A metric's reading: its value and the publisher's class, so a piece's `stated` is all its
        # features'.)
        varying = tuple((k, *ms[k]) for k in sorted(specs) if isinstance(specs[k], (dict, list)))
        g = groups.setdefault((by, varying), {"geoms": [], "ms": ms, "name": props.get(b["name"]) if b.get("name") else b.get("label")})
        g["geoms"].append(geom)
    crs = b.get("crs", "OGC:CRS84")
    read = {"piece": b["id"], "method": method, "url": b.get("url") or b.get("stac") or b["urls"][0], "features": n,
            "date": net.fetched or today, "tolerance_m": tol_m}
    if "grain_m" in b:
        read["grain_m"] = b["grain_m"]
    feats = []
    shared = {}
    for by, _ in groups:
        shared[by] = shared.get(by, 0) + 1
    for (by, varying), g in sorted(groups.items(), key=lambda kv: str(kv[0])):
        native = shapely.union_all([polygons(shapely.make_valid(x)) for x in g["geoms"]])
        c = pyproj.CRS.from_user_input(crs)
        step = tol_deg if c.is_geographic else tol_m * 20
        geom = tidy(to_wgs84(native, crs, step), tol_deg)
        if geom is None:
            continue
        pid = "/".join([b["id"], *by])
        if shared[by] > 1:
            pid = "/".join([pid, *(f"{k}={v if cl is None else cl}" for k, v, cl in varying if (v, cl) != (None, None))])
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
