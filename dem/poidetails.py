#!/usr/bin/env python3
"""Details for the stops & sights (pois.json): the OSM tags worth showing, plus Wikidata facts.

Each POI in data/build/pois.json is matched to its OSM feature in data/poi/pois.geojsonseq
(osmium export of the POI tags, see the Makefile): the nearest feature of the same kind within
60 m, a same-name one preferred. Kept per kind:
  peaks        tagged prominence / isolation, UK hill lists (Munro, Corbett, Graham, Marilyn …),
               SOTA reference, summit cross / register
  waterfalls   height, width, intermittent
  lighthouses  light character, colour, period, range and focal height (seamark tags), tower
               height, first lit, operator, heritage status
  viewpoints   direction, tower type
  rest stops   toilets, drinking water, shelter, tables, barbecue, fee, opening hours
  trailheads   the same amenities
  covered bridges  length, structure, date built
and for all: OSM id, description, website, Wikipedia. POIs with a wikidata tag are looked up on
QLever's Wikidata endpoint: short description, English article, height, elevation, prominence,
isolation, discharge (flow), inception, water body (P206), mountain range (P4552).

Summits tagged natural=peak as well as tourism=viewpoint (Mont Blanc) were extracted as
viewpoints; they become peaks here (pois.json is rewritten, same order, each feature gaining its
index as `i`, which the server's /api/detail/poi/{i} uses).

Writes data/build/pois.json (in place) and data/build/details-poi.jsonl.

usage: poidetails.py
"""
from __future__ import annotations

import json
import math
import re
import sys
from collections import defaultdict
from pathlib import Path

from heritagewd import sparql, val

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
P = ROOT / "data" / "poi"

KIND_TAGS = {
    "peak": lambda t: t.get("natural") in ("peak", "volcano"),
    "waterfall": lambda t: t.get("waterway") == "waterfall" or t.get("natural") == "waterfall",
    "lighthouse": lambda t: t.get("man_made") == "lighthouse",
    "viewpoint": lambda t: t.get("tourism") == "viewpoint",
    "picnic_site": lambda t: t.get("tourism") == "picnic_site" or t.get("leisure") == "picnic_table",
    "rest_area": lambda t: t.get("highway") in ("rest_area", "services"),
    "trailhead": lambda t: t.get("highway") == "trailhead",
    "covered_bridge": lambda t: t.get("bridge") == "covered",
}
COMMON = ["description", "website", "wikipedia", "wikidata", "operator", "access", "fee", "opening_hours", "start_date", "heritage", "alt_name", "name:en"]
KEEP = {
    "peak": ["prominence", "isolation", "munro", "corbett", "graham", "donald", "marilyn", "hewitt", "wainwright", "nuttall",
             "communication:amateur_radio:sota", "summit:cross", "summit:register", "volcano:status", "volcano:type", "natural"],
    "waterfall": ["height", "width", "intermittent", "seasonal"],
    "lighthouse": ["height", "seamark:light:character", "seamark:light:colour", "seamark:light:period", "seamark:light:range",
                   "seamark:light:height", "seamark:light:sequence", "seamark:light:reference", "seamark:name", "building:colour",
                   "tower:type", "historic", "heritage:operator", "seamark:light:1:character", "seamark:light:1:colour",
                   "seamark:light:1:period", "seamark:light:1:range", "seamark:light:1:height"],
    "viewpoint": ["direction", "tower:type", "height", "ele", "man_made"],
    "picnic_site": ["toilets", "drinking_water", "shelter", "bench", "picnic_table", "fireplace", "bbq", "covered", "capacity"],
    "rest_area": ["toilets", "drinking_water", "shelter", "picnic_table", "bench", "fuel", "restaurant", "shop", "wheelchair", "capacity"],
    "trailhead": ["toilets", "drinking_water", "parking", "capacity", "route_ref", "hiking", "shelter"],
    "covered_bridge": ["bridge:structure", "bridge:name", "material", "historic", "bridge:ref", "layer"],
}
WD_BATCH = 250


