# 3D buildings

**Plan, not built** (2026-10-05). The first of plan.md §10's phase 7 features ("3D buildings, then
PLATEAU"); plan.md §6 (Global-source layers) points here. Companions: `docs/plan.md` (the pipeline,
keys, order), `docs/formats.md` (files), `docs/workers.md` and `docs/pool.md` (sharing the work).
Its sources are on the NAS (§2.6).

**The idea in one line:** every building in the coverage, extruded to its height on the 3D terrain,
from the same pinned Overture release the roadside buildings read, the missing heights estimated
from the neighbours, then GHSL's 100 m building heights, then the footprint's size and kind; built
per z6 tile in a chain of its own beside the regions', shared with helper Macs and pages, and drawn
by MapLibre's fill-extrusion.

## 1. What the map shows

**A building** is a footprint polygon (with its holes) extruded from the ground to its height.
- Overture's building parts (OSM's `building:part`: a church's nave and tower, a skyscraper's
  setbacks) are drawn between their own base and top, and the outline they belong to (`has_parts`)
  isn't extruded (its parts are its shape), as OSM's Simple 3D Buildings has it; an outline none of
  whose parts are in the files is drawn as a building.
- Flat roofs. Roof shapes are known for 0.6 % of the coverage's buildings and roof colours for
  0.1 % (§2.2): neither is drawn in the first phases. Pitched roofs are a later option (§4.7).
- Underground buildings (`is_underground`) are left out.

**Where:** every building that touches the coverage, as a way does (plan.md §5: any vertex inside
it, with its 1 km buffer), whether or not a road is near; not the world. About 340 million
buildings in today's 88 regions (§2.5). Where the coverage ends, buildings end, as roads do.

