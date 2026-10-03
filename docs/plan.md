# Plan: regions as modules, data on the NAS

**Version 6**, 2026-10-02: v5 after the fourth Opus review (§13). Implementation started on the
user's go-ahead; Opus reviews at each phase may amend this plan when an unforeseen constraint
justifies it (record the change in §13). Amended in phases 1–2 (§13, Implementation).

- Owner: Claude. The user asked me to own the design, to treat their earlier requirements as
  malleable, and to prefer correctness to speed (fix rough edges rather than work around them).
- This file is for my own reference. The user follows the diagram artifact.
- The diagram's source is `docs/diagram/`. To rebuild it, run `python3 page.py out.html`, then
  republish `out.html` to https://claude.ai/artifact/PrjaaDXLuuXGpGr2xt5Vjg.

**The idea in one line:** OpenStreetMap comes from one worldwide download, cut by area, and a region
is only an outline saying which areas to build. Every step runs either per area or once for the
whole world, so region size, count and borders never show on the map, and nothing in storage or
serving knows about regions.

## 1. The user's surface

**The map is always there.**
- A small launcher is a login item on both Macs and keeps the server running. The server loads
  nothing until asked, so it costs nothing while idle.
- The user opens a bookmark (`http://localhost:8080`), or runs `scenic`.
- The server mounts the NAS itself when it's missing: at start, after wake and after network
  changes.

**The Regions panel** in the app. `scenic add`, `scenic remove` and `scenic status` do the same from
a terminal.
- **Search** for a place, offline, on the map's own worldwide list of places and administrative
  areas.
- **Choose an area.** The panel offers the administrative areas containing the place (county,
  province, country), plus Geofabrik units where they exist (Kanto, US Northeast), each with its
  cost.
- **Add, remove, rename, split, merge, or redraw** an outline (drag its points on the map).
  Renaming, splitting and merging rebuild nothing; changing what's covered rebuilds only what
  changed.
- Coverage is drawn on the map; progress and anything waiting show in the status bar; new areas
  appear as they finish.
- Away from home, edits queue on the Mac and go to the NAS when it's back.

**Translations:** drop `.jsonl` files into `translations/` on the NAS (in an area's folder, or at the
top with the area in the name, e.g. `places-jp.out.jsonl`). The map shows them within a minute of
being in use, with nothing rebuilt. Names still lacking English are listed in `translations/todo/`.
Descriptions work the same way (§7).

**Everything else is automatic:** building, refreshing (whenever the build Mac is plugged in at
home), publishing, mirroring to each Mac, distributing the app, backups. Commands are safe to
re-run. Messages say what happened and what to do, in plain words.

## 2. How the work is divided

A **unit** is a z6 tile (about 600 km across at the equator, 300 km at 60°), split into z7 or z8
tiles where its OSM piece is dense (Tokyo–Osaka). Units are fixed per OSM pass. Packs for serving
stay per z6 tile.

All work happens on the build Mac (the M4), in four kinds of job:
1. **The OSM pass**, twice a year or when asked. The NAS downloads the planet; the build Mac turns it
   into:
   - **pieces:** filtered OSM per unit with a 10 km buffer (ways kept whole), for all land, each
     with its data extent;
   - **worldwide sets:** rail (tracks, stations, routes), ferries, designated areas (complete),
     places, administrative and ISO 3166 outlines;
   - **worldwide road values:** each way's road id, road length, and offset and direction along its
     road (§6), sliced per unit;
   - **the basemap** (Planetiler, worldwide), and the labels' and names' worldwide inputs.
2. **Global-source layers**, pack by pack: terrain, slope, trees, the analysis grids, overlays,
   later buildings.
3. **Per unit**, for every unit whose piece's data extent meets the coverage:
   - **base(U):** the features U owns (a way belongs to the unit of its first node) that touch the
     coverage, with per-vertex elevations, grade and scenic channels. Each value is computed once.
4. **Per z6 pack and rankings over the coverage:**
   - **pack(T):** tiles, query parts and indexes for T, from the base data of every unit whose
     features reach within 100 km of T, plus T's slices of the worldwide values;
   - **rankings:** landmark fame and top-by-kind, labels, ferries and rail service, names todo.

Because road values come from the whole planet, adding or removing a region never changes anything
outside its own units and their 100 km halo.

## 3. Storage

NAS root: `personal/projects/scenic-roads/` (the `personal` share; its Recycle Bin is on).

```
README.md                     one screen: what's here; the two folders that are yours
translations/  descriptions/  the user's drop-ins (README; todo/ written by the agent)
inputs/                       regions/<id>.toml, outlines/, manual/ (MOI DTM, fhd.xlsx), keys.env, timetables/
sources/                      osm/<date>/ (planet, filtered.osm.pbf, pieces/, sets/, roads/), aws/, canopy/, overture/, registers/, gtfs/, pageviews/, basemap/, legacy/
base/                         base packs, one per unit
global/                       worldwide values sliced per unit; names tables
layers/<layer>/               root, lo and hi packs (our layers); basemap.<hash>.pmtiles
app/                          published app builds; current.json; launcher/
catalog/                      <n>.json.zst: every current file, with its hash and format version
state/                        status.json (heartbeat), lock, logs/, backups/
nas/                          scripts the NAS runs itself (fetch-planet.sh)
```

