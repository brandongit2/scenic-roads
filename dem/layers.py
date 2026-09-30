#!/usr/bin/env python3
"""The overlays as the map loads them (layer-*.json in data/build, served by /api/layer/): only the
properties the map draws, filters and labels with, in draw order, with polygons simplified.

  heritage, pois   points sorted by fame (fa, least known first), so the best known are drawn on
                   top without a sort key (which would cost a draw call per dot). A World
                   Heritage Site in several components (whsshapes.py) is one dot at its centre
                   (with the lead component's record and np, the number of components), and its
                   components parts (pt 1, cn the component's name), drawn small close in. The stops &
                   sights are also written per kind (layer-pois-<kind>.json, the Layers panel's
                   kinds: rest areas with picnic sites): the map loads only the kinds shown, and
                   half the stops are peaks. layer-summits.json: the named peaks by height
                   ([lon, lat, ele, name]), for the highest summit in view. Heritage sites
                   keep what the dots, labels, filters and hover title use; the rest of their
                   properties (dates, authority, source, links …) go to props-heritage.jsonl by
                   feature index, which the server merges into /api/detail/heritage/{i}.
  whs-shapes       the World Heritage outlines (whsshapes.py) with i, their site's record.
  heritage-areas   simplified to ~1 m (topology preserved); coordinates to 6 decimals.
  indigenous, special   coordinates to 6 decimals.

Named features carry `en`, their English where it truly differs (names.py english_at: a heritage
site's own, else the table's for where it is), for the labels and the app's text.

Also layer-summary.json: per polygon overlay, the feature count and each feature's area (km²),
for the Layers panel's counts under the area filters, without loading the polygons.

usage: layers.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

from shapely.geometry import mapping, shape

import names
import whsshapes

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
# Heritage properties kept on the map (style, filters, labels, hover title, Sights list).
HERITAGE_KEEP = {"i", "name", "en", "designation", "t", "level", "fa", "ia", "mz", "pv", "sl", "dy", "by", "wp", "approx", "np", "pt", "cn"}
SIMPLIFY_DEG = 1e-5


def rounded(g):
    """Coordinates to 6 decimals (~0.1 m)."""
    if isinstance(g, (list, tuple)):
        if g and isinstance(g[0], (int, float)):
            return [round(v, 6) for v in g]
        return [rounded(x) for x in g]
    return g


def write(name: str, fc: dict) -> None:
    tmp = B / f"{name}.tmp"
    tmp.write_text(json.dumps(fc, ensure_ascii=False, separators=(",", ":")))
    tmp.replace(B / name)


# World Heritage id → the record (heritage index i) its dot shows: the lead component's, else its
# only one's (merged_sites), for its outlines (whs_outlines).
WHS_RECORD: dict[str, int] = {}


def merged_sites(feats: list[dict]) -> list[dict]:
    """The heritage sites with each World Heritage Site in several components as one dot at its
    centre, and its components as parts."""
    whs = whsshapes.load_sites()
    role = whsshapes.roles(feats, whs)
    for i, f in enumerate(feats):
        sid = whsshapes.whs_id(f["properties"])
        if sid and "i" in f["properties"] and (sid not in WHS_RECORD or role.get(i, ("",))[0] == "lead"):
            WHS_RECORD[sid] = f["properties"]["i"]
    out = []
    for i, f in enumerate(feats):
        if i not in role:
            out.append(f)
            continue
        r, sid = role[i]
        p = f["properties"]
        if r == "lead":
            out.append({"type": "Feature", "geometry": {"type": "Point", "coordinates": whs[sid]["dot"]},
                        "properties": {**{k: v for k, v in p.items() if k != "component"}, "np": whs[sid]["n"]}})
        part = {k: v for k, v in p.items() if k not in ("ia", "mz", "component")}
        out.append({"type": "Feature", "geometry": f["geometry"], "properties": {**part, "pt": 1, "cn": p.get("component") or p.get("name")}})
    print(f"layers: heritage: {sum(r == 'lead' for r, _ in role.values())} World Heritage Sites in several components "
          f"as one dot, {len(role)} components drawn apart", file=sys.stderr)
    return out


def points(src: str, keep: set[str] | None, heritage: bool) -> None:
    fc = json.load(open(B / f"{src}.json"))
    if heritage:
        fc["features"] = merged_sites(fc["features"])
    fame = (lambda p: p["fa"] if p.get("fa") is not None else (5 - (p.get("level") or 5)) if heritage else 0.0)
    fc["features"].sort(key=lambda f: fame(f["properties"]))
    props, seen = [], set()
    wiki = names.wiki_titles() if heritage else {}
    n_en = 0
    for f in fc["features"]:
        p = f["properties"]
        own = names.site_english(p.get("name") or "", p.get("name_en"), wiki.get(p.get("i")))[0] if heritage else None
        en = names.english_at(p.get("name"), names.first_point(f["geometry"]), own)
        if en:
            p["en"] = en
            n_en += 1
        if keep is not None:
            rest = {k: v for k, v in p.items() if k not in keep}
            # (a merged site's dot and its lead component share the record: once)
            if rest and "i" in p and p["i"] not in seen:
                seen.add(p["i"])
                props.append({"i": p["i"], **rest})
            f["properties"] = {k: v for k, v in p.items() if k in keep}
        f["geometry"]["coordinates"] = rounded(f["geometry"]["coordinates"])
    write(f"layer-{src}.json", fc)
    if keep is not None:
        tmp = B / f"props-{src}.jsonl.tmp"
        with open(tmp, "w", encoding="utf-8") as out:
            for r in sorted(props, key=lambda r: r["i"]):
                out.write(json.dumps(r, ensure_ascii=False, separators=(",", ":")) + "\n")
        tmp.replace(B / f"props-{src}.jsonl")
    print(f"layers: {src}: {len(fc['features'])} points ({n_en} with English)", file=sys.stderr)


def whs_outlines() -> None:
    """layer-whs-shapes.json: the World Heritage outlines with i, their site's record (its dot's),
    so an outline opens the site like its dot."""
    fc = json.load(open(B / "whs-shapes.json"))
    for f in fc["features"]:
        i = WHS_RECORD.get(str(f["properties"]["id"]))
        if i is not None:
            f["properties"]["i"] = i
    write("layer-whs-shapes.json", fc)
    print(f"layers: whs-shapes: {len(fc['features'])} outlines", file=sys.stderr)


# Stops & sights kinds → the Layers panel's kind (web/src/state.ts OVERLAYS).
POI_KIND = {"rest_area": "rest", "picnic_site": "rest"}


def pois_by_kind() -> None:
    """layer-pois.json split per kind (same order, same properties)."""
    fc = json.load(open(B / "layer-pois.json"))
    by: dict[str, list] = {}
    for f in fc["features"]:
        k = f["properties"].get("kind") or ""
        by.setdefault(POI_KIND.get(k, k), []).append(f)
    for k, feats in by.items():
        if k:
            write(f"layer-pois-{k}.json", {"type": "FeatureCollection", "features": feats})
    # Named peaks by height, compact, for the highest summit in view (the In view panel) without
    # loading every stop.
    peaks = [(p["ele"], *f["geometry"]["coordinates"], p["name"]) for f in by.get("peak", [])
             for p in [f["properties"]] if p.get("name") and isinstance(p.get("ele"), (int, float))]
    peaks.sort(key=lambda r: -r[0])
    write("layer-summits.json", {"p": [[round(x, 5), round(y, 5), round(e), n] for e, x, y, n in peaks]})
    print("layers: pois by kind: " + ", ".join(f"{k} {len(v)}" for k, v in sorted(by.items())), file=sys.stderr)


def polygons(src: str, simplify: float) -> dict:
    fc = json.load(open(B / f"{src}.json"))
    for f in fc["features"]:
        g = f["geometry"]
        p = f["properties"]
        en = names.english_at(p.get("name"), names.first_point(g), p.get("name_en"))
        if en:
            p["en"] = en
        if simplify and g:
            s = shape(g).simplify(simplify, preserve_topology=True)
            if not s.is_empty:
                g = mapping(s)
        f["geometry"] = {"type": g["type"], "coordinates": rounded(g["coordinates"])}
    write(f"layer-{src}.json", fc)
    print(f"layers: {src}: {len(fc['features'])} areas", file=sys.stderr)
    return {"n": len(fc["features"]), "a": [f["properties"].get("a") for f in fc["features"]]}


def main() -> None:
    points("heritage", HERITAGE_KEEP, True)
    if (B / "whs-shapes.json").exists():
        whs_outlines()
    points("pois", None, False)
    pois_by_kind()
    summary = {
        "heritageAreas": polygons("heritage-areas", SIMPLIFY_DEG),
        "indigenous": polygons("indigenous", 0),
        "special": polygons("special", 0),
    }
    write("layer-summary.json", summary)


if __name__ == "__main__":
    main()
