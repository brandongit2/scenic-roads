#!/usr/bin/env python3
"""The heritage registers as the drop box inputs/heritage/ takes them (docs/inputs.md §6.5): for
each register, <register>.geojson (its entries, any geometry, with the standard properties) and
<register>.toml (its description: name, credit, licence, source, download date, class, territory,
the Wikidata property and OSM tags holding its ids).

The registers are read by the readers in eu.py and na.py, from the data folder they cache their
downloads in (--data: laid out as the old app's registers' snapshot was, `sources/registers/legacy`
extracted). A reader's features carry their register's credit string in `source`: `REGISTERS`
sorts them by it (and, for Wales and the special places, by what they are) into one register each.
Then, per register:
- names tidied (registers' quoting glitches; a name in capitals in title case), as the app's
  heritage.py did before the registers became an input;
- a point and an area that are one entry (the same properties, the point the area's
  representative point) made one polygon with `dot: true`;
- `tier`, the kind of designation the map's Heritage sites toggles group by (tiers.py), for
  `heritage` registers;
- `id`, the register's own reference (`ref_of`) where it has one, else one made from the entry's
  name and place; when two entries share a reference, the later ones' ids take `~2`, `~3`… and their
  `ref` stays the reference; an entry with no reference of the register's has `ref: ""` (nothing
  to look it up by);
- for UNESCO's list, each component's id its UNESCO reference (`1221-001`), `site` the site's id
  and `site_area_km2` its inscribed area.

usage: registers.py --data <folder> --out <folder> [<register id> …]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import unicodedata
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from shapely.geometry import shape

sys.path.insert(0, str(Path(__file__).resolve().parent))
import eu  # noqa: E402
import na  # noqa: E402
import tiers  # noqa: E402


@dataclass
class Register:
    """A register: what the map credits and how its entries are known."""
    name: str
    what: str
    credit: str
    licence: str
    source: str
    territory: list[str] | str
    # The reader that reads it, and which of the reader's features are this register's.
    reader: Callable
    takes: Callable[[dict], bool]
    # The data folder's files it's read from (globs): their newest time is its download date.
    files: list[str]
    klass: str = "heritage"
    wikidata_property: str | None = None
    # The pattern on an entry's `url` whose first group is its reference in the register.
    ref_pattern: str | None = None
    osm_ref_tags: list[str] = field(default_factory=list)
    notes: str = ""


def src(s: str) -> Callable[[dict], bool]:
    return lambda p: p.get("source") == s


def src_url(s: str, part: str) -> Callable[[dict], bool]:
    return lambda p: p.get("source") == s and part in (p.get("url") or "")


def special(kind: str, certifier: str) -> Callable[[dict], bool]:
    return lambda p: p.get("kind") == kind and p.get("certifier") == certifier


FRANCE = ["FR", "GP", "MQ", "GF", "RE", "YT", "PM", "BL", "MF"]
SG = "Contains information from {} accessed on "

REGISTERS: dict[str, Register] = {
    "unesco-whc": Register(
        "UNESCO World Heritage List", "UNESCO World Heritage", "UNESCO World Heritage Centre, World Heritage List (data.unesco.org)",
        "CC-BY-SA-4.0", "https://data.unesco.org/explore/dataset/whc001/", "world", eu.unesco, src(eu.WHC_SRC), ["unesco/*"],
        klass="world-heritage", wikidata_property="P757", ref_pattern=r"whc\.unesco\.org/en/list/(\d+)"),
    "fr-merimee": Register(
        "Mérimée, monuments historiques", "France heritage", "Ministère de la Culture, base Mérimée (POP)", "etalab-2.0",
        "https://www.pop.culture.gouv.fr/", FRANCE, eu.france, src(eu.FR_SRC), ["fr/merimee.csv"],
        wikidata_property="P380", ref_pattern=r"pop\.culture\.gouv\.fr/notice/merimee/(\w+)", osm_ref_tags=["ref:mhs"]),
    "fr-spr": Register(
        "Sites patrimoniaux remarquables", "France heritage areas", "Sites patrimoniaux remarquables via the Géoportail de l'Urbanisme (servitude AC4)",
        "etalab-2.0", "https://www.geoportail-urbanisme.gouv.fr/", FRANCE, eu.france,
        src("Géoportail de l'Urbanisme (servitude AC4), Licence Ouverte 2.0"), ["fr/spr-ac4.geojson"]),
    "ad-patrimoni": Register(
        "Inventari general del patrimoni cultural d'Andorra", "Andorra heritage",
        "Govern d'Andorra, Inventari general del patrimoni cultural (IDE Andorra)", "Private, personal use only",
        "https://www.ideandorra.ad/", ["AD"], eu.andorra, src(eu.AD_SRC), ["ad/*"]),
    "gb-eng-nhle": Register(
        "National Heritage List for England", "England heritage",
        "© Historic England, National Heritage List for England; contains Ordnance Survey data © Crown copyright and database right",
        "OGL-UK-3.0", "https://historicengland.org.uk/listing/the-list/", ["GB-ENG"], eu.england, src(eu.EN_SRC), ["uk/en-*"],
        wikidata_property="P1216", ref_pattern=r"historicengland\.org\.uk/listing/the-list/list-entry/(\d+)", osm_ref_tags=["HE_ref", "ref:GB:nhle"]),
    "gb-sct-hes": Register(
        "Historic Environment Scotland designations", "Scotland heritage",
        "Contains Historic Environment Scotland and Ordnance Survey data © Historic Environment Scotland – Scottish Charity No. SC045925 © Crown copyright and database right",
        "OGL-UK-3.0", "https://portal.historicenvironment.scot/", ["GB-SCT"], eu.scotland, src(eu.SC_SRC), ["uk/sc-*"],
        wikidata_property="P709", ref_pattern=r"portal\.historicenvironment\.scot/designation/(\w+)", osm_ref_tags=["ref:hs", "ref:GB:hs"]),
    "gb-wls-listed": Register(
        "Cadw listed buildings", "Wales heritage: listed buildings",
        "Designated Historic Asset GIS Data, The Welsh Historic Environment Service (Cadw), via DataMapWales", "OGL-UK-3.0",
        "https://datamap.gov.wales/", ["GB-WLS"], eu.wales, src_url(eu.WA_SRC, "/listedbuilding/"), ["uk/wa-listed.geojson"],
        wikidata_property="P1459", ref_pattern=r"cadwpublic-api\.azurewebsites\.net/reports/listedbuilding/FullReport\?lang=en&id=(\d+)"),
    "gb-wls-sam": Register(
        "Cadw scheduled monuments", "Wales heritage: scheduled monuments",
        "Designated Historic Asset GIS Data, The Welsh Historic Environment Service (Cadw), via DataMapWales", "OGL-UK-3.0",
        "https://datamap.gov.wales/", ["GB-WLS"], eu.wales, src_url(eu.WA_SRC, "/sam/"), ["uk/wa-scheduled.geojson"],
        wikidata_property="P3007", ref_pattern=r"cadwpublic-api\.azurewebsites\.net/reports/sam/FullReport\?lang=en&id=(\w+)"),
    "gb-wls-parks": Register(
        "Cadw registered historic parks and gardens", "Wales heritage: parks and gardens",
        "Designated Historic Asset GIS Data, The Welsh Historic Environment Service (Cadw), via DataMapWales", "OGL-UK-3.0",
        "https://datamap.gov.wales/", ["GB-WLS"], eu.wales, src_url(eu.WA_SRC, "/parkgarden/"), ["uk/wa-parks.geojson"]),
    "gb-nir-hed": Register(
        "Historic Environment Division designations", "Northern Ireland heritage",
        "Department for Communities, Historic Environment Division", "OGL-UK-3.0", "https://www.communities-ni.gov.uk/",
        ["GB-NIR"], eu.northern_ireland, src(eu.NI_SRC), ["uk/ni-*"]),
    "gg-dpa": Register(
        "Guernsey protected buildings and monuments", "Guernsey heritage", "States of Guernsey Development & Planning Authority",
        "gov.gg terms: research and private use", "https://www.gov.gg/", ["GG"], eu.guernsey, src(eu.GG_SRC), ["gg/*"]),
    "ie-niah": Register(
        "National Inventory of Architectural Heritage", "Ireland heritage: buildings",
        "National Inventory of Architectural Heritage, Department of Housing, Local Government and Heritage", "CC-BY-4.0",
        "https://www.buildingsofireland.ie/", ["IE"], eu.ireland, src(eu.IE_NIAH_SRC), ["ie/niah-*"]),
    "ie-smr": Register(
        "Sites and Monuments Record", "Ireland heritage: monuments",
        "National Monuments Service, Archaeological Survey of Ireland (Department of Housing, Local Government and Heritage)", "CC-BY-4.0",
        "https://www.archaeology.ie/", ["IE"], eu.ireland, src(eu.IE_SMR_SRC), ["ie/smr-*"],
        wikidata_property="P4057", ref_pattern=r"query=[^&]*?%2CSMRS%2C([A-Z]{2}\d{3}-\d{3}(?:\d{3})?)", osm_ref_tags=["ref:IE:smr"]),
    "es-ct-bcin": Register(
        "Béns culturals d'interès nacional", "Spain heritage: Catalonia", "Generalitat de Catalunya, Inventari del Patrimoni Cultural",
        "free use with credit", "https://sig.gencat.cat/", ["ES-CT"], eu.spain,
        src("Generalitat de Catalunya, Inventari del Patrimoni Cultural"), ["es/cat-*"],
        wikidata_property="P1600", ref_pattern=r"invarquit\.cultura\.gencat\.cat/card/(\d+)"),
    "es-cl-bic": Register(
        "Bienes de interés cultural de Castilla y León", "Spain heritage: Castilla y León", "Junta de Castilla y León",
        "free use with credit", "https://idecyl.jcyl.es/", ["ES-CL"], eu.spain, src("Junta de Castilla y León"), ["es/cyl-*"]),
    "es-ar-bic": Register(
        "Bienes de interés cultural de Aragón", "Spain heritage: Aragón", "Gobierno de Aragón (Aragón Open Data)",
        "free use with credit", "https://opendata.aragon.es/", ["ES-AR"], eu.spain, src("Gobierno de Aragón (Aragón Open Data)"), ["es/aragon-*"]),
    "es-vc-bic": Register(
        "Bienes de interés cultural valencianos", "Spain heritage: Comunitat Valenciana",
        "Generalitat Valenciana, Inventario General del Patrimonio Cultural Valenciano", "free use with credit",
        "https://terramapas.icv.gva.es/", ["ES-VC"], eu.spain,
        src("Generalitat Valenciana, Inventario General del Patrimonio Cultural Valenciano"), ["es/valencia-*"]),
    "es-ga-bic": Register(
        "Bens de interese cultural de Galicia", "Spain heritage: Galicia", "Xunta de Galicia", "CC-BY-SA-4.0",
        "https://ficheiros-web.xunta.gal/patrimonio/bic/", ["ES-GA"], eu.spain, src("Xunta de Galicia (CC BY-SA 4.0)"), ["es/galicia-*"]),
    "es-nc-bic": Register(
        "Bienes de interés cultural de Navarra", "Spain heritage: Navarra", "Gobierno de Navarra, IDENA", "CC-BY-4.0",
        "https://idena.navarra.es/", ["ES-NC"], eu.spain, src("Gobierno de Navarra, IDENA (CC BY 4.0)"), ["es/navarra-*"]),
    "es-ex-bic": Register(
        "Bienes de interés cultural de Extremadura", "Spain heritage: Extremadura", "Junta de Extremadura", "free use with credit",
        "https://www.juntaex.es/", ["ES-EX"], eu.spain, src("Junta de Extremadura"), ["es/extremadura-*"]),
    "es-an-iaph": Register(
        "Patrimonio inmueble protegido de Andalucía", "Spain heritage: Andalucía", "Instituto Andaluz del Patrimonio Histórico (IAPH), Junta de Andalucía",
        "free use with credit", "https://www.iaph.es/", ["ES-AN"], eu.spain, src("Instituto Andaluz del Patrimonio Histórico (IAPH)"), ["es/andalucia-*"]),
    "pt-atlas": Register(
        "Atlas do Património Classificado", "Portugal heritage",
        "Património Cultural, I.P., Atlas do Património Classificado e em Vias de Classificação", "CC-BY-NC-4.0",
        "https://www.patrimoniocultural.gov.pt/", ["PT"], eu.portugal, src(eu.PT_SRC), ["pt/*"],
        wikidata_property="P1702", ref_pattern=r"patrimoniocultural\.pt/.*/view/(\d+)", osm_ref_tags=["ref:dgpc"]),
    "hk-amo": Register(
        "Declared monuments and graded historic buildings", "Hong Kong heritage",
        "Antiquities and Monuments Office, via the Common Spatial Data Infrastructure (CSDI) Portal", "DATA.GOV.HK terms",
        "https://portal.csdi.gov.hk/", ["HK"], eu.hong_kong, src(eu.HK_SRC), ["hk/*"]),
    "jp-bunkacho": Register(
        "国指定文化財等データベース", "Japan heritage",
        "出典：文化庁 国指定文化財等データベース（https://kunishitei.bunka.go.jp/）を加工して作成 (Agency for Cultural Affairs, Database of National Cultural Properties, edited); preservation districts: 国土数値情報（伝統的建造物群保存地区データ）(MLIT)",
        "PDL-1.0; CC-BY-4.0", "https://kunishitei.bunka.go.jp/", ["JP"], eu.japan, src(eu.JP_SRC), ["jp/*"],
        wikidata_property="P4275", ref_pattern=r"kunishitei\.bunka\.go\.jp/heritage/detail/(\d+/\d+)"),
    "tw-boch": Register(
        "文化資產個案", "Taiwan heritage", eu.TW_SRC, "OGDL-Taiwan-1.0", "https://data.boch.gov.tw/", ["TW"], eu.taiwan,
        src(eu.TW_SRC), ["tw/*"], wikidata_property="P6890", ref_pattern=r"nchdb\.boch\.gov\.tw/assets/advanceSearch/(\w+/\d+)"),
    "sg-nhb-monuments": Register(
        "National Monuments (NHB)", "Singapore heritage: national monuments",
        "Contains information from Monuments (NHB) from data.gov.sg which is made available under the terms of the Singapore Open Data Licence version 1.0 https://data.gov.sg/open-data-licence",
        "Singapore Open Data Licence 1.0", "https://data.gov.sg/", ["SG"], eu.singapore,
        lambda p: (p.get("source") or "").startswith(SG.format("Monuments (NHB)")), ["sg/d_b29c230ec6b609e29ed42f71ca9a8767.geojson"]),
    "sg-nhb-historic-sites": Register(
        "Historic Sites (NHB)", "Singapore heritage: historic sites",
        "Contains information from Historic Sites (NHB) from data.gov.sg which is made available under the terms of the Singapore Open Data Licence version 1.0 https://data.gov.sg/open-data-licence",
        "Singapore Open Data Licence 1.0", "https://data.gov.sg/", ["SG"], eu.singapore,
        lambda p: (p.get("source") or "").startswith(SG.format("Historic Sites (NHB)")), ["sg/d_31e16b12809e66673e90d8b04fdee1b2.geojson"]),
    "sg-ura-conservation": Register(
        "Conservation areas (URA)", "Singapore heritage: conservation areas",
        "Contains information from Master Plan 2019 SDCP Conservation Area layer (URA) from data.gov.sg which is made available under the terms of the Singapore Open Data Licence version 1.0 https://data.gov.sg/open-data-licence",
        "Singapore Open Data Licence 1.0", "https://data.gov.sg/", ["SG"], eu.singapore,
        lambda p: (p.get("source") or "").startswith(SG.format("Master Plan 2019 SDCP Conservation Area layer (URA)")), ["sg/d_8c8162ffb9deb8d11b00623048f65a70.geojson"]),
    "ca-dfhd": Register(
        "Directory of Federal Heritage Designations", "Canadian federal designations", "Parks Canada Directory of Federal Heritage Designations",
        "OGL-Canada-2.0", "https://parks.canada.ca/culture/dfhd", ["CA"], na.federal, src("Parks Canada DFHD"), ["federal.json", "fhd.xlsx"],
        notes="Located by federal.py (the directory has no coordinates): `approx` where matched loosely."),
    "us-nrhp": Register(
        "National Register of Historic Places", "US designations", "NPS National Register of Historic Places", "public domain",
        "https://www.nps.gov/subjects/nationalregister/", ["US"], na.nrhp, src("NPS National Register"), ["nrhp.json"],
        wikidata_property="P649", osm_ref_tags=["ref:nrhp"], notes="New York and New England (na.STATES)."),
    "ca-qc-rpcq": Register(
        "Répertoire du patrimoine culturel du Québec", "Québec heritage", "MCC Répertoire du patrimoine culturel", "CC-BY-4.0",
        "https://www.patrimoine-culturel.gouv.qc.ca/", ["CA-QC"], na.quebec, src(na.QC_SRC), ["qc/*"],
        wikidata_property="P633", ref_pattern=r"patrimoine-culturel\.gouv\.qc\.ca/rpcq/detail\.do\?methode=consulter&id=(\d+)"),
    "ca-on-oha": Register(
        "Ontario Heritage Act Register", "Ontario heritage", "Ontario Heritage Act Register (Ontario Heritage Trust)",
        "personal non-commercial use only", "https://www.heritagetrust.on.ca/oha/search-the-oha-register", ["CA-ON"], na.ontario, src(na.ON_SRC), ["on/*"]),
    "ca-ns-rhp": Register(
        "Registered Heritage Properties", "Nova Scotia heritage", "Nova Scotia Registered Heritage Properties", "NS Open Government Licence",
        "https://data.novascotia.ca/d/7pnv-7sdm", ["CA-NS"], na.nova_scotia,
        src("Nova Scotia Registered Heritage Properties (NS Open Government Licence)"), ["ns/registered.csv"]),
    "ca-ns-hrm": Register(
        "Halifax municipally registered heritage properties", "Halifax heritage", "Halifax Regional Municipality open data", "HRM open data",
        "https://www.halifax.ca/", ["CA-NS"], na.nova_scotia, src("Halifax Regional Municipality open data"), ["ns/hrm.geojson"]),
    "ca-nb-moncton": Register(
        "Moncton heritage properties", "Moncton heritage", "City of Moncton open data", "Moncton open data", "https://www.moncton.ca",
        ["CA-NB"], na.new_brunswick_moncton, src("City of Moncton open data"), ["nb/*"]),
    "ca-crhp": Register(
        "Canadian Register of Historic Places", "NB, PEI, NL, western & northern Canada heritage",
        "Canadian Register of Historic Places (historicplaces.ca)", "non-commercial reproduction with credit",
        "https://www.historicplaces.ca/", ["CA-NB", "CA-PE", "CA-NL", "CA-MB", "CA-SK", "CA-AB", "CA-BC", "CA-YT", "CA-NT", "CA-NU"],
        na.crhp, src("Canadian Register of Historic Places (historicplaces.ca)"), ["crhp.json", "crhp/*"],
        wikidata_property="P477", ref_pattern=r"historicplaces\.ca/en/rep-reg/place-lieu\.aspx\?id=(\d+)",
        notes="Read by crhp.py (crhp.json); provincial and municipal designations only."),
    "unesco-mab": Register(
        "UNESCO biosphere reserves", "Biosphere reserves", "UNESCO Man and the Biosphere Programme", "facts from the official registry",
        "https://www.unesco.org/en/mab/map", "world", na.special_official, special("biosphere", "UNESCO"), ["special-*"], klass="special"),
    "unesco-iggp": Register(
        "UNESCO Global Geoparks", "Geoparks", "UNESCO Global Geoparks", "facts from the official registry",
        "https://www.unesco.org/en/iggp/geoparks", "world", na.special_official, special("geopark", "UNESCO"), ["special-*"], klass="special"),
    "darksky": Register(
        "International Dark Sky Places", "Dark-sky places", "DarkSky International", "facts from the official registry",
        "https://darksky.org/what-we-do/international-dark-sky-places/all-places/", "world", na.special_official,
        special("dark_sky", "DarkSky International"), ["special-*"], klass="special"),
    "rasc-dsp": Register(
        "RASC Dark-Sky Preserves", "Dark-sky preserves (Canada)", "Royal Astronomical Society of Canada", "facts from the official registry",
        "https://rasc.ca/lpa/dark-sky-sites", "world", na.special_official, special("dark_sky", "RASC"), ["special-*"], klass="special"),
}


# ---- the entries ---------------------------------------------------------------------------------

def tidy_name(v):
    """A register's name tidied: its quoting glitches, and title case for one in capitals."""
    v = na.tidy_quotes(v)
    return v.title() if v and v.isupper() and len(v) > 3 else v


