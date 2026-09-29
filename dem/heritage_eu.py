"""Heritage designations outside North America, from the designating authorities' open data.

Each source function returns (points, areas) as GeoJSON features in heritage.py's format:
level 1 World Heritage · 2 national, highest grade · 3 national, other grades · 4 regional ·
5 local. Downloads are cached under data/heritage/<country>/ (delete a file to refresh it).

  World Heritage: UNESCO World Heritage List, data.unesco.org dataset whc001 (CC BY-SA 4.0)
  France:  Ministère de la Culture, base Mérimée (POP): monuments historiques classés (2) and
           inscrits (3); sites patrimoniaux remarquables (areas) from the Géoportail de
           l'Urbanisme (servitude AC4). Licence Ouverte 2.0.
  Andorra: Govern d'Andorra, Inventari general del patrimoni cultural (IDE Andorra WFS): béns
           d'interès cultural (2) and béns inventariats (3). Private, personal use only.
  England: Historic England, National Heritage List for England (OGL v3): listed buildings
           Grade I (2) and II* (3), scheduled monuments (3), registered parks & gardens and
           battlefields (points and areas).
  Scotland: Historic Environment Scotland designations (OGL v3): listed buildings category A (2)
           and B (3), scheduled monuments (3), gardens & designed landscapes, battlefields.
  Wales:   Cadw via DataMapWales (OGL v3): listed buildings Grade I (2) and II* (3), scheduled
           monuments (3), registered historic parks & gardens.
  Northern Ireland: Department for Communities, Historic Environment Division (OGL v3): listed
           buildings A (2) and B+ (3), scheduled and state care monuments (3), historic parks.
  Guernsey: States of Guernsey protected buildings (A 2, B 3) and monuments (2); gov.gg terms,
           private use.
  Ireland: NIAH buildings rated National or International (2); Sites and Monuments Record entries
           in State care or under a preservation order (3). CC BY 4.0.
  Spain:   Bienes de interés cultural (2) from the regions that publish locations: Catalonia
           (BCIN), Castilla y León, Aragón, Comunitat Valenciana, Galicia, Navarra, Extremadura;
           Andalucía's protected heritage (3; the IAPH doesn't separate BIC). The national register
           has no coordinates; Madrid, Castilla-La Mancha and the Basque Country publish none.
  Portugal: Património Cultural, I.P. Atlas: monumentos nacionais (2), interesse público (3),
           interesse municipal (5); mainland only. CC BY-NC 4.0.
  Hong Kong: Antiquities and Monuments Office via the CSDI Portal: declared monuments (2), graded
           historic buildings Grade I (3), II (4), III (5).
  The lowest listing grades (England and Wales Grade II, Scotland C, Northern Ireland B1/B2:
  ~400,000 buildings) are left out: too many to show as points.
  No usable open register: Monaco, Isle of Man (permission required), Jersey (token-protected),
  Gibraltar (statutory text only).
"""
from __future__ import annotations

import csv
import io
import json
import re
import string
import sys
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

from pyproj import Transformer
from shapely.geometry import shape

H = Path(__file__).resolve().parent.parent / "data" / "heritage"
UA = {"User-Agent": "road-elevations/0.1 (personal offline map)"}
csv.field_size_limit(1 << 26)


def fetch(url: str, path: Path, params: dict | None = None) -> Path:
    """Download `url` to `path` once (cached)."""
    if path.exists() and path.stat().st_size > 0:
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    if params:
        url += ("&" if "?" in url else "?") + urllib.parse.urlencode(params)
    print(f"  downloading {url[:110]}", file=sys.stderr)
    tmp = path.with_suffix(path.suffix + ".part")
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=600) as r:
            tmp.write_bytes(r.read())
    except urllib.error.URLError as e:
        # Servers that send an incomplete certificate chain: curl verifies them against the
        # system trust store (which fetches the missing intermediate); verification stays on.
        if "CERTIFICATE_VERIFY_FAILED" not in str(e):
            raise
        import subprocess
        subprocess.run(["curl", "-sSfL", "-A", UA["User-Agent"], "-o", str(tmp), url], check=True, timeout=600)
    tmp.replace(path)
    return path


def arcgis(url: str, where: str, fields: str, cache: Path, page: int = 2000, extra: dict | None = None) -> list[dict]:
    """All features of an ArcGIS REST layer (GeoJSON, WGS84), paged by resultOffset; cached."""
    if cache.exists() and cache.stat().st_size > 0:
        return json.loads(cache.read_text())
    feats, off = [], 0
    try:  # the layer's object id field (usually OBJECTID) keeps paging stable
        with urllib.request.urlopen(urllib.request.Request(f"{url}?f=json", headers=UA), timeout=120) as r:
            oid = json.loads(r.read()).get("objectIdField") or "OBJECTID"
    except Exception:  # noqa: BLE001
        oid = "OBJECTID"
    while True:
        q = {"where": where, "outFields": fields, "outSR": 4326, "f": "geojson", "orderByFields": oid,
             "resultOffset": off, "resultRecordCount": page, **(extra or {})}
        url_q = f"{url}/query?{urllib.parse.urlencode(q)}"
        with urllib.request.urlopen(urllib.request.Request(url_q, headers=UA), timeout=600) as r:
            d = json.loads(r.read())
        if "error" in d:
            raise RuntimeError(f"{url}: {d['error']}")
        got = d.get("features", [])
        feats += got
        print(f"  {url.rsplit('/services/', 1)[-1][:60]}: {len(feats)}", file=sys.stderr)
        if len(got) < page and not d.get("exceededTransferLimit") and not d.get("properties", {}).get("exceededTransferLimit"):
            break
        off += len(got)
        if not got:
            break
    cache.parent.mkdir(parents=True, exist_ok=True)
    cache.write_text(json.dumps(feats))
    return feats


def rep_point(g: dict) -> tuple[float, float] | None:
    """A point on the feature (the point itself, or one inside a polygon)."""
    if not g or not g.get("coordinates"):
        return None
    if g["type"] == "Point":
        return tuple(g["coordinates"][:2])
    c = shape(g).representative_point()
    return c.x, c.y


SMALL = {"de", "del", "la", "las", "los", "el", "els", "les", "i", "y", "e", "da", "das", "do", "dos", "du", "des", "en", "a", "al",
         "of", "the", "and", "on", "upon", "in", "at", "by", "sur", "sous", "lès", "les", "et", "o"}


def tidy(name):
    """ALL-CAPS or Capitalised-Every-Word register names in normal capitalisation ("MANOR FARMHOUSE"
    → "Manor Farmhouse", "Casa De La Vila" → "Casa de la Vila", "L'ESGLÉSIA" → "L'Església")."""
    if not isinstance(name, str) or not name:
        return name
    words = name.split(" ")
    if name.isupper():
        words = [string.capwords(w.lower(), "-") for w in words]
        words = [re.sub(r"^([DdLl])'(\w)", lambda m: m.group(1).upper() + "'" + m.group(2).upper(), w) for w in words]
    elif not all(w[:1].isupper() for w in words if w[:1].isalpha()):
        return name  # already written with its own capitalisation
    out = [w if i == 0 or w.lower() not in SMALL else w.lower() for i, w in enumerate(words)]
    out = [re.sub(r"^([DL])'", lambda m: m.group(1).lower() + "'", w) if i > 0 else w for i, w in enumerate(out)]
    return " ".join(out)


