#!/usr/bin/env python3
"""Rail stops for the map: every stop of a passenger route relation, with the stop spacing of the
lines calling there (how far apart their stops are on average), which sets when a stop's dot
appears and how big it is. An intercity station, whose line stops every 40 km, shows from far out
and large; a tram stop, 400 m from the next, only close in and small; no rule by type.

Input (Makefile): the route relations (route=train, subway, tram, light_rail, monorail, funicular)
of merged.osm.pbf as OPL (data/rail/stops/relations.opl), and their stop objects exported
(stops.geojsonseq: stop positions, platforms, stations). A route's stops are its members with a
stop role (stop, stop_entry_only, stop_exit_only), else its platforms, in order; its spacing is
the distance between consecutive stops, averaged. The members of one stop (stop positions,
platforms, the station) and the same stop on several routes are merged by name within 800 m
(unnamed ones within 100 m); a stop takes the largest spacing of the lines calling there, and the
service group of that line (as extract.rs groups routes; heritage services by their tags).

Writes data/build/stations.json: points with n (name), g (group: 0 tram, 1 metro, 2 commuter &
regional, 3 intercity, 4 heritage & mountain), m (bit per group of the lines calling), sp (spacing,
m), mz (the zoom from which the spacing spans one pixel, 512 px tiles: the map shows a stop once
it spans a few) and en (the name's English where it truly differs: names.py english_at).

usage: stations.py
"""
from __future__ import annotations

import json
import math
import re
from collections import defaultdict
from pathlib import Path

import names

ROOT = Path(__file__).resolve().parent.parent
S = ROOT / "data" / "rail" / "stops"
OUT = ROOT / "data" / "build" / "stations.json"

TRAM, METRO, COMMUTER, INTERCITY, HERITAGE = range(5)
INTERCITY_WORDS = (
    "tgv", "inoui", "ouigo", "intercités", "intercites", "eurostar", "thalys", "lyria", "ave ", "alvia", "euromed",
    "iryo", "avlo", "talgo", "alfa pendular", "intercidades", "amtrak", "via rail", "acela", "lner", "avanti",
    "crosscountry", "cross country", "sleeper", "night riviera", "nightjet", "intercity", "inter city", "inter-city",
    "enterprise", "adirondack", "maple leaf", "vermonter", "ethan allen", "downeaster", "lake shore", "the canadian",
    "the ocean", "northeast regional", "hull chelsea", "transcantábrico", "costa verde express", "al andalus",
    "shinkansen", "新幹線", "特急", "limited express", "高鐵", "自強", "thsr",
)


def group(t: dict) -> int | None:
    """Service group of a route relation (extract.rs rail_route)."""
    route = t.get("route")
    tourist = t.get("service") in ("tourism", "heritage") or t.get("tourism") == "yes" or t.get("historic") == "yes" \
        or t.get("railway:preserved") == "yes" or t.get("heritage:railway") == "yes"
    if route == "tram":
        return HERITAGE if tourist else TRAM
    if route in ("subway", "light_rail", "monorail"):
        return METRO
    if route == "funicular":
        return HERITAGE
    if route == "train":
        if tourist:
            return HERITAGE
        sv = t.get("service")
        if sv in ("long_distance", "high_speed", "night", "international", "car_shuttle"):
            return INTERCITY
        if sv:
            return COMMUTER
        text = " ".join(t.get(k, "") for k in ("name", "brand", "network", "operator")).lower()
        return INTERCITY if any(w in text for w in INTERCITY_WORDS) else COMMUTER
    return None


def unescape(v: str) -> str:
    return re.sub(r"%([0-9a-fA-F]+)%", lambda m: chr(int(m.group(1), 16)), v)


def dist(a, b) -> float:
    la1, la2 = math.radians(a[1]), math.radians(b[1])
    h = math.sin((la2 - la1) / 2) ** 2 + math.cos(la1) * math.cos(la2) * math.sin(math.radians(b[0] - a[0]) / 2) ** 2
    return 12742000 * math.asin(min(1.0, math.sqrt(h)))


def centre(g: dict) -> tuple[float, float] | None:
    t, c = g["type"], g["coordinates"]
    pts = [c] if t == "Point" else c if t == "LineString" else c[0] if t == "Polygon" else c[0][0] if t == "MultiPolygon" else []
    if not pts:
        return None
    return sum(p[0] for p in pts) / len(pts), sum(p[1] for p in pts) / len(pts)