def rounded(c) -> tuple[float, float]:
    return round(float(c[0]), 6), round(float(c[1]), 6)


def rep(g: dict) -> tuple[float, float] | None:
    """An area's representative point, as a reader takes a site from it (rounded as sites are)."""
    try:
        p = shape(g).representative_point()
    except Exception:  # noqa: BLE001
        return None
    return None if p.is_empty else rounded((p.x, p.y))


def made_id(p: dict, c) -> str:
    """An id for an entry the register gives no reference for: from its name, kind of designation
    and place (a site's point, or its area's first coordinate)."""
    key = json.dumps([p.get("name"), p.get("designation") or p.get("kind"), c], ensure_ascii=False)
    return "h" + hashlib.blake2b(key.encode(), digest_size=8).hexdigest()


def first_coord(g: dict):
    c = g["coordinates"]
    while isinstance(c[0], (list, tuple)):
        c = c[0]
    return rounded(c)


class Refs:
    """The National Register's reference numbers by the National Archives link a site carries."""
    nara: dict[str, str] | None = None

    @classmethod
    def nrhp(cls, url: str) -> str | None:
        if cls.nara is None:
            cls.nara = {}
            for f in json.loads((na.H / "nrhp.json").read_text())["features"]:
                p = f["properties"]
                if p.get("NARA_URL") and p.get("NRIS_Refnum"):
                    cls.nara[p["NARA_URL"]] = p["NRIS_Refnum"]
        if url in cls.nara:
            return cls.nara[url]
        m = re.search(r"npgallery\.nps\.gov/AssetDetail/NRIS/(\d+)", url)
        return m.group(1) if m else None