def site(lon, lat, **props) -> dict:
    props["name"] = tidy(props.get("name"))
    return {"type": "Feature", "geometry": {"type": "Point", "coordinates": [round(float(lon), 6), round(float(lat), 6)]},
            "properties": {k: v for k, v in props.items() if v not in (None, "", "None")}}


def area(geom: dict, **props) -> dict:
    props["name"] = tidy(props.get("name"))
    return {"type": "Feature", "geometry": geom, "properties": {k: v for k, v in props.items() if v not in (None, "", "None")}}


# ---- names in the region's language ------------------------------------------------------------

def lang_at(lon: float, lat: float) -> str:
    """Primary language of the place (rough boxes, enough to pick a label): French in Québec,
    France and Monaco; Catalan in Catalonia, the Balearics and Andorra; Galician in Galicia;
    Spanish elsewhere in Spain; Portuguese in Portugal; Chinese in Hong Kong; else English."""
    if lon < -40:
        # Maine's border with Québec runs from (-71.1, 45.3) to (-70.0, 46.4) to (-69.2, 47.45).
        maine = lon > -71.1 and lat < (45.3 + (lon + 71.1) if lon <= -70.0 else 46.4 + (lon + 70.0) * 1.36)
        quebec = lat > 45.0 and -79.5 < lon < -61.5 and not (lon < -74.35 and lat < 45.6) and not (lon > -69.05 and lat < 47.95) and not maine
        return "fr" if quebec else "en"
    if 113.8 < lon < 114.5 and 22.1 < lat < 22.6:
        return "zh"
    if 1.40 < lon < 1.79 and 42.42 < lat < 42.66:
        return "ca"
    if (lat > 49.4 and lon < 1.7 and not (lon > -2.0 and lat < 51.0 and lon > 1.4)) or (-2.8 < lon < -1.9 and 49.1 < lat < 49.8):
        return "en"  # Britain, Ireland, the Channel Islands
    if -5.4 < lon < -5.3 and 36.1 < lat < 36.16:
        return "en"  # Gibraltar
    # Portugal: west of the border (the Guadiana in the south, the Douro/Minho in the north), plus
    # Madeira and the Azores.
    if (lon < -15.5 and lat > 32.0) or (lon < -24.0 and 36.5 < lat < 40.0):
        return "pt"
    pt_east = -7.40 if lat < 38.2 else -7.0 if lat < 39.7 else -6.85 if lat < 41.0 else -6.2 if lat < 41.6 else -6.6
    pt_north = 42.05 if lon < -8.1 else 41.95
    if -9.6 < lon < pt_east and 36.8 < lat < pt_north:
        return "pt"
    # Spain: south of the Pyrenean border (lower in the east, higher toward the Basque coast; the
    # Cantabrian coast runs to 43.8° N), the Balearics and the Canaries.
    border = 42.45 if lon > 1.8 else 42.75 if lon > -0.8 else 43.1 if lon > -1.5 else 43.37 if lon > -1.79 else 43.85
    if (lat < border and -9.4 < lon < 3.35) or (38.5 < lat < 40.3 and 1.1 < lon < 4.4) or (27.5 < lat < 29.5 and -18.3 < lon < -13.3):
        if 0.15 < lon < 3.4 and 40.5 < lat < 42.9:
            return "ca"
        if 1.1 < lon < 4.4 and 38.6 < lat < 40.2:
            return "ca"
        if -9.4 < lon < -6.7 and 41.8 < lat < 43.8:
            return "gl"
        return "es"
    if -5.5 < lon < 9.7 and 41.3 < lat < 51.2:
        return "fr"
    return "en"


def wd_sparql(query: str, cache: Path) -> list[dict]:
    """Wikidata query (JSON bindings), cached."""
    d = json.loads(fetch("https://query.wikidata.org/sparql", cache, {"query": query, "format": "json"}).read_text())
    return [{k: v["value"] for k, v in b.items()} for b in d["results"]["bindings"]]


def wd_search_labels(name: str, cache: dict, langs=("fr", "es", "pt", "ca", "gl", "zh"), search_lang: str = "en") -> dict:
    """Labels of the Wikidata item a name finds first (cached in `cache`; polite pacing, backing
    off when rate-limited)."""
    key = name if search_lang == "en" else f"{search_lang}:{name}"
    if key in cache:
        return cache[key]
    import time

    def get(params: dict) -> dict:
        url = f"https://www.wikidata.org/w/api.php?{urllib.parse.urlencode({**params, 'format': 'json'})}"
        for attempt in range(6):
            try:
                with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
                    return json.loads(r.read())
            except urllib.error.HTTPError as e:
                if e.code != 429 or attempt == 5:
                    raise
                time.sleep(float(e.headers.get("Retry-After") or 5) * (attempt + 1))
        return {}
    try:
        hits = get({"action": "wbsearchentities", "search": name, "language": search_lang, "limit": 1}).get("search", [])
        labels = {}
        if hits:
            ent = get({"action": "wbgetentities", "ids": hits[0]["id"], "props": "labels", "languages": "|".join(langs)})["entities"][hits[0]["id"]]
            labels = {k: v["value"][:1].upper() + v["value"][1:] for k, v in ent.get("labels", {}).items()}
        time.sleep(1.0)
    except Exception as e:  # noqa: BLE001
        print(f"  wikidata {name!r}: {e}", file=sys.stderr)
        return {}
    cache[key] = labels
    return labels


# ---- UNESCO World Heritage List (all countries) --------------------------------------------------

WHC_SRC = "UNESCO World Heritage Centre (data.unesco.org, CC BY-SA 4.0)"
WHC_NOTICE = "© UNESCO World Heritage Centre. World Heritage List data licensed under CC BY-SA 4.0."
COMPONENT = re.compile(r"\{name: (.*?), ref: ([^,]*), latitude: (-?[\d.]+), longitude: (-?[\d.]+)\}")


