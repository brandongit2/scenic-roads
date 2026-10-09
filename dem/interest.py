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

A World Heritage Site in several components (whsshapes.py) counts once, at its dot: its lead
component carries the site's isolation there, and the others (drawn small, close in) have none. A
World Heritage Site's fame is its best-known item's: its own or one of its components' (the Rideau
Canal's own item has no articles; the canal's has).

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
from scipy.spatial import cKDTree

import whsshapes
from timings import phase

ROOT = Path(__file__).resolve().parent.parent
B = ROOT / "data" / "build"
PV = ROOT / "data" / "pageviews" / "items.json"
# A viewpoint's Wikidata item is often the thing it looks at or stands on; its fame counts only when
# that is a landscape, a lookout or the like (not a mine, chapel or barrow tagged as a viewpoint).
VIEW_ITEM = re.compile(r"viewpoint|lookout|observation|belvedere|mirador|mountain|hill|peak|summit|cliff|headland|promontory|"
                       r"point|cape|peninsula|pass|col\b|gorge|canyon|valley|falls|waterfall|geosite|park|tower|lighthouse|beach|bay|island|lake")
# Isolation within this on the plane, farther by great circle.
NEAR_KM = 60.0


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


def _better(tree: cKDTree, pts: np.ndarray, todo: np.ndarray, rank: np.ndarray, k: int) -> tuple[np.ndarray, np.ndarray]:
    """For the points `todo`: the distance to the nearest better-ranked one among their k nearest
    (inf where none is), and to the k-th nearest."""
    best, kth = np.empty(len(todo)), np.empty(len(todo))
    step = max(1, 4_000_000 // k)  # bounded memory: rows × k neighbours
    for s in range(0, len(todo), step):
        t = todo[s:s + step]
        d, j = tree.query(pts[t], k=k)
        d, j = d.reshape(len(t), k), j.reshape(len(t), k)
        better = rank[j] < rank[t][:, None]
        first = better.argmax(axis=1)
        best[s:s + step] = np.where(better.any(axis=1), d[np.arange(len(t)), first], np.inf)
        kth[s:s + step] = d[:, -1]
    return best, kth


def isolation(lon: np.ndarray, lat: np.ndarray, score: np.ndarray) -> np.ndarray:
    """Distance (km) from each point to the nearest point with a higher score (ties: earlier index).

    Within NEAR_KM on the plane (each point's x scaled by its own latitude), farther by great-circle
    distance; by k nearest neighbours, k growing for the points with nothing better among them (a
    grid search before: quadratic in cities, where a 5 km cell holds thousands of places)."""
    n = len(lon)
    out = np.full(n, 20000.0)
    if n < 2:
        return out
    order = np.lexsort((np.arange(n), -score))  # best first
    rank = np.empty(n, dtype=np.int64)
    rank[order] = np.arange(n)
    xy = np.column_stack([lon * 111.32 * np.cos(np.radians(lat)), lat * 110.57])
    tree = cKDTree(xy)
    todo, far, k = order[1:], [], 16
    while len(todo):
        kk = min(k, n)
        best, kth = _better(tree, xy, todo, rank, kk)
        near = best <= NEAR_KM
        out[todo[near]] = best[near]
        # Beyond NEAR_KM: a better one farther than that, or none among neighbours reaching past it.
        beyond = ~near & (np.isfinite(best) | (kth > NEAR_KM) | (kk == n))
        far.append(todo[beyond])
        todo = todo[~near & ~beyond]
        k *= 4
    todo = np.concatenate(far)
    if not len(todo):
        return out
    # The nearest better one anywhere: chords between unit vectors order as great circles do.
    la, lo = np.radians(lat), np.radians(lon)
    u = np.column_stack([np.cos(la) * np.cos(lo), np.cos(la) * np.sin(lo), np.sin(la)])
    stree, k = cKDTree(u), 64
    km = lambda chord: 6371 * 2 * np.arcsin(np.minimum(1.0, chord / 2))
    while len(todo):
        # Few better ones: all of them.
        few = rank[todo] <= k
        for i in todo[few]:
            out[i] = float(km(np.sqrt(((u[order[:rank[i]]] - u[i]) ** 2).sum(axis=1)).min()))
        todo = todo[~few]
        if not len(todo):
            break
        best, _ = _better(stree, u, todo, rank, min(k, n))
        hit = np.isfinite(best)
        out[todo[hit]] = km(best[hit])
        todo = todo[~hit]
        k *= 4
    return out


def main() -> None:
    with phase("the layers and details read", "disk"):
        views = json.loads(PV.read_text()) if PV.exists() else {}
    if not views:
        print("interest: no pageviews yet (pageviews.py); fame from sitelinks only", file=sys.stderr)

    def fame(qid: str | None, sl: int) -> tuple[float, float | None]:
        pv = views.get(qid) if qid else None
        if pv:
            return math.log10(1 + pv), pv
        return (0.3 * math.log10(1 + sl) if sl else 0.0), None

    # ---- stops & sights ----
    with phase("the layers and details read", "disk"):
        pfc = json.load(open(B / "pois.json"))
        det = jsonl("details-poi.jsonl")
    with phase("the fame scored", "compute"):
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
    with phase("the isolation measured", "compute"):
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
    with phase("the layers written", "disk"):
        write(pfc, "pois.json")

    # ---- heritage sites ----
    with phase("the layers and details read", "disk"):
        hfc = json.load(open(B / "heritage.json"))
        hdet = jsonl("details-heritage.jsonl")
        hf = hfc["features"]
        whs = whsshapes.load_sites()
    with phase("the fame scored", "compute"):
        role = whsshapes.roles(hf, whs)
        base = np.zeros(len(hf))
        for i, f in enumerate(hf):
            p = f["properties"]
            d = hdet.get(p.get("i"), {})
            fm, pv = fame(d.get("qid"), d.get("sl", 0))
            sid = whsshapes.whs_id(p)
            for q in whs.get(sid, {}).get("q", []) if sid else []:
                if views.get(q) and math.log10(1 + views[q]) > fm:
                    fm, pv = math.log10(1 + views[q]), views[q]
            tie = 0.5 * bool(p.get("name")) + 0.2 * bool(d.get("qid")) + 0.3 * (5 - (p.get("level") or 5)) / 4
            base[i] = fm + 0.01 * tie
            for k in ("fa", "pv", "ia", "mz"):
                p.pop(k, None)
            p["fa"] = round(float(base[i]), 3)
            if pv:
                p["pv"] = round(pv)
    # Isolation among the sites as the map shows them: a merged site once, at its dot.
    with phase("the isolation measured", "compute"):
        at = {i: whs[sid]["dot"] for i, (r, sid) in role.items() if r == "lead"}
        keep = np.array([i for i in range(len(hf)) if role.get(i, ("",))[0] != "part"], dtype=np.int64)
        lon = np.array([at[i][0] if i in at else hf[i]["geometry"]["coordinates"][0] for i in keep])
        lat = np.array([at[i][1] if i in at else hf[i]["geometry"]["coordinates"][1] for i in keep])
        ia = isolation(lon, lat, base[keep])
        for n, i in enumerate(keep):
            hf[i]["properties"]["ia"] = round(float(ia[n]), 1)
            hf[i]["properties"]["mz"] = min_zoom(lat[n], float(ia[n]))
    print(f"interest: heritage        {len(keep):7d}, {int((ia >= 20).sum())} best within 20 km "
          f"({len(at)} World Heritage Sites as one dot, {len(hf) - len(keep)} of their components apart)", file=sys.stderr)
    with phase("the layers written", "disk"):
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
