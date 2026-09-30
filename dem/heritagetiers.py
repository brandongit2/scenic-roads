#!/usr/bin/env python3
"""Kinds of designation for the Heritage sites sub-toggles: each site's `t`, from its designation
(and level, for World Heritage and anything not recognised). Grouped as in the Layers panel
(web/src/basemap.ts HERITAGE_TIERS, same keys): World Heritage (w.*), National (n.*), Provincial
/ state (p.*), Municipal (m.*). Kinds cut across jurisdictions: England's Grade I, Scotland's
Category A and France's monuments classés are all "top grade".

usage: heritagetiers.py   (prints the designations per kind in data/build/heritage.json)
"""
from __future__ import annotations

import re

# (kind, pattern on the designation), first match wins within the site's level group.
NATIONAL = [
    ("n.mon", r"scheduled monument|state care monument|preservation order|national monument in state care|protected monument"
              r"|^historic site \("),
    ("n.land", r"park and garden|garden and designed landscape|battlefield|^place of scenic beauty|cultural landscape"),
    ("n.hist", r"national historic site of canada|national historic landmark"),
    ("n.fed", r"federal heritage building|heritage railway station|heritage lighthouse"),
    ("n.lower", r"graded historic building, grade [23]\b|registered tangible cultural property|registered monument \(|historic site marker"),
    ("n.second", r"graded historic building, grade 1\b|grade ii\*|grade b\+|category b\b|grade b\b|ungraded|monument historique inscrit"
                 r"|interesse público|bé inventariat|patrimonio protegido|national register of historic places"),
    ("n.top", r"grade i\b|grade a\b|category a\b|monument historique classé|inter[eéè]s (cultural|nacional)|monumento nacional"
              r"|(national|international) rating|declared monument"),
]
PROVINCIAL = [
    ("p.area", r"site patrimonial|heritage district|historic area|provincial park|settlement|cultural landscape|preservation district"),
    ("p.reg", r"^registered historic (place|site)|recognized|^historic building \(|commemorative building"),
]
MUNICIPAL = [
    ("m.area", r"site patrimonial cité|conservation area|conjunto de interesse|sítio de interesse"),
    ("m.agr", r"agreement|covenant"),
    ("m.reg", r"community heritage register|local register|register of local historic places|registered historic place"),
]


def tier(p: dict) -> str:
    """Kind of designation of a heritage site (its properties)."""
    lv = p.get("level") or 5
    d = (p.get("designation") or "").lower()
    if lv == 1:
        return "w.n" if p.get("category") in ("Natural", "Mixed") else "w.c"
    rules = NATIONAL if lv <= 3 or "graded historic building" in d else PROVINCIAL if lv == 4 else MUNICIPAL
    for k, pat in rules:
        if re.search(pat, d):
            return k
    return {2: "n.top", 3: "n.second", 4: "p.des"}.get(lv, "m.des")


if __name__ == "__main__":
    import json
    from collections import Counter, defaultdict
    from pathlib import Path

    fc = json.load(open(Path(__file__).resolve().parent.parent / "data" / "build" / "heritage.json"))
    by = defaultdict(Counter)
    for f in fc["features"]:
        p = f["properties"]
        by[tier(p)][re.sub(r"\s*\([^)]*\)$", "", p.get("designation", "?"))[:70]] += 1
    for k in sorted(by):
        print(f"{k:9s} {sum(by[k].values()):7d}")
        for d, n in by[k].most_common():
            print(f"            {n:6d}  {d}")
