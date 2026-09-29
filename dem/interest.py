#!/usr/bin/env python3
"""How interesting each stop & sight and heritage site is: fame, and rarity nearby.

  fa  fame: log10(1 + average monthly Wikipedia pageviews), summed over the place's articles in
      the map's languages (pageviews.py); without any, a little for Wikidata sitelinks (articles
      in other languages). A viewpoint's item counts only when it is a landscape or lookout
      (VIEW_ITEM), not a mine or chapel that happens to be tagged as a viewpoint. Places with
      neither tie at 0, and ties are broken by a small amount (at most 0.01, so never above a
      real difference in fame): named before unnamed, then how
      much OpenStreetMap says about it, then its size within its kind (prominence for peaks,
      height for waterfalls and lighthouses, length for covered bridges, designation group for
      heritage sites).
  pv  average monthly pageviews (when it has articles).
  ia  interest isolation, km: the distance to the nearest place of the same kind (peaks with
      peaks, heritage sites with heritage sites …) that scores higher. The best-known waterfall for
      80 km around has 80; one beside a famous fall has 0.3. The best of each kind has 20000.
  mz  the zoom from which that distance spans one pixel (512 px tiles); the map shows a place from
      mz + log2(spacing in px).

The map shows a place from the zoom where its isolation spans enough pixels (an even density at
every zoom, the best-known and locally best first), sized and labelled by fame.

Rewrites pois.json and heritage.json in data/build (after filterprops.py).

usage: interest.py
"""
from __future__ import annotations

import json
import math
import re
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
PV = ROOT / "data" / "pageviews" / "items.json"
CELL_KM = 5.0
# A viewpoint's Wikidata item is often the thing it looks at or stands on; its fame counts only when
# that is a landscape, a lookout or the like (not a mine, chapel or barrow tagged as a viewpoint).
VIEW_ITEM = re.compile(r"viewpoint|lookout|observation|belvedere|mirador|mountain|hill|peak|summit|cliff|headland|promontory|"
                       r"point|cape|peninsula|pass|col\b|gorge|canyon|valley|falls|waterfall|geosite|park|tower|lighthouse|beach|bay|island|lake")
RINGS = 12  # grid search out to ~60 km; farther ones are found by a full scan


def jsonl(name: str) -> dict:
    out = {}
    p = B / name
    if p.exists():
        for line in open(p, encoding="utf-8"):
            r = json.loads(line)
            out[r["i"]] = r
    return out


def percentile(vals: list[float | None]) -> list[float]:
    """Rank of each value among the known ones, 0–1 (unknown: 0)."""
    known = sorted(v for v in vals if v is not None)
    if not known:
        return [0.0] * len(vals)
    arr = np.array(known)
    return [float(np.searchsorted(arr, v, side="right")) / len(arr) if v is not None else 0.0 for v in vals]


def isolation(lon: np.ndarray, lat: np.ndarray, score: np.ndarray) -> np.ndarray:
    """Distance (km) from each point to the nearest point with a higher score (ties: earlier index)."""
    n = len(lon)
    order = np.lexsort((np.arange(n), -score))  # best first
    rank = np.empty(n, dtype=np.int64)
    rank[order] = np.arange(n)
    x = lon * 111.32 * np.cos(np.radians(lat))
    y = lat * 110.57
    cx, cy = np.floor(x / CELL_KM).astype(np.int64), np.floor(y / CELL_KM).astype(np.int64)
    grid: dict[tuple[int, int], list[int]] = {}
    for i in range(n):
        grid.setdefault((int(cx[i]), int(cy[i])), []).append(i)
    out = np.full(n, 20000.0)
    far = []
    for i in order[1:]:
        r, gx, gy = rank[i], int(cx[i]), int(cy[i])
        best = math.inf
        for k in range(RINGS + 1):
            if best <= (k - 1) * CELL_KM:
                break
            for dx in range(-k, k + 1):
                for dy in range(-k, k + 1):
                    if max(abs(dx), abs(dy)) != k:
                        continue
                    for j in grid.get((gx + dx, gy + dy), ()):
                        if rank[j] < r:
                            d = math.hypot(x[j] - x[i], y[j] - y[i])
                            if d < best:
                                best = d
        if best <= RINGS * CELL_KM:
            out[i] = best
        else:
            far.append(i)
    # Places with nothing better within the grid search: the nearest better one anywhere
    # (great-circle distance).
    lo, la = np.radians(lon), np.radians(lat)
    for i in far:
        better = order[: rank[i]]
        d = np.sin((la[better] - la[i]) / 2) ** 2 + np.cos(la[i]) * np.cos(la[better]) * np.sin((lo[better] - lo[i]) / 2) ** 2
        out[i] = float(6371 * 2 * np.arcsin(np.sqrt(d.min())))
    return out