def unesco() -> tuple[list[dict], list[dict]]:
    d = json.loads(fetch("https://data.unesco.org/api/explore/v2.1/catalog/datasets/whc001/exports/json",
                         H / "unesco" / "whc001.json").read_text())
    # Names UNESCO doesn't publish (Portuguese, Catalan, Galician): Wikidata's labels by site id.
    wd: dict[str, dict] = {}
    try:
        for r in wd_sparql("""SELECT ?id ?pt ?ca ?gl WHERE { ?item wdt:P757 ?id .
              OPTIONAL { ?item rdfs:label ?pt FILTER(LANG(?pt) = "pt") } OPTIONAL { ?item rdfs:label ?ca FILTER(LANG(?ca) = "ca") }
              OPTIONAL { ?item rdfs:label ?gl FILTER(LANG(?gl) = "gl") } }""", H / "unesco" / "wd-labels.json"):
            m = re.fullmatch(r"(\d+)(?:bis|ter|quater)?", r["id"])  # "320bis" (extension) → 320; not components ("875-001")
            if m:
                prev = wd.setdefault(m.group(1), {})
                for k in ("pt", "ca", "gl"):
                    if r.get(k) and not prev.get(k):
                        prev[k] = r[k][:1].upper() + r[k][1:]
    except Exception as e:  # noqa: BLE001
        print(f"  UNESCO labels from Wikidata: {e}", file=sys.stderr)
    pts = []
    for s in d:
        comps = [(float(lo), float(la), n) for n, _ref, la, lo in COMPONENT.findall(s.get("components_list") or "")]
        if not comps and s.get("coordinates"):
            comps = [(s["coordinates"]["lon"], s["coordinates"]["lat"], None)]
        untag = lambda v: re.sub(r"<[^>]+>", "", v).strip() if v else v  # noqa: E731 (names carry <i> markup)
        names = {k: untag(v) for k, v in {"en": s["name_en"], "fr": s.get("name_fr"), "es": s.get("name_es"), "zh": s.get("name_zh"),
                                          **wd.get(str(s["id_no"]), {})}.items() if k in ("en", "fr", "es", "zh", "pt", "ca", "gl")}
        s["name_en"] = names["en"]
        # One country: its language (regional within Spain and Canada); several: by location.
        isos = [c.strip().lower() for c in (s.get("iso_codes") or "").split(",") if c.strip()]
        country = {"fr": "fr", "mc": "fr", "pt": "pt", "ad": "ca", "gb": "en", "ie": "en", "us": "en", "gi": "en"}.get(isos[0]) if len(isos) == 1 else None
        for lon, lat, cname in comps:
            here = lang_at(lon, lat)
            lang = country or (here if len(isos) != 1 or isos[0] in ("es", "ca") else here)
            if len(isos) == 1 and isos[0] == "es" and here not in ("es", "ca", "gl"):
                lang = "es"
            name = names.get(lang) or (names.get("es") if lang in ("ca", "gl") else None) or s["name_en"]
            pts.append(site(lon, lat, name=name, name_en=s["name_en"] if name != s["name_en"] else None,
                            component=cname if cname and cname not in (s["name_en"], name) else None,
                            level=1, designation="UNESCO World Heritage Site", category=s.get("category"),
                            date=s.get("date_inscribed"), criteria=s.get("criteria_txt"),
                            in_danger=True if str(s.get("danger")) == "True" else None,
                            url=f"https://whc.unesco.org/en/list/{s['id_no']}", source=WHC_SRC, notice=WHC_NOTICE))
    return pts, []


# ---- France --------------------------------------------------------------------------------------

FR_SRC = "Ministère de la Culture – base Mérimée (POP), Licence Ouverte 2.0"
FR_DATE = re.compile(r"(\d{4})/(\d{2})/(\d{2})\s*:\s*([^;]*)")


def france() -> tuple[list[dict], list[dict]]:
    path = fetch("https://ministere-culture.s3.sbg.io.cloud.ovh.net/POP/merimee.csv", H / "fr" / "merimee.csv")
    pts = []
    with open(path, encoding="utf-8", newline="") as f:
        for r in csv.DictReader(f, delimiter="|"):
            typ = (r.get("Typologie_de_la_protection") or "").lower()
            classe, inscrit = "classé" in typ, "inscrit" in typ
            if not (classe or inscrit):
                continue  # cross-references, delisted or destroyed buildings
            c = (r.get("coordonnees_au_format_WGS84") or "").replace(" ", "")
            try:
                lat, lon = (float(v) for v in c.split(","))
            except ValueError:
                continue
            grade = "classé" if classe else "inscrit"
            parts = {t.strip() for t in typ.split(";")}
            partial = f"{grade} mh partiellement" in parts and f"{grade} mh" not in parts
            dates = [(f"{y}-{m}-{d}", t.lower()) for y, m, d, t in FR_DATE.findall(r.get("Date_et_typologie_de_la_protection") or "")]
            first = min((d for d, t in dates if grade in t), default=None)
            pts.append(site(lon, lat, name=r.get("Titre_editorial_de_la_notice") or r.get("Denomination_de_l_edifice"),
                            level=2 if classe else 3,
                            designation=f"Monument historique {grade}" + (" (partiellement)" if partial else ""),
                            date=first, municipality=r.get("Commune_forme_editoriale"),
                            category=r.get("Denomination_de_l_edifice"), authority="Ministère de la Culture",
                            url=f"https://pop.culture.gouv.fr/notice/merimee/{r['Reference']}", source=FR_SRC))
    # Sites patrimoniaux remarquables: perimeters from the Géoportail de l'Urbanisme.
    spr = json.loads(fetch("https://data.geopf.fr/wfs/ows", H / "fr" / "spr-ac4.geojson", {
        "SERVICE": "WFS", "VERSION": "2.0.0", "REQUEST": "GetFeature", "TYPENAMES": "wfs_sup:generateur_sup_s",
        "CQL_FILTER": "suptype='ac4'", "OUTPUTFORMAT": "application/json", "COUNT": 5000}).read_text())
    areas = []
    seen = set()
    for ft in spr["features"]:
        p, g = ft["properties"], ft["geometry"]
        if not g or p.get("idsup") in seen:
            continue
        seen.add(p.get("idsup"))
        areas.append(area(g, name=(p.get("nomsuplitt") or "Site patrimonial remarquable"), level=2,
                          designation="Site patrimonial remarquable", authority="Ministère de la Culture",
                          url="https://www.geoportail-urbanisme.gouv.fr/", source="Géoportail de l'Urbanisme (servitude AC4), Licence Ouverte 2.0"))
    return pts, areas


# ---- Andorra ------------------------------------------------------------------------------------

AD_SRC = "Govern d'Andorra – Inventari general del patrimoni cultural (IDE Andorra); private, personal use only"


def andorra() -> tuple[list[dict], list[dict]]:
    d = json.loads(fetch("https://www.ideandorra.ad/Serveis/wms_cultura/ows", H / "ad" / "immobles_BIC_BI.geojson", {
        "SERVICE": "WFS", "VERSION": "2.0.0", "REQUEST": "GetFeature", "TYPENAMES": "wms_cultura:immobles_BIC_BI",
        "OUTPUTFORMAT": "application/json", "SRSNAME": "EPSG:4326"}).read_text())
    pts = []
    for ft in d["features"]:
        p, g = ft["properties"], ft["geometry"]
        if not g:
            continue
        c = shape(g).representative_point()
        bic = "primera" in (p.get("Seccio") or "").lower()
        # EPSG:4326 in WFS 2.0 may come back lat,lon: Andorra is at ~42.5 N, 1.5 E.
        lon, lat = (c.x, c.y) if c.y > 40 else (c.y, c.x)
        pts.append(site(lon, lat, name=p.get("Nom"), level=2 if bic else 3,
                        designation="Bé d'interès cultural" if bic else "Bé inventariat",
                        category=p.get("Classific") or p.get("Tipus_be"), municipality=p.get("Parroquia"),
                        authority="Govern d'Andorra",
                        url="https://www.govern.ad/ca/tematiques/cultura-i-esports/patrimoni-cultural/coneixer-el-patrimoni-cultural/bens-immobles",
                        source=AD_SRC))
    return pts, []


