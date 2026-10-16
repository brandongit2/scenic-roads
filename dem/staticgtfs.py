"""Static GTFS, compiled by hand (docs/inputs.md §6.7.2): what the rail and ferry counters read of
the accepted zips of inputs/timetables/gtfs/, beyond what a feed has.

- No expiry, but seasons: `calendar.txt`'s dates are read as month and day, the year ignored, over
  one reference year (`REF_YEAR`, not a leap year: a range ending on 29 February ends on the 28th);
  a start after the end wraps the new year. `calendar_dates.txt` is ignored.
- `counts.txt`: services known only by counts, so no departure time is invented. A row: `route_id`;
  for rail, `from_stop_id` and `to_stop_id` (trains each way between them) and `at_least` (1: a
  lower bound); then `per_day`, `per_week`, `per_day_low`, `headway`, `days`, `months`, `season`,
  `overnight` and `note`, each taken as given (an empty cell: not given). Numbers are read as
  written (`10` a whole number, `1.5` not; `null`: the source read gives none), `overnight` as 1
  or 0, a `headway` that isn't a number as
  the text the source words it in.
- `routes.txt`'s extra columns: `checked` (when the source was read), `months`, and `osm` (the OSM
  line the route is pinned to: `w<id>` or `r<id>`).

usage (the counters import it): staticgtfs.py --list <zip> prints its routes and count rows.
"""
from __future__ import annotations

import csv
import io
import json
import re
import sys
import zipfile
from collections import defaultdict
from datetime import date, timedelta
from pathlib import Path

REF_YEAR = 2001
WEEKDAYS = ("monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday")
COUNT_FIELDS = ("per_day", "per_week", "per_day_low", "headway", "days", "months", "season", "overnight", "note")
NUMBERS = ("per_day", "per_week", "per_day_low", "headway")


def rows(z: zipfile.ZipFile, name: str):
    names = {Path(n).name: n for n in z.namelist()}
    if name not in names:
        return
    with z.open(names[name]) as f:
        yield from csv.DictReader(io.TextIOWrapper(f, encoding="utf-8-sig", newline=""))


def month_day(s: str) -> date:
    """A calendar date read as month and day, in the reference year."""
    s = s.strip()
    m, d = int(s[-4:-2]), int(s[-2:])
    if (m, d) == (2, 29):
        d = 28
    return date(REF_YEAR, m, d)


def service_dates(z: zipfile.ZipFile) -> dict[str, set[date]]:
    """Each service's days in the reference year, from `calendar.txt` read as months and days."""
    dates: dict[str, set[date]] = defaultdict(set)
    first, last = date(REF_YEAR, 1, 1), date(REF_YEAR, 12, 31)
    for c in rows(z, "calendar.txt"):
        try:
            a, b = month_day(c["start_date"]), month_day(c["end_date"])
        except (KeyError, ValueError):
            continue
        wd = [c.get(k, "").strip() == "1" for k in WEEKDAYS]
        spans = [(a, b)] if a <= b else [(first, b), (a, last)]
        for lo, hi in spans:
            d = lo
            while d <= hi:
                if wd[d.weekday()]:
                    dates[c["service_id"]].add(d)
                d += timedelta(days=1)
    return dates


def number(s: str):
    """A number as written: a whole number an int, else a float; None when it isn't one."""
    if re.fullmatch(r"-?\d+", s):
        return int(s)
    if re.fullmatch(r"-?(\d+\.\d*|\.\d+|\d+)([eE][-+]?\d+)?", s):
        return float(s)
    return None


def count_values(row: dict) -> dict:
    """A count row's given fields, typed (`COUNT_FIELDS`; empty cells left out)."""
    out = {}
    for k in COUNT_FIELDS:
        v = row.get(k) or ""
        if v == "":
            continue
        if k in NUMBERS:
            n = number(v)
            out[k] = None if v == "null" else n if n is not None else v
        elif k == "overnight":
            out[k] = v == "1"
        else:
            out[k] = v
    return out


def count_rows(z: zipfile.ZipFile) -> list[dict]:
    return list(rows(z, "counts.txt"))


def routes(z: zipfile.ZipFile) -> dict[str, dict]:
    return {r["route_id"]: r for r in rows(z, "routes.txt")}


def ferry_counts(z: zipfile.ZipFile, is_ferry) -> list[dict]:
    """The ferry records of a zip's count rows, in their order: the record of the route's pinned OSM
    line (`line`), its source as the route names it (`source`: route_long_name, `url`: route_url,
    `checked`) and the row's fields as given."""
    rs = routes(z)
    out = []
    for c in count_rows(z):
        r = rs.get(c.get("route_id", ""))
        if not r or not is_ferry(r.get("route_type", "")) or not (r.get("osm") or "").strip():
            continue
        rec = {"line": r["osm"].strip(), "kind": "timetable", **count_values(c)}
        for k, col in (("source", "route_long_name"), ("url", "route_url"), ("checked", "checked")):
            if r.get(col):
                rec[k] = r[col]
        out.append(rec)
    return out


def main():
    a = sys.argv[1:]
    if len(a) != 2 or a[0] != "--list":
        sys.exit("usage: staticgtfs.py --list <zip>")
    with zipfile.ZipFile(a[1]) as z:
        print(json.dumps({"routes": routes(z), "counts": count_rows(z)}, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
