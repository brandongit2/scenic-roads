#!/usr/bin/env python3
"""The MTR's lines (Hong Kong) as a static GTFS zip for inputs/timetables/gtfs/ (docs/inputs.md
§6.7.2): from the research (--mtr: `mtr.json`, researched by hand from MTR's published
frequencies, one entry per line or branch: its stations in order, `per_day`, `first`, `last`,
`note`, `source`, `url`, `checked`), a route per entry and a count row per pair of consecutive
stations, holding the trains a day each way.

MTR counts are exact only for the Airport Express and High Speed Rail, whose timetables MTR
publishes in full. For the other lines MTR publishes average headways per named period (morning
peak, off-peak…) without the periods' clock times, plus first and last trains, so their trains a
day are a lower bound (`at_least` 1): the service hours at the slowest published weekday off-peak
headway, read from the entry's note ("weekday off-peak 3.6-5 min"). Off-peak, LOHAS Park's branch
is the shuttle from Tiu Keng Leng (through trains run at peaks only). An entry with neither a count
nor such a note, or with fewer than two stations placed, has no route.

Stations are stops, one per name as normalised (`norm`: a station on two lines is one stop), placed
by either:
- --stations: Hong Kong's stations from OSM as GeoJSON lines (a pass's filtered stations), by their
  English names, a `railway=station` first;
- --pairs: the stop pairs the research was turned into before (the rail sources' old `mtr-pairs`:
  little-endian f32 lon_a, lat_a, lon_b, lat_b, u8 mode, f32 trains), read back in the order they
  were written, so the stops are where those pairs had them (the conversion of #141).
Two consecutive stations at one place (two platforms of one stop) make no row. Route types: Light
Rail 0 (tram), the High Speed Rail 2 (rail), the rest 1 (metro).

usage: mtr.py --mtr mtr.json (--stations hk-stations.geojsonseq | --pairs mtr-pairs.bin) --out mtr.zip
"""
from __future__ import annotations

import argparse
import json
import re
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import gtfszip  # noqa: E402


def norm(s: str) -> str:
    s = re.sub(r"\(.*?\)", " ", s.lower().replace("’", "'"))
    s = re.sub(r"\b(station|stop|mtr|light rail|lr)\b", " ", s)
    return re.sub(r"[^a-z0-9]+", " ", s).strip()


def lower_bound(L: dict) -> tuple[int, str] | None:
    """Trains a day from the service hours at the slowest published weekday off-peak headway."""
    m = re.search(r"weekday off-peak (\d+(?:\.\d+)?)(?:-(\d+(?:\.\d+)?))? min", L.get("note", ""))
    if not m or not L.get("first") or not L.get("last"):
        return None
    hw = float(m.group(2) or m.group(1))
    hm = lambda t: int(t[:2]) * 60 + int(t[3:5])
    a, b = hm(L["first"]), hm(L["last"])
    if b <= a:
        b += 24 * 60
    return int((b - a) / hw) + 1, f"{L['first']}–{L['last']} every {hw:g} min"


def counted(lines: list[dict]) -> list[tuple[dict, list[str], int | float, bool, str]]:
    """Each entry with a count: (entry, its stations as counted, trains a day, a lower bound, how)."""
    out = []
    for L in lines:
        per_day, lower, how = L.get("per_day"), False, ""
        stations = L.get("stations", [])
        if not per_day:
            lb = lower_bound(L)
            if not lb:
                continue
            per_day, lower, how = lb[0], True, lb[1]
            if L.get("branch") == "LOHAS Park" and "Tiu Keng Leng" in stations:
                stations = stations[stations.index("Tiu Keng Leng"):]
        out.append((L, stations, per_day, lower, how))
    return out


def places_from_stations(path: Path) -> dict[str, tuple[float, float]]:
    places: dict[str, list[tuple[float, float, int]]] = {}
    for line in path.open(encoding="utf-8"):
        line = line.lstrip("\x1e").strip()
        if not line:
            continue
        f = json.loads(line)
        p, g = f["properties"], f["geometry"]
        if g["type"] == "Point":
            x, y = g["coordinates"]
        else:
            ring = {"Polygon": lambda c: c[0], "MultiPolygon": lambda c: c[0][0], "MultiLineString": lambda c: c[0]}.get(g["type"], lambda c: c)(g["coordinates"])
            x, y = sum(c[0] for c in ring) / len(ring), sum(c[1] for c in ring) / len(ring)
        rank = 0 if p.get("railway") == "station" else 1
        for k in ("name:en", "name", "alt_name"):
            if p.get(k):
                for part in re.split(r"[;/]", p[k]):
                    n = norm(re.sub(r"^[^\x00-\x7f]+\s*", "", part))
                    if n:
                        places.setdefault(n, []).append((x, y, rank))
    # (Each place as f32, as the counter stores it.)
    f32 = lambda v: struct.unpack("<f", struct.pack("<f", v))[0]
    return {n: (f32(x), f32(y)) for n, c in places.items() for x, y, _ in [min(c, key=lambda c: c[2])]}


