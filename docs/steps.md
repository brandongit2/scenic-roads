# The build's steps

What each step does, in detail. The steps as one table (names, chains, the checklist, the build page) are `docs/pipeline.md`.

## 1. The OSM pass

1. **The NAS fetches the planet** (`nas/fetch-planet.sh`, from `tools/nas/`): DSM runs it daily; it
   does nothing until the newest planet is six months old or `nas/fetch-now` exists, then downloads
   the newest planet with a published MD5, resumably, checks it and moves it to
   `sources/osm/<date>/`.
2. **Copy to the SSD** when there's room (docs/macs.md).
3. **Filter:** one `osmium tags-filter` into (a), the filtered planet, kept until the next pass so a
   new tag needs no download.
4. **Sets and the basemap's input (b)**, filtered from (a). Sets are versioned (`summits-v2`).
   `pass-sets` makes a finished pass's missing sets (copying (a) here when there's room for it and
   20 GB).

   A filter whose input isn't copied here reads it off the NAS over SSH, never the SMB mount
   (`osmpass::streamed`: `ssh brandontsang@fishandchips.local cat …`, one stream for each of
   osmium's two reads, through a named pipe, cipher list `aes128-gcm@openssh.com,chacha20-poly1305@openssh.com,aes128-ctr` (the first the NAS also offers; GCM measured 25 -> 59 MB/s on the Mac mini); the soft mount timed out on the hour-long reads). It
   needs SSH keys to the NAS (docs/macs.md) and fails saying so. A job under the budgets' sandbox
   reaches the NAS through its agent's proxy (`CONNECT`, unmetered). The bytes count as NAS reads;
   each read must carry the file's whole size (taken over SSH first). The first cut (`osmium
   extract`, four reads) streams (a) the same way when the local copy went. Known retry cause: a
   job frozen by memguard for over 10 minutes freezes its stream too (the feeder is in the job's
   process group), the proxy ends the idle tunnel, and the job fails; its retry redoes that
   stage.
5. **Outlines:** administrative levels 2–8 and ISO 3166 areas, assembled into polygons in a
   sectioned file, with simplified copies. Made by their own step (`outlines`,
   `osmpass::make_outlines`, keyed by `OUTLINES_V`, the pass, the set and the previous outlines);
   nothing reading outlines is planned while it has work (after three failures the rest goes on).
   - osmium silently leaves out a relation whose rings don't close. `pipeline::outlines::assemble`
     finds those: **countries and territories** are assembled by joining ways and closing gaps of up
     to 1 km (`MAX_GAP_M`) with straight lines crossing nothing, flagged `REPAIRED`; one that still
     can't be keeps the previous pass's outline, flagged `STALE`; with none, the step fails.
     **Others** are left out with a warning.
   - A country's code is its own assigned `ISO3166-1` tag, else read from its ISO 3166-2 code.
6. **Basemap:** Planetiler on (b), worldwide (`layers/basemap/world-<date>`), its jar and data pinned
   in `sources/basemap/`. The pass refuses a jar of another version than
   `pipeline::water::PLANETILER_VERSION` until `tools/coastcheck` has checked it.
7. **Cut (a) into z6 tile extracts:** a depth-first quad tree, four outputs per osmium run (osmium
   keeps ~4 GB of id sets per output). Ways whole, multipolygons completed, 10 km buffer. Each
   extract (stored in `sources/osm/<date>/pieces/`: the folder's old name, kept) is uploaded with
   its road links as soon as it's cut. (`scenic-build patch-ferries` gives extracts the ferry ways
   a pass filtered without `w/route=ferry` lacks.)
8. **Road values** (below).
9. **Finish:** the pass's summary is written; older passes retire from the manifest (but the
   previous pass's outlines, which the outlines step reads).

## 2. Global-source layers

- **Terrain** (`pipeline::terrain_pack`), two steps:
  - **terrain**, a run per z6 tile near the coverage (`build_piece`): its z9–12 within the
    coverage plus 20 km, zoom capped by latitude so pixels stay ≥ 15 m (z12 to 67°, z11 to 79°, z10
    beyond): its hi pack; and its mid (`work/terrain-mid/6-x-y`, never served: its z9 tiles' 2×2
    means as f32, and its lakes' levels). A z6 tile making no hi tile drops its hi pack; there the
    server makes tiles from the z8 ones.
  - **terrain-lo**, an assembly per z3 tile (`build_lo`): z8 → z3 made again from children (a
    z6 tile's z9 means from its mid), since a lake's level at z8 is gathered from the whole z3 tile: its
    lo pack.
  - A z6 tile's z8 subtrees, grouped by shared lakes, are tasks (`terrainsub`, docs/workers.md §3).
    The z6 tile's own run prepares each level a block at a time (a z8 subtree's tiles at z12, the
    tiles under one four levels up: `terrain_pack::blocks`), the next block prepared while this
    one's tiles are finished. A tile whose lakes all have their levels by then (known from finer
    levels, or lakes no other block's tile has, found from the water's lake polygons first) is
    finished and let go; the rest wait for the level's last block (about a tenth at z12 in 6/8/18).
    So a run holds about two blocks' prepared tiles, not the level's: the same bytes, since a lake's
    level is of its samples in any order. Measured (task #447, footprint with `MallocLargeCache=0`):
    6/8/18 5.5–6.0 GB → 2.6–3.0 GB, 6/13/18 6.5 → 2.8 GB, 6/10/21 3.2 → 1.1 GB. `scenic-build
    terrain-same 6/x/y [--whole]` makes a live z6 tile either way and sets its pack's and mid's
    content names beside the manifest's, writing nothing to the NAS.
  - Together they make the same packs, byte for byte, as the z3 tile's whole run (`build_q_with`,
    `scenic-build terrain 3/x/y`; `terrain_pack` tests).
  - **The root (z0–2)** is made from the lo packs once no terrain work is left.
  - **Sources:** declared in `inputs/elevation/` (`terrain = true`, docs/inputs.md §6.8,
    crate::terrain_sources): the base's raw tiles (AWS's Terrarium tiles, fetched once by the
    declared template, packed onto the NAS: docs/macs.md); north of 60°N, Copernicus GLO-30 by its declared
    index (kept in `sources/fetched/glo30/`); and the water of the latest pass's basemap. Each is
    pinned in the key (`TERRAIN_V`, the declarations' `keyed` digests, the digest of the water the
    target reads), so
    a rebuild with the same key gives identical packs. The digests are worked out once a basemap by
    the `terrain-water` job, so a new pass remakes only z6 tiles whose water changed.
  - **Repair** (`roadcore::grid::repair_terrain`, README "Terrain repair"): one pass taking what is
    broken or undefined and nothing else. Voids are filled; a tower or pit is a blob of the tile's
    component tree, taken whole and judged against the ground it meets (standing out over 100 m,
    steeper than terrain can be, towering over its surroundings); small spikes go when two of three
    hold (walled, on flat ground, beside a blob taken). Repairing its own output changes nothing
    (`terrain --scan`). It reads AWS's values, bathymetry included.
  - **Seam spikes and walled patches** (`seam_spikes`, `walled_patches`): towers and pits where AWS's
    sources meet are clamped into the ground around them; a region of a z10–12 tile walled by sharp
    steps that AWS's z9 tile doesn't show is moved down by its step. The whole repair runs again on
    each tile as stored (`terrain_pack::repair_stored`).
  - **North of 60°N, GLO-30** (crate::terrain_north): AWS mixes an ellipsoidal source with sea-level
    ones there (lakes 10–50 m off). Tiles of z9 and finer are resampled from GLO-30 (EGM2008, its
    water flattened) and blended in by a smoothstep from 59.5°N to 60°N. Where GLO-30 was filled
    (filling mask ≥ 3), AWS's tile is taken, shifted by the median difference. GLO-30 over FABDEM:
    FABDEM draws stair-steps and stops at 80°N. Credited.
  - **Water flattened** (crate::terrain_water): from the basemap's `water` layer at the tile's zoom,
    4 × 4 samples a pixel. The sea goes to 0; each lake, pond, reservoir or dock to its level (the
    10th percentile of its shore's dry pixels, or its source's own lower level); rivers,
    intermittent water and pools are left. One level per lake by OSM id across the tiles made
    together. Partly-water pixels are blended.
  - **Below zero** is clamped to 0 after the water. Planned: polders keeping their depth.
  - **Finer than the base: terrain-fine** (crate::terrain_fine, task #533; a z8 tile a target, any
    Mac, once its z6 tile's terrain is done under its current key): where a roads' elevation dataset
    is finer than the terrain's finest declared dataset by over a tenth (by its full resolution,
    inputs.md §6.8 `full`: HRDEM 2 m, USGS 3DEP 10 m, GSI's), the terrain goes on past its finest
    base zoom to the dataset's (pixels no finer than half its cell: 2 m to z16, 3DEP to z14, z13
    north of ~55°; a tile pyramid to its own zoom). A fine tile is the datasets read at full
    resolution and brought to EGM2008 as the roads' are (inputs.md §6.8, `heights`: so fine tiles,
    the base, the lakes' levels and the road profiles agree), blended at their edges as the roads'
    sampler does, the stored base tile (from its z6 tile's hi pack) last so the data blends into it
    over four base pixels where it ends; where only the base gives a height, the base read across its
    tiles' edges (each padded by a pixel of its neighbours, a ring of one tile read past the z8 tile,
    as MapLibre draws the base: no seam at a base tile's edge); the base's water kept by each pixel's share of the base
    tile's sea and lakes; below zero to 0; rounded to 1 cm; Terrarium RGB as lossless JPEG XL
    (libjxl, effort 7: Chrome and Safari decode it natively, every value exact; the same bytes on
    every Mac, crates/jxl's golden hashes). Only base tiles
    whose datasets have data in a 128 × 128 grid of them make any, and only tiles with some of that
    data are stored, in fine packs by z10 tile (formats.md, Pack); elsewhere the server makes finer
    tiles from their ancestors. A run drops its z8 tile's packs it no longer makes, all of them where
    no dataset is fine any more (a z8 tile with packs and no target runs for that alone). The base's
    levels (z9–12) and the terrain's keys don't depend on it, so the analysis grid (z11) and the
    roads don't change. Measured (M4, read over the network; JXL at 1 cm): a z12 tile's subtree to
    z16 (340 tiles) 10–14 MB (HRDEM: a flat field 10.3, hills 11.1, Quebec 14.0), sampling ~10 s and
    encoding ~11 s on a thread; to z14 (3DEP, 20 tiles) 0.6–1.3 MB, ~1.3 s + 0.8 s. Over the
    coverage (HRDEM 48.6 k z12 tiles, 3DEP 154 k south of 50° to z14 and 101 k in Alaska to z13):
    ~0.57 TB HRDEM, ~0.17 TB 3DEP; ~310 and ~90 process-hours.
- **Slope** (`pipeline::slope_pack`): z11 and coarser stored, from transient z12 Horn slope; the
  server makes z12 and finer on demand (to z14, or as deep as the terrain's fine tiles go: the
  catalog's terrain maxzoom). **slope**, a run per z6 tile (its z9–11 hi pack and its mid,
  `work/slope-mid/6-x-y`: its z6–8 tiles), reading the terrain packs (its own z6 tile's copied
  whole first) and its neighbours' edge strips; **slope-lo**, an assembly per z3 tile from its
  z6 tiles' mids (each read whole, eight at once, under a deadline and tried again while it fails
  transiently). Together the z3 tile's whole run's packs, byte for byte.
- **Worldwide z8 terrain** (`sources/terrain-z8-v3`, once, not served): every z8 tile repaired, with
  each tile's maximum (AWS's tiles alone, task #465). Peaks read it.
- **Grids (z11):** land cover, canopy and cover, for analysis only. Each `unit` job makes the grid tiles
  its packs lack and uploads them as its z6 tile's `grid-*` hi packs; they aren't in those jobs' keys
  (made from fixed datasets, the same whether read or made).
- **Trees** (cover, height, leaf type), zoom 4–12, clipped to the coverage (`pipeline::treepacks`,
  running the `trees` program, `pipeline::trees`):
  - **trees**, a run per z6 tile the coverage meets (`trees --z6`): its zoom 9–12 hi packs and its
    mid (`work/trees-mid/6-x-y`: zoom-8 tiles and values).
  - **trees-lo**, an assembly per z3 tile (`trees --assemble-lo`): zoom 8–4 from its z6 tiles' mids.
  - Together the same bytes as a z3 tile's whole run (`trees --z3`, `scenic-build trees 3/x/y`), from
    Meta's canopy squares (`sources/canopy/`, each downloaded once under a `<file>.lock`) and the
    leaf-type squares (`sources/trees/leaf/`, made by `dem/leaftype.py`).
  - Lossless WebP by `pipeline::webp`. `tools/check/trees-same.py` compares it with `dem/trees.py
    --z3` (same pixels); `SCENIC_TREES_PY=1` uses trees.py for a whole run.
  - `trees --block`, `--blocks` and `--assemble` run blocks alone; a row of a z6 tile's blocks is a
    task (`treeblock`, docs/workers.md §3), the same bytes natively and in WebAssembly.
- **Area overlays** (the heritage and designated areas' outlines): `docs/phase5.md`. The `overlays` job runs after marks (it needs the World
  Heritage dots' ids).
- **3D buildings:** `docs/buildings3d.md`. Every building in the coverage from the Overture release,
  tiles z12–14 per z6 tile (`bldprep`, `bldtiles`; `pipeline::bld`, dem/bldprep.py), a z6 tile's
  z8 tiles offered as tasks (`bldtile`); below zoom 14 the far buildings' field (buildings3d.md
  §4.8; `?bldfield=off` leaves it out). Planned: national heights (PLATEAU, BD TOPO).

The server builds missing deeper terrain and slope tiles from their ancestors.

## 3. Depth (per z3 and z6 tile, from the depth sources)

Depth under the seas and lakes (`pipeline::depth`; its sources docs/inputs.md §6.10; its files
docs/formats.md, Depth), split by tile like the terrain, so downloads and the mirror take it as it is.
- **Targets:** every z3 tile within 200 nautical miles (`REACH_KM`, a region's EEZ's reach) of the
  coverage that a source meets (`depth-lo`, any Mac), and every z6 tile within it that a source
  finer than z8 meets (`depth`, any Mac). Both read only the NAS (the sources' grids, the basemap,
  the lo packs) and write their packs there. The 15″ grid covers the rest of a z3 tile's water.
- **Reading the sources:** a source's grids are packed by z6 tile (docs/inputs.md §6.10, Packs:
  a tar of whole GeoTIFFs, each read in place by its byte range), so a job opens a pack once and
  reads its grids from it, not a file a grid; a pack is kept open by the job's reader like a grid
  (the least recently used let go past the open-files cap, task #438), and its open has a loose
  file's deadline and retries (below). Loose grids (an index feature without `pack`) are read as
  before. Each read over SMB waits round trips, so a source's grids meeting a block
  (hundreds of BlueTopo's) are opened 8 at once, then sampled 8 at once (merged in their order),
  and each window's blocks read 8 at once into the shared block cache (`Tiff::prefetch_window`).
  A grid's open reads its header in windows of 32 KB (`geotiff::Windowed`: its directories, their
  values and its tile arrays lie in one or two clusters, the first image's at the file's start
  and the overviews' after its data) and keeps the tile arrays, so an open is one or two reads
  (it was 24–38, one a value) and a block's read is one, its data's (it was three: the tile's
  offset, its length, then its bytes); task #492.
  A grid finer than the window is read from the coarsest overview whose cells are whole pixels of
  the window's zoom (each pixel's mean exact), else, when its origin lines up with no overview
  (most of BlueTopo's), from the coarsest at least four cells to a pixel's side, each cell counted
  to the pixel its centre is in (`grids::level_for`, `ASKEW_FINER`: the cells counted to a pixel
  are the pixel's moved by at most half a cell, an eighth of a pixel; the sources' overviews
  average their cells and the zoom files' take the nearest, producers/depth/grid.py). A grid's
  zoom and low-water files are read at the overview its grid is.
  3/1/3's z6 block: 1,359 s read in turn, 456 s so, 1.6 GB (19 Oct, the M1, release). Each window's
  prefetch is held to its share of the block cache (an eighth: the grids read at once). A file's
  open and header read run on a thread of their own, waited 120 s at most: past it the job fails
  (its own retry takes over). A transient failure (NotFound, as SMB says for a moment on a file
  that's there: two depth-lo jobs, 19 Oct; EIO, ESTALE, an interrupted or reset call) is tried
  again after 2, 5, 15 and 30 s; a file that isn't a GeoTIFF is not.
- **Timings and progress:** a job's phases: the sources' declarations read, the basemap's water
  opened and read, `composing` (its sub-phases: the source files opened, the sources' windows read,
  the stored levels scaled up, feathering, gaps filled), tiles encoded, the coarse levels from
  their children (depth-lo) and the packs written. Its progress moves within a zoom, by its blocks
  done and the grids of the block under way sampled, said every 5 s.
- **The water's reads:** a level's blocks share the water tiles their margins overlap
  (`depth::build::WaterCache`), and a z14 tile's polygons, the basemap's deepest, are read and
  decoded once for its 4 z15 and 16 z16 descendants, across the two levels, shared among them and
  kept within `POLYS_BYTES` (48 MB; past it the rows farthest up go first). A block's tiles it
  lacks (a hundred, with its margin) are read from the basemap together
  (`WaterSource::depth_polys_many`, `PmTiles::get_many`: their places sorted and those within
  64 KB of each other read in one range of at most 4 MB, 8 ranges at once; a clustered archive keeps neighbours
  together, so a block is a few reads, not a hundred round trips), then decoded and rasterised on
  the job's threads (task #492). The same polygons and water as one by one.
- **depth-lo (a z3 tile):** z6–8 composed (below), z3–5 each its children's mean.
- **depth (a z6 tile):** z9–16, a block only where a source has a grid of that zoom or finer; its
  capped pixels scaled up from this job's coarser tiles and, at z8 and coarser, from the lo packs
  of its z3 tile and its neighbours (its neighbours' fine tiles are another job's: within a block's
  32-pixel margin across the tile's edge, their lo levels stand in).
- **The water** (`depth::build`, `terrain_water`'s `depth_polys`): the basemap's z6–14 (its z14
  scaled at z15–16), the terrain's sea and lakes and also its rivers (class `river`). A pixel is
  water when they cover half of it or more, or when it's narrow water (some of it water and none of
  its eight neighbours half so: a channel under a pixel wide). A river's pixel takes only the
  sources declared `rivers` (their depths below the river's own surface: NONNA's mean water level
  from CHS's stations, which slopes down the St. Lawrence with them; BlueTopo's and SHOM's mean sea
  level, in the tidal reaches they cover); a river is gap-filled from a river's data alone, keeps no
  drying heights and never dries. A river's pixel (rivers cover more of it than the sea or lakes,
  which cover under half), a pixel water only with the rivers' share, and narrow water are land
  where no source has a value, so a river no source is drawn in changes nothing. Narrow water a
  source has values in is new depth: the sea's slivers under a pixel wide change many coastal
  z6 tiles (intended).
- **Composing a block** (8 × 8 tiles and a 32-pixel margin; `depth::compose`), on that water:
  sources sampled (coarser grids
  interpolated, finer ones averaged from the overview that lines up) and painted coarse to fine;
  past its own zoom a pixel shows the stored level of its zoom scaled up as the map's fallback makes
  it (`depth::fall`, the server's too), detail fading in over three capped pixels; a source ending in
  the water feathered over its `band_m` (a source off the others' datum fills gaps only); the shore:
  positive values kept as drying heights only from intertidal sources, water without data filled
  linearly to 0 at the shore within 600 m from data of the same water body (a lake without data of
  its own stays empty), the land a 3-pixel band; dries where above the chart's low water.
- **The floor:** the depth lies on the terrain's own water, flattened to its surface, so the terrain
  less the depth is the floor; the server serves it (`/tiles/terrain/…?floor=1`), no second terrain
  stored. Rivers aren't flattened (they slope): there the terrain is what its sources draw, and the
  floor is that less the depth. 3DEP draws rivers hydro-flattened at their surface; where AWS's
  tiles come from SRTM-derived data a river's surface is noisy and not flattened, so the floor
  there is as rough. A
  pixel of narrow water is the terrain's blend of water and shore (§2, Terrain), so its floor is
  rough. The map's toggle (the 3D terrain from it, the water's surface drawn at its level, what's
  draped on the water kept at the surface) is planned (task #466).
- **The map:** two layers, each switched on its own (Layers → Depth): the depth's colour with the
  floor's shading (nautical blues on a log scale to 300 m by default; the panel's ramp picker; linear,
  log or fitted to the view), and contours with labels (on demand). Masked by the basemap's water at
  the display's resolution, over the water fill and under the coast shading, draped on the 3D
  terrain. The cursor's readout says the depth, its class and source; the map names the sources in
  view, "not for navigation", and the notices their licences ask for (CHS's, naming the owner as the
  maker), which the credits carry too.
- **The rollout:**
  1. Both Macs run the app with the depth (until a depth declaration is accepted the chain has no
     targets).
  2. The producers make the grids on the M4 (`producers/depth/`, `uv run --project producers`) into a
     local folder, packed by z6 tile, each source's packs then copied to `sources/fetched/<id>/`
     with `tar` over SSH (the NAS's small-file limits; docs/storage.md). Grids already there loose are
     packed once by `producers/depth/repack.py`.
  3. The declarations (`producers/depth/declarations/`) and the indexes are dropped into
     `inputs/sources/`. Once accepted, the terrain-water job works out the depth targets' water
     digests (once a basemap), then the assemblies run, then the z6 tiles.

## 4. Water (per pass, worldwide)

The basemap's water drops small polygons at each zoom and its vector fills' anti-aliasing pushes
shores out, so zoomed out small islands and lakes went missing. The water layer
(`pipeline::water`) gives each pixel its exact share of water at every zoom, sea and inland apart,
drawn as a raster.
- **The water** is the basemap's own at z14 (simplified by 1/4096 of a tile, nothing over 1/256 px²
  left out). The sea is OpenMapTiles' class `ocean`, the rest inland; water in tunnels is left out.
- **Coverage is exact** (`pipeline::watercov::Raster`: each edge adds its area to the pixels it
  crosses).
- **Tiles:** 512 px, two channels (sea, inland). The bytes also say whether a pixel holds anything
  but sea, and any land at all (`water::bytes`), so a tiny island stays land zoomed out, for the
  coastal shading.
  - **Stored for z0–9:** each z10 tile from its 256 z14 tiles, coarser tiles the exact mean of their
    children; only tiles not one value throughout, as grey-and-alpha PNGs (`layers/water/`;
    docs/formats.md).
  - **Deeper zooms are drawn by the server** (`crates/server/src/water.rs`) from the z14 tiles under
    the tile or the one over it.
- **Served** at `/tiles/water/{z}/{x}/{y}`: `?c=<sea>,<lake>[,<land>]` gives a coloured PNG (with
  the land's colour, the alpha that mixes water and land as light mixes: `alpha_for`); `?raw=1` the
  shares; a tile not stored takes its stored ancestor's value; a stored zoom it can't read is drawn
  from the basemap at z9. Drawn tiles are kept (192); at most 4 drawn at once, on threads of their
  own; the open sea's z14 tile is drawn once.
- **Drawn** (`web/src/basemap.ts`) as a raster source of 256-CSS-px tiles to z18, linear, no
  cross-fade, under the Water switch. MapLibre is told the tiles are 256/√2 px (`WATER_TILE_SIZE`)
  so it never stretches a coarser level; shrunk, tiles are sampled trilinearly (the `scenic:mipmap`
  metadata, read by our MapLibre's `draw_raster.ts`). Colours are in the URL (Settings → Map →
  Water), so a new colour needs no rebuild.
  - The coastal shading (`web/src/coast.ts`) measures its shore from the same shares at 2 texels a
    CSS px: every pixel holding any land is a shore, placed by its share (`web/src/coastdist.ts`).
    The distance is exact within 192 pixels; past 60° the far field comes from coarser ancestors.
    Stored in metres in MapLibre's 'custom' raster-DEM encoding.
  - Without the layer in the catalog, the basemap's water polygons are drawn.
  - The water has no hover or click; its names are the labels'.
- **Checked** by `tools/coastcheck` (its README has the metric and views): the map in an eval mode
  against full detail over 137 views, and with `--screen` the screen in the app's colours against
  full detail rendered and shrunk in linear light. Visible difference is about 0.03 % (0.15 % on
  the densest small-water views), against 7.7 % for the earlier dots over polygons.
- **Lakes filled from Canada's own data** (`pipeline::lakefill`; the `lakefill` step once a pass,
  before the water, and the `lakefill` tool). OSM's CanVec import skipped some map sheets, whose
  small lakes vanish in rectangles.
  - **The survey** (`lakefill::run::survey`, `gaps`): every NTS 1:50,000 sheet in the Canadian
    regions, its small lakes (under 1 km²) per 100 km² of land. A sheet is a gap when that's under a
    tenth of its neighbours' (75th percentile within 3 sheets), under 2, or its size curve is cut
    off.
  - **The source:** CanVec 1:50,000 Hydro `waterbody_2`, the provinces' zips, downloaded by hand
    (the FTP's robots.txt disallows `/pub`) and kept in the declared source's folder,
    `sources/fetched/canvec-lakes/` (`lakefill::run::source_dir`, docs/inputs.md §6.9), read from
    there (`lakefill::zip`). Taken: lakes, reservoirs, ponds and unidentified water bodies; intermittent
    ones drawn as water but not flattened. Licence: OGL–Canada, credited.
  - **The rules** (`lakefill::select`): whole lakes only (pieces sharing an edge or a sheet line are
    one); none touching OSM's water within 10 m; OSM's lakes cut at a gap sheet's edge completed with
    the source's part inside the gap; duplicates once; lakes over 1° across left out; a sheet filled
    only where under half the source's lakes in it touch OSM's water (`FillOpts::canvec`).
  - `lakefill seamcheck` checks the edges where filled lakes meet OSM's water.
  - **Where it plugs in:** the step makes `work/lakefill/<date>.<h>.sect` (docs/formats.md), keyed
    by the basemap, the outlines and the source's zips (`run::source_pin`); only the fill from the
    latest basemap is drawn (`lakefill::current`). The water, the terrain's water digests and the
    catalogs wait for it (`hold_for_lakefill`) until it has failed 3 times in a row
    (`water_waits_for_lakefill`), then go without it. The water layer draws its lakes as an extra
    z14 tile (`water::Extra`, the catalog's `global.lakefill`); the terrain flattens them
    (`lakefill::FilledWater`; `terrain_pack::water_source_pin` names the fill).

## 5. Worldwide road values (in the OSM pass)

**One chaining** serves whole roads, strokes, drives, hover, profiles and rides.
- **How ways pair:** at each node, way ends of the same kind pair by mutual best continuation: same
  ref (any shared token), else same name, else same class when both are unnamed, else a nameless way
  continuing a named one of its class within 35°. The straightest go first, within 100°. Oneways
  pair only in their direction. Ties: higher level, straightest, lower way id. Rail pairs by line
  identity; ferries and ways with under two vertices never chain.
- **Per extract:** the pairing at a node depends only on the ways through it, so it's exact per z6 tile extract.
- **Roads:** pairings form paths and cycles; a union-find and an ordered walk give each way its road
  id (the road's lowest way id), the road's length, and its offset and direction.
- **Storage:** per z6 tile, `sources/osm/<date>/roads/<u>`, copied by each `unit` job into
  `global/roads/<u>` with `byroad` (ways sorted by road).
- Lengths include parts outside the coverage.
- Elevation smoothing uses a local continuation rule (`Net.cont`) in base(U).

## 6. Roads, per z6 tile: base(U)

The `unit` job ("Roads"; its step name is kept as stored) runs the programs for one z6 tile U on
a scratch folder, wiped at each run:
1. **extract:** U's ways that touch the coverage, on U's extract. Rail tracks without a route relation
   are kept by type.
2. **Elevations:** `elev` (`pipeline::dem`), the declared elevation datasets (docs/inputs.md §6.8) by
   their plan for U (`elevation.json`) through one sampler, blended at their edges, on U's slice of
   the per-vertex DEM cache (the seed, where one of its samples could be valid, and the z6 tiles' kept
   samples). U's samples are kept for later runs and its neighbours'. The vertices the cache lacks are
   sampled in batches of 2 M along a Z-order curve (`dem::generic::BATCH`, task #496), the files,
   blocks and tiles read kept across them: each vertex's height is its own, so the batches change
   nothing but the memory (some 150 bytes a vertex sampled at once: the densest z6 tiles took 16 GB).
3. **Heritage:** the heritage-sites job's slices within U + 30 km; `areaflags` rasterises the heritage and designated areas
   onto U's grid.
4. **Terrain and grids:** terrain z11 and the grids, staged from the packs the manifest names.
   Missing grid tiles are made.
5. **`tile elev`:** clean-up and grade, with junction context from the extract.
6. **scenic:** `scenic-metrics` prep, canopy, view, buildings and flags for every sample of the ways
   U owns. Buildings come from the roadside buildings' z8 tiles within 1 km of U's tile + 20 km and
   its long roads. Canopy and view results are kept in `cache/scenic-units/` (`scache::Carry`), so a
   later run redoes only new samples and those near grid tiles whose terrain or land cover changed;
   a change of `scache::SCENIC_V` redoes all.
7. **Output:** the base pack (per-vertex arrays and records, indexed by z9 sub-tile),
   `global/roads/<u>`, `global/roadids/<u>` (its roads' ids, what the road → z6 tiles index reads),
   `global/roaden/<u>` (the roads' own English), and `grid-*` hi packs. A z6 tile
   left with no ways in the coverage drops them.

Elevations are u16 decimetres from −500 m (to 6,053.5 m); readers also take older packs' i16.

Landmark candidates and peaks have their own per-z6-tile jobs (`docs/phase5.md`).

## 7. Roadside buildings: the world, once per release

The buildings job (`pipeline::buildtiles`) makes Overture's building boxes for the world from one
pinned release, before any `unit` job runs: `dem/buildings.py --world` reads the release's bbox columns,
24 files at a time, into local parts by z8 tile (each file marked done once written, so a run cut
short resumes); each tile's parts are merged onto the NAS, then the index. A z6 tile's key names the
index, so a new release rebuilds every z6 tile.

## 8. Per z6 pack: pack(T)

- **Reads:** the base packs and road values of T's owners, every z6 tile whose ways come into T (its
  owned extent from the pass's reach; without one, its tile + 20 km), grown by the base pack's own
  extent where that lies further (a pack built by other reaches; each pack's header extent kept
  in `state/build/extents.json`, which the lead keeps as it plans, reading each new pack's header
  once; jobs only read it, and read the headers of packs it lacks without writing). A job opens
  only its tiles' owners (`build::owners_of`, the rule their keys read); one given no tiles, or
  whose reaches don't list every z6 tile, opens every one. Of each it reads from the NAS only the
  byte ranges of the ways that can reach its tiles (task #396, `crate::basepart`): the ways cut
  into chunks of 1,024 in file order, each chunk's box and first sample in the pack's *way index*,
  kept on the Mac (`<cache>/base/<content name>.idx`, a few kB) once worked out from the pack's
  ways, vertices and samples (read whole that once); the chunks whose box meets a tile are read,
  their records, vertices, per-vertex values, samples and road values, neighbouring ranges in
  one read, into sparse files at the top of the job's scratch (the disk its agent counts),
  deleted as the job ends. What it makes is byte-identical to what the whole files make. Measured
  on the M1 over Wi-Fi, 6/18/22's nine owners (5.6 GB of files): copying them whole took 75–119 s;
  read in part, 2.1 GB in 49 s the first time (the indexes worked out), 0.57 GB in 21 s after
  (18 Oct, the M1 busy with other builds).
- **Writes:** road and rail hi tiles (z9–14) and T's hidata: the ways-here index; query parts (each
  road's 100 m samples and channels with offsets, for drives and rides); rail lines; the zoomed-out
  summaries (`docs/phase5.md`).
- **Lo packs** per z3 tile hold roads and rail z4–8, made from the base packs.
- Landmarks, stations, ferries and overlays come from their own jobs, so no road pack depends on
  worldwide rankings.

## 9. Worldwide steps after the z6 tiles

- **Road → z6 tiles index** (`global/roadunits`, its stored name): for whole-road hover. From the z6 tiles' road ids
  (`global/roadids/<u>`; a z6 tile built before them: its road values), keyed on them by content; it
  reads only the lists changed since the index before, which names each by content (`srcs`).
- **Labels** (per pass): places, states, seas, lakes and parks from the labels set (`dem/labels.py`,
  with own English, kana reading, OSM languages and OSM object: docs/names.md).
- **Water** (per pass): above.
- **Spoken languages** (per pass, the `spoken` job): `global/spoken` (docs/names.md).
- **Rail stops:** from the rail set, for the built z6 tiles + 20 km.
- **Ferries:** worldwide, from the ferries set, the ferry feeds (`dem/ferrygtfs.py`, docs/inputs.md
  §6.7.0) and the static timetables (docs/inputs.md §6.7.2).
- **Rail service** (below).
- **Landmarks:** `docs/phase5.md`.
- **The to-do lists:** `names-todo` after a catalog the map serves that changed what it reads
  (`namestodo::inputs_key`: the z6 tiles, coverage, languages spoken, labels' hi packs, base packs,
  own-road lists, hi data, landmark points and area overlays, by content; a catalog of depth or
  terrain alone changes none), at most daily while z6 tiles build, after any such once none's left
  (docs/names.md, `NAMES_TODO_EVERY_S`); `timetables-todo` after each rail
  and ferries run (docs/inputs.md §6.7.3).

## 10. Rail service

Trains a day on each rail way of the coverage (`global/railfreq`), from operators' GTFS timetables
and static timetables compiled by hand (`pipeline::rail`; docs/inputs.md §6.7). Two jobs, in a chain
of their own (docs/building.md §2):
- **`rail-feeds`** (`dem/railfeeds.py`) finds the feeds from the accepted declarations of
  `inputs/timetables/feeds/` (docs/inputs.md §6.7.1):
  - The Mobility Database catalogue's active, keyless GTFS feeds of the countries the coverage is in
    (`pipeline::rail::countries`: a country holding 5 % of one of the coverage's outlines, or half of
    whose area the coverage holds; a territory with an ISO 3166-2 code brings its country too) whose
    box meets it (as `pipeline::coverage` answers: an outline's edge meeting the box grown by its
    buffer as a rectangle, else the box inside it under the even–odd rule; asked of the rings through an STR tree, never of
    the outlines merged, `dem/near.py`). The declaration's `own_code` and `country` correct the catalogue's codes.
  - Of those, the ones running rail, checked once by reading `routes.txt` with range requests (only
    a whole answer counts).
  - The declared feeds first, in their `order`, where a territory matches and their stops meet the
    coverage; keyed feeds when `inputs/keys.env` holds the key.
  - Left out: a feed another replaces, and what the catalogue declaration's `exclude` names.
  - Each zip is fetched once into `sources/rail/gtfs/` and fetched again only after its
    declaration's `refresh` days (a week by default), or if it was already out of date. A request
    without an answer is tried three times; a feed still without one fails the job, and after 3 days
    is left out until rail-feeds next runs. A definite refusal leaves it out.
  - It also lists the ferry feeds (all of them: ferries are worldwide).
- **`rail`** counts the trains (`dem/railgtfs.py`: each feed's median-busy Tuesday to Thursday from
  30 days before to 90 after its zip's day, a train in two feeds counted once; the static
  timetables likewise, or their count rows as given), marks stops beyond the coverage, and
  `railfreq` matches the pairs onto the pass's rail ways touching the coverage (`extract` at 8 m).
- The chain waits for the declarations' first accepted version, and `rail` for the static
  timetables'.
- **Planned:** Japan's ODPT and Taiwan's TDX feeds once keys exist (`ODPT_KEY`, `TDX_CLIENT_ID`,
  `TDX_CLIENT_SECRET`); refetching the catalogue and timetables every ~6 months.

## 11. Hand-made inputs

What no job makes, kept on the NAS with how it was made. Scripts run in `dem/` (`uv run python
<script>`); `scenic-build` takes `--root <NAS project folder> --scratch <local dir>`.
- **Heritage registers** (`inputs/heritage/`, made by `producers/heritage/`: docs/inputs.md §6.5)
  and the owner's matches (`inputs/heritage-matches/`, docs/inputs.md §6.5.4).
- **Timetable feeds' declarations** (`inputs/timetables/feeds/`, docs/inputs.md §6.7.1), how they were found in its
  `how/`.
- **Static timetables** (`inputs/timetables/gtfs/`, docs/inputs.md §6.7.2): the ferry timetables looked up by hand
  (the brief and batches in its `how/`); the MTR's lines (`mtr.zip`, made by
  `producers/timetables/mtr.py --mtr mtr.json --stations hk-stations.geojsonseq --out mtr.zip`).
- **Translations' and descriptions' methods** (docs/names.md): `inputs/translations/how/`,
  `inputs/descriptions/how/`.
- **Every drop box's `how/`** says how its input is made, pointing to `producers/<input>/`
  (docs/inputs.md §8); its README is kept in the repository as `producers/how/<box>/README.md`, and
  `inputs/README.md` as `producers/how/inputs-README.md`.
- **Fonts** (`app/fonts/` on the NAS, `data/fonts` in a checkout): three Noto Sans styles' glyph
  ranges, `scripts/fonts.sh <folder>`.

## 12. Job keys

A job's key is its step version plus what it reads, mostly by content name. The ones that cascade:
- **terrain-water (one):** the water source's pin (the latest basemap, and the lake fill) and what
  each terrain target reads of it; it fills the digests file (`work/water-idx/<hash16 of the pin>`)
  when a target's digest is missing. The digest (`terrain_water::tiles_digest`) is over the
  polygons as the terrain reads them, so only a change the terrain could see changes it. Until a
  target's digest exists its key is unknown and its z3 tile waits.
- **terrain (a z6 tile):** the coverage within 20 km of its tile, the terrain's declarations
  (crate::terrain_sources::pin: their ids and `keyed` digests; until they're accepted its key is
  unknown and it waits), the digest of
  its water (`agent::build::water_key`), and "no hi tiles" for a z6 tile making none
  (`piece_key_made`). A "none" z6 tile is keyed on the version and the tile (`none_piece_key`).
- **terrain-fine (a z8 tile):** `terrain_fine::versions()` (its version, 1 cm, the JXL effort,
  the linked libjxl's version, `FINER`, `MAX_Z`, the presence grid, the pack zoom), the tile, each fine dataset meeting it and
  finer there by its line (`id keyed`, and its heights' conversion to EGM2008) and the zoom it
  reaches there (so a dataset's measured resolution moving moves the key), its z6 tile's hi pack by
  content and its terrain key. A z8 tile with fine packs and no dataset fine there: "none".
- **terrain-lo (an assembly):** its z6 tiles' mids by content ("-" for a z6 tile without one) and the
  digest of its z6–8 water; `TERRAIN_LO_V`. "none" likewise (`none_lo_key`).
- **slope (a z6 tile):** every terrain tile it can read, by content, from the packs' indexes
  (`agent::build::slope_piece_reads`), so a change along a neighbour's edge reruns it, one inside
  doesn't.
- **slope-lo:** its z6 tiles' mids and the z3 tile's terrain lo pack; `SLOPE_LO_V`.
- **trees (a z6 tile):** the coverage inside the tile; `TREES_V` changes with any change to the bytes
  a z6 tile writes.
- **trees-lo:** its z6 tiles' mids; `TREES_LO_V`.
- **heritage-sites:** the pass, its areas set (the protected and Indigenous areas), the registers meeting the cover (their `keyed`
  digests), the matches' accepted version, the coverage.
- **unit** (a z6 tile): its extract, the pass's road values, the coverage as its ways meet it, the versions of the
  location rules where its ways go, its heritage slices, the roadside buildings' index, the elevation
  datasets' plan, and the terrain tiles it reads by content (`agent::build::unit_terrain`: each
  tile's XXH3 from its pack's index): what it stages, every z11 tile, and at its ways' points the
  z12 tile or the finest staged above it. A z6 tile whose terrain indexes can't be read has no key and
  isn't built; the status says so.
- **pack(T):** the base packs and road values of its owners, the z6 tiles whose ways come into T.
- **lo:** the base packs and road values of the z6 tiles whose owned extent meets its z3 tile.
- **rail-feeds:** the declarations' accepted index, the catalogue's copy, the coverage, the pass's
  outlines, and which keys `inputs/keys.env` holds by name (never values). Not what it writes.
- **rail:** the feeds' list (each zip by content name and its day), the static timetables' accepted
  index, the pass's rail set and the coverage.
- **water:** its version and the pass's basemap.
- **bld-fetch** (network): its version, the release and the coverage.
- **bldprep (per z6 tile):** `BLDPREP_V`, the release, and each downloaded file's row groups and GHSL
  tile meeting it (`crate::bld::sources`).
- **depth-lo (a z3 tile):** `DEPTH_V`, `DEPTH_LO_V`, the tile, the depth sources meeting it (id and
  digest of declaration and index), the digest of its z6–8 water as the depth reads it, rivers
  included ("depth 3/x/y", worked out by the terrain-water job).
- **depth (a z6 tile):** the same versions, the tile, the sources meeting it (2 km round), the content
  names of the depth lo packs of its z3 tile and its neighbours, and the digest of its z6–12 water,
  rivers included ("depth 6/x/y").
- **bldtiles (per z6 tile):** `BUILDINGS_V`, the normalized files of the tile and its 8 neighbours,
  and the coverage's shapes over it with their countries (`Coverage::shapes_key`).

The landmark jobs, stations, ferries and overlays: `docs/phase5.md`. An output identical to before
keeps its content name, so jobs keyed on it stop there.

## 13. Served

| What | How |
|---|---|
| our layers | the pack for the tile; names attached |
| basemap | tiles from the catalog's PMTiles archives (`/tiles/base`); names attached |
| drives, rides, rail lines | parts from the hidata in view plus half the window, joined by offset; zoomed out (wider than ~1,200 km) from the 500 m summaries (`approx`) |
| whole road (hover) | the road's parts, through the road → z6 tiles index |
| details, profiles | by OSM id plus location (ways-here index → base pack) |
| landmarks, overlays, stations, ferries | by view (`docs/phase5.md`) |
| `/api/meta` | the map's meta, summed from the base packs' summaries |
| `/api/catalog` | the catalog's number, layers, zoom ranges, versions, credits, regions, NAS and build state |
| `/api/coverage` | each region's outlines, simplified |

**Browser:** the map is drawn by our own MapLibre, `web/maplibre/` (MapLibre GL JS 6.11.2's source,
changed where needed: `web/maplibre/FORK.md`), compiled with the app. English comes from the server.
The page switches to a new catalog in place (it polls `/api/catalog` every minute).
