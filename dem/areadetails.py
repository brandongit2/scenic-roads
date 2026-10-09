#!/usr/bin/env python3
"""Details for the highlighted areas: land area for every polygon, and for parks, protected areas
and Indigenous lands their OSM tags and Wikidata facts.

  heritage-areas.json, special.json, indigenous.json (data/build): each feature gains its index
      as `i` (by which the overlays job joins them), and details-{harea,special,indigenous}.jsonl
      get its area (km², spherical) — Indigenous lands also the tags of the OSM boundary with
      the same name (data/areas/areas.geojsonseq).
  parks (drawn from the basemap's park layer, which has no ids): details-park.jsonl, one record
      per named protected area in data/areas/areas.geojsonseq — name, bounding box, area,
      protection title / class, operator, owner, access, start date, website, description, and
      Wikidata (inception, short description, English article, area, visitors) where tagged. The
      server finds the one a hovered park is by name and position.

usage: areadetails.py
"""
from __future__ import annotations

import json
import math
import re
import sys
from pathlib import Path

from heritagewd import sparql, val

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
AREAS = ROOT / "data" / "areas" / "areas.geojsonseq"
R_EARTH = 6_371_008.8

PARK_TAGS = ["protection_title", "protect_class", "designation", "operator", "owner", "ownership", "access", "start_date",
             "website", "description", "wikipedia", "wikidata", "leisure", "boundary", "iucn_level", "opening_hours", "fee"]


def ring_area(ring: list) -> float:
    """Spherical area of a lon/lat ring, m² (Chamberlain & Duquette)."""
    s = 0.0
    for (x0, y0), (x1, y1) in zip(ring, ring[1:] + ring[:1]):
        s += math.radians(x1 - x0) * (2 + math.sin(math.radians(y0)) + math.sin(math.radians(y1)))
    return abs(s) * R_EARTH * R_EARTH / 2


def area_km2(g: dict | None) -> float:
    if not g:
        return 0.0
    polys = [g["coordinates"]] if g["type"] == "Polygon" else g["coordinates"] if g["type"] == "MultiPolygon" else []
    a = 0.0
    for p in polys:
        if not p:
            continue
        a += ring_area([tuple(c[:2]) for c in p[0]]) - sum(ring_area([tuple(c[:2]) for c in h]) for h in p[1:])
    return round(max(a, 0) / 1e6, 3)


def bbox(g: dict) -> list[float]:
    xs, ys = [], []

    def walk(c):
        if isinstance(c[0], (int, float)):
            xs.append(c[0])
            ys.append(c[1])
        else:
            for x in c:
                walk(x)
    walk(g["coordinates"])
    return [round(min(xs), 5), round(min(ys), 5), round(max(xs), 5), round(max(ys), 5)]


def norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "", (s or "").lower())


def wd_parks(qids: list[str], save=None) -> dict[str, dict]:
    out = {}
    for k in range(0, len(qids), 250):
        if save and k:
            save(out)
        values = " ".join(f"wd:{q}" for q in qids[k:k + 250])
        q = f"""PREFIX psn: <http://www.wikidata.org/prop/statement/value-normalized/> PREFIX p: <http://www.wikidata.org/prop/>
        SELECT ?item (SAMPLE(?den) AS ?d_en) (SAMPLE(?dloc) AS ?d_loc) (SAMPLE(?wen) AS ?w_en) (SAMPLE(?inc0) AS ?inc)
          (MAX(?a0) AS ?area) (MAX(?v0) AS ?visitors) (GROUP_CONCAT(DISTINCT ?opL; SEPARATOR="|") AS ?op)
          (GROUP_CONCAT(DISTINCT ?instL; SEPARATOR="|") AS ?inst) WHERE {{
          VALUES ?item {{ {values} }}
          OPTIONAL {{ ?item schema:description ?den FILTER(LANG(?den) = "en") }}
          OPTIONAL {{ ?item schema:description ?dloc FILTER(LANG(?dloc) IN ("fr", "es", "pt", "ca")) }}
          OPTIONAL {{ ?a schema:about ?item ; schema:isPartOf <https://en.wikipedia.org/> ; schema:name ?wen }}
          OPTIONAL {{ ?item wdt:P571 ?inc0 }}
          OPTIONAL {{ ?item p:P2046 ?st . ?st psn:P2046 ?nv . ?nv wikibase:quantityAmount ?a0 }}
          OPTIONAL {{ ?item wdt:P1174 ?v0 }}
          OPTIONAL {{ ?item wdt:P137 ?o . ?o rdfs:label ?opL FILTER(LANG(?opL) = "en") }}
          OPTIONAL {{ ?item wdt:P31 ?i . ?i rdfs:label ?instL FILTER(LANG(?instL) = "en") }}
        }} GROUP BY ?item"""
        for b in sparql(q):
            rec = {}
            for key in ("d_en", "d_loc", "w_en", "op", "inst"):
                if val(b, key):
                    rec[key] = val(b, key)
            if val(b, "inc"):
                rec["inception"] = val(b, "inc")[:10]
            if val(b, "area"):
                rec["area_km2"] = round(float(val(b, "area")) / 1e6, 2)  # normalised to m²
            if val(b, "visitors"):
                rec["visitors"] = int(float(val(b, "visitors")))
            out[val(b, "item").rsplit("/", 1)[1]] = rec
        print(f"  wikidata {min(k + 250, len(qids))}/{len(qids)}", file=sys.stderr, flush=True)
    return out


