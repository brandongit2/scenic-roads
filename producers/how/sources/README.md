# Sources: how they're made

Source declarations (docs/inputs.md §6.9, §6.10), kept in the repository and dropped here; the app
fetches the data itself.

- `producers/sources/`: the declarations (land cover, canopy, leaf type, buildings, building
  heights, Wikidata, Wikipedia, pageviews, lake fill), and the producers of two indexes:
  `canopy.py --out canopy-squares.geojson` and `ghsl.py --out ghsl-tiles.geojson`
  (`uv run --project producers python producers/sources/…`). CanVec's lake zips are kept by hand in
  the NAS's `sources/fetched/canvec-lakes/`.
- `producers/depth/declarations/`: the depth sources (staged, dropped here as the depth rolls out:
  plan.md §6, Depth), each with its index of grids made by the depth producers in `producers/depth/`
  (`gebco.py`, `bluetopo.py`, `nonna.py`, `raster.py`, with tide stations from `stations.py`); each
  declaration's `notes` says how its grids were made. GEBCO is the owner's hand download
  (`sources/gebco/`).
