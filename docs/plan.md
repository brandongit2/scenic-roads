# Plan: regions as modules, data on the NAS

**Version 4**, 2026-10-02: v3 after two Opus review rounds (§11), plus the user's border and
region-size challenges. Re-review v4 before implementing.

- Design only; nothing is built yet. Build only when the user says so.
- Owner: Claude. The user asked me to own the design and to treat their earlier requirements as
  malleable.
- This file is for my own reference. The user follows the diagram artifact.
- The diagram's source is `docs/diagram/`. To rebuild it, run `python3 page.py out.html`, then
  republish `out.html` to https://claude.ai/artifact/PrjaaDXLuuXGpGr2xt5Vjg.

**The idea in one line:** regions are only how data comes in. Everything the map reads is one
worldwide layer per kind, stored by area. Region size and count then don't matter to the map:
borders are seamless, and zoomed-out views read the same few packs whether a country is one region
or fifty.

## 1. The user's surface

**`scenic`** views the map. It:
1. finds the NAS mount with statfs (smbfs from fishandchips, wherever it's mounted), and mounts it
   if it's missing;
2. starts the published app's server if it isn't running;
3. opens the map.

The app's status bar shows build and copy progress.

**`scenic add <place>`** takes a Geofabrik id or name, or any place name. A place name is geocoded,
and the smallest Geofabrik unit containing it is offered, with larger ones as alternatives: "Tokyo
is in Kanto (Japan). Add Kanto (0.6 GB), or all of Japan (2.5 GB)?" It says when the place is
already covered, then writes a request; the build Mac does the rest.

Other commands:
- `scenic remove <place>`: confirms the region's name and size first.
- `scenic keep <place> [radius]`, also "Keep this view" in the app: keeps an area on this Mac for
  trips. Shows the size first; toggles.
- `scenic status`.

The user's only folders are `translations/` and `descriptions/` on the NAS (§7).

Everything else is automatic: building, refreshing (when the build Mac is idle and on power),
publishing, caching, distributing the app, backups. Commands are safe to re-run. Messages say what
happened and what to do, in plain words.

## 2. Two kinds of data

**1. Regions (inputs).** Each region is fetched and processed on its own into **base data**:
- owned ways with their per-point elevations, grade and scenic channels;
- POIs, heritage and details, peaks, designated areas;
- names and their English;
- trains a day per rail way, ferries, stations.

Base data is stored by area: one shard per z6 tile that holds the features' midpoints. A region is
a unit of fetching, processing, ownership and refresh, and nothing else.

**2. Layers (everything the map reads).** One worldwide tile pyramid per layer, in packs:
- z0–9: one pack per z3 tile (≈160 MB);
- z10 and up: one pack per z6 tile.

There are two kinds:
- From **global sources**: terrain, slope, trees, the analysis grids (land cover, canopy, cover),
  the basemap, later buildings.
- From **regions' base data**, built by area steps (§6): roads and rails, landmarks, stations,
  overlays, labels, and the startup and query indexes the server searches with.

Coverage only grows; removing a region leaves global-source tiles and rebuilds the packs around it.
Nothing is merged when serving. There are no per-region products, so there are only two scopes:
PER REGION (base data) and GLOBAL (layers).

## 3. Storage

NAS root: `personal/projects/scenic-roads/`.

```
README.md                    one screen: what's here; the two folders that are yours
translations/  descriptions/  user drop-ins (README, todo/ written by builds)
inputs/                      hand-made or hand-fetched: regions/<id>.toml, clips/, manual/ (MOI DTM, fhd.xlsx), keys.env, timetables/
sources/                     kept downloads: geofabrik/, aws/ (raw packs), overture/, canopy/, registers/, gtfs/ (the planet is never kept)
regions/<id>/                base data: shards per z6 tile, plus region files (manifest, names, todo)
layers/<layer>/              lo/ and hi/ packs + manifests
app/                         published app builds, with the data format versions they read
catalog/                     catalog.<n>.json: each written once with a checksum line; readers take the highest complete one
state/                       requests/<uuid>.json, macs/<host>.json, status.json, lock, logs/
```

**Readable, immutable files.** Every file is named `<logical>.<hash8>.<ext>` and never rewritten.
- Manifests list the current files, with full hashes and per-file format versions.
- An unchanged file keeps its name, so a rebuild uploads only what changed.
- A Mac's copy is current by definition, and a file is never replaced under an mmap.

**Packs, never one file per tile.** SMB manages about 80 random operations per second.

**One writer.** The build Mac's agent writes everything, except:
- `state/requests/`: any Mac creates request files there, exclusively;
- `state/macs/<host>.json`: each Mac writes only its own, giving the catalog it serves;
- the user's two folders.

**GC** keeps every file that any catalog from the last 14 days references, and every file the
catalogs named in `state/macs/` reference. A Mac record unrefreshed for 30 days expires.

**Backups:**
- Btrfs snapshots of the project folder, if the user enables them. They undo deletions and bad
  overwrites, which RAID doesn't.
- The agent mirrors `translations/`, `descriptions/` and `inputs/` to the build Mac, trash-safe.

## 4. Macs

**Build Mac (M4, 48 GB, ~225 GB free after cleanup).**
- `scenic agent` runs under launchd, with openjdk@21, rustup and the mise shims on its PATH. It
  starts through a stable wrapper, so macOS's network-volume permission survives rebuilds.
- Background priority (`taskpolicy -b`); one job at a time.
- Every job's staging is bounded: one region's base data, one area pack (plus its halo), or one
  global-source pack.
- 40 GB stays free for swap. A closed lid stops a job; it resumes on wake. Only this Mac builds.

**App Macs (both laptops).**
- They run the published app (`app/current`), so they need no toolchain.
- The app reads the current and the previous format version of every file kind.
- A development override runs the repo.

**Local cache, per Mac.** The budget is set at first run (default: half the free space; this Mac
will have about 170 GB free once its project data moves to the NAS). Order of precedence:
1. lo packs of every layer: the zoomed-out minimum, about 8–10 GB;
2. kept areas: their packs and base shards;
3. everything else, least recently used first.

Below the minimum, low zooms stream from the NAS.

**Server.**
- Local files are mmapped. NAS files are read by range on a separate I/O pool with timeouts, never
  mmapped; a failure marks the NAS offline (a banner).
- Nothing is loaded at start.
- Views read packs: tiles, plus startup and query indexes per pack (endpoints, drive and ride
  strokes, samples and channels).
- gid lookups (road, profile, details) read the owning region's base shard by range.

**ETags:** each tile's content hash, stored in the pack index. A 304 needs no NAS read, survives a
pack rewrite, and works offline.

## 5. Regions

**Recipe** (`inputs/regions/<id>.toml`): `id`, `name`, `index` (assigned at add, never reused),
`source` (`geofabrik:<id>`, optionally with a `clip`, or `overpass:<area>` for tiny territories).
DEMs, registers, official data, timetable feeds and name reading areas apply by location, from a
module registry.

**Granularity is free.**
- The map is the same whether England is one region or 48 counties. Only the base processing is
  per region, and its fixed cost per region is small (a download, a few files, a minute of
  overhead).
- Today's set stays as it is: 34 regions minus Monaco, Isle of Man, Guernsey-Jersey and Gibraltar,
  which lie inside France's, Britain-and-Ireland's and Spain's extracts, so 30 regions.
- Saint-Pierre-et-Miquelon stays, via Overpass; it lies outside Newfoundland's outline.
- `add` refuses a region inside an existing one.

**Ownership.**
- The owner is the lowest-index region whose outline (Geofabrik's `.poly`) intersects the feature.
- Each region writes only the features it owns, complete (Geofabrik keeps crossing ways and
  multipolygons whole), into the shard of the feature's midpoint.
- Nothing is dropped, and adding a region never changes an earlier one.

**Context for base steps:** the region's own extract. Its buffer and the complete crossing ways are
enough for junction smoothing, bridges, tunnels and ferries.

**IDs:**
- gid = (region index, shard, local index), sent as a compact string.
- Each shard has an index version, bumped only when its base data is rebuilt.
- APIs send the version; the server answers 409 for one it no longer serves, and the client
  refetches.

**Borders and pack edges are seamless:**
- **Global sources:** layers built from them have no borders.
- **Network values:** area steps read every region's base shards within the pack plus a 100 km
  halo. So strokes, whole-road lengths (the length filter) and climbs come out the same on both
  sides of any region border or pack edge. A climb belongs to the pack holding its start.
- **At view time** the server chains drives, rides and whole-road highlights across packs by shared
  OSM nodes, so they are exact however long the road is.
- **When a region publishes,** the packs within its area plus the halo rebuild. That's bounded by
  area, not by how many neighbouring regions there are.
- **The only seam:** where no region exists yet, the data ends, as at the map's edge today.

## 6. Pipeline

### Per region (base data)

Heavy steps (they depend on OSM and rerun on refresh):
1. **fetch:** the Geofabrik extract, plus a clip or Overpass where needed.
2. **extract:** owned ways, POIs, stops.
3. **elevations:** DEMs by location; one DEM cache per region; then clean-up and grade.
4. **heritage, details, peaks:** registers by location; peaks flood into the terrain layer.
5. **scenic:** reads the terrain, grids and canopy layers, and the region's designated areas.
6. **transit:** trains a day per rail way, ferries, stations.
7. **shards:** all of the above, split by z6 tile.

Cheap steps (they rerun when translations or descriptions change):
1. **names:** inventory; English from the thing's own, else a translation; todo lists;
   per-region English tables.

**Validation:** decodes, plausible ranges, summit tolerance for 30 m DEMs (Snowdon reads 1,040 m for
1,085 m), and on a refresh, a drop of more than 20 % in ways flags the region. The previous version
stays live until a new one passes.

### Layers from global sources (pack by pack, bounded staging, rebuilt from raw and never patched)

Coverage is z0–9 for the whole world, and z10 and up for land inside region outlines (a sea mask
skips all-sea tiles). Maximum zoom by latitude keeps pixels at least 15 m: z12 to 67°, z11 to 79°,
z10 beyond. The server builds deeper tiles from their ancestors.

- **Terrain:** raw AWS packs, repaired by today's `repair_terrain` so repairs never compound.
  Values below zero are clamped only where the sea mask says sea. z0–8 from z9 by 2×2 means.
  Depends only on AWS and code.
- **Slope:** Horn at the finest level, quarter means up.
- **Trees:** canopy 10° files and leaf type inside outlines; z4–8 aggregated.
- **Grids (z11):** land cover, canopy and cover near any region's roads. `grid.terrain` goes; the
  analysis reads terrain's z11 tiles.
- **Basemap:** yearly, transient on the M4:
  1. download the planet to the SSD;
  2. `osmium tags-filter` locally (water, waterways, drawn admin levels, parks, places, named
     water);
  3. delete the planet;
  4. Planetiler (pinned jar, sparse node map);
  5. packs.

  Peak use is about 100–160 GB, so it runs alone. Measure the filter ratio first.
- **Buildings (phase 7):** Overture plus official data → MVT z13–14 inside outlines.

### Layers from regions (area steps, one per z6 pack)

Each step reads every region's base shards in the pack plus a 100 km halo, plus the global layers it
needs.
- **Roads and rails:** strokes, whole-road lengths, climbs, RT tiles z9–14, and the pack's
  low-zoom piece (z4–8 lines of the ways whose midpoint it holds).
- **Startup and query indexes:** endpoints for chaining; drive and ride strokes with their samples
  and channels.
- **Landmarks and stations:** per kind; overlays (heritage areas, Indigenous lands, special areas,
  World Heritage outlines). English baked in from the regions' English tables.
- **Lo packs (z0–8):** built from the hi packs' pieces once those are done.

Region-independent rankings, cheap and over all regions:
- **Labels:** importance and min zooms over every region's candidates, plus world places from the
  basemap outside regions → label packs with English.
- **Landmark fame and top-by-kind** for zoomed-out views.
- **Ferries:** a few thousand routes, one small layer.

Area steps and rankings depend on regions' base data by content, and on global-source layers by code
version.

### Served

| What | How |
|---|---|
| every layer | the pack for the tile or the area; nothing merged or appended |
| APIs | by gid plus index version, from base shards |
| drives, rides, whole roads | strokes from the packs in view, chained across packs by shared nodes |
| `/api/catalog` | regions, bounds, credits, versions, progress |

**Browser:**
- One source per layer kind.
- The coast worker reads water from `/tiles/base`.
- Credits for the regions in view.
- A progress indicator.

## 7. Names and descriptions

**One pipeline for every named thing:**
- places, states, seas and lakes, rivers and waterways;
- parks and protected areas, natural features;
- stops and sights, heritage sites and areas, special places, Indigenous lands;
- stations, rail lines, ferries;
- roads (named; route numbers unchanged).

River labels and road names join in phase 7.

**English, in order:** the thing's own (OSM's name:en or name:ja-Latn; for heritage, UNESCO's or
English Wikipedia's title), else a translation, else nothing. It shows only when it truly differs
from the name.

**translations/<scope>.jsonl:**
- Scope is a region id or a reading area (`ja`, `zh-tw`, `zh-hk`, `en-sg`, `latin-fr`, …); the old
  keys `jp`, `tw`, `hk`, `sg` and `latin` are accepted.
- Lines are `{"n", "en"}`; extra fields are ignored. A region uses its own file, then its area's.
- A file is read only after its size and modification time have held for 10 s. An unfinished last
  line is ignored, and so is anything that isn't `*.jsonl`.
- `todo/<region>.jsonl` is regenerated after builds and after translation changes. The NAS copy is
  the master.

**Trigger:** the agent polls both folders (FSEvents can't see changes from other machines, and SMB
listings are cached for 30–60 s). On a change:
1. the affected regions' cheap step reruns;
2. the named layers' packs in those regions rebuild (labels, landmarks, stations, ferries); road
   tiles carry no names;
3. publish.

That takes minutes, with the M4 awake.

**Descriptions** use the same contract: `descriptions/todo/<region>.jsonl` and
`descriptions/<region>.jsonl`. I write them on request with Sonnet writers.

## 8. Building

**Requests:** `add` and `remove` write `state/requests/<uuid>.json`. The agent holds `state/lock`
with a token, and checks it before writing a catalog.

**Fingerprints:** chained (step version, recipe, input fingerprints); tools pinned. Unchanged steps'
files are reused by reference.

**Order of work:**
1. requests;
2. translation and description changes;
3. area packs that region publishes touched (their area plus the halo), then lo packs, then the
   rankings;
4. global-source packs that new coverage needs;
5. stale work after step-version bumps, oldest first, when idle;
6. refreshes, when idle and on power: OSM per region every ~3 months; Overture, registers and
   timetables every ~6; the basemap yearly.

**Format bumps:** publish an app that reads both formats, then the data; drop the old reader once
everything is rebuilt.

**App publishing:** after code changes (pushed to GitHub, or rsynced by me), the agent builds and
publishes the app.

## 9. Migration (phases 1–6 reproduce today's map; features come after)

1. **Foundations:** storage layout, catalogs, content-named files, local cache and budgets, statfs
   mount detection, the I/O pool. `scenic` serves today's `data/build` as a legacy pseudo-layer
   set.
2. **Agent:** requests, status, launcher, app publishing, Mac records. Test the macOS permission
   prompt. Move both Macs' project data to the NAS: ~164 GB here, ~210 GB on the M4 (the user
   approved). Move `keys.env` to `inputs/` (approved). Remove the empty `road-elevations` folder
   (approved).
3. **Layers from global sources:** import today's terrain, slope and trees archives into packs. The
   basemap from the planet, after measuring the filter ratio. Grow coverage in the background.
4. **Regions and area steps:** base shards, gids, area packs, indexes. Pilot on Iceland beside the
   legacy data (no overlap; Iceland stays).
5. **Rankings:** labels, landmark fame and top files, ferries.
6. **Cutover:** build the 30 regions, compare with the legacy build (counts; screenshots of fixed
   views in the bench Chrome), switch, delete the legacy data and `data/build`.
7. **Features,** each on its own:
   - river labels and road names in the translation pipeline;
   - 3D buildings, then PLATEAU;
   - building heights in horizons and the viewshed tool;
   - the new terrain repair;
   - sharper terrain from national DEMs;
   - a regions panel in the app.

## 10. Risks and checks

- **macOS network-volume permission** for the launchd agent: test it in phase 2.
- **Hard SMB mounts:** the I/O pool. **Leftover mount-point folders:** statfs.
- **The basemap job's disk:** filter ratio first; run it alone; sparse node map.
- **Route relations** aren't completed at Geofabrik borders: stop lists there are partial.
- **Version bumps** at globe scale take days: background, oldest first.
- **Area-pack fan-out:** a big region's refresh rebuilds all its packs plus the halo. That's the
  same work as rebuilding its tiles today.
- **Dense packs** (Tokyo plus a 100 km halo) must fit the M4's 48 GB: measure in phase 4. If one
  doesn't, split that pack's job by z7.

## 11. Review resolutions

**v4 (the user's challenges).**
- **Borders:** "fewer borders, fewer cut climbs" was a real gap in v3. Fixed by the 100 km halo and
  view-time chaining.
- **Tiny regions** (e.g., every English county): v3 would still have looked seamless, but:
  - each county would reprocess its neighbours' roads within 100 km (build work grows as regions
    shrink);
  - every addition or refresh would rerun the border work of every region within 100 km;
  - zoomed-out views would open dozens of regions' files and append dozens of road chunks per tile.
- **The fix:** store everything the map reads by area, in layers built from all regions' base
  shards. Regions become input units only. Region size and count no longer affect the map, and
  build work scales with area. This also removes chunk appending, per-region landmark loading and
  border reruns. Today's 30 regions stay as they are; no merging into countries.

**Rounds 1 and 2 (35 items).** All resolved in v3, and carried into v4:
- **Storage and writers:** one worldwide basemap; packs, not one file per tile; the M4 as the only
  writer; numbered catalogs; GC that keeps 14 days of history plus what each Mac serves.
- **Regions:** ownership by intersection, with modules applying by location.
- **Terrain:** one pyramid from AWS only, with a sea-mask clamp and a latitude cap.
- **Server and caching:** per-tile ETags; lazy, pack-based serving; per-Mac budgets and area
  `keep`.
- **Versions:** a format version per file kind, read two at a time; index versions with 409s.
- **Planet:** only passes through the M4.
- **Translations:** the folder rules; descriptions follow the same contract.
- **Process:** features after the cutover; the Iceland pilot; chained fingerprints; validation
  thresholds; backups.
