#!/usr/bin/env python3
"""Passenger ferries: lines, their service groups, and how often they sail.

Geometry and descriptions come from OpenStreetMap: ways tagged route=ferry and ferry route
relations (osmium extracts in data/ferries/, see the Makefile). A *line* is one service in both
directions: the relations of a route_master, or the direction variants of the same ref / the same
pair of ports, or a named route=ferry way (or run of them) that no route relation uses.

Frequencies are never estimated. Each line's sailings come from, in order:
  1. a timetable feed (GTFS) the operator publishes (data/ferries/freq/gtfs-*.json, gtfs.py),
  2. a published timetable looked up by hand (data/ferries/freq/timetables-*.json, with the page
     it came from),
  3. the line's OSM interval and opening_hours tags, when they give both the headway and the day's
     span of service.
Lines with none of these stay "unknown".

Service groups: urban & commuter (city water buses and commuter boats), short crossings, long-
distance & overnight (2 h 30 or more), cable & chain ferries.

Outputs (data/build/): ferries.json (GeoJSON: one feature per way, plus terminals) and
ferry-lines.json (every line's details, for the hover card).

usage: ferries.py
"""
from __future__ import annotations

import json
import math
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
F = ROOT / "data" / "ferries"
OUT = ROOT / "data" / "build"

URBAN, CROSSING, LONG, CABLE = 0, 1, 2, 3
GROUP_NAMES = ["Urban & commuter", "Short crossing", "Long-distance & overnight", "Cable & chain ferry"]

# Networks and operators of city transit systems (water buses, commuter boats). Lower case,
# matched as substrings of network / operator.
URBAN_NETS = [
    "mbta", "nyc ferry", "ny waterway", "staten island", "seastreak", "london river", "thames clipper", "uber boat",
    "transtejo", "soflusa", "mistral", "rtm", "batobus", "star ferry", "sun ferry", "hong kong & kowloon", "hkkf",
    "fortune ferry", "coral sea", "discovery bay", "park island", "tsui wah", "chuen kee", "halifax transit",
    "toronto island", "navette fluviale", "navibus", "tan", "semitan", "batcub", "tbm", "yélo", "yelo", "ctrl",
    "bibus", "vaporetto", "casco bay", "boston harbor city cruises", "city cruises", "new york city ferry",
    "gosport ferry", "nexus", "mersey ferries", "clyde", "bus de mer", "navette maritime", "tpm", "stm", "exo",
    "metrobus", "rhode island public transit", "ripta", "bateau-bus", "catamaran", "metro de", "tmb",
]
CABLE_WORDS = ["cable ferry", "chain ferry", "bac à câble", "traille", "reaction ferry", "floating bridge", "chain link ferry", "transbordador"]
ROAD_FERRY = {"motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential", "service", "track", "yes", "regular", "local"}
MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]


def unesc(s: str) -> str:
    return re.sub(r"%([0-9a-fA-F]+)%", lambda m: chr(int(m.group(1), 16)), s)


def read_relations(path: Path) -> list[dict]:
    rels = []
    for line in path.open(encoding="utf-8"):
        parts = line.rstrip("\n").split(" ")
        r = {"id": int(parts[0][1:]), "tags": {}, "members": []}
        for p in parts[1:]:
            if p.startswith("T") and len(p) > 1:
                for kv in p[1:].split(","):
                    if "=" in kv:
                        k, v = kv.split("=", 1)
                        r["tags"][unesc(k)] = unesc(v)
            elif p.startswith("M") and len(p) > 1:
                for m in p[1:].split(","):
                    typ, rest = m[0], m[1:]
                    ref, _, role = rest.partition("@")
                    r["members"].append((typ, int(ref), unesc(role)))
        rels.append(r)
    return rels


def seg_km(coords: list) -> float:
    d = 0.0
    for (x0, y0), (x1, y1) in zip(coords, coords[1:]):
        k = math.cos(math.radians((y0 + y1) / 2))
        d += math.hypot((x1 - x0) * k, y1 - y0) * 111.195
    return d


