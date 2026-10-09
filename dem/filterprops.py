#!/usr/bin/env python3
"""The numbers the Stops & sights filters work on, stamped onto the map layers' features (the
details stay on the server; these few fields are what MapLibre filters and the in-view counts
need). Same preferences as the popups (web/src/details.ts): tagged values first, then Wikidata,
then computed.

  pois.json       peaks: pr (prominence m), is (isolation km); waterfalls: h (height m);
                  lighthouses: h (tower m), fh (focal height m), rg (range nmi), y (first lit);
                  covered bridges: len (m), y (built); viewpoints: pan (panoramic), tw (tower);
                  rest areas, picnic sites, trailheads: fac (bits: 1 toilets, 2 drinking water,
                  4 shelter, 8 tables, 16 barbecue, 32 parking)
  heritage.json   by (year built), dy (year designated), sl (Wikipedia/Wikimedia sitelinks),
                  wp (has a Wikipedia article), t (kind of designation, heritagetiers.py)
  heritage-areas.json, special.json, indigenous.json   a (area km²)

usage: filterprops.py   (after heritagedetails.py and areadetails.py)
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

from heritagetiers import tier

B = Path(__file__).resolve().parent.parent / "data" / "build"


def num(v) -> float | None:
    if v is None or v == "":
        return None
    m = re.search(r"-?\d+(?:[.,]\d+)?", str(v))
    return float(m.group(0).replace(",", ".")) if m else None


def year(v) -> int | None:
    m = re.search(r"-?\d{3,4}", str(v or ""))
    return int(m.group(0)) if m else None


def yes(v) -> bool:
    return v in ("yes", "designated", True)


def jsonl(name: str) -> dict[int, dict]:
    out = {}
    p = B / name
    if p.exists():
        for line in open(p, encoding="utf-8"):
            r = json.loads(line)
            out[r["i"]] = r
    return out


def write(name: str, fc: dict) -> None:
    tmp = B / (name + ".tmp")
    tmp.write_text(json.dumps(fc, ensure_ascii=False, separators=(",", ":")))
    tmp.rename(B / name)


def pois() -> None:
    fc = json.load(open(B / "pois.json"))
    det = jsonl("details-poi.jsonl")
    peaks = {o["i"]: o for o in json.load(open(B / "peaks.json"))} if (B / "peaks.json").exists() else {}
    n = 0
    for f in fc["features"]:
        p = f["properties"]
        i, k = p.get("i"), p.get("kind")
        d = det.get(i, {})
        wd = d.get("wd", {})
        for key in ("pr", "is", "h", "fh", "rg", "y", "len", "pan", "tw", "fac"):
            p.pop(key, None)
        if k == "peak":
            pk = peaks.get(i)
            pr = num(d.get("prominence")) if num(d.get("prominence")) is not None else wd.get("prominence", pk["p"] if pk else None)
            iso = wd["isolation"] / 1000 if "isolation" in wd else (pk["iso"] if pk else None)
            if pr is not None:
                p["pr"] = round(pr)
            if iso is not None:
                p["is"] = round(iso, 2)
        elif k == "waterfall":
            h = num(d.get("height")) if num(d.get("height")) is not None else wd.get("height")
            if h is not None:
                p["h"] = round(h, 1)
        elif k == "lighthouse":
            h = num(d.get("height")) if num(d.get("height")) is not None else wd.get("height")
            fh = num(d.get("seamark:light:height") or d.get("seamark:light:1:height"))
            fh = fh if fh is not None else wd.get("focal")
            rg = num(d.get("seamark:light:range") or d.get("seamark:light:1:range"))
            y = year(d.get("start_date") or wd.get("inception"))
            for key, v in (("h", h), ("fh", fh), ("rg", rg), ("y", y)):
                if v is not None:
                    p[key] = round(v, 1) if isinstance(v, float) else v
        elif k == "covered_bridge":
            if d.get("length_m"):
                p["len"] = d["length_m"]
            y = year(d.get("start_date") or wd.get("inception"))
            if y is not None:
                p["y"] = y
        elif k == "viewpoint":
            dr = str(d.get("direction", "")).strip().upper()
            rng = re.fullmatch(r"(\d+)\s*-\s*(\d+)", dr)
            if dr in ("0-360", "360", "ALL") or (rng and ((int(rng.group(2)) - int(rng.group(1))) % 360 or 360) >= 300):
                p["pan"] = 1
            if d.get("tower:type") or d.get("man_made") == "tower":
                p["tw"] = 1
        elif k in ("rest_area", "picnic_site", "trailhead"):
            fac = (1 if yes(d.get("toilets")) else 0) | (2 if yes(d.get("drinking_water")) else 0) \
                | (4 if yes(d.get("shelter")) or yes(d.get("covered")) else 0) | (8 if yes(d.get("picnic_table")) or yes(d.get("bench")) else 0) \
                | (16 if yes(d.get("fireplace")) or yes(d.get("bbq")) else 0) | (32 if yes(d.get("parking")) else 0)
            if fac:
                p["fac"] = fac
        n += any(key in p for key in ("pr", "is", "h", "fh", "rg", "y", "len", "pan", "tw", "fac"))
    write("pois.json", fc)
    print(f"pois.json: {n} POIs with filter values", file=sys.stderr)


def heritage() -> None:
    fc = json.load(open(B / "heritage.json"))
    det = jsonl("details-heritage.jsonl")
    n = 0
    for f in fc["features"]:
        p = f["properties"]
        d = det.get(p.get("i"), {})
        for key in ("by", "dy", "sl", "wp"):
            p.pop(key, None)
        by = year(d.get("inception"))
        if by is not None:
            p["by"] = by
        dy = year(p.get("date"))
        if dy is not None:
            p["dy"] = dy
        if d.get("sl"):
            p["sl"] = d["sl"]
        if d.get("wiki"):
            p["wp"] = 1
        p["t"] = tier(p)
        n += "by" in p or "wp" in p
    write("heritage.json", fc)
    print(f"heritage.json: {n} sites with a build year or article", file=sys.stderr)


def areas() -> None:
    for name, layer in (("heritage-areas.json", "harea"), ("special.json", "special"), ("indigenous.json", "indigenous")):
        fc = json.load(open(B / name))
        det = jsonl(f"details-{layer}.jsonl")
        for f in fc["features"]:
            a = det.get(f["properties"].get("i"), {}).get("area_km2")
            if a:
                f["properties"]["a"] = a
        write(name, fc)
    print("areas: a (km²) on heritage districts, biospheres & co., Indigenous lands", file=sys.stderr)


if __name__ == "__main__":
    pois()
    heritage()
    areas()