# ---- United Kingdom ------------------------------------------------------------------------------

EN_SRC = "© Historic England, National Heritage List for England (OGL v3); contains Ordnance Survey data © Crown copyright and database right"
NHLE = "https://services-eu1.arcgis.com/ZOdPfBS3aqqDYPUQ/arcgis/rest/services/National_Heritage_List_for_England_NHLE_v02_VIEW/FeatureServer"
EN_URL = "https://historicengland.org.uk/listing/the-list/list-entry/{}"


def year(v) -> str | None:
    """A designation date from an ArcGIS date (ms) or text."""
    if v in (None, "", 0):
        return None
    if isinstance(v, (int, float)):
        from datetime import datetime, timezone
        try:
            return datetime.fromtimestamp(v / 1000, timezone.utc).strftime("%Y-%m-%d")
        except (OverflowError, OSError, ValueError):
            return None
    return str(v)[:10]


def england() -> tuple[list[dict], list[dict]]:
    pts, areas = [], []
    lb = arcgis(f"{NHLE}/0", "Grade IN ('I','II*')", "ListEntry,Name,Grade,ListDate", H / "uk" / "en-listed.json",
                page=10000, extra={"maxRecordCountFactor": 5})
    for f in lb:
        p, c = f["properties"], rep_point(f["geometry"])
        if c:
            pts.append(site(*c, name=p["Name"], level=2 if p["Grade"] == "I" else 3, designation=f"Listed building, Grade {p['Grade']}",
                            date=year(p.get("ListDate")), authority="Historic England", url=EN_URL.format(p["ListEntry"]), source=EN_SRC))
    sm = arcgis(f"{NHLE}/6", "1=1", "ListEntry,Name,SchedDate", H / "uk" / "en-scheduled.json", page=2000)
    for f in sm:
        p, c = f["properties"], rep_point(f["geometry"])
        if c:
            pts.append(site(*c, name=p["Name"], level=3, designation="Scheduled monument", date=year(p.get("SchedDate")),
                            authority="Historic England", url=EN_URL.format(p["ListEntry"]), source=EN_SRC))
    for layer, cache, desig, datef in ((7, "en-parks.json", "Registered park and garden", "RegDate"),
                                        (8, "en-battlefields.json", "Registered battlefield", "RegDate")):
        for f in arcgis(f"{NHLE}/{layer}", "1=1", f"ListEntry,Name,{'Grade,' if layer == 7 else ''}{datef}", H / "uk" / cache):
            p, c = f["properties"], rep_point(f["geometry"])
            if not c:
                continue
            g = p.get("Grade")
            props = dict(name=p["Name"], level=2 if g == "I" else 3, designation=desig + (f", Grade {g}" if g else ""),
                         date=year(p.get(datef)), authority="Historic England", url=EN_URL.format(p["ListEntry"]), source=EN_SRC)
            pts.append(site(*c, **props))
            areas.append(area(f["geometry"], **props))
    return pts, areas


SC_SRC = ("Contains Historic Environment Scotland and Ordnance Survey data © Historic Environment Scotland - Scottish Charity "
          "No. SC045925 © Crown copyright and database right (OGL v3)")
HES = "https://inspire.hes.scot/arcgis/rest/services/HES/HES_Designations/MapServer"
SC_URL = "https://portal.historicenvironment.scot/designation/{}"


def hes_layer(layer: int, where: str, fields: str, cache: str, step: int = 10000) -> list[dict]:
    """HES layers allow 10,000 records per request and no offset paging: page by FID (polygon
    layers in smaller, simplified pages)."""
    path = H / "uk" / cache
    if path.exists() and path.stat().st_size > 0:
        return json.loads(path.read_text())
    feats, lo = [], 0
    while True:
        q = {"where": f"({where}) AND FID >= {lo} AND FID < {lo + step}", "outFields": fields, "outSR": 4326, "f": "geojson",
             "maxAllowableOffset": 0.0002, "geometryPrecision": 6}
        with urllib.request.urlopen(urllib.request.Request(f"{HES}/{layer}/query?{urllib.parse.urlencode(q)}", headers=UA), timeout=600) as r:
            got = json.loads(r.read()).get("features", [])
        feats += got
        # Stop past the last FID (a range with nothing in it after one that had features).
        q2 = {"where": f"FID >= {lo + step}", "returnCountOnly": "true", "f": "json"}
        with urllib.request.urlopen(urllib.request.Request(f"{HES}/{layer}/query?{urllib.parse.urlencode(q2)}", headers=UA), timeout=600) as r:
            left = json.loads(r.read()).get("count", 0)
        print(f"  HES layer {layer}: {len(feats)}", file=sys.stderr)
        if not left:
            break
        lo += step
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(feats))
    return feats


def scotland() -> tuple[list[dict], list[dict]]:
    pts, areas, seen = [], [], set()
    for f in hes_layer(0, "CATEGORY IN ('A','B')", "DES_REF,DES_TITLE,CATEGORY,DESIGNATED", "sc-listed.json"):
        p, c = f["properties"], rep_point(f["geometry"])
        if not c or p["DES_REF"] in seen:
            continue  # one designation can have several points
        seen.add(p["DES_REF"])
        pts.append(site(*c, name=p["DES_TITLE"], level=2 if p["CATEGORY"] == "A" else 3,
                        designation=f"Listed building, Category {p['CATEGORY']}", date=year(p.get("DESIGNATED")),
                        authority="Historic Environment Scotland", url=SC_URL.format(p["DES_REF"]), source=SC_SRC))
    for layer, cache, desig, is_area in ((5, "sc-scheduled.json", "Scheduled monument", False),
                                         (4, "sc-gardens.json", "Garden and designed landscape", True),
                                         (3, "sc-battlefields.json", "Inventory battlefield", True)):
        for f in hes_layer(layer, "1=1", "DES_REF,DES_TITLE,DESIGNATED", cache, step=2000):
            p, c = f["properties"], rep_point(f["geometry"])
            if not c:
                continue
            props = dict(name=p["DES_TITLE"], level=3, designation=desig, date=year(p.get("DESIGNATED")),
                         authority="Historic Environment Scotland", url=SC_URL.format(p["DES_REF"]), source=SC_SRC)
            pts.append(site(*c, **props))
            if is_area:
                areas.append(area(f["geometry"], **props))
    return pts, areas