**Immutable, content-named files.** Every file is named `<logical>.<hash16>.<ext>` and never
rewritten.
- Writes go to `<name>.tmp`, are checked against their hash, then renamed. An unchanged file keeps
  its name, so a rebuild uploads only what changed.
- The catalog is the only index: one zstd JSON per publish, with a checksum line. Readers take the
  highest complete one.

**Deletions go through SMB.** On this share they're permanent (tested 2026-10-02: a file deleted over
SMB leaves nothing in the Recycle Bin), and the build Mac can't use SSH unattended (§13).

**Packs, never one file per tile.** SMB manages about 80 random reads per second per file.
- **Our layers** (roads, rails, terrain, slope, trees, labels, overlays, landmarks, stations) use
  packs: a header, then an index of (tile key, offset, length, 64-bit content hash) sorted by key,
  then blobs. Identical blobs are stored once.
  - **Root pack:** z0–2. **Lo packs:** z3–8, one per z3 tile. **Hi packs:** z9–14, one per z6 tile.
  - z9 and finer exist only within the coverage plus 20 km, so hi-pack indexes stay small.
- **The basemap** is Planetiler's worldwide PMTiles file (it deduplicates the sea), served tile by
  tile by the server.
- **Base packs** hold per-vertex arrays per unit, indexed by z9 sub-tile so a halo is a few range
  reads.

**One writer.** The build Mac's agent writes everything, except:
- region recipes, which any Mac's server creates exclusively (`O_EXCL`), and marks removed by
  renaming to `.removed`;
- the user's folders.

**GC** keeps every file that any catalog from the last 14 days references, and every file under 14
days old (in-flight work). It deletes over SMB, only in the folders catalogs index.

**Backups:** when the user's folders or `inputs/` change, the agent keeps a dated copy for 30 days
under `state/backups/`, mirrored to the build Mac. Everything else can be regenerated. (A separate
share with DSM snapshots and no Recycle Bin would be cleaner; that needs the user in DSM, so it's
offered, not assumed.)

## 4. Macs

**Build Mac (M4, 48 GB, ~225 GB free once its legacy data is gone).**
- `scenic agent` runs under the launcher.
- **Priority:** the launcher's jobs are `ProcessType Interactive` (launchd would otherwise throttle
  them). Build jobs lower themselves with `taskpolicy -c utility` (`-b` confines them to the 4
  efficiency cores, ~17× slower).
  - Half the cores while the user is active, all of them when idle.
  - Each job holds `caffeinate -s -w <pid>`: no idle sleep on power, and none kept on battery.
  - One CPU job at a time, plus one network-bound fetch job (DEM range reads, AWS tiles).
- **Build cache:** one LRU cache of build inputs and outputs on the SSD (pieces, base packs, canopy
  10° files, DEM caches), ~120 GB, cleared before the OSM pass. It also serves the M4's own map.
- 40 GB stays free for swap.
- It may be asleep, away or unplugged at any time (§8, Interruptions).

**App Macs (both laptops).**
- **The launcher** (`tools/launcher/launcher.c`) is built once and never rebuilt. It runs the
  command in `~/Library/Application Support/scenic/run/<name>` as a child and restarts it when it
  exits. The NAS reads fine from it (tested on both Macs, 2026-10-02); if a future macOS asks, Full
  Disk Access for the launcher is the fallback.
- The server copies a newly published app to local disk, then restarts into it when the map is idle.
- The app reads the current and the previous format version of every file kind.
- A development override runs the repo.

**Server.**
- **Finding the NAS:** `getfsstat(MNT_NOWAIT)`, never `statfs`. If the share is missing, it mounts
  it with `osascript` and the Keychain.
- `~/Library/Preferences/nsmb.conf` makes the share a soft mount.
- **NAS access:** every operation (open, stat, list, read) goes through a bounded I/O pool with
  timeouts. The first timeout marks the NAS offline (a banner); one prober checks it. NAS files are
  never mmapped.
- **Offline start:** the last catalog and every pack's index stay local.
- Nothing is loaded at start.
- **ETags:** each tile's content hash, combined for named tiles with the translations version of
  the areas it touches. A 304 never needs the NAS.

**Mirror, per Mac.** While at home, each Mac copies every current pack and base pack in the
background (sequential reads, paused while the M4 is publishing), so the map is as fast as today's.
- The budget is the free space minus a reserve (50 GB on the M1, the build cache on the M4).
- Beyond the budget: zoomed-out packs first, then the most recently used.
- "Keep this view" waits until coverage outgrows the disks.

## 5. Coverage and regions

**Recipe** (`inputs/regions/<id>.toml`): `id`, `name`, and `outline`, a list whose union is the
region. Each entry is one of:
- `osm:<relation>`: an administrative or ISO 3166 area from the pass's outline set. Neighbours
  share borders exactly, so their unions are seamless. A 1 km buffer covers coasts.
