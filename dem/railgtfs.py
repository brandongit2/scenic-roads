#!/usr/bin/env python3
"""Passenger rail service frequency from published timetables (GTFS): trains between consecutive
stops on a typical weekday, for matching onto the OSM track network (`railfreq`). The `rail` job
runs it (docs/plan.md §6, Rail service).

Feeds (--feeds: railfeeds.py's list, in its order, each with `path`, its zip, and `fetched`, the day
its timetable counts from), each read:
  - rail routes only (route types: tram, metro, rail, funicular, monorail and the extended ones);
  - the typical day: the Tuesday, Wednesday or Thursday from 30 days before the feed's day to 90
    days after with the median number of rail trips, so holidays don't count; feeds with none are
    stale. The window follows the feed, not the day it's read, so a zip gives the same counts
    whenever it's read (--today puts every feed's window at one day instead);
  - every trip that runs that day (headway-based trips expanded), as its stop sequence;
  - trains that don't run every day (the Canadian twice a week): where a stop pair has no train on
    the typical day, the trains of that day's Monday–Sunday week ÷ 7 (so a weekly train reads
    as 0.14 a day rather than none).
The same train published in two feeds (a national feed and a regional one, an aggregate and its
parts) is counted once: trips are keyed by first and last stop (to ~1 km) and times (to the
minute), across all feeds. Where that doesn't catch them (an aggregate republishing an operator's
feed with its own times), the operator's feed `replaces` the other, which is left out.

Writes --out: per (stop A, stop B, mode) the number of trains that day from A to B; little-endian
f32 lon_a, lat_a, lon_b, lat_b, u8 mode (0 tram, 1 metro, 2 rail, 3 funicular), f32 trains. Every
pair, wherever its stops are: the job then marks the stops beyond the coverage (+0x20 for A, +0x40
for B: a cross-border service, which railfreq runs as far as the track goes toward it) and leaves
out pairs with both beyond (pipeline::rail). And --used: per feed its day, trips and duplicates, or
why it was left out.

usage: railgtfs.py --feeds feeds.json --out pairs.bin --used used.json [--today YYYY-MM-DD]
"""
from __future__ import annotations

import argparse
import csv
import io
import json
import struct
import sys
import zipfile
from collections import defaultdict
from datetime import date, timedelta
from pathlib import Path


def mode_of(t: int) -> int | None:
    if t in (0, 5) or 900 <= t <= 999:
        return 0
    if t in (1, 12) or 400 <= t <= 499:
        return 1
    if t == 2 or 100 <= t <= 199:
        return 3 if t == 116 else 2
    if t in (7, 1400):
        return 3
    return None


def rows(z: zipfile.ZipFile, name: str):
    names = {Path(n).name: n for n in z.namelist()}
    if name not in names:
        return
    with z.open(names[name]) as f:
        r = csv.reader(io.TextIOWrapper(f, encoding="utf-8-sig", errors="replace", newline=""))
        head = [h.strip() for h in next(r, [])]  # some feeds (Renfe) pad names and values with spaces
        for row in r:
            yield {k: v.strip() for k, v in zip(head, row)}


def secs(t: str) -> int:
    try:
        h, m, s = t.strip().split(":")
        return int(h) * 3600 + int(m) * 60 + int(s)
    except ValueError:
        return -1


def ymd(s: str) -> date:
    return date(int(s[:4]), int(s[4:6]), int(s[6:8]))


def rail_routes(z: zipfile.ZipFile) -> dict[str, tuple[int, str]]:
    """The feed's rail routes: route id → (mode, short name)."""
    routes = {}
    for r in rows(z, "routes.txt"):
        try:
            m = mode_of(int(r.get("route_type", "")))
        except ValueError:
            m = None
        if m is not None:
            routes[r["route_id"]] = (m, r.get("route_short_name") or r.get("route_long_name") or "")
    return routes


