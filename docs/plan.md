# Plan: regions as modules, data on the NAS

**Version 7**, 2026-10-03.
- v6 came after the fourth Opus review, and phases 1–5 were built from it.
- v7 adds names by language (§7) and checks every section against the code: what's built is
  described as built, what isn't is marked planned, and where the code falls short of the design,
  §10 lists the gap.
- Opus reviews at each phase may amend this plan when an unforeseen constraint justifies it; §12
  records each change and why.

- Owner: Claude. The user asked me to own the design, to treat their earlier requirements as
  malleable, and to prefer correctness to speed (fix rough edges rather than work around them).
- This file is for my own reference. The user follows the diagram artifact.
- The diagram's source is `docs/diagram/`. To rebuild it, run `python3 page.py out.html`, then
  republish `out.html` to https://claude.ai/artifact/PrjaaDXLuuXGpGr2xt5Vjg.
- Companions: `docs/phase5.md` (landmarks, stations, ferries and overlays by view; the landmark
  jobs; the zoomed-out summaries) and `docs/formats.md` (file formats).

**The idea in one line:** OpenStreetMap comes from one worldwide download, cut by area, and a region
is only an outline saying which areas to build. Every step runs per area, per pack near the
coverage, or once for the whole world, so region size, count and borders never show on the map, and
nothing built depends on how the coverage is divided into regions.

## 1. The user's surface

**The map is always there.**
- A launcher (`tools/launcher`, a LaunchAgent) keeps the server running on both Macs. An idle server
  loads nothing, so it costs almost nothing.
- The user opens `http://localhost:8080`.
- `scenic status` shows what the build Mac is doing, and so does the menu bar item on both Macs
  (Scenic.app, `tools/status`). It shows the state as an icon: building, paused, waiting, nothing to
  build, a problem, or out of touch. Its menu holds the job's progress bar with the time left and a
  checklist of every step to the end, and it sends a notification for every change.
- The server mounts the NAS itself when it's missing, at home: at start, then from its periodic
  check.

**The Regions panel** (Settings → Regions):
- **What it does today:**
  - It shows the regions and their coverage on the map.
  - It makes new regions from administrative areas (levels 2–8 and ISO 3166, from the pass's
    outlines), found by name or by a click on the map (the areas containing the point), each shown
    with its area in km².
  - It renames regions (rebuilding nothing) and removes them.
  - Edits made away from home wait on the Mac and go to the NAS when it's back.
- **From a terminal:** `scenic add` and `scenic remove` do the same, straight to the NAS.
- **Planned:**
  - drawing and redrawing outlines (dragging points; saved as a `.poly`);
  - splitting and merging regions;
  - Geofabrik units and place-and-radius outlines in the panel (recipes take both already);
  - each choice's cost in build time.
- Coverage is drawn on the map. The status bar shows the build Mac's state and offers to reload when
  new data or a new app is in.
- New areas appear when the build publishes its catalog: after every changed area and the roads
  steps. Planned: spatial waves (§8), so areas appear as they finish.

**Translations and descriptions** (§7). The user's two folders on the NAS:
- `translations/` holds finished translation files: the map shows them within a minute or two of
  being dropped there, with nothing rebuilt.
- `descriptions/` likewise holds descriptions.
- Planned: the agent will list what still lacks English, or a description, in each folder's
  `todo/`.

**Everything else is automatic:**
- **Building and refreshing:** whenever the build Mac is awake, at home, and has power (plugged in,
  or on battery down to 30 %).
- Mirroring to each Mac.
- **Installing a newly published app:** each Mac's server picks it up and restarts into it when the
  map is idle.
- Backups.

Publishing the app is a command I run (`tools/app/publish.sh`). Commands are safe to re-run, and
messages say what happened and what to do, in plain words.

## 2. How the work is divided

**Units.** A unit is a z6 tile: about 600 km across at the equator, 300 km at 60°.
- Units are fixed by the OSM pass's cut. A split into z7 or z8 tiles waits until a unit outgrows
  the build Mac (§11).
- Packs for serving are per z6 tile (hi), per z3 tile (lo) and one root (z0–2).

**Where it runs.** All work happens on the build Mac (the M4), except the planet's download, which
the NAS does itself. The jobs (§8 has their order and keys):
1. **The OSM pass**, when the NAS holds a newer planet. It makes:
   - the filtered planet;
   - the worldwide sets (rail, ferries, designated areas, places, outlines, labels, summits, hiking
     routes, heritage-named objects);
   - the administrative and ISO 3166 outlines;
   - the worldwide basemap;
   - pieces: filtered OSM per unit, with a 10 km buffer and ways kept whole;
   - worldwide road values: each way's road id, road length, and offset and direction along its
     road (§6), sliced per unit.
2. **Worldwide jobs, once per pass:**
   - a finished pass's missing sets;
   - hiking routes' ends;
   - each unit's reach: where the roads, rail and ferries of its piece go (§5);
   - summits;
   - place labels;
   - once ever, the worldwide z8 terrain for peaks.
3. **Global-source layers, per z3 pack near the coverage:** terrain, then slope.
4. **Heritage sites and designated areas:** one job over the coverage plus 20 km.
5. **Per unit**, for every unit meeting the coverage: base(U), the ways U owns (a way belongs to the
   unit of its first node) that touch the coverage, with per-vertex elevations, grade and scenic
   channels. Each value is computed once.
6. **Then two chains**, which don't wait for each other:
   - **Roads:**
     - the road → units index;
     - pack(T): tiles, query parts and indexes for each z6 tile T, from the base data of every unit
       within 110 km;
     - lo packs per z3 tile;
     - rail stops and ferries;
     - the root tiles;
     - then a catalog.
   - **Landmarks** (`docs/phase5.md`):
     - candidates and peaks per unit;
     - then worldwide: Wikidata facts and pageviews;
     - the heritage chain;
     - landmark points (marks);
     - area overlays.

Because road values come from the whole planet, adding a region changes nothing outside its own
units and the packs within 110 km of them.

## 3. Storage

NAS root: `personal/projects/scenic-roads/`, on the `personal` share. Its Recycle Bin is on, but
deletions over SMB bypass it (tested 2026-10-02).