def minutes(v: str | None) -> float | None:
    """OSM duration / interval: HH:MM, H:MM:SS, 90, 1h30, 10min, 48:00."""
    if not v:
        return None
    v = v.strip().lower().replace(" ", "")
    m = re.fullmatch(r"(\d+):(\d{1,2})(?::(\d{1,2}))?", v)
    if m:
        return int(m[1]) * 60 + int(m[2]) + (int(m[3] or 0) / 60)
    m = re.fullmatch(r"(\d+)h(?:(\d+)(?:m|min)?)?", v)
    if m:
        return int(m[1]) * 60 + int(m[2] or 0)
    m = re.fullmatch(r"(\d+(?:\.\d+)?)(?:m|min|mins|minutes)?", v)
    if m:
        return float(m[1])
    return None


def daily_span(oh: str | None) -> float | None:
    """Hours of service per day from opening_hours, only when it is one simple daily span."""
    if not oh:
        return None
    oh = oh.strip()
    if oh == "24/7":
        return 24 * 60
    m = re.fullmatch(r"(?:Mo-Su\s+)?(\d{1,2}):(\d{2})-(\d{1,2}):(\d{2})", oh)
    if not m:
        return None
    a, b = int(m[1]) * 60 + int(m[2]), int(m[3]) * 60 + int(m[4])
    if b <= a:
        b += 24 * 60
    return b - a


def season_of(tags: dict) -> tuple[int, str]:
    """(category, text) from OSM tags: 0 unknown, 1 year-round daily, 2 year-round some days, 3 seasonal."""
    s = (tags.get("seasonal") or "").lower()
    oh = (tags.get("opening_hours") or "").strip()
    ohl = oh.lower()
    if s and s != "no":
        return 3, {"yes": "Seasonal"}.get(s, "Seasonal (" + s.replace(";", ", ") + ")")
    if re.search(r"\b(" + "|".join(MONTHS) + r")\b", ohl) or "agu" in ohl:
        return 3, "Seasonal: " + oh
    if ohl == "24/7" or re.fullmatch(r"(mo-su\s+)?\d{1,2}:\d{2}-\d{1,2}:\d{2}", ohl):
        return 1, "Daily" + ("" if ohl == "24/7" else " " + oh.replace("Mo-Su ", ""))
    if re.match(r"^(mo-fr|mo-sa|sa|su|sa-su)\b", ohl) and "mo-su" not in ohl and not re.search(r"(sa|su)[ -]", ohl.split(";")[-1] if ";" in ohl else ""):
        return 2, oh
    if s == "no":
        return 1 if not oh else 0, "Year-round"
    return 0, ""


MONTH_DAYS = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
_MSTART = [sum(MONTH_DAYS[:i]) for i in range(12)]
_DATE = re.compile(r"(?:(early|mid|late)[\s-]*)?(?:(\d{1,2})\s*)?(" + "|".join(MONTHS) + r")[a-z]*\.?(?:\s+(\d{1,2})(?![\d:]))?", re.I)