- `geofabrik:<id>`: Geofabrik's outline (buffered; neighbours overlap a little).
- a `.poly` in `inputs/outlines/` (the panel writes one when an outline is drawn or edited);
- a place and a radius.

**Coverage** is the union of the outlines.
- A feature is built if it touches the coverage: for a way, any node inside, as Geofabrik does.
  Ways are kept whole, so roads end a little past the edge, as today.
- **Redefining regions is a recipe edit.** Builds depend on coverage, never on regions: base(U)'s key
  holds U's selection (the sorted ids of the features it builds). Same ground, nothing rebuilt;
  grown or shrunk coverage, only the units whose selection changed, the packs within 100 km of them,
  and the rankings.
- Shrinking leaves global-source tiles in place; they're harmless.

**Today's set:** 34 regions become 30 outlines (Monaco, Isle of Man, Guernsey-Jersey and Gibraltar
lie inside others). Singapore and Saint-Pierre-et-Miquelon become `osm:` outlines.

**Modules apply by location,** from ISO 3166-1 and 3166-2 outlines (Hong Kong is ISO 3166-1 HK,
not admin_level 2):
- DEM order and densification spacing (8 m in North America, 15 m elsewhere);
- heritage registers (national and provincial);
- timetable feeds (default: Mobility Database feeds touching the coverage);
- road network codes and their colours;
- leaf-type source (EEA in Europe, NALCMS in North America, none elsewhere);
- reading area for names;
- credits.

A country without a module gets defaults: FABDEM, no register, colours by road class. `status` says
which defaults each region uses.

**No seams:**
- **Region borders:** no step knows about them.
- **Tile edges:** per-vertex values are computed once, by the owning unit. Road values come from the
  planet. Climbs come from the 100 km halo. Queries join road parts by offset.
- **Where coverage ends,** roads end, as today.

**IDs:** OSM type and id. Way ids fit u32 until the 2040s; node ids already don't, so POIs use u64.
- Each hi pack has a "ways here" index: every way drawn in its tiles → its owner unit and its index
  in that unit's base pack. Details and profiles ask with the id plus the location, which picks the
  pack.
- Within a draw class, RT lines are sorted by id (delta coding keeps tiles small).

## 6. Pipeline

### The OSM pass (twice a year)

1. **The NAS fetches the planet.** `nas/fetch-planet.sh` downloads the newest dated planet from a
   mirror, resuming after any interruption, checks its MD5 and renames it into `sources/osm/<date>/`.
   DSM's Task Scheduler runs it daily (it does nothing until the newest planet is six months old,
   or `nas/fetch-now` exists, which the agent can create over SMB); it runs on the NAS on its own.
2. The M4 copies the planet to its SSD (resumable, ~30 min).
3. `osmium tags-filter`, generous, from the filters every step declares:
   - (a) what the pipeline reads, including `landuse=forest` and the buildings the heritage step
     names; kept on the NAS until the next pass, so a new tag needs no new download;
   - (b) the basemap's input;
   - (c) the worldwide sets, matching route relations directly (osmium adds members one level deep).
