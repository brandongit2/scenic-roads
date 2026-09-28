#!/usr/bin/env python3
"""Canadian federal heritage designations, located and cross-checked.

The authority is Parks Canada's Directory of Federal Heritage Designations (DFHD, official,
open data — but without coordinates). Each designated place in our provinces (National
Historic Sites, heritage lighthouses and railway stations, federal heritage buildings) is
matched by normalised English/French name, within the same province, to:
  1. a Wikidata item carrying the same designation (coordinates from Wikidata),
  2. else any Wikidata heritage item of that name in the province,
  3. else a named OpenStreetMap feature (historic / heritage / museum / lighthouse / station …).
Anything still unlocated is flagged. Wikidata items claiming a federal designation that is not
in the official list are flagged too, and left off the map.

Outputs (data/heritage/): federal.json (located places), federal-report.csv (every record with
its status), and a summary on stdout.

usage: federal.py
"""
from __future__ import annotations

import csv
import difflib
import json
import re
import sys
import unicodedata
from collections import defaultdict
from pathlib import Path

import openpyxl
from shapely.geometry import Point, shape
from shapely.prepared import prep

H = Path(__file__).resolve().parent.parent / "data" / "heritage"
PROVINCES = {
    "Ontario": "CA-ON", "Quebec": "CA-QC", "New Brunswick": "CA-NB", "Nova Scotia": "CA-NS",
    "Prince Edward Island": "CA-PE", "Newfoundland and Labrador": "CA-NL",
}
TYPES = {
    "National Historic Site of Canada": "Q1568567",
    "Heritage Lighthouse of Canada": "Q15641550",
    "Heritage Railway Station of Canada": "Q3098283",
    "Classified Federal Heritage Building": "Q14480054",
    "Recognized Federal Heritage Building": "Q14451490",
}
DFHD_URL = "https://parks.canada.ca/culture/dfhd"

STRIP = [  # longest phrases first
    r"\(.*?\)", r"national historic sites? of canada", r"national historic sites?", r"lieux? historiques? nationa(l|ux)( du canada)?",
    r"heritage railway station", r"heritage lighthouse", r"federal heritage building",
    r"\bformer\b", r"\bancien(ne)?\b", r"\brailway station\b", r"\btrain station\b", r"\bstation\b", r"\bgare\b",
    r"\bhistoric district\b", r"\bhistoric site\b", r"\bbuilding\b", r"\bedifice\b",
    r"\bnhs\b", r"\blhn\b", r"\bthe\b", r"\bof\b", r"\bdu\b", r"\bde la\b", r"\bdes\b", r"\bde\b", r"\bd'", r"\bl'", r"\ble\b", r"\bla\b", r"\bles\b",
]


def norm(s: str | None) -> str:
    if not s:
        return ""
    s = unicodedata.normalize("NFKD", s).encode("ascii", "ignore").decode().lower()
    s = s.replace("&", " and ").replace("saint ", "st ").replace("sainte ", "ste ").replace("st. ", "st ").replace("ste. ", "ste ")
    for p in STRIP:
        s = re.sub(p, " ", s)
    s = re.sub(r"[^a-z0-9]+", " ", s)
    return " ".join(s.split())


def aliases(name: str | None) -> set[str]:
    """Normalised alternatives: ' / ' splits, parenthetical contents, and the whole name."""
    if not name:
        return set()
    out = {norm(name)}
    for part in re.split(r"\s+/\s+", name):
        out.add(norm(part))
    for par in re.findall(r"\(([^)]*)\)", name):
        out.add(norm(par))
    out.discard("")
    return out


def token_match(a: str, b: str) -> bool:
    ta, tb = set(a.split()), set(b.split())
    small, big = (ta, tb) if len(ta) <= len(tb) else (tb, ta)
    return len(small) >= 2 and small <= big


GENERIC = set("""tower lighttower light lighthouse lightstation stable house church barn granary shelter pavilion
quarters museum hall city town old fort building complex cathedral basilica chapel notre dame st ste saint island point
cape harbour range front rear private married vip family collector federal office post armoury drill customs custom
college school mill farm site park canal village mission district roman catholic anglican united presbyterian church
new north south east west upper lower great little residence main home block""".split())


def stem_sim(a: str, b: str) -> tuple[float, int]:
    """Token Jaccard where tokens sharing a 5-letter prefix count as equal (citadel ~ citadelle),
    and the number of shared tokens that are not generic words."""
    ta, tb = set(a.split()), set(b.split())
    if not ta or not tb:
        return 0.0, 0
    eq = lambda x, y: x == y or (len(x) >= 6 and len(y) >= 6 and x[:5] == y[:5])
    shared = [x for x in ta if any(eq(x, y) for y in tb)]
    distinct = sum(1 for x in shared if x not in GENERIC and len(x) > 2)
    return len(shared) / (len(ta) + len(tb) - len(shared)), distinct


