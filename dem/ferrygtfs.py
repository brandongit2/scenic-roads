#!/usr/bin/env python3
"""The ferry counter (docs/inputs.md §6.7.0): ferry sailings per OSM ferry line from timetable feeds
(GTFS), for the `ferries` job (ferries.py), which runs it on the declared feeds the `rail-feeds` job
fetched (docs/inputs.md §6.7.1), each zip with the day it was fetched.

For every ferry line (its two ends), counts the timetabled ferry trips that call at both of its
ports, i.e. stop within 400 m (at most 30 % of the distance between them) of each end of the line.
Per day of service: departures each way (both directions averaged). Reported per line:
  per_day      median over the days the line runs; lines not sailing both ways most days: the
               weekly count ÷ 7 (a weekly crossing reads "1 a week"),
  per_day_low  median in the quietest month (feeds covering a whole year only),
  headway      median gap between departures from the first port, 07:00–19:00, on a typical day,
  days         weekdays with service (e.g. "daily", "Mo–Fr"),
  months       months with service over the 12 months from the feed's day, when the line's
               timetable breaks for a month or more (seasonal); a timetable that just ends says
               nothing about the season.
The reference day is the day the feed's zip was fetched, so a zip gives the same counts whenever
it's read. Only service dates from a year before it to 400 days after count (a feed with none is
stale). Only ferry routes are counted (route_type 4, 1000–1099, 1200–1299, or the route ids the
declaration lists in `ferry_routes` when the feed labels its ferries as something else).

A static file compiled by hand (docs/inputs.md §6.7.2, `static=True`) is counted the same way over
one reference year, its calendar read as months and days (dem/staticgtfs.py), its days from 1
January of that year; its records are looked-up timetables' (`kind` "timetable"), each with the
source its first matched route names (`route_long_name`, `route_url`, `checked`). A route pinned to
an OSM line (`osm`) counts for that line alone, its ports the calls nearest the line's two ends,
however far.
"""
from __future__ import annotations

import csv
import io
import math
import statistics
import sys
import zipfile
from collections import defaultdict
from datetime import date, timedelta
from pathlib import Path

import staticgtfs

R_KM = 0.4
DAYS = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]


def km(a, b) -> float:
    k = math.cos(math.radians((a[1] + b[1]) / 2))
    return math.hypot((a[0] - b[0]) * k, a[1] - b[1]) * 111.195


def is_ferry(route_type: str) -> bool:
    try:
        t = int(route_type)
    except ValueError:
        return False
    return t == 4 or 1000 <= t <= 1099 or 1200 <= t <= 1299


def rows(z: zipfile.ZipFile, name: str):
    names = {Path(n).name: n for n in z.namelist()}
    if name not in names:
        return
    with z.open(names[name]) as f:
        yield from csv.DictReader(io.TextIOWrapper(f, encoding="utf-8-sig", newline=""))


def secs(t: str) -> int:
    h, m, s = (t.strip() or "0:0:0").split(":")
    return int(h) * 3600 + int(m) * 60 + int(s)


def ymd(s: str) -> date:
    return date(int(s[:4]), int(s[4:6]), int(s[6:8]))


def days_text(active_wd: set[int]) -> str:
    if len(active_wd) == 7:
        return "daily"
    runs, cur = [], []
    for d in range(7):
        if d in active_wd:
            cur.append(d)
        elif cur:
            runs.append(cur)
            cur = []
    if cur:
        runs.append(cur)
    return ", ".join(DAYS[r[0]] if len(r) == 1 else f"{DAYS[r[0]]}–{DAYS[r[-1]]}" for r in runs)


def months_text(ms: set[int]) -> str:
    # Contiguous runs, wrapping over the new year (e.g. Nov–Mar).
    if len(ms) == 12:
        return "all year"
    start = next((m for m in range(12) if m in ms and (m - 1) % 12 not in ms), None)
    if start is None:
        return "all year"
    runs, m, cur = [], start, []
    for _ in range(12):
        if m in ms:
            cur.append(m)
        elif cur:
            runs.append(cur)
            cur = []
        m = (m + 1) % 12
    if cur:
        runs.append(cur)
    return ", ".join(MONTHS[r[0]] if len(r) == 1 else f"{MONTHS[r[0]]}–{MONTHS[r[-1]]}" for r in runs)