def norm(name: str) -> str:
    return re.sub(r"\s+", " ", re.sub(r"\b(station|gare|estación|estação|halt|stop|platform|quai|voie|bahnhof|駅)\b|[()（）.,'’-]", " ", name.lower())).strip()


def main() -> None:
    objs: dict[str, tuple[tuple[float, float], str, bool]] = {}
    for line in (S / "stops.geojsonseq").open(encoding="utf-8"):
        f = json.loads(line.lstrip("\x1e"))
        p = f["properties"]
        c = centre(f["geometry"])
        if c:
            station = p.get("railway") in ("station", "halt") or p.get("public_transport") == "station"
            objs[f"{p['@type'][0]}{p['@id']}"] = (c, p.get("name") or "", station)

    # Stops (merged by name within 800 m, unnamed within 100 m), each with its lines' spacing.
    cells: dict[tuple, list[int]] = defaultdict(list)
    stops: list[dict] = []

    def stop_of(key: str) -> int | None:
        o = objs.get(key)
        if not o:
            return None
        (x, y), name, station = o
        nm = norm(name)
        r = 800 if nm else 100
        k = 0.02 if nm else 0.002  # cell size, degrees (≥ r at these latitudes)
        cx, cy = int(x / k), int(y / k)
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                for i in cells[(nm, k, cx + dx, cy + dy)]:
                    s = stops[i]
                    if dist(s["c"], (x, y)) <= r:
                        if station and not s["station"]:
                            s["c"], s["station"] = (x, y), True  # the dot goes on the station
                        s["names"][name] += 1
                        return i
        stops.append({"c": (x, y), "station": station, "names": defaultdict(int, {name: 1}), "sp": 0.0, "g": None, "m": 0})
        cells[(nm, k, cx, cy)].append(len(stops) - 1)
        return len(stops) - 1

    routes = 0
    for line in (S / "relations.opl").open(encoding="utf-8"):
        tm = re.search(r" T(\S*)", line)
        mm = re.search(r" M(\S*)", line)
        if not tm or not mm:
            continue
        tags = dict(kv.split("=", 1) for kv in tm.group(1).split(",") if "=" in kv)
        tags = {unescape(k): unescape(v) for k, v in tags.items()}
        g = group(tags)
        if g is None:
            continue
        members = [m.split("@", 1) for m in mm.group(1).split(",") if "@" in m]
        stop_m = [k for k, role in members if role.startswith("stop")]
        if len(stop_m) < 2:
            stop_m = [k for k, role in members if role.startswith("platform")]
        seq: list[int] = []
        for k in stop_m:
            i = stop_of(k)
            if i is not None and (not seq or seq[-1] != i):
                seq.append(i)
        if len(set(seq)) < 2:
            continue
        routes += 1
        length = sum(dist(stops[a]["c"], stops[b]["c"]) for a, b in zip(seq, seq[1:]))
        sp = min(200000.0, length / (len(seq) - 1))
        for i in set(seq):
            s = stops[i]
            s["m"] |= 1 << g
            if sp > s["sp"]:
                s["sp"], s["g"] = sp, g

    feats = []
    for s in stops:
        if s["g"] is None or s["sp"] < 50:
            continue
        (x, y) = s["c"]
        name = max(s["names"].items(), key=lambda kv: (bool(kv[0]), kv[1]))[0]
        mz = math.log2(40075016.7 * math.cos(math.radians(y)) / (512 * s["sp"]))
        en = names.english_at(name, (x, y))
        feats.append({"type": "Feature", "geometry": {"type": "Point", "coordinates": [round(x, 6), round(y, 6)]},
                      "properties": {"n": name, "g": s["g"], "m": s["m"], "sp": round(s["sp"]), "mz": round(mz, 2), **({"en": en} if en else {})}})
    # Widest spacing first: drawn below the rest, labelled first.
    feats.sort(key=lambda f: -f["properties"]["sp"])
    OUT.write_text(json.dumps({"type": "FeatureCollection", "features": feats}, ensure_ascii=False, separators=(",", ":")))
    by = defaultdict(list)
    for f in feats:
        by[f["properties"]["g"]].append(f["properties"]["sp"])
    print(f"{routes} routes, {len(feats)} stops → {OUT}")
    for g, v in sorted(by.items()):
        v.sort()
        print(f"  group {g}: {len(v)} stops, spacing median {v[len(v) // 2]} m")


if __name__ == "__main__":
    main()
