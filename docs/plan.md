# Plan: regions as modules, data on the NAS

**Version 5**, 2026-10-02: rebuilt after the third Opus review (§12), plus resilience to the build
Mac being asleep, away or unplugged (§8, Interruptions). Re-review v5 before implementing.

- Design only; nothing is built yet. Build only when the user says so.
- Owner: Claude. The user asked me to own the design and to treat their earlier requirements as
  malleable.
- This file is for my own reference. The user follows the diagram artifact.
- The diagram's source is `docs/diagram/`. To rebuild it, run `python3 page.py out.html`, then
  republish `out.html` to https://claude.ai/artifact/PrjaaDXLuuXGpGr2xt5Vjg.

**The idea in one line:** OpenStreetMap comes from one worldwide download, cut by area, and a region
is only an outline saying which areas to build. Every step runs either per area (a z6 tile) or once
for the whole world. So region size, count and borders never show on the map, and nothing in
storage or serving knows about regions.

## 1. The user's surface

**The map is always there.**
- A small launcher is a login item on both Macs and keeps the server running. The server loads
  nothing until asked, so it costs nothing while idle.
- The user opens a bookmark (`http://localhost:8080`), or runs `scenic`. `scenic` also mounts the
  NAS if it isn't mounted.

**The Regions panel** in the app. `scenic add`, `scenic remove` and `scenic status` do the same from
a terminal.
- **Search** for a place. The search runs offline, on the map's own worldwide place list.
- **Choose a unit.** The panel offers the smallest Geofabrik unit containing the place, and the
  larger ones, each with its cost: "Kanto: ~3 GB, ~2 h of building. All of Japan: ~12 GB, ~8 h."
- **Add or remove.** Removing confirms first. Adding a unit that contains existing ones folds them
  in.
- **See it on the map.** Coverage is drawn on the map, and progress shows in the status bar. New
  areas appear as they finish.
