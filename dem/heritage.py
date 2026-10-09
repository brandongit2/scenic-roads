#!/usr/bin/env python3
"""Officially designated places and areas, from the designating authorities wherever they
publish usable data.

Heritage sites (heritage.json, points) — level 1 World Heritage · 2 national · 3 national
register · 4 provincial/state · 5 municipal:
  1  UNESCO World Heritage List (data.unesco.org dataset whc001, CC BY-SA 4.0; one point per
     component site; descriptions not reproduced)
  2  Canada: Parks Canada Directory of Federal Heritage Designations, located by federal.py
     (National Historic Sites, heritage lighthouses & railway stations, federal heritage
     buildings); US: National Historic Landmarks (NPS)
  3  US: National Register of Historic Places (NPS)
  4  Quebec: Répertoire du patrimoine culturel (MCC) classified / declared / national;
     Ontario: Ontario Heritage Act Register (Ontario Heritage Trust), ministerial decisions;
     Nova Scotia: Registered Heritage Properties; NB, PEI, NL: Canadian Register of Historic
     Places (crhp.py)
  5  municipal designations from the same sources, plus Halifax (HRM) and Moncton open data
  Outside North America, the national registers in heritage_eu.py (France, Andorra, …), with
  the same levels: 2 highest national grade, 3 other national grades, 4 regional, 5 local.
Heritage areas (heritage-areas.json, polygons): Quebec heritage-site perimeters, Ontario
  heritage conservation districts.
Special areas (special.json): UNESCO biosphere reserves and Global Geoparks, dark-sky places —
  official registries (special-official.json) with OSM boundaries where mapped; Wikidata only
  if that file is missing.
Protected areas and Indigenous lands come from OSM (data/areas/areas.geojsonseq).

All areas are rasterised into grid.areas.u8 bits (roadcore::scenic::flag): PARK, HERITAGE,
SPECIAL_AREA, INDIGENOUS. Afterwards run `scenic <build> flags` to refresh the road flags.

With --tiles <file> --zoom <z> (the `heritage-sites` job, docs/phase5.md "Heritage and area
flags"): what is covered is that file's tiles (uint32 x, y pairs at zoom z: those within 20 km of
the coverage), not the build's analysis grid (grid.idx, zoom 11), and the areas aren't rasterised:
their polygons, each with its flag bit, go to area-shapes.geojsonseq for the units to rasterise
onto their own grids (the `areaflags` program). --date stamps heritage-sources.json with the pass's date
instead of the time of the run.

usage: heritage.py <build_dir> [--tiles <tiles.u32> --zoom <z>] [--date YYYY-MM-DD]
"""
from __future__ import annotations

import csv
import io
import json
import math
import os
import re
import sys
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
from rasterio import features
from rasterio.transform import from_bounds
from shapely import STRtree, wkt
from shapely.geometry import Point, box, mapping, shape
from shapely.ops import transform as shp_transform
from tqdm import tqdm

import heritage_eu
from timings import phase

UA = {"User-Agent": "scenic-roads/0.1 (personal offline map)"}
H = Path(__file__).resolve().parent.parent / "data" / "heritage"
NRHP = "https://mapservices.nps.gov/arcgis/rest/services/cultural_resources/nrhp_locations/MapServer/0/query"
STATES = ["NEW YORK", "VERMONT", "NEW HAMPSHIRE", "MAINE", "MASSACHUSETTS", "CONNECTICUT", "RHODE ISLAND"]
PARK, HERITAGE, SPECIAL, INDIGENOUS = 1 << 1, 1 << 4, 1 << 6, 1 << 7
csv.field_size_limit(1 << 24)


def get(url: str, params: dict | None = None, accept: str | None = None) -> bytes:
    if params:
        url += "?" + urllib.parse.urlencode(params)
    h = dict(UA)
    if accept:
        h["Accept"] = accept
    with phase("sources downloaded", "net"):
        for attempt in range(12):
            try:
                with urllib.request.urlopen(urllib.request.Request(url, headers=h), timeout=180) as r:
                    return r.read()
            except urllib.error.HTTPError as e:
                if attempt == 11:
                    raise
                # Respect rate limits (the Wikidata query service may allow only 1 request/min).
                wait = int(e.headers.get("Retry-After") or 0) or (65 if e.code == 429 else 2 ** attempt)
                print(f"  HTTP {e.code}; waiting {wait} s", file=sys.stderr)
                time.sleep(wait)
            except Exception as e:  # noqa: BLE001
                if attempt == 11:
                    raise
                print(f"  retry ({e})", file=sys.stderr)
                time.sleep(2 ** min(attempt, 6))
    raise RuntimeError("unreachable")