4. Delete the local planet.
5. Cut (a) into units in batches (osmium's ID sets span the global range), keeping ways whole,
   completing multipolygons and adding a 10 km buffer; record each piece's data extent; decide the
   unit split by way count.
6. **Worldwide road values**, per piece then joined (below).
7. **Basemap:** Planetiler on (b), worldwide, with its jar and extras pinned (Natural Earth, water
   polygons, lake centerlines), in `sources/basemap/`.
8. Outlines: assemble administrative (levels 2–8) and ISO 3166 areas into polygons for the panel and
   the modules.

### Global-source layers (pack by pack, rebuilt from raw, never patched)

- **Terrain:**
  - z0–8: AWS's own tiles, repaired, worldwide (independent of coverage);
  - z9–12 within the coverage plus 20 km (viewsheds see 15 km), capped by latitude so pixels stay
    ≥ 15 m: z12 to 67°, z11 to 79°, z10 beyond;
  - repaired by today's `repair_terrain`; values below zero are clamped only where the sea mask
    (the pass's water polygons, clipped per tile) says sea.
- **Slope:** z11 and coarser stored, from transient z12 Horn slope. The server makes z12 on demand
  with the same encoder and an LRU (56 % of today's archive).
- **Trees:** canopy 10° files and leaf type within the coverage; z4–8 aggregated.
- **Grids (z11):** land cover, canopy and cover within 20 km of built roads (analysis only; not
  served). `grid.terrain` goes: the analysis reads terrain's z11 tiles.
- **Overlays:** heritage areas, Indigenous lands, special areas and World Heritage outlines, from
  the sets plus the registers; assembled once, simplified per zoom, clipped per pack.
- **Buildings (phase 7):** Overture plus official data → z13–14 within the coverage.
- **Peaks** read worldwide z8 terrain, so their prominence doesn't depend on coverage.

The server builds missing deeper tiles from their ancestors.

### Worldwide road values (in the OSM pass)

- **One chaining** for whole roads, strokes, drives, hover, profiles and rides: at each node, way
  ends are paired by mutual best continuation (same ref, else same name, else same class when both
  are unnamed, else an unnamed way continuing a named one of its class within 35°; straightest
  first within 100°; oneways in their direction). The pairing at a node
  depends only on the ways through it, so it's computed per piece for nodes inside the unit
  (exact: a piece holds every way through those nodes).
- Pairings form paths and cycles. A union-find joins them; an ordered walk from an unpaired end (a
  cycle starts at its lowest way id) gives each way its road id (the road's lowest way id), the
  road's length, and the way's offset and direction.
- Per way ~30 bytes: ~7.5 GB for the world, under an hour per pass.
- Lengths include parts outside the coverage: a road's length is a fact about the road.
- Elevation smoothing keeps today's local continuation rule (`Net.cont`); it runs in base(U).

### Per unit: base(U)

1. **extract:** U's owned features that touch the coverage, by today's rules (rail tracks without a
   route relation kept by type).
2. **elevations:** DEMs by location. The previous base(U) and today's per-vertex DEM cache are the
   cache.
3. **clean-up and grade,** with junction context from the piece. (Structure networks cut by the
   buffer can disagree at a shared node; checked in the pilot.)
4. **scenic:** reads terrain z11, grids, canopy and designated areas; unchanged samples keep their
   values.
5. **POIs, heritage, details, peaks.**
6. **names inventory.**
7. **output:** the base pack (per-vertex arrays and records, indexed by z9 sub-tile).

### Per z6 pack: pack(T)

- **Reads:** base data of every unit whose features reach within 100 km of T (from base packs'
  extents), and T's road-value slices.
- **Writes:**
  - road and rail tiles z6–14 (z6–8 to T's lo pack);
  - climbs that start in T;
  - **query parts:** for each road through T, its 100 m samples and channels with their offsets
    (drives, rides);
  - the "ways here" index;
  - landmark and station tiles;
  - T's z4–5 contribution.
- **Lo packs** per z3 tile: roads z3–8, each kind's top landmarks, per-kind counts per z6 cell, and
  1 km query summaries.

### Rankings over the coverage (cheap, after packs)

Landmark fame and top-by-kind (one worldwide pageview table per article and season); labels;
ferries; rail service (trains a day on the worldwide track graph; timetables processed once per
feed version); names todo. Their outputs are small worldwide files or slices; a change rebuilds only
the lo packs and slices whose content changed.

### Job keys

Each job's key is its step version plus the hashes of exactly what it reads, clipped:
- base(U): U's piece, U's selection, terrain/grid/canopy tile hashes within U + 20 km (from pack
  indexes, not pack files), DEM source versions, module versions;
- pack(T): the base packs' sub-tile hashes within T + 100 km, T's road-value slice;
- terrain pack: AWS tile hashes, the sea mask clipped per tile;
- rankings: their inputs' hashes.

Outputs are content-hashed; an output identical to before stops the cascade.

### Served

| What | How |
|---|---|
| our layers | the pack for the tile; English attached to named features |
| basemap | tiles from the worldwide PMTiles; English attached |
| drives, rides | parts from packs in view plus a margin of half the drive length, joined by offset; zoomed out (more than ~6 hi packs): 1 km summaries |
| whole road (hover) | the road's parts, from the road → units index |
| details, profiles | by OSM id plus location (the "ways here" index → base pack) |
| `/api/catalog` | coverage, credits, versions, progress |

**Browser:** our layers' tile URLs as today; the basemap and labels as tile URLs (no longer
`pmtiles://`, the coast worker included); landmarks, stations and overlays by view; English from
the server; switching to a new catalog in place.

## 7. Names and descriptions

**One pipeline for every named thing:** places, states, seas and lakes, rivers and waterways, parks
and protected areas, natural features, stops and sights, heritage sites and areas, special places,
Indigenous lands, stations, rail lines, ferries, and road names (in the app's text; no road labels
on the map).

**Display:** each name has a **main** label and an optional **sub** line.
- Map labels: main as the label, sub below it. App text: "main (sub)".
- Order: a translation line for the name in its area, else the thing's own English (OSM's
  `name:en` or `name:ja-Latn`; UNESCO's or English Wikipedia's title for heritage) as sub, else
  nothing. Sub shows only when it truly differs from main.

**Applied when serving.** The server reads the user's folders directly, compiles them locally, and
attaches main and sub to every tile and record it serves. It polls while the map has had requests in
the last ten minutes (every minute), so a drop shows within a minute and nothing is rebuilt.

**Files:** `translations/**/*.jsonl`.
- Lines are the translation work's display format: `{"n", "main", "sub"}` (extra fields such as
  `case`, `via`, `check` ignored). `n` is the name as in OSM. Older `{"n", "en"}` lines mean
  main = n, sub = en; `"en": null` means checked, nothing to add.
- The area is the folder (`translations/jp/…`), else a token in the file name (`places-jp.out.jsonl`).
  Codes are the translation work's: `jp`, `tw`, `hk`, `sg`, `fr` (France, Monaco), `ib` (Spain,
  mainland Portugal, Andorra, Gibraltar), `pt` (Azores, Madeira), `na` (Canada, the US), `gb`
  (Britain, Ireland, Isle of Man, Channel Islands). New countries get new codes.
- Later file names win. A file is read once its size and modification time have held for 10 s; an
  unfinished last line is ignored.
- Reading areas come from the ISO 3166 outlines (a few island groups by box).
- The source is the user's `place-translations` work (branch `claude/exciting-cori-x46sfk`,
  `out/display/`, copied 2026-10-02).