def ferry_routes(z: zipfile.ZipFile, feed: dict) -> dict[str, dict]:
    """The feed's ferry routes, by id: by their type, or listed in its `ferry_routes`."""
    extra = set(feed.get("ferry_routes") or [])
    return {r["route_id"]: r for r in rows(z, "routes.txt") if is_ferry(r.get("route_type", "")) or r["route_id"] in extra}


def service_dates(z: zipfile.ZipFile, day: date) -> dict[str, set[date]]:
    """Each service's dates (calendar.txt, then calendar_dates.txt), from a year before `day` to 400
    days after."""
    dates: dict[str, set[date]] = defaultdict(set)
    for c in rows(z, "calendar.txt"):
        d, end = ymd(c["start_date"]), ymd(c["end_date"])
        wd = [c[k] == "1" for k in ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")]
        while d <= end:
            if wd[d.weekday()]:
                dates[c["service_id"]].add(d)
            d += timedelta(days=1)
    for c in rows(z, "calendar_dates.txt"):
        d = ymd(c["date"])
        (dates[c["service_id"]].add if c["exception_type"] == "1" else dates[c["service_id"]].discard)(d)
    lo, hi = day - timedelta(days=365), day + timedelta(days=400)
    for sid in dates:
        dates[sid] = {d for d in dates[sid] if lo <= d <= hi}
    return dates


def has_service(zpath: Path, feed: dict, day: date) -> bool:
    """Whether a feed's zip has ferry service around `day` (the fetching job's check of a copy)."""
    with zipfile.ZipFile(zpath) as z:
        routes = ferry_routes(z, feed)
        trips = {t["trip_id"]: t for t in rows(z, "trips.txt") if t["route_id"] in routes}
        dates = service_dates(z, day)
        return any(dates.get(t["service_id"]) for t in trips.values())


def process(feed: dict, zpath: Path, lines: dict, day: date | None, static: bool = False, unmatched: list | None = None) -> list[dict]:
    """The records of the lines of `lines` (id → {"ends": [a, b]}) a feed's ferries sail, counted
    from `day`, the day its zip was fetched. `feed`: its `id`, `name`, `url`, `source` (the page
    about it) and `ferry_routes`. A static file (`static`): see the module's docs. `unmatched`, when
    given, gets the routes with service in the window that match no line (`unmatched_route`)."""
    z = zipfile.ZipFile(zpath)
    if static:
        day = date(staticgtfs.REF_YEAR, 1, 1)
    routes = ferry_routes(z, feed)
    if not routes:
        print(f"  {feed['id']}: no ferry routes", file=sys.stderr)
        return []
    trips = {t["trip_id"]: t for t in rows(z, "trips.txt") if t["route_id"] in routes}
    agencies = {a.get("agency_id", ""): a.get("agency_name", "") for a in rows(z, "agency.txt")}
    stops = {}
    for s in rows(z, "stops.txt"):
        try:
            stops[s["stop_id"]] = (float(s["stop_lon"]), float(s["stop_lat"]))
        except (ValueError, KeyError):
            pass
    calls: dict[str, list[tuple[int, str, int]]] = defaultdict(list)  # trip → (seq, stop, departure s)
    for st in rows(z, "stop_times.txt"):
        tid = st["trip_id"]
        if tid in trips:
            t = st.get("departure_time") or st.get("arrival_time") or ""
            calls[tid].append((int(st["stop_sequence"]), st["stop_id"], secs(t) if t else -1))
    for c in calls.values():
        c.sort()
    # Service dates: the current timetable only, from a year before the feed's day to a year and a
    # bit after. A feed with nothing in that window is stale (its lines are left to the looked-up
    # timetables).
    today = day
    lo = today - timedelta(days=365)
    dates = staticgtfs.service_dates(z) if static else service_dates(z, today)
    pins = {rid: r["osm"].strip() for rid, r in routes.items() if static and (r.get("osm") or "").strip()}
    # Headway-based trips: departures per trip run.
    runs: dict[str, list[int]] = defaultdict(list)  # trip → start offsets (s)
    for fr in rows(z, "frequencies.txt"):
        if fr["trip_id"] in trips:
            a, b, hw = secs(fr["start_time"]), secs(fr["end_time"]), int(fr["headway_secs"])
            runs[fr["trip_id"]] += list(range(a, b, max(hw, 60)))
    all_dates = sorted({d for tid in trips for d in dates.get(trips[tid]["service_id"], ())})
    if not all_dates:
        print(f"  {feed['id']}: " + ("no timed sailings" if static else f"stale (no service since {lo})"), file=sys.stderr)
        return []
    feed_name = feed.get("name") or feed["id"]

    # Only stops ferries call at, and lines with both ports near one.
    fstops = {s: stops[s] for c in calls.values() for _, s, _ in c if s in stops}
    if not fstops:
        return []
    xs, ys = [p[0] for p in fstops.values()], [p[1] for p in fstops.values()]
    box = (min(xs) - 0.02, min(ys) - 0.02, max(xs) + 0.02, max(ys) + 0.02)
    inside = lambda p: box[0] <= p[0] <= box[2] and box[1] <= p[1] <= box[3]

    out = []
    pinned_lines = set(pins.values())
    all_matched: set = set()
    for lid, L in lines.items():
        a, b = L["ends"]
        sep = km(a, b)
        if sep < 0.05:
            continue  # round trips: no two ports to match
        if lid in pinned_lines:
            near_a = near_b = set()
        else:
            if not (inside(a) and inside(b)):
                continue
            r = min(R_KM, sep * 0.3)
            near_a = {s for s, p in fstops.items() if km(p, a) <= r}
            near_b = {s for s, p in fstops.items() if km(p, b) <= r}
            if not near_a or not near_b:
                continue
        per_date = [defaultdict(int), defaultdict(int)]
        deps_a: dict[date, list[int]] = defaultdict(list)
        matched_routes = set()
        for tid, c in calls.items():
            pin = pins.get(trips[tid]["route_id"])
            if pin is not None:
                # Pinned: this line's alone, its ports the calls nearest its ends.
                at = [i for i, (_, s, _) in enumerate(c) if s in stops]
                if pin != lid or len(at) < 2:
                    continue
                ia = min(at, key=lambda i: km(stops[c[i][1]], a))
                ib = min(at, key=lambda i: km(stops[c[i][1]], b))
            else:
                ia = next((i for i, (_, s, _) in enumerate(c) if s in near_a), None)
                ib = next((i for i, (_, s, _) in enumerate(c) if s in near_b), None)
            if ia is None or ib is None or ia == ib:
                continue
            d = 0 if ia < ib else 1
            starts = runs.get(tid) or [0]
            t0 = c[ia][2]
            matched_routes.add(trips[tid]["route_id"])
            all_matched.add(trips[tid]["route_id"])
            for day in dates.get(trips[tid]["service_id"], ()):
                per_date[d][day] += len(starts)
                if d == 0 and t0 >= 0:
                    deps_a[day] += [t0 + s if runs.get(tid) else t0 for s in starts]
        both = bool(per_date[0]) and bool(per_date[1])
        days = sorted(set(per_date[0]) | set(per_date[1]))
        if not days:
            continue
        per_day = {d: (per_date[0][d] + per_date[1][d]) / (2 if both else 1) for d in days}
        # Sailings a day on the days it runs; for lines running fewer than 4 days a week, the
        # weekly count spread over the week (a weekly crossing is "1 a week", not "1 a day").
        weeks = defaultdict(list)
        for d in days:
            weeks[d.isocalendar()[:2]].append(d)
        days_per_week = statistics.median(len(v) for v in weeks.values())
        daily = days_per_week >= 4 and statistics.median(per_day.values()) >= 1
        if daily:
            typical = statistics.median(per_day.values())
        else:
            typical = statistics.median(sum(per_day[d] for d in v) for v in weeks.values()) / 7
        names = sorted({agencies.get(routes[x].get("agency_id", ""), "") for x in matched_routes} - {""})
        who = ", ".join(names[:2]) if names and len(agencies) > 1 else feed_name
        rec = {
            "line": lid,
            "kind": "gtfs",
            "per_day": round(typical, 2 if typical < 1 else 1),
            "source": f"{who} timetable (GTFS)",
            "url": feed.get("source") or feed["url"],
            "checked": f"{all_dates[0].isoformat()} to {all_dates[-1].isoformat()}",
            "gtfs_routes": sorted(routes[x].get("route_short_name") or routes[x].get("route_long_name") or x for x in matched_routes),
        }
        if static:
            first = routes[min(matched_routes)]
            rec["kind"] = "timetable"
            rec["source"] = first.get("route_long_name") or first.get("route_short_name") or feed_name
            rec["url"] = first.get("route_url", "")
            rec["checked"] = first.get("checked", "")
        # Weekdays with service in at least half the weeks the line runs.
        wd = {w for w in range(7) if sum(any(d.weekday() == w for d in v) for v in weeks.values()) >= 0.5 * len(weeks)}
        rec["days"] = days_text(wd)
        # Season, over the next 12 months: a break of a month or more in the line's own
        # timetable means it is seasonal; a timetable running the whole year means year-round; one
        # that just ends (the next period not yet published) says nothing.
        fwd = sorted(d for d in days if today <= d < today + timedelta(days=365))
        if fwd:
            gap = any((b - a).days >= 30 for a, b in zip(fwd, fwd[1:]))
            # (A static file's year comes round again: its break may span the new year, and it
            # never just ends.)
            if static:
                gap = gap or (fwd[0] - fwd[-1]).days + 365 >= 30
            by_month = defaultdict(list)
            for d in fwd:
                by_month[d.month - 1].append(per_day[d])
            if gap:
                rec["months"] = months_text(set(by_month))
                rec["season"] = "seasonal"
            elif static or ((fwd[-1] - today).days >= 330 and (fwd[0] - today).days <= 30):
                rec["season"] = "year" if len(wd) == 7 else "some-days"
            if daily and len(by_month) >= 6:
                low = min(statistics.median(v) for v in by_month.values())
                if low < rec["per_day"] * 0.8:
                    rec["per_day_low"] = round(low, 1)
        # Typical daytime headway on the day nearest the median.
        day = min(days, key=lambda d: (abs(per_day[d] - typical), d.weekday() >= 5))
        ts = sorted(t for t in deps_a.get(day, []) if 7 * 3600 <= t <= 19 * 3600)
        if len(ts) >= 4:
            gaps = [y - x for x, y in zip(ts, ts[1:]) if y > x]
            if gaps:
                rec["headway"] = round(statistics.median(gaps) / 60)
        out.append(rec)
    if unmatched is not None:
        # Routes sailing in the window, between two ports, that no line took.
        live = {trips[tid]["route_id"] for tid, c in calls.items() if dates.get(trips[tid]["service_id"]) and len({s for _, s, _ in c if s in stops}) >= 2}
        for rid in sorted(live - all_matched):
            r = routes[rid]
            pin = pins.get(rid, "")
            unmatched.append(unmatched_route(feed["id"], static, r, agencies.get(r.get("agency_id", ""), ""), pin,
                                             "its osm pin names no ferry line of this pass" if pin and pin not in lines else
                                             "its ports match no OpenStreetMap ferry line"))
    return out


def unmatched_route(source: str, static: bool, r: dict, agency: str, pin: str, why: str) -> dict:
    """A timetable route that matches nothing on the map (docs/inputs.md §6.7.3)."""
    return {"mode": "ferry", "source": source, "static": static, "route": r.get("route_id", ""),
            "name": r.get("route_short_name") or r.get("route_long_name") or "", "agency": agency, "osm": pin, "why": why}
