# #533 Terrain at the full resolution of its finest source — design

## Rule
- A dataset is *fine* when its full resolution (`Measured::full`, else `resolution`) is under 0.9× the
  terrain's finest declared dataset's (GLO-30 30.9 m): HRDEM, USGS 3DEP, GSI 1A/5A/5B/5C/10 are;
  MRDEM and FABDEM (30 m, like the base) are not. Nothing named in code: a new declaration in
  `inputs/elevation/` is picked up by its measured facts.
- Its zoom at a latitude: the finest whose pixels are ≥ half its cell (`zoom_for`): 2 m → z16 to
  ~65°N, z15 beyond; 3DEP 10.3 m → z14 to ~55°, z13 north; a tile pyramid → its own zoom (GSI z15/z14).
  Counts only where it beats the base's latitude cap (`max_zoom_at`, unchanged for the 30 m base).
- Per area: per base tile (each place's finest stored base tile, z12 / z11 / z10), the finest zoom of
  the fine datasets whose 1° cover meets it; tiles stored only where a fine dataset has data.

## Facts
- `Measured.full` (new, optional): the full resolution of a dataset read coarser (`read_resolution`).
  The elevation check's version 1 → 2 so the gate measures it; `keyed` doesn't depend on facts, so
  no road key moves. Until it's re-accepted HRDEM counts as 8 m (z14).
- `read_resolution` keeps its role for the roads (8 m: the Fine path and road results unchanged);
  the map's terrain reads every fine dataset at full resolution.

## Making (step `terrain-fine`, a z8 tile a target: `terrain_fine::make`)
- Runs once its z6 tile's terrain is done under its current key (the forecast's chain dep on the
  terrain); any Mac; resumable (each z8 tile its own target, saved as it ends); measured by the pool
  like any step (cost key `terrain-fine 8/x/y`; honours its granted threads: rayon over base tiles).
- One sampler per z8 tile (crate::dem::generic): the fine datasets meeting it at full resolution,
  converted to EGM2008 as the roads' are (`declared::plan_converted` with the accepted geoid
  sources), then the stored base tiles (its z6 tile's hi pack, read for that z8 tile only) as a last
  pseudo-dataset (`scenic-terrain:{z}/{x}/{y}`), so the sampler's edge blend (4 coarser pixels)
  blends fine data into the base where it ends.
- Presence: a 128×128 grid per base tile read at the roads' resolution; no data → nothing made.
- Fine tile: no fine pixel → not stored. NaN → base bilinear; water: each pixel by its share of the
  base tile's sea+lake takes the base's flattened height; < 0 → 0; rounded to 1 cm (owner, 21 Oct);
  Terrarium RGB as lossless JPEG XL (crate `jxl`: libjxl 0.12 static, effort 7; CMake from mise).
- Packs: `layers/terrain/fine/10-x-y`. A run drops its z8 tile's packs it no longer makes; with no
  fine dataset there (or no hi pack) all of them; a z8 tile with packs and no target gets a "none"
  target that drops them.
- The terrain step itself is as before: hi packs, keys and z9–12 identical with or without fine
  data (a test holds the keys equal).

## Keys / what rebuilds
- terrain-fine key: `terrain_fine::versions()` (FINE_V, quantum, JXL effort, FINER, MAX_Z, PRESENCE,
  PACK_Z), the tile, each meeting fine dataset's line (`id keyed` + its geoid conversion) and the
  zoom it reaches there, the z6 tile's hi pack by content and its terrain key. HRDEM counting as 8 m
  until the elevation check re-measures it, then 2 m: its zoom moves (14 → 16) and so its keys.
- Nothing else rebuilds: the terrain's keys don't name the fine datasets.

## Serving
- Catalogue: terrain layer `fine` (z10 keys) and `fine_zoom` (`terrain_fine::zoom_of` per pack);
  `maxzoom` the finest of those. Meta: `terrainAreas` `{zoom: 10, tiles: {"x/y": z}}`.
- Server: past the base's zoom at the tile's latitude it reads the fine pack; a fine tile that
  can't be read (NAS away: fine packs aren't mirrored, not in downloads) is made from its hi-pack
  ancestor instead of a 503. Content type by bytes (`image/jxl` for the fine tiles). Slope on demand
  to the terrain's maxzoom; it decodes JXL with jxl-oxide (`roadcore::grid::decode_terrain`).