```
translations/  descriptions/  the user's drop-ins (descriptions/README.md; todo/: planned)
inputs/        regions/<id>.toml, outlines/ (.poly; geofabrik/), ferries/freq/ (timetables), hold-catalog
sources/       osm/<date>/ (planet, filtered, pieces/, sets/, roads/, outlines, reach, pass), basemap/
               (Planetiler's jar and data), registers/ (the registers snapshot), items/<date>/,
               terrain-z8-v1, legacy/ (today's map's build inputs, until the cutover)
base/          base packs, one per unit
hidata/        per z6 tile: the ways-here index, query parts, climbs, rail lines, zoomed-out summaries
markdata/      per z6 tile: landmark points
ovdata/        per z3 tile: area and park details
global/        worldwide files: road values per unit (roads/), road → units, rail frequencies,
               landmark totals, heritage/, legacy/ (today's converted files)
layers/<layer>/  root, lo and hi packs; basemap/world-<date>.<hash>.pmtiles
work/          build intermediates (not served)
catalog/       <n>.json.zst: every file the map reads, by content name
catalog-held/  a catalog kept back for review (while inputs/hold-catalog exists)
app/           published app versions; current.json, previous.json
state/         status.json (the agent's heartbeat), build/ (manifest, job keys, pending, summaries),
               backups/, logs/ (the planet fetch)
nas/           fetch-planet.sh, which the NAS runs itself
```

**Immutable, content-named files.**
- Every built file is named `<logical>.<hash16>.<ext>` and never rewritten.
- A write goes to `<name>.tmp`, is read back and checked against its hash, then renamed.
- An unchanged file keeps its name, so a rebuild uploads only what changed.
- **Catalogs** list every file the map reads: one zstd JSON per publish, with zstd's content
  checksum. Readers take the highest that decodes.
- **The build manifest** (`state/build/manifest.json`) lists everything built: sources, work and
  outputs.
- **Job keys** (`state/build/jobs.json`) say what each output was made from.

**Deletions go through SMB.** They're permanent on this share. The build Mac can't use SSH
unattended, because 1Password asks to approve every new session.

**Packs, never one file per tile.** SMB manages about 80 random reads per second per file.
- **Our layers** (roads, rails, terrain, slope, trees, labels, overlays, marks, stations, ferries)
  use packs.
  - A pack is a header and meta, then the blobs (identical blobs are stored once), then an index of
    (tile key, offset, length, raw length, 64-bit content hash) sorted by key.
  - **Root pack:** z0–2. **Lo packs:** z3–8, one per z3 tile (roads and rail z4–8). **Hi packs:**
    z9–14, one per z6 tile.
  - Hi tiles exist only near the coverage: terrain and slope within 20 km of it, the rest for the
    built units.
- **The basemap** is Planetiler's worldwide PMTiles file, one per pass. Its sea is deduplicated. The
  server serves it tile by tile.
- **Base packs, hidata, markdata and ovdata** are sectioned files (named arrays): mapped when local,
  read from the NAS in pages (§4).

**Writers.** The build Mac's agent writes everything built: one agent per Mac (a lock), and build
steps merge their manifest changes under a lock. The exceptions:
- **Region recipes:** any Mac's server writes them. A new recipe is created exclusively (`O_EXCL`), a
  rename is rewritten through a temporary file, and a removal is renamed to `.removed`. `scenic add`
  and `scenic remove` write them too.
- The user's folders.
- **The app:** `tools/app/publish.sh`, run by hand.
- **The planet:** the NAS's own fetch.

**GC** runs daily, in the agent.
- **Roots:** the newest catalog, every catalog of the last 14 days, and the build manifest.
- An unreferenced content-named file goes once it's also older than 14 days. Catalogs older than 14
  days go too, except the newest, and stray `.tmp` files go after 2 days.
- It sweeps only the folders catalogs index. Translations, descriptions, inputs, state, app, nas and
  sources are never swept.
- Gaps: see §10. A retired pass's sources stay, and built data stays after its region is removed.

**Backups.**
- Daily, the agent copies the user's folders and `inputs/` into a content-addressed store under
  `state/backups/`. It writes a dated list on days something changed and keeps lists for 30 days.
  The store is mirrored to the build Mac, and `todo/` isn't backed up.
- Everything else can be regenerated.
- A separate share with DSM snapshots would be cleaner. That needs the user in DSM, so it's offered,
  not assumed.

## 4. Macs

**Build Mac (M4, 48 GB).**
- **The agent:** `scenic agent` runs under the launcher, as a LaunchAgent with `ProcessType
  Interactive`. Without it, launchd throttles the agent.
- **One job at a time.** Each job is a child process group at utility priority (`taskpolicy -c
  utility`; `-b` would confine it to the efficiency cores, ~17× slower).
  - Rust steps take half the cores when the user is active as the job starts, and all of them when
    idle (`RAYON_NUM_THREADS`).
  - osmium, Planetiler and the Python steps take what they take.
- **Power:** CPU jobs run on mains power or on battery down to 30 % charge, then pause until the Mac
  is plugged in. Every job also needs the NAS.
- **Sleep:** each running job holds `caffeinate -i -s -w <pid>`: no idle sleep, on battery too, and no
  system sleep on mains power. It's dropped while the job is paused, so a paused Mac can sleep.
- **Caches** (`~/Library/Application Support/scenic/agent/cache`):
  - AWS's raw terrain tiles;
  - canopy 10° files;
  - the per-vertex DEM cache: today's, copied once from `sources/dem-cache/` (the seed), and each
    unit's samples from its last run (`dem-units/`), so a vertex is sampled from the DEM servers
    once;
  - Wikidata and pageview caches;
  - the registers snapshot, extracted;
  - the pack cache: base packs for pack(T) not in its mirror, pruned every run and cleared when an
    OSM pass starts.
- **Its own map** is served from its mirror, which keeps a 150 GB reserve so builds have room.
- **An OSM pass** starts with 80 GB free (the pack cache counting as free). It copies the planet to
  the SSD first when there's room for the planet, a filtered file of up to 75 % of it, and 10 GB;
  otherwise it reads the planet from the NAS.
- It may be asleep, away or unplugged at any time (§8, Interruptions).

**App Macs (both laptops).**
- **The launcher** (`tools/launcher/launcher.c`) is built once and never rebuilt.
  - It runs the command in `~/Library/Application Support/scenic/run/<name>` (server, agent,
    status) and restarts it when it exits.
  - Pointing a run file at the repo's build runs a development server.
  - The NAS reads fine from it (tested on both Macs, 2026-10-02). If a future macOS asks, Full Disk
    Access for the launcher is the fallback.
  - A server started any other way, such as from the desktop app's preview, waits on macOS's
    network-volume prompt. Test servers run from the shell.
- **The updater** checks the NAS's `app/current.json` every five minutes.
  - It copies a newer app to local disk, checking every file's SHA-256.
  - It restarts into it when the map is idle, and keeps checking while it waits.
