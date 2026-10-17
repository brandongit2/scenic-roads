# Timetable feeds: how they're made

Written by hand, one declaration per feed (docs/inputs.md §6.7.1): its URL, licence, credit and
modes; the app fetches the feeds.

- **Ferry feeds** (`modes = ["ferry"]`): the 40 operators' GTFS feeds a Claude agent found on
  2026-09-28, each downloaded and its `routes.txt` checked for ferry routes; `gtfs-feeds.md` is its
  summary and `operators.txt` the OSM ferry operators it searched, by their lines.
- **Rail feeds:** national operators the Mobility Database's catalogue lacks and feeds behind a key
  (`inputs/keys.env`), chosen by hand.
- **The catalogue** (`mobility-database.toml`): the Mobility Database's feed list, with the
  corrections found by hand.

To remake: the declarations are the decisions; a feed whose URL dies is found again by hand.
