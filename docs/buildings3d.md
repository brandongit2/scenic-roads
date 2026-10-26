# 3D buildings

Every building in the coverage, extruded to its height on the 3D terrain, from the pinned Overture
release the roadside buildings also read. Missing heights are estimated from floors and neighbours,
then from GHSL's 100 m heights in high-rise cores, then from the footprint's size and kind. Built per
z6 tile in a chain of its own (shared with helper Macs and pages), drawn near by MapLibre's
fill-extrusion (meshes, z14 tiles) and far by a ray-marched density field (§4.8).

Code: `pipeline::bld` (fetch, overture, ghsl, prep, fill, tiles, field, job, task),
`agent::build` (`bld_targets`, `bld_work`), `web/src/buildings.ts`, `web/src/bldfield.ts`.
Companions: `docs/steps.md` (pipeline, keys), `docs/building.md` (order), `docs/formats.md` (bytes), `docs/workers.md` and
`docs/pool.md` (sharing the work).

## 1. What the map shows

- **A building** is a footprint polygon (with holes) extruded from the ground to its height.
  Building parts (OSM's `building:part`) are drawn between their own base and top, and an outline
  with parts (`has_parts`) isn't extruded (Simple 3D Buildings); an outline none of whose parts are
  in the files is drawn as a building. Flat roofs only. Underground buildings are left out.
- **Where:** every building touching the coverage (any vertex inside it, with its 1 km buffer, as a
  way: docs/regions.md). Where the coverage ends, buildings end.
- **Zooms:** meshes from z14 tiles alone, wherever MapLibre's cover takes z14 tiles (from map zoom
  14, and nearer the camera of a tilted view below it); above z14 the z14 tiles are drawn whole up
  to zoom 19.5 (§4.1). Elsewhere the field (§4.8), on by default where the catalogue has the layer
  `buildings-field` (`?bldfield=off` leaves it out; without it, no buildings below zoom 14).
- **On the terrain:** the roof is level at the centroid's ground plus the height; each wall's foot
  stands 2 m below the terrain under its own corner (§4.3). Heights are true by default; a setting
  scales them 1–3× or follows the terrain's exaggeration. The extrusions and the field take the
  map's distance fog (our MapLibre fork's, FORK.md "Distance fog").
- **With roads and labels:** buildings are drawn after the road and rail layers (§4.2): a road
  behind a building is hidden (faint through it at opacity under 1); a road in front stays in front
  (it lies on the terrain); bridges and elevated rail are drawn after the buildings. Landmark dots
  draw without a depth test, after the buildings; labels are symbols, never hidden by extrusions.
- **Look:** a muted blue-grey lit from the hill-shading's light direction, walls darker toward the
  ground. Colour modes (§4.4): plain, by height, by where the height comes from; buildings holding a
  heritage site tinted in its group's colour while Heritage sites show.
- **Settings → Buildings** (on by default): 3D or flat footprints, colour mode, plain colour (with
  a reset), heritage tint, opacity, height scale. Link fields `bd=` (its seventh field the plain
  colour) and `bh=`; **B** toggles the layer. All hidden until the
  catalogue has the layer.
- **Hover:** the bottom bar gives height, where it comes from ("measured", "from 6 floors",
  "estimated by Microsoft", "estimated from neighbours", "estimated, GHSL", "estimated from its
  size"), base and kind; the hovered building turns amber (§4.5). No click action.

## 2. Data

### 2.1 Sources

| Source | Use |
|---|---|
| **Overture Maps buildings**, release 2026-09-23.1 (the buildings source's declared `version`, docs/inputs.md §6.9), ODbL 1.0 | Every footprint and attribute: `height`, `num_floors`, `min_height`, `min_floor`, class, subtype, roof shape; building parts. Conflates OSM, Microsoft ML Buildings (heights estimated), USGS lidar, Esri, IGN España, Google and others |
| **GHSL GHS-BUILT-H R2023A, ANBH** (EC JRC), CC BY 4.0 | Average building height per 3″ cell: the fill in high-rise cores (§2.3) |
| France's IGN BD TOPO, Japan's PLATEAU | Planned (§5): measured heights where they replace estimates |

The same release serves the roadside-buildings factor and the 3D buildings, so they agree; a new
release re-keys every z6 tile's roads and every buildings tile together. In the coverage, ~339 M buildings and
2.8 M parts; 48 % have a height or floors, 37 % of all heights being Microsoft's estimates (which
grow ~1.2 m a floor and rarely reach 20 m, so they rank after floors).

### 2.3 Filling the missing heights

Each building's height is the first of these it has; it records which (`s`, 0–5) for the hover and
the source colouring (`pipeline::bld::fill`). The order, fits and defaults were set by a held-out
tenth of the measured heights (measured when the tables were fitted; median error of the chain 0.7 m, p90 2.9 m;
tall unmeasured buildings still come out low).

0. **Measured:** Overture's `height` from anything but Microsoft (USGS lidar, OSM, Esri, a city),
   2–700 m. A part's base: `min_height`.
1. **Floors:** `num_floors` × the country's storey height + a roof allowance (least absolute
   deviations): US 3.10 m + 0.90, Japan 2.82 + 1.87, UK 3.21 + 1.59, France 2.95 + 1.60, Spain
   3.00 + 0, Canada 3.47 + 1.59, Portugal 3.00 + 1.00, Ireland 2.50 + 2.50, Puerto Rico 2.92 + 0.37,
   Taiwan 3.32 − 0.96, Singapore 3.62 + 1.54, Hong Kong 3.32 − 1.32; elsewhere 3.03 + 1.27. A part's
   base from `min_floor` likewise.
