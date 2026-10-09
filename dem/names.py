"""English for the names our layers carry (`en`: layers.py, ferries.py): a name's own English, else
the names table's for where it is (data/names/english.json beside the scripts, the heritage job's
from its seeds; none without it). English counts only when it truly differs from the name, not just
in accents, case, punctuation or spacing (Montréal, Montreal). A heritage site's own English is
UNESCO's, else its English Wikipedia article's title when that's English.

A name's table is by region, by location, which decides how a name is read (中山 is Nakayama in
Japan, Zhongshan in Taiwan): jp Japan, tw Taiwan, hk Hong Kong, sg Singapore; elsewhere one table
for Latin script.
"""
from __future__ import annotations

import json
import re
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
N = ROOT / "data" / "names"


# A Wikipedia title is English (not the site's other name: "Hôtel du Tillet de la Bussière") when it
# has an English word for what the site is.
ENGLISH = re.compile(r"\b(" + "|".join("""
abbey castle church cathedral chapel house houses palace bridge tower towers museum priory manor fort fortress hall
garden gardens park lake river mount mountain island islands cave caves monastery convent basilica mill lighthouse
station square street market theatre theater hospital school college university library gate walls wall ruins site
temple shrine tomb tombs mound battlefield aqueduct viaduct canal harbour port citadel villa estate farm barracks
bastion arch column monument memorial statue fountain cross well baths amphitheatre hotel courthouse prison mine
quarry dam reservoir falls waterfall valley gorge beach beaches bay cape forest peak hill hills range district
building buildings cemetery pavilion residence mansion cottage lodge inn pier wharf dock dockyard railway line road
trail avenue centre center observatory stadium rock stone stones circle henge barrow hillfort camp keep gallery
town city village old""".split()) + r")\b", re.I)

# Where a name is read: by location.
REGIONS = [("hk", 113.8, 22.1, 114.5, 22.6), ("sg", 103.55, 1.1, 104.2, 1.5), ("tw", 118.0, 21.8, 122.3, 26.5),
           ("jp", 122.5, 20.0, 154.5, 46.0)]


def region(lon: float, lat: float) -> str:
    for r, w, s, e, n in REGIONS:
        if w <= lon <= e and s <= lat <= n:
            return r
    if lon < -30:
        return "pt" if lat < 45 and lon > -35 else "na"  # (the Azores are at 25–31 °W)
    if lon < -12:
        return "pt"  # Madeira
    if lat >= 49.8 and lon < 2.0 or lat >= 51.5:
        return "gb"
    # Iberia south of the Pyrenees (the border, roughly: 42.7 °N, and 43.4 °N on the Basque coast).
    return "ib" if lat < 42.7 or (lon < -1.7 and lat < 43.4) else "fr"


TABLE = {"jp": "jp", "tw": "tw", "hk": "hk", "sg": "sg"}  # the Latin regions share one table


def table_of(r: str) -> str:
    return TABLE.get(r, "latin")


def norm(s: str) -> str:
    s = unicodedata.normalize("NFKD", s.lower())
    return re.sub(r"[\W_]+", "", "".join(c for c in s if not unicodedata.combining(c)))


def differs(name: str, en: str | None) -> bool:
    return bool(en) and norm(en) != norm(name)


def latin(s: str) -> bool:
    lat = other = 0
    for c in s:
        if c.isalpha():
            if "LATIN" in unicodedata.name(c, ""):
                lat += 1
            else:
                other += 1
    return lat >= other


def site_english(name: str, own: str | None, wiki: str | None) -> tuple[str | None, str]:
    """A heritage site's own English: UNESCO's, else its English Wikipedia article's title when
    that's English (or the name isn't in Latin script)."""
    if differs(name, own):
        return own.strip(), "unesco"
    if wiki:
        wiki = re.sub(r"\s*\([^)]*\)$", "", wiki).strip()
        if differs(name, wiki) and (not latin(name) or ENGLISH.search(wiki)):
            return wiki, "wikipedia"
    return None, ""


def wiki_titles() -> dict[int, str]:
    """Heritage record → its English Wikipedia article's title."""
    out = {}
    items = ROOT / "data" / "heritage" / "wd" / "items.jsonl"
    if items.exists():
        for line in open(items, encoding="utf-8"):
            it = json.loads(line)
            t = (it.get("wiki") or {}).get("en")
            if t:
                out[it["i"]] = t
    return out


def first_point(g: dict | None) -> tuple[float, float] | None:
    """A geometry's first position (None when it has none)."""
    def walk(c):
        if isinstance(c, list):
            if len(c) >= 2 and isinstance(c[0], (int, float)) and isinstance(c[1], (int, float)):
                return c[0], c[1]
            for x in c:
                p = walk(x)
                if p:
                    return p
        return None
    if not g:
        return None
    return walk(g.get("coordinates") or [x.get("coordinates") for x in g.get("geometries", [])])


_TABLES: dict | None = None


def english_of(name: str, at) -> str | None:
    """The table's English for a name where it is (at: lon, lat)."""
    global _TABLES
    if _TABLES is None:
        p = N / "english.json"
        _TABLES = json.loads(p.read_text()) if p.exists() else {}
    en = _TABLES.get(table_of(region(at[0], at[1])), {}).get(name.strip())
    return en if differs(name, en) else None


def english_at(name: str | None, at, own: str | None = None) -> str | None:
    """A name's English for our layers' `en` (as web/src/english.ts): its own, else the table's for
    where it is (at: lon, lat); none when it doesn't truly differ."""
    if not name:
        return None
    if differs(name, own):
        return own.strip()
    return english_of(name, at) if at else None