- The app reads the current and the previous form of every file kind: by format version, or by which
  sections a file has.

**Server.**
- **Finding the NAS:** `getfsstat(MNT_NOWAIT)`, never `statfs`.
  - At home (when `fishandchips.local` answers on port 445), it mounts the share with `osascript`
    and the Keychain, at most every five minutes.
  - It mounts by the LAN name `fishandchips.local` when the Keychain has its password, else by the
    bare name. The bare name resolves to the NAS's Tailscale address, whose userspace networking held
    SMB to 12 MB/s.
- **`nsmb.conf`** makes the share a soft mount, under both names.
- **NAS access:** reads go through a bounded I/O pool with timeouts.
  - An overrun trips the breaker (an offline banner) only when a quick probe of the share also fails,
    so on a busy link a slow read fails alone. One prober watches for the NAS's return.
  - NAS files are read in 1 MB pieces with their handles kept open, and are never mmapped.
  - Base packs and hidata are read in 256 KB pages for the records a request touches, or as whole
    sections for what a query scans.
- **Failures are never cached.** A read the NAS can't answer is a 503: the client asks again after 2
  s, doubling to a minute, and at once when the NAS is back. Nothing built from a failed read is
  kept.
- **Memory:** caches are keyed by content name, with budgets: 512 MB of base-pack views, 1 GB of
  sectioned files read whole, 384 MB of pages, 1.5 GB of whole sections, and at most 128 open NAS
  files. What's read from the NAS is dropped once the mirror has the file.