def load_provinces():
    out = {}
    for line in open(H / "osm" / "prov.geojsonseq"):
        f = json.loads(line.strip("\x1e"))
        p = f["properties"]
        iso = p.get("ISO3166-2")
        if p.get("admin_level") == "4" and iso in PROVINCES.values():
            out[iso] = prep(shape(f["geometry"]).buffer(0.01))
    return out


def province_of(provs, lon, lat):
    pt = Point(lon, lat)
    for iso, g in provs.items():
        if g.contains(pt):
            return iso
    return None


def main():
    provs = load_provinces()
    print(f"provinces: {sorted(provs)}")

    # Official list.
    wb = openpyxl.load_workbook(H / "fhd.xlsx", read_only=True)
    rows = wb["OpenData_EN"].iter_rows(values_only=True)
    hdr = next(rows)
    ix = {h: i for i, h in enumerate(hdr)}
    dfhd = []
    for r in rows:
        t, p = r[ix["Designation type"]], r[ix["Province or territory"]]
        if t in TYPES and p in PROVINCES:
            others = [o.strip() for o in re.split(r"[;\n]|, (?=[A-Z])", r[ix["Other names"]] or "") if o.strip()]
            dfhd.append({
                "id": r[ix["Directory of Federal Heritage Designations Identifier"]],
                "name": r[ix["Designation name"]], "others": others, "type": t, "prov": PROVINCES[p],
                "date": str(r[ix["Designation date"]])[:10] if r[ix["Designation date"]] else "",
                "area": r[ix["Local area"]] or "", "address": r[ix["Street address"]] or "",
            })
    print(f"DFHD: {len(dfhd)} place designations in our provinces")

    # Wikidata (cached by heritage.py).
    wd = []
    for r in csv.DictReader(open(H / "wd-canada.csv", encoding="utf-8")):
        m = re.match(r"Point\(([-\d.eE]+) ([-\d.eE]+)\)", r["coord"] or "")
        if not m:
            continue
        lon, lat = float(m.group(1)), float(m.group(2))
        wd.append({"q": r["item"].rsplit("/", 1)[1], "des": r["des"].rsplit("/", 1)[1], "lon": lon, "lat": lat,
                   "en": r["name_en"], "fr": r["name_fr"], "wiki": r["wiki"]})
    prov_cache = {}
    by = defaultdict(list)  # (prov, norm) -> items
    for it in wd:
        k = (round(it["lon"], 4), round(it["lat"], 4))
        if k not in prov_cache:
            prov_cache[k] = province_of(provs, it["lon"], it["lat"])
        it["prov"] = prov_cache[k]
        if not it["prov"]:
            continue
        it["names"] = aliases(it["en"]) | aliases(it["fr"])
        for nm in it["names"]:
            by[(it["prov"], nm)].append(it)
    by_type = defaultdict(list)
    for it in wd:
        if it["prov"]:
            by_type[(it["prov"], it["des"])].append(it)
    print(f"Wikidata: {len(wd)} designation records, {sum(1 for i in wd if i['prov'])} in our provinces")

    def wd_match(rec):
        names = aliases(rec["name"]).union(*[aliases(o) for o in rec["others"]])
        want = TYPES[rec["type"]]
        # 1. same designation, exact name
        for nm in names:
            c = [i for i in by.get((rec["prov"], nm), []) if i["des"] == want]
            if c:
                return c[0], "wikidata"
        # 2. any heritage designation, exact name
        for nm in names:
            c = by.get((rec["prov"], nm), [])
            if c:
                return c[0], "wikidata (other designation)"
        # 3. fuzzy / token containment, same designation
        best, bs = None, 0.0
        for it in by_type.get((rec["prov"], want), []):
            for a in names:
                for b in it["names"]:
                    if token_match(a, b):
                        return it, "wikidata (partial name)"
                    s = difflib.SequenceMatcher(None, a, b).ratio()
                    if s > bs:
                        best, bs = it, s
        if best and bs >= 0.88:
            return best, f"wikidata (fuzzy {bs:.2f})"
        return None, None

    located, report = [], []
    used_q = set()
    unmatched = []
    for rec in dfhd:
        it, how = wd_match(rec)
        if it:
            used_q.add(it["q"])
            located.append((rec, it["lon"], it["lat"], how, it["wiki"] or f"https://www.wikidata.org/wiki/{it['q']}"))
        else:
            unmatched.append(rec)

    # Pair remaining records with unused Wikidata items carrying the same designation, by
    # stemmed-token similarity (≥ 0.5 in-province, ≥ 0.75 across a border), best pairs first.
    pairs = []
    fed_items = [i for i in wd if i["prov"] and i["des"] in set(TYPES.values()) and i["q"] not in used_q]
    for ri, rec in enumerate(unmatched):
        names = aliases(rec["name"]).union(*[aliases(o) for o in rec["others"]])
        for it in fed_items:
            if it["des"] != TYPES[rec["type"]]:
                continue
            best = max((stem_sim(a, b) + (len(a.split()),) for a in names for b in it["names"]), default=(0, 0, 0))
            sim, distinct, ntok = best
            # Generic-only names ("Tower", "Stable") need an exact multi-word match.
            ok = distinct >= 1 or (sim == 1.0 and ntok >= 2)
            if ok and sim >= (0.5 if it["prov"] == rec["prov"] else 0.75):
                pairs.append((sim, ri, it))
    pairs.sort(key=lambda x: -x[0])
    done_r = set()
    for sim, ri, it in pairs:
        if ri in done_r or it["q"] in used_q:
            continue
        done_r.add(ri)
        used_q.add(it["q"])
        rec = unmatched[ri]
        located.append((rec, it["lon"], it["lat"], f"wikidata (paired {sim:.2f}: '{it['en'] or it['fr']}')",
                        it["wiki"] or f"https://www.wikidata.org/wiki/{it['q']}"))
    unmatched = [r for i, r in enumerate(unmatched) if i not in done_r]

    # OSM fallback for the rest.
    targets = defaultdict(list)
    for rec in unmatched:
        for nm in aliases(rec["name"]).union(*[aliases(o) for o in rec["others"]]):
            targets[nm].append(rec)
    hits = defaultdict(list)
    for line in open(H / "osm" / "named.geojsonseq"):
        f = json.loads(line.strip("\x1e"))
        p = f["properties"]
        for key in ("name", "name:en", "name:fr", "official_name", "alt_name", "old_name"):
            nm = norm(p.get(key))
            if nm and nm in targets:
                g = shape(f["geometry"])
                c = g.representative_point() if g.geom_type != "Point" else g
                hits[nm].append((c.x, c.y, p.get("name")))
                break
    still = []
    for rec in unmatched:
        cand = []
        for nm in aliases(rec["name"]).union(*[aliases(o) for o in rec["others"]]):
            cand += [h for h in hits.get(nm, []) if province_of(provs, h[0], h[1]) == rec["prov"]]
        if cand:
            lon, lat, osm_name = cand[0]
            located.append((rec, lon, lat, f"OSM ('{osm_name}')", None))
        else:
            still.append(rec)

    feats = []
    for rec, lon, lat, how, url in located:
        feats.append({"type": "Feature", "geometry": {"type": "Point", "coordinates": [round(lon, 6), round(lat, 6)]},
                      "properties": {"name": rec["name"], "designation": rec["type"], "level": 2, "date": rec["date"],
                                     "dfhd_id": rec["id"], "url": url or DFHD_URL, "source": "Parks Canada DFHD",
                                     "location": how}})
    (H / "federal.json").write_text(json.dumps({"type": "FeatureCollection", "features": feats}, ensure_ascii=False))

    fed_q = set(TYPES.values())
    wd_only = {i["q"]: i for i in wd if i["prov"] and i["des"] in fed_q and i["q"] not in used_q}
    with open(H / "federal-report.csv", "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["status", "designation", "province", "name", "dfhd_id", "local_area", "located_via", "lon", "lat"])
        for rec, lon, lat, how, _ in located:
            w.writerow(["located", rec["type"], rec["prov"], rec["name"], rec["id"], rec["area"], how, f"{lon:.5f}", f"{lat:.5f}"])
        for rec in still:
            w.writerow(["MISSING (not located)", rec["type"], rec["prov"], rec["name"], rec["id"], rec["area"], "", "", ""])
        for it in wd_only.values():
            w.writerow(["NOT IN OFFICIAL LIST (Wikidata only)", it["des"], it["prov"], it["en"] or it["fr"], "", "", it["q"], f"{it['lon']:.5f}", f"{it['lat']:.5f}"])

    # Summary.
    print("\nlocated / total by designation:")
    for t in TYPES:
        tot = sum(1 for r in dfhd if r["type"] == t)
        loc = [x for x in located if x[0]["type"] == t]
        via = defaultdict(int)
        for x in loc:
            via[x[3].split(" (")[0]] += 1
        print(f"  {t:40s} {len(loc):4d} / {tot:4d}   via {dict(via)}")
    print(f"\nmissing: {len(still)}  (National Historic Sites: {sum(1 for r in still if r['type'].startswith('National'))})")
    for r in [r for r in still if r["type"].startswith("National")][:60]:
        print(f"   {r['prov']}  {r['name']}  [{r['area']}]")
    print(f"Wikidata-only federal claims (excluded): {len(wd_only)}")


if __name__ == "__main__":
    sys.exit(main())