def sparql(q: str, cache: str) -> list[dict]:
    """Run a Wikidata query, caching the CSV in data/heritage/ (to ask again, delete the wd-*.csv:
    the build agent's is the pass's copy of the registers' snapshot, cache/heritage-<date>-<id>/ in
    its folder; the next heritage-sites run asks, and the answer goes to the NAS with the pass's,
    pipeline::answers)."""
    path = H / cache
    if path.exists():
        b = path.read_bytes()
    else:
        with phase("sources downloaded", "net"):
            b = get("https://query.wikidata.org/sparql", {"query": q}, accept="text/csv")
            path.write_bytes(b)
            time.sleep(61)  # stay under the query service's rate limit
    return list(csv.DictReader(io.StringIO(b.decode("utf-8"))))


def point(s: str):
    m = re.match(r"Point\(([-\d.eE]+) ([-\d.eE]+)\)", s or "")
    return (float(m.group(1)), float(m.group(2))) if m else None


def site(lon, lat, **props) -> dict:
    return {"type": "Feature", "geometry": {"type": "Point", "coordinates": [round(float(lon), 6), round(float(lat), 6)]},
            "properties": {k: v for k, v in props.items() if v not in (None, "", "None")}}


def title(s: str | None) -> str | None:
    return s.title() if s and s.isupper() else s


def tidy_quotes(s: str | None) -> str | None:
    """Registers' quoting glitches: doubled quotes (CSV escaping kept), quotes around the whole
    name, spaces just inside quotes ('dite " maison Jézéquel "')."""
    if not s or '"' not in s:
        return s
    s = re.sub(r'"{2,}', '"', s).strip()
    if len(s) > 1 and s[0] == s[-1] == '"' and s[1:-1].count('"') % 2 == 0:
        s = s[1:-1].strip()
    elif s[0] == '"' and s.count('"') % 2 == 1:  # an opening quote left unmatched
        s = s[1:].strip()
    parts = s.split('"')
    if len(parts) % 2 == 1:  # balanced: odd parts are quoted
        s = '"'.join(t.strip() if i % 2 else t for i, t in enumerate(parts))
    return s


def iso_date(ms) -> str | None:
    try:
        return datetime.fromtimestamp(int(ms) / 1000, timezone.utc).strftime("%Y-%m-%d") if ms else None
    except (TypeError, ValueError):
        return None


# ---- sources -----------------------------------------------------------------------------------

def federal() -> list[dict]:
    d = json.loads((H / "federal.json").read_text())
    for f in d["features"]:
        p = f["properties"]
        p["authority"] = "Government of Canada"
        if not p.get("location", "").startswith("wikidata") or "paired" in p.get("location", ""):
            p["approx"] = True  # located via OSM or by a looser name match: check before relying on it
    return d["features"]


def nrhp() -> list[dict]:
    path = H / "nrhp.json"
    if not path.exists():
        feats = []
        where = "State IN (" + ",".join(f"'{s}'" for s in STATES) + ")"
        total = json.loads(get(NRHP, {"where": where, "returnCountOnly": "true", "f": "json"}))["count"]
        for off in tqdm(range(0, total, 2000), desc="NPS National Register", unit="page"):
            d = json.loads(get(NRHP, {
                "where": where, "outFields": "RESNAME,ResType,City,State,Is_NHL,CertDate,NRIS_Refnum,NARA_URL",
                "resultOffset": off, "resultRecordCount": 2000, "outSR": 4326, "f": "geojson"}))
            feats += d.get("features", [])
        path.write_text(json.dumps({"type": "FeatureCollection", "features": feats}))
    out = []
    for f in json.loads(path.read_text())["features"]:
        g, p = f.get("geometry"), f["properties"]
        if not g or g.get("type") != "Point":
            continue
        nhl = str(p.get("Is_NHL") or "").strip().lower() in ("y", "yes", "true", "1", "x")
        out.append(site(*g["coordinates"][:2], name=title(p.get("RESNAME")), level=2 if nhl else 3,
                        designation="National Historic Landmark" if nhl else "National Register of Historic Places",
                        type=p.get("ResType"), municipality=title(p.get("City")), date=iso_date(p.get("CertDate")),
                        authority="National Park Service",
                        url=p.get("NARA_URL") or f"https://npgallery.nps.gov/AssetDetail/NRIS/{p.get('NRIS_Refnum')}",
                        source="NPS National Register"))
    return out


QC = H / "qc"
QC_SRC = "Répertoire du patrimoine culturel du Québec (MCC, CC BY 4.0)"


