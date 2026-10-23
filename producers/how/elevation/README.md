# Elevation: how it's made

Source declarations (docs/inputs.md §6.8): one `<dataset>.toml` per DEM, written by hand (name,
credit, licence, heights, rank and where the files are), with a tile index beside the large ones,
made by `producers/elevation/indexes.py` in the repository from each source's own listing:

    uv run --project producers python producers/elevation/indexes.py hrdem --squares producers/elevation/hrdem-squares.geojson --out hrdem-tiles.geojson
    uv run --project producers python producers/elevation/indexes.py 3dep --out usgs-3dep-tiles.geojson
    uv run --project producers python producers/elevation/indexes.py glo30 --out glo30-tiles.geojson

One pool: the roads and the 3D terrain read the same declarations. The worldwide ones
(`glo30.toml`, `terrarium.toml`) and the 1 m datasets' are kept in the repository's
`producers/elevation/`; the others (HRDEM, 3DEP, GSI) only here. The app fetches the data itself,
but for a dataset kept in the store (GLO-30), filled once by `scenic-build store-fetch <id> --index
<its index>`.