def months_of_text(text: str) -> float | None:
    """Months a year a line runs, from a season text: "Apr-Oct", "mid-Jun–15 Sep", "late May-early
    Sep", "4 Jul-30 Aug daily; 3 Apr-2 Jul & 5 Sep-1 Nov weekends" (ranges united, wrapping over
    the new year), "year-round" / "all year" / reduced winter service = 12. None if no months."""
    t = (text or "").lower()
    if not t:
        return None
    if re.search(r"year[- ]round|all year|toute l'année|todo el año|reduced|winter (timetable|service)", t):
        return 12.0
    days = [False] * 365

    def doy(m: re.Match, end: bool) -> int:
        mod, day, mon = m.group(1), m.group(2) or m.group(4), MONTHS.index(m.group(3)[:3].lower())
        if day:
            d = int(day) - 1
        elif mod:
            d = {"early": 4, "mid": 14, "late": 24}[mod.lower()]
        else:
            d = MONTH_DAYS[mon] - 1 if end else 0
        return _MSTART[mon] + min(d, MONTH_DAYS[mon] - 1)

    found = False
    # Ranges "A–B" (hyphen, en dash, "to"), or single months.
    for part in re.split(r"[;,&]| and |\(|\)|\+", t):
        ms = list(_DATE.finditer(part))
        i = 0
        while i < len(ms):
            a = ms[i]
            b = ms[i + 1] if i + 1 < len(ms) and re.fullmatch(r"\s*(\d{4})?\s*(-|–|—|to|au|a|até)\s*", part[a.end():ms[i + 1].start()]) else None
            if b:
                x, y = doy(a, False), doy(b, True)
                i += 2
            else:
                x, y = doy(a, False), doy(a, True)
                i += 1
            found = True
            k = x
            while True:
                days[k] = True
                if k == y:
                    break
                k = (k + 1) % 365
    if not found:
        return None
    return max(0.5, round(sum(days) / (365 / 12) * 2) / 2)


def carries_vehicles(t: dict) -> bool:
    if any(t.get(k) in ("yes", "designated", "permissive") for k in ("motorcar", "motor_vehicle", "vehicle", "hgv")):
        return True
    return (t.get("ferry") or "") in ROAD_FERRY and t.get("motor_vehicle") != "no" and t.get("motorcar") != "no"


def closed(t: dict) -> bool:
    return t.get("access") in ("private", "no") and t.get("foot") not in ("yes", "designated", "permissive")


def is_cable(t: dict) -> bool:
    text = " ".join(t.get(k, "") for k in ("name", "name:en", "description", "note")).lower()
    return (t.get("ferry") in ("cable", "chain", "chain_tray") or t.get("ferry:cable") == "yes" or t.get("cable") == "yes"
            or any(w in text for w in CABLE_WORDS))


def is_urban(t: dict) -> bool:
    text = " ".join(t.get(k, "") for k in ("network", "operator", "network:short", "operator:short", "brand")).lower()
    return any(re.search(r"(?<![\w])" + re.escape(w.strip()) + r"(?![\w])", text) for w in URBAN_NETS)


def clean_name(n: str) -> str:
    # PTv2 direction names: "Ferry X: A => B" / "A → B" → keep the line name part.
    n = re.split(r"\s*:\s*(?=[^:]*(=>|→|->|-->|—>))", n)[0] if re.search(r"(=>|→|->)", n) else n
    return n.strip()


def line_ends(coords_list: list[list]) -> list[list[float]]:
    """The two ends of a line made of several ways: the farthest-apart pair of way ends (the ports;
    with each direction mapped as its own way, the ends aren't the ones used once)."""
    ends = list({(round(p[0], 5), round(p[1], 5)) for c in coords_list for p in (c[0], c[-1])})
    ends = [list(p) for p in ends]
    if len(ends) == 1:
        return [ends[0], ends[0]]
    best, pair = -1.0, (ends[0], ends[1])
    for i in range(len(ends)):
        for j in range(i + 1, len(ends)):
            d = seg_km([ends[i], ends[j]])
            if d > best:
                best, pair = d, (ends[i], ends[j])
    return [pair[0], pair[1]]