def quebec() -> tuple[list[dict], list[dict]]:
    pts, areas = [], []

    def props(p, level, desig):
        return dict(name=p["nom_bien"], level=level, designation=desig, date=(p.get("date_attri_stat_jurid_princ")
                    or p.get("date_attribution_stat_jurid_principal_actuel") or "")[:10],
                    authority=(p.get("autorite_protection") or p.get("autorite") or "").strip(),
                    municipality=p.get("municipalite"), category=p.get("categorie"),
                    url=p.get("url_rpcq"), source=QC_SRC)

    for fn, desig in (("immeubles-classes-points.geojson", "Immeuble patrimonial classé"),
                      ("sites-patrimoniaux-classes-par-la-ministre-de-la-culture-et-des-communications.geojson", "Site patrimonial classé"),
                      ("sites-patrimoniaux-declares-par-le-gouvernement-du-quebec.geojson", "Site patrimonial déclaré")):
        for f in json.loads((QC / fn).read_text())["features"]:
            p = f["properties"]
            if p["statut_juridique_princ"] not in ("Classement", "Déclaration"):
                continue  # notices of intent are not designations yet
            g = f["geometry"]
            if not g or not g["coordinates"]:
                continue
            c = g["coordinates"][0] if g["type"] == "MultiPoint" else g["coordinates"]
            pts.append(site(*c, **props(p, 4, desig)))
    for fn, desig in (("sites-patrimoniaux-classes-par-la-ministre-de-la-culture-et-des-communications-perimetres.geojson", "Site patrimonial classé"),
                      ("sites-patrimoniaux-declares-par-le-gouvernement-du-quebec-perimetres.geojson", "Site patrimonial déclaré")):
        for f in json.loads((QC / fn).read_text())["features"]:
            p = f["properties"]
            if f["geometry"] and p["statut_juridique_princ"] in ("Classement", "Déclaration"):
                areas.append({"type": "Feature", "geometry": f["geometry"], "properties": props(p, 4, desig)})
    for fn, level, desig in (("site-patrimonial-national-declare-par-la-loi-sur-le-patrimoine-culturel-par-le-gouvernement-du-.csv", 4, "Site patrimonial national"),
                             ("immeubles-patrimoniaux-cites-par-les-municipalites-et-les-communautes-autochtones.csv", 5, "Immeuble patrimonial cité"),
                             ("sites-patrimoniaux-cites-par-les-municipalites-et-les-communautes-autochtones.csv", 5, "Site patrimonial cité")):
        for r in csv.DictReader(open(QC / fn, encoding="utf-8")):
            try:
                lon, lat = float(r["longitude"]), float(r["latitude"])
            except ValueError:
                g = (r.get("geometrie") or "").strip()
                if not g or g.upper() == "NULL":
                    continue
                c = wkt.loads(g).representative_point()
                lon, lat = c.x, c.y
            pts.append(site(lon, lat, **props(r, level, desig)))
    return pts, areas


ON_SRC = "Ontario Heritage Act Register (Ontario Heritage Trust; personal non-commercial use)"


def ontario() -> tuple[list[dict], list[dict]]:
    pts = []
    for f in json.loads((H / "on" / "oht-register.geojson").read_text())["features"]:
        p, g = f["properties"], f["geometry"]
        if p.get("Heritage_Conserv_District"):
            continue  # Part V: represented by its district polygon
        if g:
            lon, lat = g["coordinates"][:2]
        elif p.get("Latitude") and p.get("Longitude"):
            lon, lat = p["Longitude"], p["Latitude"]
        else:
            continue
        prov = p.get("OHASectionName") == "Ministerial Decisions"
        name = p.get("Property_Name") or p.get("Address")
        pts.append(site(lon, lat, name=name, level=4 if prov else 5,
                        designation="Provincial designation (Ontario Heritage Act)" if prov else "Designated under Part IV, Ontario Heritage Act",
                        authority="Minister of Citizenship and Multiculturalism" if prov else p.get("Municipality_Display"),
                        municipality=p.get("Municipality_Display"), address=p.get("Address") if p.get("Address") != name else None,
                        type=p.get("Historical_Function_Type"),
                        built=p.get("Construction_End_Year") or p.get("Construction_Start_Year"),
                        url="https://www.heritagetrust.on.ca/oha/search-the-oha-register", source=ON_SRC))
    areas = []
    for f in json.loads((H / "on" / "hcd.geojson").read_text())["features"]:
        p = f["properties"]
        if not f["geometry"] or (p.get("Status") or "").lower().startswith("under"):
            continue
        areas.append({"type": "Feature", "geometry": f["geometry"], "properties": dict(
            name=p["HCD_Name"], level=5, designation="Heritage Conservation District (Part V)", municipality=p.get("Municipality"),
            authority=p.get("Municipality"), bylaw=p.get("Bylaw_number"), properties_count=p.get("Number_of_Properties"),
            url=p.get("OHA_Register_Link"), source=ON_SRC)})
    return pts, areas


