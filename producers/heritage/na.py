"""The North American registers and the special places, from the designating authorities' open
data: the readers registers.py runs for these registers (docs/inputs.md §6.5).

  1  UNESCO World Heritage List: eu.py
  2  Canada: Parks Canada Directory of Federal Heritage Designations, located by federal.py
     (federal.json); US: National Historic Landmarks (NPS)
  3  US: National Register of Historic Places (NPS), New York and New England (`STATES`)
  4  Quebec: Répertoire du patrimoine culturel (MCC) classified / declared / national;
     Ontario: Ontario Heritage Act Register (Ontario Heritage Trust), ministerial decisions;
     Nova Scotia: Registered Heritage Properties; NB, PEI, NL, the west and north: the Canadian
     Register of Historic Places (crhp.py, crhp.json)
  5  municipal designations from the same sources, plus Halifax (HRM) and Moncton open data
Heritage areas: Quebec heritage-site perimeters, Ontario heritage conservation districts.
Special places: UNESCO biosphere reserves and Global Geoparks, dark-sky places, from the official
registries as compiled by hand (special-official.json), named in the region's language from
Wikidata (cached in the data folder).

Each function returns (points, areas) or points as GeoJSON features; `source` is the credit
string registers.py sorts them into registers by. Everything is read from the data folder
(registers.py --data, `H`); the National Register is downloaded there once when it's missing.
"""
from __future__ import annotations

import csv
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

from shapely import wkt
from shapely.geometry import shape
from tqdm import tqdm

import eu

UA = {"User-Agent": "scenic-roads/0.1 (personal offline map)"}
# What it fetches (crate::fetch::Kind, docs/inputs.md §6.9): ArcGIS REST services and published downloads.
FETCHES = "api, data"
# The data folder (registers.py --data).
H = Path()
NRHP = "https://mapservices.nps.gov/arcgis/rest/services/cultural_resources/nrhp_locations/MapServer/0/query"
STATES = ["NEW YORK", "VERMONT", "NEW HAMPSHIRE", "MAINE", "MASSACHUSETTS", "CONNECTICUT", "RHODE ISLAND"]
csv.field_size_limit(1 << 24)


def get(url: str, params: dict | None = None, accept: str | None = None) -> bytes:
    if params:
        url += "?" + urllib.parse.urlencode(params)
    h = dict(UA)
    if accept:
        h["Accept"] = accept
    if True:
        for attempt in range(12):
            try:
                with urllib.request.urlopen(urllib.request.Request(url, headers=h), timeout=180) as r:
                    return r.read()
            except urllib.error.HTTPError as e:
                if attempt == 11:
                    raise
                # Respect rate limits.
                wait = int(e.headers.get("Retry-After") or 0) or (65 if e.code == 429 else 2 ** attempt)
                print(f"  HTTP {e.code}; waiting {wait} s", file=sys.stderr)
                time.sleep(wait)
            except Exception as e:  # noqa: BLE001
                if attempt == 11:
                    raise
                print(f"  retry ({e})", file=sys.stderr)
                time.sleep(2 ** min(attempt, 6))
    raise RuntimeError("unreachable")


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
        for f in json.loads((H / "qc" / fn).read_text())["features"]:
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
        for f in json.loads((H / "qc" / fn).read_text())["features"]:
            p = f["properties"]
            if f["geometry"] and p["statut_juridique_princ"] in ("Classement", "Déclaration"):
                areas.append({"type": "Feature", "geometry": f["geometry"], "properties": props(p, 4, desig)})
    for fn, level, desig in (("site-patrimonial-national-declare-par-la-loi-sur-le-patrimoine-culturel-par-le-gouvernement-du-.csv", 4, "Site patrimonial national"),
                             ("immeubles-patrimoniaux-cites-par-les-municipalites-et-les-communautes-autochtones.csv", 5, "Immeuble patrimonial cité"),
                             ("sites-patrimoniaux-cites-par-les-municipalites-et-les-communautes-autochtones.csv", 5, "Site patrimonial cité")):
        for r in csv.DictReader(open(H / "qc" / fn, encoding="utf-8")):
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
    rows = eu.wd_sparql("""SELECT ?item ?coord """ + " ".join(f"?{l}" for l in langs) + """ WHERE {
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
        lang = eu.lang_at(p["lon"], p["lat"])
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
            labels = eu.wd_search_labels(p["name"], cache)
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


def norm_name(s: str) -> str:
    s = unicodedata.normalize("NFKD", s or "").encode("ascii", "ignore").decode().lower()
    return " ".join(re.findall(r"[a-z0-9]+", s))