def centre(g: dict) -> tuple[float, float] | None:
    t, c = g["type"], g["coordinates"]
    if t == "Point":
        return c[0], c[1]
    pts = c if t == "LineString" else c[0] if t == "Polygon" else c[0][0] if t == "MultiPolygon" else None
    if not pts:
        return None
    return sum(p[0] for p in pts) / len(pts), sum(p[1] for p in pts) / len(pts)


def line_km(g: dict) -> float:
    if g["type"] != "LineString":
        return 0.0
    c = g["coordinates"]
    s = 0.0
    for (x0, y0), (x1, y1) in zip(c, c[1:]):
        kx = 111.32 * math.cos(math.radians((y0 + y1) / 2))
        s += math.hypot((x1 - x0) * kx, (y1 - y0) * 110.57)
    return s


def norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "", (s or "").lower())


def wikidata(qids: list[str]) -> dict[str, dict]:
    out: dict[str, dict] = {}
    q_props = {
        "height": "P2048", "elevation": "P2044", "prominence": "P2660", "isolation": "P2659", "discharge": "P2225", "focal": "P2923",
    }
    for k in range(0, len(qids), WD_BATCH):
        chunk = qids[k:k + WD_BATCH]
        values = " ".join(f"wd:{q}" for q in chunk)
        num = "\n".join(
            f"OPTIONAL {{ ?item p:{p} ?st_{n} . ?st_{n} psn:{p} ?nv_{n} . ?nv_{n} wikibase:quantityAmount ?v_{n}0 }}" for n, p in q_props.items())
        q = f"""PREFIX psn: <http://www.wikidata.org/prop/statement/value-normalized/> PREFIX p: <http://www.wikidata.org/prop/>
        SELECT ?item (SAMPLE(?sl0) AS ?sl) (SAMPLE(?den) AS ?d_en) (SAMPLE(?dloc) AS ?d_loc) (SAMPLE(?wen) AS ?w_en) (SAMPLE(?inc0) AS ?inc)
          {" ".join(f"(MAX(?v_{n}0) AS ?v_{n})" for n in q_props)}
          (GROUP_CONCAT(DISTINCT ?waterL; SEPARATOR="|") AS ?water) (GROUP_CONCAT(DISTINCT ?rangeL; SEPARATOR="|") AS ?range) WHERE {{
          VALUES ?item {{ {values} }}
          OPTIONAL {{ ?item wikibase:sitelinks ?sl0 }}
          OPTIONAL {{ ?item schema:description ?den FILTER(LANG(?den) = "en") }}
          OPTIONAL {{ ?item schema:description ?dloc FILTER(LANG(?dloc) IN ("fr", "es", "pt", "ca")) }}
          OPTIONAL {{ ?a schema:about ?item ; schema:isPartOf <https://en.wikipedia.org/> ; schema:name ?wen }}
          OPTIONAL {{ ?item wdt:P571 ?inc0 }}
          {num}
          OPTIONAL {{ ?item wdt:P206 ?w . ?w rdfs:label ?waterL FILTER(LANG(?waterL) = "en") }}
          OPTIONAL {{ ?item wdt:P4552 ?r . ?r rdfs:label ?rangeL FILTER(LANG(?rangeL) = "en") }}
        }} GROUP BY ?item"""
        for b in sparql(q):
            qid = val(b, "item").rsplit("/", 1)[1]
            rec = {"sl": int(val(b, "sl") or 0)}
            for k2 in ("d_en", "d_loc", "w_en", "water", "range"):
                if val(b, k2):
                    rec[k2] = val(b, k2)
            if val(b, "inc"):
                rec["inception"] = val(b, "inc")[:10]
            for n in q_props:
                if val(b, f"v_{n}"):
                    rec[n] = round(float(val(b, f"v_{n}")), 2)
            out[qid] = rec
        print(f"  wikidata {min(k + WD_BATCH, len(qids))}/{len(qids)}", file=sys.stderr, flush=True)
    return out


