# Plan: regions as modules, data on the NAS

The design of the build, the NAS layout, the Macs' roles and the map's serving. What isn't built is
marked planned; where the code falls short of the design, §10 lists the gap.

- The diagram's source is `docs/diagram/` (`python3 page.py out.html`, then republish `out.html` to
  https://claude.ai/artifact/PrjaaDXLuuXGpGr2xt5Vjg).
- Companions: `docs/pipeline.md` (every step as one row of one table: the concepts, the names,
  the checklist and the build page), `docs/phase5.md` (landmarks, stations, ferries and overlays by
  view; the landmark jobs; the zoomed-out summaries), `docs/formats.md` (file formats),
  `docs/inputs.md` (every input through a checked drop box), `docs/pool.md` (the pool),
  `docs/workers.md` (tasks for any worker), `docs/buildings3d.md` (3D buildings).

**The idea in one line:** OpenStreetMap comes from one worldwide download, cut by area, and a region
is only an outline saying which areas to build. Every step runs per area, per pack near the
coverage, or once for the whole world, so region size, count and borders never show on the map.

## 1. The user's surface

**The map is always there.**
- A launcher (`tools/launcher`, a LaunchAgent) keeps the server running on both Macs; an idle server
  loads nothing. The user opens `http://localhost:7311`.
- Zoomed out, every road is drawn at every zoom. The water zoomed out looks as full detail rendered
  and shrunk would: an island or lake smaller than a pixel is as faint as its share of the pixel,
  never exaggerated nor dropped, with the coastal shading full detail gives it (§6, Water).
- **Scenic** (`/Applications/Scenic.app`, `tools/status`) is an app opened from the Dock, Launchpad
  or /Applications, and at login. Opening it starts this Mac's build service (the agent: the app's
  own launchd job, registered with SMAppService and listed under Scenic in Login Items; launchd
  restarts it after a crash, and it restarts itself into an update). Quitting stops it: at once with
  no job running; with one, the app asks "Finish This Job First" (nothing new taken, the app quits
  when it ends) or "Stop Now" (the job given back, its work kept on disk); a logout, restart or
  shutdown takes Stop Now without asking. A leading Mac quitting hands the lead to a member that can
  take it (waiting up to three minutes for the handover), else the build waits for it
  (`control::Quit`). The map server stays a service of its own: Scenic's window and menu show it
  and start or stop it when asked, never on opening or quitting. On an update Scenic copies the
  installed version's Scenic.app over itself and opens again (without its window).
- **Switching a Mac over** from the login items Scenic replaces (`local.scenic.agent`,
  `local.scenic.status`): until then the menu bar item runs from `app/current` as before, and a
  copy of Scenic run from inside an installed version, or while `local.scenic.agent` is loaded or
  its plist is in `~/Library/LaunchAgents`, manages nothing (it says "Switch over with
  tools/app/install.sh"). On each Mac once the version is installed, the lead's Mac last (or hand
  the lead away first, `scenic lead give <the other Mac>`): run `tools/app/install.sh` (it removes
  the two login items, the old agent's job given back, copies Scenic.app to /Applications and
  opens it), allow Scenic in Login Items if macOS asks, and check its window says "Build service:
  running". The way back, until both Macs have switched: `tools/app/install.sh --legacy` (Scenic
  unregistered and removed, the two login items as before), then `tools/app/publish.sh --rollback`
  if the version itself is to go.
- `scenic status` and the menu bar item (Scenic's) show the build's state. A click
  opens the build page in a popover; the menu pauses and resumes the build, hands over the lead,
  sets the disk room and clears the Mac's build caches (`scenic clean`). It notifies on every change.
- `scenic`, `scenic-build`, `scenic-metrics`, `lakefill` and `coastcheck` check their command line
  first (pipeline::cli): `--help`/`-h` prints usage; an unknown flag, a missing value or an extra
  argument is refused with usage (exit 2).
- The server mounts the NAS itself when it's missing, at home.

**The Regions panel** (Settings → Regions):
- The regions, read only, grouped by their folders, each with its input-check state (accepted,
  checking, held), and on the map the published coverage and each region's accepted outline.
  Regions change only as files in `inputs/regions/` (docs/inputs.md §6.1); the app writes none.
- **Downloads on this Mac** (§4, Mirror): nothing is copied to a Mac unless downloaded there.
  - **World, zoomed out**: its own row, Download or Remove. Without it the map needs the NAS.
  - **Each region**: its size, Download or Remove, and its state (downloaded, copying N %, next,
    waiting for room, away). A region brings the World; the World can't be removed while a region
    or view is downloaded. A region still building says "x of y tiles, on the map as they come"
    (every unit built but not yet recorded built: "finishing: x of x tiles, the last map tiles and
    layers being made")
    (its catalog's `tiles`; the menu bar and `scenic status` say the same, from the agent's
    status) and can be downloaded: its download grows with each catalog, what's here never fetched
    again, copied at the build's slow rate.
  - **Download this view**: the ground on screen, named after the most important place in it.
  - Remove confirms with the size. A download that wouldn't fit above the reserve is refused.
  - The Mac's line: what's downloaded, free space, the reserve, the copy under way, and that copies
    keep to 20 MB/s while the build runs.
- The status bar offers to reload when new data or a new app is in, and says when the NAS or the map server can't be reached.
- New areas appear as their catalog goes out, a unit's tiles within minutes of its build (§8, Publishing).

**Translations and descriptions** (§7) are drop boxes of the input check: `inputs/translations/` and
`inputs/descriptions/` (docs/inputs.md §6.3, §6.4). A drop shows on the map a few minutes after it's
accepted, with nothing rebuilt. After each build the agent lists what still lacks English or a
description in each folder's `todo/`, with the briefs.

**Everything else is automatic:** building whenever a Mac of the pool is awake and reaches the NAS
(at any charge; away from home through Tailscale, slowly), copying downloads and keeping them
current, emptying build caches once the build is done, installing a newly published app when the
map is idle, backups. Publishing the app is a command (`tools/app/publish.sh`, §8, App publishing).

## 2. How the work is divided

**Units.** A unit is a z6 tile (about 600 km across at the equator). Units are fixed by the OSM
pass's cut; a split into z7 or z8 tiles waits until a unit outgrows the M4 (§11). Packs for serving
are per z6 tile (hi), per z3 tile (lo) and one root (z0–2).

**Where it runs.** All work runs on the pool's Macs (§8, The pool) except the planet's download,
which the NAS does itself. The jobs (§8 has their order and keys):
1. **The OSM pass**, when the NAS holds a newer planet: the filtered planet, the worldwide sets
   (rail, ferries, designated areas, places, outlines, labels, summits, hiking routes,
   heritage-named objects, water), the worldwide basemap, pieces (filtered OSM per unit, 10 km
   buffer, ways whole) and worldwide road values (§6).
2. **Worldwide jobs, once per pass:** missing sets, the outlines, hiking routes' ends, each unit's
   reach (§5), summits, place labels, the water, the lake fill; once ever, the worldwide z8 terrain.
3. **Heritage sites and designated areas:** one job over the coverage plus 20 km.
4. **Global-source layers near the coverage:** terrain, slope and tree cover, a piece per z6 tile
   and an assembly per z3 tile.
5. **Per unit**, for every unit meeting the coverage, a region at a time: base(U), the ways U owns
   (a way belongs to the unit of its first node) that touch the coverage, with per-vertex
   elevations, grade and scenic channels.
6. **Then chains** that don't wait for each other: **roads** (road → units index, pack(T) per z6
   tile from the base data of the units whose ways come into it, lo packs, rail stops and ferries, roots,
   then a catalog), **rail service** (§6), **landmarks** (`docs/phase5.md`) and **3D buildings**
   (`docs/buildings3d.md`).

Because road values come from the whole planet, adding a region changes nothing outside its own
units and the packs of the tiles their ways come into.

## 3. Storage

NAS root: `personal/projects/scenic-roads/`, on the `personal` share. Deletions over SMB bypass its
Recycle Bin.

```
inputs/        the drop boxes (docs/inputs.md): regions/, languages/, translations/, descriptions/,
               heritage/, heritage-matches/, timetables/{feeds,gtfs}/, elevation/, sources/, each
               with how/ (how it's made); README.md; keys.env (API keys, KEY=value); hold-catalog;
               _gate-test/ (the input check's test unit)
sources/       osm/<date>/ (planet, filtered, pieces/, sets/, roads/, outlines, reach, pass),
               basemap/ (Planetiler's jar and data), items/<date>/ (facts, pageviews, answers),
               pageviews/, dem-cache/ (per-vertex DEM seed), fetched/fabdem/, buildings/<release>/,
               trees/, canopy/, aws-terrarium/, rail/, canvec/, fetched/glo30/, terrain-z8-v3,
               inputs/<unit>/ (the input check's checked copies and accepted indexes: inputs.md §4.6)
base/          base packs, one per unit
hidata/        per z6 tile: ways-here index, query parts, rail lines, zoomed-out summaries
markdata/      per z6 tile: landmark points
ovdata/        per z3 tile: area and park details
global/        worldwide files: road values per unit, road → units, rail frequencies, landmark
               totals, heritage/, spoken
layers/<layer>/  root, lo and hi packs; basemap/world-<date>.<hash>.pmtiles
work/          build intermediates, not served
cache/         what units keep for later runs, shared by both Macs: dem-units/, scenic-units/
catalog/       <n>.json.zst (or deltas): every file the map reads, by content name
catalog-held/  a catalog kept back for review (while inputs/hold-catalog exists)
app/           published app versions; current.json, previous.json
state/         status.json (the lead's heartbeat), build/ (manifest, job keys, pending, terms),
               journal/, coord/, pool/ (switches), helpers/, backups/, logs/
nas/           fetch-planet.sh, which the NAS runs (copied from tools/nas/ by publish.sh)
```

**Immutable, content-named files.**
- Every built file is `<logical>.<hash16>.<ext>`, never rewritten. A write goes to `<name>.tmp`, is
  read back and checked against its hash, then renamed. An unchanged file keeps its name, so a
  rebuild uploads only what changed.
- **Catalogs** list every file the map reads, with the coverage they were built for and the credits
  of their sources. Readers take the highest that decodes.
- **The build manifest** (`state/build/manifest.json`) lists everything built; **job keys**
  (`state/build/jobs.json`) say what each output was made from. These and the unverified uploads
  (`pending.json`) and the files' sizes (`sizes.json`) are written by the lead from its term's records
  (`state/build/term/<E>/records.json`, the truth: §8, The pool).

**Copies carry the bytes and permissions, nothing else** (`store::sys::copy_data`; `cp -X` in
scripts): the share refuses a copy that sets a `com.apple.provenance` attribute differing from its
folder's, so `std::fs::copy` or plain `cp` fail with "Permission denied".

**Packs, never one file per tile.** SMB manages about 80 random reads per second per file.
- **Our layers** use packs: a header and meta, the blobs (identical blobs stored once), then an index
  of (tile key, offset, length, raw length, 64-bit hash) sorted by key. Root: z0–2; lo: z3–8 per z3
  tile; hi: z9–14 per z6 tile. Hi tiles exist only near the coverage for the layers built by area;
  worldwide layers (labels, water) have them wherever they have tiles.
- **The basemap** is Planetiler's worldwide PMTiles file, one per pass, served tile by tile.
- **Base packs, hidata, markdata and ovdata** are sectioned files: mapped when local, read from the
  NAS in pages (§4).

**Writers.** Every job uploads its files (content-named) and hands its record changes to the journal
on the NAS, which the lead merges; no step writes the records directly once a term exists
(`crate::out::check_writer`). Other writers: the owner's drop boxes, `tools/app/publish.sh`, and the
NAS's own planet fetch.

**GC** runs daily, in the agent.
- **Roots:** the newest catalog, every catalog of the last 14 days, and the build manifest.
- An unreferenced content-named file goes once older than 14 days; older catalogs go except the
  newest; stray `.tmp` files after 2 days.
- Retired passes' sources (`sources/osm/<date>/`, `sources/items/<date>/`) go by the same rule, their
  other files 14 days after the newer pass completed.
- The input check's checked copies (`sources/inputs/`, inputs.md §4.9) are kept while the manifest,
  an accepted index, an unmerged journal entry or an index of the last 14 days names them.
- Never swept: the newest pass, a planet waiting for its pass, the rest of `sources/`, inputs,
  state, app and nas.

**Backups.** Daily, the agent copies `inputs/` and the input check's acceptances
(`state/inputs/accepted/`) into a content-addressed store under `state/backups/`, with a dated list
on days something changed (kept 30 days), mirrored to the M4; `todo/` isn't backed up. Everything
else can be regenerated.

## 4. Macs

**Each Mac's agent.** Every Mac runs the agent (`scenic agent`, set up by `tools/app/install.sh`), a
member of the pool (§8, The pool): it leads while its Mac holds the term, else takes work from the
lead's coordinator. Below, the M4 (48 GB) unless said.
- It runs under the launcher as a LaunchAgent with `ProcessType Interactive` (else launchd throttles
  it).
- **Several jobs at once** (§8, Jobs at once). Each job is a child process group at utility
  priority (`taskpolicy -c utility`; `-b` would confine it to the efficiency cores), its Rust steps
  on the threads its grant gives it.
- **Power:** jobs run at any charge. Each holds `caffeinate -i -s -w <pid>` (no idle sleep; no system
  sleep on mains), dropped while paused. A Mac that runs down mid-job loses only the target under
  way.
- **Away from home** the agent mounts the share by the NAS's bare name through Tailscale (~12 MB/s)
  when the Keychain has its password; every job runs, slowly. Home again, with no job running, it
  remounts by the LAN name.
- **Downloads are kept on the NAS, each made once** (`sources/`): Meta's canopy squares, AWS's raw
  terrain tiles, FABDEM's tiles (all of them, fetched once by `scenic-build fabdem-fetch`; builds
  only read them: docs/inputs.md §6.8), the leaf-type sources, Overture's buildings, Wikipedia's monthly
  pageviews (`sources/pageviews/<month>.tsv.zst`, every article of the map's languages counted, so a
  dump is streamed once). A Mac's copy is a cache filled from the NAS; one the NAS lacks is uploaded
  the next time it's looked up.
  - **AWS's raw terrain tiles** are packed onto the NAS (`pipeline::rawpack`,
    `sources/aws-terrarium/packs/`, listed in `index.json`) in archives grouped as the terrain's
    packs are (z9–12 by z6 tile, z3–8 by z3 tile, z0–2 together). A job's new tiles go up at its end
    as one archive beside its area's others (the NAS's small-file writes run at ~23 a second). An
    area's archives are each kept more than twice the size of all those after it, else merged, so
    an area has a dozen at most. Archives leave the index through its `gone` list and are deleted a
    day later, so a reader of the old index stays right. Only the lead changes the index; a job
    puts its archives there unlisted and hands them off. A tile is read by range for an area's first
    16, then each archive is copied whole once and checked against its name.
  - **The Wikidata and Wikipedia answers** the items job and the heritage chain got for a pass
    (`pipeline::answers`) are kept as one archive per pass (`sources/items/<date>/answers.tar.zst`,
    `heritage.tar.zst`), written as the step starts (when the NAS lacks what the Mac has) and as it
    ends. The two sides are merged by key where both changed; a file deleted by hand stays deleted;
    an archive that doesn't read whole is moved aside, not written over. A new pass's answers start
    from the last pass's.
  - **Whole:** each copy is written by a temporary name, flushed and its length checked before the
    rename, and checked whole when read (`pipeline::whole`, `dem/whole.py`). One that isn't is taken
    again from the NAS, else the source.
  - **"Not there"** (a 404, or S3's 403) is remembered only once the source says so twice.
  - **What's fetched again** is only new data (a planet, Wikidata facts and pageviews, timetables, an
    Overture release) or a window not yet covered (DEM points not sampled before, WorldCover tiles
    the packs lack).
