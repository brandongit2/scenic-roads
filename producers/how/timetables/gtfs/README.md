# Static timetables: how they're made

Static GTFS compiled by hand for lines no feed covers (docs/inputs.md §6.7.2), written with
`producers/timetables/` in the repository (`gtfszip.py`: zips whose bytes depend on the tables
alone, so a file made again is the same file).

- **Ferries** (`ferries-batch-*.zip`): Claude research agents (the model wasn't recorded), one
  batch of OSM ferry lines each, with `PROMPT.md`; the batches and the lines not found are in
  `research/`. Their answers (`timetables-batch-*.json`, today in the NAS's old
  `inputs/ferries/freq/`) became the zips by

      uv run --project producers python producers/timetables/ferry_lookups.py --out <folder> timetables-batch-*.json

  Lines still without a timetable are listed in `todo/` (its README is the brief for a GTFS answer);
  a researcher's zip is added here.
- **The MTR** (`mtr.zip`): researched by hand from MTR's published frequencies into `mtr.json`,
  then, with a Hong Kong stations extract from OSM,

      uv run --project producers python producers/timetables/mtr.py --mtr mtr.json --stations hk-stations.geojsonseq --out mtr.zip

  No research brief was written for it.

Each route carries its source's page and the month it was read; remaking means reading the sources
again.
