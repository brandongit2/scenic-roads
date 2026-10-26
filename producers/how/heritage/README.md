# Heritage registers: how they're made

Made by `producers/heritage/` in the repository (docs/inputs.md §6.5): `registers.py` writes each
register's `<register>.geojson` and `<register>.toml` (credit, licence, source, download date,
territory) from the designating authorities' open data, read by `eu.py` (UNESCO, Europe) and `na.py`
(North America, the special places); `federal.py` locates Canada's federal designations (the DFHD,
which has no coordinates) through Wikidata and OSM; `crhp.py` reads the Canadian Register of
Historic Places' pages; `tiers.py` gives each entry its `tier`.

    uv run --project producers python producers/heritage/federal.py --dir <data>
    uv run --project producers python producers/heritage/crhp.py --dir <data>
    uv run --project producers python producers/heritage/registers.py --data <data> --out <folder> [<register id> …]

`<data>` is the readers' download cache, laid out as the old app's registers' snapshot was: the
NAS's `sources/registers/legacy.*.tar.zst` extracted. To fetch a register again, delete its folder
there; the readers download what's missing. Add the outputs here (update the heritage inputs); the input check takes them in.

Not recorded: each register file's exact download URL beyond its description's `source` and
download date. A register whose site has changed since needs its reader updated.