def ref_of(rid: str, reg: Register, p: dict) -> str | None:
    """The entry's reference in its register, where it has one we know."""
    url = p.get("url") or ""
    if rid == "us-nrhp":
        return Refs.nrhp(url)
    if rid == "ca-dfhd" and p.get("dfhd_id") is not None:
        return str(p["dfhd_id"])
    if reg.ref_pattern:
        m = re.search(reg.ref_pattern, url)
        return m.group(1) if m else None
    return None


def whc_components() -> dict[tuple[str, tuple[float, float]], list[str]]:
    """UNESCO's references of the World Heritage components, by site id and place, in the list's
    order (several components can share a place: the reader's points come in that order)."""
    out: dict[tuple[str, tuple[float, float]], list[str]] = {}
    for s in json.loads((eu.H / "unesco" / "whc001.json").read_text()):
        for _n, ref, la, lo in eu.COMPONENT.findall(s.get("components_list") or ""):
            out.setdefault((str(s["id_no"]), rounded((float(lo), float(la)))), []).append(ref.strip())
    return out


def whc_areas() -> dict[str, float]:
    out = {}
    for s in json.loads((eu.H / "unesco" / "whc001.json").read_text()):
        try:
            if s.get("area_hectares"):
                out[str(s["id_no"])] = float(s["area_hectares"]) / 100
        except (TypeError, ValueError):
            pass
    return out


