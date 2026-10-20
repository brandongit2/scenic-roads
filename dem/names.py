"""English for the names our layers carry (`en`: layers.py, ferries.py): a thing's own English,
when it truly differs from the name, not just in accents, case, punctuation or spacing (Montréal,
Montreal). A heritage site's own English is UNESCO's (its register's `name_en`), else its English
Wikipedia article's title when that's English. Any other English is the map's server's, from the
translations' live tables as it serves the layers (crates/names, docs/names.md): the `en` written
here is the thing's own, which the server shows first; a name with neither goes on the
translations' to-do list (pipeline::namestodo).
"""
from __future__ import annotations

import json
import re
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


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


def english_at(name: str | None, own: str | None) -> str | None:
    """A name's English for our layers' `en`: its own, when it truly differs; else none (the map's
    server gives it its translation where it is)."""
    if not name:
        return None
    return own.strip() if differs(name, own) else None
