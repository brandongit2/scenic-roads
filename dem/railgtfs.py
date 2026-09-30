#!/usr/bin/env python3
"""Passenger rail service frequency from published timetables (GTFS): trains between consecutive
stops on a typical weekday, for matching onto the OSM track network (`railfreq`).

Feeds: data/rail/feeds.json (railfeeds.py: every GTFS feed in the Mobility Database catalogue that
runs rail in our regions) plus EXTRA below (national operators the catalogue lacks). Each is
downloaded (data/rail/gtfs, big ones deleted afterwards) and read:
  - rail routes only (route types: tram, metro, rail, funicular, monorail and the extended ones);
  - the typical day: the Tuesday, Wednesday or Thursday within the next 90 days (or the last 30)
    with the median number of rail trips, so holidays don't count; feeds with none are stale;
  - every trip that runs that day (headway-based trips expanded), as its stop sequence;
  - trains that don't run every day (the Canadian twice a week): where a stop pair has no train on
    the typical day, the trains of that day's Monday–Sunday week ÷ 7 (so a weekly train reads
    as 0.14 a day rather than none).
The same train published in two feeds (a national feed and a regional one, an aggregate and its
parts) is counted once: trips are keyed by first and last stop (to ~1 km) and times (to the
minute), across all feeds. Where that doesn't catch them (an aggregate republishing an operator's
feed with its own times), the operator's feed `replaces` the other, which is left out.

Writes data/rail/pairs.bin: per (stop A, stop B, mode) the number of trains that day from A to B;
little-endian f32 lon_a, lat_a, lon_b, lat_b, u8 mode (0 tram, 1 metro, 2 rail, 3 funicular;
+0x20 when stop A is beyond the map's regions, +0x40 when B is: a cross-border service, which
railfreq runs as far as the track goes toward it; pairs with both beyond are left out), f32
trains. And data/rail/feeds-used.json (feed, day, trips, share deduplicated).

usage: railgtfs.py [feed-id ...]
"""
from __future__ import annotations

import csv
import io
import json
import statistics
import struct
import subprocess
import sys
import zipfile
from collections import defaultdict
from datetime import date, timedelta
from pathlib import Path

from shapely import contains_xy, prepare

from leaftype import regions

ROOT = Path(__file__).resolve().parent.parent
R = ROOT / "data" / "rail"
CACHE = R / "gtfs"
UA = "road-elevations/0.1 (personal offline map)"
KEEP_MB = 5000  # downloads bigger than this are deleted once read (cached for reruns)

EXTRA = [
    {"id": "sncf", "provider": "SNCF Voyageurs (TGV INOUI, OUIGO, Intercités, TER)", "url": "https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip", "licence": "Licence Ouverte 2.0"},
    {"id": "renfe-av-ld-md", "provider": "Renfe (AVE, Larga y Media Distancia)", "url": "https://ssl.renfe.com/gtransit/Fichero_AV_LD/google_transit.zip", "licence": "CC BY 4.0"},
    {"id": "renfe-cercanias", "provider": "Renfe Cercanías / Rodalies", "url": "https://ssl.renfe.com/ftransit/Fichero_CER_FOMENTO/fomento_transit.zip", "licence": "CC BY 4.0"},
    # Great Britain: the national rail timetable (RDG's ATOC CIF) as GTFS, rebuilt daily by Catenary
    # Transit (the official download needs a Rail Data Marketplace account).
    {"id": "gb-national-rail", "provider": "National Rail (GB), RDG timetable via Catenary Transit", "url": "https://github.com/catenarytransit/pfaedled-gtfs-actions/releases/download/latest/nationalrailuk.zip", "licence": "RDG open data (attribution)"},
    {"id": "hk-pfaedle", "provider": "Hong Kong Transport Department (trams, Peak Tram), via Catenary Transit", "url": "https://github.com/catenarytransit/pfaedled-gtfs-actions/releases/download/latest/hk-gtfs-pfaedle.zip", "licence": "DATA.GOV.HK terms",
     "local": str(ROOT / "data" / "rail" / "gtfs" / "hk-pfaedle.zip")},
]

# Feeds behind an API key (data/keys.env, KEY=value lines, git-ignored): added when their key is
# there; `get` names how the download link is obtained.
KEYS = ROOT / "data" / "keys.env"


def keys() -> dict[str, str]:
    if not KEYS.exists():
        return {}
    return {k.strip(): v.strip() for k, _, v in (l.partition("=") for l in KEYS.read_text().splitlines()) if v.strip() and not k.startswith("#")}


def keyed_feeds() -> list[dict]:
    k = keys()
    out = []
    if k.get("LTA_ACCOUNT_KEY"):
        # (The Mobility Database's Singapore feed has the same trains, mostly at other times.)
        out.append({"id": "sg-lta-train", "provider": "Land Transport Authority, LTA DataMall (MRT and LRT)", "licence": "Singapore Open Data Licence 1.0",
                    "url": "https://datamall2.mytransport.sg/ltaodataservice/GTFSScheduleTrain", "get": "lta", "replaces": ["mdb-1076"]})
    return out


def keyed_link(feed: dict) -> str | None:
    """The download link of a keyed feed (LTA: a signed link, valid 15 minutes, in the answer)."""
    import ast
    import urllib.request

    if feed["get"] == "lta":
        req = urllib.request.Request(feed["url"], headers={"User-Agent": UA, "AccountKey": keys()["LTA_ACCOUNT_KEY"], "accept": "application/json"})
        v = json.loads(urllib.request.urlopen(req, timeout=60).read())["value"]
        v = ast.literal_eval(v) if isinstance(v, str) else v
        return v[0]["link"] if v else None
    return None


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