**Todo:** `translations/todo/<area>.jsonl`, with priority metrics, regenerated by the agent.

**Descriptions:** `descriptions/**/*.jsonl`, lines `{"qid", "long"}` (or `{"id": "n123"|"w123"|"r123",
"long"}` for things without a Wikidata item). Today's `data/heritage/desc/*.out.jsonl` move there.
`descriptions/todo/` lists what lacks one. I write them on request with Sonnet writers.

## 8. Building

**Regions:** recipes in `inputs/regions/`; the agent compares them with what's built and works out
the difference. No request files.

**Scheduler:** every job is one step for one unit or pack, or one worldwide step, keyed as in §6.
Unchanged jobs reuse their outputs by name.

**Order, in spatial waves** (a cluster of units plus its halo), so new areas appear as they finish:
1. the OSM pass, when the NAS holds a newer planet than the pieces come from;
2. global-source packs for new coverage (plus 20 km);
3. per wave: base(U), then pack(T), then publish a catalog;
4. rankings and lo packs after the last wave, or every few hours.

Then, when idle: stale work after step-version bumps, oldest first; Overture, registers and
timetables every ~6 months.

**Interruptions.** The build Mac may be asleep, away or unplugged at any time, or close its lid
mid-job. Nothing depends on it being available at a given time.
- **No deadlines.** Until work is done, the map serves the last catalog.
- **Conditions per step.** CPU work needs power; NAS steps need the NAS; local steps carry on away
  from home if plugged in. When a condition lapses, the agent pauses the job (`SIGSTOP` to its
  process group) and resumes it (`SIGCONT`) when it holds again.
- **Sleep** suspends every process. Open SMB handles often don't survive it, so a stage that touched
  the NAS is retried from its inputs after wake; downloads and copies resume by byte range.
- **Kills** lose at most the current stage: units and packs take minutes; the OSM pass is a chain
  of stages with completion markers, each under about an hour.
- **Atomic writes** (§3); anything half-written is never referenced.
- **Staging is disposable:** the build cache can always be refilled from the NAS.
- **Status:** the agent writes a heartbeat; the app shows what's waiting and why ("Build Mac last
  seen yesterday; Kanto waits for it to be plugged in at home").

**Determinism:** the same inputs give the same bytes (sorted outputs, no hash-map order, fixed
reductions). Each step has a "build twice, compare hashes" test.

**Validation:** files decode; values in plausible ranges; summit tolerance for 30 m DEMs (Snowdon
reads 1,040 m for its 1,085 m). A unit that loses more than 20 % of its ways without its coverage
shrinking holds its wave's publish; the previous version keeps serving, and status says so.

**Format bumps:** publish an app that reads both formats, then the data; drop the old reader once
everything is rebuilt.

**App publishing:** I sync code to the M4 and run `scenic publish`: build, tests, a smoke test (a
server on a spare port against the current catalog), then publish. `scenic publish --rollback`
restores the previous app. The agent never fetches from git.

**Names:** the user's command is `scenic`; the pipeline's metrics binary becomes `scenic-metrics`.

## 9. Sizes (estimates; measured in phase 3)

| | today's coverage | whole world |
|---|---|---|
| planet + filtered (a) | ~90 + ~45 GB | the same |
| OSM pieces (all land) | ~50 GB | ~50 GB |
| road values (all land) | ~8 GB | ~8 GB |
| base packs | ~30 GB | ~300 GB |
| our layers (terrain and slope ~3.4× today's, for roadless coverage) | ~150 GB | ~1.2 TB, with buildings |
| basemap (worldwide) | ~20 GB | ~20 GB |
| sources kept | ~60 GB | ~500 GB |
| NAS in all, with 14 days of replaced files | 400–600 GB | 2–3 TB (~9 TB free) |
| M4 staging peak | ~150 GB (the OSM pass) | the same |
| an app Mac's mirror | everything, ~200 GB (M1: budget-limited) | budget-limited |

## 10. Migration (phases 1–6 reproduce today's map; features come after)

At each phase's end an Opus agent reviews the work against this plan.

1. **Foundations, on today's data.**
   - The `store` crate: pack format, catalog, content naming, the I/O pool, `getfsstat`, mounting,
     the mirror.
   - Convert today's `data/build` into NAS packs, base packs (per unit, from today's arrays) and a
     catalog: the new formats, holding today's data.
   - The server serves from the catalog and packs: lazy, per-pack way data, ids plus location,
     offline start, server-side main/sub English.
   - The client follows: OSM ids, basemap and labels as tile URLs, English from the server.
   - Both Macs run the published server through the launcher. Compare with the legacy server
     (golden responses, screenshots), and measure performance against the baseline.