def nova_scotia() -> list[dict]:
    out = []
    for r in csv.DictReader(open(H / "ns" / "registered.csv", encoding="utf-8")):
        try:
            lon, lat = float(r["longitude"]), float(r["latitude"])
        except ValueError:
            continue
        out.append(site(lon, lat, name=r["property_name"], level=4, designation="Provincially Registered Heritage Property",
                        authority="Province of Nova Scotia", municipality=r["community"], type=r["type"],
                        date=r["notice_of_registration_date"][:10], built=r.get("year_built"),
                        url="https://data.novascotia.ca/d/7pnv-7sdm", source="Nova Scotia Registered Heritage Properties (NS Open Government Licence)"))
    for f in json.loads((H / "ns" / "hrm.geojson").read_text())["features"]:
        if not f["geometry"]:
            continue
        p = f["properties"]
        c = shape(f["geometry"]).representative_point()
        out.append(site(c.x, c.y, name=p.get("HRTG_NM") or "Heritage property", level=5,
                        designation="Municipally Registered Heritage Property", authority="Halifax Regional Municipality",
                        municipality="Halifax", built=p.get("HRTG_YEAR"), date=iso_date(p.get("REG_DATE")),
                        url="https://www.halifax.ca/about-halifax/regional-community-planning/heritage-properties-programs",
                        source="Halifax Regional Municipality open data"))
    return out


def new_brunswick_moncton() -> list[dict]:
    out = []
    for f in json.loads((H / "nb" / "moncton-Heritage_Properties.geojson").read_text())["features"]:
        if not f["geometry"]:
            continue
        p = f["properties"]
        c = shape(f["geometry"]).representative_point()
        out.append(site(c.x, c.y, name=title(p.get("LOCATION")) or "Heritage property", level=5,
                        designation="Municipal heritage property (By-law Z-1116)", authority="City of Moncton",
                        municipality="Moncton", url="https://www.moncton.ca", source="City of Moncton open data"))
    return out


def crhp() -> list[dict]:
    path = H / "crhp.json"
    return json.loads(path.read_text())["features"] if path.exists() else []


def dedupe(feats: list[dict], radius_m=40.0) -> list[dict]:
    """Drop municipal points that sit on another source's point of the same or higher level with
    a similar name (e.g. Moncton or HRM properties also listed in the Canadian Register)."""
    def nm(s):
        s = unicodedata.normalize("NFKD", s or "").encode("ascii", "ignore").decode().lower()
        return set(re.findall(r"[a-z0-9]{3,}", s))
    grid = {}
    for i, f in enumerate(feats):
        lon, lat = f["geometry"]["coordinates"]
        grid.setdefault((round(lon, 3), round(lat, 3)), []).append(i)
    drop = set()
    for i, f in enumerate(feats):
        p = f["properties"]
        if p["level"] < 5:
            continue
        lon, lat = f["geometry"]["coordinates"]
        for dx in (-0.001, 0, 0.001):
            for dy in (-0.001, 0, 0.001):
                for j in grid.get((round(lon + dx, 3), round(lat + dy, 3)), []):
                    if j == i or j in drop:
                        continue
                    q = feats[j]["properties"]
                    if q["source"] == p["source"] or q["level"] > p["level"]:
                        continue
                    lo2, la2 = feats[j]["geometry"]["coordinates"]
                    d = math.hypot((lon - lo2) * 111320 * math.cos(math.radians(lat)), (lat - la2) * 110540)
                    if d <= radius_m and (nm(p["name"]) & nm(q["name"])) and j not in drop:
                        drop.add(i)
    return [f for i, f in enumerate(feats) if i not in drop]


# ---- special places ------------------------------------------------------------------------------

def special_wikidata() -> list[dict]:
    kinds = {"Q158454": "biosphere", "Q53444003": "geopark", "Q1324355": "geopark",
             "Q52216504": "dark_sky", "Q72114283": "dark_sky", "Q3457162": "dark_sky"}
    rows = sparql("""
SELECT ?item ?coord ?k (SAMPLE(?en) AS ?name) (SAMPLE(?a) AS ?area) (SAMPLE(?art) AS ?wiki) WHERE {
  VALUES ?k { """ + " ".join("wd:" + k for k in kinds) + """ }
  VALUES ?country { wd:Q16 wd:Q30 }
  { ?item wdt:P31 ?k } UNION { ?item wdt:P1435 ?k }
  ?item wdt:P17 ?country ; wdt:P625 ?coord .
  OPTIONAL { ?item wdt:P2046 ?a }
  OPTIONAL { ?item rdfs:label ?en FILTER(LANG(?en) = "en") }
  OPTIONAL { ?art schema:about ?item ; schema:isPartOf <https://en.wikipedia.org/> }
} GROUP BY ?item ?coord ?k""", "wd-special.csv")
    out = {}
    for r in rows:
        p = point(r["coord"])
        if not p:
            continue
        q = r["item"].rsplit("/", 1)[1]
        try:
            area = float(r["area"]) if r["area"] else None
        except ValueError:
            area = None
        out[q] = {"lon": p[0], "lat": p[1], "kind": kinds[r["k"].rsplit("/", 1)[1]], "name": r["name"],
                  "area_km2": area, "url": r["wiki"] or r["item"], "source": "Wikidata"}
    return list(out.values())