def entries(rid: str, reg: Register, pts: list[dict], areas: list[dict]) -> list[dict]:
    """The register's entries from a reader's points and areas (its own among them)."""
    out: list[dict] = []
    if reg.klass == "special":
        for s in pts:
            p = {k: v for k, v in s.items() if k not in ("lon", "lat", "source", "polygon_hint")}
            if "polygon_hint" in s:
                p["osm_name"] = s["polygon_hint"]
            out.append({"type": "Feature", "geometry": {"type": "Point", "coordinates": [s["lon"], s["lat"]]}, "properties": p})
    else:
        # (An area without coordinates isn't an entry: a register's empty geometry.)
        empty = [a for a in areas if not a["geometry"].get("coordinates") or shape(a["geometry"]).is_empty]
        if empty:
            print(f"{rid}: {len(empty)} areas without coordinates left out", file=sys.stderr)
            areas = [a for a in areas if a not in empty]
        for f in pts + areas:
            for k in ("name", "name_en"):
                if k in f["properties"]:
                    f["properties"][k] = tidy_name(f["properties"][k])
        # A point that is an area's representative point, with the same properties: one entry.
        by_place: dict[tuple, list[int]] = {}
        for i, f in enumerate(pts):
            by_place.setdefault(tuple(f["geometry"]["coordinates"]), []).append(i)
        taken: set[int] = set()
        area_of: dict[int, dict] = {}
        alone: list[dict] = []
        for a in areas:
            r = rep(a["geometry"])
            i = next((i for i in by_place.get(r, []) if i not in taken and pts[i]["properties"] == a["properties"]), None) if r else None
            if i is None:
                alone.append(a)
            else:
                taken.add(i)
                area_of[i] = a
        for i, f in enumerate(pts):
            if i in area_of:
                out.append({"type": "Feature", "geometry": area_of[i]["geometry"], "properties": {**f["properties"], "dot": True}})
            else:
                out.append(f)
        out.extend(alone)
    comps = whc_components() if reg.klass == "world-heritage" else {}
    km2 = whc_areas() if reg.klass == "world-heritage" else {}
    seen: dict[str, int] = {}
    for f in out:
        p = f["properties"]
        g = f["geometry"]
        c = rounded(g["coordinates"]) if g["type"] == "Point" else first_coord(g)
        if reg.klass == "heritage":
            p["tier"] = tiers.tier(p)
        ref = ref_of(rid, reg, p)
        if reg.klass == "world-heritage":
            site = ref
            at = comps.get((site, c))
            ref = at.pop(0) if at else site
            p["site"] = site
            if site in km2:
                p["site_area_km2"] = km2[site]
        p.pop("source", None)
        base = ref or made_id(p, c)
        n = seen.get(base, 0) + 1
        seen[base] = n
        p["id"] = base if n == 1 else f"{base}~{n}"
        if ref is None:
            p["ref"] = ""
        elif ref != p["id"]:
            p["ref"] = ref
        # The id first, as the shape lists it.
        f["properties"] = {"id": p.pop("id"), **p}
    return out


