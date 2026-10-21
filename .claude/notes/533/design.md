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

## Making (in the terrain z6 run, after its hi pack: `terrain_fine::make`)
- One sampler for the z6 tile (crate::dem::generic, its block caches and COG headers shared):
  plan = the fine datasets meeting the tile at full resolution, then the stored base tiles as a last
  pseudo-dataset (`scenic-terrain:{z}/{x}/{y}`, served from the run's memory), so the generic
  sampler's own edge blend (4 coarser pixels) blends fine data into the base where it ends.
- Presence: a 128×128 grid per base tile read at the roads' resolution; no data → nothing made.
- Fine tile: pixels sampled; no fine pixel → not stored. NaN → base bilinear; water: each pixel by its
  share of the base tile's sea+lake (terrain_water at the base zoom) takes the base's flattened
  height; < 0 → 0; rounded to 0.1 m; PNG High compression (2.7× smaller than 1/256 m Fast).
- Heights in each dataset's own reference (no geoid conversion; the base is mixed itself).
- Packs: `layers/terrain/fine/10-x-y` (z13+ under a z10 tile), uploaded one by one (a z6 tile's would
  be 45–130 GB: too big for one hi pack or a Mac's scratch). Stale ones under the z6 tile dropped;
  a z6 tile the coverage leaves drops them with its hi pack.
- The base levels z9–12 are untouched: the z11 analysis grid and the roads don't shift; slope's
  stored levels (z≤11, reading z≤12) don't change.

## Keys / what rebuilds
- Terrain z6 key: `sources` = pin, plus `fine FINE_V` and the fine datasets' `id keyed` lines only
  where one meets the tile and is finer there (`TerrainPin::sources_of`). Every other z6 tile keeps
  its key. Rebuilds: the terrain z6 tiles meeting HRDEM / 3DEP / GSI cover (most of Canada, the US,
  Japan), each remaking its z9–12 identically plus its fine packs; slope/units don't rerun (they key
  on tile contents, which are the same). The re-accepted elevation index (version 2) changes nothing
  else.

## Serving
- Catalog: layer `fine` map (z10 keys); terrain `maxzoom` = `terrain_fine::max_zoom` (16) when fine
  packs exist, else 12. Server: z ≥ 13 reads the fine pack, else the hi pack; absent → made from its
  nearest ancestor (as before), so overzoom elsewhere. Slope on demand to the terrain's maxzoom
  (was 14). Fine packs aren't mirrored (read from the NAS) and not in downloads.
- Client: terrain `dem`, `dem-hs`, slope sources and the contours' DemSource take the catalog's
  terrain maxzoom (trees keep 12).

## Measured (M4, 8 threads, read-only NAS, network reads)
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