# Local names the registries and Wikidata don't supply.
LOCAL_NAMES = {
    "Regional Natural Park of Vercors": "Parc naturel régional du Vercors",
    "Regional Natural Park of Morvan": "Parc naturel régional du Morvan",
    "Regional Natural Park of Millevaches in Limousin": "Parc naturel régional de Millevaches en Limousin",
    "Graciosa Island Biosphere Reserve": "Reserva da Biosfera da Ilha Graciosa",
    "Lanzarote and Chinijo Islands UNESCO Global Geopark": "Geoparque Mundial UNESCO Lanzarote y Archipiélago Chinijo",
    "Courel Mountains UNESCO Global Geopark": "Xeoparque Mundial UNESCO Montañas do Courel",
    "Calatrava Volcanoes. Ciudad Real UNESCO Global Geopark": "Geoparque Mundial UNESCO Volcanes de Calatrava. Ciudad Real",
}


def special_official() -> list[dict] | None:
    path = H / "special-official.json"
    if not path.exists():
        return None
    d = json.loads(path.read_text())
    out = [{**p, "source": p.get("certifier", "official registry")} for p in d["places"]]
    # Names in the region's language (the registries publish English): the label of the Wikidata
    # biosphere reserve / geopark / dark-sky place nearest it (≤ 80 km) that shares a distinctive
    # word with its name, else of the item a name search finds, when it names the same kind of
    # place. The English name stays the key for matching OSM polygons.
    cache_path = H / "special-wd-labels.json"
    cache = json.loads(cache_path.read_text()) if cache_path.exists() else {}
    kind_word = {"biosphere": ("biosf", "biosph", "エコパーク", "生物圏"), "geopark": ("geopar", "géopar", "xeopar", "地質公園", "ジオパーク")}
    langs = ("en", "fr", "es", "pt", "ca", "gl", "zh", "ja", "zh-hant")
    # (A failed query fails the run: special areas missing their local names otherwise.)
    items = []
    rows = heritage_eu.wd_sparql("""SELECT ?item ?coord """ + " ".join(f"?{l}" for l in langs) + """ WHERE {
          VALUES ?cls { wd:Q158454 wd:Q28055306 wd:Q61453609 wd:Q1324355 wd:Q53444003 wd:Q72114283 }
          { ?item wdt:P31 ?cls } UNION { ?item wdt:P1435 ?cls }
          ?item wdt:P625 ?coord . """ + " ".join(f'OPTIONAL {{ ?item rdfs:label ?{l} FILTER(LANG(?{l}) = "{l}") }}' for l in langs) + " }",
        H / "special-wd-items.json")
    for r in rows:
        m = re.match(r"Point\(([-\d.eE]+) ([-\d.eE]+)\)", r.get("coord", ""))
        if m:
            items.append((float(m.group(1)), float(m.group(2)), {l: r[l][:1].upper() + r[l][1:] for l in langs if r.get(l)}))
    stop = {"biosphere", "reserve", "unesco", "global", "geopark", "international", "dark", "sky", "park", "national", "regional",
            "natural", "transboundary", "the", "and", "of", "de", "del", "la", "las", "los", "le", "du", "des", "et", "e", "y", "da", "do", "das", "dos",
            "mont", "mount", "monte", "montes", "monts", "sierra", "sierras", "serra", "island", "isla", "ilha", "cabo", "costa", "valle", "valles",
            "vall", "massif", "lake", "lac", "lago", "reserva", "reserve", "réserve", "biosfera", "biosphère", "parc", "parque"}
    tokens = lambda t: {w for w in norm_name(t).split() if len(w) >= 4 and w not in stop}  # noqa: E731
    for p in out:
        lang = heritage_eu.lang_at(p["lon"], p["lat"])
        if lang == "en":
            continue
        words = kind_word.get(p.get("kind"))
        if words is None:  # dark-sky places: reserves and parks by their own names
            words = ("parc", "parque", "park") if "park" in p["name"].lower() and "sky" not in p["name"].lower() else ("ciel", "étoil", "cielo", "estrell", "céu", "cel ")

        def fits(local):
            if not local or local == p["name"]:
                return False
            if words:
                return any(w in local.lower() for w in words)
            return bool(tokens(local) & tokens(p["name"]))
        pick = lambda labels: labels.get(lang) or (labels.get("es") if lang in ("ca", "gl") else None)  # noqa: E731
        local = None
        mine = tokens(p["name"])
        best = None
        for lon, lat, labels in items:
            d = math.hypot((lon - p["lon"]) * math.cos(math.radians(p["lat"])), lat - p["lat"]) * 111.2
            if d > 80:
                continue
            common = mine & set().union(*(tokens(v) for v in labels.values()))
            # Most of the name's own words, or one of them very close by.
            if common and (len(common) >= 0.5 * len(mine) or d < 25) and fits(pick(labels)) and (best is None or (len(common), -d) > best[0]):
                best = ((len(common), -d), pick(labels))
        if best:
            local = best[1]
        else:
            labels = heritage_eu.wd_search_labels(p["name"], cache)
            local = pick(labels) if fits(pick(labels)) else None
        if not local:
            # No local label anywhere: the local generic term with the place's own name, when that
            # name isn't itself English ("Reserva de la Biosfera Doñana", not "… Marshes and Tides").
            core = re.sub(r"\s*(\(.*?\)|UNESCO Global Geopark|Transboundary Biosphere Reserve|Biosphere Reserve|International Dark Sky (Reserve|Park))\s*",
                          " ", p["name"]).strip()
            english = re.search(r"\b(and|of|the|between|in|city|islands?|mountains?|volcanoes|marshes|tides|coast|intercontinental)\b", core, re.I)
            sky = "Park" if p["name"].endswith("Dark Sky Park") else "Reserve"
            term = {"biosphere": {"fr": "Réserve de biosphère", "es": "Reserva de la Biosfera", "pt": "Reserva da Biosfera", "ca": "Reserva de la Biosfera",
                                  "gl": "Reserva da Biosfera"},
                    "geopark": {"fr": "Géoparc mondial UNESCO", "es": "Geoparque Mundial UNESCO", "pt": "Geoparque Mundial UNESCO",
                                "ca": "Geoparc Mundial UNESCO", "gl": "Xeoparque Mundial UNESCO"},
                    "dark_sky": {"fr": f"{'Parc' if sky == 'Park' else 'Réserve'} internationale de ciel étoilé".replace("Parc internationale", "Parc international"),
                                 "es": f"{'Parque' if sky == 'Park' else 'Reserva'} internacional de cielo oscuro",
                                 "ca": f"{'Parc' if sky == 'Park' else 'Reserva'} internacional de cel fosc",
                                 "pt": f"{'Parque' if sky == 'Park' else 'Reserva'} internacional de céu escuro"} if "Dark Sky" in p["name"] else {}}.get(p.get("kind"), {}).get(lang)
            local = LOCAL_NAMES.get(p["name"])
            if not local:
                if not term or english or core == p["name"]:
                    continue
                local = f"{term} {core}"
        p.setdefault("polygon_hint", p["name"])
        p["name_en"], p["name"] = p["name"], local
    tmp = cache_path.with_name(cache_path.name + ".tmp")
    tmp.write_text(json.dumps(cache, ensure_ascii=False, indent=0))
    os.replace(tmp, cache_path)
    return out