# ---- writing ---------------------------------------------------------------------------------------

def toml_str(s: str) -> str:
    return json.dumps(s, ensure_ascii=False)


def description(rid: str, reg: Register, retrieved: str) -> str:
    lines = [
        f"name = {toml_str(reg.name)}",
        f"what = {toml_str(reg.what)}",
        f"credit = {toml_str(reg.credit)}",
        f"licence = {toml_str(reg.licence)}",
        f"source = {toml_str(reg.source)}",
        f"retrieved = {retrieved}",
        f"class = {toml_str(reg.klass)}",
        "territory = " + (toml_str(reg.territory) if isinstance(reg.territory, str) else "[" + ", ".join(toml_str(t) for t in reg.territory) + "]"),
    ]
    if reg.wikidata_property:
        lines.append(f"wikidata_property = {toml_str(reg.wikidata_property)}")
    if reg.osm_ref_tags:
        lines.append("osm_ref_tags = [" + ", ".join(toml_str(t) for t in reg.osm_ref_tags) + "]")
    if reg.notes:
        lines.append(f"notes = {toml_str(reg.notes)}")
    return "\n".join(lines) + "\n"


def write_geojson(path: Path, feats: list[dict]) -> None:
    tmp = path.with_name(path.name + ".tmp")
    with open(tmp, "w", encoding="utf-8") as f:
        f.write('{"type":"FeatureCollection","features":[\n')
        f.write(",\n".join(json.dumps(x, ensure_ascii=False, separators=(",", ":")) for x in feats))
        f.write("\n]}\n")
    tmp.replace(path)


