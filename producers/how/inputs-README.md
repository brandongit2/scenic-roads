# inputs/: the map's drop boxes

Everything the map is built from that isn't OpenStreetMap or a job's own download comes in through
a folder here. The app never writes here except each request–fulfil box's `todo/`. The full
contract is docs/inputs.md in the repository.

## The drop boxes

| Folder | Kind | What goes in |
|---|---|---|
| `regions/` | declare | `<id>.toml` per region, `<name>.geojson` shapes, `_group.toml` per folder, `pins.toml` |
| `languages/` | declare | `territories.tsv`, `about.toml` |
| `translations/` | request–fulfil | answer files `**/*.jsonl`; the app's lists in `todo/` |
| `descriptions/` | request–fulfil | answer files `**/*.jsonl`, `about.toml`; the app's lists in `todo/` |
| `heritage/` | declare | `<register>.geojson` + `<register>.toml` per register |
| `heritage-matches/` | declare | `*.jsonl`: pairs of register entries that are, or aren't, one place |
| `timetables/feeds/` | source declarations | `<feed>.toml`: GTFS feeds the app fetches |
| `timetables/gtfs/` | declare | `<name>.zip` static GTFS [+ `<name>.toml`]; the app's list of lines without a timetable in `todo/` |
| `elevation/` | source declarations | `<dataset>.toml`, its tile index `.geojson` |
| `sources/` | source declarations | `<id>.toml`, its index `.geojson` (land cover, canopy, leaf type, buildings, Wikidata, Wikipedia, lake fill, depth) |

Not inputs: `keys.env` (secrets), `hold-catalog` (a control).

## The three kinds

- **Declare:** the files are the data, read as given.
- **Request–fulfil:** after a build the app lists what it lacks in `todo/` with a brief; answers
  dropped anywhere else in the box are read like a declare input.
- **Source declarations:** a small file naming a standard format, where to fetch the data, its
  licence and credit; the app fetches the data itself.

## In every drop box

- `todo/` is the app's, `how/` the owner's: how the input is made, pointing to the producers in the
  repository's `producers/<input>/`, with agents' briefs and what was done by hand. Neither is read
  as input.
- Names starting with `.`, `@`, `#` or `_` are ignored (except regions' `_group.toml`).
- Any other file not in the box's shape is an error, never skipped silently (`regions/` ignores
  unknown files).

## The input check

Within a few minutes of a change (a file is taken once it is 10 s old) the lead's agent checks the
box. A clean box becomes its new accepted version, which the build reads; a box with an error keeps
its last accepted version, with the reasons on the build page and in `scenic status`. A warning
holds its file until you accept it there. The checked copies live under `sources/inputs/`: never
edit them.