def main() -> None:
    views = json.loads(PV.read_text()) if PV.exists() else {}
    if not views:
        print("interest: no pageviews yet (pageviews.py); fame from sitelinks only", file=sys.stderr)

    def fame(qid: str | None, sl: int) -> tuple[float, float | None]:
        pv = views.get(qid) if qid else None
        if pv:
            return math.log10(1 + pv), pv
        return (0.3 * math.log10(1 + sl) if sl else 0.0), None

    # ---- stops & sights ----
    pfc = json.load(open(B / "pois.json"))
    det = jsonl("details-poi.jsonl")
    feats = pfc["features"]
    group = [("rest" if f["properties"]["kind"] in ("rest_area", "picnic_site") else f["properties"]["kind"]) for f in feats]
    size = []
    for f in feats:
        p = f["properties"]
        k = p["kind"]
        size.append(p.get("pr") if k == "peak" and p.get("pr") is not None else p.get("ele") if k in ("peak", "viewpoint")
                    else p.get("h") if k == "waterfall" else (p.get("fh") or p.get("h") or p.get("rg")) if k == "lighthouse"
                    else p.get("len") if k == "covered_bridge" else None)
    base = np.zeros(len(feats))
    fa_, pv_ = [0.0] * len(feats), [None] * len(feats)
    for g in set(group):
        idx = [i for i, x in enumerate(group) if x == g]
        sp = percentile([size[i] for i in idx])
        for n, i in enumerate(idx):
            p = feats[i]["properties"]
            d = det.get(p.get("i"), {})
            qid = (d.get("wikidata") or "").split(";")[0].strip() or None
            if g == "viewpoint" and not VIEW_ITEM.search((d.get("wd") or {}).get("d_en", "").lower()):
                qid = None
            f, pv = fame(qid, (d.get("wd") or {}).get("sl", 0) if qid else 0)
            rich = sum(bool(d.get(t)) for t in ("wikidata", "wikipedia", "description", "website", "image")) / 3
            tie = 0.5 * bool(p.get("name")) + 0.2 * min(1.0, rich) + 0.3 * sp[n]
            base[i] = f + 0.01 * tie
            fa_[i], pv_[i] = f, pv
    ia = np.zeros(len(feats))
    lon = np.array([f["geometry"]["coordinates"][0] for f in feats])
    lat = np.array([f["geometry"]["coordinates"][1] for f in feats])
    for g in set(group):
        idx = np.array([i for i, x in enumerate(group) if x == g])
        ia[idx] = isolation(lon[idx], lat[idx], base[idx])
        print(f"interest: {g:15s} {len(idx):7d}, {int((ia[idx] >= 20).sum())} best within 20 km", file=sys.stderr, flush=True)
    for i, f in enumerate(feats):
        p = f["properties"]
        for k in ("fa", "pv", "ia", "mz"):
            p.pop(k, None)
        p["fa"] = round(float(base[i]), 3)
        if pv_[i]:
            p["pv"] = round(pv_[i])
        p["ia"] = round(float(ia[i]), 1)
        p["mz"] = min_zoom(lat[i], float(ia[i]))
    write(pfc, "pois.json")

    # ---- heritage sites ----
    hfc = json.load(open(B / "heritage.json"))
    hdet = jsonl("details-heritage.jsonl")
    hf = hfc["features"]
    base = np.zeros(len(hf))
    for i, f in enumerate(hf):
        p = f["properties"]
        d = hdet.get(p.get("i"), {})
        fm, pv = fame(d.get("qid"), d.get("sl", 0))
        tie = 0.5 * bool(p.get("name")) + 0.2 * bool(d.get("qid")) + 0.3 * (5 - (p.get("level") or 5)) / 4
        base[i] = fm + 0.01 * tie
        for k in ("fa", "pv", "ia", "mz"):
            p.pop(k, None)
        p["fa"] = round(float(base[i]), 3)
        if pv:
            p["pv"] = round(pv)
    lon = np.array([f["geometry"]["coordinates"][0] for f in hf])
    lat = np.array([f["geometry"]["coordinates"][1] for f in hf])
    ia = isolation(lon, lat, base)
    for i, f in enumerate(hf):
        f["properties"]["ia"] = round(float(ia[i]), 1)
        f["properties"]["mz"] = min_zoom(lat[i], float(ia[i]))
    print(f"interest: heritage        {len(hf):7d}, {int((ia >= 20).sum())} best within 20 km", file=sys.stderr)
    write(hfc, "heritage.json")


def min_zoom(lat: float, ia_km: float) -> float:
    """Zoom at which ia_km spans one pixel: 78.27 km per pixel at zoom 0 on 512 px tiles."""
    return round(math.log2(78.2715 * math.cos(math.radians(lat)) / max(ia_km, 0.01)), 2)


def write(fc: dict, name: str) -> None:
    tmp = B / f"{name}.tmp"
    tmp.write_text(json.dumps(fc, ensure_ascii=False, separators=(",", ":")))
    tmp.replace(B / name)


if __name__ == "__main__":
    main()
