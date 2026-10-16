# Lines with no timetable

Written by the lead after each rail and ferries run (`timetables-todo`): OpenStreetMap's rail and
ferry lines in the regions that no timetable covers, highest priority first. How far down to go is
a choice. The list is made again after every run; don't edit it.

## An entry

`lines.jsonl`, one line each: `{"osm", "kind", "name", "ref", "operator", "from", "to", "km",
"priority"}`.

- `osm`: the line's OpenStreetMap object (`r123` a relation, `w123` a way): the line a route is
  counted for, and its pin (below). A line is a service's direction variants together (one
  network's `ref`, or the same two ends); `osm` is its first relation.
- `kind`: `rail` (a train, metro, tram, light rail, monorail or funicular route: on the list while
  less than half its length has trains) or `ferry` (a ferry line without a timetable's count: an
  OSM `interval` tag isn't one).
- `km`: a rail line's length in the regions, a ferry line's whole length. `priority`: its length, a tenth of it for a line whose tags say
  it carries no passengers (`passenger=no`, `service=freight`).

## The answer

A static GTFS zip in `inputs/timetables/gtfs/` (beside this folder, not in it), as docs/inputs.md
§6.7.2 describes and `../how/PROMPT.md` details for researchers:

- The reference's tables (`agency.txt`, `stops.txt`, `routes.txt`, `trips.txt`, `stop_times.txt`,
  `calendar.txt`, `feed_info.txt`); `frequencies.txt` for "every N minutes"; `counts.txt` where the
  source gives only counts ("about 10 sailings a day"): never invent a departure time.
- Calendars without years: `calendar.txt`'s dates are read as month and day, every year
  (`20000501`–`20001031` is May to October).
- **Cite:** each route's `route_url` (required): the exact page or PDF read; `checked` (the month it
  was read, `2026-10`); `route_long_name` naming the source ("CalMac summer 2026 timetable"). A
  source needing a credit line beyond its page (a licence's attribution) gets a `<name>.toml`
  description beside the zip (§5.1).
- **Pin the line:** the route's `osm` column, the entry's `osm`. A ferry route counted by
  `counts.txt` must have it; a timed route is matched by its stops (rail: onto the tracks; ferries:
  within 400 m of the line's two ends) and counts for its pinned line alone when pinned.
- Real sources only: no estimates, nothing inferred from similar lines. A line with no published
  timetable stays on the list.

The input check takes a zip in within a few minutes; a mistake holds it, with the reason on the
build page. A route that then matches nothing on the map (a pin naming no line, stops far from any
track) is listed in the report of unmatched timetable routes on the build page and in
`scenic status`.