2. **Agent and moves.** The agent (recipes, heartbeat, scheduler with conditions, gated app
   publishing, GC); move both Macs' project data to the NAS and delete local copies
   (approved); `keys.env` to `inputs/`; descriptions to `descriptions/`; remove the empty
   `road-elevations` folder.
3. **The OSM pass and global-source layers:** filter ratios, the pass (pieces, sets, road values,
   outlines), the worldwide basemap, terrain/slope/trees/grids/overlays packs.
4. **Per-unit pipeline and rankings.** Pilot: Northumberland plus the Scottish Borders (two `osm:`
   outlines across the England–Scotland border, with a z6 edge at 55.776° N between them; Kielder,
   the national park, Hadrian's Wall, the East Coast Main Line, the Borders Railway). Compare with
   today's data in that area and across the edge; determinism tests.
5. **Browser:** landmarks, stations and overlays by view; zoomed-out queries; the Regions panel; the
   status bar; catalog switching.
6. **Cutover:** coverage = today's 30 outlines; build; compare (counts and distributions: lengths,
   drives, climbs change under the new chaining); screenshots and performance; switch; delete the
   converted legacy data.
7. **Features,** each on its own: 3D buildings, then PLATEAU; building heights in horizons and the
   viewshed tool; the new terrain repair; sharper terrain from national DEMs.

## 11. Risks and checks

- **The OSM pass's disk and time on the M4:** measure first; it runs alone.
- **Hard SMB mounts:** soft mount, bounded pool with a circuit breaker, `getfsstat`, offline start.
- **The build Mac's availability:** work progresses only while the M4 is awake, plugged in and, for
  NAS steps, at home. A closed lid stops building; nothing is lost while it waits. If waiting proves
  too slow, the other Mac could take per-unit jobs (needs a toolchain there and a movable writer
  lease; not planned).
- **Dense units with their halo** must fit in 48 GB: the unit split handles base(U); pack(T) splits by
  z7 if needed.
- **Remote DEM servers** may be slow or change; today's cache seeds the first builds.
- **Version bumps at globe scale** take days: background, oldest first.
- **Way ids past u32** (2040s): the tile format is versioned.

## 12. Answers to the review's questions (2026-10-02)

- `personal` has its Recycle Bin on and no snapshots, but SMB deletions bypass it (tested): GC
  deletes over SMB; a separate share is
  offered, not assumed.
- No map freeze: phase 1 serves today's data in the new formats on both Macs.
- The new chaining's lengths, highlights and drives differ from today's; compared by distribution.
  Lengths include parts outside the coverage.
- Profiles keep per-vertex resolution (base packs, ~30 GB today).
- Both Macs mirror everything that fits; the M1 keeps 50 GB free.
- No road labels on the map.
- Fine terrain in roadless coverage stays (the user asked for it); grids stay near roads.
- Wired M4, remote access while travelling: unknown; the design doesn't need either.
- Per-area translation tables, as the translation work has them.
- SMB reads with several handles per file: measured in phase 1 to size the I/O pool.

## 13. Review resolutions

**v6 (fourth review, 20 items, 8 simplifications):** keys clipped to what each job reads, with output
early cut-off; one chaining, with an ordered walk for offsets; per-vertex base packs for profiles,
query parts as 100 m samples, queries with a margin, a "ways here" index; mirror everything that
fits; deletions over SSH (Recycle Bin); self-mounting, soft mounts, a circuit breaker, stages retried
after sleep; `ProcessType Interactive`; units chosen by their pieces' data extents; `osm:` outlines;
translation codes and file names as the translation work has them, descriptions as `{"qid","long"}`;
phase 1 on today's data (no freeze); basemap worldwide in PMTiles, our layers' z9+ within coverage;
a generous filter kept on the NAS, today's rail rules; adaptive units; waves; polling only in use,
per-area ETags; holds that ignore shrinking; ISO 3166-1/2 modules with spacing and leaf type.
Simplifications adopted: worldwide road values in the pass (S1); mirroring (S4); the M4's build
cache (S5); recipes instead of requests (S7). Kept: our own pack format for our layers (simple,
already used by the pipeline and server; PMTiles for the basemap); Geofabrik units as an extra
outline source; homemade backups until the user sets up a share.

**After v5 (the user's questions):** the NAS fetches the planet; jobs pause and resume; regions can be
split, merged, renamed and redrawn; translations in the display format.

**v5 (third review):** OSM from the planet, cut by area; regions as outlines; worldwide sets;
translations applied when serving; an always-on server and a Regions panel.

**v4 and earlier:** everything the map reads stored by area; one worldwide basemap; the M4 as the
only writer; numbered catalogs; per-tile ETags; terrain from AWS with a sea-mask clamp and a
latitude cap; format versions read two at a time.

**Implementation (phases 1–2, 2026-10-02):**
- **No SSH from the build Mac.** 1Password asks for every new SSH session, so nothing unattended
  uses it: GC deletes over SMB; uploads are checked by `write_atomic`'s read-back (the NAS-side
  SHA-256 check, `scenic-build verify`, is a manual extra from an app Mac); the planet fetch runs
  from DSM's Task Scheduler.