- **Caches** (`~/Library/Application Support/scenic/agent/cache`): what a trim or clear empties is a
  copy of what the NAS keeps, or made from it, refilled from the NAS, never the internet.
  - **The cheap ones** (emptied for room, trimmed after the build, cleared): canopy squares
    (`chm10/`; the M4's trim keeps them), raw terrain tiles and archive copies (`aws-terrarium/`),
    copies of record files staging reads (`blobs/`: hi packs, roadside buildings' z8 tiles, units'
    kept DEM samples), pageview month indexes (`items/months/`).
  - **Cleared too, on the owner's ask:** the pack cache (`base/`, base packs for pack(T) and lo), the
    per-vertex DEM seed (only while the NAS has it whole), local copies the summits and peaks jobs
    read, and the heritage jobs' clip of the filtered planet (made again by osmium in about an hour).
  - **Kept:** the answers (`items/`, `heritage-<date>/`), the trains' stop pairs (`rail/`), the unit
    stages' timings (`unit-stages.json`), what a unit kept that isn't on the NAS yet, and the
    coverage as last read (`coverage/<key>.cov`, reused while every recipe and outline file has the
    name, length and time it had).
  - Nothing is deleted through a link, nor anything in the NAS's project folder.
- **Its own map** is served from the NAS and what it has downloaded (§4, Mirror).
- **An OSM pass** starts with 80 GB free (the pack cache counting as free) and copies the planet to
  the SSD when there's room, else reads it from the NAS.
- **What a unit keeps for its later runs** is on the NAS (`cache/`), shared by both Macs: its DEM
  samples (`dem-units/`) and canopy and view results (`scenic-units/`, §6 base(U)). A unit's build
  folders go once it's built.

**The M1 (16 GB)** leads when it holds the term; as a member it builds what the lead's coordinator
gives it (§8, The pool): any shared step's jobs that fit the memory it spares (three eighths of its
memory; five eighths while its owner is away, for a job its last runs say ends within twenty
minutes) and its disk (15 GB free a job, 5 a task), from the far end of the list (terrain from the
near end), else units' tails (`docs/workers.md`). A unit's predicted peak is the most its job's
processes held together last time, else about ten times its piece, never under 3.7 GB nor below its
floor (docs/pool.md §7.2). A member's status is `state/helpers/<host>.json`, the lead's the heartbeat
(`state/status.json`).

**App Macs (both laptops).**
- **The launcher** (`tools/launcher/launcher.c`) runs the command in `~/Library/Application
  Support/scenic/run/<name>` and restarts it when it exits: `bin/scenic-launcher server` (the map
  server's LaunchAgent), and Scenic.app's own copy running `agent` (the build service; there an
  agent exiting with `QUIT_EXIT` ends it cleanly, so launchd leaves it stopped). A server started
  any other way (such as the desktop app's preview) waits on macOS's network-volume prompt; test
  servers run from the shell.
- **The updater** checks the NAS's `app/current.json` every five minutes, copies a newer app locally
  checking every file's SHA-256, and restarts into it when the map is idle. It keeps the current
  version, its own, the agent's and one to roll back to.
- The app reads the current and the previous form of every file kind.

**Server.**
- **Finding the NAS:** `getfsstat(MNT_NOWAIT)`, never `statfs`. At home (`fishandchips.local`
  answers on 445) it mounts the share with `osascript` and the Keychain, at most every five minutes,
  by the LAN name when the Keychain has it, else the bare (Tailscale) name. `nsmb.conf` makes it a
  soft mount.
- **NAS access** goes through a bounded I/O pool with timeouts. An overrun trips the breaker (offline
  banner) only when a quick probe also fails. Files are read in 1 MB pieces with handles kept open,
  never mmapped; base packs and hidata in 256 KB pages, or whole sections for scans.
- **Failures are never cached:** an unanswerable read is a 503; the client retries after 2 s,
  doubling to a minute, and at once when the NAS is back.
- **Memory:** caches keyed by content name, with budgets (512 MB base-pack views, 1 GB whole
  sectioned files, 384 MB pages, 1.5 GB whole sections, 128 open NAS files).
- **Place search** (`crates/server/src/places.rs`, `web/src/ui/search.ts`; `/` opens it): the
  labels layer's places (every label of the map's areas, worldwide what shows by zoom 8), by any
  word of the shown name, own name or English, ignoring accents, case and punctuation (inside CJK
  names too, not Thai). Whole name first, then prefix, then inner word; then importance, then
  distance. Return or a click flies there and marks it. The index is built on this Mac at the first
  search after the labels or translations change, on three threads, and dropped after half an hour
  unused (its arrays mapped, so the memory goes back). Unreadable packs are passed over and the
  index rebuilt later.
- **Offline start:** the last catalog and the pack indexes read so far stay local.
- **In use** means any request in the last ten minutes except the status polls (`/api/catalog`,
  `/api/ping`, `/api/build`, `/api/downloads`) and the search's `poll=1`. It checks for a newer
  catalog every 10 s while the lead's heartbeat (`state/status.json`, read at most every 30 s)
  says a build runs, else every 30 s in use and every 10 minutes otherwise; each check is one
  listing of `catalog/`. On the lead's Mac the agent also POSTs `/api/catalog/refresh` (this Mac's
  own requests alone) after writing a catalog, so it is adopted at once. A mirror's copy stopped by a
  new catalog resumes under it (copies are kept by content name).
- **URLs and ETags:** data URLs carry a content version (`?v=`): a tile's is its pack's (meta
  `packs`), so a new catalog re-fetches only the changed packs' tiles; a versioned response is
  `immutable` while current. ETags are content hashes, for named tiles combined with the versions of
  the languages spoken there and the spoken-languages raster (§7). The basemap's come from the
  catalog, so its 304s read nothing.

**Mirror, per Mac** (`store::mirror`, `store::pieces`, `crates/server/src/downloads.rs`).
- **Only what's downloaded.** The map reads a file from the Mac when it's there, else from the NAS.
  What's downloaded is this Mac's alone (`downloads.json` in its home).