def service_days(z: zipfile.ZipFile, anchor: date) -> dict[str, set[date]]:
    """Each service's days within 400 days of `anchor` (calendar.txt, then calendar_dates.txt)."""
    days: dict[str, set[date]] = defaultdict(set)
    for c in rows(z, "calendar.txt"):
        try:
            d, end = ymd(c["start_date"]), ymd(c["end_date"])
        except (KeyError, ValueError):
            continue
        wd = [c.get(k) == "1" for k in ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")]
        end = min(end, anchor + timedelta(days=400))
        d = max(d, anchor - timedelta(days=400))
        while d <= end:
            if wd[d.weekday()]:
                days[c["service_id"]].add(d)
            d += timedelta(days=1)
    for c in rows(z, "calendar_dates.txt"):
        try:
            d = ymd(c["date"])
        except (KeyError, ValueError):
            continue
        (days[c["service_id"]].add if c.get("exception_type") == "1" else days[c["service_id"]].discard)(d)
    return days


def typical_day(trips: dict[str, tuple[str, str]], days: dict[str, set[date]], anchor: date) -> date | None:
    """The median-busy Tuesday–Thursday from 30 days before `anchor` to 90 after, by rail trips; None
    when there's no weekday service in that window (a stale feed)."""
    cand = [anchor + timedelta(days=k) for k in range(-30, 91)]
    cand = [d for d in cand if d.weekday() in (1, 2, 3)]
    per_service = defaultdict(int)
    for _, sid in trips.values():
        per_service[sid] += 1
    counts = {d: sum(n for sid, n in per_service.items() if d in days.get(sid, ())) for d in cand}
    busy = sorted((n, d) for d, n in counts.items() if n > 0)
    return busy[len(busy) // 2][1] if busy else None


def has_service(zpath: Path, anchor: date) -> bool:
    """Whether a feed's zip has rail service in its window (railfeeds.py's check of a cached feed)."""
    with zipfile.ZipFile(zpath) as z:
        routes = rail_routes(z)
        trips = {t["trip_id"]: (t["route_id"], t["service_id"]) for t in rows(z, "trips.txt") if t.get("route_id") in routes}
        return bool(routes) and typical_day(trips, service_days(z, anchor), anchor) is not None


def process(zpath: Path, anchor: date, seen: set, pairs: dict, weekly: dict, seen_week: set) -> dict:
    z = zipfile.ZipFile(zpath)
    routes = rail_routes(z)
    if not routes:
        return {"status": "no rail routes"}
    trips = {t["trip_id"]: (t["route_id"], t["service_id"]) for t in rows(z, "trips.txt") if t.get("route_id") in routes}
    days = service_days(z, anchor)
    day = typical_day(trips, days, anchor)
    if day is None:
        return {"status": "stale (no weekday service in the window)"}
    active = {tid for tid, (_, sid) in trips.items() if day in days.get(sid, ())}
    # Its week, for trains that don't run that day: trip → days it runs that week.
    week = [day - timedelta(days=day.weekday()) + timedelta(days=k) for k in range(7)]
    in_week = {tid: n for tid, (_, sid) in trips.items() if tid not in active and (n := sum(d in days.get(sid, ()) for d in week))}
    runs: dict[str, list[int]] = defaultdict(list)
    for fr in rows(z, "frequencies.txt"):
        if fr.get("trip_id") in active:
            a, b, hw = secs(fr["start_time"]), secs(fr["end_time"]), int(float(fr.get("headway_secs") or 0) or 0)
            if a >= 0 and b > a and hw > 0:
                runs[fr["trip_id"]] += list(range(a, b, hw))
    stops = {}
    for s in rows(z, "stops.txt"):
        try:
            ll = (float(s["stop_lon"]), float(s["stop_lat"]))
        except (KeyError, ValueError):
            continue
        if abs(ll[0]) > 0.01 or abs(ll[1]) > 0.01:  # 0,0: no location given
            stops[s["stop_id"]] = ll
    calls: dict[str, list[tuple[int, str, int]]] = defaultdict(list)
    for st in rows(z, "stop_times.txt"):
        tid = st.get("trip_id")
        if tid in active or tid in in_week:
            try:
                seq = int(st["stop_sequence"])
            except (KeyError, ValueError):
                continue
            calls[tid].append((seq, st["stop_id"], secs(st.get("departure_time") or st.get("arrival_time") or "")))
    n_trips = n_dup = 0
    for tid, c in calls.items():
        c.sort()
        c = [x for x in c if x[1] in stops]  # calls at stops with no location are passed over
        pts = [stops[s] for _, s, _ in c]
        if len(c) < 2:
            continue
        mode = routes[trips[tid][0]][0]
        t0, t1 = c[0][2], c[-1][2]
        if tid in in_week:
            # Not on the typical day: its runs that week, a seventh each.
            key = (round(pts[0][0], 2), round(pts[0][1], 2), t0 // 60, round(pts[-1][0], 2), round(pts[-1][1], 2), t1 // 60, mode)
            if key in seen_week:
                continue
            seen_week.add(key)
            for a, b in zip(pts, pts[1:]):
                if a != b:
                    weekly[(round(a[0], 5), round(a[1], 5), round(b[0], 5), round(b[1], 5), mode)] += in_week[tid] / 7
            continue
        starts = runs.get(tid) or [None]
        for off in starts:
            dt = 0 if off is None else off - (t0 if t0 >= 0 else 0)
            key = (round(pts[0][0], 2), round(pts[0][1], 2), (t0 + dt) // 60, round(pts[-1][0], 2), round(pts[-1][1], 2), (t1 + dt) // 60, mode)
            if key in seen:
                n_dup += 1
                continue
            seen.add(key)
            n_trips += 1
            for a, b in zip(pts, pts[1:]):
                if a != b:
                    k = (round(a[0], 5), round(a[1], 5), round(b[0], 5), round(b[1], 5), mode)
                    pairs[k] += 1
    return {"status": "ok", "day": day.isoformat(), "trips": n_trips, "duplicates": n_dup, "rail_routes": len(routes)}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--feeds", required=True, help="railfeeds.py's list, with each zip's path")
    ap.add_argument("--out", required=True, help="the stop pairs")
    ap.add_argument("--used", required=True, help="per feed: its day, trips and duplicates")
    ap.add_argument("--today", help="every feed's window at this day (YYYY-MM-DD), not the day it was fetched")
    a = ap.parse_args()
    feeds = json.loads(Path(a.feeds).read_text())["feeds"]
    today = date.fromisoformat(a.today) if a.today else None
    replaced = {r: f["id"] for f in feeds if f.get("path") for r in f.get("replaces", ())}
    pairs: dict[tuple, int] = defaultdict(int)
    weekly: dict[tuple, float] = defaultdict(float)
    seen: set = set()
    seen_week: set = set()
    used = []
    for k, feed in enumerate(feeds):
        print(f"progress: {k}/{len(feeds)} feeds", file=sys.stderr, flush=True)
        facts = {key: feed[key] for key in ("id", "provider", "url", "licence", "fetched") if key in feed}
        if feed["id"] in replaced:
            used.append({**facts, "status": f"replaced by {replaced[feed['id']]}"})
            continue
        if not feed.get("path"):
            used.append({**facts, "status": feed.get("status") or "no zip"})
            continue
        anchor = today or date.fromisoformat(feed["fetched"])
        try:
            res = process(Path(feed["path"]), anchor, seen, pairs, weekly, seen_week)
        except (zipfile.BadZipFile, KeyError, csv.Error) as e:
            res = {"status": f"unreadable: {e}"}
        used.append({**facts, **res})
        print(f"{feed['id']:<28} {feed.get('provider', '')[:40]:<40} {res.get('status')}  {res.get('day', '')} trips {res.get('trips', 0)} dup {res.get('duplicates', 0)}", file=sys.stderr, flush=True)
    print(f"progress: {len(feeds)}/{len(feeds)} feeds", file=sys.stderr, flush=True)
    # Pairs with no train on the typical day take their weekly average.
    n_week = 0
    for k, v in weekly.items():
        if not pairs.get(k):
            pairs[k] = v
            n_week += 1
    out = Path(a.out)
    tmp = out.with_name(out.name + ".tmp")
    with tmp.open("wb") as f:
        for (ax, ay, bx, by, m), n in pairs.items():
            f.write(struct.pack("<ffffBf", ax, ay, bx, by, m, float(n)))
    tmp.replace(out)
    print(f"{n_week} stop pairs served only on other days of the week (weekly average)", file=sys.stderr)
    Path(a.used).write_text(json.dumps(used, ensure_ascii=False, indent=1))
    ok = [u for u in used if u.get("status") == "ok"]
    print(f"{len(pairs)} stop pairs from {len(ok)} feeds, {sum(u['trips'] for u in ok)} trains, {sum(u['duplicates'] for u in ok)} duplicates dropped", file=sys.stderr)


if __name__ == "__main__":
    main()