- **Units are z6 tiles** for now; the z7/z8 split comes with phase 4 if a dense unit needs it.
- **Failures are never cached.** A read the NAS can't answer is a 503 (a versioned 2xx is cached for
  a year), and nothing built from a failed read is kept: details, layer files, basemap archives,
  whole roads. NAS files are read in 1 MB pieces with their handles kept open, so a busy NAS doesn't
  trip the circuit breaker on one big read.
- **Names:** separate places and roads tables per area (road names read the roads table first,
  everything else the places table, each falling back to the other); `todo`/`skipped` lines are
  ignored; a sub already among main's parts is dropped. Basemap ETags take the areas within a tile
  of the tile (Planetiler's label buffer). URL versions of named data include the translations
  version.
- **Programs:** `scenic` is the user's command and the agent; the legacy analysis binary is
  `scenic-metrics`; the published app carries `server`, `scenic`, `scenic-build` and `extract`.
- **The agent** (`pipeline::agent`): one job at a time as a child process group (utility priority,
  `caffeinate -s -w`), SIGSTOP/SIGCONT on conditions, restarted after sleep when it touches the NAS,
  failures retried after 10 min doubling to 6 h, orphans of a crashed agent stopped at start; daily
  content-addressed backups (30 days, mirrored locally) and GC; the OSM pass when a newer planet is
  complete on the NAS and 150 GB are free. Later phases add their jobs to its plan.
- **Golden test** (`golden`, legacy server vs new): Singapore on the converted data: 400/400 ways
  equal, profile elevations equal at all shared vertices, layer counts and details equal.
- **Testing note:** a freshly built server started from the desktop app's preview waits on macOS's
  network-volume permission prompt; test servers run from the shell, and the launcher (granted
  once) runs the published ones.

**Review of phases 1–2 (Opus, 2026-10-02), adopted:**
- **GC roots:** the newest catalog always, every catalog of the last 14 days, and the build manifest;
  an unreferenced file goes only when it's also older than 14 days, and a reused upload is touched.
- **Catalogs list only what the map reads** (layers, base packs, hidata, global files, the latest
  outlines), one basemap (the pass's worldwide archive once there is one), and fail on a missing
  file. Build sources stay out, so mirrors never copy them.
- **Server caches are keyed by content name**; what's read from the NAS has memory budgets (3 GB of
  base packs, 1 GB each of hidata and other sections) and at most 128 open NAS files (the open-file
  limit is raised at start); it's dropped once the mirror has the file, so offline use works.
- **Writers:** one agent per Mac (a lock); build steps merge their manifest changes under a lock;
  an orphaned job is stopped only when its leader's start time proves it ours.
- **App manifests carry SHA-256s**, checked by the updater, which marks programs executable.
- **"In use" means any request:** an idle server loads nothing (warming starts at the first
  request), polls the NAS every 10 minutes instead of 30 s, and the agent rewrites its heartbeat
  only when it changes or every five minutes.
- **Versioned URLs are cached for good only when their version is current**, so a switch can't pin
  new data under an old URL. NAS failures are 503s everywhere (way lookups and queries included)
  and the client doesn't cache them.

**Implementation (phases 3–5, 2026-10-02):**
- **Outlines** are a stage of the OSM pass (`sources/osm/<date>/outlines`, a sectioned file of
  administrative levels 2–8 and ISO 3166 areas with simplified copies); catalogs name the latest as
  `global.outlines`. **Coverage** gives each outline a cell grid (centre state and nearby edges) so a
  point test reads one cell; `osm:` outlines get the 1 km coastal buffer.
- **Terrain and slope for new coverage** are per z3 pack: its z6 tiles' hi packs, then the lo pack
  with them folded in. Always from AWS's raw tiles (a raw-tile cache on the build Mac): processing
  a processed tile isn't idempotent, so stored tiles are never inputs; slope parents are made from
  their children as stored. Both are deterministic (checked: identical packs on reruns).
- **base(U) runs today's steps on a unit-sized folder** (extract on the piece, cut to the ways
  touching the coverage, sample.py on the unit's own slice of the per-vertex DEM cache, tile elev,
  scenic-metrics), with terrain.tiles and the z11 grids staged from the packs, then today's
  conversion for the ways the unit owns, with the pass's road values.
- **The agent plans the regions' work by job keys** (`state/build/jobs.json`): terrain, slope, units,
  packs, lo, catalog, each target keyed by what it reads; the first stale step runs as one job.
  Spatial waves come later; for now each step runs over all its stale targets.
- **Descriptions** (`descriptions/`) are served like translations: copied while the map is in use,
  laid over popup details (later file names win; `drop` removes). Today's written descriptions are
  in `descriptions/heritage/`, prefixed 1–4 to keep their old order.
- **Regions panel API:** `/api/regions` (recipes, created exclusively), `/api/areas` (containing a
  point, by name, one outline) and `/api/coverage`.