# ---- rasterise areas onto the z11 grid ----------------------------------------------------------

R = 6378137.0


def to_merc(x, y, z=None):
    x = np.asarray(x) * math.pi / 180 * R
    y = np.log(np.tan(math.pi / 4 + np.clip(np.asarray(y), -85, 85) * math.pi / 360)) * R
    return x, y


def rasterise(b: Path, shapes_bits: list[tuple]) -> None:
    tiles = np.fromfile(b / "grid.idx", dtype=np.uint32).reshape(-1, 2)
    out = np.zeros((len(tiles), 256, 256), np.uint8)
    geoms = [g for g, _ in shapes_bits]
    tree = STRtree(geoms)
    world = 2 * math.pi * R
    for i, (tx, ty) in enumerate(tqdm(tiles, desc="areas → z11 grid", unit="tile")):
        x0 = tx / 2048 * world - world / 2
        x1 = (tx + 1) / 2048 * world - world / 2
        y1 = world / 2 - ty / 2048 * world
        y0 = world / 2 - (ty + 1) / 2048 * world
        hits = tree.query(box(x0, y0, x1, y1))
        if len(hits) == 0:
            continue
        tr = from_bounds(x0, y0, x1, y1, 256, 256)
        for bit in (PARK, HERITAGE, SPECIAL, INDIGENOUS):
            sel = [geoms[h] for h in hits if shapes_bits[h][1] == bit]
            if sel:
                m = features.rasterize(((g, 1) for g in sel), out_shape=(256, 256), transform=tr, dtype=np.uint8,
                                       all_touched=bit == HERITAGE)
                out[i] |= (m * bit).astype(np.uint8)
    out.tofile(b / "grid.areas.u8.tmp")
    os.replace(b / "grid.areas.u8.tmp", b / "grid.areas.u8")


