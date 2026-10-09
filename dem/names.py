#!/usr/bin/env python3
"""English for the map's non-English names: under them in map labels, in parentheses in the app's text.

Where English comes from, in order: OSM's name:en (else, in Japan, its romanised name:ja-Latn /
name:ja_rm); for heritage sites, their English name (UNESCO's) or their English Wikipedia article's
title; else a translation (Claude Haiku, in batches: data/names/tr/TRANSLATORS.md). English counts
only when it truly differs from the name, not just in accents, case, punctuation or spacing
(Montréal, Montreal).

The names: every named place, water body, park or protected area, natural feature, stop or sight,
station, rail or ferry line in OSM (an osmium filter of data/osm/merged.osm.pbf), and the heritage
sites and areas, special places and Indigenous lands; each with its region, by location, which
decides how a name is read (中山 is Nakayama in Japan, Zhongshan in Taiwan): jp Japan, tw Taiwan,
hk Hong Kong, sg Singapore; elsewhere Latin script, with the country as a hint for the translators
(na North America, gb Britain & Ireland, fr France, ib Iberia, pt the Portuguese islands). In
Japan, with the name's kana reading where OSM has one (本町: ほんまち, Honmachi, or ほんちょう,
Honcho), for the translators to romanise from.

A name's English is the one most of its features with English have (else none: a name with
several). A heritage site's own English (UNESCO's, or its Wikipedia article's title when that's
English, not just another French name) is its own, not the name's: the name's only when the name
is the site's alone ("Église" is thousands of churches).

Translated: the names the map shows (TRANSLATE: not OSM's historic features, urban parks, forests,
valleys or capes, which it doesn't label), non-Latin ones without English, and Latin ones without
English that have a generic word in them (Lac, Rivière, Parc, Río, Castillo, Afon…); the rest are
proper names, the same in English.

  filter      data/names/named.osm.pbf: the objects, with the nodes and ways they're made of
  inventory   data/names/inventory.jsonl: per region and name: kind, English and where it's from,
              how many features have it
  batches     the names to translate, 1,000 a batch, commonest first: data/names/tr/<region>-NNN.jsonl
              (numbered on from the finished ones, with the names in those left out)
  check B     a translated batch's output against it (data/names/tr/<batch>.jsonl): one valid line
              per name, in order, English for every non-Latin name
  table       data/names/english.json: per table (jp, tw, hk, sg, latin) name → English: OSM's (most
              features with the name agree) or the translation; and the app's share of it,
              data/build/names-en.json (rail lines: APP_KINDS)
  patch       data/names/name-en.osc.gz: the basemap's kinds of OSM objects without English, with
              the table's as name:en, for the labels archive (Makefile: labels.pmtiles, which the
              map's place, water and park names are drawn from; a lookup table in the map style
              instead stalls MapLibre at this size)

english_at() gives the English our layers carry (layers.py, ferries.py: `en`).

usage: names.py filter | inventory | batches | check <batch> | table | patch [<pbf> <osc>]
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
import unicodedata
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PBF = ROOT / "data" / "osm" / "merged.osm.pbf"
N = ROOT / "data" / "names"
TR = N / "tr"
B = ROOT / "data" / "build"
BATCH = 1000

FILTERS = [
    "nwr/place",
    "nwr/natural=water,bay,strait,cape,peninsula,glacier,spring,beach,valley,mountain_range,ridge,peak,volcano,saddle",
    "nw/waterway=river,canal,waterfall",
    "nwr/boundary=national_park,protected_area",
    "nwr/leisure=nature_reserve,park",
    "nwr/landuse=forest",
    "nwr/railway=station,halt,tram_stop",
    "n/public_transport=station",
    "r/route=train,subway,tram,light_rail,monorail,funicular,ferry",
    "nwr/tourism=viewpoint,picnic_site,attraction,museum",
    "nwr/highway=rest_area,trailhead",
    "nwr/man_made=lighthouse",
    "nwr/historic",
    "nwr/amenity=ferry_terminal",
]

# Latin names worth translating have one of these (French, Spanish, Portuguese, Catalan, Welsh, Irish
# and Scottish Gaelic generic words); English ones (Lake, Fort, Reserve) are left out on purpose.
GENERIC = re.compile(r"(^|[\s\-'’])(" + "|".join("""
lac lacs rivière riviere fleuve ruisseau étang etang baie anse cap pointe île ile îles iles îlot ilot mont monts montagne
montagnes col pic forêt foret parc réserve reserve chute chutes cascade lagune grotte gorge gorges vallée vallee plage
marais château chateau église eglise cathédrale cathedrale abbaye basilique chapelle pont gare musée musee tour phare
moulin
río rio lago laguna embalse sierra monte montes isla islas bahía bahia cabo punta playa parque reserva pico puerto cueva
castillo iglesia catedral monasterio ermita puente estación estacion museo torre faro molino fuente barranco valle
lagoa serra ilha ilhéu ilheu baía praia gruta castelo igreja sé mosteiro convento capela ponte estação estacao museu
farol moinho ribeira vale
riu llac estany muntanya illa badia platja castell església esglesia estació
afon llyn mynydd coed eglwys
sliabh abhainn inis oileán oilean caisleán caislean teampall""".split()) + r")([\s\-'’]|$)", re.I)

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


def kind(t) -> str | None:
    g = t.get
    if g("place"):
        return "place"
    if g("waterway") == "waterfall":
        return "waterfall"
    if g("waterway") or g("natural") in ("water", "bay", "strait", "glacier", "spring", "beach"):
        return "water"
    if g("natural") in ("peak", "volcano", "saddle"):
        return "peak"
    if g("natural") in ("cape", "peninsula", "valley", "mountain_range", "ridge"):
        return "natural"
    if g("boundary") in ("national_park", "protected_area") or g("leisure") == "nature_reserve":
        return "park"
    if g("leisure") == "park":
        return "urban park"
    if g("landuse") == "forest":
        return "forest"
    if g("railway") in ("station", "halt", "tram_stop") or g("public_transport") == "station":
        return "station"
    if g("route") == "ferry":
        return "ferry line"
    if g("route"):
        return "rail line"
    if g("tourism") in ("viewpoint", "picnic_site") or g("highway") in ("rest_area", "trailhead") or g("man_made") == "lighthouse":
        return "stop"
    if g("amenity") == "ferry_terminal":
        return "terminal"
    if g("historic") or g("tourism") in ("attraction", "museum"):
        return "sight"
    return None


# The kinds the map shows by name (the basemap's places, water, rivers and protected areas; our
# stops, stations, lines, terminals and heritage sites).
TRANSLATE = {"place", "water", "park", "peak", "waterfall", "station", "rail line", "ferry line", "stop", "terminal",
             "heritage site"}
READINGS = ("name:ja-Hira", "name:ja_kana", "name:ja-Kana")


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


def filter_() -> None:
    N.mkdir(parents=True, exist_ok=True)
    subprocess.run(["osmium", "tags-filter", "--overwrite", str(PBF), *FILTERS, "-o", str(N / "named.osm.pbf")], check=True)


class Rows:
    """Per (region, name): kinds, English candidates (and where from), sites' own English, kana
    readings, features (and how many have English)."""

    def __init__(self):
        self.kinds: dict[tuple, Counter] = defaultdict(Counter)
        self.en: dict[tuple, Counter] = defaultdict(Counter)
        self.site: dict[tuple, Counter] = defaultdict(Counter)
        self.rd: dict[tuple, Counter] = defaultdict(Counter)
        self.count: Counter = Counter()
        self.with_en: Counter = Counter()

    def add(self, r: str, name: str, k: str, en: str | None, src: str, rd: str | None = None, site: bool = False) -> None:
        key = (r, name)
        self.kinds[key][k] += 1
        self.count[key] += 1
        if rd:
            self.rd[key][rd.strip()] += 1
        if site:
            if en:
                self.site[key][(en, src)] += 1
            return
        if en:
            self.with_en[key] += 1
        if differs(name, en):
            self.en[key][(en.strip(), src)] += 1

    def english(self, key: tuple) -> tuple[str | None, str]:
        """The name's English: most of its features', else (the name its site's alone) the site's."""
        best = self.en[key].most_common(1)
        if best and 2 * best[0][1] >= self.with_en[key]:
            return best[0][0]
        if not best and self.count[key] == 1 and self.site[key]:
            return self.site[key].most_common(1)[0][0]
        return None, ""


def inventory() -> None:
    import osmium

    pbf = str(N / "named.osm.pbf")
    # Pass 1: relations' first way member (their location).
    first_way: dict[int, int] = {}

    class Rel(osmium.SimpleHandler):
        def relation(self, rel):
            if "name" in rel.tags and kind(rel.tags):
                for m in rel.members:
                    if m.type == "w":
                        first_way[rel.id] = m.ref
                        break

    Rel().apply_file(pbf)
    need = set(first_way.values())
    way_loc: dict[int, tuple[float, float]] = {}
    rows = Rows()

    def english(t, r: str) -> tuple[str | None, str]:
        if t.get("name:en"):
            return t.get("name:en"), "osm"
        if r == "jp":
            for k in ("name:ja-Latn", "name:ja_rm"):
                if t.get(k):
                    return t.get(k), "osm-romaji"
        return None, ""

    def take(t, lon: float, lat: float) -> None:
        name, k = t.get("name"), kind(t)
        if not name or not k:
            return
        r = region(lon, lat)
        en, src = english(t, r)
        rd = next((t.get(x) for x in READINGS if t.get(x)), None) if r == "jp" else None
        rows.add(r, name.strip(), k, en, src, rd)

    class Main(osmium.SimpleHandler):
        def node(self, n):
            if "name" in n.tags and n.location.valid():
                take(n.tags, n.location.lon, n.location.lat)

        def way(self, w):
            loc = next(((nd.lon, nd.lat) for nd in w.nodes if nd.location.valid()), None)
            if loc is None:
                return
            if w.id in need:
                way_loc[w.id] = loc
            if "name" in w.tags:
                take(w.tags, *loc)

        def relation(self, rel):
            loc = way_loc.get(first_way.get(rel.id, -1))
            if loc and "name" in rel.tags:
                take(rel.tags, *loc)

    Main().apply_file(pbf, locations=True)
    # Our own layers' names: the heritage sites (with their own English) and areas, special places,
    # Indigenous lands.
    wiki = wiki_titles()
    for f in json.load(open(B / "heritage.json"))["features"]:
        p = f["properties"]
        name, at = (p.get("name") or "").strip(), first_point(f.get("geometry"))
        if name and at:
            en, src = site_english(name, p.get("name_en"), wiki.get(p.get("i")))
            rows.add(region(*at), name, "heritage site", en, src, site=True)
    for src, k in (("heritage-areas", "heritage site"), ("special", "park"), ("indigenous", "park")):
        if (B / f"{src}.json").exists():
            for f in json.load(open(B / f"{src}.json"))["features"]:
                p = f["properties"]
                name, at = (p.get("name") or "").strip(), first_point(f.get("geometry"))
                if name and at:
                    rows.add(region(*at), name, k, p.get("name_en"), "own", site=True)
    with open(N / "inventory.jsonl", "w", encoding="utf-8") as f:
        for key, c in rows.count.most_common():
            r, name = key
            en, src = rows.english(key)
            row = {"r": r, "n": name, "k": rows.kinds[key].most_common(1)[0][0], "en": en, "src": src, "c": c}
            if rows.rd[key]:
                row["rd"] = rows.rd[key].most_common(1)[0][0]
            f.write(json.dumps(row, ensure_ascii=False) + "\n")
    have = sum(1 for key in rows.count if rows.english(key)[0])
    print(f"{len(rows.count)} names ({have} with English); relations placed: {sum(1 for r in first_way if first_way[r] in way_loc)}/{len(first_way)}", file=sys.stderr)


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


def jsonl(p: Path) -> list[dict]:
    return [json.loads(l) for l in open(p, encoding="utf-8")] if p.exists() else []


def translatable(row: dict) -> bool:
    n = row["n"]
    if row["en"] or row["k"] not in TRANSLATE or not any(c.isalpha() for c in n):
        return False
    return not latin(n) or bool(GENERIC.search(n))


def batches() -> None:
    TR.mkdir(parents=True, exist_ok=True)
    given: dict[str, set] = defaultdict(set)
    nxt: dict[str, int] = defaultdict(int)
    for p in sorted(TR.glob("*-[0-9][0-9][0-9].jsonl")):
        r, num = p.stem.rsplit("-", 1)
        if p.with_name(p.stem + ".out.jsonl").exists():
            given[r] |= {x["n"] for x in jsonl(p)}
            nxt[r] = max(nxt[r], int(num) + 1)
        else:
            p.unlink()  # unfinished: made again
    todo: dict[str, list] = defaultdict(list)
    for row in jsonl(N / "inventory.jsonl"):  # (commonest first)
        if translatable(row) and row["n"] not in given[row["r"]]:
            todo[row["r"]].append({"n": row["n"], "k": row["k"], **({"rd": row["rd"]} if row.get("rd") else {})})
    for r, rows in sorted(todo.items()):
        for b in range(0, len(rows), BATCH):
            with open(TR / f"{r}-{nxt[r] + b // BATCH:03d}.jsonl", "w", encoding="utf-8") as f:
                for x in rows[b:b + BATCH]:
                    f.write(json.dumps(x, ensure_ascii=False) + "\n")
        print(f"{r}: {len(rows)} to translate, {(len(rows) + BATCH - 1) // BATCH} batches from {r}-{nxt[r]:03d}", file=sys.stderr)


# What a word-for-word substitution gives (TRANSLATORS.md): an English generic word with the
# original's articles and prepositions still around it ("Lake de la Point", "Bief du Mill"), in the
# original's word order ("Lake Long"), or a church without "of" ("Church Saint-Martin").
_GEN = ("lake|lakes|river|pond|stream|brook|creek|mill|castle|church|chapel|mount|mountain|mountains|pass|wood|woods|"
        "forest|park|bay|island|islands|point|cape|beach|valley|waterfall|falls|station|square|hamlet|tower|bridge|"
        "spring|fountain|abbey|priory|cathedral|basilica|museum|reserve|peak|hill|cave|gorge|marsh|marshes|lagoon|"
        "reservoir|harbour|port|canal|meadow|field|fields|moor|rock|house|manor|farm|monastery|convent")
_ROM = r"de|du|des|d'|d’|la|le|les|l'|l’|à|au|aux|del|dels|da|do|dos|das|el|los|las|y|et|en|di|della"
HALF = re.compile(rf"^({_GEN})\s+({_ROM})(\s|$)|\s({_ROM})\s*(la\s+|le\s+|les\s+|l'|l’)?({_GEN}|clear|black|white|green|red)$", re.I)
ORDER = re.compile(r"^(lake|river|pond|brook|creek|stream|mount|hill|island|bay|castle|mill|wood|forest|pass)\s+("
                   r"long|round|black|green|white|red|blue|yellow|grey|gray|little|big|great|clear|deep|crooked|lost|"
                   r"beaver|trout|grand|small|upper|lower|north|south|east|west|old|new|high|low|dry|cold|hot|dead|bear|"
                   r"wolf|fox|duck|swan|eagle|pike|pine|pines|birch|cedar|maple|oak|stone|sand|mud|narrow|wide|hidden|"
                   r"salmon|moose|caribou|otter|loon|heron|rat|rats|spruce|castor|vert|noir|rond|blanc|rouge|perdu)$", re.I)
SAINT = re.compile(r"^(church|chapel|cathedral|basilica|abbey|priory|castle|monastery|convent|hermitage)\s+"
                   r"(saint|sainte|san|santa|santo|são|sant|st\.?|notre)\b", re.I)
MACRON = re.compile("[āēīōūâêîôûĀĒĪŌŪÂÊÎÔÛ]")
# Kunrei-shiki where Hepburn is wanted (Otubo, Isikawa, Hukuoka, Kozima), in a romanised word
# (not the English words around it: Station, Institute).
KUNREI = re.compile(r"(?<!s)tu|ti|si|(?<![sc])hu|zi|sy|ty|zy", re.I)
ENGLISH_WORDS = set(_GEN.split("|")) | set("""
line lines shrine temple district village town city ward street road route trail observatory viewpoint observation
deck platform garden gardens hall gate school university institute site ruins tomb tombs mound dam tunnel spring
springs hot plateau ridge wetland lighthouse memorial monument building market center centre national prefectural
quasi natural historic history old new upper lower east west north south central first second third main branch
drain channel ditch pool lagoon estuary strait sea ocean inlet cove sound headland peninsula summit highland
highlands shopping station stations street avenue sanctuary cemetery pagoda residence house district's""".split())
SAINT_OUT = re.compile(r"\b(?:Saint|St\.?)\s+([A-Z][\w’']+)")
SAINT_IN = re.compile(r"\b(?:Saint|Sainte|San|Santa|Santo|São|Sant|Sankt|St)[\s\-]+([^\W\d_][\w’']*)", re.I)
ESTABLISHED = {"lawrence", "john", "quebec", "montreal"}


def strip_accents(w: str) -> str:
    return "".join(c for c in unicodedata.normalize("NFKD", w) if not unicodedata.combining(c))


def _words(s: str) -> set[str]:
    return set(re.findall(r"[a-z]{4,}", strip_accents(s.lower())))


def drift(names: list[str], ens: list[str | None]) -> list[str]:
    """Stretches of 100 lines whose English belongs to the names a few lines away (a translator that
    lost its place): Latin-script names keep their proper part, so an English line shares a word
    with its own name far more often than with a neighbour's."""
    bad = []
    for w in range(0, len(names), 100):
        idx = [i for i in range(w, min(w + 100, len(names))) if ens[i] and latin(names[i])]
        hits = {k: sum(1 for i in idx if 0 <= i + k < len(names) and _words(ens[i]) & _words(names[i + k]))
                for k in range(-5, 6)}
        k = max(hits, key=hits.get)
        if k and hits[k] >= 5 and hits[k] > 2 * hits[0]:
            bad.append(f"lines {w + 1}–{w + 100}: the English is {abs(k)} line{'s' * (abs(k) > 1)} "
                       f"{'ahead of' if k > 0 else 'behind'} its names (redo them, each on its own name's line)")
    return bad


def check(batch: str) -> None:
    src = Path(batch)
    out = src.with_name(src.stem + ".out.jsonl")
    region_ = src.stem.split("-")[0]
    rows = jsonl(src)
    names = [x["n"] for x in rows]
    bad, got = [], []
    for i, line in enumerate(open(out, encoding="utf-8") if out.exists() else [], 1):
        try:
            x = json.loads(line)
            assert isinstance(x.get("n"), str) and (x.get("en") is None or isinstance(x["en"], str))
            got.append(x)
        except (ValueError, AssertionError):
            bad.append(f"line {i}: not a {{\"n\", \"en\"}} JSON line: {line.strip()[:80]}")
    for i, (n, x) in enumerate(zip(names, got), 1):
        en = (x.get("en") or "").strip()
        if x["n"] != n:
            bad.append(f"line {i}: n is {x['n']!r}, the input's is {n!r} (a line missing or out of order?)")
            break
        if not en:
            if not latin(n):
                bad.append(f"line {i}: {n!r} needs an English name (it isn't in Latin script)")
            continue
        if not latin(en) or any(unicodedata.name(c, "").startswith(("CJK", "HIRAGANA", "KATAKANA")) for c in en):
            bad.append(f"line {i}: {en!r} still has characters to romanise")
        elif not differs(n, en):
            bad.append(f"line {i}: {en!r} is the name itself: write null")
        elif HALF.search(en):
            bad.append(f"line {i}: {en!r} is half translated: put it all in English word order ({n!r})")
        elif ORDER.search(en):
            bad.append(f"line {i}: {en!r} is in the original's word order ({n!r}): English puts the describing word first")
        elif SAINT.search(en):
            bad.append(f"line {i}: {en!r}: write \"Church of Saint-…\" (or \"St …'s Church\")")
        elif region_ == "jp" and MACRON.search(en):
            bad.append(f"line {i}: {en!r}: no macrons (Hepburn without them: Tokyo, Ryukyu)")
        elif region_ == "jp" and any(KUNREI.search(w) for w in re.findall(r"[A-Za-z]+", en) if w.lower() not in ENGLISH_WORDS):
            bad.append(f"line {i}: {en!r}: Hepburn spellings, please (tsu, shi, chi, fu, ji: Otsubo, Ishikawa)")
        elif latin(n) and (lost := [w for w in re.findall(r"[^\W\d_]+", n)[1:] if strip_accents(w) != w and not GENERIC.search(w)
                                    and re.search(rf"\b{re.escape(strip_accents(w))}\b", en)
                                    and strip_accents(w).lower() not in ESTABLISHED]):
            bad.append(f"line {i}: {en!r}: keep the accents in names as written ({', '.join(lost)})")
        elif latin(n) and [m for m in SAINT_OUT.findall(en)
                           if strip_accents(m).lower() not in {strip_accents(x).lower() for x in SAINT_IN.findall(n)}
                           and m.lower() not in ESTABLISHED]:
            bad.append(f"line {i}: {en!r}: keep the saint's name as written ({n!r}: Saint-Pierre, not Saint Peter)")
    if len(got) != len(names):
        bad.append(f"{len(got)} output lines for {len(names)} names")
    elif not bad:
        bad += drift(names, [x.get("en") for x in got])
    # Nulls where English was wanted (translators that gave up and nulled the rest):
    # a Latin-script name in a batch has a generic word English translates, so only places keep
    # theirs, and few others are English already (at most 14 % in a finished batch). Not in
    # Britain and Ireland, where most are (Loch Ness, Kinder Scout National Nature Reserve).
    if len(got) == len(names) and region_ != "gb":
        cand = [i for i, x in enumerate(rows) if x.get("k") != "place" and latin(x["n"])]
        empty = [i for i in cand if not (got[i].get("en") or "").strip()]
        if len(cand) >= 10 and len(empty) > 0.2 * len(cand):
            bad.append(f"{len(empty)} of the {len(cand)} names that aren't places are null ("
                       + ", ".join(repr(names[i]) for i in empty[:4]) + " …): English leaves few of them alone, "
                       "so give each its English (Río Urdiales: Urdiales River, Pico Bajero: Bajero Peak)")
    print("\n".join(bad[:40]) + (f"\n… and {len(bad) - 40} more" if len(bad) > 40 else "") if bad
          else f"{len(got)} lines, all good", file=sys.stderr)
    sys.exit(1 if bad else 0)


# The app looks up only the names nothing it draws carries English for: rail lines (named from its
# own rail data). The basemap's names get theirs written into the OSM data its labels are built
# from (patch), our layers theirs baked in (english_at).
APP_KINDS = {"rail line"}
# The kinds the basemap labels (places, water, rivers, protected areas): their English goes into
# the labels archive (patch).
BASEMAP_KINDS = {"place", "water", "park"}


def table() -> None:
    tr: dict[str, dict[str, str]] = defaultdict(dict)
    for p in sorted(TR.glob("*-[0-9][0-9][0-9].out.jsonl")):
        r = p.name.split("-", 1)[0]
        src, out = jsonl(p.with_name(p.name.replace(".out", ""))), jsonl(p)
        # Line for line when they match up (a name copied back not quite as given still counts).
        pairs = zip((x["n"] for x in src), out) if len(src) == len(out) else ((x.get("n"), x) for x in out)
        for n, x in pairs:
            if n and isinstance(x.get("en"), str) and x["en"].strip():
                tr[r][n] = x["en"].strip()
    full: dict[str, dict[str, str]] = defaultdict(dict)
    app: dict[str, dict[str, str]] = defaultdict(dict)
    n_osm = n_tr = 0
    for row in jsonl(N / "inventory.jsonl"):  # (commonest first: its English wins)
        t, name = table_of(row["r"]), row["n"]
        if name in full[t]:
            continue
        en = row["en"]
        if en:
            n_osm += 1
        else:
            en = tr[row["r"]].get(name)
            if not differs(name, en):
                continue
            n_tr += 1
        full[t][name] = en
        if row["k"] in APP_KINDS:
            app[t][name] = en
    (N / "english.json").write_text(json.dumps(full, ensure_ascii=False, separators=(",", ":")))
    B.mkdir(parents=True, exist_ok=True)
    (B / "names-en.json").write_text(json.dumps(app, ensure_ascii=False, separators=(",", ":")))
    print(f"english.json: {n_osm} from OSM and the heritage registers, {n_tr} translated ("
          + ", ".join(f"{t} {len(v)}" for t, v in sorted(full.items())) + f"); names-en.json: {sum(map(len, app.values()))} rail lines",
          file=sys.stderr)


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


def patch(pbf_in: str | None = None, osc_out: str | None = None) -> None:
    """data/names/name-en.osc.gz: OSM's places, water and protected areas (BASEMAP_KINDS) that have no
    English of their own, with the table's as name:en (a version on, so osmium apply-changes takes
    them): applied to the named objects, they give the labels archive (Makefile)."""
    import osmium

    pbf = pbf_in or str(N / "named.osm.pbf")
    first_way: dict[int, int] = {}

    class Rel(osmium.SimpleHandler):
        def relation(self, rel):
            if "name" in rel.tags and "name:en" not in rel.tags and kind(rel.tags) in BASEMAP_KINDS:
                for m in rel.members:
                    if m.type == "w":
                        first_way[rel.id] = m.ref
                        break

    Rel().apply_file(pbf)
    need = set(first_way.values())
    way_loc: dict[int, tuple[float, float]] = {}
    out = Path(osc_out) if osc_out else N / "name-en.osc.gz"
    tmp = out.with_name(out.name.replace(".osc.gz", ".new.osc.gz"))
    tmp.unlink(missing_ok=True)
    writer = osmium.SimpleWriter(str(tmp))
    counts: Counter = Counter()

    def english(o, at) -> dict | None:
        t = o.tags
        if "name" not in t or "name:en" in t or kind(t) not in BASEMAP_KINDS or at is None:
            return None
        en = english_of(t["name"], at)
        if not en:
            return None
        counts[kind(t)] += 1
        tags = {tg.k: tg.v for tg in t}
        tags["name:en"] = en
        return tags

    class Main(osmium.SimpleHandler):
        def node(self, n):
            tags = english(n, (n.location.lon, n.location.lat) if n.location.valid() else None)
            if tags:
                writer.add_node(n.replace(tags=tags, version=n.version + 1))

        def way(self, w):
            loc = next(((nd.lon, nd.lat) for nd in w.nodes if nd.location.valid()), None)
            if loc and w.id in need:
                way_loc[w.id] = loc
            tags = english(w, loc)
            if tags:
                writer.add_way(w.replace(tags=tags, version=w.version + 1))

        def relation(self, rel):
            tags = english(rel, way_loc.get(first_way.get(rel.id, -1)))
            if tags:
                writer.add_relation(rel.replace(tags=tags, version=rel.version + 1))

    Main().apply_file(pbf, locations=True)
    writer.close()
    tmp.replace(out)
    print(f"name-en.osc.gz: English for {sum(counts.values())} objects ({dict(counts)})", file=sys.stderr)


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "check" and len(sys.argv) > 2:
        check(sys.argv[2])
    elif cmd == "patch" and len(sys.argv) > 3:
        patch(sys.argv[2], sys.argv[3])
    else:
        {"filter": filter_, "inventory": inventory, "batches": batches, "table": table, "patch": patch}.get(
            cmd, lambda: print(__doc__, file=sys.stderr))()
