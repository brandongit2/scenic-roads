#!/usr/bin/env python3
"""The overlays as the map loads them (layer-*.json in data/build, served by /api/layer/): only the
properties the map draws, filters and labels with, in draw order, with polygons simplified.

  heritage, pois   points sorted by fame (fa, least known first), so the best known are drawn on
                   top without a sort key (which would cost a draw call per dot). Heritage sites
                   keep what the dots, labels, filters and hover title use; the rest of their
                   properties (dates, authority, source, links …) go to props-heritage.jsonl by
                   feature index, which the server merges into /api/detail/heritage/{i}.
  heritage-areas   simplified to ~1 m (topology preserved); coordinates to 6 decimals.
  indigenous, special   coordinates to 6 decimals.

Also layer-summary.json: per polygon overlay, the feature count and each feature's area (km²),
for the Layers panel's counts under the area filters, without loading the polygons.

usage: layers.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

from shapely.geometry import mapping, shape

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
# Heritage properties kept on the map (style, filters, labels, hover title, Sights list).
HERITAGE_KEEP = {"i", "name", "name_en", "designation", "t", "level", "fa", "ia", "mz", "pv", "sl", "dy", "by", "wp", "approx"}
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


def points(src: str, keep: set[str] | None, heritage: bool) -> None:
    fc = json.load(open(B / f"{src}.json"))
    fame = (lambda p: p["fa"] if p.get("fa") is not None else (5 - (p.get("level") or 5)) if heritage else 0.0)
    fc["features"].sort(key=lambda f: fame(f["properties"]))
    props = []
    for f in fc["features"]:
        p = f["properties"]
        if keep is not None:
            rest = {k: v for k, v in p.items() if k not in keep}
            if rest and "i" in p:
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
    print(f"layers: {src}: {len(fc['features'])} points", file=sys.stderr)


def polygons(src: str, simplify: float) -> dict:
    fc = json.load(open(B / f"{src}.json"))
    for f in fc["features"]:
        g = f["geometry"]
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
    points("pois", None, False)
    summary = {
        "heritageAreas": polygons("heritage-areas", SIMPLIFY_DEG),
        "indigenous": polygons("indigenous", 0),
        "special": polygons("special", 0),
    }
    write("layer-summary.json", summary)


if __name__ == "__main__":
    main()