- **Offline start:** the last catalog and every pack's index stay local.
- **In use** means any request in the last ten minutes, except the status polls (`/api/catalog`,
  `/api/ping`, `/api/build`).
  - An idle server loads nothing; warming starts at the first request (starting isn't a use).
  - It checks the NAS for a newer catalog every 30 s while in use, every 10 minutes otherwise.
- **URLs and ETags:**
  - Data URLs carry a content version (`?v=`: a file's hash, a layer's packs' content names, plus
    the translations version for named data).
  - A versioned response is cached for good (`immutable`) while that version is current, else it's
    revalidated.
  - ETags are content hashes, combined for named tiles with the versions of the translations they
    use.

**Mirror, per Mac.**
- At home, each Mac copies every file the current catalog lists, in the background: one file at a
  time, in large sequential reads, paused while the build Mac runs a job. That makes the map as fast
  as from local files, and keeps it working away from home.
- **Copy order:** small worldwide files, then root and lo packs, hidata and road values, base packs,
  hi packs, and the rest (the basemap). The most recently used go first within each group.
- **Budget:** the free space minus a reserve (50 GB; 150 GB on the build Mac).
- **Eviction:** when space runs short, files no recent catalog references go, least recently used
  first. Files of the last two catalogs never go.
- Planned: "Keep this view", for trips, once the coverage outgrows the disks.

## 5. Coverage and regions

**Recipe** (`inputs/regions/<id>.toml`): an `id`, a `name`, and an `outline`, a list whose union is
the region. Each entry is one of these:
- `osm:<relation>`: an administrative or ISO 3166 area from the pass's outlines. Neighbours share
  borders exactly, so their unions are seamless, and a 1 km buffer covers coasts.
- `geofabrik:<id>`: Geofabrik's outline, read from `inputs/outlines/geofabrik/<id>.poly`. It's put
  there by hand; nothing fetches it. Neighbours overlap a little.
- `poly:<file>`: a `.poly` in `inputs/outlines/`.
- `place:<lon>,<lat>,<km>`: a circle of up to 500 km, drawn as a 64-gon.

**Coverage** is the union of the outlines.
- **What gets built:** a feature is built if it touches the coverage. For a way, that means any node
  inside, as Geofabrik does. Ways are kept whole, so roads end a little past the edge.
- **Units:** chosen by their reach, worked out once per pass from each piece's roads, rail and
  ferries (`sources/osm/<date>/reach`, `pipeline::reach`).
  - Most of a unit's ways stay within its tile + 20 km; of those, the box of the ones it owns is
    kept. The few that reach further (ferries, long rural roads) are kept whole.
  - A unit is built when its owned box meets the coverage, or one of its own long ways touches it.
    So a road starting in a tile far from every outline is built when it enters the coverage.
  - The unit step then keeps exactly the ways touching the coverage (a cell grid per outline).
- **Builds depend on coverage, never on regions.**
  - Renaming a region rebuilds nothing.
  - Shapes enter keys by their geometry alone, never by their region, so renaming, splitting or
    merging regions with the same outlines reruns nothing.
  - A job is keyed on the coverage inside the box it reads (`Coverage::fingerprint`: which edges
    cross the box, and whether a corner is inside), so changing an outline reruns only what its
    changed part reaches: the units (their tile + 20 km, and whether each long way touches the
    coverage), the terrain packs, the landmark candidates and the heritage-sites job.
- **Shrinking** leaves global-source tiles in place, which is harmless.

**Today's set:** 34 recipes (`tools/cutover/regions`).
- 31 are Geofabrik outlines, the legacy builds' own.
- Gibraltar, Saint-Pierre-et-Miquelon and Singapore are `osm:` relations.
- Their coverage meets 482 z6 units.

**By location.** These rules depend on where a thing is. Today each is written into its step, and the
units' ones (DEM order, densification, road network codes) are versioned by area in
`pipeline::rules`: a unit's key names the versions of the rules where its ways go, so a changed
rule (its version bumped) reruns only the units it applies to. The plan is modules declared per ISO
3166-1 country or 3166-2 subdivision, with defaults:
- DEM order (`dem/sample.py`) and densification spacing (8 m in North America and Japan, 15 m
  elsewhere: `extract`);
- heritage registers (the snapshot, `dem/heritage.py`);
- timetables:
  - rail: Mobility Database feeds, `dem/railfeeds.py`, with no job yet;
  - ferries: `inputs/ferries/freq`;
- road network codes (`extract`) and their colours (`web/src/mapschemes.ts`);
- leaf-type source: EEA in Europe, NALCMS in North America, none elsewhere, with no job yet;
- the languages spoken there, for names (§7; today `names::area`'s boxes);
- credits (`web/src/ui/strip.ts`; catalogs carry none yet).

Planned for a country without a module: defaults (FABDEM, no register, colours by road class), with
`scenic status` saying which defaults each region uses.

**No seams:**
- **Region borders:** no step knows about them.
- **Tile edges:**
  - per-vertex values are computed once, by the owning unit;
  - road values come from the planet;
  - climbs come from the 110 km halo;
  - queries join road parts by offset.
- **Where coverage ends,** roads end.

**IDs:** OSM type and id. Way ids fit u32 until the 2040s; node ids already don't, so points use u64.
- Each z6 tile's hidata has a "ways here" index (`here`, `ends`): every way whose box meets the tile,
  with its owner unit and its index in that unit's base pack.
- Details and profiles ask with the id plus the location. The location picks the tile, or one of
  its eight neighbours.
- Within a draw class, RT lines are sorted by id, so delta coding keeps tiles small.

## 6. Pipeline

### The OSM pass

1. **The NAS fetches the planet.** `nas/fetch-planet.sh` is on the NAS (not yet in the repo).
   - DSM's Task Scheduler runs it daily. It does nothing until the newest planet there is six months
     old, or `nas/fetch-now` exists.
   - It downloads the newest planet whose MD5 is published, from a mirror, resuming after any
     interruption.
   - It checks the MD5 and moves the file into `sources/osm/<date>/`.
2. **Copy to the SSD.** The M4 copies the planet to its SSD, resumably, when there's room (§4).
3. **Filter.** One `osmium tags-filter` of the planet into (a), the filtered planet. Its filter is
   what the pipeline reads, including `landuse=forest` and the heritage-named objects. (a) is kept
   on the NAS until the next pass, so a new tag needs no new download.
4. **Sets and the basemap's input (b)**, both filtered from (a).
   - Sets are versioned (a changed filter is a new set, `summits-v2`).
   - `pass-sets` makes a finished pass's missing sets from (a).
5. **Outlines:** administrative levels 2–8 and ISO 3166 areas, assembled into polygons. They go in a
   sectioned file, with simplified copies for the panel.
6. **Basemap:** Planetiler on (b), worldwide, giving `layers/basemap/world-<date>`. Its jar and data
   (Natural Earth, water polygons, lake centerlines) are pinned in `sources/basemap/`.
7. **Cut (a) into z6 pieces.**
   - The cut is a depth-first quad tree, four outputs per osmium run. osmium keeps about 4 GB of id
     sets per output on a planet-sized input, so a wider cut wouldn't fit in 48 GB.
   - It keeps ways whole, completes multipolygons and adds a 10 km buffer.
   - Each piece is uploaded with its road links (the pairings below) as soon as it's cut. A tile's
     file goes once its four quarters are cut.
8. **Road values** (below).
9. **Finish:** the pass's summary is written, and older passes retire: their entries leave the
   manifest.

### Global-source layers

- **Terrain** is built per z3 pack near the coverage.
  - **z9–12:** within the coverage plus 20 km (viewsheds see 15 km). Zoom is capped by latitude so
    pixels stay ≥ 15 m: z12 to 67°, z11 to 79°, z10 beyond.
  - **z3–8:** the whole z3 tile. z8 and coarser are made again from their children where those
    exist, since AWS's coarse levels come from coarser sources.
  - **The root (z0–2):** from the lo packs.
  - **Source:** always AWS's raw tiles, from a raw-tile cache on the build Mac, repaired by
    `repair_terrain`. Processing a processed tile isn't idempotent, so stored tiles are never inputs.
  - **Below zero:** values are clamped to 0. Planned: a sea mask from the pass's water polygons, so
    that polders and depressions keep their depth.
  - Deterministic: reruns give identical packs.
- **Slope:** z11 and coarser are stored, from transient z12 Horn slope. The server makes z12 on
  demand with the same encoder and an LRU (56 % of the full archive).
- **Worldwide z8 terrain** (`sources/terrain-z8-v1`, once, not served): every z8 tile, repaired, with
  each tile's maximum. Peaks read it, so their prominence and isolation don't depend on coverage.
- **Grids (z11):** land cover, canopy and cover, for analysis only (not served).
  - Each unit's job makes the grid tiles its packs lack: `landcover.py --only`, and the scenic canopy
    step.
  - It uploads them as its own z6 tile's `grid-*` hi packs.
- **Trees** (cover, height, leaf type): today's packs, converted. Planned: a job for new coverage.
- **Area overlays:** see `docs/phase5.md`. The `overlays` job runs after marks, because it needs the
  World Heritage dots' ids. It's off until the heritage switch; today's converted packs serve.
- **Buildings (phase 7):** Overture plus official data, giving z13–14 within the coverage.

The server builds missing deeper terrain and slope tiles from their ancestors.

### Worldwide road values (in the OSM pass)

**One chaining** serves whole roads, strokes, drives, hover, profiles and rides.
- **How ways pair:** at each node, way ends of the same kind pair by mutual best continuation.
  - Same ref (any shared token of a multi-ref; two ways whose refs share none never pair).
  - Else same name.
  - Else same class when both are unnamed (no name and no ref).
  - Else, a way with neither continuing one that has a name or ref, of its class within 35°.
  - The straightest go first, within 100°.
  - Oneways pair only in their direction.
  - Ties go to the higher level, then the straightest, then the lower way id.
  - Rail pairs by line identity; ferries and ways with fewer than two vertices never chain.
- **Per piece:** the pairing at a node depends only on the ways through it, so it's computed per
  piece for the nodes inside the unit. That's exact, since a piece holds every way through those
  nodes.
- **Roads:** pairings form paths and cycles. A union-find joins them, and an ordered walk (from a
  path's end on its lower way id; a cycle from its lowest way id) gives each way:
  - its road id: the road's lowest way id;
  - the road's length;
  - the way's offset and direction.
- **Storage:** per unit, `sources/osm/<date>/roads/<u>`. Each unit job copies them into
  `global/roads/<u>`, with `byroad`: its ways sorted by road, so a profile finds a road's ways with
  one binary search.
- Lengths include parts outside the coverage: a road's length is a fact about the road.
- Elevation smoothing keeps today's local continuation rule (`Net.cont`); it runs in base(U).

### Per unit: base(U)

The unit job runs today's steps on a unit-sized folder, wiped at each run:
1. **extract:** on U's piece, U's ways that touch the coverage, by today's rules. Rail tracks without
   a route relation are kept by type.
2. **Elevations:** `sample.py`, DEMs by location, on U's slice of the per-vertex DEM cache (the seed,
   and the units' kept samples, which win). U's samples are kept afterwards for its later runs and
   its neighbours'.
3. **Heritage:** the sites and designated areas of the heritage-sites job's slices within U + 30 km.
   `areaflags.py` rasterises the areas onto U's grid.
4. **Terrain and grids:** terrain z11 and the grids, staged from the packs (as the build manifest has
   them when the unit runs, which is what its key names). Missing grid tiles are made.
5. **`tile elev`:** clean-up and grade, with junction context from the piece.
6. **scenic:** `scenic-metrics` prep, canopy, view, buildings (today's Overture boxes) and flags, for
   every sample.
7. **Output:**
   - the base pack: per-vertex arrays and records, indexed by z9 sub-tile;
   - `global/roads/<u>`;
   - `global/roaden/<u>`: the roads' own English (OSM's `name:en` where it isn't the name);
   - `grid-*` hi packs for U's tile when it made grid tiles.
   - A unit left with none of its ways in the coverage drops its base pack, road values and
     English.

Elevations are u16 decimetres from −500 m (to 6,053.5 m). Packs made before 2026-10-03 hold i16,
clamped at ±3,200 m, and readers take both.

Landmark candidates and peaks have their own per-unit jobs (`docs/phase5.md`).

### Per z6 pack: pack(T)

- **Reads:** the base packs and road values of every unit whose box meets T + 110 km.
- **Writes:**
  - road and rail hi tiles, z9–14;
  - T's hidata:
    - the ways-here index;
    - query parts: for each road through T, its 100 m samples and channels with their offsets
      (drives, rides);
    - climbs that start in T;
    - rail lines;
    - the zoomed-out summaries (`docs/phase5.md`).
- **Lo packs** per z3 tile hold roads and rail z4–8, made from the base packs.
- Landmarks, stations, ferries and overlays come from their own jobs, so no road pack depends on
  worldwide rankings.

### Worldwide steps after the units

- **Road → units index** (`global/roadunits`): for whole-road hover.
- **Labels** (per pass, worldwide): places, states, seas, lakes and parks from the pass's labels set
  (`dem/labels.py`, with each thing's own English).
- **Rail stops:** from the rail set, for the built units' tiles + 20 km.
- **Ferries:** worldwide, from the ferries set and `inputs/ferries/freq`.
- **Landmarks:** candidates and peaks per unit, then Wikidata facts and pageviews, marks, and
  overlays (`docs/phase5.md`).
- **Planned:**
  - rail service: trains a day on the worldwide track graph, with timetables processed once per
    feed version. Today the map has today's converted `global/railfreq`;
  - names todo (§7);
  - descriptions todo (§7).

### Job keys

A job's key is its step version plus what it reads, mostly by content name. The ones that cascade:
- **terrain (per z3 pack):** the z6 tiles to build, and the coverage inside its z3 tile + 20 km;
- **slope:** its terrain pack;
- **heritage-sites:** the pass, its areas set, the registers snapshot, the coverage;
- **unit:** its piece, the pass's road values, the coverage as its ways meet it (inside its tile +
  20 km, and whether each long way touches it), the versions of the location rules where its ways
  go, the terrain and grid hi packs within 30 km, and its heritage slices;
- **pack(T):** every base pack and road-values file within T + 110 km;
- **lo:** the base packs and road values within 110 km of its z3 tile.

The landmark jobs, stations, ferries and overlays: `docs/phase5.md`.

An output identical to before keeps its content name, so jobs keyed on it stop there.

### Served

| What | How |
|---|---|
| our layers | the pack for the tile; names attached |
| basemap | tiles from the catalog's PMTiles archives (`/tiles/base`); names attached |
| drives, rides, rail lines | parts from the hidata in view plus a margin of half the window, joined by offset; zoomed out (a view wider than ~1,200 km), from the 500 m summaries (`approx`) |
| whole road (hover) | the road's parts, through the road → units index |
| details, profiles | by OSM id plus location (the ways-here index → base pack) |
| landmarks, overlays, stations, ferries | by view (`docs/phase5.md`) |
| `/api/meta` | the map's meta, added up from the units' summaries (each base pack's own) |
| `/api/catalog` | the catalog's number, layers and zoom ranges, versions, the NAS and build state |

A layer's zoom range comes from its definition (terrain z0–12, slope z0–11, …).

**Browser:**
- our layers' tile URLs are as before;
- the basemap and labels come as tile URLs, the coast worker included;
- landmarks, stations and overlays come by view;
- English comes from the server;
- the page switches to a new catalog in place (it polls `/api/catalog` every minute).

## 7. Names and descriptions

**One pipeline for every named thing:** places, states, seas and lakes, rivers and waterways, parks
and protected areas, natural features, stops and sights, heritage sites and areas, special places,
Indigenous lands, stations, rail lines, ferries, and road names (in the app's text; no road labels
on the map).

**Display.** A name shows as a **main** label and an optional smaller **sub** line: sub under main on
map labels, "main (sub)" in the app's text. Sub shows only when it truly differs from main:
- not the same but for accents, case, punctuation or spacing (Montréal);
- not one of main's own parts ("Alba / Scotland" and "Scotland").

**Today (built).**
- **Tables:** the server reads the translation work's nine area tables: `translations/<area>/`,
  `places-<area>.jsonl` and `roads-<area>.jsonl`, for jp, tw, hk, sg, fr, ib, pt, na and gb.
- **A name's area** comes from where its thing is (`names::area`). That is the translation work's
  box function, within rectangles around today's coverage; elsewhere there's no area, and only own
  English shows.
- **Lookup:** the translation line comes first, else the thing's own English as sub.
  - A road reads the roads table first, then the places table; everything else reads the reverse.
  - Lines not done (`via` todo or skipped) are left out.
- **Arrival:**
  - The server copies files from the NAS once they've held still for 10 s, and compiles the local
    copy.
  - It checks every minute while names are being shown, and for ten minutes after it starts. A drop
    shows within about a minute and a half, with nothing rebuilt.
  - Tiles' ETags include the versions of the tables they use.

**The design (v7, not built yet): names by language.**

**A thing's English, in order:**
1. **Its own, from a source about that thing.** The sources:
   - OSM's `name:en`;
   - its romanised name (`name:ja-Latn`, `name:zh-Latn-pinyin`…), or its kana reading romanised by
     rule (Hepburn);
   - its Wikidata item's English label;
   - its English Wikipedia article's title;
   - a heritage register's or UNESCO's English.

   It belongs to that thing alone. It wins over its name's translation, and it's never copied to
   other things with the same name. A citable source is shown even where it's wrong ("Leclerc tank"
   for one "Monument aux Morts"): the user prefers that to overwriting it.
2. **Its name's translation:** the line for its name, kind and language. The lookup tries first the
   language OSM gives the name (a `name:br` equal to `name` makes it Breton), then each language
   spoken where the thing is, in order.
3. **Else none,** and the name goes on the to-do list.

**Translations are keyed by name, kind and language, the same way in every script.**
- **Kind:** road, settlement (city to hamlet) or other. The same words can be a hamlet that keeps its
  name and a mill to translate ("Moulin").
- **Language:** the language the name is read in.
  - A line holds for one language or several. A name that could be in any of the local languages,
    with the same English in each, is filed under all of them, so nothing claims a language it can't
    know.
  - 中山 has a Chinese line (Zhongshan, as Taiwan reads it) and a Japanese one (Nakayama). "Lac Bleu"
    has one French line, used in France and Quebec alike.
- **The languages describe the name, not a region.** So a line serves every place where they're
  spoken, and stays valid when a region gains a language.
- **The cost:** the same name in two languages (a "Hotel Central" in Spain and another in Italy) is
  translated once in each.

**Languages spoken where a thing is**, in order:
- Per ISO 3166-1 country and, where it matters, 3166-2 subdivision (the pass's outlines).
- From CLDR's territory data (official and widely used languages), refined by the country modules
  (§5): Quebec French then English, Catalonia Catalan then Spanish, Wales English then Welsh,
  Brittany French then Breton.
- It's the only place location enters: it sets the lookup order and which to-do list a name goes on.

**Files:** `translations/**/*.jsonl` (not `todo/`), one line per translation, wherever the file is:
`{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu", "sub": "Blue Lake",
"via": "agent:haiku"}`.
- **Fields:**
  - `n`: the name exactly as in OSM.
  - `kind`: road, settlement or other, or a list of them.
  - `langs`: the languages it holds for.
  - `main`, `sub`: the display (`sub` null: nothing under main).
  - `via`: how it was made (free text, for the record).
- Where lines share a name, kind and language, the later file name wins.
- A file is read once its size and modification time have held for 10 s; an unfinished last line is
  ignored.
- A tile's ETag includes the versions of the languages spoken within it.

**To-do:** `translations/todo/<language>.jsonl`, written by the agent after each build.
- **What's listed:** a name, when something in the coverage has it, no English of its own and no line
  in any language spoken there.
  - Names already in English aren't listed. Where English is spoken, a name without another
    candidate language's signs (script, accents, words) counts as English.
- **Each entry:**
  - the name and its kind;
  - the candidate languages: those spoken where the things lacking English are, narrowed by OSM's
    language tags;
  - how many things lack English, and one of them (its OSM id and position);
  - a priority (fame, place class and population, road class).
- Entries are sorted by priority. Each goes on its first candidate's list, and the answer says which
  candidates it holds for.
- Entries carry no other thing's sourced English: that belongs to its thing.
- **The brief:** `translations/todo/README.md` holds the line format and the conventions:
  - title case;
  - settlements keep their own name unless they have a well-known English one;
  - Hepburn without macrons in Japan, Hanyu Pinyin in Taiwan, the Hong Kong government's
    romanisation.

**Today's tables are converted once.** They come from the translation work (`place-translations`,
branch `claude/exciting-cori-x46sfk`, `out/display/`, copied 2026-10-02). After the conversion, the
box function and the per-area folders go.
- **Lines made from the name itself become lines in the new format.** That covers names kept as they
  are, split, rules on their words, Taiwan's romanisation of the characters, and agents'
  translations.
  - Each holds for the languages spoken where its name's things are.
  - Places' lines hold for settlements and other things; roads' lines for roads.
- **Lines taken from particular things' sources are dropped.**
  - `via: osm` lines carry the `name:en` that most things with the name agreed on: about 197,000
    lines, most in Japan and Taiwan. Japan's romanisations of OSM's kana readings go too.
  - Each of those things shows its own English from its own tags. Things with the name but no
    English of their own go on the to-do list.
- **Lines not done** (`todo`, `skipped`) are dropped.
- **Names whose converted lines disagree** are split by kind or corrected one by one: a settlement's
  line against a feature's, one thing's English filed for the name, typos.
- **The build must also carry each thing's own English and its language tags** into what it serves.
  Today roads' own English exists only for today's coverage (the converted `global/legacy/road-en`),
  and no job reads Wikidata's English labels yet.

**Descriptions.**
- **Today (built):**
  - Descriptions are `descriptions/**/*.jsonl`, lines `{"qid", "long", "src"}`, or `{"id":
    "n123"|"w123"|"r123", "long"}` for things without a Wikidata item. `"drop": true` removes one.
  - They're laid over the popups of details, landmarks and area overlays when serving: matched by
    the record's `qid`, else its first `wikidata`, else its `osm`. Later file names win, and `src`
    is credited.
  - Today's written descriptions are in `descriptions/heritage/`, prefixed 1–4 to keep their order.
  - They arrive as translations do.
- **Planned:** `descriptions/todo/`, written by the agent.
  - It lists the landmarks and areas that have an English Wikipedia article or a register entry and
    no description, by fame, so how far down to go is a choice.
  - I write them on request with Sonnet writers: from the article, or researched with credited
    sources where the article is about something else.

## 8. Building

**Regions:** recipes in `inputs/regions/`. The agent works out what they change from job keys; there
are no request files.

**Scheduler.**
- **The plan:** every step's targets come with their keys (`state/build/jobs.json`). A target is
  stale when its key changed.
- **A job** is one step over a batch of stale targets: terrain 1, slope and lo 2, unit 6, peaks 12,
  pack 16, pois 24, the worldwide steps all. So a failure or a new app costs one batch.
- **Order:** the agent starts the first job that can run, in plan order.
- **A newly installed app:** the running job finishes under the old one, nothing new starts, and the
  agent exits so the launcher starts the new one.
- **Retries:** a failed job is retried after 10 minutes, doubling to 6 hours. The orphans of a crashed
  agent are stopped at start (only when their leader's start time proves them ours, or the leader is
  gone and every member started after the job).
- **The heartbeat:** the agent writes it locally with each loop (about every 20 s), and to the NAS
  (`state/status.json`) when it changes or every five minutes; the user's idle seconds don't count as
  a change, only whether they're at the Mac. It holds the job, its progress (from the job's `progress:` lines) with the time left, and a checklist
  of every step to the end.

**Order:**
1. **The OSM pass**, when the NAS holds a newer planet than the newest pass.
2. **The pass's worldwide jobs:**
   - `pass-sets`;
   - hiking routes' ends;
   - the units' reach (`reach`);
   - `terrain-z8` (once);
   - summits;
   - labels.
3. **The regions' build:**
   - terrain, then slope (nothing else in the regions' plan runs while terrain is stale);
   - heritage-sites;
   - every stale unit.
4. **Two chains**, each contributing its first stale step:
   - **Roads:** road → units index, pack, lo, stations, ferries, terrain and slope roots.
   - **Landmarks:** pois, peaks, items, heritage, marks, overlays. Heritage and overlays are off
     (`HERITAGE_JOBS = false`) until the cutover's comparison.
5. **A catalog** once the roads chain is done: a new one whenever the served files change. While
   `inputs/hold-catalog` exists, it goes to `catalog-held/` instead.
6. **Daily:** backup and GC.

**Planned:**
- spatial waves: a cluster of units plus its halo, published as it finishes;
- stale work after step-version bumps, done oldest first in idle time;
- registers, Overture and timetables fetched every ~6 months.

**Interruptions.** The build Mac may be asleep, away or unplugged at any time, or close its lid
mid-job. Nothing depends on it being available at a given time.
- **No deadlines.** Until work is done, the map serves the last catalog.
- **Conditions per step:**
  - Every job needs the NAS. CPU jobs also need power: mains, or the battery at 30 % or more.
  - When a condition lapses, the agent pauses the job (`SIGSTOP` to its process group) and resumes
    it (`SIGCONT`) when it holds again.
- **Sleep** suspends every process. Open SMB handles often don't survive it, so a job that touches the
  NAS is restarted after wake.
- **Kills** lose the current batch: keys are recorded only when a whole batch succeeds.
  - A batch takes minutes to tens of minutes.
  - The OSM pass is a chain of stages with completion markers; its filter and basemap stages take
    an hour or more each.
  - The planet's copy and the mirror resume by byte range; uploads restart their `.tmp`.
- **Atomic writes** (§3): anything half-written is never referenced.
- **Caches are disposable:** they refill from the NAS and the original sources.

**Status.**
- `scenic status`.
- The app's status bar: the build Mac's state, the NAS, and new data or a new app in.
- The menu bar item. It asks the local server (`/api/build`): this Mac's agent's status when it runs
  here, else the NAS's copy.
- Each says what's waiting and why ("Build Mac last seen yesterday; Kanto waits for it to be plugged
  in at home").

**Determinism:** the same inputs give the same bytes: sorted outputs, no hash-map order, fixed
reductions. Checked by hand so far (terrain, slope, units, candidates); planned: a "build twice,
compare hashes" test per step.

**Validation:**
- **Built:** every upload is read back and checked against its hash, and a catalog fails on a
  missing file.
- **Before a switch:** a held catalog is compared with the current map by hand (`catalog-held/`).
- **Planned:** files decode; values in plausible ranges; a summit tolerance for 30 m DEMs (Snowdon
  reads 1,040 m for its 1,085 m); and a unit that loses more than 20 % of its ways without its
  coverage shrinking holds the publish (the previous catalog keeps serving, and the status says so).

**Format bumps:** publish an app that reads both forms, then the data; drop the old reader once
everything is rebuilt.

**App publishing:** `tools/app/publish.sh`, run by hand on this Mac with the NAS mounted. It:
1. builds the server and pipeline;
2. runs the crates' tests, the type check and the web build;
3. smoke-tests a server on a spare port against the NAS's catalog;
4. writes `app/<version>/` and `app/current.json` with every file's SHA-256.

`publish.sh --rollback` swaps `current.json` and `previous.json`. The agent never fetches from git.

**Programs:**
- `scenic` is the user's command and the agent;
- `scenic-build` holds the build steps;
- `server` serves the map;
- `extract`, `tile` and `scenic-metrics` are today's steps, which units run;
- the app also carries `dem/` (the Python steps), Scenic.app (the menu bar item), `web/` and
  `fonts/`.

## 9. Sizes

**Measured** (the 2026-09-28 planet's pass, and today's converted data on the NAS, 2026-10-03):

| | |
|---|---|
| planet | 95.1 GB |
| filtered (a) | 60.6 GB (64 % of the planet) |
| sets | 10.9 GB |
| outlines | 2.7 GB |
| OSM pieces (all land) | ~58 GB (measured as the cut finished) |
| basemap (worldwide) | 28.6 GB (its input 16.3 GB; Planetiler needs ~6× its input while it runs; 46 min) |
| today's 34 regions, converted | base packs 26.4 GB (181 units), hidata 5.8 GB, layers 80.3 GB (including both basemaps), markdata 0.1 GB, global 1.6 GB |
| today's build inputs (`sources/legacy`) | 105.4 GB, until the cutover |
| NAS | 8.5 TB free of 35 TB |

**Estimated:**

| | today's coverage | whole world |
|---|---|---|
| road values | ~8 GB (all land) | ~8 GB |
| base packs | ~30 GB | ~300 GB |
| our layers (terrain and slope for roadless coverage too) | ~150 GB | ~1.2 TB, with buildings |
| sources kept per pass | ~200 GB | the same |
| an app Mac's mirror | everything, ~200 GB (M1: budget-limited) | budget-limited |

A retired pass's sources stay until the GC gap (§10) is closed, so each pass adds about 200 GB.

## 10. Phases and status

At each phase's end an Opus agent reviews the work against this plan.

1. **Foundations, on today's data: done.**
   - The `store` crate: packs, catalogs, content naming, the I/O pool, mounting, the mirror.
   - Today's data converted into NAS packs, base packs and a catalog.
   - The server serves from them: lazy, paged, by id plus location, offline start, names attached.
     The client follows.
   - Golden checks: Singapore (400/400 ways equal) and nine places against the legacy server (way
     info, profiles, every tile layer and popup details equal).
   - Left: a speed check against the legacy measurements (README) once this Mac's mirror is
     complete.
2. **Agent and moves: done.**
   - The agent (recipes, heartbeat, conditions with the battery rule, batches, progress and
     checklist, backups, GC).
   - Both Macs' data moved to the NAS. Local copies deleted, and the legacy `data/build` gone from
     both Macs.
   - Descriptions moved to `descriptions/`. The menu bar item.
3. **The OSM pass and global-source layers: mostly done.**
   - **Built:**
     - the pass (filter, sets, outlines, the worldwide basemap, the cut, road values);
     - terrain and slope per z3 pack;
     - the worldwide z8 terrain;
     - grids inside the units.
   - The 2026-09-28 planet's pass is running: the cut is finishing, then road values.
   - **Not built:** trees for new coverage; the sea mask.
4. **Per-unit pipeline and rankings: mostly done.**
   - **Built:**
     - base(U): the pilot (Northumberland and the Scottish Borders) matched today's data, with the
       same ways, elevations within 1.8 m and every scenic channel and flag;
     - heritage sites and flags;
     - labels, stations, ferries;
     - the landmark jobs: pois, peaks, items, marks.
   - **Written and checked, off:** the rest of the heritage chain and its consumers. It reproduces
     today's 17 outputs, and every overlay pack byte for byte; fame differs where today's was stale.
   - **Not built:** rail service, names and descriptions todo, determinism tests, validation.
5. **Browser: done.**
   - Built:
     - landmarks, stations, ferries and overlays by view (In view answers equal to the legacy
       worker's in 163 views);
     - zoomed-out queries;
     - the Regions panel (add, rename, remove);
     - the status bar;
     - catalog switching.
   - **Not built:** drawing, splitting and merging regions; "Keep this view".
6. **Cutover: under way.**
   1. Today's 34 recipes are installed, with `inputs/hold-catalog`.
   2. The agent builds them after the pass: terrain, slope, heritage sites, the 482 units, both chains.
   3. The held catalog is compared with today's map: counts and distributions (lengths, drives and
      climbs change under the new chaining), screenshots and performance.
   4. Then the heritage switch is compared.
   5. Then the hold is released, and the converted legacy data deleted.
7. **Features,** each on its own: 3D buildings, then PLATEAU; building heights in horizons and the
   viewshed tool; the new terrain repair; sharper terrain from national DEMs. Not started.

**Gaps:** the code falls short of the design here. Most need fixing before regions beyond today's
are added.
1. **Removing a region removes nothing built.**
   - Its base packs, road values, hidata and road packs stay in the manifest, so every catalog keeps
     serving them and GC never frees them.
   - `scenic remove` and the panel used to promise otherwise; their messages now say so.
2. **GC never sweeps `sources/`.** A retired pass's planet, filtered file, pieces, sets and road
   values stay, about 200 GB a pass. `work/pois` and `work/peaks` of units that leave the coverage
   stay referenced for good.
3. **New regions miss what today's coverage has from converted files:**
   - trees;
   - roadside buildings (Overture boxes for today's regions only);
   - trains a day;
   - heritage points and area overlays (until the heritage switch).
   - Taiwan's MOI DTM has no place on the NAS: tgos.tw refuses requests from outside Taiwan, so
     FABDEM serves.
4. **The scenic cache doesn't carry over between unit runs:** each unit run recomputes every scenic
   sample.
5. **Server details:**
   - The Regions API reads and writes the share outside the I/O pool, so a hung mount can hold a
     request.
   - A basemap tile's 304 still reads the NAS while the basemap isn't mirrored.
6. **The agent:**
   - It runs nothing away from home, since every job needs the NAS (the design let local steps go
     on).
7. **Catalogs:**
   - `credits` and `coverage` are empty: `/api/coverage` builds the coverage per request.
8. **The repo:**
   - `nas/fetch-planet.sh` lives only on the NAS.
   - `inputs/keys.env` is read by nothing (`dem/railgtfs.py` still reads `data/keys.env`).

## 11. Risks and checks

- **NAS throughput:**
  - writes ran at 12 MB/s through Tailscale's userspace networking until the LAN name;
  - a hung SMB session once stalled this Mac's reads, even a forced unmount, for minutes.
  - Planned: a rate limit on the pass's uploads, and remounting a hung mount (the breaker's probe
    times out) rather than waiting.
- **The build Mac's availability:**
  - work progresses only while the M4 is awake, at home, and has power;
  - a closed lid stops building, and nothing is lost while it waits;
  - if waiting proves too slow, the other Mac could take per-unit jobs (needs a toolchain there and
    a movable writer lease; not planned).
- **Dense units:**
  - the densest (Kanto, a 3 GB base pack converted) is first built in the cutover;
  - if a unit or its 110 km halo doesn't fit in 48 GB, units split into z7 or z8 tiles, and pack(T)
    by z7.
- **Remote DEM servers** may be slow or change. Today's cache seeds the units, and their new samples
  are kept, on the build Mac only: losing its cache costs sampling those again.
- **Version bumps at globe scale** would take days: today a bump makes every target stale in the
  normal order.
- **Way ids past u32** (2040s): the tile format is versioned.
- **Disk:** each pass adds its sources to the NAS until GC sweeps them (§10).

## 12. Changes

**v7 (2026-10-03):**
- **Names by language (§7).**
  - Translations are keyed by name, kind and language, in every script, and each line holds for the
    languages it was found to hold for.
  - Location enters only through the languages spoken there.
  - A thing's sourced English belongs to it alone, and wins over its name's translation.
  - Why: the reading areas were a box function with quirks the old tables were built on; English
    spread from one thing to every thing with its name; and new languages would have needed more
    boxes.
- **Every section checked against the code** (seven Opus reviews):
  - the history of v4–v6 and the implementation notes folded into the sections;
  - what isn't built marked planned;
  - the gaps listed in §10.

**Implementation decisions since v6 (2026-10-02 and 03), now in the sections above:**
- **No SSH from the build Mac:** 1Password asks for every new session. So GC deletes over SMB,
  uploads are checked by reading them back, and the planet fetch runs from DSM.
- **Units are z6 tiles only;** the split waits for a unit that needs it.
- **The NAS by its LAN name.** Tailscale's userspace networking on the NAS held SMB to 12 MB/s,
  against 60 MB/s on the LAN.
- **The cut is a depth-first quad tree,** four outputs per run. osmium's id sets need about 4 GB per
  output.
- **Terrain and slope per z3 pack, always from AWS's raw tiles.** Repairing a repaired tile isn't
  idempotent.
- **base(U) runs today's steps on a unit-sized folder,** staged from the build manifest. The pilot
  matched today's data.
- **Landmarks, stations and ferries have jobs of their own,** and pack(T) writes no landmark tiles
  (`docs/phase5.md`). Otherwise every road pack would depend on worldwide rankings.
- **Heritage sites and flags come before the units, in their own job;** the rest of the heritage chain
  comes after the landmark candidates. Wikidata and pageview outages mustn't hold up the roads.
- **Roads and landmarks build as two chains,** for the same reason.
- **Elevations are u16 decimetres from −500 m.** They were clamped at ±3,200 m, and roads in the
  Andes and the Himalaya reach 5,800 m.
- **Failures are never cached, and the breaker needs a failed probe.** A busy link slows reads
  without the NAS being gone.
- **Power: mains, or the battery down to 30 %** (asked for 2026-10-03); caffeinate per job.
- **The menu bar item,** with progress to the end (asked for 2026-10-03).
- **The cutover keeps today's 34 regions as 34 recipes,** with Gibraltar as an `osm:` relation.
  Keeping the legacy outlines makes the comparison like for like.