- Client: `dem`, `dem-hs` and `slope` keep maxzoom 12 and carry `scenic:zoom-by-area`
  (= terrainAreas); our MapLibre's cover caps each tile at its area's zoom (`areaMaxZoom`), so
  outside the areas the cover is today's, inside it goes on to z16; the terrain's DEM lookup
  (`getSourceTile`) follows the same cap. raster-dem tiles are decoded with createImageBitmap
  (`colorSpaceConversion: 'none'`), which reads JXL natively in Chrome and Safari (owner checked:
  exact).
- Contours read the DEM by area too (#543: our copy of maplibre-contour, web/src/contour/, its worker
  capping each contour tile's DEM at its area's zoom); a new catalogue's areas are taken in place
  (`RasterTileSource.setZoomByArea`, the contours' DEM anew).

## Measured, 1 cm JXL (M4, 21 Oct; network reads; `encode` timed on one thread)
| sample (z12 subtree) | tiles | MB | z13 / z14 / z15 / z16 KB | encode ms/tile |
|---|---|---|---|---|
| HRDEM 12/942/1391 (flat field, MB) | 340 | 10.25 | 235 / 767 / 2478 / 6524 | 32 |
| HRDEM 12/1215/1457 (hills, QC) | 340 | 11.09 | 208 / 717 / 2369 / 7531 | 35 |
| HRDEM 12/1169/1449 (QC) | 340 | 13.96 | 260 / 955 / 3221 / 9195 | 35 |
| 3DEP 12/837/1560 (Rockies) | 20 | 1.34 | 295 / 1017 | 47 |
| 3DEP 12/880/1580 (plains) | 20 | 0.57 | 155 / 402 | 36 |
- cjxl 0.12 `-d 0 -e 7` gives the same bytes ±0.1 %. For reference on the flat field: PNG High at
  1/256 m 18.4 MB, at 1 cm 16.2 MB; zune-jpegxl (pure Rust) 2.8× libjxl's: not used.
- Extrapolated: HRDEM 48.6 k × ~11.8 MB ≈ 0.57 TB (0.50–0.68); 3DEP 154 k × ~0.95 MB ≈ 0.15 TB +
  Alaska 101 k × ~0.2 MB ≈ 0.02 TB. Total ≈ 0.74 TB. Encode adds ~11.6 s per HRDEM subtree (~157
  process-hours) and ~0.8 s per 3DEP one (~34 h) to the sampling's ~160 h and ~64 h.

## Measured earlier (0.1 m PNG, before the owner's 1 cm and JXL)
- HRDEM 6/18/22 (z6, Quebec/Ontario, 100 % lidar), 14 random z12 subtrees to z16:
  1/256 m PNG Fast 31–33 MB per z12 subtree, 0.1 m PNG High 10.5–11.8 MB; per level (0.1 m High,
  mean tile): z13 62 KB, z14 51 KB, z15 39 KB, z16 27–31 KB. Time ~10–15 s per subtree (sampling ~8–11 s,
  encode ~2.5 s), mostly one core.
- 3DEP 6/13/24 (Colorado), 32 subtrees to z14: 0.77 MB (0.1 m High) / 2.05 MB (Fast) per subtree,
  1.3 s per subtree.
- Footprint (data sampled on a 0.1° / 0.25° grid at the coarse overviews, within the 627 z6 tiles near
  the coverage): HRDEM 1.91 M km² = 48.6 k z12 tiles; 3DEP 10.6 M km² = 258 k z12 tiles (154 k south of
  50°, 101 k Alaska north of 55°, z13 only there).
- Extrapolated: HRDEM to z16 ≈ 0.53 TB (0.1 m High; 1.55 TB at the old encoding), to z15 only ≈ 0.19 TB;
  3DEP ≈ 0.12 TB (CONUS z14) + 0.02 TB (Alaska z13) ≈ 0.14 TB. Today's whole terrain layer: 194 GB on
  the NAS (627 hi packs ≈ 100 MB each). NAS free: 6.4 TiB.
- Build time: HRDEM ≈ 48.6 k × ~12 s ≈ 160 process-hours; 3DEP ≈ 154 k × 1.3 s + Alaska ≈ 64 h; a dense
  HRDEM z6 tile ≈ 4096 subtrees ≈ 13 h on one core, less with the run's threads (subtrees in parallel).
  Not measured: GSI (est. ~4 MB per z12 subtree to z15, Japan ~ 10 k z12 tiles ≈ 40 GB).