WA_SRC = "Designated Historic Asset GIS Data, The Welsh Historic Environment Service (Cadw), licensed under the Open Government Licence v3"
DMW = "https://datamap.gov.wales/geoserver/ows"
WA_URL = "https://cadwpublic-api.azurewebsites.net/reports/{}/FullReport?lang=en&id={}"


def wales() -> tuple[list[dict], list[dict]]:
    def wfs(layer, cache):
        return json.loads(fetch(DMW, H / "uk" / cache, {"service": "WFS", "version": "2.0.0", "request": "GetFeature",
                                                          "typeNames": layer, "outputFormat": "application/json",
                                                          "srsName": "EPSG:4326"}).read_text())["features"]

    def ll(c):
        # WFS 2.0 with EPSG:4326 may answer lat,lon: Wales is at ~52 N, 4 W.
        return (c[1], c[0]) if c[0] > 40 else c
    pts, areas = [], []
    for f in wfs("inspire-wg:Cadw_ListedBuildings", "wa-listed.geojson"):
        p, c = f["properties"], rep_point(f["geometry"])
        g = (p.get("Grade") or "").strip()
        if not c or g not in ("I", "II*"):
            continue
        pts.append(site(*ll(c), name=(p.get("Name") or "").strip(), level=2 if g == "I" else 3, designation=f"Listed building, Grade {g}",
                        date=year(p.get("DesignationDate")), authority="Cadw", url=WA_URL.format("listedbuilding", p["RecordNumber"]), source=WA_SRC))
    for f in wfs("inspire-wg:Cadw_SAM", "wa-scheduled.geojson"):
        p, c = f["properties"], rep_point(f["geometry"])
        if c:
            pts.append(site(*ll(c), name=(p.get("Name") or "").strip(), level=3, designation="Scheduled monument",
                            date=year(p.get("DesignationDate")), authority="Cadw",
                            url=WA_URL.format("sam", (p.get("SAMNumber") or "").strip()), source=WA_SRC))
    for f in wfs("geonode:cadw_rhpg_registeredareas", "wa-parks.geojson"):
        p, c = f["properties"], rep_point(f["geometry"])
        if not c:
            continue
        g = (p.get("grade_gradd") or "").strip()
        props = dict(name=(p.get("site_name_en") or "").strip(), level=2 if g == "I" else 3,
                     designation="Registered historic park and garden" + (f", Grade {g}" if g else ""), date=year(p.get("designation_date")),
                     authority="Cadw", url=WA_URL.format("parkgarden", p.get("reference_number")), source=WA_SRC)
        pts.append(site(*ll(c), **props))
        if c[0] < 40:  # geometry already lon,lat
            areas.append(area(f["geometry"], **props))
    return pts, areas


NI_SRC = "Department for Communities, Historic Environment Division (NI). Contains public sector information licensed under the Open Government Licence v3.0"
HED = "https://services2.arcgis.com/BdBkthNLO9mzGAMO/arcgis/rest/services/Historic_Environment_Division_GIS_Data/FeatureServer"


def northern_ireland() -> tuple[list[dict], list[dict]]:
    pts, areas = [], []
    for f in arcgis(f"{HED}/1", "CurrentGra IN ('A','B+')", "HB_ref,CurrentGra,Address,Townland,MainID", H / "uk" / "ni-listed.json"):
        p, c = f["properties"], rep_point(f["geometry"])
        if not c:
            continue
        name = (p.get("Address") or "").replace("\r", "").split("\n")[0].strip() or p.get("HB_ref")
        pts.append(site(*c, name=name, level=2 if p["CurrentGra"] == "A" else 3, designation=f"Listed building, Grade {p['CurrentGra']}",
                        municipality=p.get("Townland"), authority="Historic Environment Division (NI)",
                        url=f"https://apps.communities-ni.gov.uk/Buildings/buildview.aspx?id={p['MainID']}", source=NI_SRC))
    for f in arcgis(f"{HED}/0", "Protection LIKE '%Scheduled%' OR Protection LIKE '%State Care%'", "MONID,SMRNo,Edited_Type,Protection,Townland_s_",
                    H / "uk" / "ni-monuments.json"):
        p, c = f["properties"], rep_point(f["geometry"])
        if not c:
            continue
        care = "State Care" in (p.get("Protection") or "")
        town = (p.get("Townland_s_") or "").strip()
        pts.append(site(*c, name=(p.get("Edited_Type") or "Monument").strip() + (f", {town}" if town else ""),
                        level=3, designation="State care monument" if care else "Scheduled monument",
                        authority="Historic Environment Division (NI)",
                        url=f"https://apps.communities-ni.gov.uk/NISMR-public/Details.aspx?MonID={p['MONID']}", source=NI_SRC))
    return pts, areas


GG_SRC = "States of Guernsey Development & Planning Authority (gov.gg terms: research and private use)"
GG = "https://services-eu1.arcgis.com/2WsiAmk95geSmFqr/arcgis/rest/services"


def guernsey() -> tuple[list[dict], list[dict]]:
    pts = []
    for f in arcgis(f"{GG}/Guernsey_Protected_Buildings/FeatureServer/0", "1=1", "*", H / "gg" / "buildings.json"):
        p, c = f["properties"], rep_point(f["geometry"])
        if not c:
            continue
        g = (p.get("Grade") or "").strip()
        pts.append(site(*c, name=f"Protected building {p.get('PB') or p.get('RefNo') or ''}".strip(), level=2 if g.startswith("A") else 3,
                        designation="Protected building" + (f", Grade {g}" if g else ""), authority="States of Guernsey",
                        url=p.get("WebAddress"), source=GG_SRC))
    for f in arcgis(f"{GG}/Guernsey_Listed_Monuments/FeatureServer/0", "1=1", "*", H / "gg" / "monuments.json"):
        p, c = f["properties"], rep_point(f["geometry"])
        if c:
            pts.append(site(*c, name=f"Protected monument {p.get('Refno') or ''}".strip(), level=2, designation="Protected monument",
                            authority="States of Guernsey", url=p.get("WebLink"), source=GG_SRC))
    return pts, []


SOURCES = [
    ("UNESCO World Heritage (all)", unesco),
    ("France: Mérimée + SPR", france),
    ("Andorra: inventari", andorra),
    ("England: NHLE", england),
    ("Scotland: HES", scotland),
    ("Wales: Cadw", wales),
    ("Northern Ireland: HED", northern_ireland),
    ("Guernsey", guernsey),
]


# ---- helpers for projected sources -------------------------------------------------------------

_tr: dict[int, Transformer] = {}


def to_ll(epsg: int, x: float, y: float) -> tuple[float, float]:
    """Projected (EPSG) → lon, lat."""
    if epsg not in _tr:
        _tr[epsg] = Transformer.from_crs(epsg, 4326, always_xy=True)
    return _tr[epsg].transform(x, y)