- **Edit a region:** rename it, split it into smaller units, merge it with others, or change its
  outline (pick other units, or drag the outline's points on the map). Renaming, splitting and
  merging rebuild nothing; changing what's covered rebuilds only the areas that changed.
- **Keep this view** keeps an area on this Mac for trips. It shows the size first, and toggles.

**Translations:** the user drops `.jsonl` files into `translations/<area>/` on the NAS (`ja/`,
`zh-tw/`, …). Both Macs show them within a minute, with nothing rebuilt. Names that still lack
English are listed in `translations/todo/`. Descriptions work the same way (§7).

**Everything else is automatic:** building, refreshing (whenever the build Mac is plugged in),
publishing, caching, distributing the app, backups. Commands are safe to re-run. Messages say what
happened and what to do, in plain words.

## 2. How the work is divided

A **z6 tile** is about 600 km across at the equator and 300 km at 60°. All work happens on the build
Mac, in four kinds of job:

1. **The OSM pass** runs twice a year, or when asked. The NAS fetches the planet; the build Mac
   processes it whenever it's next plugged in at home, and leaves behind:
   - **pieces:** filtered OSM per z6 tile, each with a 10 km buffer, for all land;
   - **worldwide sets:** rail (tracks, stations, routes), ferries, designated areas (complete),
     places, and country outlines;
   - **the basemap's input.**
2. **Per area.** For each z6 tile T that meets the coverage:
   - **base(T):** the features T owns, with per-point elevations, grade and scenic channels. A way
     belongs to the tile of its first node, so each value is computed once. Context comes from T's
     piece.
   - **pack(T):** the tiles and query parts for T. It reads the base data of T and of every tile
     whose features reach within 100 km of T, plus T's slices of the worldwide results.
3. **Worldwide:** whole roads, rail service, ferries, landmark fame, labels and names. These read
   compact per-tile outputs or the worldwide sets, and write results sliced per tile.
4. **Global-source layers:** terrain, slope, trees, grids, the basemap, overlays and, later,
   buildings. They're built pack by pack from their sources.

Region outlines only decide which tiles and features get built.

## 3. Storage

NAS root: `personal/projects/scenic-roads/`.

```
README.md                     one screen: what's here; the two folders that are yours
translations/  descriptions/  the user's drop-ins (README; todo/ written by the agent)
inputs/                       regions/<id>.toml, outlines/, manual/ (MOI DTM, fhd.xlsx), keys.env, timetables/
sources/                      osm/<date>/ (the planet, pieces/, sets/), aws/, canopy/, overture/, registers/, gtfs/, pageviews/, basemap/
base/                         base packs, one per z6 tile
global/                       the worldwide steps' outputs, sliced per tile; names tables
layers/<layer>/               root, lo and hi packs
app/                          published app builds; current.json
catalog/                      <n>.json.zst: every current file, with its hash and format version
state/                        requests/, status.json, lock, logs/, backups/
```

**Immutable, content-named files.** Every file is named `<logical>.<hash8>.<ext>` and never
rewritten.
- An unchanged file keeps its name, so a rebuild uploads only what changed.
- A Mac's copy of a file is current by definition.
- The catalog is the only index. It's one compressed file per publish, about 1–3 MB even at globe
  scale.

**Packs, never one file per tile.** SMB manages about 80 random reads per second per file.
- Each pack starts with an index: for each tile, its offset, length and content hash.
- Identical tiles are stored once, which covers most sea tiles. No tile is skipped, because the
  vector basemap draws the sea as a polygon.
- **Root pack:** z0–2.
- **Lo packs:** z3–8, one per z3 tile.
- **Hi packs:** z9–14, one per z6 tile.
- Base packs are indexed by z9 sub-tile, so the 100 km halo is a few range reads.

**One writer.** The build Mac's agent writes everything, except two things:
- request files, which any Mac creates exclusively;
- the user's folders.

**GC** keeps every file that any catalog from the last 14 days references.

**Backups:**
- When the user's folders or `inputs/` change, the agent keeps a dated copy for 30 days under
  `state/backups/`, mirrored to the build Mac.
- Everything else can be regenerated.
- Don't snapshot the project's share: snapshots would keep every replaced pack alive. If the user
  wants snapshots of `personal`, the regenerable folders move to a share of their own first.

## 4. Macs

**Build Mac (M4, 48 GB, ~225 GB free once the legacy data is gone).**
- `scenic agent` runs under the launcher, with openjdk@21, rustup and the mise shims on its PATH.
- **Priority:** jobs run at `taskpolicy -c utility`. `-b` would confine them to the 4 efficiency
  cores, about 17× slower (measured).
  - Jobs use half the cores while the user is active on the M4, and all of them when it's idle.
  - Each job holds `caffeinate -s -w <pid>`. That stops idle sleep on power only, so a job paused
    on battery never keeps the Mac awake.
  - One job runs at a time.
- **It may be asleep, away or unplugged at any time.** Work waits and resumes; see §8,
  Interruptions.
- **Staging is bounded per job:** the OSM pass (~150 GB peak; it runs alone), one base(T), one
  pack(T) with its halo, or one global-source pack.
- 40 GB stays free for swap.
- Its own map cache gets 20 GB, evicted before the OSM pass and the basemap.

**App Macs (both laptops).**
- **The launcher** is built once and never rebuilt.
  - It copies the published app to local disk, runs it, and restarts it when a newer app is
    published and the map is idle.
  - A stable launcher keeps macOS's network-volume permission across app updates.
  - If the phase 1 test shows it doesn't, sign the agent and the server with a self-signed
    identity. That's one Keychain step for the user.
- The app reads the current and the previous format version of every file kind.
- A development override runs the repo.

**Server.**
- **Finding the NAS:** it uses `getfsstat(MNT_NOWAIT)`, never `statfs`, which can hang on a dead
  mount.
- **NAS access:** every operation (open, stat, list, read) goes through an I/O pool with timeouts.
  NAS files are never mmapped.
- **Offline start:** it starts without the NAS. The last catalog and the index of every pack it has
  opened stay local, and a banner says it's offline.
- Nothing is loaded at start.
- **ETags:** each tile's content hash, combined with the translations version for tiles with names.
  A 304 never needs the NAS.

**Local cache, per Mac.**
- The unit is the whole pack. After a pack's first use, it's copied in the background with one
  sequential read (46 MB/s, against about 4 MB/s for random reads). Until then, reads are by range.
- The budget is set at first run. The default is half the free space; this Mac will have about
  170 GB free after the cutover.
- Precedence:
  1. the root and lo packs: the zoomed-out minimum, about 5–10 GB;
  2. kept areas;
  3. everything else, least recently used first.

## 5. Coverage and regions

**Recipe** (`inputs/regions/<id>.toml`): `id`, `name`, and `outline`. The outline is a list whose
union is the region; each entry is one of:
- `geofabrik:<id>`;
- a `.poly` file in `inputs/outlines/` (the panel writes one when an outline is drawn or edited);
- a place and a radius.

**Coverage** is the union of the outlines.
- A feature is built if it touches the coverage: for a way, any node inside, as Geofabrik does.
- Ways are kept whole, so roads end a little past the edge, as today.

**Granularity is free.** Tiles, not regions, are the unit of work, and the union of outlines has no
inner borders. England as 48 county outlines builds exactly what one England outline builds.

**Redefining regions is a recipe edit.** Splitting, merging, renaming or redrawing a region changes
only `inputs/regions/` (and `inputs/outlines/`).
- Builds depend on coverage, never on regions. base(T)'s fingerprint holds T's selection (the
  sorted ids of the features it builds), not the outlines' shapes.
- So a change that covers the same ground rebuilds nothing.
- A change that grows or shrinks coverage rebuilds only the tiles whose selection changed, the packs
  within 100 km of them, and the worldwide slices that follow.
- Shrinking leaves global-source tiles (terrain, slope, trees, grids) in place; they're harmless.
- Merging makes one recipe whose outline lists the parts; splitting makes one recipe per part.

**Today's set:**
- 34 regions become 30 outlines. Monaco, Isle of Man, Guernsey-Jersey and Gibraltar lie inside
  France's, Britain-and-Ireland's and Spain's outlines.
- Saint-Pierre-et-Miquelon is an ordinary outline; no Overpass.

**Modules apply by location,** found from the country outlines. They cover:
- DEM order;
- heritage registers;
- timetable feeds;
- road network codes and their colours;
- reading areas;
- credits.

A country without a module gets defaults: FABDEM, no register, no timetables, colours by road class.
`status` says which defaults each region uses.

**No seams:**
- **Region borders:** no step knows about them.
- **Tile edges:**
  - per-way values are computed once, by the owning tile;
  - network values come from worldwide steps (road ids, lengths and offsets; rail service;
    ferries; designated areas) or from the 100 km halo (climbs);
  - queries join road parts by their offset along the road.
- **Where coverage ends,** roads end, as today.

**IDs:** OSM type and id.
- Way ids fit in u32 for decades, so road tiles keep their u32 column. The tile format is
  versioned for when they don't.
- Details and profiles are asked for by id plus location, which picks the pack.
- Links survive rebuilds.

## 6. Pipeline

### The OSM pass (twice a year)

1. **The NAS fetches the planet,** so the laptop's whereabouts never matter.
   - A scheduled task on the NAS (DSM's Task Scheduler, set up once) runs a small script daily
     with `curl`. The script lives in the project folder, so updating it needs no DSM changes.
   - When the newest planet on the NAS is six months old, it downloads the newest dated planet
     (~90 GB) from a mirror. It resumes by byte range after any interruption, verifies the MD5,
     then renames it into `sources/osm/<date>/`.
   - The NAS is always on and wired, and it does nothing heavier than this.
2. The M4 copies the planet to its SSD when it's next at home and plugged in. The copy is
   resumable and takes about 30 minutes.
3. Run `osmium tags-filter` three times:
   - (a) the tags the pipeline reads (no buildings, addresses or land use);
   - (b) the basemap's tags;
   - (c) the worldwide sets, with relations complete.
4. Delete the local copy. The NAS keeps the planet until the next pass finishes.
5. Cut (a) into z3 pieces, then each z3 piece into z6 pieces. The cut keeps ways whole, completes
   multipolygons and adds a 10 km buffer. Upload each piece, then delete it locally.
6. Run Planetiler on (b), with its jar and extras pinned. The extras are Natural Earth and the OSM
   water polygons, kept in `sources/basemap/`.

Measure the filter ratios before the first pass.

Because the sets are complete worldwide, rail routing, ferries and designated areas don't stop at
the coverage edge.

### Global-source layers (pack by pack, rebuilt from raw, never patched)

**Coverage:**
- z0–8 for the whole world;
- z9 and finer within the coverage plus 20 km (viewsheds see 15 km);
- maximum zoom by latitude, so pixels stay at least 15 m: z12 to 67°, z11 to 79°, z10 beyond.

The server builds missing deeper tiles from their ancestors.

**Layers:**
- **Terrain:** raw AWS packs, repaired by today's `repair_terrain`, so repairs never compound.
  - Values below zero are clamped only where the sea mask says sea. The mask is the pass's OSM
    water polygons, and it's in the fingerprint.
  - z0–8 come from z9 by 2×2 means.
- **Slope:** z11 and coarser are stored, made from transient z12 slope (Horn). The server makes
  z12 on demand with the same encoder, plus an LRU. That saves 56 % of today's archive.
- **Trees:** canopy 10° files and leaf type, within the coverage plus 20 km; z4–8 aggregated.
- **Grids (z11):** land cover, canopy and cover within the coverage plus 20 km. `grid.terrain`
  goes; the analysis reads terrain's z11 tiles.
- **Basemap:** packs from Planetiler's output, worldwide.
- **Overlays:** heritage areas, Indigenous lands, special areas and World Heritage outlines, taken
  from the sets plus the registers. Each is assembled once, simplified per zoom and clipped per
  pack, so a 2,000 km outline is whole everywhere.
- **Buildings (phase 7):** Overture plus official data → MVT z13–14 within the coverage.

### Per area

**base(T):**
1. **extract:** the features T owns that touch the coverage. Rail ways are taken when the rail set
   puts them on a passenger route.
2. **elevations:** DEMs by location.
   - The previous base(T) is the cache: unchanged vertices keep their values.
   - Today's per-vertex DEM cache seeds the first builds, so the cutover doesn't fetch every DEM
     block again.
3. **clean-up and grade,** with junction context from the piece.
4. **scenic:** reads terrain, grids, canopy and designated areas. Unchanged samples keep their
   values.
5. **POIs, heritage, details, peaks.** Peaks read terrain.
6. **names inventory.**
7. **outputs:**
   - the base pack;
   - compact outputs for the worldwide steps: junction pairings, way lengths, rail ways, landmark
     candidates, names.

**pack(T):**
- **Reads:** the base data of T and of every tile whose features reach within 100 km of T (known
  from each base pack's extent), plus T's worldwide slices.
- **Writes:**
  - road and rail tiles for z6–14. z6–8 lie inside T, so they go to its z3 lo pack.
  - climbs that start in T;
  - query parts: each road part's samples and channels, with its offset along the road;
  - landmark and station tiles;
  - T's z4–5 contribution.

**Lo packs** are built per z3 tile from their tiles' outputs. They hold roads z3–8, each kind's top
landmarks, per-kind landmark counts, and 1 km query summaries.

### Worldwide steps

- **Whole roads.** At each junction, the pairing of ways is a local decision: same ref, else same
  name, else same class; straightest first; oneways in their direction.
  - base(T) computes the pairings for nodes inside T. They're exact, because a piece holds every way
    through those nodes.
  - A union-find over all pairings gives each way its road id, the road's length, and the way's
    offset and direction along it.
  - Outputs: slices per tile, and an index from road to tiles.
  - The input is ~30 bytes per way: 0.75 GB today, about 7.5 GB for the world.
- **Rail.**
  - Trains a day per rail way, routing stop pairs on the worldwide track graph.
  - Line identities and offsets; stations.
  - Timetables are processed once per feed version.
- **Ferries:** one small layer. Ferries are drawn from it, not from road tiles.
- **Landmark fame, and each kind's top landmarks:** one worldwide pageview table per article and
  season, for all languages.
- **Labels:** places from the sets plus candidates from base data, ranked → label packs.
- **Names:** per reading area, the own-English table (each name's most common own English) and
  todo lists.

**Slices:** each step writes its outputs per tile, and pack(T) depends on its slices' hashes. A
change rebuilds only the packs whose values changed, e.g. along a highway whose length grew.

### Served

| What | How |
|---|---|
| tiles | the pack for the tile; English attached to named features |
| drives, rides, whole roads | parts from the packs in view, joined by offset along the road |
| the same, zoomed out (more than ~6 hi packs in view) | 1 km summaries from lo packs |
| details, profiles | by OSM id plus location |
| `/api/catalog` | coverage, credits, versions, progress |

**Browser:**
- The same tile URLs as today.
- Landmarks, stations and overlays load by view.
- English comes from the server.
- It switches to a new catalog in place.

## 7. Names and descriptions

**One pipeline for every named thing:**
- places, states, seas and lakes, rivers and waterways;
- parks and protected areas, natural features;
- stops and sights, heritage sites and areas, special places, Indigenous lands;
- stations, rail lines, ferries;
- roads (named ones; route numbers are unchanged).

The basemap's names get English too, so rivers have it before river labels exist.

**English, in order:**
1. the thing's own: OSM's `name:en` or `name:ja-Latn`; for heritage, UNESCO's or English
   Wikipedia's title;
2. else a translation;
3. else the own-English table;
4. else nothing.

It shows only when it truly differs from the name.

**Applied when serving.**
- The server reads the user's folders directly. It polls every minute, compiles them locally, and
  attaches English to every tile and record it serves.
- So a drop shows within a minute on either Mac, whether or not the M4 is awake, and nothing is
  rebuilt.
- Label and basemap tiles are rewritten on the fly, about 1 ms each, and cached.

**Files:** `translations/<area>/*.jsonl`, with any names and any number of files.
- Lines are `{"n", "en"}`. Extra fields, like the translators' `via`, are ignored.
- Later file names win.
- A file is read once its size and modification time have held for 10 s. An unfinished last line
  is ignored.
- Areas: `ja`, `zh-tw`, `zh-hk`, `en-sg`, `latin-…`. The old keys `jp`, `tw`, `hk`, `sg` and
  `latin` are accepted as folder names.
- Reading areas come from the country outlines; there are no hard-coded boxes.

**Todo:** `translations/todo/<area>.jsonl`, with priority metrics. The agent regenerates it after
builds and drops.

**Descriptions:** `descriptions/*.jsonl`, keyed by Wikidata QID, else by OSM type and id. Same rules,
and `descriptions/todo/`. I write them on request with Sonnet writers.

## 8. Building

**Requests:** `add` and `remove` write `state/requests/<uuid>.json`. The agent holds `state/lock`
with a token, and checks it before writing a catalog.

**Scheduler:**
- Every job is one step for one tile, or one worldwide step.
- Its fingerprint chains the step's version, the recipe and the hashes of its inputs. Tools are
  pinned.
- Unchanged jobs reuse their outputs by name.

**Order:**
1. requests;
2. the OSM pass, when the NAS holds a newer planet than the pieces come from;
3. global-source packs for new coverage (plus 20 km);
4. base(T) for changed tiles;
5. the worldwide steps;
6. pack(T) for changed tiles and changed slices;
7. lo packs;
8. publish a catalog. This happens every ~30 minutes of work and at the end, so new areas appear as
   they finish.

Then, when idle:
- stale work after step-version bumps, oldest first;
- refreshes:
  - the OSM pass, whenever the NAS has fetched a new planet. Every tile rebuilds, but reuse makes
    that cheap.
  - Overture, registers and timetables every ~6 months.

**Interruptions.** The build Mac may be asleep, away from home or unplugged at any time, and may
close its lid mid-job. Nothing depends on it being available at a given time.
- **No deadlines.** Refreshes only become due. Until the work is done, the map serves the last
  catalog, and requests queue on the NAS.
- **Conditions per step.** CPU work needs power. Steps that touch the NAS need the NAS. Local steps
  (filtering or cutting a planet already copied) carry on away from home if the Mac is plugged in.
  When a condition lapses, the agent pauses the job (`SIGSTOP` to its process group) and resumes it
  (`SIGCONT`) when it holds again.
- **Sleep.** A closed lid suspends every process, and they continue on wake. Lost connections are
  retried: downloads and copies resume by byte range, and NAS operations go through the I/O pool
  with timeouts and a remount.
- **Kills** (a reboot, a crash, an update) lose at most the current stage:
  - per-tile and worldwide jobs take minutes, and simply rerun;
  - the OSM pass is a chain of stages with completion markers (copy, three filters, cut per z3
    piece, Planetiler, uploads per piece), each under about an hour.
- **Atomic writes.** Every file is uploaded to a temporary name, checked against its hash, then
  renamed. A catalog is written last, with a checksum line, and readers take the highest complete
  one. Anything half-written is never referenced. GC never deletes files under 14 days old, and it
  removes temporary files older than a day.
- **Staging is disposable.** Everything on the M4's SSD can be copied from the NAS again. The agent
  deletes it if the Mac's free space falls below its floor, or after a job has been paused for a
  week.
- **Status.** The agent writes a heartbeat. The app shows what's waiting and why: "Build Mac last
  seen yesterday; Kanto waits for it to be plugged in at home." 

**Determinism:** the same inputs give the same bytes: sorted outputs, no hash-map order, fixed
reductions. Each step gets a "build twice, compare hashes" test.

**Validation:**
- Files decode, and values are in plausible ranges.
- Summit tolerance for 30 m DEMs: Snowdon reads 1,040 m for its 1,085 m.
- If a tile loses more than 20 % of its ways since its last build, its publish is held.

**Format bumps:** publish an app that reads both formats, then the data. Drop the old reader once
everything is rebuilt.

**App publishing:**
- I sync the code to the M4 and run `scenic publish`. It builds, runs the tests and smoke-tests a
  server on a spare port against the current catalog, then publishes.
- `scenic publish --rollback` restores the previous app.
- The agent never fetches from git: the remote's SSH key needs 1Password's approval.

**Names:** the user's command is `scenic`. The pipeline's `scenic` binary (the road metrics) is
renamed `scenic-metrics`.

## 9. Sizes (estimates; measure in phase 3)

| | today's coverage | whole world |
|---|---|---|
| OSM pieces (all land, one pass) | ~45 GB | ~45 GB |
| base packs | ~30 GB | ~300 GB |
| layers | ~110 GB | ~1.2 TB, with buildings |
| sources kept | ~60 GB | ~500 GB |
| NAS in all, with 14 days of replaced files | 250–450 GB | 2–3 TB (~10 TB free) |
| M4 staging peak | ~150 GB (the OSM pass) | the same |
| an app Mac's minimum cache (root and lo packs) | 5–10 GB | 15–25 GB |

## 10. Migration (phases 1–6 reproduce today's map; features come after)

1. **Foundations:**
   - the pack format and reader; the catalog;
   - the I/O pool and `getfsstat`;
   - the local cache;
   - the launcher as a login item; offline start.

   The server serves today's `data/build` as a legacy layer set. Test the network-volume permission
   for the server and the agent.
2. **Agent and moves:**
   - the agent: requests, status, the scheduler, and gated app publishing;
   - move both Macs' project data to the NAS, except `data/build`: ~60 GB here, ~90 GB on the M4
     (approved);
   - move `keys.env` to `inputs/` (approved);
   - the NAS's planet fetch: one DSM scheduled task, created by the user or with their OK;
   - remove the empty `road-elevations` folder (approved);
   - measure the internet speed.

   `data/build` stays on this Mac until the cutover.
3. **The OSM pass and global-source layers:**
   - delete the M4's legacy build; the legacy map runs on this Mac until the cutover;
   - measure the filter ratios, then run the pass;
   - build the basemap;
   - import today's terrain, slope (z11 and coarser) and trees into packs;
   - grow coverage in the background.
4. **Per-area and worldwide steps.** The pilot is Northumberland plus the Scottish Borders:
   - Two outlines across the England–Scotland border, with a z6 edge at 55.78° N between them.
   - It has Kielder, Northumberland National Park, Hadrian's Wall, the East Coast Main Line and the
     Borders Railway.
   - Serve it from a development server beside the legacy map.
   - Compare its values with the legacy data, and across the tile edge.
   - Run the determinism tests.
5. **Browser:**
   - landmarks, stations and overlays by view;
   - OSM ids for rail service and road tiles;
   - one basemap source;
   - English from the server;
   - zoomed-out queries;
   - switching catalogs in place;
   - the Regions panel and the status bar.
6. **Cutover:**
   - set the coverage to today's 30 outlines and build it;
   - compare with the legacy build: counts, and screenshots of fixed views in the bench Chrome;
   - switch;
   - delete the legacy data and `data/build`.
7. **Features,** each on its own:
   - river labels and road names on the map;
   - 3D buildings, then PLATEAU;
   - building heights in horizons and the viewshed tool;
   - the new terrain repair;
   - sharper terrain from national DEMs.

## 11. Risks and checks

- **The OSM pass's disk and time on the M4:** measure first; it runs alone.
- **macOS network-volume permission:** tested in phase 1.
- **Hard SMB mounts:** the I/O pool, `getfsstat`, offline start.
- **The build Mac's availability:** work progresses only while the M4 is awake, plugged in and,
  for NAS steps, at home. A closed lid stops building, because macOS sleeps a closed laptop, and
  that stays as it is. Nothing is lost while it waits. If waiting proves too slow, let the other Mac
  take per-tile jobs as well. That isn't planned, because it needs a toolchain on both Macs and a
  movable writer lease.
- **Dense tiles with their halo** (Tokyo) must fit in 48 GB: measure in phase 4. If one doesn't,
  split that tile's pack job by z7.
- **Remote DEM servers** may be slow or change. Today's cache seeds the first builds, and reuse
  keeps later ones small.
- **Cascades from worldwide steps** are bounded to tiles whose values changed. A new region that
  extends a long highway rebuilds the packs along it, which takes seconds each.
- **Version bumps at globe scale** take days: background, oldest first.
- **Way ids past u32:** decades away; the tile format is versioned.

## 12. Review resolutions

**v5 (third review: 26 items, 7 simplifications).**

**The core change** answers the review's simplification S1 ("by area" as the only scope), going one
step further: OSM comes from the planet, cut by tile. That fixes the four blockers:
- **Relations crossing borders:** pieces come from the planet, with multipolygons complete, and
  designated areas, rail and ferry routes come from the worldwide sets.
- **Long features beyond the halo:** ferries and areas are worldwide, and pack(T) reads every tile
  whose features reach it.
- **Whole-road lengths:** local junction pairings plus a worldwide union-find. Offsets also make
  drives and whole roads exact across packs.
- **Ownership:** a way belongs to the tile of its first node. Regions never own anything, so
  adding, removing or refreshing a region rebuilds nothing beyond its tiles and their slices.

**The other items:**
- **Rail service:** a worldwide step on the worldwide track graph. Ferries are worldwide only.
- **Priority:** `-c utility` plus `caffeinate`, not `-b`.
- **Order:** global sources before base data. The sea mask is named, and terrain within the
  coverage plus 20 km means a neighbour's new region never changes existing viewsheds.
- **Zoomed-out queries:** 1 km summaries and landmark counts in lo packs.
- **Zoom ranges:** root z0–2, lo z3–8, hi z9–14.
- **z12 slope:** made when served.
- **App Macs:**
  - a local app, `getfsstat`, all NAS operations in the pool, and offline start;
  - `data/build` stays on this Mac until the cutover.
- **Network-volume permission:** a stable launcher, with a self-signed identity as the fallback.
- **IDs:** OSM ids replace gids, index versions and 409s.
- **Snapshots:** dated backups of the user's folders instead.
- **Per-region costs** are gone: pageviews, registers and feeds are processed once.
- **Translations:** any files in per-area folders, and the `via` field is accepted.
- **New countries:** module defaults, which `status` reports.
- **Phases:**
  - the browser gets its own phase;
  - the pilot crosses a border and a tile edge.
- **Determinism tests.**
- **App publishing** is gated and can roll back, with no git fetch.
- **M4 cache budget:** 20 GB.
- **`scenic-metrics` rename.**
- **Sea tiles** are deduplicated, not skipped, and Planetiler's extras are pinned.
- **Chaining** is by offsets, not node ids.
- **Wording.**

**After the review (the user's question about the laptop):** the NAS fetches the planet; jobs
pause and resume by condition; stages, atomic writes and disposable staging; a heartbeat in the app
(§8, Interruptions).

**Simplifications adopted:**
- S2: no Mac records;
- S3: stable ids;
- S4: no stored z12 slope;
- S5: an always-on server, and the Regions panel as the main surface;
- S6: reading areas only;
- S7: English applied when serving.

**v4 (the user's challenges):**
- Borders and tiny regions led to storing everything the map reads by area. v5 keeps that, and
  removes regions from processing too.

**Rounds 1 and 2 (35 items),** resolved in v3 and still holding:
- **Storage and writers:** one worldwide basemap; packs; the M4 as the only writer; numbered
  catalogs; 14-day GC.
- **Terrain:** from AWS only, with a sea-mask clamp and a latitude cap.
- **Server:** per-tile ETags; lazy, pack-based serving; per-Mac budgets; `keep` by area.
- **Format versions:** read two at a time.
- **The planet:** only passes through the M4.
- **Translations:** the folder rules; descriptions follow the same contract.
- **Process:** features after the cutover; chained fingerprints; validation thresholds; backups.
