#!/usr/bin/env python3
"""The ferry timetables looked up by hand (the researchers' answers, `timetables-*.json`: a list of
records `{"line": "w23471991", "per_day" | "per_week", "per_day_low"?, "headway"?, "days"?,
"months"?, "season"?, "overnight"?, "source", "url", "checked", "note"}`) as static GTFS zips of
count rows for inputs/timetables/gtfs/ (docs/inputs.md §6.7.2), one zip per file
(`timetables-batch-01.json` → `ferries-batch-01.zip`), so the zips come in the files' order.

Per record: a route (`route_id` the line, `osm` the line, `route_type` 4, `route_long_name` its
`source`, `route_url` its `url`, `checked` its `checked`) and a count row with exactly the record's
other fields, so nothing is invented and nothing is lost (a number given as null, the source read
and giving none, written `null`). No stops, trips or calendar: the services are known by counts.

usage: ferry_lookups.py --out DIR timetables-*.json
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import gtfszip  # noqa: E402

ROUTE = {"line", "kind", "source", "url", "checked"}
COUNT = {"per_day", "per_week", "per_day_low", "headway", "days", "months", "season", "overnight", "note"}


def convert(records: list[dict]) -> dict[str, list[dict]]:
    routes, counts = [], []
    for r in records:
        unknown = set(r) - ROUTE - COUNT
        if unknown:
            sys.exit(f"{r.get('line')}: fields {sorted(unknown)} have no place in the zip")
        if r.get("kind", "timetable") != "timetable":
            sys.exit(f"{r['line']}: kind {r['kind']!r}")
        if not re.fullmatch(r"[wr][0-9]+", r["line"]):
            sys.exit(f"{r['line']}: not an OSM way or relation")
        routes.append({"route_id": r["line"], "agency_id": "looked-up", "route_long_name": r["source"], "route_type": 4, "route_url": r["url"], "checked": r["checked"], "osm": r["line"]})
        counts.append({"route_id": r["line"], **{k: "null" if r[k] is None else r[k] for k in COUNT if k in r}})
    return {
        "agency.txt": [{"agency_id": "looked-up", "agency_name": "The operators, as each route's page names them", "agency_url": "", "agency_timezone": ""}],
        "routes.txt": routes,
        "counts.txt": counts,
        "feed_info.txt": [{"feed_publisher_name": "Looked up by hand from each route's page (route_url)", "feed_publisher_url": "", "feed_lang": "en"}],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", required=True)
    ap.add_argument("files", nargs="+")
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    n = 0
    for f in sorted(a.files, key=lambda p: Path(p).name):
        stem = Path(f).stem
        if not stem.startswith("timetables-"):
            sys.exit(f"{f}: not a timetables-*.json file")
        records = json.loads(Path(f).read_text())
        gtfszip.write(out / f"ferries-{stem.removeprefix('timetables-')}.zip", convert(records))
        n += len(records)
    print(f"{n} records in {len(a.files)} zips", file=sys.stderr)


if __name__ == "__main__":
    main()
