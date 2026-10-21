# Macs

Each Mac's agent, server and mirror, and the devices the map opens on. The pool itself is `docs/pool.md`; tasks for any worker, `docs/workers.md`.

**Each Mac's agent.** Every Mac runs the agent (`scenic agent`, set up by `tools/app/install.sh`), a
member of the pool (docs/building.md §2, The pool): it leads while its Mac holds the term, else takes work from the
lead's coordinator. Below, the M4 (48 GB) unless said.
- It runs under the launcher as a LaunchAgent with `ProcessType Interactive` (else launchd throttles
  it).
- **Several jobs at once** (docs/building.md §2, Jobs at once). Each job is a child process group at utility
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
    as one archive beside its shard's others (the NAS's small-file writes run at ~23 a second). A
    shard's archives are each kept more than twice the size of all those after it, else merged, so
    a shard has a dozen at most. Archives leave the index through its `gone` list and are deleted a
    day later, so a reader of the old index stays right. Only the lead changes the index; a job
    puts its archives there unlisted and hands them off. A tile is read by range for a shard's first
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
    copies of record files staging reads (`blobs/`: hi packs, roadside buildings' z8 tiles, z6 tiles'
    kept DEM samples), pageview month indexes (`items/months/`).
  - **Cleared too, on the owner's ask:** the pack cache (`base/`, the base packs' way indexes for
    pack(T) and lo), the
    per-vertex DEM seed (only while the NAS has it whole), local copies the summits and peaks jobs
    read, and the heritage job's clip of the pass's `whs` set (made again by osmium).
  - **Kept:** the answers (`items/`, `heritage-<date>/`), the trains' stop pairs (`rail/`), the z6 tile
    stages' timings (`unit-stages.json`), what a z6 tile kept that isn't on the NAS yet, and the
    coverage as last read (`coverage/<key>.cov`, reused while every recipe and outline file has the
    name, length and time it had).
  - Nothing is deleted through a link, nor anything in the NAS's project folder.
- **Its own map** is served from the NAS and what it has downloaded (Mirror).
- **An OSM pass** starts with 80 GB free (the pack cache counting as free) and copies the planet to
  the SSD when there's room, else reads it from the NAS.
- **What a z6 tile keeps for its later runs** is on the NAS (`cache/`), shared by both Macs: its DEM
  samples (`dem-units/`) and canopy and view results (`scenic-units/`, docs/steps.md §6). A z6 tile's build
  folders go once it's built.

**The M1 (16 GB)** leads when it holds the term; as a member it builds what the lead's coordinator
gives it (docs/building.md §2, The pool): any shared step's jobs that fit the memory its owner gives the build there
(`scenic memory`, within the memory guard's limit: docs/pool.md §7.1) and its disk (its room target kept free past each job's measured disk; a task 5 GB, tree cover's rows and terrain's subtrees 1 GB), from the far end of the list (terrain from the
near end), else z6 tiles' tails (`docs/workers.md`). A z6 tile's predicted peak is the most its job's
processes held together last time, else about ten times its extract, never under 3.7 GB nor below its
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
  the languages spoken there and the spoken-languages raster (docs/names.md). The basemap's come from the
  catalog, so its 304s read nothing.

**Mirror, per Mac** (`store::mirror`, `store::pieces`, `crates/server/src/downloads.rs`).
- **Only what's downloaded.** The map reads a file from the Mac when it's there, else from the NAS.
  What's downloaded is this Mac's alone (`downloads.json` in its home).
- **The World, zoomed out:** the build's worldwide files (`global/`: only those a step makes, the
  catalog lists no other), the Canadian lake fill
  (`work/lakefill/<date>`, read by the water's deeper zooms anywhere in Canada), every layer's root
  and lo packs (z0–8), markdata, ovdata, and the basemap's zooms 0–10: about 10 GB. Not the pass's
  outlines.
- **A region, or a view:** every layer's hi pack and the base pack and road values of each z6 tile
  within 2 km of it; the basemap's zooms 11–14 over those tiles; the terrain's and grids' hi packs
  within 25 km (the viewshed's reach); the hi data of the z6 tiles within 50 km. With the World,
  that's what the map reads there (but task #457).
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
  lo their owners' base packs and road values (whole, mapped, in place of their part reads), the marks and overlay conversions the heritage
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