def ll_order(c: tuple[float, float], lat_range: tuple[float, float]) -> tuple[float, float]:
    """WFS 2.0 with EPSG:4326 may answer lat,lon: swap when the first value looks like a latitude."""
    return (c[1], c[0]) if lat_range[0] <= c[0] <= lat_range[1] and not (lat_range[0] <= c[1] <= lat_range[1]) else c


def wfs_json(url: str, layer: str, cache: Path, srs: str = "EPSG:4326", extra: dict | None = None) -> list[dict]:
    return json.loads(fetch(url, cache, {"service": "WFS", "version": "2.0.0", "request": "GetFeature", "typeNames": layer,
                                         "outputFormat": "application/json", "srsName": srs, **(extra or {})}).read_text())["features"]


def read_shp_points(zip_path: Path) -> list[tuple[float, float, dict]]:
    """Points and attributes of the (first) point shapefile in a zip: a minimal reader."""
    import struct
    with zipfile.ZipFile(zip_path) as z:
        names = z.namelist()
        shp = z.read(next(n for n in names if n.lower().endswith(".shp")))
        dbf = z.read(next(n for n in names if n.lower().endswith(".dbf")))
    pts = []
    pos = 100
    while pos + 8 <= len(shp):
        clen = struct.unpack(">i", shp[pos + 4:pos + 8])[0] * 2
        rec = shp[pos + 8:pos + 8 + clen]
        pos += 8 + clen
        st = struct.unpack("<i", rec[:4])[0]
        pts.append(struct.unpack("<2d", rec[4:20]) if st in (1, 11, 21) else None)
    n, hlen, rlen = struct.unpack("<IHH", dbf[4:12])
    fields, off = [], 32
    while dbf[off] != 0x0D:
        name = dbf[off:off + 11].split(b"\0")[0].decode("latin-1")
        fields.append((name, dbf[off + 16]))
        off += 32
    out = []
    for i in range(n):
        r = dbf[hlen + i * rlen + 1: hlen + (i + 1) * rlen]
        props, o = {}, 0
        for name, ln in fields:
            raw = r[o:o + ln]
            try:
                props[name] = raw.decode("utf-8").strip()
            except UnicodeDecodeError:
                props[name] = raw.decode("latin-1").strip()
            o += ln
        if i < len(pts) and pts[i]:
            out.append((pts[i][0], pts[i][1], props))
    return out


def read_ods(path: Path) -> list[list[str]]:
    """Rows of the first sheet of an OpenDocument spreadsheet (text values)."""
    ns = {"table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0", "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0"}
    with zipfile.ZipFile(path) as z:
        root = ET.fromstring(z.read("content.xml"))
    sheet = root.find(".//table:table", ns)
    rows = []
    for tr in sheet.iter(f"{{{ns['table']}}}table-row"):
        row = []
        for tc in tr:
            if not tc.tag.endswith("table-cell") and not tc.tag.endswith("covered-table-cell"):
                continue
            rep = int(tc.get(f"{{{ns['table']}}}number-columns-repeated", "1"))
            txt = " ".join("".join(p.itertext()) for p in tc.findall("text:p", ns)).strip()
            row.extend([txt] * min(rep, 64))
        if any(row):
            rows.append(row)
    return rows


def dmy(text: str | None) -> str | None:
    """First dd-mm-yyyy / dd/mm/yyyy date in a text, as yyyy-mm-dd."""
    m = re.search(r"(\d{1,2})[-/.](\d{1,2})[-/.](\d{4})", text or "")
    return f"{m.group(3)}-{int(m.group(2)):02d}-{int(m.group(1)):02d}" if m else None


# ---- Ireland ------------------------------------------------------------------------------------

IE_NIAH_SRC = "National Inventory of Architectural Heritage, Department of Housing, Local Government and Heritage (CC BY 4.0)"
IE_SMR_SRC = "National Monuments Service, Archaeological Survey of Ireland (CC BY 4.0)"
IE = "https://services-eu1.arcgis.com/HyjXgkV6KGMSF3jt/arcgis/rest/services"