- **The World, zoomed out:** the build's worldwide files (`global/`), the Canadian lake fill
  (`work/lakefill/<date>`, read by the water's deeper zooms anywhere in Canada), every layer's root
  and lo packs (z0–8), markdata, ovdata, and the basemap's zooms 0–10: about 10 GB. Not the pass's
  outlines.
- **A region, or a view:** every layer's hi pack and the base pack and road values of each z6 tile
  within 2 km of it; the basemap's zooms 11–14 over those tiles; the terrain's and grids' hi packs
  within 25 km (the viewshed's reach); the hi data of the z6 tiles within 50 km. With the World,
  that's what the map reads there (but §10, Gaps).
- **The basemap's pieces** (`mirror/.basemap/<archive hash>/`): each a PMTiles archive of its own,
  `lo` (zooms 0–10) and `6-x-y` (a z6 tile's zooms 11–14, shared by every download meeting it). On
  PMTiles' Hilbert curve a z6 tile's tiles at each zoom are one run, so a piece is four directory
  runs and mostly sequential reads. Sizes are known ahead (`sizes.json`); a piece cut short resumes;
  each tile's gzip checksum is checked. Pieces rather than a sparse copy: removed by deleting, sized
  as packs are, shared, opened as any archive.
- **Kept current:** each new catalog's files a download names are copied, and what no download
  names any more goes. Nothing else goes, ever.
- **Copies:** one file or piece at a time, the World first, then small worldwide files, root and lo
  packs, hi data and road values, base packs, hi packs, buildings, basemap pieces. While the build
  runs (the lead's heartbeat says a job runs) copies keep to 20 MB/s. Each copy is checked before
  it's moved into place.
- **Room:** a per-Mac reserve ("Keep free", 30 GB unless set; `reserve.json`, `PUT
  /api/downloads/reserve`), applied at once. A download that wouldn't fit is refused; when the disk
  fills later, what doesn't fit waits, and nothing downloaded goes for it.
- **The agent's jobs** don't need the mirror, but read some files from it where present: pack and
  lo their owners' base packs and road values, the marks and overlay conversions the heritage
  job's files. Each is read under a shared lock (`store::cachefile`). The mirror deletes a file
  only under an exclusive lock taken without waiting, so a file a job holds stays until a later
  sync.
- **Pack indexes** are cached as they're read (`idx/`), for offline starts.
- `/api/downloads` gives the panel its state; the menu bar says what's downloaded.

**Devices: an iPhone, an iPad.** Either Mac's map opens on them over the tailnet.
- **Who's answered:** the server answers only this Mac, its LAN and the tailnet, also through
  `tailscale serve` (`pipeline::net::reached`); anything else is 403, Funnel included. No key.
- **A page elsewhere is never the map's:** the `Host` must be an address, localhost, or a name only a
  tailnet or local network resolves (DNS rebinding refused); an `Origin` must be the map's own page
  or this Mac's; CORS only to this Mac's own pages.
- **The address** is `<home>/map-page` (`crates/server/src/remote.rs`): HTTPS where `tailscale serve
  --bg --https=8443 http://127.0.0.1:7311` proxies the server, else the tailnet address. The menu's
  Copy the Map's Address and `scenic status` give it. Only a proxy that says so (`X-Forwarded-For`)
  keeps its requests from passing for this Mac's own.
- **An app** (`web/public/manifest.webmanifest`): Add to Home Screen. Its service worker
  (`web/public/sw.js`, HTTPS only, never on localhost) keeps the page, its scripts and the catalog's
  metadata, plus the fonts, icons and `immutable` data as viewed (12,000 files; 3D buildings' tiles
  in a cache of their own, 2,000). A Mac that doesn't answer within 4 s is taken as away for a
  minute.
- **Touch** (`web/src/trackpad.ts`, `web/src/ui/touch.ts`): one finger pans; two pinch, rotate and
  tilt; double tap zooms; a long press offers Street View, Google Maps and OpenStreetMap. Popups
  stay on screen.
- **A phone's layout:** settings fold into a panel; lists become a bottom sheet (a card on its
  side). On a tablet the panel stays docked. Touch screens get bigger controls.

## 5. Coverage and regions

**Regions** are files in `inputs/regions/` (docs/inputs.md §6.1): `osm:` relations, GeoJSON shapes
and other regions combined, each pinned to an OSM snapshot, checked by the input check before the
build uses them. Overlaps are harmless; nothing checks for gaps.

**Coverage** is the union of the regions.
- **What gets built:** a feature that touches the coverage; for a way, any node inside. Ways are kept
  whole, so roads end a little past the edge.
- **Units** are chosen by their reach, worked out once per pass from each piece's roads, rail and
  ferries (`sources/osm/<date>/reach`, `pipeline::reach`): a unit is built when the box of the ways
  it owns meets the coverage, or one of its own long ways touches it. The unit step then keeps
  exactly the ways touching the coverage.
- **Builds depend on coverage, never on regions.** Renaming, splitting or merging regions with the
  same outlines reruns nothing. A job is keyed on the coverage inside the box it reads
  (`Coverage::fingerprint`), so changing an outline reruns only what its changed part reaches.
- **Shrinking:** what only the removed part built leaves the manifest once the units are built (a
  prune): units no longer built, their candidates and peaks, map tiles no unit's ways reach. A z6
  tile the coverage has left loses its terrain, slope and tree cover hi packs and mids ("none"
  pieces), a z3 tile its lo packs ("none" assemblies). The next catalog drops them, and GC frees
  them.

**By location.** Rules that depend on where a thing is; the units' rules are versioned by area in
`pipeline::rules`, so a changed rule reruns only the units it applies to:
- elevation datasets and their blend (`pipeline::dem`, docs/inputs.md §6.8) and densification
  spacing (8 m where a declared dataset finer than 15 m has data, 15 m elsewhere:
  `pipeline::dem::spacing`, docs/inputs.md §6.8);
- heritage registers (`inputs/heritage/`, docs/inputs.md §6.5);
- timetables: the Mobility Database feeds of the countries the coverage is in, declared feeds and
  static timetables (§6, Rail service; docs/inputs.md §6.7);
- road network codes (`extract`) and their colours (`web/src/mapschemes.ts`);
- leaf-type source: EEA in Europe, NALCMS in North America, none elsewhere (`dem/leaftype.py`);
- the languages spoken there (§7; docs/inputs.md §6.2);
- credits (`pipeline::rules::CREDITS`; a catalog lists those meeting its coverage + 20 km or its
  units' ways).

**No seams:** no step knows region borders; per-vertex values are computed once, by the owning unit;
road values come from the planet; queries join road parts by
offset. Where coverage ends, roads end.

**IDs:** OSM type and id. Way ids fit u32 until the 2040s; points use u64.
- Each z6 tile's hidata has a "ways here" index (`here`, `ends`): every way whose box meets the tile,
  with its owner unit and its index in that unit's base pack.
- Details and profiles ask with the id plus the location, which picks the tile or a neighbour.

## 6. Pipeline

### The OSM pass

1. **The NAS fetches the planet** (`nas/fetch-planet.sh`, from `tools/nas/`): DSM runs it daily; it
   does nothing until the newest planet is six months old or `nas/fetch-now` exists, then downloads
   the newest planet with a published MD5, resumably, checks it and moves it to
   `sources/osm/<date>/`.
2. **Copy to the SSD** when there's room (§4).
3. **Filter:** one `osmium tags-filter` into (a), the filtered planet, kept until the next pass so a
   new tag needs no download.
4. **Sets and the basemap's input (b)**, filtered from (a). Sets are versioned (`summits-v2`).
   `pass-sets` makes a finished pass's missing sets.
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
7. **Cut (a) into z6 pieces:** a depth-first quad tree, four outputs per osmium run (osmium keeps
   ~4 GB of id sets per output). Ways whole, multipolygons completed, 10 km buffer. Each piece is
   uploaded with its road links as soon as it's cut. (`scenic-build patch-ferries` gives pieces the
   ferry ways a pass filtered without `w/route=ferry` lacks.)
8. **Road values** (below).
9. **Finish:** the pass's summary is written; older passes retire from the manifest (but the
   previous pass's outlines, which the outlines step reads).

### Global-source layers

- **Terrain** (`pipeline::terrain_pack`), two steps:
  - **terrain**, a piece per z6 tile near the coverage (`build_piece`): its z9–12 within the
    coverage plus 20 km, zoom capped by latitude so pixels stay ≥ 15 m (z12 to 67°, z11 to 79°, z10
    beyond): its hi pack; and its mid (`work/terrain-mid/6-x-y`, never served: its z9 tiles' 2×2
    means as f32, and its lakes' levels). A piece making no hi tile drops its hi pack; there the
    server makes tiles from the z8 ones.
  - **terrain-lo**, an assembly per z3 tile (`build_lo`): z8 → z3 made again from children (a
    piece's z9 means from its mid), since a lake's level at z8 is gathered from the whole area: its
    lo pack.
  - A piece's z8 subtrees, grouped by shared lakes, are tasks (`terrainsub`, docs/workers.md §3).
  - Together they make the same packs, byte for byte, as the area's whole run (`build_q_with`,
    `scenic-build terrain 3/x/y`; `terrain_pack` tests).
  - **The root (z0–2)** is made from the lo packs once no terrain work is left.
  - **Sources:** declared in `inputs/elevation/` (`terrain = true`, docs/inputs.md §6.8,
    crate::terrain_sources): the base's raw tiles (AWS's Terrarium tiles, fetched once by the
    declared template, packed onto the NAS: §4); north of 60°N, Copernicus GLO-30 by its declared
    index (kept in `sources/fetched/glo30/`); and the water of the latest pass's basemap. Each is
    pinned in the key (`TERRAIN_V`, the declarations' `keyed` digests, the digest of the water the
    target reads), so
    a rebuild with the same key gives identical packs. The digests are worked out once a basemap by
    the `terrain-water` job, so a new pass remakes only pieces whose water changed.
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
- **Slope** (`pipeline::slope_pack`): z11 and coarser stored, from transient z12 Horn slope; the
  server makes z12 on demand. **slope**, a piece per z6 tile (its z9–11 hi pack and its mid,
  `work/slope-mid/6-x-y`: its z6–8 tiles), reading the terrain packs (its own z6 tile's copied
  whole first) and its neighbours' edge strips; **slope-lo**, an assembly per z3 tile from its
  pieces' mids. Together the area's whole run's packs, byte for byte.
- **Worldwide z8 terrain** (`sources/terrain-z8-v3`, once, not served): every z8 tile repaired, with
  each tile's maximum (AWS's tiles alone, §10). Peaks read it.
- **Grids (z11):** land cover, canopy and cover, for analysis only. Each unit makes the grid tiles
  its packs lack and uploads them as its z6 tile's `grid-*` hi packs; they aren't in units' keys
  (made from fixed datasets, the same whether read or made).
- **Trees** (cover, height, leaf type), zoom 4–12, clipped to the coverage (`pipeline::treepacks`,
  running the `trees` program, `pipeline::trees`):
  - **trees**, a piece per z6 tile the coverage meets (`trees --z6`): its zoom 9–12 hi packs and its
    mid (`work/trees-mid/6-x-y`: zoom-8 tiles and values).
  - **trees-lo**, an assembly per z3 tile (`trees --assemble-lo`): zoom 8–4 from its pieces' mids.
  - Together the same bytes as a z3 tile's whole run (`trees --z3`, `scenic-build trees 3/x/y`), from
    Meta's canopy squares (`sources/canopy/`, each downloaded once under a `<file>.lock`) and the
    leaf-type squares (`sources/trees/leaf/`, made by `dem/leaftype.py`).
  - Lossless WebP by `pipeline::webp`. `tools/check/trees-same.py` compares it with `dem/trees.py
    --z3` (same pixels); `SCENIC_TREES_PY=1` uses trees.py for a whole run.
  - `trees --block`, `--blocks` and `--assemble` run blocks alone; a row of a piece's blocks is a
    task (`treeblock`, docs/workers.md §3), the same bytes natively and in WebAssembly.
- **Area overlays:** `docs/phase5.md`. The `overlays` job runs after marks (it needs the World
  Heritage dots' ids).
- **3D buildings:** `docs/buildings3d.md`. Every building in the coverage from the Overture release,
  tiles z12–14 per z6 tile (`bldprep`, `bldtiles`; `pipeline::bld`, dem/bldprep.py), a z6 tile's
  z8 areas offered as tasks (`bldtile`); below zoom 14 the far buildings' field (buildings3d.md
  §4.8; `?bldfield=off` leaves it out). Planned: national heights (PLATEAU, BD TOPO).

The server builds missing deeper terrain and slope tiles from their ancestors.

### Depth (per area and tile, from the depth sources)

Depth under the seas and lakes (`pipeline::depth`; its sources docs/inputs.md §6.10; its files
docs/formats.md, Depth), split by tile like the terrain, so downloads and the mirror take it as it is.
- **Targets:** every z3 tile within 200 nautical miles (`REACH_KM`, a region's EEZ's reach) of the
  coverage that a source meets (`depth-lo`), and every z6 tile within it that a source finer than z8
  meets (`depth`, any Mac). The 15″ grid covers the rest of an area's water.
- **depth-lo (a z3 tile):** z6–8 composed (below), z3–5 each its children's mean.
- **depth (a z6 tile):** z9–16, a block only where a source has a grid of that zoom or finer; its
  capped pixels scaled up from this job's coarser tiles and, at z8 and coarser, from the lo packs
  of its z3 tile and its neighbours (its neighbours' fine tiles are another job's: within a block's
  32-pixel margin across the tile's edge, their lo levels stand in).
- **Composing a block** (8 × 8 tiles and a 32-pixel margin; `depth::compose`), on the terrain's water
  (`terrain_water`, the basemap's z6–14, its z14 scaled at z15–16): sources sampled (coarser grids
  interpolated, finer ones averaged from the overview that lines up) and painted coarse to fine;
  past its own zoom a pixel shows the stored level of its zoom scaled up as the map's fallback makes
  it (`depth::fall`, the server's too), detail fading in over three capped pixels; a source ending in
  the water feathered over its `band_m` (a source off the others' datum fills gaps only); the shore:
  positive values kept as drying heights only from intertidal sources, water without data filled
  linearly to 0 at the shore within 600 m from data of the same water body (a lake without data of
  its own stays empty), the land a 3-pixel band; dries where above the chart's low water.
- **The floor:** the depth lies on the terrain's own water, flattened to its surface, so the terrain
  less the depth is the floor; the server serves it (`/tiles/terrain/…?floor=1`), no second terrain
  stored. The map's toggle (the 3D terrain from it, the water's surface drawn at its level, what's
  draped on the water kept at the surface) is planned (§10).
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
     local folder, each source then copied whole to `sources/fetched/<id>/` with `tar` over SSH (the
     NAS's small-file limits; plan §3).
  3. The declarations (`producers/depth/declarations/`) and the indexes are dropped into
     `inputs/sources/`. Once accepted, the terrain-water job works out the depth targets' water
     digests (once a basemap), then the assemblies run, then the pieces.

### Water (per pass, worldwide)

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
  - **The source:** CanVec 1:50,000 Hydro `waterbody_2`, the provinces' zips in
    `sources/canvec/50k/`, downloaded by hand (the FTP's robots.txt disallows `/pub`), read in place
    (`lakefill::zip`). Taken: lakes, reservoirs, ponds and unidentified water bodies; intermittent
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

### Worldwide road values (in the OSM pass)

**One chaining** serves whole roads, strokes, drives, hover, profiles and rides.
- **How ways pair:** at each node, way ends of the same kind pair by mutual best continuation: same
  ref (any shared token), else same name, else same class when both are unnamed, else a nameless way
  continuing a named one of its class within 35°. The straightest go first, within 100°. Oneways
  pair only in their direction. Ties: higher level, straightest, lower way id. Rail pairs by line
  identity; ferries and ways with under two vertices never chain.
- **Per piece:** the pairing at a node depends only on the ways through it, so it's exact per piece.
- **Roads:** pairings form paths and cycles; a union-find and an ordered walk give each way its road
  id (the road's lowest way id), the road's length, and its offset and direction.
- **Storage:** per unit, `sources/osm/<date>/roads/<u>`, copied by each unit job into
  `global/roads/<u>` with `byroad` (ways sorted by road).
- Lengths include parts outside the coverage.
- Elevation smoothing uses a local continuation rule (`Net.cont`) in base(U).

### Per unit: base(U)

The unit job runs the unit's programs on a unit folder, wiped at each run:
1. **extract:** U's ways that touch the coverage, on U's piece. Rail tracks without a route relation
   are kept by type.
2. **Elevations:** `elev` (`pipeline::dem`), the declared elevation datasets (docs/inputs.md §6.8) by
   their plan for U (`elevation.json`) through one sampler, blended at their edges, on U's slice of
   the per-vertex DEM cache (the seed, where one of its samples could be valid, and the units' kept
   samples). U's samples are kept for later runs and its neighbours'.
3. **Heritage:** the heritage-sites job's slices within U + 30 km; `areaflags` rasterises the areas
   onto U's grid.
4. **Terrain and grids:** terrain z11 and the grids, staged from the packs the manifest names.
   Missing grid tiles are made.
5. **`tile elev`:** clean-up and grade, with junction context from the piece.
6. **scenic:** `scenic-metrics` prep, canopy, view, buildings and flags for every sample of the ways
   U owns. Buildings come from the roadside buildings' z8 tiles within 1 km of U's tile + 20 km and
   its long roads. Canopy and view results are kept in `cache/scenic-units/` (`scache::Carry`), so a
   later run redoes only new samples and those near grid tiles whose terrain or land cover changed;
   a change of `scache::SCENIC_V` redoes all.
7. **Output:** the base pack (per-vertex arrays and records, indexed by z9 sub-tile),
   `global/roads/<u>`, `global/roadids/<u>` (its roads' ids, what the road → units index reads),
   `global/roaden/<u>` (the roads' own English), and `grid-*` hi packs. A unit
   left with no ways in the coverage drops them.

Elevations are u16 decimetres from −500 m (to 6,053.5 m); readers also take older packs' i16.

Landmark candidates and peaks have their own per-unit jobs (`docs/phase5.md`).

### Roadside buildings: the world, once per release

The buildings job (`pipeline::buildtiles`) makes Overture's building boxes for the world from one
pinned release, before any unit runs: `dem/buildings.py --world` reads the release's bbox columns,
24 files at a time, into local parts by z8 tile (each file marked done once written, so a run cut
short resumes); each tile's parts are merged onto the NAS, then the index. A unit's key names the
index, so a new release rebuilds every unit.

### Per z6 pack: pack(T)

- **Reads:** the base packs and road values of T's owners, every unit whose ways come into T (its
  owned extent from the pass's reach; without one, its tile + 20 km), grown by the base pack's own
  extent where that lies further (a pack built by other reaches; each pack's header extent kept
  in `state/build/extents.json`, which the lead keeps as it plans, reading each new pack's header
  once; jobs only read it, and read the headers of packs it lacks without writing). A job opens
  only its tiles' owners (`build::owners_of`, the rule their keys read); one given no tiles, or
  whose reaches don't list every unit, opens every unit.
- **Writes:** road and rail hi tiles (z9–14) and T's hidata: the ways-here index; query parts (each
  road's 100 m samples and channels with offsets, for drives and rides); rail lines; the zoomed-out
  summaries (`docs/phase5.md`).
- **Lo packs** per z3 tile hold roads and rail z4–8, made from the base packs.
- Landmarks, stations, ferries and overlays come from their own jobs, so no road pack depends on
  worldwide rankings.

### Worldwide steps after the units

- **Road → units index** (`global/roadunits`): for whole-road hover. From the units' road ids
  (`global/roadids/<u>`; a unit built before them: its road values), keyed on them by content; it
  reads only the lists changed since the index before, which names each by content (`srcs`).
- **Labels** (per pass): places, states, seas, lakes and parks from the labels set (`dem/labels.py`,
  with own English, kana reading, OSM languages and OSM object: §7).
- **Water** (per pass): above.
- **Spoken languages** (per pass, the `spoken` job): `global/spoken` (§7).
- **Rail stops:** from the rail set, for the built units' tiles + 20 km.
- **Ferries:** worldwide, from the ferries set, the ferry feeds (`dem/ferrygtfs.py`, docs/inputs.md
  §6.7.0) and the static timetables (§6.7.2).
- **Rail service** (below).
- **Landmarks:** `docs/phase5.md`.
- **The to-do lists:** `names-todo` after a catalog the map serves, at most daily while units
  build, after any once none's left (§7, `NAMES_TODO_EVERY_S`); `timetables-todo` after each rail
  and ferries run (docs/inputs.md §6.7.3).

### Rail service

Trains a day on each rail way of the coverage (`global/railfreq`), from operators' GTFS timetables
and static timetables compiled by hand (`pipeline::rail`; docs/inputs.md §6.7). Two jobs, in a chain
of their own (§8):
- **`rail-feeds`** (`dem/railfeeds.py`) finds the feeds from the accepted declarations of
  `inputs/timetables/feeds/` (docs/inputs.md §6.7.1):
  - The Mobility Database catalogue's active, keyless GTFS feeds of the countries the coverage is in
    (`pipeline::rail::countries`: a country holding 5 % of one of the coverage's outlines, or half of
    whose area the coverage holds; a territory with an ISO 3166-2 code brings its country too) whose
    box meets it. The declaration's `own_code` and `country` correct the catalogue's codes.
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

### Hand-made inputs

What no job makes, kept on the NAS with how it was made. Scripts run in `dem/` (`uv run python
<script>`); `scenic-build` takes `--root <NAS project folder> --scratch <local dir>`.
- **Heritage registers** (`inputs/heritage/`, made by `producers/heritage/`: docs/inputs.md §6.5)
  and the owner's matches (`inputs/heritage-matches/`, §6.5.4).
- **Timetable feeds' declarations** (`inputs/timetables/feeds/`, §6.7.1), how they were found in its
  `how/`.
- **Static timetables** (`inputs/timetables/gtfs/`, §6.7.2): the ferry timetables looked up by hand
  (the brief and batches in its `how/`); the MTR's lines (`mtr.zip`, made by
  `producers/timetables/mtr.py --mtr mtr.json --stations hk-stations.geojsonseq --out mtr.zip`).
- **Translations' and descriptions' methods** (§7): `inputs/translations/how/`,
  `inputs/descriptions/how/`.
- **Every drop box's `how/`** says how its input is made, pointing to `producers/<input>/`
  (docs/inputs.md §8); its README is kept in the repository as `producers/how/<box>/README.md`, and
  `inputs/README.md` as `producers/how/inputs-README.md`.
- **Fonts** (`app/fonts/` on the NAS, `data/fonts` in a checkout): three Noto Sans styles' glyph
  ranges, `scripts/fonts.sh <folder>`.

### Job keys

A job's key is its step version plus what it reads, mostly by content name. The ones that cascade:
- **terrain-water (one):** the water source's pin (the latest basemap, and the lake fill) and what
  each terrain target reads of it; it fills the digests file (`work/water-idx/<hash16 of the pin>`)
  when a target's digest is missing. The digest (`terrain_water::tiles_digest`) is over the
  polygons as the terrain reads them, so only a change the terrain could see changes it. Until a
  target's digest exists its key is unknown and its area waits.
- **terrain (a piece):** the coverage within 20 km of its tile, the terrain's declarations
  (crate::terrain_sources::pin: their ids and `keyed` digests; until they're accepted its key is
  unknown and it waits), the digest of
  its water (`agent::build::water_key`), and "no hi tiles" for a piece making none
  (`piece_key_made`). A "none" piece is keyed on the version and the tile (`none_piece_key`).
- **terrain-lo (an assembly):** its pieces' mids by content ("-" for a piece without one) and the
  digest of its z6–8 water; `TERRAIN_LO_V`. "none" likewise (`none_lo_key`).
- **slope (a piece):** every terrain tile it can read, by content, from the packs' indexes
  (`agent::build::slope_piece_reads`), so a change along a neighbour's edge reruns it, one inside
  doesn't.
- **slope-lo:** its pieces' mids and the area's terrain lo pack; `SLOPE_LO_V`.
- **trees (a piece):** the coverage inside the tile; `TREES_V` changes with any change to the bytes
  a piece writes.
- **trees-lo:** its pieces' mids; `TREES_LO_V`.
- **heritage-sites:** the pass, its areas set, the registers meeting the cover (their `keyed`
  digests), the matches' accepted version, the coverage.
- **unit:** its piece, the pass's road values, the coverage as its ways meet it, the versions of the
  location rules where its ways go, its heritage slices, the roadside buildings' index, the elevation
  datasets' plan, and the terrain tiles it reads by content (`agent::build::unit_terrain`: each
  tile's XXH3 from its pack's index): what it stages, every z11 tile, and at its ways' points the
  z12 tile or the finest staged above it. A unit whose terrain indexes can't be read has no key and
  isn't built; the status says so.
- **pack(T):** the base packs and road values of its owners, the units whose ways come into T.
- **lo:** the base packs and road values of the units whose owned extent meets its z3 tile.
- **rail-feeds:** the declarations' accepted index, the catalogue's copy, the coverage, the pass's
  outlines, and which keys `inputs/keys.env` holds by name (never values). Not what it writes.
- **rail:** the feeds' list (each zip by content name and its day), the static timetables' accepted
  index, the pass's rail set and the coverage.
- **water:** its version and the pass's basemap.
- **bld-fetch** (network): its version, the release and the coverage.
- **bldprep (per z6 tile):** `BLDPREP_V`, the release, and each downloaded file's row groups and GHSL
  tile meeting it (`crate::bld::sources`).
- **depth-lo (a z3 tile):** `DEPTH_V`, `DEPTH_LO_V`, the tile, the depth sources meeting it (id and
  digest of declaration and index), the digest of its z6–8 water (the terrain assembly's).
- **depth (a z6 tile):** the same versions, the tile, the sources meeting it (2 km round), the content
  names of the depth lo packs of its z3 tile and its neighbours, and the digest of its z6–12 water
  ("depth 6/x/y", worked out by the terrain-water job).
- **bldtiles (per z6 tile):** `BUILDINGS_V`, the normalized files of the tile and its 8 neighbours,
  and the coverage's shapes over it with their countries (`Coverage::shapes_key`).

The landmark jobs, stations, ferries and overlays: `docs/phase5.md`. An output identical to before
keeps its content name, so jobs keyed on it stop there.

### Served

| What | How |
|---|---|
| our layers | the pack for the tile; names attached |
| basemap | tiles from the catalog's PMTiles archives (`/tiles/base`); names attached |
| drives, rides, rail lines | parts from the hidata in view plus half the window, joined by offset; zoomed out (wider than ~1,200 km) from the 500 m summaries (`approx`) |
| whole road (hover) | the road's parts, through the road → units index |
| details, profiles | by OSM id plus location (ways-here index → base pack) |
| landmarks, overlays, stations, ferries | by view (`docs/phase5.md`) |
| `/api/meta` | the map's meta, summed from the base packs' summaries |
| `/api/catalog` | the catalog's number, layers, zoom ranges, versions, credits, regions, NAS and build state |
| `/api/coverage` | each region's outlines, simplified |

**Browser:** the map is drawn by our own MapLibre, `web/maplibre/` (MapLibre GL JS 6.11.2's source,
changed where needed: `web/maplibre/FORK.md`), compiled with the app. English comes from the server.
The page switches to a new catalog in place (it polls `/api/catalog` every minute).

## 7. Names and descriptions

**One pipeline for every named thing:** places, states, seas and lakes, rivers, parks and protected
areas, natural features, sights, heritage sites and areas, Indigenous lands, stations, rail lines,
ferries, and road names (in the app's text).

**Display.** A **main** label and an optional smaller **sub** line (under main on the map, "main
(sub)" in text). Sub shows only when it truly differs from main: not the same but for accents, case,
punctuation or spacing, and not one of main's parts.

**A thing's English, in order** (`names::display`):
1. **Its own,** from a source about that thing, never copied to other things of the same name, and
   shown even where wrong (`names::own`): OSM's `name:en`; its romanised name (`name:ja-Latn`,
   `name:zh-Latn-pinyin`…); its kana reading romanised by modified Hepburn without macrons
   (`names::romaji`); a register's, UNESCO's or English Wikipedia's title where the heritage and
   landmark jobs put it in `en`. An own English equal to the name is none.
2. **Its name's translation:** the line for its name, kind and language, trying first the languages
   OSM tags the name in (each `name:<language>` equal to `name`), those spoken there first, then each
   language spoken where the thing is. A name tagged Chinese is read in Cantonese where Cantonese is
   spoken and Mandarin isn't.
3. **Else none:** the name alone, and the name goes on the to-do list.

**Translations are keyed by name, kind and language, in every script.**
- **Kind:** road, settlement (city to isolated dwelling) or other. A name is looked up in its own
  kind only. Labels: a place by its class, the rest other; the basemap: `place` by class,
  `transportation_name` road, the rest other; roads road; everything else other.
- **Language:** ISO 639, first subtag. A line holds for one language or several (a name that could
  be in any local language with the same English is filed under all). 中山 has a Chinese, a Japanese
  and a Cantonese line.
- The languages describe the name, not a region, so a line serves everywhere they're spoken.

**Languages spoken where a thing is** (`names::spoken`), in order:
- **Per territory:** the languages input's table (docs/inputs.md §6.2: `inputs/languages/
  territories.tsv`, from CLDR by `producers/languages/cldr-languages.py`): official or de facto
  official languages, most spoken first.
- **Refined** per subdivision (the producer's `REFINED`): Quebec French then English, New Brunswick
  English then French, Catalonia, Valencia, Galicia, the Basque Country, Wales, Scotland, Brittany,
  Hong Kong and Macau (Cantonese).
- **Where:** the pass's outlines of ISO 3166-1 territories and of the subdivisions with a line,
  simplified (1 km, 250 m), as a raster of 1/128° cells, run-length coded, the smallest outline
  winning. Beyond every territory no language is spoken.
  - **Made once per pass** by the `spoken` job (keyed by the outlines, `spoken::RULES` and the
    languages' accepted version): `global/spoken`, in the World download.
  - A catalog without it: the server makes the raster from the catalog's outlines and keeps it in
    its home (`names/spoken-…bin`).
  - Until it's there, names show with their own English alone (`/api/catalog`'s `names.waiting`).
- It's the only place location enters.

**Files:** `inputs/translations/**/*.jsonl` (not `todo/` nor `how/`), checked by the input check
(docs/inputs.md §6.3): `{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu",
"sub": "Blue Lake", "via": "agent:haiku"}`.
- `n`: the name as in OSM; `kind`: one or a list; `langs`: one or a list; `main`, `sub`: the display
  (`main` empty: the name; `sub` null: nothing); `via`: how it was made ("todo" and "skipped" lines
  are left out).
- Where lines share a name, kind and language, the later file (by path) wins, then the later line.
- The server copies the accepted version the lead's records name (crate::accepted,
  pipeline::inputs::served), checking every minute while in use.
- Compiled per file into one arena indexed by name (`names::table`), shared through one `Arc`
  (`names::Namer`).
- **Versions:** each language's is a hash of the accepted files holding lines in it. A tile's ETag
  includes the versions of the languages spoken within it (a tile wider than 40° takes all) and the
  raster's.

The tables converted from the earlier translation work are `0-converted/<language>.jsonl`
(converted once; the log is `inputs/translations/how/conversion-log.txt`); their
lines hold for their source area's languages.

**The build carries each thing's own English and language tags:** labels (`dem/labels.py`: `en`,
`kana`, `l` the languages OSM gives its name, `o` its OSM object); the basemap (`name:en`, `name:fr`,
and from the next pass `osmpass::BASEMAP_LANGUAGES`); roads (`global/roaden/<u>`, no language tags).

**To-do** (`pipeline::namestodo`, the `names-todo` job, after a catalog the map serves: at most
daily while units build, after any once none's left):
- **`inputs/translations/todo/<language>.jsonl`:** each name something in the coverage has with no
  English of its own and no line in any language spoken there. Read from labels, roads and rail
  lines (base packs, each way placed once by the ways-here index), and landmarks. Names already in
  English, without letters, or settlements in Latin script aren't listed.
  - **Each entry:** the name and kind; candidate languages (spoken there, narrowed by OSM's tags and
    the script); how many things lack English and one of them; a priority (by class, population,
    fame or road class, plus log₁₀ of the count). Each goes on its first candidate's list.
  - **The brief:** `inputs/translations/todo/README.md` (`tools/names/translations-todo.md`), with
    `check.py` (`tools/names/check.py`) to check an answer file.
- **`inputs/descriptions/todo/`:** `landmarks.jsonl` and `areas.jsonl`, things with an English
  Wikipedia article or register entry and no description, by fame; `README.md`
  (`tools/names/descriptions-todo.md`) is the writers' brief.
- Running the translators and writers is done on request.

**Descriptions** are `inputs/descriptions/**/*.jsonl` (docs/inputs.md §6.4): `{"qid", "long",
"src"}`, or `{"id": "n123"|"w123"|"r123", "long"}`; `"drop": true` removes one. They're laid over
the popups of details, landmarks and area overlays when serving, matched by `qid`, else the first
`wikidata`, else `osm`. Later file names win; `src` is credited.

## 8. Building

**Regions:** the agent works out what a region change affects from job keys. After an edit to
`inputs/regions/` the regions' work waits a quarter of an hour for more edits, an hour at most
from the first (`edit_held`).

**Scheduler.**
- **The plan:** every step's targets come with their keys (`state/build/jobs.json`); a target is
  stale when its key changed.
- **A new key scheme** (`agent::rekey`): when a step's keys change what they name, its records are
  re-keyed rather than built again: a record current under the old scheme gets its new key when
  every input the new key names is pinned by the old key's; else it's built again. Every reader
  re-keys records as it reads them (`agent::rekey::as_read`: the plan, the status, the checklist,
  the regions' state, `--expect-same`, `scenic-build p5-check`), deriving in memory
  (`rekey::derive`), a record a job saved always winning. The old schemes (`rekey::v1`, `v2`, and
  the terrain pieces' keys naming their sources as code had them, `rekey::SOURCES_BEFORE`) are
  kept for late records. `scenic-build rekey-check`, `scenic-build p5-check terrain`, `scenic-build p5-check trees` and `scenic-build water-key-check`
  say what they'd do, reading only. Pieces current only by derivation have no mid: it's made in idle
  time, expected the same (Order).
- **A job** is one step over a run of stale targets: as many as fit a quarter of an hour by their
  measured times (`coord::JOB_S`), a step kept to the lead in one job, so a failure or a new app
  costs one run.
- **Order:** the lead offers its plan in its order through its coordinator, and every Mac's agent,
  the lead's included, asks it for work (docs/pool.md §7.2). It plans again when what
  the plan depends on changes (`plan_seen`: the manifest, records, hand-offs, the gate, the pool,
  the pause, a newer app, switch files, catalogs, leases), at the earliest end of its timed waits
  (`replan_at`), and otherwise every five minutes (`plan_due`). Planning reads the terrain packs'
  indexes once each by content name, kept in the agent's `pack-idx/`. On the lead, the journal is
  merged each loop.
- **Jobs at once** (docs/pool.md §7.2, §7.7): every job starts by a grant of the lead's
  coordinator, the lead's own too, packed by their measures: a job beside others only when their
  measured memory, disk and cores fit the Mac's room. Never beside a cold start (a step none of
  whose targets has run), and one job at a time of a step kept to the lead. When the jobs hold more
  than the Mac's limit, the memory guard stops one beside the largest at its next safe point, or
  the larger at once (`agent::memguard`). Room is made only while no other job runs. Each slot past
  the first has its own scratch (`scratch-<k>/`), record and threads, and is a worker of its own in
  the history ("<host> #<k>"); the status has every slot's job in `jobs`, each with its slot.
  Several jobs per Mac only while every member reports the new app (its asks carry a capacity):
  a member on an older app is granted one job at a time and holds every Mac to one meanwhile, the
  old status fields (`job`, `beside`, `more`) written for it until it updates (docs/pool.md §7.1,
  Mixed versions).
- **A newly installed app:** nothing new starts and every running job is asked to stop at its next
  safe point (what it finished kept); once every slot is free the agent exits and the launcher
  starts the new one. A job reaching no safe point in time is stopped then (`Agent::update_wait`):
  the time its target under way is expected to need, by its progress's pace, else its expected time
  (its own measured run, its step's fit or prior, `needs::Book::secs`, from the coordinator's costs;
  else its step's time on this Mac lately) less the time since the job last noted a target done (or
  began), ×1.5 (a target's time varies by about a third between runs), at most 3 hours, never under
  15 minutes; 15 minutes for a target with no estimate. A member, without the coordinator's costs,
  goes by its pace, else its step's time there, else 15 minutes. Once 15 minutes have passed, a job
  whose processes used less than a core-minute of CPU over the last 30 minutes (the memory guard's
  sampler) is stopped: by its CPU, since a long step may print no progress for hours. The log and the status's Waiting line say how long and
  why. Pauses, the memory guard and the disk guard keep their own rules. The lead
  grants nothing to an agent on another app, and an agent with a newer app waiting asks for
  nothing.
- **Pausing** (`pipeline::control`): one pause for the whole build, every Mac's jobs and the pages'
  tasks.
  - **Asked for** from the menu bar (Pause Building; Option: Pause Building Now), the map's build
    panel or `scenic pause [--now]`, lifted by Resume or `scenic resume`: an ask in the agent's folder
    (`pause-request.json`) passed to the lead's coordinator; an ask older than the build's last
    change is passed over.
  - **Held** by the lead's coordinator (`coord/pause.json`, mirrored to `state/build/pause.json`) and
    told to every worker; each agent keeps what it last heard.
  - **At a safe point** (the default): each job finishes its target, notes it done (`SCENIC_DONE`)
    and exits as paused (75; `SCENIC_CONTROL` said "drain"); a job not there within 15 minutes is
    frozen instead. **Now:** every job frozen at once (SIGSTOP).
  - **While paused** no lease lapses, and a job stopped by the pause, sleep or a restart is given
    back, not failed.
  - A job without the NAS is frozen at once and goes on when it's back.
  - **Any job's end** records the targets it noted done.
- **Room on the disk:** before a job starts, when the Mac has less free than the job needs (its
  measured disk, at least 30 GB; the M1 15 GB; the OSM pass 80 GB), copies of what the NAS keeps
  lose files until there's a sixth more.
  - **Mid-job, safely by design** (`store::cachefile`, `dem/cachefile.py`): every cache read goes
    through one accessor that refills a missing file from where that cache fills from, so a wrong
    deletion costs a fetch, never a failure. A job holds each cache file it uses with a shared
    `flock`; a file is deleted only under an exclusive lock taken without waiting; a file is made
    under a temporary name and named only if the name is free. A guard test fails on code naming a
    cache path outside the accessor (`tests/cache_accessor.rs`), and chaos tests delete cache files
    under real steps and check the outputs are byte-identical. What can't be refilled (the DEM seed
    while the NAS hasn't it whole, the heritage clip without its planet, raw tiles and pageview
    months the NAS lacks) stays. What queued and running jobs read goes last (`room::Hints`).
  - **Disk measured** (docs/pool.md §7.7): what each job writes is counted by the accessor and by
    the agent's sampler over the job's scratch folder; the most a target took is its **disk floor**
    (the lead's `disks.json`). The **disk guard**: below the reserve, the fastest-growing job is
    frozen, room is made, and the job goes on or is given back.
  - The order of freeing: canopy squares and copies idle an hour, least recently used first; then
    raw tiles a folder at a time and the squares read since. A file goes once the NAS has it at the
    same size; what the NAS lacks is put there first (raw tiles packed as an archive), or kept.
  - **After the build** (`room::trim`): once the lead's forecast says nothing is left, each agent at
    home empties those copies once (the M4 keeps the canopy squares), and again only after a job has
    run there. Logged in its status (`caches.trimmed`) and the history.
  - **On the owner's ask** (`room::clear`): Clear the Build's Caches in the menu bar, or `scenic
    clean`, after a confirmation naming each cache and how it comes back, writes `clear-request.json`;
    once the build is done the agent empties every cache a later job refills from the NAS
    (`caches.cleared`), else says why not (`caches.declined`). The menu shows what it would free
    (`caches.clearable`, `caches.each`).
  - **The owner's room target** (`room::Target`, `room::toward`): `scenic room <GB>` / `scenic room
    off` or the menu's Disk Room, per Mac (`room-target.json`). A job starts only with the target free
    past its own room; one that can't waits, saying so in `waiting`. Whenever the disk is short of
    the target, the agent frees its caches toward it on a thread of its own, the cheapest to refill
    first (copies, base packs, the DEM seed, the heritage clip last), measuring as it goes; short with
    nothing more to free, it says so (`caches.room.short`). It never deletes what a job holds.
  - **With the mirror's reserve** (§4, Mirror): the two floors are apart; the mirror never frees for
    the agent's target and the agent never touches the mirror (§10, Gaps).
  - A trim, clear or freeing runs on its own thread; jobs run meanwhile. Nothing goes through a link.
    Each ends early when the agent is asked to stop.
- **Units run in map order** (by 10° square, then tile), so what one fetches serves the next.
- **Retries:** a failed job is retried after 10 minutes, doubling to 6 hours. A crashed agent's
  orphans are stopped at start when provably ours.
- **The heartbeat:** written locally each loop (~20 s) and to the NAS (`state/status.json`) when it
  changes or every two minutes. It holds the job, its progress (from the job's `progress:` lines)
  with the time left, and a checklist of the build to the end: a line per stage with an item per
  step (docs/pipeline.md §4), its targets done of all in its noun, its jobs left in order, and why
  work isn't this Mac's now. A job of several parts says them (`parts: <i>
  [names]`) and the status lists them, done, under way and to come. A long part says how far it is;
  osmium's `--progress` counts. A unit job's area counts by its stages, each weighted by its learned
  time (`unit-stages.json`). A part's time left comes from its own pace; a stuck part shows as such.
- **The heartbeat's resources:** each Mac's memory, cores and load, disk free and droppable caches,
  NAS latency and free space; its build caches (`caches`) and room target (`caches.room`).
- **The forecast** (`agent::forecast`), made with each plan and in the heartbeat: the work left run
  through in the order the agent runs it. The lead takes the pass's worldwide jobs, then a region at
  a time (terrain, then units), else the chains' work; its other slots any work in its order that
  fits beside it, a lane per slot its capacity packs (docs/pool.md §10, Each Mac's jobs); each other member the far end of the first shared step it can do (terrain's near end).
  Units reading stale terrain count `forecast::EXPECTED_STALE` (0.6) as work to come. A region's
  units' map tiles go out as they're built; it reaches the map once its last unit's do, the measured
  publishing time after it's done (`forecast::update_secs`: the publishing chain's jobs and catalog,
  summed per catalog). Each target takes its last run's time at the
  lead's pace, else its step's mean, else a first guess; each machine at its measured speed (half
  the lead's until measured). Run three times for a range. It says when each step, region and
  everything will be done, when each region reaches the map, and each machine's schedule, or why
  there's no finish.
- **The history** (`coord::history`): the last week's events (50,000 at most) on the lead's disk
  (`coord/history.jsonl`): jobs, leases, tasks, map tiles planned, catalogs, pauses, workers,
  agents, conditions, cache trims and clears. Summed by the hour for the worker page, and the
  forecast's measure of members' pace and of publishing's time.
- **Timings** (`pipeline::timings`; `dem/timings.py`; a page's task in `web/work/worker.js`): every
  job's run is broken into named phases, each with wall time, CPU time (`getrusage`), its class
  (`nas-read`, `nas-write`, `disk`, `net`, `compute`, `wait`, `mixed`), spans, bytes and files. One
  level of sub-phases; a child program's phases arrive as sub-phases (`SCENIC_PHASES_TO`). Rules:
  every bulk data move, compute stage and external program is a phase; `Out::open` and `Out::save`
  are phases for free; a loop is never a phase per iteration; nothing is timed per item in a hot
  loop; untimed time stays under about 5 %. Background and overlapped phases are marked. The record
  (docs/formats.md, Timings) goes to `SCENIC_TIMINGS`; the agent keeps `timings.jsonl`, the lead's
  coordinator `coord/timings.jsonl` for every worker. `scenic timings [kind] [--last N] [--host
  mac]` prints them (`--here` this Mac's own).
- **Publishing's times** (`pipeline::pubtimes`, task #250): each stage from a unit's job to its
  tiles on every Mac's map leaves an event. In the history: the unit's job ending; its hand-off
  merged (`merged`: each journal entry's key, member, step and targets); the map tiles
  first planned at their keys (`planned`: noted once per tile and key, what was noted known again
  from the history after a restart or a change of lead; with, per unit, the tiles its change made
  stale: their owners whose base pack or road values differ from the served catalog's), leased and
  done; and catalog n written (`catalog`: its `n`, its job's time, delta or whole, the files listed
  and stat'ed, its phases once written, from the catalog's own `catalog-stats:` log line). On the
  NAS, each Mac's map server appends to `state/publish/<host>.jsonl` its adoption of each catalog
  and its mirror's sync of it (begun, and through everything wanted). The Mac writing a catalog
  tells its own server at once (`/api/catalog/refresh`); the others look every 10 s while a build
  runs (§4). `scenic timings publish [--last N]` joins them: a unit is in the first catalog begun
  once it was merged and its stale tiles packed; per catalog and per unit, the hand-off, planned, leased, packed, catalog, servers
  and mirrors, the tiles each unit restaled, which of ferries, stations, roadunits, marks and
  names-todo ran, and the medians (the catalogs' phases split delta and whole).

**The pool** (`docs/pool.md`; `crate::pool`, `crate::agent::pool`; other workers: `docs/workers.md`),
always on. Every Mac's agent is a member; one leads each term. A process not in the pool (no NAS at
start, or its member lock held) starts nothing. `scenic pool [status]` shows it.
- **Who leads** is the terms' (`state/build/terms/`): the first Mac makes term 1 (or the owner takes
  over: `scenic lead`, pool.md §11) and leads it: it plans, runs a coordinator (`pipeline::coord`,
  port 8090) from which every other worker asks for work, and merges the journal into the records.
  A lead restarting or waking makes the next term naming itself; one on an app older than its term's
  stands down, and another member takes over after two minutes. With `state/pool/slots` on, a
  process changes part in place (docs/pool.md §7.6); otherwise it restarts into it between jobs.
- **What's offered** (crate::agent::every): every step to every member except the lead's own
  (`LEAD_ONLY`: catalogs, prune, `bld-fetch`, the OSM pass, GC, backup, input checks, …), the steps
  that keep the pass's answers, and the worldwide steps. The steps table (`agent::steps`, docs/pool.md
  §7.2) ranks the shared steps (`agent::steps::SHARED`): terrain's and slope's pieces, tree cover's
  pieces, units, the landmarks' candidates and peaks, bldprep and bldtiles. The status marks a step
  any member may take with ⇄ (`agent::steps::place`).
- **What fits a member:** each target is offered with its expected memory: a unit's from its piece;
  another's what its last run took (sampled four times a second, `SCENIC_COSTS`), else a related
  measure, else a first guess per step (terrain piece 3.3 GB, its assembly 1.2, tree cover piece 1,
  slope piece 1.5, peaks 2.5; the 3D buildings' by the rows they read, `agent::bld_peak`). Measures
  of older `coord::cost_version`s count for nothing. A member asks only for steps its disk has room
  for, never while a newer app waits, and takes the earliest step with a fitting target, from the
  far end of the plan (terrain from the near end).
- **The same app** (docs/pool.md §6.1): a member on another app than the lead's gets nothing (409,
  why in words), since its work would be recorded under keys another app's code made. Its jobs under
  way go on. A page's tasks run the coordinator's own WebAssembly; their results are checked (the
  first three, then one in eight: docs/workers.md §5).
- **The contact:** `state/coordinator.json`, the coordinator's addresses and the agents' token
  (pages need none: docs/workers.md §7), removed when the agent stops.
- **Leases:** ten minutes on the coordinator's clock, renewed by a beat each minute; a lapsed lease's
  work is offered again. Paused, every lease is held. The lead's own jobs hold leases too, so a
  target is never built twice at once. A worker's failed unit isn't offered to it for an hour,
  doubling.
- **The coordinator's state** (leases, costs, failures, the pause) is the term's, on the NAS
  (`state/coord/term/<E>/state.json`); the workers' token and accepted devices are the pool's
  (`state/coord/`); the history is each lead's own file per day (`state/coord/history/`).
- **Every job hands off:** under a lease `<term>-<id>` it saves into `agent/pool/jobs/<term>-<id>/`
  (`SCENIC_HANDOFF`, `pipeline::handoff`), then as an entry to the journal
  (`state/journal/<day>/<term>-<id>.json`) with its done record and costs. The lead merges entries
  into its term's records all or nothing, checking each against its step's write-set
  (`agent::steps::writes`) and, for a shared step, the files it saved. It writes the three files
  readers read (`manifest.json`, `jobs.json`, `pending.json`) from them. Until merged, the agent
  plans with its done records on top.
- **Raw terrain tiles a job fetches:** it packs them and puts the archives on the NAS itself, handing
  the lead only their names (`Handoff::raw`); the lead names them in the raw store's index, which
  it alone writes.
- **A job that fails** hands off the targets it finished; the rest are held against it.
- **A catalog** goes out only once the lead's records reflect the journal. **GC** runs only on a
  step that re-asserted the lead's term; it keeps every file the newest records or an unmerged
  journal entry names (`agent::gc::unmerged`), and renames a file to a tombstone first, putting it
  back if touched since.
- **Temporary names** are each Mac's and each process's own.

**Order:**
0. **The input check** (`inputs <unit>`, docs/inputs.md §4.3), before every other job: a unit whose
   drop box, acceptances or accepted index changed is checked (in full once a day), in the first
   slot or beside a job.
1. **The OSM pass**, when the NAS holds a newer planet.
2. **The pass's worldwide jobs:** `pass-sets`, `outlines` (the rest waits for them), hiking routes'
   ends, `reach`, `terrain-z8` (once), roadside buildings (once per release; units wait), summits,
   labels, the lake fill, the water.
3. **The regions' build,** a region at a time (`agent::build::plan`):
   - heritage-sites first;
   - regions the map hasn't at all first, then those it has; of each, the one with the fewest units
     left first. For each: the stale terrain areas it reads (pieces, then each assembly once its
     pieces are current with mids), then its units whose terrain is built, then slope and tree
     cover; but first, the slope and tree cover of the regions done that the map hasn't as they are
     now. Other members take the earliest shared step with work that fits, from the far end
     (terrain from the near end).
   - **Publishing**, before all of the regions' work (after heritage-sites), as soon as the records
     change, from the records as they are (nothing is fixed as of a time): a prune of what the
     coverage no longer builds (§5, Shrinking), then the publishing chain (4, Roads), its map tiles
     whose owners changed a unit's together (`build::by_unit`: the units whose base pack or road
     values differ from the served catalog's, in build order), offered in the plan's order to every
     Mac asking, the lead's agent among them (docs/pool.md §7.2); once the map tiles and the road
     index are drawn (`build::CATALOG_GATE`) and none of their hand-offs waits to be merged (or ten
     minutes after it was first held back), a
     catalog (5), listed before the chain's rest (the low zooms, rail stops, ferries, roots), which
     goes out with a later one. Nothing is published before a unit is
     built, while the regions have work left. The lead keeps `publishing.json` in its agent's folder
     (`build::LastCatalog`: when its last catalog went out, the last with the low zooms made, and
     when a catalog due was first held back since),
     and records what publishing does and why (`build::PublishLine`), shown in the build status
     (`publishing`), the worker page and `scenic status`, and logged when it changes.
   - **The low zooms' cadence** (`build::low_gate`): the low zooms (roads' and rails' lo packs,
     terrain's and slope's roots, and with `near-slope` the slope and tree cover assemblies) are
     planned when nothing else is queued (no unit, terrain, slope or tree piece, 3D buildings or
     depth work), at once; else `build::LOW_EVERY_S` (three hours) after a catalog last went out
     with them made (`LastCatalog::low_at`), when half the roads' lo packs are stale
     (`build::low_due`), or once everything is built. The gate decides only when; their keys stay
     by content. Zoom ranges don't overlap, so a region new since shows its roads from z9 until the
     low zooms are next made.
   - **Publishing's switches** (crate::agent::publish), under `state/pool/`, each off while missing
     or `off`:
     - `delta-catalog`: the catalog job (`scenic-build catalog --delta`) writes what changed over the
       last (docs/formats.md, Delta catalogs), each file's size from the records
       (`state/build/sizes.json`, which the files' jobs hand off) or the last catalog: it looks up no
       file's size on the NAS but a file written before sizes were recorded. The whole catalog, which looks
       at every file, is an idle-time job (`scenic-build catalog --whole`) the lead's agent runs
       when the deltas add up to a whole one (`store::catalog::whole_due`), or a delta's run nears
       `MAX_CHAIN`. Readers apply deltas (`store::catalog::latest_nas_from`). Switch on only once
       every Mac's server reads deltas. Held catalogs are always whole.
     - `near-slope`: a done region's slope and tree cover are the pieces near it
       (`Publishing::near`), not every stale piece around; it's recorded built with those.
4. **Four chains:**
   - **Roads** (the publishing chain, `build::publish_chain`): a prune of map tiles no unit is near,
     pack (the map tiles), the road → units index, lo, stations, ferries, terrain and slope roots,
     its first stale step at a time.
   - **Rail service**, from the start: `rail-feeds`, then `rail` (§6, Rail service).
   - **Landmarks**, from the start, each step once what it reads is built
     (`agent::build::landmarks_work`): candidates, peaks, items, the heritage chain, marks, then the
     overlays. Marks at most hourly while units build (`MARKS_EVERY_S`), the overlays at most every
     6 h (`OVERLAYS_EVERY_S`) on their own gate, from the dots the last marks run wrote; both at
     once after the last unit.
   - **Gated by time** (`build::GATED`: marks, overlays, names-todo): the gate decides only when
     a step is planned; its key stays by content. When each last ran is the lead's observation of
     its recorded key changing (`LastCatalog::observe`, kept in `publishing.json`; not known: due).
   - **3D buildings** (`docs/buildings3d.md` §3.3; `agent::build::bld_work`): `bld-fetch`, a prune,
     `bldprep` per z6 tile, then `bldtiles` once a tile and its 8 neighbours are prepared, in the
     regions' order.

   The trains', landmarks' and buildings' work is listed after the regions'. Last, in idle time: the
   mids of pieces current without one, each made again with `--expect-same` (`scenic-build
   trees|terrain|slope --expect-same`: a pack differing from the manifest fails the job).
5. **A catalog** whenever the served files, the regions or which of them are done change
   (`build::catalog_key`), at most one each `build::CATALOG_EVERY_S` (a minute) since the last
   began. It waits while the map tiles or the road index are left to draw (`build::CATALOG_GATE`)
   or a hand-off of theirs waits to be merged. That wait is counted from when a catalog due was
   first held back, whether or not one was made before (`Publish::held_for`: the lead's agent keeps
   that time in `publishing.json`, `LastCatalog::held_at`, so a restart doesn't reset it; a new
   lead's agent, with a record of its own, starts the count afresh). It's reset by each catalog
   made, and when none is held back, none is due and none waits out its minute: past
   `build::CATALOG_CAP_S` (ten minutes) one goes out with what's drawn and merged, its minute
   allowing, the status saying which held it (the map tiles or the road index to draw, or their
   hand-offs waiting to be merged). It records as built (`--ready <id>=<outline
   digest>,…`) the regions done whose units are merged, whose map tiles are drawn and merged and
   whose slope and tree cover are made, and the others as the last catalog had them; the status
   says why a done region isn't yet ("waiting to be merged", "map tiles to draw", "slope and tree
   cover to make"). A unit's hand-off waiting holds no catalog: it goes out with a later one.
   While `inputs/hold-catalog` exists, catalogs go to `catalog-held/`.
6. **Daily:** backup and GC.

**Planned:** stale work after step-version bumps done oldest first in idle time; registers, Overture
and timetables refetched every ~6 months.

**Interruptions.** Any Mac may be asleep, away or unplugged at any time; nothing depends on it being
available. Until work is done, the map serves the last catalog.
- When the NAS goes, the agent freezes the job (`SIGSTOP`) and resumes it (`SIGCONT`) when it's back.
- Sleep often breaks SMB handles, so a job touching the NAS restarts after wake.
- **Kills** lose only the target under way: each target is noted done as it's saved. The OSM pass is
  a chain of stages with completion markers. The planet's copy and the mirror resume by byte range.
- **Atomic writes** (§3); **caches are disposable**.

**Status.**
- `scenic status` (the lead, its jobs, what waits, the checklist by stage and step, the regions),
  `scenic steps` (the steps table: docs/pipeline.md), `scenic clean`, `scenic room` (Room on the
  disk, above).
- The app's status bar: the NAS or map server out of reach, new data or a new app.
- **The menu bar item** asks the local server (`/api/build`): this Mac's agent's status, else the
  NAS's copy. An agent not heard from for six minutes shows as out of touch.
  - **Its panel:** a click opens the lead's build page in a popover (`/api/build`'s `pages`, the
    first that answers: HTTPS through `tailscale serve`, else the lead's addresses), with `?view` in
    a web view that keeps nothing, so it only watches (docs/workers.md §7).
  - **Its menu** (right-click, or "⋯"): the state, downloads and caches; Pause/Resume Building; the
    lead items (docs/pool.md §11); Clear the Build's Caches; Disk Room ▸; Open the Build Log; Open
    the Map; Copy the Map's Address; Open/Copy the Build Page.
  - Its timers run in the run loop's common modes, so it updates while a menu is open
    (`scenic-status --menu-proof`).
  - **The input check** (docs/inputs.md §4.7): a held input badges the icon, its menu has a line per
    held unit (Accept its warnings; Show on the Build Page), and it notifies. `scenic status` and
    `scenic inputs` print the units' states; the build page shows a banner
    per held unit.

**Determinism:** the same inputs give the same bytes, on any machine and in WebAssembly: sorted
outputs, no hash-map order, reductions independent of thread count, and every transcendental
function from one implementation (the `det` crate, over `libm`). Checked per step by hand and by
`tools/check/`; planned: a "build twice, compare hashes" test per step.

**Validation:** every upload is read back and checked against its hash, and a catalog fails on a
missing file. A held catalog is compared with the current map by hand. Planned: decode checks,
plausible ranges, and holding a publish when a unit loses over 20 % of its ways without its coverage
shrinking.

**Tests:** a test holds on any Mac in any state, so none turns on the wall clock, the real machine
or its siblings' order:
- A decision is a function of its inputs, the time among them (`edit_held`, `runs_at`).
- Code that waits takes a clock (`store::clock`: `Offload::clock`, `Shared::clock`,
  `Side::set_clock`); tests use a virtual clock and count waits rather than time them.
- The agent reads its Mac through `cond::Mac`, fixed in tests (`Mac::TEST`), the disk's free space
  set (`room::TEST_FREE`, `free_set`).
- Exchanges with real threads or processes hold on gates, not sleeps.
- What must happen eventually is waited for, bounded only by a five-minute watchdog that says what
  never came.
- File times come from the time the code is given, or far off.
- A test that would see other tests' work runs alone in its own process. Unit tests are offline.
  Tests reading real data (the NAS, `data/build`) are `#[ignore]`d.

**Format bumps:** publish an app that reads both forms, then the data; drop the old reader once
everything is rebuilt.

**App publishing:** `tools/app/publish.sh`, run on either Mac with the NAS mounted:
1. takes a lock on the NAS (`app/publish.lock`, create-new; one over six hours old taken over);
2. refuses unless the tree is clean (the task board's db aside) and HEAD is on origin's main;
3. refuses while the lead's role's old name appears in the code, docs or UI (`tools/taskboard`
   exempt);
4. builds the server and pipeline;
5. runs the crates' tests, the type check and the web build, and checks every Python step loads
   from a copy of `dem/` in a fresh environment and passes pyflakes;
6. smoke-tests a server on a spare port against the NAS's catalog;
7. writes `app/<version>/` and `app/current.json` with every file's SHA-256;
8. copies `tools/nas/fetch-planet.sh` to the NAS's `nas/` when it differs.

`publish.sh --rollback` swaps `current.json` and `previous.json`. The agent never fetches from git.

**Programs:** `scenic` (the user's command and the agent); `scenic-build` (the build steps);
`server`; the unit's programs `extract`, `elev`, `areaflags`, `landcover`, `tile`, `scenic-metrics`
(rail runs `extract` and `railfreq`, tree cover `trees`). The app also carries `dem/` (the Python
steps, run in `dem/.venv`, which uv makes from `uv.lock` at a Mac's first Python step: §10),
Scenic.app, `web/` and `fonts/`. `tools/app/install.sh` gives the agent a PATH with uv, osmium, zstd
and Java 21.

## 9. Sizes

**Measured** (the 2026-09-28 pass): planet 95.1 GB; filtered 60.6 GB; sets 10.9 GB; outlines 2.7 GB;
pieces ~58 GB; basemap 28.6 GB (Planetiler needs ~6× its 16.3 GB input while it runs); water 1.38 GB.

**Estimated:**

| | today's coverage | whole world |
|---|---|---|
| road values | ~8 GB | ~8 GB |
| base packs | ~30 GB | ~300 GB |
| our layers | ~150 GB | ~1.2 TB, with buildings |
| sources kept per pass | ~200 GB | the same |
| roadside buildings (once per release) | ~40 GB | the same |
| 3D buildings (`docs/buildings3d.md` §2.5): sources, normalized files, packs | 62 + 17–22 + 4.6–6.9 GB | not planned |
| depth (§6, Depth): its sources' grids; its packs | a sample's: 15″ grid 6 MB over 9.5° × 6°; 56 US coastal tiles 0.6 GB of grids → a z6 tile's packs 168 MB (4,173 tiles); the depth lab's estimate, quantised: ~25 GB seas, 5–20 GB surveyed lakes | — |
| an app Mac's mirror | the World ~10.5 GB; a region tens of MB to ~20 GB | as downloaded |

The NAS holds about two passes' sources at most (a retired pass's go 14 days after the next
completes).

**Time, measured** (`scenic timings`, both Macs' jobs 2026-10-09 → 17, 653 runs; a step's share of
the jobs' wall time, and where a run's time went):

| step | share | a run | CPU/wall | where the time went |
|---|---|---|---|---|
| unit | 34 % | 10 min (19 in a rebuild of every unit's elevations) | 0.4 | `elev` 55–78 %: the blends' distance scan 34–48 %, 3DEP read over the network 18–27 %; the DEM seed copied 5 %; the base pack uploaded 5 % |
| bldtiles | 22 % | 14 min | 4.0 | the areas' tiles made (compute) 88 % |
| terrain | 10 % | 5 min | 1.5 | tiles prepared 58 %, uploaded 15 % |
| names-todo | 6 % | 11 min | 0.1 | every base pack's names read from the NAS, after each catalog |
| pack | see `scenic timings pack` | see `scenic timings pack` | — | what a tile reads: its `units` and `bytes` sizes (crate::costin) |
| slope | 5 % | 2 min | 0.4 | its terrain pack copied 29 %, uploaded 45 % |
| heritage | 4 % | 81 min | 1.0 | the filtered planet clipped (osmium, once a pass) 86 % |
| peaks | 3 % | 3 min | 1.1 | z12 tiles read one by one from the packs on the NAS (25,000 files a run) 75 % |

A tail task on the other Mac fetches 0.65–1 GB at 2–6 MB/s (the Macs reach each other over Wi-Fi at
about 4 MB/s; the NAS at 25–55 MB/s), so a tail is worth offering only to a Mac that would otherwise
idle. `elev`'s memory is the point arrays (30 M vertices: ~2 GB) and the files' decoded blocks
(docs/inputs.md §6.8: one 512 MB cache).

## 10. Phases and status

1. **Foundations: done.** The `store` crate; the server serving from packs, base packs and catalogs.
2. **Agent: done.**
3. **The OSM pass and global-source layers: done**, but the sea mask (planned).
4. **Per-unit pipeline and rankings: done**, but determinism tests and validation (§8).
5. **Browser: done** (`docs/phase5.md`), but drawing, splitting and merging regions in the app.
6. **Cutover: done.** Left: the NAS's files from the build before the agent, which nothing reads
   (`global/legacy/`, `layers/basemap/legacy-*`, `sources/legacy/`, `sources/registers/legacy`,
   `sources/registers/legacy-seeds`); `pipeline::legacy` still names live code; the `terrain` and
   `slope` programs' build-folder modes (`terrain --scan` is live).
7. **Features,** each on its own. Built: the terrain repair and fix (§6, Terrain), the water (§6,
   Water), 3D buildings (`docs/buildings3d.md`). Planned: single-pixel towers of 100–300 m at z7–z9
   judged against the finer level; building heights in horizons and the viewshed; sharper terrain
   from national DEMs; PLATEAU.
8. **Builds anywhere: under way** (`docs/workers.md`). Next: OPFS, ranged reads, journaled group
   commits.

**Gaps:** the code falls short of the design here.
1. **The pool** (`docs/pool.md` §12 lists what's open in detail). In short: the build page is served
   by the lead alone; members joining and leaving aren't in the history; without
   `state/pool/slots` a process restarts to change part; readers read today's three files, not the
   snapshot; the raw tiles' archive names stay in the records; the coordinator's term state is
   written on the loop after a grant; members' messages go by mailbox; the forecast doesn't model a
   first job waiting for the second's; the memory guard samples every 5 s; write-sets are a
   predicate over names, not a type; the budgets meter rate, not concurrency; resuming covers the
   OSM pass alone; brokers and placement have the limits pool.md lists; create-new between two
   Macs and the share's stale-read window are unchecked on the real share; GC of the journal itself
   isn't built, so the records snapshot keeps every key; `STALE_S` is an assumption. The lead's own jobs get no thread grant
   (docs/pool.md §7.1, Threads granted): they keep the agent's old rule (all the cores; the second
   job half, or all beside a job waiting on a worker), so the lead's two jobs together may run
   more threads than it has cores.
2. **The Python steps' environment is made inside the app's version folder**
   (`app/<version>/dem/.venv`), so the published copy changes and each version makes its own
   (~380 MB). Fix: one environment per lock file, made as an app is installed.
3. **Downloaded areas offline** (§4, Mirror): a whole road leaving a downloaded area fails away from
   the NAS when a unit it crosses isn't on the Mac, and a way starting over 2 km outside has no way
   info there. Fix: cut the road at the area, and keep the owners of the ways the area's hi data
   list.
4. **Meta's canopy squares on the equator row kept as "none":** two squares asked for under
   `lat=0.0` before `trees::chm_urls` asked for `lat=-0.0` are kept as empty files
   (`sources/canopy/meta_chm_lat=0.0_lon={100.0,-60.0}_*`, and in both Macs' caches). No road reads
   them today. Fix: remove the six files before any region reaches south of the equator.
5. **The water's deeper zooms need the basemap** (§6, Water): away from the NAS, z10+ is drawn only
   where the basemap's pieces are downloaded; elsewhere the server scales up the nearest stored zoom,
   uncached.
6. **The pass's `water` set serves only the shoreline check** (~6 GB a pass).
7. **The mirror doesn't know the room target** (§8, Room on the disk): a download can fill room the
   agent freed toward a target above the mirror's reserve. Fix: the server keeps its reserve at the
   larger of the two.
8. **Names by language** (§7):
   - the area tables' folders (`translations/<area>/`) are still on the NAS; delete them once a
     published server shows the converted lines;
   - the converted lines hold for their area's languages, not those spoken where their things are;
   - roads carry no language tags (planned: the unit job writing them, with the next pass);
   - the basemap carries `name:en` and `name:fr` alone until the next pass;
   - Wikidata's English labels aren't carried (`w_en` is in the popup record only);
   - the to-do lists don't read the basemap's things, stations, ferries or area overlays' names, and
     the descriptions' list lacks heritage areas, Indigenous lands, special areas and World Heritage
     outlines;
   - refinements cover only the coverage's subdivisions (Geneva reads German first; Åland Finnish);
   - near a border a thing may take its neighbour's languages (the outlines are simplified to 1 km).
9. **A worker's tail in a one-unit job holds its slot** (docs/workers.md §3): the job waits on a
   worker only while its pace says it'll be back first, and says so (`SCENIC_WAITING`), so a second
   job may start beside it on the lead's slots (docs/pool.md §7.7). A member's waiting job still
   idles its slot.
10. **The timetable feeds** (docs/inputs.md §6.7.1): the catalogue is never refreshed (a new copy is
    put by hand: `scenic-build put sources/rail/catalogue csv feeds_v2.csv`); a declaration's missing
    key isn't a warning; the declarations' credits are in no catalog yet; rail-feeds' key names the
    accepted index, so an edited note reruns it.
11. **The terrain's water keys for lakes without an OSM id** (`terrain_water::own_key`): from a
    tile's 16th feature on the key's bits meet the row's, so two tiles of a column could give two
    lakes one key. No live lake lacks an id; a test pins the keys. Fix with the next `TERRAIN_V`.
12. **The worldwide z8 and the peaks' z12 outside the packs** take the repair but not GLO-30 nor the
    water; the z8's version (`terrain_z8::V`) isn't tied to `TERRAIN_V`, guarded by
    `terrain_z8::tests::the_raw_path_is_versioned`. A lake across two z6 tiles may take levels a
    metre or two apart.

13. **Depth** (§6, Depth): the map's 3D floor toggle isn't built (the server's `?floor=1` is); the
    lakes' estimated depths (GLOBathy's and the small-lake estimate's bowls, the depth lab's lakes
    chain) have no producer, so only surveyed lakes have depth; GtTM is read from windows made by
    hand, SHOM's from its archives unpacked by hand (no range reads of the HDF5 grid or the 7z yet);
    a piece's key names its water's z6–12 tiles, so a change the basemap shows only at z13–14
    doesn't remake it; MSL sources aren't drawn in lakes (the Great Lakes' source is `ncei-greatlakes`);
    EMODnet's interpolation flag isn't applied (emodnet.py reads the MSL ASCII tiles alone); CRM
    volumes on EGM2008 aren't corrected to MSL;
    rivers the basemap draws as rivers get none; targets the coverage leaves keep their packs (no
    prune yet); the map's click popup for depth isn't built (the readout is).
14. **Heights to EGM2008** (inputs.md §6.8, step 3): where no declared geoid source has a value
    (3DEP off the conterminous US and Alaska), a dataset's heights stay in its reference; in a blend
    band whose first dataset was converted and whose dataset beneath has no value, the seam mixes references (counted, dem-stats'
    `unconverted_seam`, not corrected). NAD83(2011) and NAD83(CSRS) are taken as one frame, WGS 84 as
    ITRF2008.

## 11. Risks and checks

- **NAS throughput:** a hung SMB session once stalled reads, even a forced unmount, for minutes.
  Planned: a rate limit on the pass's uploads, and remounting a hung mount.
- **The Macs' availability:** work progresses only while a Mac of the pool is awake and reaches the
  NAS; the lead's duties wait for a lead (another member takes over: §8, The pool).
- **Dense units** (Kanto, a 3 GB base pack): if a unit doesn't fit in 48 GB,
  units split into z7 or z8 tiles.
- **Remote DEM servers** may be slow or change; the units' samples are kept on the NAS.
- **Version bumps at globe scale** would take days.
- **Way ids past u32** (2040s): the tile format is versioned.

## 12. Decisions

Why the design is as it is, where the code alone doesn't say.
- **No SSH from the Macs** (1Password asks for every new session): GC deletes over SMB, uploads are
  checked by reading back, the planet fetch runs from DSM.
- **The NAS by its LAN name:** measured 60 MB/s on the LAN against 12 MB/s through Tailscale's
  userspace networking.
- **What a step needs is measured, enforced at use, or derived from code** (docs/pool.md §7.7),
  never a hand-kept figure: a wrong declaration may cost speed, never correctness. A job far over
  its memory is stopped, not frozen (a frozen job keeps what it holds), and what it held becomes its
  target's floor.
- **The raw tiles' packing stays in the terrain piece's job:** measured at about 1 % of a piece's
  time.
- **A member builds only on the lead's app**, since a change without a key change would leave a
  result under a key that calls it current.
- **Downloads only by the owner's choice:** the basemap is split by z6 tile rather than copied whole
  or sparse; copies keep to 20 MB/s during the build rather than pausing.
- **Names by language, kind and name** rather than area boxes: a line serves wherever its language
  is spoken; a thing's sourced English belongs to it alone. Hong Kong's and Macau's names read in
  Cantonese.
- **Landmarks, heritage, rail and buildings run as chains apart from roads,** so Wikidata, pageview
  or operator outages never hold up the roads, and no road pack depends on worldwide rankings.
- **The rail service keeps the feeds it has:** a zip counts from the day it was fetched.
- **The terrain repair weighs blobs, not pixels:** a per-pixel ring left inner layers of clusters
  and needed repeated passes.
- **A unit's key names the terrain tiles it reads, by content;** the peaks' key names the terrain hi
  packs within 30 km and `TERRAIN_V` instead (the summits change with any terrain fix anyway, and a
  finer key would spare little).
- **A key scheme change re-keys records** rather than rebuilding what would come out the same.
- **Terrain, slope and tree cover a z6 tile at a time,** with z3 assemblies from mids: a change
  remakes only its pieces, and any member can take them.
- **The water as exact coverage, drawn as a raster:** vectors can't draw a sub-pixel polygon's area.
