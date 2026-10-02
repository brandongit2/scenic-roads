# Plan: regions as modules, data on the NAS

**Version 3**, after two Opus review rounds on 2026-10-02 (resolutions in §11).

- Design only; nothing is built yet. Build only when the user says so.
- Owner: Claude. The user asked me to own the design and to treat their earlier requirements as
  malleable.
- This file is for my own reference. The user follows the diagram artifact.
- The diagram's source is `docs/diagram/`. To rebuild it, run `python3 page.py out.html`, then republish `out.html` to
  https://claude.ai/artifact/PrjaaDXLuuXGpGr2xt5Vjg.

## 1. The user's surface

### Commands

**`scenic`** views the map. It:
1. finds the NAS mount with statfs (smbfs from fishandchips, wherever it's mounted), and mounts it
   if it's missing;
2. starts the published app's server if it isn't running;
3. opens the map.

The app's status bar shows build and copy progress.

**`scenic add <place>`** takes a Geofabrik id or name, or any place name. A place name is geocoded,
then the containing country is offered:
- "Tokyo is in Japan. Add Japan (2.5 GB)?"
- If regions inside that country already exist, it offers to replace them: "Add the United States
  (12 GB), replacing us-northeast?"
- Says if the place is already covered.

It then writes a request, and the build Mac does the rest.

**`scenic remove <place>`** confirms the region's name and size first.

**`scenic keep <place> [radius]`** (also "Keep this view" in the app) keeps an area on this Mac for
use away from home. It shows the size first and toggles.

**`scenic status`** shows what's going on.

### Folders and automation

The user may drop files into two NAS folders: `translations/` and `descriptions/` (§7).

Everything else is automatic: building, refreshing (when the build Mac is idle and on power),
publishing, caching, distributing the app, backups. Commands are safe to re-run. Messages say what
happened and what to do, in plain words.

## 2. Three kinds of data

1. **Global layers:** terrain, slope, trees, the analysis grids (land cover, canopy, cover) and the
   basemap.
   - They come from global sources and don't depend on regions.
   - One tile pyramid per layer, in packs:
     - z0–9 in one pack per z3 tile (≈160 MB each);
     - z10 and up in one pack per z6 tile.
   - Coverage only grows; removing a region leaves its tiles.
   - No per-region copies, no ownership, no seams between regions, no merging when serving.
2. **Regions:** what comes from OSM networks and places.
   - Ways, and per-point elevations, grade and scenic channels.
   - Road and rail tiles, packed per z6 tile so an area can be kept.
   - Climbs, POIs, heritage and details, peaks, designated areas, names, transit.
   - The per-kind files the browser loads.
3. **Shared:** built from all regions. Labels, area overlays, zoomed-out landmark files, the
   catalog.

## 3. Storage

NAS root: `personal/projects/scenic-roads/`.

```
README.md                    one screen: what's here; the two folders that are yours
translations/  descriptions/  user drop-ins (README, todo/ written by builds)
inputs/                      hand-made or hand-fetched: regions/<id>.toml, clips/, manual/ (MOI DTM, fhd.xlsx), keys.env, timetables/
sources/                     kept downloads: geofabrik/, aws/ (raw packs), overture/, canopy/, registers/, gtfs/ (the planet is never kept)
global/<layer>/              lo/ and hi/ packs + manifests
regions/<id>/                the region's files + manifests
shared/                      label and overlay packs, landmark top files + manifest
app/                         published app builds with the data format versions they read
catalog/                     catalog.<n>.json, each written once with a checksum line; readers take the highest complete one
state/                       requests/<uuid>.json, macs/<host>.json, status.json, lock, logs/
```

**Readable, immutable files.** Every output is named `<logical>.<hash8>.<ext>` and never rewritten.
- Manifests list the current files, with full hashes and per-file format versions.
- Unchanged files keep their names, so a rebuild uploads only what changed.
- A Mac's copy of a file is current by definition.
- A file is never replaced under an mmap.

**Packs, never one file per tile.** SMB manages about 80 random operations per second.

**One writer.** The build Mac's agent writes everything, except:
- `state/requests/`: other Macs create request files there, exclusively;
- `state/macs/<host>.json`: each Mac writes only its own, giving the catalog it serves;
- the user's two folders.

**GC** keeps:
- every file any catalog of the last 14 days references;
- every file the catalogs named in `state/macs/` reference. A Mac record unrefreshed for 30 days
  expires.

**Backups:**
- Btrfs snapshots of the share: enable once, daily, keep 30.
- The agent mirrors `translations/`, `descriptions/` and `inputs/` to the build Mac. Deleted files
  go to a trash folder.

## 4. Macs

**Build Mac (M4, 48 GB, ~225 GB free after cleanup).**
- `scenic agent` runs under launchd, with openjdk@21, rustup and the mise shims on its PATH. It
  starts through a stable wrapper, so macOS's network-volume permission survives rebuilds.
- Jobs run at background priority (`taskpolicy -b`), one at a time. Each job's staging is bounded:
  one region's network build, or one global pack.
- 40 GB stays free for swap.
- A closed lid stops a job; it resumes on wake.
- Only this Mac builds.

**App Macs (both laptops).**
- The launcher runs `app/current` from the local cache, so app Macs need no toolchain.
- The app reads the current and the previous format version of every file kind. Gradual rebuilds
  (days) never make regions unavailable.
- A development override runs the repo.

**Local cache, per Mac.** The budget is set at first run (default: half the free space) and can be
changed. Order of precedence:
1. startup and query files of regions in use;
2. the low-zoom minimum, about 8 GB: z0–9 packs of terrain, slope, trees and basemap, plus labels
   and overlays;
3. kept areas;
4. everything else, least recently used first.

Below the minimum, low zooms stream from the NAS and `status` says so. The M1 Pro (16 GB, 8.9 GB
free today) will mostly stream at home.

**Server.**
- Local files are mmapped.
- NAS files are read by range on a separate I/O pool with timeouts, never mmapped (a dropped share
  means SIGBUS). A failure marks the NAS offline: a banner shows, and local data keeps working.
- Regions load lazily.

File classes:
- **startup:** small indexes the pipeline builds (way endpoints, drive and rail-line indexes,
  details offsets);
- **query:** samples and scenic channels, for drive and ride searches; ≈2.5 GB for today's set,
  copied with the startup files;
- **on-demand:** per-point arrays for road and profile, details; read by range;
- **tiles:** packs;
- **build-only:** never reaches app Macs.

A Mac keeps serving a region's version N until version N+1's startup and query files are local.

**ETags.** Each tile's ETag is its content hash, stored in the pack index:
- a 304 needs no NAS read;
- unchanged tiles keep their ETag when a pack is rewritten;
- an offline Mac can still answer 304s.

## 5. Regions

**Recipe** (`inputs/regions/<id>.toml`): `id`, `name`, `index` (assigned at add, never reused) and
`source`. `source` is `geofabrik:<id>`, optionally with a `clip`.

The following apply by location, from a module registry with coverage areas, not from the recipe:
- DEMs;
- heritage registers;
- official data;
- timetable feeds;
- name reading areas.

**One region per country by default.**
- Fewer borders, so fewer cut drives, climbs and roads.
- The M4 builds today's whole set (21 GB of PBF) in one run, and the US is 12 GB.
- Adding a country replaces sub-country regions inside it.

**Today's set becomes 11 regions:**
- canada: all provinces and territories, plus Saint-Pierre-et-Miquelon, which is inside canada.poly;
- us-northeast: the closest match to today's 7 states (it adds NJ and PA);
- france: includes Monaco;
- spain: includes Gibraltar;
- britain-and-ireland: includes the Isle of Man and Guernsey-Jersey;
- portugal, andorra, japan, taiwan, hong-kong;
- singapore: clipped from malaysia-singapore-brunei.

**Ownership** applies only to regional vector features.
- The owner is the lowest-index region whose outline (Geofabrik's `.poly`) intersects the feature.
- Derived features (climbs, rail lines, strokes) belong to the owner of their first way. A climb that
  runs past that region's extract is cut short; drives and whole roads join across regions at serve
  time.
- Nothing is dropped.
- Adding a region never changes an earlier one. Removing one queues the regions whose ownership
  grows.

**Context.** A region computes over its whole extract, then writes only what it owns.
- Geofabrik keeps crossing ways and multipolygons complete; route relations are not completed.
- So junction smoothing, bridges, tunnels and ferries, whole roads and rail matching all see the
  neighbours' ways inside the overlap.

**IDs:**
- `gid = region index << 32 | local index`.
- Road-tile chunks and landmark files carry the region's index version. It is bumped only when heavy
  steps rerun.
- APIs send it; the server answers 409 for a version it no longer serves, and the client refetches
  that region.

## 6. Pipeline

### Global layers

The agent builds these pack by pack, with bounded staging:
1. fetch the raw tiles for one z6 tile;
2. repair;
3. compute slope;
4. write the pack;
5. upload it;
6. delete the local copy.

Packs are always rebuilt from raw tiles plus the current coverage and never patched, so the result
doesn't depend on the order regions were added. Lo packs are rebuilt after the hi packs under them.

**Coverage:**
- z0–9 for the whole world.
- z10 and up for land tiles inside region outlines (a sea mask skips all-sea tiles).
- Maximum zoom by latitude, keeping pixels at least 15 m, near the best sources there:
  - z12 up to 67°N/S;
  - z11 up to 79°;
  - z10 beyond.

  The server builds deeper tiles from their ancestors, as it does today.

**Terrain:**
- Raw AWS tiles are kept as raw packs.
- Today's `repair_terrain` runs on the raw tiles, so repairs never compound.
- Values below zero are clamped only where the sea mask says it's sea, which keeps the Dead Sea,
  Death Valley and polders.
- z0–8 come from z9 by 2×2 means.
- Depends only on AWS and code.

**Slope:** Horn's method at the finest level present, then quarter means upward. Slopes step at the
edge of the finest coverage, as they do today.

**Trees:** from the canopy 10° files and leaf type, z9 up to the cap inside outlines; z4–8
aggregated.

**Analysis grids (z11):**
- Land cover, canopy and cover, within 14 km of any region's roads.
- `grid.terrain` goes: scenic and the viewshed tool read terrain's z11 tiles, decoded and cached.

**Basemap**, yearly, transient on the M4:
1. download the planet to the SSD;
2. `osmium tags-filter` locally: water, waterways, the admin boundary levels the style draws, parks
   and protected areas, places, named water;
3. delete the planet;
4. Planetiler, pinned jar, sparse node map, on the filtered PBF;
5. packs.

Peak use is about 100–160 GB, so it runs alone. Before phase 3, measure the filter's output ratio
on today's merged.osm.pbf.

The place and water_name features are label candidates outside regions; the browser draws no text
from the basemap.

**Buildings** (phase 7): Overture plus official data → MVT z13–14 inside outlines, through
Planetiler's YAML profile reading GeoParquet after a height pass.

**Dependencies.** Region steps that read global layers (scenic, drape heights, peaks) depend on
each layer's code version, not its content. A neighbour's border tiles shifting slightly is caught
up at the next refresh.

### Per region

**Heavy steps** (they depend on OSM and rerun on refresh):
1. extract (context vs owned);
2. elevations (DEMs by location; one DEM cache file per region);
3. elevation clean-up;
4. heritage, details and peaks;
5. scenic;
6. tile: road and rail z4–14 as RT v7 length-prefixed chunks with region index and index version,
   packed per z6 tile, plus the startup indexes;
7. transit.

**Cheap steps** (they rerun when translations or descriptions change):
1. names: inventory; English from the thing's own, else a translation; todo lists;
2. named outputs, all carrying English: landmark files per kind, stations, ferries, road and
   rail-line name tables, label and overlay candidates.

**Validation:**
- files decode;
- values fall in plausible ranges;
- summits pass a tolerance that allows for 30 m DEMs (Snowdon reads 1,040 m for 1,085 m);
- on a refresh, a drop of more than 20 % in ways flags the region.

The previous version stays live until a new one passes.

### Shared

Rebuilt after any region publishes; only the packs it touches.
- **Labels:** every region's candidates, plus world places from the basemap outside regions.
- **Overlays:** heritage areas, Indigenous lands, special areas and World Heritage outlines, tiled
  into one vector source.
- **Landmark top files,** per kind.

### Served

| What | How |
|---|---|
| global layers | the layer's pack for the tile |
| roads, rails | chunks from each region whose tile index lists the tile, appended. The worker merges lines minor→major and sorts by (region, way) |
| labels, overlays | shared packs |
| landmarks (per kind) | the top file below z7; region files for regions in view from z7 (worker) |
| stations, ferries | per region via the worker; one source per kind |
| APIs | by gid plus index version. Drives and whole roads join across regions by endpoint coordinates |
| `/api/catalog` | regions, bounds, credits, versions, progress |

**Browser:**
- One source per layer kind, never per region.
- The coast worker reads water from `/tiles/base`.
- Credits for regions in view.
- A progress indicator.

## 7. Names and descriptions

**One pipeline for every named thing:**
- places, states, seas and lakes, rivers and other waterways;
- parks and protected areas, natural features;
- stops and sights, heritage sites and areas, special places, Indigenous lands;
- stations, rail lines, ferry routes and terminals;
- roads (named; route numbers unchanged).

River labels and road names join in phase 7.

**English, in order:**
1. the thing's own: OSM's name:en or name:ja-Latn; for heritage sites, UNESCO's or English
   Wikipedia's title;
2. else a translation;
3. else nothing.

It shows only when it truly differs from the name.

**translations/<scope>.jsonl:**
- Scope is a region id or a reading area: `ja`, `zh-tw`, `zh-hk`, `en-sg`, `latin-fr`, … The old
  keys `jp`, `tw`, `hk`, `sg` and `latin` are accepted.
- Lines are `{"n", "en"}`; extra fields are ignored.
- A region uses its own file, then its area's.
- A file is read only after its size and modification time have held for 10 s. An unfinished last
  line is ignored, and so is anything that isn't `*.jsonl`.
- `todo/<region>.jsonl` is regenerated after builds and after translation changes.
- The NAS copy is the master copy.

**Trigger:**
- The agent polls both folders: FSEvents can't see changes made on other machines, and SMB listings
  are cached for 30–60 s.
- On a change it reruns the cheap steps for the affected regions, then the shared packs they touch,
  then publishes.
- That takes minutes, with the M4 awake.

**Descriptions** use the same contract: `descriptions/todo/<region>.jsonl` and
`descriptions/<region>.jsonl`. I write them on request with Sonnet writers.

## 8. Building

**Requests.** `add` and `remove` write `state/requests/<uuid>.json`. The agent holds `state/lock`
with a token, and checks the token before writing a catalog.

**Fingerprints** are chained: step version, recipe, and input fingerprints. Global layers count by
code version. Tools are pinned (the Planetiler jar, osmium). Unchanged steps' files are reused by
reference.

**Order of work:**
1. requests;
2. translation and description changes;
3. global packs that new coverage needs;
4. stale regions after step-version bumps, oldest first, when idle;
5. refreshes, when idle and on power:
   - OSM, per region every ~3 months;
   - Overture, registers and timetables every ~6 months;
   - the basemap yearly.

**Format bumps:** publish the app that reads both formats first, then data; drop the old reader once
every file is rebuilt.

**App publishing:** after code changes (pushed to GitHub, or rsynced by me), the agent builds and
publishes the app.

## 9. Migration (phases 1–6 reproduce today's map; features come after)

1. **Foundations:**
   - storage layout, catalogs, content-named files;
   - local cache and budgets;
   - statfs mount detection, the I/O pool;
   - `scenic` serving today's `data/build` as a legacy pseudo-region (no ownership).
2. **Agent:**
   - requests, status, launcher, app publishing, Mac records;
   - test the macOS permission prompt.
3. **Global layers:**
   - import today's terrain, slope and trees archives into packs (hours);
   - the basemap from the planet (after measuring the filter ratio);
   - serve them beside the legacy region;
   - grow coverage to full outlines in the background.
4. **Region pipeline:**
   - gids and index versions, chunked road packs, startup and query files, lazy loading;
   - pilot on Iceland beside the legacy region (no overlap).
5. **Shared:** labels, overlays, landmark top files.
6. **Cutover:**
   - build the 11 regions;
   - compare with the legacy build: counts, and screenshots of fixed views in the bench Chrome;
   - switch all at once;
   - delete the legacy region and `data/build`.
7. **Features**, each on its own:
   - river labels and road names in the translation pipeline;
   - 3D buildings, then official 3D data (PLATEAU);
   - building heights in horizons and the viewshed tool;
   - the new terrain repair (its own project, with tests for ridges and cliffs, and a cap on blob
     size);
   - sharper terrain from national DEMs;
   - a regions panel in the app.

## 10. Risks and checks

- **macOS network-volume permission** for the launchd agent: test it in phase 2 (stable wrapper).
- **Hard SMB mounts:** the I/O pool with timeouts. **Leftover mount-point folders:** statfs.
- **The basemap job's disk use:** measure the filter ratio first, run it alone, use a sparse node
  map.
- **Route relations** aren't completed at Geofabrik borders: stop lists there are partial.
- **Version bumps** at globe scale take days of M4 time: they run in the background, oldest first.
- **Canada at full coverage** is the biggest global-layer job. Packs keep its staging bounded; the
  latitude cap halves it.

## 11. Review resolutions

### Round 1 (27 items)

| # | Challenge | Resolution |
|---|---|---|
| 1 | Per-region basemaps composed in the browser | One worldwide basemap from the planet, in packs |
| 2 | Only tiles can come from the NAS; startup walks every way | Pipeline-built startup indexes, file classes, lazy regions, version switch after startup and query files |
| 3 | Owned-only ways break network computations at borders | Compute over the whole extract, write owned only; modules by location |
| 4 | Ownership drops or misfiles features; regions inside others | Owner = lowest index whose outline intersects; Monaco, Gibraltar, Isle of Man, Guernsey-Jersey and SPM folded in |
| 5 | Several writers on SMB | The M4 agent is the only writer; request files and Mac records; lock token |
| 6 | Splitting Japan; borders | One region per country by default; joins by endpoint at serve time |
| 7 | World terrain ≠ regions'; OSM in repair | One global pyramid from AWS only; new repair moved to phase 7 |
| 8 | Rasters owned by tile centre | Global packed layers |
| 9 | Immutable caching vs incremental publishes | Per-tile content-hash ETags |
| 10 | Cache policy on the M1 Pro | Budgets, precedence, the minimum, area `keep` |
| 11 | One file per tile | Packs |
| 12 | Shared products rewritten whole | Packs by area; roads appended in the server at every zoom |
| 13 | Area overlays missing | Tiled overlays; stations and ferries via the worker |
| 14 | Mounting and drops | statfs, I/O pool, PATH, wrapper |
| 15 | Translations folder details | Stable-file rule, scopes without ':', old keys, todo regeneration, polling |
| 16 | Features mixed into the migration | Phase 7 |
| 17 | Legacy pseudo-region owns everything | No ownership for legacy; Iceland pilot; switch all at once |
| 18 | Descriptions | Same contract as translations |
| 19 | Code and data drift | Published app; two format versions read |
| 20 | Determinism | Chained fingerprints, pinned tools |
| 21 | Validation | Absolute checks, DEM summit tolerance, refresh comparison only |
| 22 | Catalog atomicity | Numbered catalogs with checksums |
| 23 | Road tile chunks | Length-prefixed chunks, merge and sort, tile index |
| 24 | Seams; clamping | Slope step accepted; sea-mask clamp |
| 25 | Version bumps | Background, oldest first, background priority |
| 26 | Backups; manual inputs | Snapshots plus a trash-safe mirror; `inputs/manual`, `keys.env` |
| 27 | Landmarks per kind | Per-kind files per region; top files below z7 |

### Round 2

| Item | Resolution |
|---|---|
| N1 Global layers too big to stage with a region (Canada ≈ 1.2 M z12 tiles) | Pack-by-pack jobs, z0–9 per z3, rebuild from raw, latitude cap, import first |
| N2 Fingerprint churn from global packs | Depend on global layers by code version |
| N3 GC retention | 14 days plus the catalogs Macs are serving |
| N4 gid and version mismatch | Index version in chunks and files; 409 then refetch |
| N5 Format versions during gradual rebuilds | Per file kind; the app reads current and previous |
| N6 Planet pipeline re-reads over SMB | Transient on the M4: filter locally, sparse node map, measure the ratio |
| N7 Surface | Area `keep`; country rule with replace; `remove` confirms |
| N8 Query files on streaming Macs | A query file class |
| #2 remainder, ETag detail, SPM, ownership simplification, `grid.terrain` duplicate | Done as above |