def slug(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", (s or "x").lower()).strip("-") or "x"


def ireland() -> tuple[list[dict], list[dict]]:
    pts = []
    for f in arcgis(f"{IE}/NIAHBuildingsOpenData/FeatureServer/0", "RATING IN ('International','National')",
                    "REG_NO,NAME,RATING,DATEFROM,DATETO,TOWN,COUNTY,ORIGINAL_TYPE", H / "ie" / "niah-national.json"):
        p, c = f["properties"], rep_point(f["geometry"])
        if not c:
            continue
        built = "–".join(str(v) for v in dict.fromkeys([p.get("DATEFROM"), p.get("DATETO")]) if v)
        pts.append(site(*c, name=p.get("NAME") or p.get("ORIGINAL_TYPE"), level=2, designation=f"NIAH, {p['RATING']} rating",
                        municipality=p.get("TOWN") or p.get("COUNTY"), category=p.get("ORIGINAL_TYPE"), built=built or None,
                        authority="National Inventory of Architectural Heritage",
                        url=f"https://www.buildingsofireland.ie/buildings-search/building/{p['REG_NO']}/{slug(p.get('NAME'))}", source=IE_NIAH_SRC))
    where = "ITM_E > 0 AND (WEB_NOTES LIKE '%State care%' OR WEB_NOTES LIKE '%State Care%' OR WEB_NOTES LIKE '%Preservation Order%' OR WEB_NOTES LIKE '%preservation order%')"
    for f in arcgis(f"{IE}/SMROpenData/FeatureServer/0", where, "SMRS,MONUMENT_CLASS,TOWNLAND,COUNTY,WEB_NOTES,WEBSITE_LINK", H / "ie" / "smr-protected.json"):
        p, c = f["properties"], rep_point(f["geometry"])
        if not c:
            continue
        notes = p.get("WEB_NOTES") or ""
        care = "state care" in notes.lower()
        town = (p.get("TOWNLAND") or "").title()
        pts.append(site(*c, name=(p.get("MONUMENT_CLASS") or "Monument") + (f", {town}" if town else ""), level=3,
                        designation="National Monument in State care" if care else "Monument under a preservation order",
                        municipality=(p.get("COUNTY") or "").title() or None, category=p.get("MONUMENT_CLASS"),
                        authority="National Monuments Service", url=p.get("WEBSITE_LINK"), source=IE_SMR_SRC))
    return pts, []


# ---- Spain --------------------------------------------------------------------------------------

def es_site(lon, lat, name, cat, src, url=None, date=None, region=None, level=2, designation="Bien de interés cultural", municipality=None):
    return site(lon, lat, name=name, level=level, designation=designation + (f" ({cat})" if cat else ""), category=cat, date=date,
                municipality=municipality, authority=region, url=url, source=src)


def spain() -> tuple[list[dict], list[dict]]:
    pts: list[dict] = []
    ok = lambda lon, lat: -18.5 < lon < 4.5 and 27.5 < lat < 44.0  # noqa: E731 (mainland, Balearics, Canaries)

    # Catalonia: BCIN (bé cultural d'interès nacional = BIC), architectural and archaeological.
    for layer in ("PATRIMONI_BCIN_ARQUITEC", "PATRIMONI_BCIN_ARQUEOPALEO"):
        try:
            feats = wfs_json("https://sig.gencat.cat/ows/PATRIMONI_CULTURAL/wfs", layer, H / "es" / f"cat-{layer.lower()}.geojson")
        except Exception as e:  # noqa: BLE001
            print(f"  Catalonia {layer}: {e}", file=sys.stderr)
            continue
        for f in feats:
            p, c = f["properties"], rep_point(f["geometry"])
            if not c:
                continue
            lon, lat = ll_order(c, (40.4, 42.95))
            if not ok(lon, lat):
                continue
            code = p.get("CODI_INVENTARI")
            pts.append(es_site(lon, lat, p.get("NOM"), p.get("TIPOLOGIA"), "Generalitat de Catalunya, Inventari del Patrimoni Cultural",
                               url=f"https://invarquit.cultura.gencat.cat/card/{code}" if code else None, region="Generalitat de Catalunya",
                               designation="Bé cultural d'interès nacional"))
    # Castilla y León.
    try:
        for f in wfs_json("https://idecyl.jcyl.es/geoserver/patrimoniocultural/wfs", "patrimoniocultural:pacu_cyl_bic_inmu_vw", H / "es" / "cyl-bic.geojson"):
            p, c = f["properties"], rep_point(f["geometry"])
            if not c:
                continue
            lon, lat = ll_order(c, (40.0, 43.3))
            if ok(lon, lat):
                pts.append(es_site(lon, lat, p.get("d_bien_denom"), p.get("d_categ_adquiere"), "Junta de Castilla y León", url=p.get("l_url_pweb"),
                                   date=year(p.get("f_bien_protec")), region="Junta de Castilla y León"))
    except Exception as e:  # noqa: BLE001
        print(f"  Castilla y León: {e}", file=sys.stderr)
    # Aragón (points, ETRS89 / UTM 30N).
    try:
        d = json.loads(fetch("https://opendata.aragon.es/GA_OD_Core/download?resource_id=333&formato=json", H / "es" / "aragon-bic.json").read_text())
        rows = d if isinstance(d, list) else d.get("features") or d.get("result") or []
        for r in rows:
            p = r.get("properties", r) if isinstance(r, dict) else {}
            g = r.get("geometry") if isinstance(r, dict) else None
            try:
                if g and g.get("coordinates"):
                    x, y = g["coordinates"][:2]
                else:
                    x, y = float(p.get("x") or p.get("X") or p.get("coord_x")), float(p.get("y") or p.get("Y") or p.get("coord_y"))
            except (TypeError, ValueError):
                continue
            lon, lat = (x, y) if abs(x) <= 180 else to_ll(25830, x, y)
            if ok(lon, lat):
                pts.append(es_site(lon, lat, p.get("denominaci") or p.get("DENOMINACI"), p.get("categoria") or p.get("CATEGORIA"),
                                   "Gobierno de Aragón (Aragón Open Data)", date=dmy(str(p.get("resolucion") or "")), region="Gobierno de Aragón"))
    except Exception as e:  # noqa: BLE001
        print(f"  Aragón: {e}", file=sys.stderr)
    # Comunitat Valenciana (CSV with WKT points, ETRS89 / UTM 30N).
    try:
        path = fetch("https://terramapas.icv.gva.es/22_IGPCV_wfs?request=GetFeature&service=WFS&version=2.0.0&typename=BIC&outputformat=csv",
                     H / "es" / "valencia-bic.csv")
        text = path.read_bytes().decode("utf-8", "replace")
        for r in csv.DictReader(io.StringIO(text)):
            geom = next((v for k, v in r.items() if v and v.startswith("POINT")), None)
            m = re.search(r"POINT\s*\(\s*([-\d.]+)\s+([-\d.]+)", geom or "")
            if not m:
                continue
            x, y = float(m.group(1)), float(m.group(2))
            lon, lat = (x, y) if abs(x) <= 180 else to_ll(25830, x, y)
            if ok(lon, lat):
                pts.append(es_site(lon, lat, r.get("denominacion"), r.get("categoria"), "Generalitat Valenciana, Inventario General del Patrimonio Cultural Valenciano",
                                   region="Generalitat Valenciana"))
    except Exception as e:  # noqa: BLE001
        print(f"  Valencia: {e}", file=sys.stderr)
    # Galicia (ODS, UTM 29N with dots as thousands separators).
    try:
        rows = read_ods(fetch("https://ficheiros-web.xunta.gal/patrimonio/bic/relacion_bics.ods", H / "es" / "galicia-bic.ods"))
        head_i = next(i for i, r in enumerate(rows) if any("X" == c.strip().upper() for c in r))
        head = [c.strip().upper() for c in rows[head_i]]
        col = lambda *names: next((head.index(n) for n in names if n in head), None)  # noqa: E731
        ix, iy, iname, icat, idate, irage = col("X"), col("Y"), col("DENOMINACIÓN", "DENOMINACION", "BEN", "NOME"), col("CATEGORIA", "CATEGORÍA"), col("DATA"), col("ID_RAGE")
        for r in rows[head_i + 1:]:
            try:
                x, y = float(r[ix].replace(".", "").replace(",", ".")), float(r[iy].replace(".", "").replace(",", "."))
            except (ValueError, IndexError, TypeError):
                continue
            lon, lat = to_ll(25829, x, y)
            if ok(lon, lat) and 41.7 < lat < 43.9:
                pts.append(es_site(lon, lat, r[iname] if iname is not None and iname < len(r) else None,
                                   r[icat] if icat is not None and icat < len(r) else None, "Xunta de Galicia (CC BY-SA 4.0)",
                                   date=dmy(r[idate]) if idate is not None and idate < len(r) else None, region="Xunta de Galicia"))
    except Exception as e:  # noqa: BLE001
        print(f"  Galicia: {e}", file=sys.stderr)
    # Navarra (point shapefiles, UTM 30N).
    for kind in ("BICarquite", "BICarqueo"):
        try:
            for x, y, p in read_shp_points(fetch(f"https://idena.navarra.es/descargas/PATRIM_Sym_{kind}.zip", H / "es" / f"navarra-{kind}.zip")):
                lon, lat = to_ll(25830, x, y)
                if ok(lon, lat):
                    pts.append(es_site(lon, lat, p.get("BIC") or p.get("DENOMINACI"), p.get("TIPO"), "Gobierno de Navarra, IDENA (CC BY 4.0)",
                                       region="Gobierno de Navarra"))
        except Exception as e:  # noqa: BLE001
            print(f"  Navarra {kind}: {e}", file=sys.stderr)
    # Extremadura (xlsx, UTM 30N or 29N: whichever lands in the region).
    try:
        import openpyxl
        wb = openpyxl.load_workbook(fetch("https://www.juntaex.es/documents/77055/5801338/Bienes_Interes_Cultural.xlsx", H / "es" / "extremadura-bic.xlsx"), read_only=True)
        ws = wb.worksheets[0]
        rows = [[("" if v is None else str(v)) for v in r] for r in ws.iter_rows(values_only=True)]
        head_i = next(i for i, r in enumerate(rows) if any(c.strip().upper() in ("X", "COORD X", "COORDENADA X", "UTM X") for c in r))
        head = [c.strip().upper() for c in rows[head_i]]
        find = lambda pred: next((i for i, c in enumerate(head) if pred(c)), None)  # noqa: E731
        ix, iy = find(lambda c: c in ("X", "COORD X", "COORDENADA X", "UTM X")), find(lambda c: c in ("Y", "COORD Y", "COORDENADA Y", "UTM Y"))
        iname = find(lambda c: "DENOMINA" in c or c == "BIEN" or "NOMBRE" in c)
        icat, idate = find(lambda c: "CATEGOR" in c), find(lambda c: "FECHA" in c)
        for r in rows[head_i + 1:]:
            try:
                x, y = float(r[ix].replace(",", ".")), float(r[iy].replace(",", "."))
            except (ValueError, IndexError, TypeError):
                continue
            for epsg in (25830, 25829):
                lon, lat = to_ll(epsg, x, y)
                if 37.9 < lat < 40.6 and -7.6 < lon < -4.6:
                    pts.append(es_site(lon, lat, r[iname] if iname is not None else None, r[icat] if icat is not None else None,
                                       "Junta de Extremadura", date=(r[idate][:10] if idate is not None and r[idate] else None), region="Junta de Extremadura"))
                    break
    except Exception as e:  # noqa: BLE001
        print(f"  Extremadura: {e}", file=sys.stderr)
    # Andalucía: protected immovable heritage (IAPH localizador; BIC and catalogued alike).
    try:
        for f in wfs_json("https://www.iaph.es/ide/localizador/wfs", "localizador:localizador", H / "es" / "andalucia-iaph.geojson",
                          extra={"CQL_FILTER": "PROTEGIDO='SI'"}):
            p, c = f["properties"], rep_point(f["geometry"])
            if not c:
                continue
            lon, lat = ll_order(c, (35.9, 38.8))
            if abs(lon) > 180:
                lon, lat = to_ll(25830, *c)
            if ok(lon, lat):
                name = p.get("DENOMINACION") or p.get("denominacion") or p.get("NOMBRE") or p.get("nombre")
                pts.append(es_site(lon, lat, name, p.get("TIPOLOGIA") or p.get("tipologia"), "Instituto Andaluz del Patrimonio Histórico (IAPH)",
                                   level=3, designation="Patrimonio protegido de Andalucía", region="Junta de Andalucía",
                                   municipality=p.get("MUNICIPIO") or p.get("municipio")))
    except Exception as e:  # noqa: BLE001
        print(f"  Andalucía: {e}", file=sys.stderr)
    return pts, []


# ---- Portugal -----------------------------------------------------------------------------------

PT_SRC = "Património Cultural, I.P., Atlas do Património Classificado e em Vias de Classificação (CC BY-NC 4.0)"


def portugal() -> tuple[list[dict], list[dict]]:
    feats = arcgis("https://services8.arcgis.com/ITVSIrZ4rxt6SbBo/arcgis/rest/services/Atlas_Patrimonio/FeatureServer/1",
                   "Situacao_A = 'Classificado'", "NINV,Designacao,Categoria,Classifica,Descricao,Concelho", H / "pt" / "atlas-classificado.json",
                   page=1000, extra={"maxAllowableOffset": 0.00005})
    best: dict[str, tuple[float, dict, dict]] = {}
    for f in feats:
        g = f["geometry"]
        if not g:
            continue
        a = shape(g).area
        k = str(f["properties"].get("NINV"))
        if k not in best or a > best[k][0]:
            best[k] = (a, f["properties"], g)
    pts = []
    for _, p, g in best.values():
        c = rep_point(g)
        cat = (p.get("Categoria") or "").strip()
        code = cat.split(" ")[0].split("/")[0].strip().upper()
        level = 2 if code == "MN" else 5 if code in ("IM", "MIM", "CIM", "SIM") else 3
        url = p.get("Descricao") or ""
        m = re.search(r'href="([^"]+)"', url)
        pts.append(site(*c, name=p.get("Designacao"), level=level, designation=cat or "Imóvel classificado",
                        date=dmy(p.get("Classifica")), municipality=p.get("Concelho"), authority="Património Cultural, I.P.",
                        url=(m.group(1) if m else url) or None, source=PT_SRC))
    return pts, []


# ---- Hong Kong ----------------------------------------------------------------------------------

HK_SRC = "Antiquities and Monuments Office, via the Common Spatial Data Infrastructure (CSDI) Portal (DATA.GOV.HK terms)"
HK = "https://portal.csdi.gov.hk/server/rest/services/common"


def hong_kong() -> tuple[list[dict], list[dict]]:
    pts = []
    seen = set()
    for f in arcgis(f"{HK}/devb_wb_rcd_1639040299687_46105/FeatureServer/0", "1=1", "*", H / "hk" / "declared-monuments.json", page=3000):
        p, c = f["properties"], rep_point(f["geometry"])
        key = p.get("DETAIL") or p.get("FILE_REF")
        if not c or key in seen:
            continue  # one declaration can have several parts
        seen.add(key)
        name = " ".join(x for x in (p.get("NAME_TC"), p.get("NAME")) if x)
        pts.append(site(*c, name=name, level=2, designation="Declared monument", date=str(p["DEC_YEAR"]) if p.get("DEC_YEAR") else None,
                        municipality=p.get("DISTRICT"), authority="Antiquities and Monuments Office", url=p.get("DETAIL"), source=HK_SRC))
    for f in arcgis(f"{HK}/devb_wb_rcd_1639040388709_10687/FeatureServer/0", "1=1", "*", H / "hk" / "graded-buildings.json", page=3000):
        p, c = f["properties"], rep_point(f["geometry"])
        g = (p.get("GRADE") or "").strip()
        k = {"Grade 1": 3, "Grade 2": 4, "Grade 3": 5}.get(g)
        if not c or not k:
            continue
        name = " ".join(x for x in (p.get("NAME_TC"), p.get("NAME")) if x)
        pts.append(site(*c, name=name, level=k, designation=f"Graded historic building, {g}", date=str(p["GRADE_YEAR"]) if p.get("GRADE_YEAR") else None,
                        municipality=p.get("ADDRESS"), authority="Antiquities Advisory Board", source=HK_SRC))
    return pts, []


SOURCES += [
    ("Ireland: NIAH + SMR", ireland),
    ("Spain: regional BIC registers", spain),
    ("Portugal: Atlas do Património", portugal),
    ("Hong Kong: AMO", hong_kong),
]