def index_layer(name: str, layer: str, extra=None) -> None:
    path = B / name
    fc = json.load(open(path))
    with open(B / f"details-{layer}.jsonl", "w") as out:
        for i, f in enumerate(fc["features"]):
            f["properties"]["i"] = i
            rec = {"i": i, "area_km2": area_km2(f.get("geometry"))}
            if extra:
                rec.update(extra(f) or {})
            out.write(json.dumps(rec, ensure_ascii=False) + "\n")
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(fc, ensure_ascii=False, separators=(",", ":")))
    tmp.rename(path)
    print(f"{name}: {len(fc['features'])} areas", file=sys.stderr)


def main():
    # Protected areas and Indigenous lands from OSM.
    parks, aboriginal = [], {}
    qids = set()
    for line in open(AREAS, encoding="utf-8"):
        d = json.loads(line.lstrip("\x1e"))
        t = d["properties"]
        name = t.get("name") or t.get("name:en") or ""
        if not name:
            continue
        rec = {"name": name, "osm": t.get("@type", "way")[0] + str(t.get("@id")), "bbox": bbox(d["geometry"]), "area_km2": area_km2(d["geometry"])}
        for k in PARK_TAGS:
            if t.get(k):
                rec[k] = t[k]
        if re.fullmatch(r"Q\d+", t.get("wikidata", "")):
            qids.add(t["wikidata"])
        if t.get("boundary") == "aboriginal_lands":
            aboriginal.setdefault(norm(name), []).append(rec)
        else:
            parks.append(rec)
    cache = ROOT / "data" / "areas" / "wikidata.json"
    have = json.loads(cache.read_text()) if cache.exists() else {}
    todo = sorted(q for q in qids if q not in have)
    if todo:
        # Saved after every batch, so a rerun picks up where a failed one stopped.
        have.update(wd_parks(todo, save=lambda part: cache.write_text(json.dumps({**have, **part}, ensure_ascii=False))))
        cache.write_text(json.dumps(have, ensure_ascii=False))
    for rec in parks + [r for rs in aboriginal.values() for r in rs]:
        w = have.get(rec.get("wikidata", ""))
        if w:
            rec["wd"] = w
    with open(B / "details-park.jsonl", "w") as out:
        for rec in parks:
            out.write(json.dumps(rec, ensure_ascii=False) + "\n")
    print(f"details-park.jsonl: {len(parks)} named protected areas ({sum(1 for r in parks if 'wd' in r)} with Wikidata)", file=sys.stderr)

    index_layer("heritage-areas.json", "harea")
    index_layer("special.json", "special")

    def indig(f):
        rs = aboriginal.get(norm(f["properties"].get("name", "")), [])
        if not rs:
            return None
        r = max(rs, key=lambda r: r["area_km2"])
        return {k: v for k, v in r.items() if k not in ("name", "bbox", "area_km2")}
    index_layer("indigenous.json", "indigenous", indig)


if __name__ == "__main__":
    main()