**At which zooms.** Tiles at z12, z13 and z14; MapLibre overzooms z14 to the map's 19.5.
- z14: every building.
- z13: buildings 20 m tall or more, or with a footprint of 2,000 m² or more.
- z12: buildings 40 m tall or more: the skyline.
- So the layer shows the towers from zoom 12, the large buildings from 13 and every building from 14.
  A tilted view takes coarser tiles toward the horizon (MapLibre's cover), so every building near the
  camera, only the tall ones farther out, and none beyond the z12 tiles. The thresholds are set in
  phase B1 by the tiles' sizes (at most ~300 KB a tile).

**Beside the terrain.**
- A building stands on the 3D terrain at its centroid's height (MapLibre samples the terrain there),
  its base sunk 10 m so that it doesn't float on a slope. Phase B3 sets each wall's foot on the
  terrain under its own corner, the roof level (§4.3).
- Heights are true, not exaggerated: at the default 3× terrain a house looks low beside the hills,
  as in Google Earth. A setting scales them (1–3×) or makes them follow the terrain's exaggeration.
- The hill-shading, slope tint, tree cover and contours lie on the ground under the buildings.
- Fog: MapLibre fogs the terrain, not extrusions (its fill-extrusion fragment shader is the colour
  alone, checked in 6.11.2); B3 patches it as `vite.config.ts` patches the circle and symbol
  shaders, so the far skyline fades with the ground.

**Beside the roads and rail.** Buildings are drawn after the road and rail layers (§4.2):
- a road behind a building is hidden by it (faint through it with an opacity under 1);
- a road in front of a building stays in front, because a road lies on the terrain, and anything
  behind the terrain's surface at a pixel fails the depth test against the terrain;
- the exceptions are what stands above the terrain: bridges and elevated rail in front of a
  building are painted over by it. B3 draws bridge and elevated pieces again after the buildings.

**Beside the landmarks and labels.** The landmark dots (`dots.ts`) draw without a depth test, after
the buildings, so a heritage site's dot stays visible on its own cathedral. Labels are symbols,
which MapLibre doesn't hide behind extrusions. The selected road, drives and climbs are draped line
layers: a building in front of them hides them, as it should.

**The look:** a muted blue-grey made for the dark map, lit from the hill-shading's light direction
(Settings → Terrain), the walls darker toward the ground (MapLibre's vertical gradient), so the
coloured roads and the landmarks stay what the eye goes to. Colour modes (§4.4): plain; by height,
with the shared colour-map picker and scale; by where the height comes from.

**The toggle:** Settings → Buildings, a section after Trees with its switch in the header, on by
default. In it: 3D or flat (footprints only, also what a 2D view shows); colour mode; opacity; height
scale; detail (skyline only, or all). In the link with the other settings; **B** toggles the layer.

**Hover:** with nothing else under the cursor, the bottom bar's row 1 gives the building's height
and floors, where the height comes from ("measured", "from 6 floors", "estimated from neighbours",
"estimated, GHSL", "estimated from its size") and its kind. Markers, roads, rail and areas win over
buildings. No popup in the first phases.

**The iPad** (8 GB iPad Pro, Safari): §4.6 has the budget and the fallbacks.

## 2. Data

### 2.1 The sources compared

| Source | Gives | Here | Licence | Use |
|---|---|---|---|---|
| **Overture Maps buildings**, release 2026-09-23.1 | 2.53 billion footprints worldwide (OSM, Microsoft, Esri, Google, IGN España, others, conflated), `height`, `num_floors`, `min_height`, `min_floor`, roof shape and colour where known; 4.49 million building parts | ~340 M buildings in the coverage; height or floors for ~46 % (§2.2) | ODbL 1.0 for the theme; each row's sources and their licences in its `sources` column | **Yes**: every footprint and attribute |
| OpenStreetMap `height`, `building:levels`, `roof:*` | The tags | Already inside Overture: OSM gave 58 % of the sample's footprints, and their tags became `height` / `num_floors` | ODbL | Through Overture. Read directly it would need every building in the pass's filter (~600 M ways, a new set) for a month's freshness and nothing else |
| Microsoft Global ML Building Footprints, with heights | ML footprints and height estimates | Already inside Overture: "Microsoft ML Buildings" is the height source of 3–51 % of the buildings in the samples (Paris 3 %, Toronto 36 %, rural England 51 %) | ODbL | Through Overture |
| USGS 3DEP lidar heights | Measured heights | Already inside Overture ("USGS Lidar": 47 % of Manhattan's sample, 73 % of Chicago's) | Public domain | Through Overture |
| Google Open Buildings 2.5D Temporal | Building presence and height rasters, 4 m, 2016–2023 | Africa, South and Southeast Asia, Latin America and the Caribbean: of the coverage, only Singapore, Puerto Rico and French Guiana | CC BY 4.0, via Earth Engine (an account) | No. Overture already merges Google's v3 footprints there |
| **GHSL GHS-BUILT-H R2023A, ANBH** (EC JRC) | The average height of the buildings in each 3″ cell (~90 m), epoch 2018, worldwide | Every built-up cell | CC BY 4.0 (© European Union) | **Yes**: the fill (§2.3), 1.28 GB |
| France: IGN BD TOPO (bâti) | Measured `HAUTEUR` and floors for every building | France | Licence Ouverte 2.0 | Later (B4): would replace France's estimates |
| Japan: MLIT PLATEAU | LoD1/LoD2 city models of ~250 cities, measured heights | Japan's cities | CC BY 4.0 compatible | Later (B4), as plan.md §10 already plans |
| Great Britain | No open building heights (Ordnance Survey's aren't open; the Environment Agency's lidar would be a project of its own) | — | — | No |

**Decision:** Overture alone for footprints and attributes, the same pinned release as the roadside
buildings (`pipeline::buildtiles::RELEASE`, 2026-09-23.1), so the roadside factor and the drawn
buildings agree; GHSL for the fill. National measured heights later, country by country, where they
replace estimates.

### 2.2 Overture, measured (2026-10-05)

- **The release:** 515 files on S3: 512 of buildings (276.9 GB, 2,533,842,612 buildings in 82,688 row
  groups of ~34,000) and 3 of building parts (0.62 GB, 4,486,107 parts). Every row group's footer
  carries its bbox statistics, so the files and row groups meeting an area are known from the footers
  alone (`dem/bldfetch.py` reads them once into `footers.json.gz`).
- **The coverage's share:** 100 building files (59.9 GB) and the 3 parts files hold a row group
  meeting the coverage grown by 20 km; those row groups hold ~357 M buildings (344 M with a 1 km
  margin) and 3.1 M parts.
- **It expires:** S3 says `x-amz-expiration: expiry-date="Wed, 25 Nov 2026"`, `rule-id="release
  data 60 day retention"`. Overture keeps about two months of releases (2026-08-19.0,
  2026-09-23.0 and 2026-09-23.1 listed today). Pinning a release means keeping its files.
- **Heights in a random sample** of 160 of the coverage's row groups (3.4 M buildings):

  | | `height` | `num_floors` | either | roof shape | roof colour |
  |---|---|---|---|---|---|
  | North America | 66.5 % | 1.9 % | 67.0 % | 0.2 % | 0.0 % |
  | Western Europe | 20.5 % | 14.3 % | 33.0 % | 1.5 % | 0.1 % |
  | East Asia | 7.0 % | 2.6 % | 7.8 % | 0.1 % | 0.0 % |
  | all | 41.3 % | 5.9 % | 45.7 % | 0.6 % | 0.1 % |

  By place (the row group holding each point): Manhattan 98 %, Chicago 92 %, Los Angeles 99 %,
  rural Kansas 47 %, rural Vermont 78 %, Toronto 48 %, Montréal 31 %, Vancouver 64 %, rural Québec
  35 %, Paris 67 % (floors, from OSM), rural France 17 %, London 49 %, rural England 72 %, Dublin
  36 %, Madrid 34 %, rural Spain 80 % (floors, from IGN España), Lisbon 19 %, Tokyo 33 %, Osaka
  72 %, rural Japan 0.2 %, Taipei 20 %, Hong Kong 33 %, Singapore 24 %, San Juan 75 %, Honolulu 83 %.
- **Footprints' sources** in the sample: OSM 58 %, Microsoft ML Buildings 34 %, IGN España 3.1 %,
  Esri Community Maps 2.4 % (CC BY 4.0 with OSM waivers), `doi:10.5281/zenodo.8174931` 1.7 % (East
  Asia), Google Open Buildings 1.3 %, City of Vancouver. 86–91 % of the buildings without a height
  have no class either.
- **Height per floor**, where a building has both (medians): East Asia 3.6 m (1 floor 4.7 m, 2
  floors 3.65, 3–4 floors 3.2); North America 3.5 (2 floors 2.9, 3–4 floors 3.0); Western Europe
  2.7 (2 floors 2.3). Measured heights in Europe and North America often stop at the eaves or come
  from ML estimates: the storey height is fitted per country (§2.3).
- **Measured heights' medians:** North America 4.5 m (p99 11.7), Western Europe 4.4 m (p99 16.4),
  East Asia 6.9 m (p99 26).
- **How this repo reads it now:** `dem/buildings.py --world` streams the release's bbox columns
  (~32 GB of its 277) into z8 tiles of boxes (`sources/buildings/2026-09-23-1/`, 11,834 tiles),
  which the units read for the roadside-buildings factor (`pipeline::buildings`). Heights, footprints
  and parts aren't read. That stays as it is.

### 2.3 Filling the missing heights

Each building's height (its top above the ground) is the first of these that it has, and it
records which (`s`, 0–4), for the hover and the "where the height comes from" colouring:

0. **Measured:** Overture's `height` (lidar, OSM's `height`, Microsoft's or Esri's estimate), taken
   when it is 2–700 m. A building part's base: `min_height`.
1. **Floors:** `num_floors` × the country's storey height + a roof allowance; a part's base from
   `min_floor` likewise. Fitted in B0 per country from the buildings that have both; until then
   3.0 m a floor + 1.0 m.
2. **Neighbours:** the median of the heights (by rule 0 or 1) of the buildings within 150 m whose
   footprint is between half and twice its own, when there are at least 5; else of any footprint
   within 300 m, when there are at least 8. Streets and blocks are uniform, and cities have heights
   for a fifth to a half of their buildings (§2.2).
3. **GHSL:** the ANBH value of the 3″ cell holding the footprint's centroid, when the cell has
   buildings. A footprint under 60 m² takes at most 4 m (garages, sheds). Sampled on 2026-10-05:
   Midtown Manhattan 31.5 m, Lower Manhattan 35.2, Brooklyn's row houses 18.0 (high), Back Bay 17.3,
   downtown Toronto 42.8, the Plateau in Montréal 12.7, Montpelier 15.0; a cell of scattered farms is
   0 (no buildings found), which falls through to the defaults.
4. **Size and kind:** sheds, garages, carports and huts, or under 30 m²: 3 m; houses and residential
   kinds, or under 250 m²: 6.5 m; 250–2,000 m²: 9 m; larger: 10 m; churches and cathedrals: 15 m.

**Checked, not guessed (B0):** a tenth of each country's measured buildings (by a hash of their id)
are held out and filled by rules 1–4 as if unmeasured; each rule's median absolute error and bias
per country are reported, and the order, radii, counts and defaults are set from them. The fill's
version is in the buildings step's key (§3.2), so a change rebuilds every tile and nothing else.

Estimated: 92 % of East Asia's buildings, two thirds of Western Europe's, a third of North
America's (§2.2). Japan's cities get measured heights from PLATEAU in B4.

### 2.4 Heights and the roadside factor

The roadside-buildings factor (scenic metrics) stays on the boxes and ignores heights. Building
heights in the near-field horizons and the viewshed tool are a separate phase-7 item: they would
change every unit's key (every unit rebuilt), so they're decided on their own. The normalized files
(§3.4) are made so that a z11 grid of building heights can be drawn from them then.

### 2.5 Sizes

| | |
|---|---|
| downloads (§2.6) | 61.8 GB: Overture 60.5 GB (103 files), GHSL 1.28 GB (91 tiles) |
| buildings in the coverage | ~340 M (344 M in the row groups meeting the coverage + 1 km) |
| z6 tiles meeting the coverage | 380. By the row groups' centres (so roughly, and counting some buildings just outside the coverage), 27 tiles hold over 5 M: the densest 6/56/25 (Tokyo to Osaka, ~30 M), 6/32/22 (Paris and northern France, 15 M), 6/18/24 (New York to Washington, 15 M), then ~10 M each: 6/32/23 (southern France), 6/31/21 (southern England), 6/17/23 (Toronto to Detroit), 6/55/25 (western Japan), … |
| normalized buildings (`work/bld/`) | ~40 B a building, zstd'd: ~14 GB |
| tiles, measured on samples | z14, gzip'd, with `h` and `s`: Tokyo 14.9 B a building, rural Japan 10.7, rural Vermont 16.9, Manhattan 18.7, London 21.8, Paris 25.9 (12.6 vertices a building) |
| tiles, estimated | ~7 GB at z14, ~8 GB with z12–13; the largest hi pack (6/56/25) ~0.5 GB |
| an app Mac's mirror | +8 GB |

### 2.6 Downloads (done 2026-10-06, for the 88 regions: 194 files, 61.8 GB)

`dem/bldfetch.py` fetches both sources onto the NAS, whole files as the sources have them:
- Overture's files with a row group meeting the coverage grown by 20 km (both types), into
  `sources/overture/2026-09-23-1/theme=buildings/type=<type>/<file>`, with `buildings.json` (each
  file's size, ETag, rows, row groups and box, and the coverage they were chosen for) and
  `footers.json.gz` (every file of the release's row-group boxes);
- GHSL's 10° tiles meeting it, into `sources/ghsl/R2023A/<file>.zip`, with `index.json`.

The coverage comes from the map's server (`/api/regions`, `/api/areas/<id>`), so adding a region and
running it again fetches only the new files, while the release is on S3. Each file is written to a
temporary name, resumed after an interruption (a range request; for S3 only while its ETag is the
same), checked (an Overture file's ETag, the MD5 of its 64 MiB parts; a GHSL zip's CRC-32s) and
renamed into place; a file in place at its listed size is skipped. Two transfers at once, at most
3 MB/s in all: the line gave ~2–4 MB/s from S3 on 2026-10-05, and the build needs some of it. The
log is the NAS's `state/logs/bldfetch.log` (a line a file, a progress line a minute). At 2–2.6
MB/s, 6.5–8.5 hours. After anything stops it (a restart, the NAS away too long), the same command,
from the repo, goes on from where it was:

```
nohup caffeinate -s uv run --project dem python dem/bldfetch.py \
  --root /Volumes/personal/projects/scenic-roads \
  >> /Volumes/personal/projects/scenic-roads/state/logs/bldfetch.log 2>&1 &
```

Why the coverage, not the world: the map draws buildings only in the coverage, and the world is
277 GB (four times as much). A region added after 2026-11-25 whose files aren't here waits for the
next pinned release (§5.2).

## 3. Pipeline

### 3.1 Steps

| Step | Target | Reads | Writes | Runs on |
|---|---|---|---|---|
| `bld-fetch` | the release | S3, JRC, the coverage | `sources/overture/<release>/`, `sources/ghsl/R2023A/` | the network slot; by hand until B2 |
| `bldprep` | a z6 tile T | Overture's row groups meeting T, the GHSL tiles meeting T | `work/bld/6-x-y` | any Mac with the NAS |
| `buildings` | a z6 tile T | `work/bld/` of T and its 8 neighbours, the coverage over T | `layers/buildings/hi/6-x-y` | any Mac; its z8 areas as tasks for pages |

- **`bld-fetch`** is `dem/bldfetch.py` (§2.6). From B2 the agent runs it as a network job (the
  second slot's first kind, with the heritage chain and the rail feeds) when its key changes: the
  release, `coverage_all` and the release's footers. It skips what's there, so a run is cheap.
- **`bldprep T`**, per z6 tile meeting the coverage (1 km buffer): `dem/bldprep.py` reads the row
  groups meeting T from the downloaded files (pyarrow, a row group at a time), keeps the buildings
  and parts whose centroid is in T, decodes their WKB (shapely), reads the GHSL cells under T
  (rasterio, out of the zips), and spools columns to local files; Rust (`scenic-build bldprep`,
  `pipeline::bld`) converts to E7, computes each centroid and area, samples GHSL at the centroid,
  sorts, and writes the normalized file (§3.4). Python only decodes; every number that ends up in a
  file is computed in Rust (one rounding rule, `det`). Reads ~60 GB over all tiles, once per
  release.
- **`buildings T`**, per z6 tile meeting the coverage: the buildings of T that touch the coverage,
  heights filled (§2.3; the neighbours' rule reads the buildings within 300 m beyond T's edges from
  the neighbours' files, by their z14 blocks), the z12–14 tiles encoded (§3.4), the hi pack written.
  Pure: its output is a function of its inputs' bytes. A z6 tile the coverage has left loses its
  pack, as tree cover does (`treepacks::targets`' "none" key).

Both are new `scenic-build` steps and agent steps; the units, the roads' chain and the landmarks
don't change, and no unit's key reads them.

### 3.2 Keys and versions

In `agent::build`, beside the others:
- `BLDPREP_V = 1`, `BUILDINGS_V = 1` (the fill's rules are in `BUILDINGS_V`).
- `Keys` gains `bldprep` and `buildings`, maps by z6 tile as `unit` and `pack` are; `Keys::map`,
  `recorded` and `record` take them, and a prune forgets them ("bldprep 6/x/y", "buildings 6/x/y").
- **`bldprep T`'s key:** `bldprep {BLDPREP_V}`, the release, and for each downloaded file with a row
  group meeting T its name, ETag and those row groups' indexes (from `footers.json.gz` and
  `buildings.json`, which the agent reads as it reads `inputs/`: a digest in `plan`'s `inputs`),
  and the GHSL tiles meeting T by name and size. A file fetched later (the coverage grew) changes the
  key of the tiles it meets; an unchanged result keeps its content name, so nothing after it reruns.
- **`buildings T`'s key:** `buildings {BUILDINGS_V}`, the content names of `work/bld/` for T and its
  8 neighbours ("-" for none), and `Coverage::fingerprint` of T's box grown by 1 km.
- **`bld-fetch`'s key:** the release, `coverage_all`, the footers' digest; kept with the lo keys under
  its own name, as `rail-feeds` is.
- The pinned release is `buildtiles::RELEASE` for both the roadside and the 3D buildings: a new one
  re-keys every unit (the roadside index) and every buildings tile together, never a mix.

### 3.3 Order, rounds, the chain

- **A fourth chain**, beside the roads', the trains' and the landmarks': it reads no unit and no
  terrain (the map puts buildings on its terrain), so it runs from the start, each step once what it
  reads is built: `bldprep T` once T's files are on the NAS; `buildings T` once T and its neighbours
  are prepared. Its work is listed after the landmarks' in `plan` (the build Mac's first job takes it
  when the regions' work is done or waits), its tiles in the regions' order (the region with the
  fewest units left first, then `spatial_order`), so the buildings of the region being built come
  first.
- **The second job** takes it beside the regions' work, as CPU work (not while the Mac is in use);
  never `bldprep` beside the OSM pass or another job that reads the planet through the NAS.
- **Rounds:** buildings don't hold a round, and a region's readiness (`ready`) doesn't wait for
  them: their packs go out with the next round's catalog, as the trains' and landmarks' outputs do;
  after the last unit, a catalog follows any chain's change. A region can reach the map before its
  buildings, which follow with a later round.
- **Batches:** about fifteen minutes of work a job, by the tiles' building counts (the densest tile
  alone).
- **Status:** the checklist gets "Raising the 3D buildings"; `label("bldprep")` "Reading the regions'
  buildings", `label("buildings")` "Raising the 3D buildings"; parts and progress lines as the other
  steps' (row groups read; z8 areas done; packs written).

### 3.4 Formats

**Normalized buildings** (`work/bld/6-<x>-<y>.<h>.sect`, RDSECT v1, content-named, in the
manifest; a work file, not served). Meta `{"fmt": 1, "tile": "6/x/y", "release", "buildings",
"parts", "srcs": [dataset names], "ghsl": "R2023A"}`.
- `index`: `(u64 z14 tile key, u64 offset, u32 len, u32 count)` per block, sorted by key.
- `blocks`: a zstd block per z14 tile (the tile of the centroid), as markdata's props are: the
  tile's records sorted by id, column by column:
  - centroid (i32 E7 × 2), footprint area (m², f32), ring and vertex counts, vertices (i32 E7,
    each ring's first absolute, the rest as deltas);
  - height and base (u16 decimetres, 0 none), floors and base floor (u8, 0 none), Overture's class
    and subtype as codes (u8 each), roof shape (u8), flags (u8: part, has parts), the height's source
    dataset (u8, into `srcs`), GHSL's value at the centroid (u16 dm, 0 none), and the OSM id where OSM
    gave the footprint (u64, type in the top bits; 0 none).
- Reading a neighbour's edge is a few blocks; a page's task is a slice of blocks (§3.6).

**Tiles** (MVT 2.1, gzip'd, extent 4096, layer `b`):
- One feature a building or part: its polygon(s), outer rings and holes, **whole, in the tile
  holding its centroid** (not clipped: a building is in one tile, its coordinates may run past the
  extent). MapLibre's extrusion then has one centroid a building, so no step at a tile edge on a
  slope, and nothing is drawn twice.
- Quantized to the tile's grid (z14: 0.6 m at the equator); repeated points dropped, rings that
  collapse dropped; at z12–13, simplified to one grid unit.
- Properties: `h` the top (dm), `m` the base (dm; parts; left out when 0), `s` the height's source
  (0–4, §2.3), `f` floors (when `s` is 1), `c` the kind (0 unknown, 1 residential, 2 outbuilding,
  3 commercial, 4 industrial, 5 religious, 6 civic, 7 agricultural, 8 transport, 9 other), `k` (1 a
  part, 2 an outline with parts: drawn by the flat layer only). No feature ids, no names (B1).
- In a tile, features sorted by their centroid's Morton code, then id.
- Encoded by `pipeline::bld` over `names::mvt`, not `vtgen`, which clips features at the tile's
  edges and simplifies at 3 units (1.8 m at z14: a house's corners).

**Packs:** `layers/buildings/hi/6-x-y` (RDPACK v1, encoding `mvt`, blobs gzip'd): z12–14 of the z6
tile. No lo or root packs. The catalog lists the layer `buildings`, encoding `mvt`, zooms 12–14
(`layer_zooms` and the encoding match in `scenic-build`'s catalog).

### 3.5 Served

- `/tiles/buildings/{z}/{x}/{y}`: the pack's tile as stored (`tiles::plain`, gzip, ETag the blob's
  hash, `?v=` the layer's version: `buildings.tiles` in `/api/meta`'s versions), 204 where there's
  none. No names attached (B1).
- **Mirror:** a copy group of its own after the hi packs (`store::mirror::groups`), so a Mac's
  mirror has the roads and terrain first; the M1's budget may leave buildings out, which the server
  then reads from the NAS.
- **Devices:** the iPad's service worker keeps versioned tiles it has shown (12,000 files at most);
  building tiles would crowd out the rest in a city, so they get a budget of their own (B2).
- **Credits** (`pipeline::rules::CREDITS`): "3D buildings: Overture Maps Foundation
  (OpenStreetMap, Microsoft, Esri Community Maps, USGS, IGN España, Google and others), ODbL" over
  the coverage; "Building heights where none are known: GHSL GHS-BUILT-H R2023A, © European Union,
  CC BY 4.0".
- Nothing is published or redistributed: the tiles stay on the NAS and the owner's Macs and devices
  (plan.md §3, the README's Terms).

### 3.6 Sharing the work

- **Helper Macs** (today's M1; any member in `docs/pool.md`): both steps are shared steps
  (`agent::claims::SHARED`), offered from the far end as terrain and units are. `bldprep` needs the
  NAS (it reads up to ~3 GB of row groups a tile) and up to ~3 GB of memory for the densest tile;
  `buildings` holds a z8 area at a time (~1–1.5 GB for the densest, Tokyo's). Each target's memory
  is learned (`SCENIC_COSTS`); first guesses: 1 GB + 120 B a building for `bldprep`, 0.5 GB + 150 B
  a building of its largest z8 area for `buildings`.
- **Pages** (`docs/workers.md`): a `buildings` job offers its z8 areas as tasks, as a unit job offers
  its tail. A task's files: the z8 area's blocks and the blocks within 300 m around it (cut from the
  work files on the Mac that runs the job), and the program `bldtile` (Rust, built for wasm32-wasi
  with the others, `/work/prog/bldtile.wasm`). It writes the area's z12–14 tiles (an RDTILES archive);
  the job assembles the pack. The densest z8 area (Tokyo's) is ~1–1.5 GB in memory, within the
  iPad page's 3 GB; a smaller ceiling gets z9 or z10 areas (workers.md's planned cutting to the
  worker). Nothing waits on a page: an area no page took runs on the job's Mac, one a page holds is
  raced there, results are compared (the ramped verification).
- **The pool's steps table** (`docs/pool.md` §6): `bldprep` {memory learned, disk 15 GB, the NAS,
  power}, `buildings` {memory learned, disk 15 GB, power}; neither needs home (`bldprep`'s reads are
  per tile, not the planet's).
- `bldprep` isn't a task: it reads 60 GB of parquet from the NAS, which pages can't reach and the
  coordinator shouldn't relay.

### 3.7 Determinism

Same inputs, same bytes, on any machine and in WebAssembly (plan.md §8, Determinism):
- the parquet decoded by one pinned pyarrow and shapely (the app's `dem/` environment), and every
  derived number computed in Rust from the decoded doubles: E7 by `(v * 1e7).round()`, Mercator by
  `det`, areas and centroids in f64 in a fixed order;
- heights as integer decimetres; a median of an even count takes the lower middle; ties by id;
- records sorted by (z14 tile, id), features by (Morton, id); no hash-map order; gzip by `flate2`
  (zlib-rs) at a fixed level, zstd at a fixed level;
- checked as the others were: a dense tile built natively on 1 and 14 threads and as WebAssembly,
  same bytes; the planned "build twice, compare hashes" covers both steps.

## 4. The map

### 4.1 Rendering

**MapLibre's fill-extrusion** (6.11.2), not a layer of our own, in the first phases:
- it handles the globe (its vertex shader projects to the sphere) and the 3D terrain (the centroid's
  elevation, the base sunk 10 m: `get_elevation(a_centroid)` in its shader), tiles and their cache,
  picking (`queryRenderedFeatures` in 3D) and data-driven paint;
- a building costs ~1 KB on the GPU (side quads and roof triangles, 16-byte vertices, the paint
  arrays) and a little less in the worker;
- an opacity under 1 draws twice (depth, then colour), so roads behind show faintly; at 1, once.

**A source and two layers** (`web/src/buildings.ts`): the vector source `bld` (z12–14,
`/tiles/buildings/…?v=`), the layer `buildings` (fill-extrusion: height `h / 10` × the scale, base
`m / 10`, colour by the mode, vertical gradient on) and `buildings-flat` (fill), for the flat mode.

### 4.2 Where in the style

- After the rail layer, before the first symbol layer: `map.addLayer(buildings, 'water-name-line')`
  after `rails` (main.ts). With the other 3D layers: between the draped layers it would split the
  terrain's drape in two (the contours' comment in main.ts).
- `buildings-flat` among the draped layers, after the water and parks, before the boundaries.
- Occlusion, as it follows from MapLibre's passes: the extrusions test and write depth (LEQUAL) in the
  translucent pass, against the terrain's depth. The road and rail layers (custom, 3D) test against
  that depth with their tolerance (1.5 % of the distance, at least 75 m × exaggeration), write none,
  and were drawn before: so the buildings paint over what's behind them and, a road being on the
  terrain, never over a road in front (§1). The dots and labels come after, without a depth test
  against extrusions.
- **B3, bridges and elevated rail:** their pieces (the tiles' bridge flag) drawn again in a small
  custom layer after the buildings, with the same occlusion against the terrain, so a viaduct in
  front of a tower stays in front.

### 4.3 On the terrain and the globe

- The 3D terrain is the z12 repaired Terrarium, 20–38 m pixels; MapLibre samples it at the
  centroid. **B3:** a shader patch (`vite.config.ts`) puts each wall's foot on the terrain under its
  own corner (`get_elevation(a_pos)` for the base vertices) and keeps the roof level at the
  centroid's ground plus the height, as a building on a slope is; the 10 m sink becomes 2 m.
- Exaggeration: MapLibre exaggerates the ground, not the height; the scale setting multiplies the
  height (1–3×, or the terrain's exaggeration).
- The globe: extrusions follow it to the hand-over at 15.5–16.5; the depth precision tuning
  (`camera3d.tuneDepth`) covers them.
- The camera stops 30 m short of the ground (camera3d): inside a tall building. B3 keeps it a few
  metres above the highest roof under it, from the loaded tiles.

### 4.4 Styling

- **Plain** (default): one blue-grey, lighter roofs, lit by `map.setLight` from the hill-shading's
  azimuth, intensity low.
- **By height:** the shared colour scale (Auto / Lock / Full, the colour-map picker, the low-end
  fade) over the buildings in view, as the terrain tint's.
- **By where the height comes from:** measured, floors, neighbours, GHSL, size, so the fill can be
  judged on the map.
- B3: buildings holding a heritage site's point tinted by its tier (the heritage overlay's colours),
  for the landmarks' sake.

### 4.5 Interaction

- Hover: `queryRenderedFeatures` on `buildings` at the pointer, only when no marker, road, rail line
  or area answers there, at most once a frame; the hovered footprint outlined (its geometry from the
  query, drawn in a small GeoJSON layer). The bottom bar's slots as in §1.
- Click: none in B1–B2. Later: **O** opens the OSM way where OSM gave the footprint (its id from the
  work file), served by `/api/building?at=` if wanted.
- The In view summary may add the tallest building in view (from the loaded tiles): B3, optional.

### 4.6 Levels of detail and the iPad

- Detail comes from the tiles (§1): z12 and z13 tiles hold only the tall and large buildings, and a
  tilted view takes them toward the horizon. "Skyline only" filters `h` (MapLibre filters before it
  builds the buckets, so filtered buildings cost no GPU memory).
- **The iPad's budget** (8 GB iPad Pro; Safari gives a tab ~4 GB, the map's roads, terrain and
  basemap take a share): buildings ≤ 300 MB in the densest view, ≤ 8 ms of GPU a frame. Estimate:
  Shinjuku at zoom 16, tilted 60°: up to ~10 z14 tiles of ~11,500 buildings near, coarser tiles
  beyond: ~120 k buildings, ~120 MB on the GPU, ~2 M triangles.
- **Measured in B1** on the iPad (Safari's Web Inspector: memory and frame timeline) at Shinjuku
  (z16, 60°), Manhattan (z15, 70°), Paris (z15), Hong Kong's Mid-Levels on its slope, Monaco, a
  Vermont village and a Japanese mountain town.
- **If over budget**, on touch devices: opacity 1 (one pass, the default there anyway), z14 tiles only
  from zoom 15 ("skyline" between 13 and 15), a smaller tile cache for `bld`.

### 4.7 Later: a layer of our own

A custom WebGL2 layer, as the roads, contours and dots have, only if B1–B3's measurements or
artefacts call for it (B4):
- exact occlusion with the roads: the buildings' depth drawn into a texture the road, rail and dot
  shaders read beside the terrain's, so a road behind a building fades as one behind a hill does
  (today hidden or faint by the opacity), and the roads' tolerance stays for the terrain alone;
- pitched roofs: tagged shapes (0.6 %), and gabled roofs inferred for small residential footprints
  (four corners, a ridge along the long side), which villages would show;
- distance-based detail like the roads' (perspective), and the iPad's memory under our control.

## 5. Phases, risks, questions, decisions

### 5.1 Phases

| Phase | What | Effort |
|---|---|---|
| **B0 Data** | The downloads (under way, §2.6). A measurement script over the files: heights and floors by country, storey heights fitted, the fill's hold-out errors per rule and country (§2.3), tile sizes at z12–14 for the densest tiles; this document's numbers updated. | 1 day |
| **B1 Pilot** | `dem/bldprep.py`, `pipeline::bld` (prep, fill, tiles), `scenic-build bldprep` and `buildings`, run by hand on 6/56/25 (Kantō), 6/32/22 (Paris), 6/18/24 (New York) and a rural tile; formats.md entries; the catalog layer, the server's route; `web/src/buildings.ts` with the settings section, the toggle and hover; the iPad measured (§4.6). | 6 days |
| **B2 In the build** | The agent: keys, targets, the chain's order, prunes, status and forecast labels, shared steps, `bld-fetch` as a job; the mirror's group, the service worker's budget, credits; every tile built and published. plan.md (§6, §8, §9, §10), workers.md, formats.md and the README updated. | 4 days |
| **B3 Sharing and polish** | `bldtile` tasks for pages (WebAssembly, byte-identical); bridges and elevated rail over buildings; walls on the terrain under each corner; fog on the extrusions; the camera's clearance; colour by height, by source, heritage tint. | 5 days |
| **B4 Each on its own measurement** | A custom layer (§4.7); measured heights from BD TOPO (France) and PLATEAU (Japan's cities); building heights in the horizons and the viewshed tool (every unit rebuilt). | 2–3 weeks |

B0–B3: about 16 days of work, the build's own time aside: `bldprep` reads 60 GB from the NAS (an
hour or two over the tiles), `buildings` ~20 minutes of CPU over all tiles natively.

### 5.2 Risks

- **The release leaves S3 on 2026-11-25.** The coverage's files are being kept now. A region added
  later whose files aren't here waits for the next pinned release ("its buildings wait for the next
  Overture release", in the status); pinning one (~6-monthly, plan.md §8 Planned) fetches 62 GB again
  (~7 hours on this line), rebuilds every unit (the roadside index) and every buildings tile. The old
  release's files are deleted by hand afterwards (GC never sweeps `sources/`).
- **Estimated heights:** about half the buildings, 92 % in East Asia. Mitigated by the measured rules
  first, the hold-out checks, the "where the height comes from" colouring, and national data later.
- **Occlusion artefacts** with MapLibre's extrusions: viaducts and elevated rail painted over (B3);
  places where the terrain mesh lies above a road, so a building behind the road shows over it (the
  roads' 75 m × exaggeration tolerance exists for that mesh error).
- **The iPad:** dense views may exceed the budget (§4.6's fallbacks).
- **The NAS:** `bldprep` reads 60 GB; with the units' writes, the NAS is the bottleneck (workers.md
  §1). Paced per tile and kept off the OSM pass.
- **Disk on the build Mac** (17 GB free on 2026-10-05): neither step stages more than a tile's row
  groups or a z8 area locally; the downloads went straight to the NAS.
- **Overture's schema** changes between releases (columns renamed or retyped): `bldprep` checks the
  columns it reads and fails with their names.
- **Overlapping footprints** left by conflation z-fight: B1 counts them; if they show, `bldprep`
  drops the one from the lower-ranked source (OSM first, as Overture ranks them) where two overlap
  by nine-tenths of the smaller.
- **Python's part in a deterministic step:** the scan only decodes, Rust computes (§3.7); the
  double-build check covers it.

### 5.3 Questions for the owner

None blocks the work; each has a default below.
- Buildings on by default? (Default: on, after the iPad's measurements.)
- Heights true by default, or scaled with the terrain's 3× exaggeration? (Default: true.)
- Raise the download's cap from 3 MB/s when the line is idle? (Default: no; it ends overnight.)
- Pitched roofs and exact occlusion (B4): worth a custom layer? (Decided after B3.)

### 5.4 Decisions made

1. **Overture alone** for footprints and attributes, the release the roadside buildings use
   (2026-09-23.1), pinned once for both; OSM, Microsoft and USGS heights through it.
2. **GHSL ANBH** (CC BY 4.0, 1.28 GB) for the fill; Google's 2.5D (an account, little overlap) not
   used; national data later, where it replaces estimates.
3. **The fill's order:** measured, floors, neighbours, GHSL, size and kind, each building saying which;
   thresholds set by hold-out errors, not by hand.
4. **The coverage**, as roads: a building touching it is built; not roadside only, not the world.
5. **Downloads:** the coverage's Overture files whole (60.5 GB of 277) rather than row groups (44.8 GB):
   the source's own bytes, checkable against its ETags, resumable, and a later region needs only
   whole files more; GHSL's tiles meeting the coverage. Two transfers, 3 MB/s, so the build keeps its
   share of the line.
6. **Tiles z12–14,** each building whole in the tile of its centroid (one centroid, no seams, no
   duplicates); z12 and z13 only the tall and large.
7. **Two steps per z6 tile:** `bldprep` (impure: the NAS's parquet) and `buildings` (pure: tasks for
   pages), so the fill and the tiles can change without reading the parquet again.
8. **A chain of its own** that holds no region and no round; tiles in the regions' order.
9. **MapLibre's fill-extrusion** first, after the road and rail layers; a custom layer only if the
   measurements call for it.
10. **True heights** by default, with a scale setting; buildings on by default.