def main() -> None:
    ways = {}
    for line in (F / "ways.geojsonseq").open(encoding="utf-8"):
        line = line.lstrip("\x1e").strip()
        if not line:
            continue
        f = json.loads(line)
        if f["geometry"]["type"] != "LineString":
            continue
        p = f["properties"]
        ways[int(p["@id"])] = {"tags": {k: v for k, v in p.items() if not k.startswith("@")}, "coords": f["geometry"]["coordinates"]}
    rels = read_relations(F / "relations.opl")
    routes = {r["id"]: r for r in rels if r["tags"].get("type") == "route" and r["tags"].get("route") == "ferry"}
    masters = [r for r in rels if r["tags"].get("type") == "route_master"]
    master_of = {}
    for m in masters:
        for typ, ref, _ in m["members"]:
            if typ == "r" and ref in routes:
                master_of[ref] = m

    # ---- Lines -------------------------------------------------------------------
    groups: dict[tuple, list[int]] = defaultdict(list)
    for rid, r in routes.items():
        t = r["tags"]
        if closed(t):
            continue
        who = (t.get("network") or t.get("operator") or "").lower()
        if rid in master_of:
            key = ("m", master_of[rid]["id"])
        elif t.get("ref"):
            key = ("ref", who, t["ref"].lower())
        elif t.get("from") and t.get("to"):
            key = ("ports", who, tuple(sorted((t["from"].lower(), t["to"].lower()))))
        else:
            key = ("r", rid)
        groups[key].append(rid)

    lines: dict[str, dict] = {}
    way_lines: dict[int, list[str]] = defaultdict(list)
    for key, rids in groups.items():
        rids.sort()
        lid = f"r{rids[0]}"
        base = dict(master_of[rids[0]]["tags"]) if key[0] == "m" else {}
        tags: dict = {}
        for rid in rids:  # first relation wins per key, the route master's tags on top
            for k, v in routes[rid]["tags"].items():
                tags.setdefault(k, v)
        for k, v in base.items():
            if k not in ("type", "route_master"):
                tags[k] = v
        wids = []
        for rid in rids:
            for typ, ref, role in routes[rid]["members"]:
                if typ == "w" and ref in ways and role in ("", "forward", "backward", "main", "route") and ref not in wids:
                    wids.append(ref)
        if not wids:
            continue
        lines[lid] = {"tags": tags, "ways": wids, "rels": rids}
        for w in wids:
            way_lines[w].append(lid)

    # Named ferry ways outside any route relation: one line per (name, operator).
    loose: dict[tuple, list[int]] = defaultdict(list)
    for wid, w in ways.items():
        t = w["tags"]
        if t.get("route") != "ferry" or wid in way_lines or closed(t):
            continue
        key = (t.get("name", "").lower(), (t.get("operator") or "").lower()) if t.get("name") else ("way", wid)
        loose[key].append(wid)
    for key, wids in loose.items():
        wids.sort()
        lid = f"w{wids[0]}"
        tags = dict(ways[wids[0]]["tags"])
        for wid in wids[1:]:
            for k, v in ways[wid]["tags"].items():
                tags.setdefault(k, v)
        lines[lid] = {"tags": tags, "ways": wids, "rels": []}
        for w in wids:
            way_lines[w].append(lid)

    # ---- Frequencies from feeds and timetables ------------------------------------
    freq: dict[str, dict] = {}
    for src in sorted((F / "freq").glob("*.json")) if (F / "freq").exists() else []:
        for rec in json.loads(src.read_text()):
            lid = rec.get("line")
            if isinstance(rec.get("headway"), str):  # "20 minutes", "2 hours (peak)"
                m = re.match(r"\s*(\d+(?:\.\d+)?)\s*(h|hour|hours|hr)?", rec["headway"])
                rec["headway"] = (float(m[1]) * (60 if m[2] else 1)) if m else None
            if rec.get("per_day") is None and rec.get("per_week") is not None:
                rec["per_day"] = round(rec["per_week"] / 7, 3)
            if rec.get("per_day") is None and rec.get("headway") is None:
                continue
            if lid in lines and (lid not in freq or rank(rec) < rank(freq[lid])):
                freq[lid] = rec

    out_lines = {}
    n_src = defaultdict(int)
    for lid, L in lines.items():
        t = L["tags"]
        km = sum(seg_km(ways[w]["coords"]) for w in L["ways"])
        dur = minutes(t.get("duration"))
        if is_cable(t) or any(is_cable(ways[w]["tags"]) for w in L["ways"]):
            g = CABLE
        elif (dur is not None and dur >= 150) or (dur is None and km >= 80):
            g = LONG
        elif is_urban(t):
            g = URBAN
        else:
            g = CROSSING
        season, season_text = season_of(t)
        vehicles = carries_vehicles(t) or any(carries_vehicles(ways[w]["tags"]) for w in L["ways"])
        rec = freq.get(lid)
        info = {
            "name": clean_name(t.get("name") or t.get("name:en") or ""),
            "ref": t.get("ref", ""),
            "operator": t.get("operator", ""),
            "network": t.get("network", ""),
            "from": t.get("from", ""),
            "to": t.get("to", ""),
            "via": t.get("via", ""),
            "group": g,
            "km": round(km, 1),
            "duration": dur,
            "vehicles": vehicles,
            "bicycle": t.get("bicycle", ""),
            "roundtrip": t.get("roundtrip") == "yes",
            "colour": t.get("colour", ""),
            "website": t.get("website") or t.get("url") or t.get("operator:website") or "",
            "wikidata": t.get("wikidata", ""),
            "osm": [f"relation/{r}" for r in L["rels"]] or [f"way/{w}" for w in L["ways"][:3]],
            "season": season,
            "seasonText": season_text,
            "ends": line_ends([ways[w]["coords"] for w in L["ways"]]),
        }
        season_txt_osm = t.get("seasonal", "") + " " + (t.get("opening_hours") or "")
        if rec:
            info["freq"] = {k: rec[k] for k in ("per_day", "per_day_low", "headway", "days", "months", "overnight", "source", "url", "checked") if k in rec}
            if rec.get("season"):
                info["season"] = {"year": 1, "some-days": 2, "seasonal": 3}.get(rec["season"], info["season"])
                info["seasonText"] = rec.get("season_text") or info["seasonText"]
            n_src[rec.get("kind", "timetable")] += 1
        else:
            hw, span = minutes(t.get("interval")), daily_span(t.get("opening_hours"))
            if hw and 1 <= hw <= 24 * 60:
                info["freq"] = {"headway": hw, "source": "OpenStreetMap interval tag", "url": f"https://www.openstreetmap.org/{info['osm'][0]}"}
                if span and hw <= span:
                    info["freq"]["per_day"] = round(span / hw)
                    n_src["osm"] += 1
                else:
                    n_src["osm (headway only)"] += 1
        # Months a year: year-round lines 12; seasonal ones from their published months (research or
        # GTFS), else their OSM season/opening hours; unknown otherwise.
        mo = None
        if info["season"] in (1, 2):
            mo = 12.0
        else:
            mo = months_of_text(info.get("freq", {}).get("months", ""))
            if mo is None and info["season"] == 3:
                mo = months_of_text(info["seasonText"]) or months_of_text(season_txt_osm)
        if mo is not None:
            info["months"] = mo
        out_lines[lid] = info

    # ---- Features -----------------------------------------------------------------
    PRIORITY = [LONG, CROSSING, URBAN, CABLE]
    feats = []
    for wid, lids in way_lines.items():
        infos = [out_lines[l] for l in lids]
        gs = {i["group"] for i in infos}
        g = next(x for x in PRIORITY if x in gs)
        # Sailings over the way: lines between different ports add up (routes sharing a stretch);
        # lines between the same two ports (duplicate OSM relations, one operator's line and an
        # all-operator one) count once, at the highest figure.
        groups: list[tuple[list, float]] = []  # (ends, highest sailings) per pair of ports
        for i in infos:
            pd = i.get("freq", {}).get("per_day")
            if pd is None:
                continue
            a, b = i["ends"]
            for gi, (ends, best) in enumerate(groups):
                c, d = ends
                if max(seg_km([a, c]), seg_km([b, d])) < 3 or max(seg_km([a, d]), seg_km([b, c])) < 3:
                    groups[gi] = (ends, max(best, pd))
                    break
            else:
                groups.append(([a, b], pd))
        known = [g[1] for g in groups]
        seasons = [i["season"] for i in infos if i["season"]]
        colour = next((i["colour"] for i in infos if i["colour"]), "")
        props = {
            "g": g,
            "gb": sum(1 << x for x in gs),
            "car": int(any(i["vehicles"] for i in infos)),
            "f": round(sum(known), 1) if known else -1,
            # Shortest published headway (min) where no line on the way has a daily count.
            "hw": min((i["freq"]["headway"] for i in infos if i.get("freq", {}).get("headway")), default=0) if not known else 0,
            "fp": int(any(i.get("freq", {}).get("per_day") is None for i in infos)) if known else 0,  # partial: some lines unknown
            "s": min(seasons) if seasons else 0,
            # Months a year (the longest-running line on the way), -1 unknown.
            "m": max((i["months"] for i in infos if "months" in i), default=-1),
            "col": colour,
            "op": next((i["operator"] or i["network"] for i in infos if i["operator"] or i["network"]), ""),
            "n": next((i["name"] for i in infos if i["name"]), ""),
            "lines": ",".join(lids),
        }
        coords = [[round(x, 6), round(y, 6)] for x, y in ways[wid]["coords"]]
        feats.append({"type": "Feature", "id": wid, "geometry": {"type": "LineString", "coordinates": coords}, "properties": props})

    # Terminals: named ferry terminals within 1 km of a line's ends.
    ends = []
    for L in lines.values():
        for w in L["ways"]:
            c = ways[w]["coords"]
            ends += [c[0], c[-1]]
    cell = defaultdict(list)
    for x, y in ends:
        cell[(round(x * 50), round(y * 50))].append((x, y))
    n_term = 0
    tpath = F / "terminals.geojsonseq"
    if tpath.exists():
        for line in tpath.open(encoding="utf-8"):
            line = line.lstrip("\x1e").strip()
            if not line:
                continue
            f = json.loads(line)
            name = f["properties"].get("name")
            if not name:
                continue
            geom = f["geometry"]
            if geom["type"] == "Point":
                x, y = geom["coordinates"]
            else:
                ring = {"Polygon": lambda c: c[0], "MultiPolygon": lambda c: c[0][0]}.get(geom["type"], lambda c: c)(geom["coordinates"])
                x, y = sum(p[0] for p in ring) / len(ring), sum(p[1] for p in ring) / len(ring)
            near = any(
                seg_km([[x, y], [ex, ey]]) < 1.0
                for dx in (-1, 0, 1) for dy in (-1, 0, 1)
                for ex, ey in cell.get((round(x * 50) + dx, round(y * 50) + dy), [])
            )
            if near:
                n_term += 1
                feats.append({"type": "Feature", "geometry": {"type": "Point", "coordinates": [round(x, 6), round(y, 6)]},
                              "properties": {"kind": "terminal", "n": name}})

    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "ferries.json").write_text(json.dumps({"type": "FeatureCollection", "features": feats}, ensure_ascii=False, separators=(",", ":")))
    (OUT / "ferry-lines.json").write_text(json.dumps(out_lines, ensure_ascii=False, separators=(",", ":")))
    (F / "lines.json").write_text(json.dumps({lid: {**i, "tags": lines[lid]["tags"]} for lid, i in out_lines.items()}, ensure_ascii=False, indent=1))
    by_g = defaultdict(int)
    for i in out_lines.values():
        by_g[GROUP_NAMES[i["group"]]] += 1
    print(f"{len(out_lines)} lines on {len(way_lines)} ways, {n_term} terminals; groups: {dict(by_g)}")
    print(f"frequency known for {sum(1 for i in out_lines.values() if i.get('freq', {}).get('per_day') is not None)} lines; sources: {dict(n_src)}")


def rank(rec: dict) -> int:
    return {"gtfs": 0, "timetable": 1}.get(rec.get("kind", "timetable"), 2)


if __name__ == "__main__":
    sys.exit(main())