PARK_TITLE = re.compile(r"park|forest|reserve|wilderness|wildlife|refuge|sanctuary|preserve|conservation area|natural area|recreation area|seashore|lakeshore", re.I)


def norm_name(s: str) -> str:
    s = unicodedata.normalize("NFKD", s or "").encode("ascii", "ignore").decode().lower()
    return " ".join(re.findall(r"[a-z0-9]+", s))


def write_json(path: Path, obj) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(obj, ensure_ascii=False, separators=(",", ":")))
    os.replace(tmp, path)


def option(args: list[str], name: str) -> str | None:
    """Removes `name value` from args and returns the value (None when absent)."""
    if name not in args:
        return None
    i = args.index(name)
    v = args[i + 1]
    del args[i:i + 2]
    return v


def main():
    args = [a for a in sys.argv[1:]]
    tiles_path, zoom, date = option(args, "--tiles"), int(option(args, "--zoom") or 11), option(args, "--date")
    b = Path(args[0] if args else "../data/build")
    areas_path = b.parent / "areas" / "areas.geojsonseq"
    # What's covered: the build's analysis grid (z11), or the job's tiles.
    with phase("the covered tiles read", "disk"):
        tiles = {tuple(t) for t in np.fromfile(Path(tiles_path) if tiles_path else b / "grid.idx", dtype=np.uint32).reshape(-1, 2).tolist()}
    n_tiles = 2 ** zoom

    def covered(lon, lat):
        if not (-85 < lat < 85 and -180 <= lon <= 180):
            return False  # a register's bad coordinates
        x = (lon + 180) / 360 * n_tiles
        y = (1 - math.log(math.tan(math.radians(lat)) + 1 / math.cos(math.radians(lat))) / math.pi) / 2 * n_tiles
        return (int(x), int(y)) in tiles

    counts = {}
    sites, harea = [], []
    # UNESCO for every country (heritage_eu.unesco), then the national and regional registers.
    registers = (*heritage_eu.SOURCES[:1], ("Parks Canada DFHD (federal)", federal),
                 ("NPS National Register", nrhp), ("Quebec RPCQ", quebec), ("Ontario Heritage Act Register", ontario),
                 ("Nova Scotia + Halifax", nova_scotia), ("Moncton", new_brunswick_moncton),
                 ("Canadian Register (NB, PEI, NL, West, North)", crhp), *heritage_eu.SOURCES[1:])
    for i, (label, fn) in enumerate(registers):
        # (A line the build agent shows as this job's progress, the register under way in brackets.)
        print(f"progress: {i}/{len(registers)} registers ({label})", file=sys.stderr, flush=True)
        with phase("the registers read", "mixed"):
            r = fn()
            pts, ars = r if isinstance(r, tuple) else (r, [])
            pts = [f for f in pts if covered(*f["geometry"]["coordinates"])]
            counts[label] = len(pts) + len(ars)
            print(f"heritage: {label:34s} {len(pts):6d} sites, {len(ars):4d} areas")
            sites += pts
            harea += ars
    print(f"progress: {len(registers)}/{len(registers)} registers", file=sys.stderr, flush=True)
    with phase("the sites tidied and deduplicated", "compute"):
        for f in sites + harea:
            for k in ("name", "name_en"):
                if k in f["properties"]:
                    v = tidy_quotes(f["properties"][k])
                    # Registers that shout ("FRANK SLIDE"): title case.
                    f["properties"][k] = v.title() if v and v.isupper() and len(v) > 3 else v
        n0 = len(sites)
        sites = dedupe(sites)
    print(f"heritage: {n0 - len(sites)} municipal duplicates dropped")
    with phase("the sites and heritage areas written", "disk"):
        write_json(b / "heritage.json", {"type": "FeatureCollection", "features": sites})
        write_json(b / "heritage-areas.json", {"type": "FeatureCollection", "features": harea})
    by = {}
    for f in sites:
        by[f["properties"]["level"]] = by.get(f["properties"]["level"], 0) + 1
    print("heritage.json:", len(sites), "sites by level", dict(sorted(by.items())))

    # Special places: official registries, else Wikidata.
    with phase("the special places read", "mixed"):
        sp = special_official()
        if sp is None:
            print("special areas: special-official.json missing — falling back to Wikidata")
            sp = special_wikidata()
        sp = [s for s in sp if covered(s["lon"], s["lat"])]
        # Entries by the OSM name to look for; several can share one (e.g. a biosphere reserve named
        # for the national park it surrounds), so each is matched on its own.
        hints: dict[str, list[int]] = {}
        for i, s in enumerate(sp):
            hints.setdefault(norm_name(s.get("polygon_hint") or s["name"]), []).append(i)

    print("protected areas & Indigenous lands (OSM)…")
    with phase("the protected areas and Indigenous lands read", "compute"):
        special_feats, indigenous_feats, matched = [], [], set()
        # The polygons in degrees, with their bits (rasterised below, or by the units: --tiles).
        shapes_ll: list[tuple] = []
        for line in tqdm(open(areas_path), desc="areas", unit="poly"):
            f = json.loads(line.strip("\x1e"))
            p = f["properties"]
            try:
                g = shape(f["geometry"])
            except Exception:  # noqa: BLE001
                continue
            if g.is_empty:
                continue
            name = p.get("name", "")
            ptitle = p.get("protection_title", "")
            for i in hints.get(norm_name(name), []):
                s = sp[i]
                # Use the mapped boundary only if it plausibly is the designated area: biosphere
                # reserves and geoparks are usually far larger than the park named in the hint.
                if i in matched or (s.get("area_km2") and g.area * 111.32 ** 2 * math.cos(math.radians(s["lat"])) < 0.5 * s["area_km2"]):
                    continue
                if g.distance(Point(s["lon"], s["lat"])) < 0.3:
                    matched.add(i)
                    special_feats.append({"type": "Feature", "geometry": mapping(g.simplify(0.0003)),
                                          "properties": {k: v for k, v in s.items() if k not in ("lon", "lat")}})
                    shapes_ll.append((g.simplify(0.0002, preserve_topology=True), SPECIAL))
            if p.get("boundary") == "aboriginal_lands":
                bit = INDIGENOUS
                indigenous_feats.append({"type": "Feature", "geometry": mapping(g.simplify(0.0003, preserve_topology=True)),
                                         "properties": {"name": name}})
            elif (p.get("boundary") == "national_park" or p.get("leisure") == "nature_reserve"
                  or p.get("protect_class") in ("1", "1a", "1b", "2", "3", "4", "5", "6") or PARK_TITLE.search(ptitle)):
                bit = PARK
            else:
                continue
            shapes_ll.append((g.simplify(0.0002, preserve_topology=True), bit))
    with phase("the remaining areas drawn", "compute"):
        for i, s in enumerate(sp):
            if i in matched:
                continue
            r_km = math.sqrt(s["area_km2"] / math.pi) if s.get("area_km2") else (15.0 if s["kind"] != "dark_sky" else 8.0)
            r_km = min(max(r_km, 3.0), 60.0)
            circ = Point(s["lon"], s["lat"]).buffer(r_km / 111.0, 48)
            circ = shp_transform(lambda x, y, lo=s["lon"], la=s["lat"]: (lo + (x - lo) / math.cos(math.radians(la)), y), circ)
            special_feats.append({"type": "Feature", "geometry": mapping(circ),
                                  "properties": {**{k: v for k, v in s.items() if k not in ("lon", "lat")}, "approx": True}})
            shapes_ll.append((circ, SPECIAL))
        print(f"special areas: {len(sp)} ({len(matched)} with OSM boundaries)")
        for f in harea:
            shapes_ll.append((shape(f["geometry"]), HERITAGE))
    with phase("the area layers written", "disk"):
        write_json(b / "special.json", {"type": "FeatureCollection", "features": special_feats})
        write_json(b / "indigenous.json", {"type": "FeatureCollection", "features": indigenous_feats})
        write_json(b / "heritage-sources.json", {"counts": counts, "special": len(sp), "built": date or datetime.now(timezone.utc).isoformat()[:19]})
    if tiles_path is not None:
        with phase("the area shapes written", "disk"):
            tmp = b / "area-shapes.geojsonseq.tmp"
            with open(tmp, "w", encoding="utf-8") as f:
                for g, bit in shapes_ll:
                    f.write(json.dumps({"type": "Feature", "geometry": mapping(g), "properties": {"bit": int(bit)}}, separators=(",", ":")) + "\n")
            os.replace(tmp, b / "area-shapes.geojsonseq")
        print(f"area-shapes.geojsonseq: {len(shapes_ll)} polygons (the units rasterise them)")
        return
    print(f"rasterising {len(shapes_ll)} polygons")
    with phase("the areas rasterised", "compute"):
        rasterise(b, [(shp_transform(to_merc, g), bit) for g, bit in shapes_ll])
    print("done — now run: scenic <build> flags")


if __name__ == "__main__":
    main()