def main():
    pois = json.load(open(B / "pois.json"))
    feats = pois["features"]
    # OSM features by kind, on a ~1 km grid.
    grid: dict[tuple[str, int, int], list[tuple[float, float, dict, str]]] = defaultdict(list)
    n_osm = 0
    for line in open(P / "pois.geojsonseq", encoding="utf-8"):
        d = json.loads(line.lstrip("\x1e"))
        c = centre(d["geometry"])
        if not c:
            continue
        t = d["properties"]
        osm = t.get("@type", "node")[0] + str(t.get("@id"))
        for k, f in KIND_TAGS.items():
            if f(t):
                t["_km"] = line_km(d["geometry"])
                grid[(k, int(c[0] * 100), int(c[1] * 100))].append((c[0], c[1], t, osm))
                n_osm += 1
    print(f"{n_osm} OSM features", file=sys.stderr)

    def find(kinds: list[str], x: float, y: float, name: str):
        best, bd = None, 1e9
        kx = 111_320 * math.cos(math.radians(y))
        for k in kinds:
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    for (ox, oy, t, osm) in grid.get((k, int(x * 100) + dx, int(y * 100) + dy), ()):
                        d = math.hypot((ox - x) * kx, (oy - y) * 110_574)
                        if name and norm(t.get("name", "")) == norm(name):
                            d *= 0.3
                        if d < bd:
                            best, bd = (t, osm, k), d
        return best if bd <= 60 else None

    details = []
    rekind = 0
    qids = set()
    for i, f in enumerate(feats):
        p = f["properties"]
        p["i"] = i
        kind = p["kind"]
        x, y = f["geometry"]["coordinates"][:2]
        kinds = [kind] + (["peak"] if kind == "viewpoint" else [])
        m = find(kinds, x, y, p.get("name", ""))
        if not m:
            continue
        t, osm, mk = m
        # A summit tagged as a viewpoint too: a peak (with its view noted).
        if kind == "viewpoint" and KIND_TAGS["peak"](t):
            p["kind"] = kind = "peak"
            rekind += 1
        rec: dict = {"i": i, "osm": osm}
        for k in COMMON + KEEP.get(kind, []):
            v = t.get(k)
            if v not in (None, ""):
                rec[k] = v
        if kind == "covered_bridge" and t.get("_km"):
            rec["length_m"] = round(t["_km"] * 1000)
        if kind == "peak" and t.get("tourism") == "viewpoint":
            rec["viewpoint"] = "yes"
        wd = t.get("wikidata", "")
        if re.fullmatch(r"Q\d+", wd):
            qids.add(wd)
        details.append(rec)
    print(f"{len(details)} of {len(feats)} POIs matched to OSM; {rekind} viewpoints are summits (now peaks); {len(qids)} with Wikidata", file=sys.stderr)

    cache = P / "wikidata.json"
    have = json.loads(cache.read_text()) if cache.exists() else {}
    todo = sorted(q for q in qids if q not in have)
    if todo:
        have.update(wikidata(todo))
        cache.write_text(json.dumps(have, ensure_ascii=False))
    n_wd = 0
    # Written descriptions (desctargets.py, WRITERS.md) by Wikidata item, with their source article.
    import heritagedetails
    long = heritagedetails.long_descriptions()
    src = {}
    ex_path = ROOT / "data" / "heritage" / "desc" / "extracts.jsonl"
    if ex_path.exists():
        for line in open(ex_path, encoding="utf-8"):
            r = json.loads(line)
            src[r["qid"]] = {"lang": r["lang"], "title": r["title"]}
    n_long = 0
    for rec in details:
        w = have.get(rec.get("wikidata", ""))
        if w:
            rec["wd"] = w
            n_wd += 1
        q = rec.get("wikidata", "").split(";")[0].strip()
        if q in long and (q in src or long[q].get("src")):
            rec["long"] = long[q]["long"]
            rec["long_src"] = {"refs": long[q]["src"]} if long[q].get("src") else src[q]
            n_long += 1

    tmp = B / "pois.json.tmp"
    tmp.write_text(json.dumps(pois, ensure_ascii=False, separators=(",", ":")))
    tmp.rename(B / "pois.json")
    with open(B / "details-poi.jsonl", "w") as f:
        for rec in details:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    print(f"details-poi.jsonl: {len(details)} POIs, {n_wd} with Wikidata facts, {n_long} with a written description", file=sys.stderr)


if __name__ == "__main__":
    main()
