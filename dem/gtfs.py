#!/usr/bin/env python3
"""Ferry sailings from operators' published timetables (GTFS).

For every ferry line in data/ferries/lines.json (ferries.py), counts the timetabled ferry trips
that call at both of its ports, i.e. stop within 400 m (at most 30 % of the distance between them) of each end of the line, from the feeds
listed in data/ferries/gtfs-feeds.json. Per day of service: departures each way (both
directions averaged). Reported per line:
  per_day      median over the days the line runs; lines not sailing both ways most days: the
               weekly count ÷ 7 (a weekly crossing reads "1 a week"),
  per_day_low  median in the quietest month (feeds covering a whole year only),
  headway      median gap between departures from the first port, 07:00–19:00, on a typical day,
  days         weekdays with service (e.g. "daily", "Mo–Fr"),
  months       months with service over the next 12 months, when the line's timetable breaks for a
               month or more (seasonal); a timetable that just ends says nothing about the season.
Only service dates from a year ago onwards count (older feeds are skipped as stale). Only ferry
routes are counted (route_type 4, 1000–1099, 1200–1299, or the route ids a feed entry
lists under "ferry_route_ids" when the feed labels its ferries as something else).

Outputs data/ferries/freq/gtfs-<feed>.json, read by ferries.py.

usage: gtfs.py [--refresh] [feed-id ...]
"""
from __future__ import annotations

import csv
import io
import json
import math
import statistics
import subprocess
import sys
import zipfile
from collections import defaultdict
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
F = ROOT / "data" / "ferries"
CACHE = F / "gtfs"
OUT = F / "freq"
R_KM = 0.4
UA = "road-elevations/0.1 (personal offline map)"
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


def download(feed: dict, refresh: bool) -> Path | None:
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / f"{feed['id']}.zip"
    if path.exists() and not refresh:
        return path
    tmp = path.with_suffix(".part")
    r = subprocess.run(["curl", "-sSL", "--fail", "-A", UA, "-o", str(tmp), feed["url"]])
    if r.returncode != 0 or not zipfile.is_zipfile(tmp):
        print(f"  {feed['id']}: download failed", file=sys.stderr)
        tmp.unlink(missing_ok=True)
        return None
    tmp.rename(path)
    return path


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


def process(feed: dict, zpath: Path, lines: dict) -> list[dict]:
    z = zipfile.ZipFile(zpath)
    extra = set(feed.get("ferry_route_ids") or [])
    routes = {r["route_id"]: r for r in rows(z, "routes.txt") if is_ferry(r.get("route_type", "")) or r["route_id"] in extra}
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
    # Service dates.
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
    # Headway-based trips: departures per trip run.
    runs: dict[str, list[int]] = defaultdict(list)  # trip → start offsets (s)
    for fr in rows(z, "frequencies.txt"):
        if fr["trip_id"] in trips:
            a, b, hw = secs(fr["start_time"]), secs(fr["end_time"]), int(fr["headway_secs"])
            runs[fr["trip_id"]] += list(range(a, b, max(hw, 60)))
    # The current timetable only: service from a year ago to a year and a bit ahead. A feed with
    # nothing in that window is stale (its lines are left to the looked-up timetables).
    today = date.today()
    lo, hi = today - timedelta(days=365), today + timedelta(days=400)
    for sid in dates:
        dates[sid] = {d for d in dates[sid] if lo <= d <= hi}
    all_dates = sorted({d for tid in trips for d in dates.get(trips[tid]["service_id"], ())})
    if not all_dates:
        print(f"  {feed['id']}: stale (no service since {lo})", file=sys.stderr)
        return []
    ops = [o for o in (feed.get("operators") or []) if o]
    feed_name = feed.get("name") or (", ".join(ops[:2]) + (" …" if len(ops) > 2 else "") if ops else feed["id"])

    # Only stops ferries call at, and lines with both ports near one.
    fstops = {s: stops[s] for c in calls.values() for _, s, _ in c if s in stops}
    if not fstops:
        return []
    xs, ys = [p[0] for p in fstops.values()], [p[1] for p in fstops.values()]
    box = (min(xs) - 0.02, min(ys) - 0.02, max(xs) + 0.02, max(ys) + 0.02)
    inside = lambda p: box[0] <= p[0] <= box[2] and box[1] <= p[1] <= box[3]

    out = []
    for lid, L in lines.items():
        a, b = L["ends"]
        if not (inside(a) and inside(b)):
            continue
        sep = km(a, b)
        if sep < 0.05:
            continue  # round trips: no two ports to match
        r = min(R_KM, sep * 0.3)
        near_a = {s for s, p in fstops.items() if km(p, a) <= r}
        near_b = {s for s, p in fstops.items() if km(p, b) <= r}
        if not near_a or not near_b:
            continue
        per_date = [defaultdict(int), defaultdict(int)]
        deps_a: dict[date, list[int]] = defaultdict(list)
        matched_routes = set()
        for tid, c in calls.items():
            ia = next((i for i, (_, s, _) in enumerate(c) if s in near_a), None)
            ib = next((i for i, (_, s, _) in enumerate(c) if s in near_b), None)
            if ia is None or ib is None or ia == ib:
                continue
            d = 0 if ia < ib else 1
            starts = runs.get(tid) or [0]
            t0 = c[ia][2]
            matched_routes.add(trips[tid]["route_id"])
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
            "url": feed.get("portal") or feed["url"],
            "checked": f"{all_dates[0].isoformat()} to {all_dates[-1].isoformat()}",
            "gtfs_routes": sorted(routes[x].get("route_short_name") or routes[x].get("route_long_name") or x for x in matched_routes),
        }
        # Weekdays with service in at least half the weeks the line runs.
        wd = {w for w in range(7) if sum(any(d.weekday() == w for d in v) for v in weeks.values()) >= 0.5 * len(weeks)}
        rec["days"] = days_text(wd)
        # Season, over the next 12 months: a break of a month or more in the line's own
        # timetable means it is seasonal; a timetable running the whole year means year-round; one
        # that just ends (the next period not yet published) says nothing.
        fwd = sorted(d for d in days if today <= d < today + timedelta(days=365))
        if fwd:
            gap = any((b - a).days >= 30 for a, b in zip(fwd, fwd[1:]))
            by_month = defaultdict(list)
            for d in fwd:
                by_month[d.month - 1].append(per_day[d])
            if gap:
                rec["months"] = months_text(set(by_month))
                rec["season"] = "seasonal"
            elif (fwd[-1] - today).days >= 330 and (fwd[0] - today).days <= 30:
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
    return out


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    refresh = "--refresh" in sys.argv
    feeds = json.loads((F / "gtfs-feeds.json").read_text())
    lines = json.loads((F / "lines.json").read_text())
    OUT.mkdir(parents=True, exist_ok=True)
    for feed in feeds:
        if args and feed["id"] not in args:
            continue
        if feed.get("status") != "verified" or not feed.get("url"):
            continue
        zpath = download(feed, refresh)
        if not zpath:
            continue
        try:
            recs = process(feed, zpath, lines)
        except Exception as e:  # a malformed feed shouldn't stop the others
            print(f"  {feed['id']}: {e}", file=sys.stderr)
            continue
        (OUT / f"gtfs-{feed['id']}.json").write_text(json.dumps(recs, ensure_ascii=False, indent=1))
        print(f"{feed['id']}: {len(recs)} lines matched")


if __name__ == "__main__":
    main()