def fetch(feed: dict) -> Path | None:
    if feed.get("local") and Path(feed["local"]).exists():
        return Path(feed["local"])
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / f"{feed['id']}.zip"
    if path.exists() and zipfile.is_zipfile(path):
        return path
    tmp = path.with_suffix(".part")
    url = keyed_link(feed) if feed.get("get") else feed["url"]
    if not url:
        return None
    r = subprocess.run(["curl", "-sSL", "--fail", "-m", "3600", "-A", UA, "-o", str(tmp), url])
    if r.returncode != 0 or not zipfile.is_zipfile(tmp):
        tmp.unlink(missing_ok=True)
        return None
    tmp.rename(path)
    return path


def process(feed: dict, zpath: Path, seen: set, pairs: dict, weekly: dict, seen_week: set) -> dict:
    z = zipfile.ZipFile(zpath)
    routes = {}
    for r in rows(z, "routes.txt"):
        try:
            m = mode_of(int(r.get("route_type", "")))
        except ValueError:
            m = None
        if m is not None:
            routes[r["route_id"]] = (m, r.get("route_short_name") or r.get("route_long_name") or "")
    if not routes:
        return {"status": "no rail routes"}
    trips = {t["trip_id"]: (t["route_id"], t["service_id"]) for t in rows(z, "trips.txt") if t.get("route_id") in routes}
    # Service days.
    days: dict[str, set[date]] = defaultdict(set)
    for c in rows(z, "calendar.txt"):
        try:
            d, end = ymd(c["start_date"]), ymd(c["end_date"])
        except (KeyError, ValueError):
            continue
        wd = [c.get(k) == "1" for k in ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")]
        end = min(end, date.today() + timedelta(days=400))
        d = max(d, date.today() - timedelta(days=400))
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
    # The typical day: median-busy Tue–Thu in the window.
    today = date.today()
    cand = [today + timedelta(days=k) for k in range(-30, 91)]
    cand = [d for d in cand if d.weekday() in (1, 2, 3)]
    per_service = defaultdict(int)
    for _, sid in trips.values():
        per_service[sid] += 1
    counts = {d: sum(n for sid, n in per_service.items() if d in days.get(sid, ())) for d in cand}
    busy = sorted((n, d) for d, n in counts.items() if n > 0)
    if not busy:
        return {"status": "stale (no weekday service in the window)"}
    n_med, day = busy[len(busy) // 2]
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
    only = set(sys.argv[1:])
    feeds = json.loads((R / "feeds.json").read_text())
    # National operators first (so duplicates in regional aggregates are the ones dropped).
    order = EXTRA + keyed_feeds() + feeds
    replaced = {r: f["id"] for f in order for r in f.get("replaces", ())}
    pairs: dict[tuple, int] = defaultdict(int)
    weekly: dict[tuple, float] = defaultdict(float)
    seen: set = set()
    seen_week: set = set()
    used = []
    for feed in order:
        if only and feed["id"] not in only:
            continue
        if feed["id"] in replaced:
            used.append({**{k: feed[k] for k in ("id", "provider", "url", "licence") if k in feed}, "status": f"replaced by {replaced[feed['id']]}"})
            continue
        zpath = fetch(feed)
        if not zpath:
            print(f"{feed['id']}: download failed", file=sys.stderr)
            used.append({**feed, "status": "download failed"})
            continue
        try:
            res = process(feed, zpath, seen, pairs, weekly, seen_week)
        except (zipfile.BadZipFile, KeyError, csv.Error) as e:
            res = {"status": f"unreadable: {e}"}
        used.append({**{k: feed[k] for k in ("id", "provider", "url", "licence") if k in feed}, **res})
        print(f"{feed['id']:<28} {feed['provider'][:40]:<40} {res.get('status')}  {res.get('day', '')} trips {res.get('trips', 0)} dup {res.get('duplicates', 0)}", file=sys.stderr, flush=True)
        if not feed.get("local") and zpath.stat().st_size > KEEP_MB * 1e6:
            zpath.unlink()
    # Pairs with no train on the typical day take their weekly average.
    n_week = 0
    for k, v in weekly.items():
        if not pairs.get(k):
            pairs[k] = v
            n_week += 1
    region = regions()
    prepare(region)
    n_beyond = 0
    with (R / "pairs.bin").open("wb") as f:
        for (ax, ay, bx, by, m), n in pairs.items():
            out_a, out_b = not contains_xy(region, ax, ay), not contains_xy(region, bx, by)
            if out_a and out_b:
                continue
            n_beyond += out_a or out_b
            f.write(struct.pack("<ffffBf", ax, ay, bx, by, m | 0x20 * out_a | 0x40 * out_b, float(n)))
    print(f"{n_beyond} stop pairs with one stop beyond the map", file=sys.stderr)
    print(f"{n_week} stop pairs served only on other days of the week (weekly average)", file=sys.stderr)
    (R / "feeds-used.json").write_text(json.dumps(used, ensure_ascii=False, indent=1))
    ok = [u for u in used if u.get("status") == "ok"]
    print(f"{len(pairs)} stop pairs from {len(ok)} feeds, {sum(u['trips'] for u in ok)} trains, {sum(u['duplicates'] for u in ok)} duplicates dropped", file=sys.stderr)


if __name__ == "__main__":
    main()