2. **Microsoft's estimate:** Overture's `height` from Microsoft ML Buildings, 2–700 m.
3. **Neighbours:** the median height (by rules 0–2) of buildings within 150 m whose footprint is
   half to twice its own, when at least 5; else, for a footprint of 60 m² or more, of any footprint
   within 300 m, when at least 8. (Smaller footprints skip the 300 m stage: it made kiosks as tall
   as their area's towers.)
4. **GHSL, in high-rise cores:** the ANBH of the cell holding the centroid, when 20 m or more; a
   footprint under 60 m² takes at most 4 m. (Only in such cells is GHSL better than rule 5.)
5. **Size and kind:** the measured median of its class, in its country where 200 of the class are
   measured, else the coverage's: churches and cathedrals 8.8 m; sheds, garages, carports, huts or
   under 30 m² 3.3 m; houses and residential kinds or under 250 m² 6.0 m; 250–2,000 m² 6.4 m;
   larger 9.1 m.

Roughly: 7 % measured, 5 % floors, 36 % Microsoft's, 27 % neighbours, 0.2 % GHSL, 25 % size. The
fill's version is in `BUILDINGS_V` (§3.2), so changing it rebuilds every buildings tile and nothing
else. Building heights don't enter the roadside factor, the horizons or the viewshed (planned, §5).

### 2.5 Sizes

| | |
|---|---|
| downloads (§2.6) | 61.8 GB: Overture 60.5 GB (103 files), GHSL 1.28 GB (91 tiles) |
| z6 tiles | 380 meet the coverage; the densest 6/56/25 (Tokyo–Osaka, 30 M buildings) |
| normalized files (`work/bld/`) | 38–50 B a building, ~17–22 GB in all |
| mesh packs | 13–20 B a building, ~5–7 GB; z14 tiles mostly under 300 KB, at most ~490 KB (old towns on Spain's cadastral parcels) |
| field hi packs | 45–90 B a building (level 14 ~58 % of it), ~15–31 GB; lo packs a few MB a z3 tile |

### 2.6 Downloads

`bld-fetch` (`pipeline::bld::fetch`, §3.1) fetches whole files onto the NAS:
- Overture's files with a row group within 20 km of the coverage (buildings and parts), into
  `sources/overture/<release>/theme=buildings/type=<type>/`, with `buildings.json` and
  `footers.json.gz` (every row group's box, from the footers);
- GHSL's 10° tiles meeting it, into `sources/ghsl/R2023A/`, with `index.json`.

The coverage is the regions' accepted outlines, so a new region fetches only new files. Each file is written to a temporary name, resumed by range requests (S3 only while its
ETag holds), checked (Overture's multipart ETag, GHSL's zip CRCs) and renamed; files in place at
their listed size are skipped. Three transfers at once, no speed cap. S3 keeps a release ~60 days (this one
until 2026-11-25); after that a region whose files aren't on the NAS waits for the next pinned
release (§5).

## 3. Pipeline

### 3.1 Steps

| Step | Target | Reads | Writes |
|---|---|---|---|
| `bld-fetch` | the release | S3, JRC, the coverage | `sources/overture/<release>/`, `sources/ghsl/R2023A/` |
| `bldprep` | z6 tile T | Overture row groups and GHSL tiles meeting T | `work/bld/6-x-y` |
| `bldtiles` | z6 tile T | `work/bld/` of T and its 8 neighbours, the coverage, T's heritage slices | `layers/buildings/hi/6-x-y`, `layers/buildings-field/hi/6-x-y`, `work/bldfield-mid/6-x-y` |
| `bldfield-lo` | z3 tile | its z6 tiles' mids | `layers/buildings-field/lo/3-x-y` |

(`buildings` is the roadside buildings' step, `pipeline::buildtiles`; hence `bldtiles`.)

- **`bld-fetch`** (`pipeline::bld::fetch`), with the coverage's outlines. A network job; it skips
  what's there. Once the release has left S3 it checks against the footers: a file
  the coverage needs and lacks fails the job.
- **`bldprep T`**, per z6 tile within 1 km of the coverage with a downloaded row group or GHSL tile
  meeting it (`pipeline::bld::prep`): the row groups meeting T are read (`bld::overture`, parts: T
  grown by 0.02°, so an outline finds its parts) with the GHSL windows (`bld::ghsl`), as columns
  (geometry as WKB); centroids (area-weighted) and areas computed in f64, rounded to E7, what's
  centred in T kept, GHSL sampled, the normalized file written. Checks the columns' names and
  types, failing on a schema change.
- **`bldtiles T`**, per z6 tile meeting the coverage, a z8 tile at a time (`pipeline::bld::job`):
  the buildings of T touching the coverage, heights filled (reading the buildings within 620 m
  around the z8 tile), the z14 tiles encoded (`bld::tiles`), and the field's levels 14–9 and each
  z8 tile's level-8 planes (`bld::field`). A building within 310 m beyond the z8 tile that reaches into
  its tiles is filled there too, for its copies (§3.4). Pure. A tile with no building loses its
  packs.
- **`bldfield-lo`**, per z3 tile once all its `bldtiles` targets are done: field levels 8–4 from the
  mids, as `terrain-lo` and `trees-lo`.

No `unit` (roads) key reads any of them.

### 3.2 Keys and versions

- `BLDPREP_V` and `BUILDINGS_V` (the fill and the tiles) in `pipeline::bld`; `BLD_FETCH_V` in
  `agent::build`. `Keys` holds `bldprep`, `bldtiles` and `bldfield-lo` by tile; a prune forgets them.
- **`bldprep T`:** `BLDPREP_V`, the release, and each downloaded file with a row group meeting T
  (name, ETag, row group indexes), and the GHSL tiles meeting T by name and size
  (`pipeline::bld::sources`, from `buildings.json`, `footers.json.gz` and `index.json`).
  `agent::input_digests` puts each as "bldprep 6/x/y" in the plan's inputs, recomputed only when an
  index changes; "bld-release" is the release, "" before any download, "?" when unreadable (the
  chain then waits).
- **`bldtiles T`:** `BUILDINGS_V`, the content names of `work/bld/` for T and its 8 neighbours ("-"
  for none), `Coverage::shapes_key` of T's box grown by 1 km (the shapes meeting it, in the recipes'
  order, each with its country: a building's country is its first containing shape's, and the
  fill's fits go by it), and the content names of the newest pass's heritage slices of T.
  `Outlines::country_code` gives an `osm:` region's ISO 3166-1 code (its own, or from a subdivision
  code ISO lists as a country: US-PR, CN-HK, FR-GF, FR-PM; else the smallest country it lies in).
- **`bldfield-lo`:** its mids' content names.
- **`bld-fetch`:** its version, the release and `coverage_all` (not the footers: it writes them).

### 3.3 Order, publishing, the chain

`agent::build::bld_work`:
- **A chain of its own**, beside the roads', trains' and landmarks'. It reads no roads and no
  terrain, so runs from the start: `bld-fetch` when its key changed; a prune of files no target
  has; every stale `bldprep`; `bldtiles T` once T and its 8 neighbours are prepared and the sources
  are here; `bldfield-lo` once its z3 tile's `bldtiles` are done. Listed after the landmarks', tiles
  in the regions' build order. No "bld-release" (or "?") in the inputs: no buildings work.
- **Jobs:** `bld-fetch` is the lead's, granted to it alone; the buildings steps go to any member
  (docs/pool.md §7.2), a job beside others by their measured memory, disk and cores, never beside a
  cold start (docs/pool.md §7.7), as many tiles as fit a quarter of an hour by their measured
  times (`coord::JOB_S`).
- **Publishing:** buildings hold no region's readiness; their packs go out with the next catalogue,
  at most one a minute (docs/building.md §2).
- **Status:** the checklist's "3D buildings" stage, its steps "Buildings' sources", "Buildings
  read", "3D buildings", "3D buildings, zoomed out" (docs/pipeline.md §3). The forecast
  chains `bldtiles` after `bldprep` and `bldfield-lo` after `bldtiles` (`forecast::chain_deps`).

### 3.4 Formats

docs/formats.md has the bytes. In brief:
- **Normalized buildings** (`work/bld/6-x-y`, RDSECT, not served): a zstd block per z14 tile (of the
  centroid), records sorted by Overture id, column by column.
- **Mesh tiles** (`layers/buildings/hi/6-x-y`, RDPACK, encoding `mvt`, z14 only, no lo or root
  packs): MVT 2.1, extent 4096, layer `b`. Each building or part is **whole, in the tile holding its
  centroid** (not clipped), so MapLibre extrudes it from one centroid: no step at tile edges, nothing
  drawn twice. A building reaching into other tiles is also **copied** whole into each (`o` 1): the
  flat and pick layers draw copies, the extrusions don't.
- **No axis-parallel edge beyond the extent:** MapLibre takes one for a clip line and draws no wall
  on it, so such edges get extra points a unit off the line at MapLibre's subdivision lines and
  midway (outside the ring, else inside, else left).
- Quantized to the tile grid (0.6 m at z14), not simplified (`vtgen` would clip and simplify at
  1.8 m); encoded over `names::mvt`. Properties `h`, `m`, `s`, `f`, `c`, `k`, `o`; feature ids
  x << 34 | y << 20 | index, a copy sharing its building's id (feature state uses them, §4.1).
- **The field** (§4.8): `layers/buildings-field/{hi,lo}`, encoding `field5-webp`.

### 3.5 Served

- `/tiles/buildings/{z}/{x}/{y}` and `/tiles/buildings-field/{z}/{x}/{y}`: the pack's tile as stored
  (`/tiles/buildings`' `?v=`: its pack's version in `/api/meta` `packs`; the field's carry none), 204 where none. The app reads them from a host of
  their own (`buildings.localhost`).
- **Mirror:** a copy group of its own after the other hi packs (`store::mirror::groups`); field lo
  packs come with the World download.
- **Devices:** the service worker keeps building tiles in a cache of their own (`bld`), so they
  don't crowd out roads and terrain.
- **Credits** (`pipeline::rules::CREDITS`, everywhere): Overture (ODbL 1.0, the release named) and
  GHSL (CC BY 4.0), as their declarations' descriptions credit them once accepted (docs/inputs.md
  §4.8, §6.9); `CREDITS`' entries stand until then, a test tying Overture's to the staged
  declaration's release.
- Nothing is published or redistributed (docs/storage.md).

### 3.6 Sharing the work

- **Helper Macs** (docs/pool.md): `bldprep` and `bldtiles` are shared steps
  (`agent::steps::SHARED`); a hand-off saves only its tiles' files (`coord::saves`). `bldprep` needs
  the NAS (up to ~3 GB of parquet a tile). Memory is learned per target (`SCENIC_COSTS`, "bldprep
  6/x/y", "bldtiles 6/x/y"); first guesses (`agent::bld_peak`): 0.3 GB + 160 B a row read for
  `bldprep`, 0.25 GB + 280 B a building of the largest z8 tile (taken as two fifths of the rows) for
  `bldtiles`.
- **Pages** (docs/workers.md §3, `pipeline::bld::task`): under the agent, a `bldtiles` job offers
  some of each tile's z8 tiles as tasks of kind `bldtile` while workers that take them are around
  (one per worker, three at once, from the far end of the tile's list). The task's files, cut on the
  lead: the blocks of T's and its neighbours' work files the z8 tile reads, the coverage's shapes that
  can answer for its points, T's heritage points. The program `bldtile` (`/work/prog/bldtile.wasm`
  in a page, the app's `bldtile` on a Mac via `scenic run-task`) writes the z8 tile's z14 tiles, field
  levels 14–9, level-8 planes and summary; the job puts them into the pack in the z8 tile's turn. In
  its turn, a z8 tile not taken waits 30 s for a faster worker, then is made here; one out is waited
  for while the worker's pace says it'll beat the job, else raced. A worker's first three results,
  then one in eight, are checked byte for byte against the job's own run; a difference marks the
  worker bad. Memory first guessed from its files and the model above; a worker's measure replaces
  it (`bldtile 8/x/y`). Z8 tiles larger than a page can hold go to a Mac.
- `bldprep` isn't a task: pages can't reach the NAS's parquet.
- An app that predates these steps drops their records; `Keys` keeps records it doesn't know
  (`other`).

### 3.7 Determinism

Same inputs, same bytes, on any machine and in WebAssembly (docs/building.md §2): parquet decoded by the
pinned parquet crate, every derived number computed in Rust (E7 by
`(v * 1e7).round()`, Mercator by `det`, f64 in a fixed order); heights as integer decimetres; a
median of an even count takes the lower middle; ties by id; records by (z14 tile, id), features by
(Morton, id); field sums in i64 fixed point (order-independent); gzip by `flate2`, zstd, at fixed
levels. The unit tests build on one thread and several; `tools/check/bldtile-same.mjs` checks a
task made natively and as WebAssembly against the whole-tile build.

## 4. The map

### 4.1 Rendering

**MapLibre's fill-extrusion** (`web/src/buildings.ts`): it handles the globe, the 3D terrain (roof at
the centroid's elevation), tiles and data-driven paint. An opacity under 1 draws in two passes
(depth, then colour); at 1, one.
- **z14 tiles whole above z14** (`keepWhole`, this source only, `reparseOverscaled` off): MapLibre
  would otherwise slice them into z15–16 pieces, each standing at its own centroid (stepped roofs,
  cut overhangs). It hooks our MapLibre's tile manager (`web/maplibre/`) and warns in the console
  if the hook is missing or a deeper tile appears.
- **A source and layers:** source `bld` (z14, `/tiles/buildings/…?v=`); `buildings`
  (fill-extrusion: height `h / 10` × the scale, base `m / 10`, vertical gradient; buildings and parts,
  not copies); `buildings-flat` (fill, for flat mode: buildings and outlines with copies);
  `buildings-pick` (fill at opacity 0: the footprints the hover queries). `buildings`' colour is a
  `case` on feature state (`hover` amber, `her` a heritage tint, else the mode's colour). After them,
  `roads-bridges` and `rails-bridges` (§4.2).
- Picking is the app's own (§4.5): MapLibre's `queryRenderedFeatures` ignores the terrain for
  extrusions and finds nothing on the globe.

### 4.2 Where in the style

- `addBuildings(map, 'water-name-line', …)` after `rails` (main.ts), when the catalogue has the layer
  or once a new catalogue brings it: among the 3D layers, since between the draped layers it would
  split the terrain's drape. `buildings-flat` and `buildings-pick` among the draped layers, after
  water and parks, before `boundary-county`.
- **Occlusion:** extrusions test and write depth against the terrain's. The road and rail layers
  (custom, 3D) test against that depth with their tolerance (1.5 % of the distance, at least 75 m ×
  exaggeration), write none, and come first: buildings paint over what's behind them and never over
  a road in front.
- **Bridges and elevated rail:** while buildings stand in 3D, the road and rail layers leave out
  their quad tiles' bridge pieces, and `RoadLayer.bridgeLayer` (`roads-bridges`, `rails-bridges`),
  after the buildings, draws them against the terrain and the buildings with a tighter tolerance
  (0.2 % of the distance, at least 3 m × exaggeration). The faint behind-terrain pass stays in the
  road layer. A deck within that tolerance of a wall behind it can show over the wall's edge.

### 4.3 On the terrain and the globe

- **Walls on the terrain under each corner** (our MapLibre's `fill_extrusion.vertex.glsl`): a base of
  0 is the ground under each corner (`get_elevation(a_pos)`) less 2 m, never above the roof; the roof
  stays level at the centroid's ground plus the height. A part with its own base keeps it above its
  centroid's ground.
- **No saw at the feet on the globe** (`_projection_globe.vertex.glsl`): float32 placement on the
  unit sphere put each vertex a metre or two off, differently per vertex and frame, so feet
  flickered. `interpolateProjectionFor3D` (terrain, its depth, extrusions, circles' and symbols'
  visibility) takes the tile's flat projection plus the globe's difference from it, interpolated
  across the vertex's z14 cell from its corners, so terrain and buildings move together.
- **Depth precision** (`camera3d.tuneDepth`): the near plane is half the nearest distance anything
  drawn can be (the ground, buildings under 30 m, each taller mesh building and field tile by its
  box), never closer than the flat map's.
- **Camera clearance:** camera3d refuses a move within 30 m of the ground or 4 m of the highest roof
  under it (`buildings.roofAt`: loaded footprints rising more than 26 m at the height scale).

### 4.4 Styling

- **Plain** (default): the plain colour (#566173 by default, Settings → Buildings → Plain colour;
  also the by-height low end's and misses' colour and the unknown source's), lighter roofs, lit by `map.setLight` from the hill-shading's
  azimuth, intensity 0.35.
- **By height:** the shared colour-scale controls (`ui/scale.ts`): a histogram of the heights of the
  buildings in view (read when idle), auto-fit to percentiles, Lock / Full / Equalise, the
  colour-map picker, low-end fade and highlight. The far field takes the same colours (§4.8).
- **By where the height comes from:** measured green, floors blue, Microsoft violet, neighbours
  amber, GHSL red-orange, size grey. The far field stays plain.
- **Heritage tint:** loaded buildings holding a heritage site's point the overlay shows
  (`dots.points`), recoloured by feature state 60 % toward their group's colour (the highest group
  where sites share a building). On by default while Heritage sites show (`bd=`'s sixth field);
  over any colour mode. The field tints the same buildings (§4.8).

### 4.5 Interaction

- **Hover** (`buildingAt`, at most once a frame): the building the cursor's view ray meets first.
  - Where the ray first meets the terrain: stepped down from the camera in 64 heights, halved 8
    times, re-stepped up to 4 times if a nearer crossing (a thin ridge) appears.
  - From a metre past that point back toward the camera until the ray is above the tallest top
    around (+ 20 m × exaggeration), each step takes the footprints tall enough to reach the ray:
    those 30 m or more from an index of the loaded tiles (remade when idle), the shorter by
    MapLibre's query in screen boxes (at most 24 × 96 px), each box's ground from its corners.
  - Each candidate tested against the ray between its roof and base as drawn (§4.3: the base at the
    lowest foot of its outline, sampled at up to 16 corners); the one met highest wins.
  - A road or rail line under the cursor gives way when the ray to it meets a building first.
    Markers win over buildings; the areas a building is in show as chips.
  - Known: off by up to ~7 px during the globe-to-flat hand-over (zoom 15.5–16.5), which `rayAt`
    doesn't follow; nothing found with the camera inside a hill.

### 4.6 Levels of detail and the iPad

- Meshes are z14's, from map zoom 14; farther out, the field (§4.8).
- **Memory:** what counts is everything the source holds, in view **and in its cache**: GPU buffers
  (MapLibre frees the page's copy once uploaded, but keeps the extrusions' centroids), raw tiles and
  feature index. With whole z14 tiles (§4.1) the source's cache is 8 tiles; the densest view after
  panning (Tokyo, z13.9, 70°) holds ~200 MB. Budget on the 8 GB iPad: ≤ 300 MB of buildings, ≤ 8 ms
  of GPU a frame.
- On the iPad (Shinjuku, Midtown, Châtelet) the buildings took ≤ ~88 MB of buffers and changed the
  frame rate within the whole map's own variation; the frame rate is the map's, not the buildings'.
- On a touch screen the default opacity is 1 (one pass) (`state.ts`).
- **If over budget** on touch devices: z14 tiles only from zoom 15, a smaller cache (4 tiles), or the
  centroids freed after upload (a change to our MapLibre).

### 4.7 Planned: a layer of our own

A custom WebGL2 layer, only if measurements call for it: exact occlusion with roads (the buildings'
depth read by the road shaders, via our MapLibre's `readsSceneDepth`); pitched roofs (tagged shapes,
gabled roofs inferred for small houses); distance-based detail under our control.

### 4.8 Far away, the field

The aim: buildings at every zoom looking as full detail would, rendered at high resolution and
downsampled. Builder `pipeline::bld::field` (in `bldtiles` and `bldfield-lo`); renderer
`web/src/bldfield.ts` with `bldfield.worker.ts`, loaded by `import()` from `main.ts` where the
catalogue has the layer `buildings-field` (`?bldfield=off`: off; `?bldfield=<url>`: tiles from there).
`web/src/bldcmp.ts` (`?bldcmp`) and `tools/bldcheck/` compare it with full detail.

**What full detail shrinks to.** Once a building is under a pixel, a pixel holds a share of roofs, a
share of visible walls (more the more tilted the view) and ground: sums over buildings. Per texel
and per **five height classes** (tops under 8 m, 8–20, 20–40, 40–100, 100 m and up), so a tower
keeps its height among houses:
- `f`, roof coverage (exact signed areas, clamped to 1);
- `w`, wall area per ground area (Σ wall length × wall height ÷ area);
- `v`, Σ coverage × top (`v ÷ f` the mean top);
- `ox`, `oy`, Σ coverage × offset to the building's centroid (so roofs are level at their own
  building's ground, as the meshes' are).

Being densities, a coarser level is exactly the mean of four: no pop between levels. This is the
roads' approach (`roads/lod.ts`: every road in every tile, areas summed) applied to buildings.

**Bands.** The field draws wherever MapLibre's cover has no loaded z14 mesh tile (below zoom 14
everywhere, beyond the meshes of a tilted view), at level ⌊zoom⌋ + 1 (4–14; under zoom 3, level 4
mipmapped), finer by distance as the cover splits (the fork's tile zoom by depth, FORK.md). A
level's texel is half a CSS pixel where first drawn. Every level holds every building, so nothing
is drawn twice nor missed.

**Tiles** (formats.md, The field): 512 × 512 texels, levels 4–14 (level 14's texel 4.8 m at the
equator).
- Built in `bldtiles T` per z8 tile with the meshes: every building and part extruded meeting the
  z8 tile, rasterized into level 14 (exact areas, walls deposited texel by texel, i64 fixed point),
  then levels 13–9 by means of four. Each z8 tile's level-8 planes go exactly into T's mid
  (`work/bldfield-mid/6-x-y`); `bldfield-lo` makes levels 8–4.
- Encoding `field5-webp`: 8-bit lossless WebP (√f, log2(1 + w), each class's mean top within its
  range; then the centroid offsets). Decoded to linear values in the worker before mipmaps.
- A level-8 tile may carry its **heritage list**: each building or part holding a heritage site's
  point (T's heritage slices, World Heritage components left out), with top, base and a simplified
  footprint (≤ 16 points).

**Rendering** (`bldfield.ts`'s header has the details):
- Each tile drawn into MapLibre's own frame as instanced boxes, one per block of 8 × 8 cells holding
  buildings (lowest ground to highest roof, widened for the globe's chords), depth-tested in
  hardware against the frame (terrain and meshes), sorted far to near.
- Each fragment ray-marches its block: walls met at a density rate below each class's roofs, a roof
  met with probability f where the ray comes down through it, events ordered as a descending ray
  meets them, the nearest surface winning; steps of at least 2 texels (`?fstx=`), only the classes a
  texel's mask holds; empty space skipped by per-cell tops; a step whose reads hold no roof as high
  as the ray skips the rest (it meets nothing). One loop walks the cells and takes the steps, the
  classes a step comes down through taken one after another, and a step without a heritage tint
  takes a copy of the code without it: the same image, ~28 % less GPU (task #222). The scene's depth (the fork's
  `sceneDepth`) stops the march at meshes or terrain on the flat map.
- A ray below the roofs where the footprints taller than it cover nearly all of a texel (over
  75 %, fully from 97 %) is inside a building, so it met a wall: those classes' walls come at up to
  2 more a metre there, opaque within a few metres. The
  walls are only in the texels their lines cross, and a facade's texel stops only part of the rays
  crossing it; without this the rest went on through the building's inside (no walls there), a
  different share each pixel column: thin vertical streaks down the walls just beyond the meshes,
  most in tilted views of dense cities (`?fdbg=37` draws without it).
- Coloured as the meshes are: plain (the plain colour), or by height on the shared colour scale, each class at its
  mean top (v ÷ f) through a lookup of the meshes' own colours (their 33 stops, the low-end fade and
  the threshold), so field and meshes match at the hand-over. Coloured by source, the field stays
  plain (it holds no source).
- Lit with fill-extrusion's formula (light, ambient, wall gradient, opacity); colours summed in
  linear light, the alpha set so the hardware's stored-value blend over the land colour gives the
  linear-light mix. Fogged as the meshes are. Lighting computed once a tile a frame on the CPU.
- Heritage: a field tile from level 8 on takes its level-8 tile's list and fills a 512² tint texture
  with the listed buildings the overlay shows (`buildings.heritageFor`; the sites asked for once a
  level-8 tile, the fill drawn in the worker); a hit's colour is mixed toward the tint by its share.
  Coarser than level 8 nothing is tinted. No hover on the field.
- GPU memory: class textures R11F_G11F_B10F with mipmaps, roof ground R16F; off-screen tiles kept
  under a soft target (`GPU_CACHE_TARGET_MB`: 1,024 MB, 512 on a `pointer: coarse` device), least
  recently wanted freed first; rebuilt after a lost context.
- Loading: at most six fetches in flight, coarsest first, abandoned ones cancelled; a loading
  tile's nearest loaded ancestor (however many levels up: zooming in fast skips levels never asked for) drawn over its area only (a ray that never enters that area discarded at once);
  failures retried after 2, 4 … 60 s; a 204 means nothing there or below. The worker packs the
  class textures and the roof ground into their GPU formats (`packfloat.ts`, as the browser would
  convert them: `tools/bench/packcheck.mjs`), so an upload is a copy.
- Elevations: each tile's grid sampled from the terrain as drawn (the fork's
  `elevationForLngLatFn`), again when the terrain's tiles in view change, within 3 ms a frame
  (`ELEV_BUDGET_MS`), the longest since sampled first; a tile without a hull yet at once. A tile
  whose grid comes out the same keeps its hull and roof ground.
- A still view isn't marched again: once two frames running draw the same tiles with the same
  camera and settings, the field is drawn into a texture of its own (RGBA16F, its fragments blended
  there as into the frame, then the texture over the frame: within one level of the frame's 8 bits
  of drawing straight into it), and later frames draw only that texture, marching again only the
  pixels whose depth (meshes, terrain) changed since (a tile coming in). Tokyo, zoom 14, 60°, still,
  M1 Pro: the field ~1 ms a frame instead of ~17 (`?fcache=0`: off). A moving view draws straight
  into the frame, as before. The cache (~56 MB at 2880 × 1626) is freed when the field is off or
  draws nothing, without the frame's depth, and after the view has moved for 60 frames running.
- Debug and tuning switches: `?fdbg=` (43: each pixel's steps, cells walked and fragments, added up
  in the frame), `?fstx=`, `?fopt=` (default 3).

**Quality and cost** (`tools/bldcheck`, against full detail shrunk in linear light, σ 8 px blur):
within about 1 L* of full detail in a district's tone from z13 to z7, the CN Tower standing as a
spike to z9; ~0.6–1.7 ms of GPU a frame on the M4 at 2880 × 1626 (budget 2 ms). On the M1 Pro at the
same size, Tokyo at zoom 14 and 60° (`tools/bench/views.mjs --native`, the frame's GPU time, task
#222): 53 ms before the march's changes, 40 ms after (Manhattan 55 → 41, Hong Kong 28 → 22; orbiting
Tokyo 56 → 36), ~8 ms with `?bldfield=off`; a still view repainted, with the cache, 7.7 ms (Manhattan
9.1, Hong Kong 7.5). Moving, the field still holds the frame rate near 30 fps there.

**Known:**
- Slightly softer than full detail at the device pixel; up to ~1–2 L* darker in dense tilted views
  and at the hand-over to the meshes near the camera of a tilted zoom-14 view (the random-overlap
  wall model against streets of buildings in rows).
- Over high ground the map's zoom (measured from a sea-level pivot) understates how close the view
  is, so mesh tiles and the field are coarser than they should be in mountain towns.
- Not done: the source colouring in the field (the owner's choice: a fourth channel, or plain); a measured forecast cost for
  `bldfield-lo`; measurements on the iPad (`__bld.frames(120)` in the console).

#### 4.8.1 The model's choices

- **Ray-marched, not painted:** painting each building's walls on the ground at its foot smeared
  towers at glancing angles into slabs; marching a volume of buildings keeps a tower a spike.
- **Five height classes** per texel rather than one mean height, for the same reason; enough for a
  smooth colour-by-height scale.
- **Nearest hit, blended once:** as MapLibre draws translucent extrusions (depth pass, then one
  colour pass), so no multi-surface transmittance.
- **Linear light:** summing hits as stored values darkened the field by several L*.
- **8-bit storage:** √f keeps a hamlet's coverage visible; log2(1 + w) to 4096 holds a tower's edge
  texel; rendered differences from 16-bit under 0.05 ΔE.
- **Densities per ground area** at each texel's own latitude, so high latitudes match.
- **Wall normals** face into the ring as the tiles wind it; the field lights walls accordingly.
- **Tiles per z6 tile:** nothing crosses the antimeridian; the cover takes tiles on both sides.
- **No cross-fade** at integer zooms: it would need both representations drawn at once.

## 5. Risks and planned work

- **The release leaves S3 on 2026-11-25.** The coverage's files stay on the NAS; a later region
  whose files are missing fails `bld-fetch` (naming them) until the next pinned release, which
  fetches ~62 GB and rebuilds every z6 tile's roads and every buildings tile. Old releases are deleted by hand (GC
  never sweeps `sources/`).
- **Estimated heights:** 88 % of buildings; tall unmeasured ones come out low, so skylines are thin
  where heights aren't measured. Planned: measured heights from BD TOPO (France) and PLATEAU
  (Japan's cities).
- **Planned:** building heights in the near-field horizons and the viewshed tool (every z6 tile's roads
  rebuilt); a custom layer (§4.7); click to open the OSM way; the tallest building in the In view
  summary.
- **Occlusion artefacts:** where the terrain mesh lies above a road, a building behind it can show
  over it; a bridge deck near a wall (§4.2).
- **Rare geometry:** a MultiPolygon's polygon or a hole wholly beyond one edge of its tile isn't
  drawn (MapLibre skips rings outside the tile); copies reach only 310 m beyond a z8 tile, so a huge
  building's flat footprint can be cut; neighbours aren't wrapped across the antimeridian.
- **Overlapping footprints** from conflation could z-fight; none seen so far.
