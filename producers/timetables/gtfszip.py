"""Writing a static GTFS zip as inputs/timetables/gtfs/ takes it (docs/inputs.md §6.7.2): the tables
as CSV, in a zip whose bytes depend on the tables alone (entries in a fixed order, a fixed time), so
a file converted again is the same file and the input check sees no change.
"""
from __future__ import annotations

import csv
import io
import zipfile
from pathlib import Path

# The tables every zip holds (the GTFS reference's required files), with at least these columns.
REQUIRED = {
    "agency.txt": ["agency_id", "agency_name", "agency_url", "agency_timezone"],
    "stops.txt": ["stop_id", "stop_name", "stop_lat", "stop_lon"],
    "routes.txt": ["route_id", "agency_id", "route_short_name", "route_long_name", "route_type", "route_url", "checked", "months", "osm"],
    "trips.txt": ["route_id", "service_id", "trip_id"],
    "stop_times.txt": ["trip_id", "arrival_time", "departure_time", "stop_id", "stop_sequence"],
    "calendar.txt": ["service_id", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "start_date", "end_date"],
    "feed_info.txt": ["feed_publisher_name", "feed_publisher_url", "feed_lang"],
}
COUNTS = ["route_id", "from_stop_id", "to_stop_id", "at_least", "per_day", "per_week", "per_day_low", "headway", "days", "months", "season", "overnight", "note"]


def cell(v) -> str:
    """A value as a cell: numbers as JSON writes them, booleans 1 or 0, None empty."""
    if v is None:
        return ""
    if isinstance(v, bool):
        return "1" if v else "0"
    if isinstance(v, float):
        return repr(v)
    return str(v)


def table(columns: list[str], rows: list[dict]) -> bytes:
    f = io.StringIO()
    w = csv.writer(f, lineterminator="\n")
    w.writerow(columns)
    for r in rows:
        unknown = set(r) - set(columns)
        assert not unknown, f"columns {sorted(unknown)} aren't the table's"
        w.writerow([cell(r.get(c)) for c in columns])
    return f.getvalue().encode("utf-8")


def write(path: Path, tables: dict[str, list[dict]]) -> None:
    """The zip at `path`: `REQUIRED`'s tables (empty but for their header where not given), and
    `counts.txt` when given."""
    names = list(REQUIRED) + (["counts.txt"] if "counts.txt" in tables else [])
    tmp = path.with_name(path.name + ".tmp")
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as z:
        for n in names:
            cols = COUNTS if n == "counts.txt" else REQUIRED[n]
            info = zipfile.ZipInfo(n, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            z.writestr(info, table(cols, tables.get(n, [])))
    tmp.replace(path)