def retrieved_of(data: Path, globs: list[str]) -> str:
    ts = [p.stat().st_mtime for g in globs for p in data.glob(g) if p.is_file()]
    if not ts:
        sys.exit(f"no files for {globs} in {data}")
    return datetime.fromtimestamp(max(ts), timezone.utc).strftime("%Y-%m-%d")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--data", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("ids", nargs="*")
    a = ap.parse_args()
    eu.H = na.H = a.data.resolve()
    want = a.ids or list(REGISTERS)
    unknown = [r for r in want if r not in REGISTERS]
    if unknown:
        sys.exit(f"unknown registers: {', '.join(unknown)} (known: {', '.join(REGISTERS)})")
    a.out.mkdir(parents=True, exist_ok=True)
    # Each reader once, for every register it reads.
    readers: dict[Callable, list[str]] = {}
    for rid in want:
        readers.setdefault(REGISTERS[rid].reader, []).append(rid)
    for reader, rids in readers.items():
        print(f"{reader.__module__}.{reader.__name__}: {', '.join(rids)}", file=sys.stderr, flush=True)
        r = reader()
        if r is None:
            sys.exit(f"{reader.__name__}: its data isn't in {a.data}")
        pts, areas = r if isinstance(r, tuple) else (r, [])
        # Every feature a register of the reader's (all of them, wanted or not).
        mine = [x for x, reg in REGISTERS.items() if reg.reader is reader]
        for f in pts + areas:
            p = f if "lon" in f else f["properties"]
            hits = [x for x in mine if REGISTERS[x].takes(p)]
            if len(hits) != 1:
                sys.exit(f"{reader.__name__}: a feature {'in no register' if not hits else 'in ' + ', '.join(hits)}: {json.dumps(p, ensure_ascii=False)[:300]}")
            f["_reg"] = hits[0]
        groups = {rid: ([f for f in pts if f["_reg"] == rid], [f for f in areas if f["_reg"] == rid]) for rid in rids}
        for f in pts + areas:
            f.pop("_reg")
        for rid in rids:
            reg = REGISTERS[rid]
            ps, ars = groups[rid]
            feats = entries(rid, reg, ps, ars)
            write_geojson(a.out / f"{rid}.geojson", feats)
            (a.out / f"{rid}.toml").write_text(description(rid, reg, retrieved_of(a.data, reg.files)), encoding="utf-8")
            n_area = sum(f["geometry"]["type"] != "Point" for f in feats)
            print(f"{rid}: {len(feats)} entries ({n_area} areas)", flush=True)


if __name__ == "__main__":
    main()
