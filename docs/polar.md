# The polar caps

**Status (task #601):** built and tested in scratch roots; not yet in the live build. The steps,
packs, catalogue entries, server routes, region downloads and the app's layer exist. Nothing of it is
registered with the live pool or published until the worldwide rebuild ends, and REMA's declaration
is staged in `producers/elevation/` but not in the NAS's inputs folder (§9). Roads, labels and landmarks
past 85° are task #668.

The Web Mercator tiles end at 85.05° N/S. Past them the map draws a cap per pole from its own
tiles, on the pole's polar stereographic grid.

## 1. Projection and tiling

- **Projection:** polar stereographic on WGS 84, variant B. The north uses EPSG:3413 (standard
  parallel 70° N, central meridian 45° W), the south EPSG:3031 (71° S, 0°). There is no false
  origin: the pole is at (0, 0). `roadcore::polar::forward`/`inverse` use Snyder's formulas
  (checked against the EPSG guidance note 7-2 example to the centimetre); `web/src/polar.ts` has
  the same pair. The elevation pool reads datasets in these CRSs through the same code
  (`dem::proj::Projection::Polar`).
- **Tiles:** a quadtree over the square ±2^20 m around each pole (`HALF`). Tiles are 256 px, and
  (z, x, y) counts columns from −x and rows from +y, like the Mercator tiles. z0 is 8,192 m a
  pixel; z9 is 16 m (`pixel_m`).
- **Finest zoom: z9** (the owner's, 25 Oct). That is 16 m on the grid, about 16.5 m on the ground
  (the scale is 0.97 at the poles): GLO-30's 30 m cell's half, the Mercator terrain's rule.
- **Cap:** the tiles meeting the disc out to 85.0° (`CAP_LAT`, `cap_tiles`): a ring of about 5 km
  past the seam, for Horn's border. A parent meets the disc whenever a child does, so each zoom's
  tiles are the parents of the next zoom's. The south cap has 76 z4 tiles and 55,856 z9 tiles.
- **Blocks:** a z4 tile of the grid (131 km a side) is a block (`polar::BLOCK_Z`): a step's target
  and a pack's root. A block's latitudes (`roadcore::polar::lat_box`, every longitude from its
  nearest point to the pole to its farthest corner) are what it is asked about: the coverage, the
  pool's datasets.

## 2. Data

- **Terrain:** from the elevation pool, like the Mercator base (`terrain_base`): the same datasets
  in the same order, blended alike, each pixel the mean of 2 × 2 samples, GeoTIFFs read at the
  overview nearest half a pixel. The tile pyramids are left out: a Web Mercator pyramid has nothing
  past 85.05°. Below zero is clamped to 0. Stored as Terrarium in lossless WebP (`terrain_codec`),
  rounded to 1 cm at z9 and 10 cm coarser. No water mask past 85° (the Arctic is sea level, drawn
  in the water colour) and no lake or ice-shelf flattening.
- **The datasets past the caps' edge:** GLO-30 everywhere south of 85° S (nothing north of 85° N,
  which is all sea), then REMA near the South Pole (below).
- **GLO-30's voids.** Its fill mask (FLM 3 and up: pixels filled from another DEM) makes a void
  that defers to the next dataset. Measured at z4 over the south cap (`scenic-polar`'s voids by
  latitude band, 25 Oct): 0.2 % of samples in 89–90° S, 2.1 % in 88–89°, 0.4 % in 87–88°, 0.7 %
  in 86–87°, 0.9 % in 85–86°. With nothing beneath they came out at sea level: pits about 2,800 m
  deep, drawn in the water colour.
- **REMA** (the owner's approval, 25 Oct; docs/inputs.md §6.8): PGC's 32 m mosaic, ranked after
  GLO-30 and the Terrarium tiles, in the pool every step reads. Two limits, both measured:
  - its index holds only the tiles wholly south of 87° S (20 tiles, 0.24 GB), so its coverage (1°
    cells and their neighbours, to 86° S) meets no Web Mercator tile and no existing key changes;
  - **the mosaic has no tiles within about 150 km of the pole** (its strips stop short of it), so
    it fills nothing in the voids nearest the pole: in the pole's four blocks it gave data at 0 of
    the void samples.
- **The last resort:** where no dataset has data but a fill mask's void, the cap takes that
  dataset's own filled value (`dem::generic::Sampler::fill_voids`, the polar sampler's alone; the
  Mercator terrain and the roads read the pool as they did). This is what fills the pole: GLO-30's
  values from its own fill. The steps log each block's samples with no data and how many of them
  were filled so.
- **Gaps between GLO-30's files:** some of its one-degree files leave a line a sample wide with no
  data along an edge. Near the pole, where the meridians converge, these drew as dashed pits along
  every meridian. A pixel with no sample with data in a gap up to 3 pixels wide takes its
  neighbours' mean (`polar::close_gaps`, `GAP_PX`); wider voids stay sea level.
- **File edges in the overviews:** the levels coarser than z9 read GLO-30's overviews, which hold
  0 along their files' edges where the full resolution has none; a pixel's mean leaves out a
  sample at exactly 0 where another has data (`polar::pixel_mean`), else these drew as pits of a
  quarter or half the height along the one-degree lines (measured in the root's z3 past the built
  blocks, 26 Oct).
- **Shading:** one precomputed pyramid, as `slope_pack` makes it: at z9 Horn's method on the grid
  (a pixel's ground size the grid's ÷ the scale at its latitude), each coarser pixel the plain mean
  of its four children, or Horn's on its own level's terrain where there are none (past the cap).
  Stored as `shade-webp`. Its directions are the grid's (east = +x, south = −y); a renderer turns
  them to the ground's by longitude (`roadcore::polar::east_north`). Nothing is computed from heights
  at render time.
- **Not here:** depth (IBCAO and IBCSO for the Arctic floor); roads, labels and landmarks past 85°
  (§10).

## 3. Steps and packs

New steps (docs/steps.md §2, keys §12): no existing step's output or version changes.

| Step | Target | Makes |
|---|---|---|
| `polar-terrain` | a block ("s/4/x/y") whose latitudes the coverage meets | its terrain, z4–9: `layers/polar-terrain-<pole>/polar/4-x-y` |
| `polar-slope` | the same block, once its and its neighbours' terrain is built | its shading, z4–9: `layers/polar-slope-<pole>/polar/4-x-y` |
| `polar-root` | a pole, once none of its blocks is stale | z0–3 of both layers (`layers/<layer>/root/0-0-0`) from the blocks' z4 tiles; drops packs of blocks no longer built |

- **Poles built:** the south (`polar::POLES`). The north cap has no land; it waits for the depth.
- **Coverage:** a block is built when the coverage meets its latitudes. The Antarctica region
  reaches 90° S, so it builds all 76 south blocks; a region at the pole builds the four there.
- **Keys:** a block's terrain names the pool's datasets meeting its latitudes by their lines, as
  the Mercator terrain's keys do; its shading the terrain packs it reads; the root everything it
  reads (`agent::build::polar_targets`). The agent plans them as a chain beside the depth's
  (`polar_work`), and a pack in a hand-off not yet merged holds what reads it.
- **Who runs them:** `polar-terrain` and `polar-slope` are offered to the pool's members like the
  other per-tile steps; `polar-root` is the lead's.
- **Packs and catalogue:** RDPACK, scope `polar`, root "4/x/y" of the grid; the catalogue's layers
  `polar-terrain-s` and `polar-slope-s` name the blocks under `polar` (docs/formats.md, Pack), the
  root holds z0–3, zooms 0–9.
- **Served:** `/tiles/polar-terrain/<pole>/{z}/{x}/{y}` and `/tiles/polar-slope/…` (`image/webp`,
  `?v=` the pack's version from meta's `packs`): as stored, else 404. Nothing is made from an
  ancestor on the server: the map draws a missing tile's loaded ancestor.
- **Offline:** the roots are in the World download with every layer's root; a region downloads
  the blocks meeting it (`areafiles::polar_blocks_meeting`: its rings' points in the pole's
  hemisphere, projected, boxed and grown 2 km).
- **Progress:** `polar-terrain` and `polar-slope` say how far they are within a block, a line per 16
  tiles (`polar::terrain_block`/`slope_block`'s `progress`; docs/steps.md §2), not once per block.
- **`scenic-polar`** runs the same code (`terrain_block`, `slope_block`, `root`) into a folder of
  tiles, for measuring and for `web/test/polar.html`; it writes nothing else.

## 4. Renderer (the globe)

MapLibre's globe works in Mercator tile coordinates and the flat map ends at 85.05°, so a cap is a
custom layer on the globe.

- **`web/src/polar.ts`** (no globe in it): the projection pair, tile boxes, the tile choice
  (`chooseTiles`: a quadtree from z0, split while a tile's pixels look larger than about 1.5 screen
  pixels, given a function from lon/lat to the view's clip coordinates and one that culls a tile),
  and `PolarTiles`, which fetches and decodes the Terrarium and shading tiles to bytes, keeps the
  loaded ones, answers a wanted tile or its loaded ancestor, and reads a height at a lon/lat
  (bilinear, from the finest loaded tile).
- **`web/src/polarglobe.ts`**: the globe's layer (`polarGlobeLayer`, renderingMode `3d`). Each
  tile is a 32 × 32 mesh (with a 400 m skirt) of unit-sphere points made in float64, lifted by its
  terrain texture times the terrain's exaggeration (flat with 3D terrain off). It shades from the
  shading texture with the map's hillshade layer's own method and paint, read each frame (every
  method, the accent, the anchor), and draws the elevation and slope tints from those layers' own
  ramps, over the land or water colour. Tiles are culled by the frustum and the horizon.
- **`web/src/polarcaps.ts`**: adds a pole's cap while the catalogue lists both its layers, its URLs
  at the packs' versions (`api.ts` `packUrl`, `packs.ts` `packToken` read `polar`); a tile no pack
  holds (a block not built) isn't asked for (`PolarTilesOptions.stored`), its ancestor drawn.
- **The fork** (web/maplibre/FORK.md):
  - `Map#setPoleCaps({north, south})` turns off upstream's pole fans on the globe: the terrain
    mesh's, the rasters', and the vector fills' and fill extrusions' pole triangles (the
    subdivision setting carries it to the workers);
  - the globe's camera centre may go to ±89.5° at a pole whose cap is off (`clampGlobeLatitude`);
  - `Map#setPoleGround(fn)`: past 85.05° the terrain's elevation lookups (the camera's centre
    height, the inside-the-ground check, the far plane's minimum, `queryTerrainElevation`) read
    `fn(lng, lat)` × the exaggeration; the cap registers its loaded heights. So a camera over the
    2,800 m plateau stays above the ice and the station can be seen up close.
- **Globe above 80° at every zoom:** the app hands the globe over to flat Mercator at z15.5–16.5;
  between 78° and 80° that hand-over moves smoothly out of reach, so above 80° the globe stays
  (`globeProjection`).
- **Float32:** sphere points are good to about 0.4 m, the globe's matrix adds 1–2 m: fine at 16 m
  pixels.

## 5. The seam at 85.05°

- The cap reaches 85.0°. Its fragments nearer the equator than 85.051129° are drawn a little deeper
  (`RING_DEPTH`), so the Mercator terrain wins wherever it is drawn; the ring fills the cracks where
  it isn't.
- Both sides' heights come from GLO-30 by the same sampler, so they agree to within their pixels'
  interpolation where the Mercator terrain is built (z10–11 there). Where the coverage hasn't built
  the Mercator terrain (Antarctica isn't built today), its tiles past z2 are made from the root's
  coarse ones: the seam then steps between coarse and fine terrain.
- **Known differences:** the Mercator side has the basemap's layers (water mask, landcover,
  labels) and the cap doesn't; a slope across the seam can step by a few percent (each side's
  shading on its own grid).

## 6. For a future flat projection

The owner plans other projections, flat ones among them that can show the poles. Nothing in the
data or the steps is the globe's:

- **The tiles are the grid's,** in EPSG:3413 and 3031, with their encodings and pyramid as the
  Mercator ones. A flat polar map (a polar stereographic view) draws them as they are, as images,
  no reprojection; another projection reprojects them per pixel or per mesh vertex with
  `roadcore::polar::inverse` / `polar.ts` `inverse`. The shading's directions are on the grid and
  turned to the ground's by `east_north`, whatever draws them.
- **The root reaches past the cap:** z0–3 cover the square out to about 80.6° (sampled past the
  blocks), so a flat view of a whole pole has data to its corners. Past 85° the blocks are the
  detail; between 80.6° and 85.05° such a view would draw the Mercator tiles' data reprojected, or
  a polar layer could be built further out (a wider `CAP_LAT`, more blocks: the steps take any).
- **The renderer's pieces:** `polar.ts` takes the view as two functions (lon/lat to clip
  coordinates, and a cull test), so a flat projection's camera plugs into the same tile choice,
  loading, decoding and height lookups; only the drawing (`polarglobe.ts`) is the globe's.
- **What a flat projection would add:** its own drawing layer (or a MapLibre projection for the
  grid), its camera, and the Mercator data near the edge drawn in it; the polar steps, packs,
  catalogue and routes stay as they are.

## 7. Sizes and costs

**Estimates,** scaled from what was built in a scratch root on the M4 (26 Oct, two jobs at a time
beside the live pool's): 25 terrain blocks (the 16 round the pole, 8 more and 8/12 at the seam), 5
shading blocks (the pole's four and 8/12) and the root. The whole cap wasn't built: the pool builds
it after the rebuild.

| What | Measured | Whole south cap (estimate) |
|---|---|---|
| Terrain, a full block (1,365 tiles, z4–9) | 84.6 MB (≈ 62 KB a tile) | ≈ 4.6 GB over ~74,700 tiles |
| Shading, a full block | 118.9 MB (≈ 87 KB a tile) | ≈ 6.5 GB |
| Root, z0–3 (41 tiles a layer) | 2.9 MB terrain, 3.6 MB shading | 6.5 MB |
| REMA's store | 20 tiles, 0.24 GB | the same |

So about **11 GB** for the south cap, under the 12.6 GB first estimated (§1's tile counts, the edge
blocks only partly in the cap). Shading is the larger: GLO-30 is a surface model, and its speckle
over the ice fills the direction bits.

**Times** (a block's job, M4, nice'd, beside the live pool): terrain 245–280 s for a full block away
from the pole, 600–1,230 s for the pole's four (GLO-30's files are densest in longitude there, and
the voids are filled in a second pass); shading 75–125 s; the root 77 min, but most of that sampled
the pool where the 51 unbuilt blocks were missing; with every block built it reads their z4 tiles
instead. So the cap is about 76 × ~6 min ≈ 7–8 h of one job, half that at two at a time.
Memory: 0.3–0.9 GB a terrain job as measured (its `mem_mb`, 4,096, is the first guess).

## 8. What remains

1. Registering the steps with the live pool and publishing, after the rebuild.
2. REMA into the inputs folder and the store on the NAS (the owner's word first: §9), then the input
   check accepts it.
3. Labels and landmarks past 85° (§10). Depth for the Arctic floor (the north cap).

## 9. Decisions

1. **REMA's reach (open; recommended: keep 87° S until after the rebuild).** Its index holds the
   tiles wholly south of 87° S, so no Web Mercator key changes. Reaching to 85° S would fill
   GLO-30's voids between 85° and 87° (0.7–0.9 % of samples) from REMA instead of from GLO-30's own
   fill, but re-keys the bottom row of Antarctic Mercator tiles once Antarctica is built.
2. **The voids' last resort (taken, the agent's call; for the owner to confirm):** GLO-30's own
   filled values where nothing lies beneath, the caps only. REMA has no data within ~150 km of the
   pole, so without it the pole keeps its 2,800 m pits. Measured: 0.405 % of the pole's z4 pixels
   were pits before, 0 after.
3. **REMA on the NAS (approved 26 Oct; not yet done).** Its store and inputs folder need writing
   (`scenic-build store-fetch rema` against the NAS, then the declaration and index into
   `inputs/elevation/`). **What accepting it re-keys: the polar targets alone.** The keys name the
   pool's datasets meeting each target's box (`declared::meeting`, unexpanded tile boxes); REMA's
   cells reach 86° S and the Mercator tiles' last row ends at 85.05° S, so no Mercator terrain,
   slope, deep, lo, root or road (`unit`) key changes, nor peaks and summits (they read the
   terrain packs' z8). The fine datasets and the roads' spacing don't change: REMA's 32 m isn't
   finer than the base (GLO-30's 30.9 m, `terrain_deep::base_resolution`). Tested by
   `polar::tests::rema_south_of_87_meets_no_mercator_tile`. Two side effects, neither a re-key: a
   terrain run under way when the index switches restarts its resume state
   (`terrain_resume::inputs`), and the agent works its terrain targets out again once. Keep it so:
   REMA's 2 m or 10 m mosaics (rank above 0, finer than the base) would lower the base resolution
   and re-key every terrain target.
4. **Members take the polar blocks (open; recommended later):** `polar-terrain` and `polar-slope` are
   offered to the pool's members like the other per-tile steps, but aren't in the shared set (the
   forecast's and the hand-off checks' per-target accounting). Add them if the cap's hours matter.
5. **The north cap (recommended: wait for the depth):** it is all sea level.
6. **Zoom near the pole (open):** the globe scales its zoom by latitude, so near the pole a small
   zoom number already puts the camera close (zoom 8 at 89.5° S is at the ground; zoom 4.6 is
   1.8 km above it). The app's zoom-dependent choices (the globe's hand-off, tile levels) read odd
   there; whether to cap that compensation near the poles is for the owner.

## 10. Roads, labels and landmarks past 85° (not built)

Measured on the 2026-09-28 pass (task #668, 26 Oct): the filtered planet clipped to 85°–90° N/S.

- **Roads:** OSM has **no `highway` way** past 85° S or 85° N. The South Pole Traverse isn't mapped
  as a road there (one route marker node, "657 km to South Pole"). Past 85° S: one `aeroway`
  (the station's skiway, a node). So there is nothing for a polar road layer to draw yet.
- **Labels** (the `labels` and `places` sets, which aren't cut by tile): 8 place nodes past 85° S
  (the South Pole, Amundsen–Scott station, six localities and plateaus) and 1 past 85° N (the North
  Pole). `labels.rs` clamps a label's latitude to ±85.05° when it places it in its Mercator tile,
  so these are drawn on the map's edge at their longitude, not where they are.
- **Landmarks:** about 185 peaks, the ceremonial pole, the station, the skiway, a few historic
  sites (Polheim, a cairn, a memorial) past 85° S; a drifting ice camp's node past 85° N. The
  landmark candidates and peaks come from the z6 tiles' extracts, whose top and bottom rows reach
  only the 10 km buffer past 85.05° (`osmpass::BUFFER_KM`, about 85.14°): everything further is
  dropped there. The `summits` set (worldwide, uncut) holds the peaks.
- **What a cap layer needs:** points only (no lines while OSM has no roads there): a polar step
  reading the uncut sets (`labels`, `summits`, `named`) for the points past 85.05°, a small file
  per pole keyed by the sets, and the cap drawing text and dots in its custom layer (MapLibre's
  symbols and the dot layer work in Mercator tile coordinates and can't place a point past
  85.05°). Moving the edge's labels there means `labels.rs` leaving out what lies past 85.05°: a
  change to the worldwide labels' output.