**Implementation (phase 4 pilot, 2026-10-02):**
- **Pilot (Northumberland and the Scottish Borders, units 6/31/19–20) against today's data:** same
  ways and geometry, all 13 scenic channels and every flag bit equal, elevation within 1 m
  everywhere (max 1.8 m), grade 99.9 % equal. Two fixes it needed:
  - land cover is classified only for the grid tiles the packs lack (`stage` lists them;
    `landcover.py --only`): its cross-run cache assumed one build folder and copied another
    unit's tiles by position;
  - today's heritage sites (the converted `heritage.json`) are staged into each unit's folder for
    the flags step (sites within 500 m), from a content-named local copy.
- **The golden comparison** (today's server vs the new one on the NAS's converted data, nine
  places): way info, profiles, terrain, slope, tree and road tiles, layers and popup details all
  equal. Descriptions laid over details are credited as the builds credit them (`refs`, else the
  record's Wikipedia article); sights match by their `wikidata`.
- **NAS reads by page.** A base pack or hidata on the NAS (not yet mirrored) is never read whole
  for a request: sections are typed views (`Sect`), mapped when local, else read in 256 KB pages
  for the records a request touches (binary searches over `here`, `byroad` and `roadunits`; a
  way's ranges), or whole for what a query scans, or for a long road touching much of a section.
  Budgets: 384 MB of pages, 1.5 GB of whole sections. A cold hover in Kanto (a 3 GB base pack) went
  from 66 s (which also tripped the breaker under load) to 1.9 s.
- **Road values files carry `byroad`** (the unit's ways sorted by road), so a profile finds a road's
  ways with a binary search; converted files without it are sorted once in memory.
- **Catalog zoom ranges** come from each layer's definition (terrain z0–12, slope z0–11, …), not the
  legacy conversion's record.
- **The OSM pass cuts a quarter at a time.** osmium keeps id sets per output spanning the whole id
  range: measured, about 4 GB of memory per output on a planet-sized input, so a 32- or 64-way cut
  couldn't fit in 48 GB. The cut is a depth-first quad tree (four outputs per run) from the
  filtered planet down to z6; each piece is uploaded and its chaining inputs kept as soon as it's
  cut, and files go once everything below them is done. The filtered planet is read from the NAS
  when the build Mac is short of room. The agent starts a pass with 80 GB free.
- **The NAS breaker** trips only when the share also fails a quick probe after an overrun: on a busy
  link a slow read fails alone instead of taking the map offline. The client asks for tiles that
  failed again (2 s, doubling to a minute; at once when the NAS is back).
- **The map's meta** (bounds, road km by elevation) is added up from the units' summaries (each base
  pack's own; for converted packs worked out once and kept in `state/build/summaries.json`). Checked
  on today's data: the same ways, vertices and rail km, total road km within rounding.
- **Install (2026-10-03):** both Macs run the published app; the build Mac's server keeps a 150 GB
  reserve so its mirror yields to builds. Today's `data/build` is gone from both Macs.
- **Known limit, to fix before high regions:** elevations are i16 decimetres everywhere (base packs,
  the unit steps' arrays, the client's GPU attribute) and clamped at ±3,200 m, as today (the Pico de
  Veleta road shows 3,200 m). Roads in the Andes or the Himalaya reach 5,800 m: they need a wider
  encoding (u16 decimetres from −500 m keeps the size) across the pipeline, the server and the client.
- **The build Mac's disk:** the pack and lo steps read base packs from this Mac's mirror where it has
  them (same content names) and copy only the rest into the agent's pack cache, which drops replaced
  packs on every run and is cleared when an OSM pass starts (its space counts as free for the pass's
  80 GB). The pass copies the planet first when there's room for it and a filtered file of up to
  60 % of it (streaming it ended the first attempt on an SMB I/O error after 31 minutes). The mirror
  doesn't shrink for builds (it evicts only older catalogs' files); on the build Mac it fills only
  past a 150 GB reserve. (§4's single shared LRU, cleared before a pass, is still the better shape.)
- **Phase 5's design (docs/phase5.md, after an Opus review and its re-check, 2026-10-03):**
  - Landmark points get their own worldwide `marks` job and stations their own `stations` job.
    pack(T) writes neither, and the lo packs hold no top landmarks or per-cell counts: those would
    make every road pack depend on worldwide rankings, and a server scan of per-z6 point columns
    (`markdata`) answers every In view statistic, which per-z6 counts can't.
  - The In view statistics come from the server (`/api/marks/view`, the worker's query in Rust,
    exactly), with the sized points the zoomed-out tiles lack (`extra`).
  - Points reach the client in a format of their own (E7 positions, typed columns), not MVT.
  - New folders `markdata/`, `ovdata/` and `work/` (build intermediates, GC'd by job keys); ids
    as in phase5.md.
  - Overlays split into geometry (before units) and details (`ovdata`, after items).
  - The OSM pass gains the `marks`, `summits` and `named` sets; peaks use the worldwide `summits`
    set, so they don't depend on unit borders or the coverage.
  - The zoomed-out drive and ride summaries get their own design after phase 5's step 3.