def places_from_pairs(path: Path, entries) -> dict[str, tuple[float, float]]:
    """The stations' places as the old pairs had them: walking the entries' consecutive stations in
    the order the pairs were written, each pair of distinct names the next two records (A to B, B to
    A), which must agree with every place read so far."""
    b = path.read_bytes()
    recs = [struct.unpack("<ffffBf", b[i:i + 21]) for i in range(0, len(b), 21)]
    places: dict[str, tuple[float, float]] = {}
    i = 0
    for L, stations, per_day, _, _ in entries:
        for a, c in zip(stations, stations[1:]):
            na, nc = norm(a), norm(c)
            if na == nc:
                continue
            ok = i + 1 < len(recs)
            if ok:
                r, s = recs[i], recs[i + 1]
                pa, pc = (r[0], r[1]), (r[2], r[3])
                ok = r[5] == float(per_day) and (s[0], s[1], s[2], s[3]) == (pc[0], pc[1], pa[0], pa[1]) and places.get(na, pa) == pa and places.get(nc, pc) == pc
            if not ok:
                sys.exit(f"the pairs don't follow the research at {L['line']}: {a} – {c} (record {i})")
            places[na], places[nc] = pa, pc
            i += 2
    if i != len(recs):
        sys.exit(f"{len(recs) - i} pairs left over: the research doesn't match these pairs")
    return places


def slug(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--mtr", required=True)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--stations")
    g.add_argument("--pairs")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    lines = json.loads(Path(a.mtr).read_text())
    entries = counted(lines)
    places = places_from_stations(Path(a.stations)) if a.stations else places_from_pairs(Path(a.pairs), entries)
    stops: dict[str, dict] = {}
    routes, counts, missing = [], [], []
    for L, stations, per_day, lower, how in entries:
        name = L["line"] + (f" ({L['branch']})" if L.get("branch") else "")
        rid = slug(name)
        rows = []
        for x, y in zip(stations, stations[1:]):
            nx, ny = norm(x), norm(y)
            px, py = places.get(nx), places.get(ny)
            for st, p in ((x, px), (y, py)):
                if p is None:
                    missing.append(f"{name}: {st}")
            if px is None or py is None or px == py:
                continue
            for st, n, p in ((x, nx, px), (y, ny, py)):
                stops.setdefault(n, {"stop_id": slug(n), "stop_name": re.sub(r"\s*\(.*?\)", "", st), "stop_lat": repr(p[1]), "stop_lon": repr(p[0])})
            rows.append({"route_id": rid, "from_stop_id": slug(nx), "to_stop_id": slug(ny), "at_least": 1 if lower else None, "per_day": per_day, "note": how or None})
        if not rows:
            continue
        low = L["line"].lower()
        rtype = 0 if "light rail" in low else 2 if "high speed" in low else 1
        routes.append({"route_id": rid, "agency_id": "mtr", "route_short_name": L.get("branch") if "light rail" in low else None, "route_long_name": name, "route_type": rtype, "route_url": L["url"], "checked": L.get("checked")})
        counts += rows
    gtfszip.write(Path(a.out), {
        "agency.txt": [{"agency_id": "mtr", "agency_name": "MTR Corporation", "agency_url": "https://www.mtr.com.hk/", "agency_timezone": "Asia/Hong_Kong"}],
        "stops.txt": list(stops.values()),
        "routes.txt": routes,
        "counts.txt": counts,
        "feed_info.txt": [{"feed_publisher_name": "Researched by hand from MTR's published frequencies", "feed_publisher_url": "https://www.mtr.com.hk/en/customer/services/train_service_index.html", "feed_lang": "en"}],
    })
    print(f"{len(routes)} routes, {len(stops)} stops, {len(counts)} count rows; stations not placed: {len(missing)}", file=sys.stderr)
    for m in missing[:40]:
        print("  ", m, file=sys.stderr)


if __name__ == "__main__":
    main()
