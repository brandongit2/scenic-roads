#!/usr/bin/env python3
"""MTR (Hong Kong) trains a day, researched by hand from MTR's published frequencies
(data/rail/mtr.json), as stop pairs for `railfreq` (data/rail/pairs-mtr.bin, same format as
railgtfs.py's). Stations are placed by their English names in OSM (data/rail/hk-stations.geojsonseq);
each consecutive pair of a line's stations gets the line's trains a day in both directions.

MTR counts are exact only for the Airport Express and High Speed Rail, whose timetables MTR
publishes in full. For the other lines MTR publishes average headways per named period (morning
peak, off-peak…) without the periods' clock times, plus first and last trains, so their trains a
day are a lower bound: the service hours at the slowest published weekday off-peak headway. Those
pairs carry mode bit 0x80 (shown as "at least").

usage: mtrpairs.py
"""
from __future__ import annotations

import json
import re
import struct
import sys
from pathlib import Path

R = Path(__file__).resolve().parent.parent / "data" / "rail"


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


def main():
    lines = json.loads((R / "mtr.json").read_text())
    places: dict[str, list[tuple[float, float, int]]] = {}
    for line in (R / "hk-stations.geojsonseq").open(encoding="utf-8"):
        line = line.lstrip("\x1e").strip()
        if not line:
            continue
        f = json.loads(line)
        p = f["properties"]
        g = f["geometry"]
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
    out = bytearray()
    missing, n_pairs = [], 0
    for L in lines:
        per_day, lower = L.get("per_day"), False
        stations = L.get("stations", [])
        if not per_day:
            lb = lower_bound(L)
            if not lb:
                continue
            per_day, lower = lb[0], True
            # LOHAS Park off-peak: the Tiu Keng Leng shuttle (through trains run at peaks only).
            if L.get("branch") == "LOHAS Park" and "Tiu Keng Leng" in stations:
                stations = stations[stations.index("Tiu Keng Leng"):]
            print(f"  {L['line']} {L.get('branch') or ''}: at least {per_day} a day ({lb[1]})", file=sys.stderr)
        mode = 0 if "light rail" in (L.get("line", "") + " " + str(L.get("branch", ""))).lower() else 1
        if "high speed" in L.get("line", "").lower() or "airport express" in L.get("line", "").lower() or "east rail" in L.get("line", "").lower():
            mode = 2 if "high speed" in L.get("line", "").lower() else 1
        pts = []
        for st in stations:
            cand = places.get(norm(st))
            if not cand:
                missing.append(f"{L['line']}: {st}")
                pts.append(None)
                continue
            x, y, _ = min(cand, key=lambda c: c[2])
            pts.append((x, y))
        for a, b in zip(pts, pts[1:]):
            if a and b and a != b:
                for p, q in ((a, b), (b, a)):
                    out += struct.pack("<ffffBf", p[0], p[1], q[0], q[1], mode | (0x80 if lower else 0), float(per_day))
                    n_pairs += 1
    (R / "pairs-mtr.bin").write_bytes(bytes(out))
    print(f"{n_pairs} MTR stop pairs; stations not found in OSM: {len(missing)}", file=sys.stderr)
    for m in missing[:40]:
        print("  ", m, file=sys.stderr)


if __name__ == "__main__":
    main()
