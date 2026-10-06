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
  checklist of every step to the end, pauses and resumes the whole build, and clears that Mac's
  build caches once the build is done (`scenic clean` too); it sends a notification for every
  change.
- The server mounts the NAS itself when it's missing, at home: at start, then from its periodic
  check.

**The Regions panel** (Settings → Regions):
- **What it does today:**
  - It shows the regions, and on the map the coverage the published data was built for. A recipe
    the catalog doesn't have yet (added or redrawn since) shows as pending.
  - It makes new regions from administrative areas (levels 2–8 and ISO 3166, from the pass's
    outlines), found by name or by a click on the map (the areas containing the point), each shown
    with its area in km².
  - It renames regions (rebuilding nothing) and removes them: what only a removed region covered
    leaves the map with the next build.
  - Edits made away from home wait on the Mac and go to the NAS when it's back.
- **From a terminal:** `scenic add` and `scenic remove` do the same, straight to the NAS.
- **Planned:**
  - drawing and redrawing outlines (dragging points; saved as a `.poly`);
  - splitting and merging regions;
  - Geofabrik units and place-and-radius outlines in the panel (recipes take both already);
  - each choice's cost in build time.
- Coverage is drawn on the map. The status bar shows the build Mac's state and offers to reload when
  new data or a new app is in.
- New areas appear when the build publishes its catalog: a round of publishing goes out as regions
  are done, about every hour while the build goes on (§8, A round).

**Translations and descriptions** (§7). The user's two folders on the NAS:
- `translations/` holds finished translation files: the map shows them within a minute or two of
  being dropped there, with nothing rebuilt.
- `descriptions/` likewise holds descriptions.
- Planned: the agent will list what still lacks English, or a description, in each folder's
  `todo/`.

**Everything else is automatic:**
- **Building and refreshing:** whenever the build Mac is awake, reaches the NAS, and has power
  (plugged in, or on battery down to 30 %). Away from home it builds through Tailscale, slowly; the
  OpenStreetMap pass and the other jobs that move the whole planet or world through the NAS wait
  for home.
- Mirroring to each Mac.
- Emptying each Mac's build caches once the build is done (§8, Room on the disk).
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
3. **Heritage sites and designated areas:** one job over the coverage plus 20 km.
4. **Global-source layers, per z3 pack near the coverage:** terrain, then slope, then tree cover
   (terrain with the first region that reads it; slope and tree cover as the regions in their area
   are published: §8, Order).
5. **Per unit**, for every unit meeting the coverage, a region at a time: base(U), the ways U owns (a way belongs to the
   unit of its first node) that touch the coverage, with per-vertex elevations, grade and scenic
   channels. Each value is computed once.
6. **Then three chains**, which don't wait for each other:
   - **Roads:**
     - the road → units index;
     - pack(T): tiles, query parts and indexes for each z6 tile T the units' ways reach, from the
       base data of every unit whose ways come within 100 km;
     - lo packs per z3 tile;
     - rail stops and ferries;
     - the root tiles;
     - then a catalog, as each region is done (at most hourly) and after the last.
   - **Rail service** (§6): the timetables of the rail feeds where the coverage is, each fetched
     once; then trains a day on the coverage's rail ways.
   - **Landmarks** (`docs/phase5.md`):
     - candidates and peaks per unit;
     - then worldwide: Wikidata facts and pageviews;
     - the heritage chain;
     - landmark points (marks);
     - area overlays.

Because road values come from the whole planet, adding a region changes nothing outside its own
units and the packs their ways come within 100 km of.

## 3. Storage

NAS root: `personal/projects/scenic-roads/`, on the `personal` share. Its Recycle Bin is on, but
deletions over SMB bypass it (tested 2026-10-02).

```
translations/  descriptions/  the user's drop-ins (descriptions/README.md; todo/: planned)
inputs/        regions/<id>.toml, outlines/ (.poly; geofabrik/), ferries/freq/ (timetables), moi-dtm/
               (Taiwan's DTM, put there by hand), keys.env (API keys, KEY=value lines: the rail
               feeds'), hold-catalog
sources/       osm/<date>/ (planet, filtered, pieces/, sets/, roads/, outlines, reach, pass), basemap/
               (Planetiler's jar and data), registers/ (the registers snapshot), items/<date>/
               (the pass's items' facts and pageviews, and the answers Wikidata and Wikipedia gave
               the items job and the heritage chain: §4), pageviews/ (Wikipedia's monthly
               pageviews), dem-cache/ (today's per-vertex DEM cache, the units' seed),
               buildings/<release>/
               (Overture's building boxes for the world, in z8 tiles, and their index), trees/
               (leaf/: the leaf-type squares; NALCMS's GeoTIFF), canopy/ (Meta's canopy squares),
               aws-terrarium/ (AWS's raw terrain tiles), fabdem/ (FABDEM's 1° tiles), rail/ (the
               rail feeds: the catalogue, their zips, the MTR's lines; §6), terrain-z8-v1, legacy/
               (today's map's build inputs, until the cutover)
base/          base packs, one per unit
hidata/        per z6 tile: the ways-here index, query parts, climbs, rail lines, zoomed-out summaries
markdata/      per z6 tile: landmark points
ovdata/        per z3 tile: area and park details
global/        worldwide files: road values per unit (roads/), road → units, rail frequencies,
               landmark totals, heritage/, legacy/ (today's converted files)
layers/<layer>/  root, lo and hi packs; basemap/world-<date>.<hash>.pmtiles
work/          build intermediates (not served)
cache/         what units keep for their later runs, shared by both Macs: dem-units/, scenic-units/
catalog/       <n>.json.zst: every file the map reads, by content name
catalog-held/  a catalog kept back for review (while inputs/hold-catalog exists)
app/           published app versions; current.json, previous.json
state/         status.json (the agent's heartbeat), build/ (manifest, job keys, pending, summaries),
               backups/, logs/ (the planet fetch)
nas/           fetch-planet.sh, which the NAS runs itself (from tools/nas/; publish.sh copies it)
```

**Immutable, content-named files.**
- Every built file is named `<logical>.<hash16>.<ext>` and never rewritten.
- A write goes to `<name>.tmp`, is read back and checked against its hash, then renamed.
- An unchanged file keeps its name, so a rebuild uploads only what changed.
- **Catalogs** list every file the map reads: one zstd JSON per publish, with zstd's content
  checksum. Readers take the highest that decodes. Each also records the coverage it was built for
  (the regions' outlines, simplified) and the credits of the sources its data comes from.
- **The build manifest** (`state/build/manifest.json`) lists everything built: sources, work and
  outputs.
- **Job keys** (`state/build/jobs.json`) say what each output was made from.

**Deletions go through SMB.** They're permanent on this share. The build Mac can't use SSH
unattended, because 1Password asks to approve every new session.

**Copies carry the bytes and permissions, nothing else** (`store::sys::copy_data`; `cp -X` in
the scripts). macOS puts a provenance attribute (`com.apple.provenance`) on whatever an app writes,
and the share refuses a copy's attempt to set one that differs from its folder's. That makes
`std::fs::copy` or a plain `cp` fail with "Permission denied" (seen 2026-10-05: a file another
program wrote, copied into the backups).

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

**Writers.** The build Mac's agent writes the build's records (the manifest, its unverified
uploads, the job keys): one agent per Mac (a lock), and build steps merge their manifest changes
under this Mac's lock. The M1's helper uploads its jobs' files (content-named) and hands its
record changes back through the build Mac's coordinator, which journals them for its agent to merge
(§8, Two Macs). The exceptions:
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
- It sweeps the folders catalogs index, and retired passes' sources (`sources/osm/<date>/` and
  `sources/items/<date>/` of passes older than the newest complete one): their content-named files
  by the same rule, their other files (the planet download, the pass's answers) 14 days after the
  newer pass completed, then the empty folders.
- Never swept: the newest pass, a planet waiting for its pass, the rest of `sources/` (registers,
  the basemap's data, the DEM seed, the rail sources with the files they replaced, today's legacy
  inputs), translations, descriptions, inputs, state, app and nas.

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
- **Two jobs at once at most:** the plan's first, and a second beside it when the two fit (§8, Two
  jobs at once). Each job is a child process group at utility priority (`taskpolicy -c utility`;
  `-b` would confine it to the efficiency cores, ~17× slower).
  - The first job's Rust steps take half the cores when the user is active as the job starts, and
    all of them when idle (`RAYON_NUM_THREADS`); the second's, four threads for a step that mostly
    waits on the network, else half the cores.
  - osmium, Planetiler and the Python steps take what they take.
- **Power:** CPU jobs run on mains power or on battery down to 30 % charge, then pause until the Mac
  is plugged in. Every job also needs the NAS.
- **Away from home** the agent mounts the share by the NAS's bare name, which Tailscale's DNS sends
  through the tunnel (~12 MB/s), when the Keychain has that name's password. Every job runs
  except those that move the whole planet or world through the NAS (the OSM pass, a pass's missing
  sets, the units' reach, the world's buildings), which wait for home. Home again, with no job
  running, it unmounts a tunnel mount and mounts the share by its LAN name.
- **Sleep:** each running job holds `caffeinate -i -s -w <pid>`: no idle sleep, on battery too, and no
  system sleep on mains power. It's dropped while the job is paused, so a paused Mac can sleep.
- **Downloads are kept on the NAS, each made once** (`sources/`): Meta's canopy squares (today's
  build's among them), AWS's raw terrain tiles, FABDEM's 1° tiles, the leaf-type sources, Overture's
  buildings, and Wikipedia's monthly pageviews (`sources/pageviews/<month>.tsv.zst`: a month's
  dump, ~5 GB, streamed once and every article of the map's languages counted, so the items and
  heritage jobs, any run, look up any article instead of streaming it again; an index says its
  languages, and one added later streams the month again). A Mac's copy is a cache filled from the
  NAS; one the NAS lacks (an upload that failed) is uploaded the next time it's looked up, and a
  damaged one is copied again, or the month streamed again.
  - **AWS's raw terrain tiles** are kept in the build Mac's cache as they come, then packed onto
    the NAS (`pipeline::rawpack`, `sources/aws-terrarium/packs/`, listed in its `index.json`) in
    archives grouped as the terrain's packs are: z9–12 by z6 tile, z3–8 by z3 tile, z0–2
    together, so a job copies the archives of what it makes and no more. A job's new tiles go up
    at its end (or by room-making, before it deletes them) as an archive of their own beside their
    area's others, one large write, where a tile at a time the NAS's small-file writes run at ~23
    a second and stall it; nothing there is written again for them. An area's archives are kept
    each more than twice the size of all those after it (from the first that isn't, they're merged
    into one), so an area has a dozen at most and a tile is rewritten a dozen times at most. On the
    NAS the build Mac's archives are named by the index or listed in its `gone`, with when: listed
    before it's put there (taken off once named, and named only once it's there whole) and when the
    index stops naming it, and deleted a day later, so a job that read the index before stays right
    and one cut short leaves nothing behind (its temporary file swept a day later too); one the
    index names that's gone from the NAS is passed over and taken out of it. Only the build Mac
    changes the index, under its build lock. A helper's jobs pack too (every loose tile in its
    cache, whichever job fetched it): they put their archives there unlisted and hand them off, and
    the build Mac names them as it merges the hand-off; one still unnamed a day later (its hand-off
    never came) is listed to go then. (Never while the index names nothing: an index read as none
    would list every archive.) A tile is read from its area's archives: an area's
    first 16 by range from the NAS's (a job that wants a tile or two of an area, as peaks do,
    copies nothing), then each copied to the Mac whole once, three at a time, and checked against
    its name (a terrain job reads thousands; an area whose copy fails is read by range for the
    rest of the job); an archive gone from the NAS when an area is opened has the index read
    again. The NAS's loose tiles from before (`<z>/<x>/<y>.png`, copied in bulk by
    `tools/nas/raw-tiles.sh`) are read while they're there, and packed by `tools/nas/raw-pack.sh`
    (the NAS's own tar over SSH, the tiles in their areas' order, so an area is packed once with a
    GB or two of the Mac's disk); none is fetched twice, only a lost disk before the packing would.
  - **The Wikidata and Wikipedia answers** the items job and the heritage chain got for a pass
    (`pipeline::answers`): the items' facts and articles (dem/items.py's caches), and the heritage
    scripts' (heritagewd.py's answers by register ID, its items' articles and English short
    descriptions, areadetails.py's park facts, heritage.py's labels, a register a script
    downloaded: the files of the pass's copy of the registers' snapshot that the snapshot lacks or
    has otherwise, but those the chain makes again each run). A step adds to them in its Mac's
    cache as it fetches, and keeps them on the NAS as one archive a pass
    (`sources/items/<date>/answers.tar.zst`, `heritage-<id>.tar.zst`), written whole as the step
    starts when the NAS hasn't what the Mac has, and as it ends, finished or not, when it fetched
    more: ~10 MB for the 2026-09-28 pass. A Mac without them, or whose are older than the NAS's
    (another Mac ran the step since), takes the NAS's; the first run seeds the NAS from the Mac's.
    A new pass asks again (the items' everything, the heritage chain what the snapshot lacks).
  - **Whole:** each copy is written by a temporary name (the Mac's and the process's), flushed, and
    its length checked before the rename (raw terrain tiles excepted: written straight to their
    names), and checked whole when read (`pipeline::whole`,
    `dem/whole.py`: a PNG to its last chunk, a TIFF's strips or tiles inside the file; FABDEM's
    copies read back and compared). One that isn't is deleted and taken from the next source: the
    NAS's copy, else the source itself. A canopy file that doesn't decode is taken again too.
  - **"Not there"** (a 404, or S3's 403) is remembered only once the source says so twice, a moment
    apart; FABDEM's `.none` only once its zip's own file list lacks the tile.
  - **What's fetched again** is new data (a planet, Wikidata facts and pageviews, timetables, an
    Overture release), or a window of a dataset read in small windows where nothing kept covers it
    yet: the national DEMs (USGS, HRDEM, MRDEM, GSI, the MOI DTM) at points not sampled before
    (each point's height is kept once sampled), ESA WorldCover for grid tiles the packs lack.
- **Caches** (`~/Library/Application Support/scenic/agent/cache`, on each Mac that runs an agent).
  Those a trim or a clear empties (§8, Room on the disk) are copies of what the NAS keeps, or made
  from it: each fills again from the NAS (or is made again from what's there) when a later job
  needs it, never from the internet.
  - **The cheap ones**, emptied when a job starts with too little free (the canopy files idle an
    hour first, then least recently used first), trimmed once the build is done, and cleared:
    - Meta's canopy 10° squares (`chm10/`), filled from `sources/canopy/` (one the NAS lacks is
      copied there before it goes); the build Mac's trim keeps them: every pass's areas read them
      again, and they never change;
    - AWS's raw terrain tiles (`aws-terrarium/`): as fetched, until packed onto the NAS (packed
      there before they go; a helper's stay until a job of its own packs them), and copies of its
      archives (`packs/`), filled from the NAS's archives;
    - copies of the records' files staging reads (`blobs/`), filled from the store.
  - **Cleared too** (by the owner's ask alone):
    - the pack cache (`base/`): base packs for pack(T) and lo not in its mirror, pruned every run
      and cleared when an OSM pass starts; the next round copies them again (66 GB on the build Mac,
      2026-10-06), from the mirror where it has them, else the NAS;
    - the per-vertex DEM cache's seed: today's cache, copied once from `sources/dem-cache/` (9 GB),
      cleared only while the NAS has it whole, and copied again by the next unit job. Each unit's
      samples from its last run are on the NAS (`cache/dem-units/`, which both Macs' units read,
      named by their box so a unit finds those near it from one listing, with the DEM rules'
      versions they were sampled under), so a vertex is sampled from the DEM servers once, and
      again only when the rule for its source changes (`pipeline::rules`). A tile a server doesn't
      answer for (a timeout, a 5xx) fails the job, to be tried again, rather than falling back to a
      coarser source for good;
    - local copies of the NAS's files the summits and peaks jobs read (the z8 terrain, the
      summits: `sources-*/`, `work-*/`), copied again when they next run;
    - the heritage jobs' clip of the pass's filtered planet to the coverage
      (`heritage-merged-<date>-<cover>.osm.pbf`, 22 GB on the build Mac, 2026-10-06), made again
      from the NAS's filtered planet (an hour or so of osmium) when the heritage chain next runs for
      that pass and coverage.
  - Nothing is deleted through a link, nor anything in the NAS's project folder (§8, Room on the
    disk).
  - **Kept:** the Wikidata and Wikipedia answers the items and heritage jobs keep (`items/`, with
    the pageview months' indexes, and the pass's copy of the registers' snapshot, which the heritage
    scripts add theirs to: `heritage-<date>-<id>/`), which the NAS keeps too (Downloads, above); the
    heritage scripts' Python environment (`heritage-venv/`, from PyPI); the registers' snapshot,
    extracted (`registers-<id>/`: the pass's copy is an APFS clone of it, so deleting it would free
    next to nothing); the trains' stop pairs (`rail/`, under a MB); the unit stages' timings
    (`unit-stages.json`); and what a unit kept that isn't on the NAS yet (`dem-units/`,
    `scenic-units/`).
- **Its own map** is served from its mirror, which keeps a 150 GB reserve so builds have room.
- **An OSM pass** starts with 80 GB free (the pack cache counting as free). It copies the planet to
  the SSD first when there's room for the planet, a filtered file of up to 75 % of it, and 10 GB;
  otherwise it reads the planet from the NAS.
- It may be asleep, away or unplugged at any time (§8, Interruptions).
- **What a unit keeps for its later runs** is on the NAS (`cache/`), shared by both Macs: its DEM
  samples and its canopy and view results (`scenic-units/`, §6 base(U)), or none when keeping them
  fails (only a later run's time is lost). What a Mac has in its own cache's `dem-units/` and
  `scenic-units/` is moved there by its next unit job. Its results read while the other Mac replaces
  them (both building the unit) aren't used.
- **A unit's build folders** go once it's built (a failed unit's, when the next unit job starts).

**The M1 helps (16 GB).** Its agent runs as a helper (`scenic agent --helper`, under the launcher
like the build Mac's; `tools/app/install.sh --helper` sets it up).
- **What it builds:** what the build Mac's coordinator gives it (§8, Two Macs): the shared steps'
  jobs that fit the memory it spares (6 GB of its 16: three eighths) and its disk, from the far end of the
  list (terrain from the near end), and when none does, units' tails (tasks, `docs/workers.md`);
  nothing while it runs another app than the build Mac's. While its owner is away (on mains power,
  not used for a quarter of an hour) it spares five eighths (10 GB) for a job that doesn't fit its
  usual memory, if the job's targets' last runs say it ends within twenty minutes (twice the build
  Mac's time where only the build Mac ran it; never one not yet measured), once no step's work fits
  its usual memory. A unit's predicted peak is the most memory one of its steps'
  programs took last time (each unit job notes it; scenic-build's own isn't counted), else about ten
  times its piece, never under 3.7 GB: over the M1's first 205 units, pieces up to 150 MB, 3.7 GB at
  most, no more for the bigger pieces.
- **How:** the build Mac's power rule (mains, or battery down to 30 %); half its cores while its user
  is at it, all but two otherwise; each job started with its step's room free (a terrain run 55 GB,
  tree cover 30, the others 15, a task 5), from the caches the NAS keeps; only work it can make that
  for is asked for, and a job it can't is given back.
- **Status:** `state/helpers/<host>.json`. The M1's status bar shows its job from its own status;
  the build Mac's shows it from that file while the build Mac's agent runs. Leases keep the two apart
  (§8, Two Macs).

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
    and the Keychain, at most every five minutes. Away it mounts nothing (the build Mac's agent
    may, through Tailscale: §4, Build Mac).
  - It mounts by the LAN name `fishandchips.local` when the Keychain has its password, else by the
    bare name. The bare name resolves to the NAS's Tailscale address, whose userspace networking held
    SMB to 12 MB/s.
- **`nsmb.conf`** makes the share a soft mount, under both names.
- **NAS access:** reads and writes (the Regions panel's recipes too) go through a bounded I/O pool
  with timeouts.
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
- **Place search** (`crates/server/src/places.rs`, `web/src/ui/search.ts`): the box at the top right,
  left of the view controls (`/` opens it; the viewshed's card goes under it where the map is too
  narrow for both side by side).
  - **What it finds:** the map's own place names (its labels layer: every label of the map's areas,
    and worldwide what shows by zoom 8: towns and cities, lakes, bays, parks, states), by any word,
    as typed, of the name the map shows (its translation's main and sub, §7) or of the place's own
    name and English. Accents, case and punctuation aside; inside a Chinese, Japanese or Korean
    name too (each ideograph, kana and hangul syllable starts a word), not inside a Thai one. Two
    letters at least, or one ideograph or kana.
  - **The order:** the whole name first, then a name starting so, then a word inside one; among
    like ones the more important, then the nearer to the view.
  - **The list:** each place as the map names it (its own name and English after it, muted, where
    they're other), what it is and how far it is; a finger's drag scrolls it. Return (on what's
    typed, searched first if the list isn't of it) or a click flies there, at the zoom that shows
    it, and marks it until the box is emptied or cleared (Esc).
  - **The index** is made on this Mac the first time it's searched after the labels or the
    translations change (the box, opened, asks for it; a new catalog whose labels and areas are the
    same keeps it), on three threads of its own, a label's copies at zooms 8 and 12 once (by its
    feature id). It's let go after half an hour unsearched, and its memory with it: its arrays are
    mapped for them alone, as macOS's allocator keeps the big blocks it frees.
  - **Its cost,** measured on the build Mac with a synthetic set the size of the live build's (4.4
    million places, no translations): made in 3–3.5 s, the server's memory some 500 MB more at
    most while it's made and 340 MB more while it's kept, back to within about 50 MB once it's let
    go; a two-letter search in about 5 ms, a word in about 1 ms.
  - **Failures:** packs and tiles it can't read are passed over (logged, and the index made again
    five minutes later if it's searched); a build that reads none fails, and the box says why and
    when it's tried again (a minute later at the soonest). Until the index is made, the open box
    asks again after a second, then two, four, up to ten; those asks aren't the map in use.
- **Offline start:** the last catalog and every pack's index stay local.
- **In use** means any request in the last ten minutes, except the status polls (`/api/catalog`,
  `/api/ping`, `/api/build`) and the place search's asks while its index is made (`poll=1`).
  - An idle server loads nothing; warming starts at the first request (starting isn't a use).
  - It checks the NAS for a newer catalog every 30 s while in use, every 10 minutes otherwise.
- **URLs and ETags:**
  - Data URLs carry a content version (`?v=`: a file's hash, a layer's packs' content names, plus
    the translations version for named data).
  - A versioned response is cached for good (`immutable`) while that version is current, else it's
    revalidated.
  - ETags are content hashes, combined for named tiles with the versions of the translations they
    use. The basemap's are its archives' content names and the tile's position, known from the
    catalog, so its 304s read nothing.

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

**Devices: an iPhone, an iPad.** Either Mac's map opens on them, at home or away, over the tailnet.
- **Who's answered:** the server listens on every IPv4 address (and IPv6's loopback) but answers
  only this Mac, its LAN and the tailnet (`pipeline::net::allowed`); anything else is refused (403).
- **A page elsewhere is never the map's,** in a browser on this Mac or on a device:
  - A request must name the map in its `Host`: an address, localhost or a name under it, or a name
    only a tailnet or a local network resolves (one label; `.local`, `.home`, `.lan`, `.internal`,
    `.ts.net`). A public name pointed at this Mac (DNS rebinding) is refused.
  - A request from a page (`Origin`) must come from the map's own page or from this Mac's.
  - Cross-origin reads (CORS) are allowed only to this Mac's own pages, whose downloads go to
    `roads.localhost` and the like.
  - So no site can read the map's data or change its regions through a browser that reaches it.
- **The key** (`crates/server/src/remote.rs`): a request that isn't this Mac's own needs the map's
  key (`<home>/remote-key`, made once, 0600), except for the app itself (its page, scripts,
  styles, manifest, service worker and icons). One handed over by a proxy on this Mac (`tailscale
  serve`: any of `X-Forwarded-For`, `-Host`, `-Proto`, `X-Real-IP`, `Forwarded` or
  `Tailscale-User-Login` set) is another device's.
  - The key comes once, in the map's address (`#k=<key>`: a fragment, never sent). The page gives it
    to `POST /api/auth`, which keeps it in an HttpOnly cookie (`scenic_k`), and drops it from the
    address. A device without the cookie (an app on the home screen has its own storage), or whose
    key is refused (a new `remote-key`, the only way to shut devices out), is asked for the address
    again.
  - The address is `<home>/map-page`, which the server rewrites when it changes: HTTPS where
    `tailscale serve` proxies the server's port at the root of an HTTPS port of its own (the app asks
    for `/api/…`, so not under a path: `tailscale serve --bg --https=8443 http://127.0.0.1:8080`),
    else the tailnet address. The status menu's Copy the Map's Address and `scenic status` give it.
  - Only a proxy that says it is one (`tailscale serve`'s HTTPS sets `X-Forwarded-For`) keeps a
    device's requests from passing for this Mac's own. A plain TCP forward on this Mac would let
    devices in without the key.
  - The map's data stays its owner's alone (the licences): without the key, only the app itself.
- **An app** (`web/public/manifest.webmanifest`, the icons): Share, then Add to Home Screen, full
  screen. Its service worker (`web/public/sw.js`) is registered only over HTTPS (so with `tailscale
  serve`), and never on this Mac's own address (localhost: the Macs have the data themselves). What
  it keeps for when the Mac is away:
  - the page, all of its scripts and styles (those it names and those they name: the map's
    workers) and the catalog's metadata, kept as it installs and again with each new page; scripts
    and styles a newer page no longer names go;
  - the fonts and icons, and the map's versioned data (`?v=`) that the Mac says never changes
    (`immutable`: its version still current), as it's looked at (the last 12,000 files kept).
  - The page and the metadata come from the Mac when it answers well within 4 s, else as kept. A
    Mac that doesn't (asleep, away, or restarting: `tailscale serve` answers 502) is taken for away
    for a minute, so what was kept is used at once. A refusal (401, 403) is never hidden.
- **Touch** (`web/src/trackpad.ts`, `web/src/ui/touch.ts`): one finger pans; two pinch to zoom, turn
  to rotate and drag up or down to tilt; a double tap zooms in; a long press offers Street View,
  Google Maps and OpenStreetMap there.
  - A tap's action waits out the double tap's 300 ms, so a double tap only zooms. Another finger
    down meanwhile cancels it.
  - Popups stay inside the screen (one too wide for either side of its point is moved inside, its
    tip hidden) and above the controls and lists.
- **A phone's layout** (a narrow or a short window, either way up): the settings fold into a panel
  that the button at the top left opens. The lists become a sheet along the bottom (on its side, a
  card at the right): its handle drags its height, and a tab tapped again folds it to its tabs. The
  bar sits clear of the home indicator. The camera's step buttons go, but the compass and the tilt still reset.
  The place search sits between the settings button and the controls, the viewshed's card and the
  toasts under it.
  - On a tablet the panel stays docked, folded by its ‹ (remembered).
  - Touch screens get bigger controls and the touch hints.

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
    coverage), the terrain packs, the tree cover per z3 tile, the landmark candidates and the
    heritage-sites job.
- **Shrinking:** what only the removed part built leaves the manifest once the units are built (a
  prune): the outputs of units no longer built, their candidates and peaks, and map tiles no unit's
  ways reach any more; pack and lo also drop what a tile no longer has (a tile without ways, a
  layer without tiles). The next catalog drops them, and GC frees their files. Terrain, slope and
  grid tiles stay, which is harmless; a z3 tile the coverage has left loses its tree cover.

**Today's set** (since 2026-10-05): 88 recipes in `inputs/regions/`, by political unit, every one
of them OpenStreetMap boundaries (`osm:` relations from the pass's outline set). The cutover's 34
are in `tools/cutover/regions`.
- **Canada:** its 13 provinces and territories.
- **The US:** every state, DC and Puerto Rico (the other territories later).
- **The UK and Ireland:** England, Scotland, Wales and Northern Ireland, and Ireland (which replaced
  Geofabrik's Britain and Ireland); Guernsey (the bailiwick: Alderney, Sark and Herm too), Jersey,
  the Isle of Man and Gibraltar.
- **Spain:** its autonomous communities but the Canaries, with Ceuta and Melilla (18 relations), and
  **the Canary Islands**.
- **Portugal:** the mainland (its 18 districts: no relation is the mainland alone), the Azores and
  Madeira.
- **France:** metropolitan France (the mainland and Corsica: one relation), French Guiana and
  Saint-Pierre-et-Miquelon (the other overseas parts later). **Andorra** and **Monaco**.
- **Japan** (the Senkakus are in no outline, so they dropped out), **Taiwan** (Kinmen, Matsu,
  Penghu, and Pratas and Taiping too, inside its relation), **Hong Kong** and **Singapore**.
- The first change (the US, the UK's countries, Portugal's three) built 81 new units and rebuilt 47.
  The second, every outline from Geofabrik's (generous buffers into the sea and across borders) to
  OSM's boundaries, and the Channel Islands split, changed the keys of 276 of the 284 units, and the
  terrain of 16 of the 18 areas: nearly every unit has a border or a coast in its reach. The regions
  on the map stay as they were until theirs are rebuilt, after the regions the map lacks.

**By location.** These rules depend on where a thing is. Today each is written into its step, and the
units' ones (DEM order, densification, road network codes) are versioned by area in
`pipeline::rules`: a unit's key names the versions of the rules where its ways go, so a changed
rule (its version bumped) reruns only the units it applies to. The plan is modules declared per ISO
3166-1 country or 3166-2 subdivision, with defaults:
- DEM order (`pipeline::dem`, the `elev` program; Taiwan's MOI DTM from `inputs/moi-dtm/` when it's
  there, which reruns Taiwan's units) and densification spacing (8 m in North America and Japan, 15 m
  elsewhere: `extract`);
- heritage registers (the snapshot, `dem/heritage.py`);
- timetables:
  - rail: the Mobility Database catalogue's feeds of the countries the coverage is in (worked out
    from the pass's outlines: `pipeline::rail::countries`), and national operators' own, each
    with its country, in `dem/railfeeds.py` (§6, Rail service);
  - ferries: `inputs/ferries/freq`;
- road network codes (`extract`) and their colours (`web/src/mapschemes.ts`);
- leaf-type source: EEA in Europe, NALCMS in North America, none elsewhere (`dem/leaftype.py`,
  made by the trees job where a square is missing, not tagged complete, or not whole);
- the languages spoken there, for names (§7; today `names::area`'s boxes);
- credits (`pipeline::rules::CREDITS`, each with the areas whose data comes from its source; a
  catalog lists those meeting its coverage, 20 km around it, or its units' ways).

Planned for a country without a module: defaults (FABDEM, no register, colours by road class), with
`scenic status` saying which defaults each region uses.

**No seams:**
- **Region borders:** no step knows about them.
- **Tile edges:**
  - per-vertex values are computed once, by the owning unit;
  - road values come from the planet;
  - climbs come from the 100 km halo;
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

1. **The NAS fetches the planet.** `nas/fetch-planet.sh` runs on the NAS; its source is
   `tools/nas/fetch-planet.sh`, which `publish.sh` copies there.
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
   - The 2026-09-28 pass's (a) was filtered without `w/route=ferry`: it has a ferry way only where
     another of the filter's tags kept it (a route relation's member, a `wikidata` tag, …). Its
     ferries set is the planet's own, with every ferry way, and `scenic-build patch-ferries` (by
     hand) gives each piece the set's ferry ways it meets, as the cut keeps ways, and lacks. Only
     the pieces that lack some change, so only their units, their landmark candidates and the
     reaches are built again. It saves the records once, at the end: a changed piece makes the
     reaches stale, and units aren't planned on either Mac until the build Mac remakes them. Every
     piece has them: those meeting the regions (their tile and 10 km round, `--near-coverage`)
     gained 3,563 ferry ways in 84 pieces, then a run over all of them, while the units built, 21,452
     in 567 more (the first 84 lacking none). So does a ferry that reaches the regions from a
     farther tile, which owns it.
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
  - **Source:** always AWS's raw tiles, each downloaded once (64 at a time) into the build Mac's
    cache and packed onto the NAS (`sources/aws-terrarium/packs/`: §3 Downloads), which fills the
    cache when it lacks one; repaired by `repair_terrain`.
    Processing a processed tile isn't idempotent, so stored tiles are never inputs.
  - **Below zero:** values are clamped to 0. Planned: a sea mask from the pass's water polygons, so
    that polders and depressions keep their depth.
  - Deterministic: reruns give identical packs.
- **Slope:** z11 and coarser are stored, from transient z12 Horn slope. The server makes z12 on
  demand with the same encoder and an LRU (56 % of the full archive).
- **Worldwide z8 terrain** (`sources/terrain-z8-v1`, once, not served): every z8 tile, repaired, with
  each tile's maximum. Peaks read it, so their prominence and isolation don't depend on coverage.
- **Grids (z11):** land cover, canopy and cover, for analysis only (not served).
  - Each unit's job makes the grid tiles its packs lack: `landcover --only`, and the scenic canopy
    step.
  - It uploads them as its own z6 tile's `grid-*` hi packs. They aren't in the units' keys: a grid
    read from its pack or made afresh is the same (from fixed datasets: WorldCover, Meta's canopy
    squares), and a unit writing its tile's would otherwise make it and its neighbours stale.
- **Trees** (cover, height, leaf type), zoom 4–12, per z3 tile the coverage meets, clipped to it
  (`pipeline::treepacks`, which runs the `trees` program, `pipeline::trees`), made for a region
  before it's published (§8, Order: once it's done, or earlier while the units wait for the pass's
  worldwide jobs; a helper takes its jobs too): from Meta's canopy squares (kept on the NAS,
  `sources/canopy/`, and copied into the agent's cache, where the units read them too; a square
  both want is downloaded once, under a `<file>.lock` in the store, the other waiting for it)
  and the leaf-type squares on the NAS (`sources/trees/leaf/`), each made whole once by
  `dem/leaftype.py` (the EEA's every chunk, a chunk without EEA data costing one small request;
  NALCMS's GeoTIFF kept beside them) and tagged complete. A z3 tile's run makes all its packs and
  drops those it no longer has (`TREES_V`); until its first run, today's converted packs serve.
  - The program is `dem/trees.py --z3` in Rust: the same tiles, to the pixel, in another lossless
    WebP encoder's bytes (`pipeline::webp`: about 1 % smaller than libwebp's on real tiles).
    `tools/check/trees-same.py` compares the two (2026-10-05: ten blocks of every kind, and the
    whole of 3/4/2 and 3/7/3, every tile the same); `SCENIC_TREES_PY=1` has the job run trees.py.
  - A block runs on its own too (`trees --block`: a zoom-8 block's tiles and its zoom-8 values,
    the same bytes natively, on any thread count, and in WebAssembly, its squares read from a
    folder or through `pipeline::fetch`), and `trees --assemble` makes a z3 tile's archives from
    blocks: tree cover as tasks (docs/workers.md) is planned.
- **Area overlays:** see `docs/phase5.md`. The `overlays` job runs after marks, because it needs the
  World Heritage dots' ids. Until its first run, today's converted packs serve.
- **3D buildings (phase 7, planned):** `docs/buildings3d.md`. Every building in the coverage, from
  the Overture release the roadside buildings read: its height measured or from its floors, else
  estimated from its neighbours, GHSL or its size and kind; tiles z12–14 per z6 tile. National
  heights (PLATEAU, BD TOPO) later.

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
2. **Elevations:** `elev` (`pipeline::dem`: `dem/sample.py`'s port, the same bytes but where a
   source needs a projection, within 3.1e-5 m), DEMs by location, on U's slice of the per-vertex DEM
   cache (the seed, and the units' kept samples, which win); FABDEM's tiles from the NAS (`sources/fabdem/`, each
   copied there from Bristol's zips once). U's samples are kept afterwards for its later runs and
   its neighbours'.
3. **Heritage:** the sites and designated areas of the heritage-sites job's slices within U + 30 km.
   `areaflags` rasterises the areas onto U's grid.
4. **Terrain and grids:** terrain z11 and the grids, staged from the packs (as the build manifest has
   them when the unit runs, which is what its key names). Missing grid tiles are made.
5. **`tile elev`:** clean-up and grade, with junction context from the piece.
6. **scenic:** `scenic-metrics` prep, canopy, view, buildings and flags, for every sample of the
   ways U owns (the others are context, and their results would be thrown away). The buildings come
   from the release's z8 tiles within 1 km of U's tile + 20 km and of its own long roads. U's canopy
   and view results are kept in the NAS's `cache/scenic-units/` after each run, shared by both Macs
   (`scache::Carry`: the samples'
   keys and results, the canopy and cover grids, and a hash of each grid tile's terrain and land
   cover); its next run starts from them, so only samples that are new, or near grid tiles whose
   terrain or land cover changed since (within the far field's 15 km for views), are done again.
   After a change of `scache::SCENIC_V` every sample is.
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

### Roadside buildings: the world, once per release

The buildings job (`pipeline::buildtiles`) makes Overture's building boxes for the whole world, from
one pinned release (2026-09-23.1), before any unit runs:
- `dem/buildings.py --world` reads the release's bbox columns (~32 GB of its 277 GB), 24 files at a
  time (a file alone runs at 0.1–0.2 M buildings a second, waiting on S3), into local parts by z8
  tile; each file of the release is marked done once written, so a run cut short goes on from
  there.
- Each tile's parts are merged onto the NAS (sorted, boxes that are bit for bit the same once, each
  file read back), then the index. A run cut short leaves the tiles already there as they are; a
  listing with no files, or a file of the release not scanned, fails the job.
- A unit's key names the index, so a new release rebuilds every unit, and nothing else does.

### Per z6 pack: pack(T)

- **Reads:** the base packs and road values of every unit whose ways come within 100 km of T (its
  owned extent, from the pass's reach: its tile, the box of its other ways and its own long ways,
  such as ferries; without a reach, its tile + 20 km).
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
- **Rail service:** trains a day on the coverage's rail ways (below).
- **Landmarks:** candidates and peaks per unit, then Wikidata facts and pageviews, marks, and
  overlays (`docs/phase5.md`).
- **Planned:**
  - names todo (§7);
  - descriptions todo (§7).

### Rail service

Trains a day on each rail way of the coverage (`global/railfreq`), from operators' published
timetables (GTFS) and the MTR's hand-researched lines (`pipeline::rail`). Two jobs, after the units,
in a chain of their own (§8):
- **`rail-feeds`** finds the feeds where the coverage is (`dem/railfeeds.py`):
  - The Mobility Database catalogue's active GTFS feeds of the countries the coverage is in, whose
    box meets it and whose download needs no key.
    - A country counts when it holds 5 % of one of the coverage's outlines (each point of the
      outline given to the smallest territory holding it, so Hong Kong's are Hong Kong's, not
      China's), or the coverage holds half of it (Monaco, Man).
    - So an outline's margin past a border doesn't bring in the neighbour (1 % of Ontario's outline
      is the US), but a small outline's wide margin may (Taiwan's is 11 % China, around Kinmen and
      Matsu).
    - A territory with an ISO 3166-2 code brings its country too (Hong Kong and Macau bring China,
      under which the catalogue files Hong Kong's feeds); the feeds' boxes keep that to those near.
      Its own code is its code's end, or for the few whose isn't, looked up (`rail::OWN_CODE`:
      Åland's FI-01 is AX, Guadeloupe's FR-971 GP, Kosovo's RS-KM XK, not Comoros' KM).
      Where the catalogue files a feed under another country than its trains', `dem/railfeeds.py`
      corrects it (two of Singapore's, under Malaysia).
  - Of those, the ones that run rail: each checked once, by reading its `routes.txt` out of the zip
    with range requests. An answer counts only whole (the bytes the zip says, with its CRC); one cut
    short or failing is no answer, and the feed is checked again.
  - National operators the catalogue lacks (SNCF, Renfe, Great Britain's timetable as GTFS by
    Catenary Transit, Hong Kong's trams), and Singapore's LTA feed when `inputs/keys.env` holds its
    key, each where its country and box are.
  - Left out: a feed another replaces (an operator's own over a copy: LTA's over the catalogue's
    three copies of Singapore's timetable, of which one is read without it; Catenary Transit's Hong
    Kong trams over the Transport Department's) while that one has a zip, and a community feed of
    the MTR's lines, which come from the hand-researched pairs.
  - Each feed's zip is fetched once, into `sources/rail/gtfs/`. A zip already there is never fetched
    again, except one that was already out of date when fetched (no rail service in its window):
    rail-feeds runs only when its key changes (Job keys, below), and its next run at least a week
    after the day the zip counts from fetches it again. A new file replaces it; the same file is
    kept once and counts from that day, so it isn't fetched again for another week; a refusal or no
    answer leaves the old copy.
  - A request without an answer (no connection, a 429 or a 5xx, an answer cut short) is tried
    three times. A feed still without one fails the job (what it checked and fetched is kept), which
    the agent tries again (waiting up to 6 hours); after 3 days without an answer it's left out
    instead, its status naming the first and the last day, so `rail` runs without it, and it's tried
    again the next time rail-feeds runs. A definite refusal (a 404, a file that isn't a zip) leaves
    that feed out.
- **`rail`** counts the trains and matches them onto the tracks:
  - each feed's trains on its typical weekday (`dem/railgtfs.py`): the median-busy Tuesday to
    Thursday from 30 days before the day its zip was fetched to 90 days after, so a zip's counts
    don't depend on the day they're made. A train in two feeds is counted once (the national
    operators' first). The stop pairs don't depend on the coverage either: the build Mac keeps them
    for the list of feeds;
  - the MTR's lines, as stop pairs (from the research, `mtr.json`, by `dem/mtrpairs.py`);
  - each pair's stops beyond the coverage marked (`railfreq` runs a cross-border service as far as
    the track goes toward the stop), and pairs with both beyond left out;
  - `railfreq` matches the pairs onto the rail ways of the pass's rail set that touch the coverage:
    the set clipped to the tiles within 20 km of it, `extract` at 8 m (as the units), then the ways
    touching it.
- **The rail sources** (`sources/rail/`, docs/formats.md) start from today's build's
  (`scenic-build rail-seed`, run once by hand): its 131 zips, the catalogue with the 1,545 feeds
  checked for it (the 1,420 found without rail routes, or without routes.txt, seeded as unanswered,
  so rail-feeds asks again once: today's check could take an answer cut short for none), and the
  MTR's lines. The seeded zips count from the day today's figures were
  counted (2026-09-30), so the job gives today's figures again; two were already out of date then
  (Chiltern Railways', Madrid's Cercanías'), and are fetched again as above. rail-seed writes the
  catalogue last, once the rest is saved, so a seeding cut short holds the chain until it's run
  again, adding only what's missing. It never replaces a file: MTR pairs made again
  (`make data/rail/pairs-mtr.bin`) go in with `scenic-build put sources/rail/mtr-pairs bin
  data/rail/pairs-mtr.bin --root <NAS project folder>` (and `mtr.json` as `sources/rail/mtr`),
  which the rail job's key follows.
- **Planned:**
  - Japan's ODPT and Taiwan's TDX feeds, behind the keys `inputs/keys.env` names for them
    (`ODPT_KEY`; `TDX_CLIENT_ID`, `TDX_CLIENT_SECRET`), each a keyed feed in `dem/railfeeds.py`
    once there are values to fetch with. ODPT's licence allows no redistribution of its raw feeds;
    TDX asks for a credit line (`pipeline::rules::CREDITS`);
  - the catalogue and the timetables fetched again every ~6 months (§8).

### Job keys

A job's key is its step version plus what it reads, mostly by content name. The ones that cascade:
- **terrain (per z3 pack):** the z6 tiles to build, and the coverage inside its z3 tile + 20 km;
- **slope:** its terrain pack;
- **heritage-sites:** the pass, its areas set, the registers snapshot, the coverage;
- **unit:** its piece, the pass's road values, the coverage as its ways meet it (inside its tile +
  20 km, and whether each long way touches it), the versions of the location rules where its ways
  go, the terrain and grid hi packs within 30 km, its heritage slices, the roadside buildings'
  index, and Taiwan's MOI DTM files where its ways meet Taiwan;
- **pack(T):** the base packs and road values it reads (above), those within its 100 km halo; and,
  after a dot, those of its owners alone (the units whose owned extent meets the tile itself: its
  ways-here index points into their base packs), so a round tells a tile whose owners changed from
  one whose halo did (a key from before has no owners' part: a tile current by its halo stays, one
  stale is taken as its owners changed);
- **lo:** the base packs and road values of the units whose owned extent meets its z3 tile (lo has
  its own version: a change in the tiling it shares with pack bumps both);
- **rail-feeds:** what decides which feeds there are: the catalogue, the coverage, the pass's
  outlines (the countries it's in), and which of the keys its keyed feeds use
  (`pipeline::rail::FEED_KEYS`) `inputs/keys.env` holds, by name (never their values). Not what it
  writes (the feeds' list, its checks and zips), so it doesn't run again for its own sake; it waits
  while `inputs/keys.env` can't be read;
- **rail:** the feeds' list (each feed's zip by content name, and the day it counts from), the MTR's
  pairs, the pass's rail set and the coverage.

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
| `/api/catalog` | the catalog's number, layers and zoom ranges, versions, its credits and the regions it was built for, the NAS and build state |
| `/api/coverage` | the catalog's coverage: each region's outlines, simplified, with the regions (built from the recipes for a catalog made before it was recorded) |

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
  Roads' own English comes with each unit (`global/roaden/<u>`, OSM's `name:en`), laid over today's
  converted table (`global/legacy/road-en`) until that goes with the cutover's converted data: until
  then a road whose `name:en` was removed since keeps today's. No job reads Wikidata's English labels
  or the language tags yet.

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
are no request files. After an edit (a recipe's outline, or an outline file in `inputs/outlines/`
that a recipe names) the regions' work waits a quarter of an hour for more edits before it starts,
an hour at most from the first of a run of them (what runs carries on; a new pass's outlines aren't
an edit, nor any other file there, nor a recipe that can't be read now): three edits in a row on
2026-10-05 built the same regions' heritage sites three times and their terrain twice.

**Scheduler.**
- **The plan:** every step's targets come with their keys (`state/build/jobs.json`). A target is
  stale when its key changed.
- **A job** is one step over a batch of stale targets: terrain and trees 1, slope and lo 2, unit 6,
  peaks 12, pack 16, pois 24, the worldwide steps all. So a failure or a new app costs one batch.
- **Order:** the agent starts the first job that can run, in plan order. It plans when a job could
  start (its second slot's: each minute), when one ends, and otherwise every five minutes for the
  heartbeat (planning reads the manifest, the keys and a dozen NAS folders); other workers' hand-offs
  are merged each loop while it waits, every two minutes while a job runs.
- **Two jobs at once** (`agent::SECOND`): beside the first job, the build Mac runs a second, the
  plan's first job of these steps, in this order: the trains' and the landmarks' steps that mostly
  wait on the internet (the heritage chain, the items' facts, the rail feeds and trains a day, the
  landmark points and overlays), then the candidates and peaks, then units and slope. A unit spent
  380 of its 860 s writing to the NAS and reading the caches (6/17/25, 2026-10-05): two at once build
  more. A second job:
  - never runs beside a job that runs alone (the OSM pass, the pass's worldwide jobs, GC), nor
    beside a job of the same step unless it's a shared one (its targets are held apart, as a
    helper's are), nor a reader of the raw terrain tiles beside another (terrain, peaks, the roots),
    nor the items' facts beside the heritage chain (both ask Wikidata, each paced as if alone);
  - while the Mac is in use, only work that mostly waits on the network;
  - only when the two fit: the first job's memory as predicted (or as it is now, if more) and the
    second's within three quarters of the Mac's, and the second's free now with 2 GB to spare;
  - starts only with its need free (10 GB for the network steps, the reserve for the others): room
    on the disk is made only while no other job runs (a job beside may read what's deleted, and the
    loop that looks after it waits meanwhile);
  - the network work is the second's: the first job leaves it to it while there's other work for
    the first (an hour of it would hold the first slot while the regions' terrain and units wait);
  - doesn't starve the first: when the first's next job can't start beside the second's (it runs
    alone, it needs room made, or the two wouldn't fit the memory), the first waits for it rather
    than start later work, and the second starts nothing new meanwhile, nor while one that runs
    alone or needs room made is the first's next;
  - has its own scratch folder (`scratch-2/`), job record, safe-point channel and costs file, its
    claims its own, and four threads for network work, half the cores for the rest;
  - is a worker of its own in the history and the forecast ("<host> (second job)"), its speed
    measured as a helper's is (four fifths of the build Mac's until it is); the forecast takes a
    Mac in use now to stay so for half an hour, not to the end (made every minute, its finish
    otherwise jumped each time the owner came or went). The status has it as `beside`, or why
    there's none (`beside_why`).
- **A newly installed app:** the first job finishes under the old one, nothing new starts, and the
  agent exits so the launcher starts the new one; a second job still running stops then (what it
  finished kept) and goes on under the new one.
- **Pausing** (`pipeline::control`): one pause for the whole build, every Mac's jobs and the worker
  pages' tasks.
  - **Asked for** from either Mac's menu bar item (Pause Building; Option: Pause Building Now), the
    map's build panel (Pause building, Pause now) or `scenic pause [--now]`, and lifted the same
    ways (Resume, `scenic resume`): an ask in that Mac's agent's folder (`pause-request.json`), which
    its agent takes up within seconds and passes on to the build Mac's coordinator, with when it was
    asked: an ask older than the build's last change (a helper's, held while it couldn't reach the
    build Mac) is passed over. A helper that can't reach it holds the ask itself meanwhile, and
    passes it on once it can. The opposite ask is always there (a menu never sticks on "Pausing…").
  - **Held** by the build Mac's coordinator, on its disk (`coord/pause.json`, `{pause, at}`, so a
    restart keeps it; one the agent knew from before its coordinator was up is given to it) and
    mirrored to the NAS (`state/build/pause.json`); told to every worker in its answers to their
    asks (an agent: refused, with the pause; a page: nothing now) and beats; each agent keeps what it
    last heard (`pause.json`), so a helper that can't reach the build Mac stays as it last heard.
  - **At a safe point** (the default): each running job finishes the target it's on (an area, a map
    tile, a terrain or slope area, a batch's candidates or peaks), saves it, notes it done
    (`SCENIC_DONE`) and ends as paused (exit 75; its channel, `SCENIC_CONTROL`, said "drain"). The
    agent records what it noted done (a helper hands those off: the coordinator takes part of a
    lease's targets) and starts nothing new; on resume the plan picks up the rest. A job that hasn't
    reached a safe point in 15 minutes (a terrain area mid-way) is frozen where it is instead, and
    goes on from there. **Now:** every running job frozen where it is at once (SIGSTOP), going on
    from there.
  - **While paused:** no lease lapses (a paused or asleep worker's work isn't given to another);
    the agents and the coordinator keep running and reporting (the heartbeat says paused, and each
    job stopping or frozen); a job stopped by the pause, or by sleep, a restart or not starting, is
    given back, not counted as a failure.
  - **By itself:** a job without the NAS (or a whole-planet job away from home) is frozen at once,
    as it can't save; on battery under 30 %, a CPU job stops at its next safe point. Each goes on once
    its condition holds again.
  - **Any job's end** (done, paused, failed, stopped) records the targets it noted done, so they're
    never built again.
- **Room on the disk:** before a job starts (and before its targets are claimed), when the Mac has
  less free than the job needs (30 GB; a terrain run 55 GB, for its area's raw tiles held twice
  while they're packed onto the NAS, and on a run again the area's archives copied here and merged,
  those copies spared; the OSM pass, its own 80 GB less the pack cache it clears; the M1's helper,
  15 GB, but a terrain run's and tree cover's as here), the local copies of what the NAS keeps (Meta's canopy squares, AWS's raw
  terrain tiles, and the copies of the records' files staging reads: `blobs/`) lose files until it
  has a sixth more (the OSM pass: what it needs), so the next jobs start without deleting again.
  - Canopy squares and copies not read in the last hour go first, each by its own use, the least
    recently used first: one listing of the NAS's canopy folder answers for every square (hundreds
    of MB a file), and a copy of a recorded file needs no listing at all (the records name only
    files the NAS has), while each raw tile folder takes its own for ~14 MB, seconds each when the
    NAS is busy.
    Then raw tiles a folder at a time (the least recently used folder, by its newest tile, first,
    so a folder's tiles go together) and the squares read since, together, least recently used
    first: the squares of the area being built, which the next jobs read again, outlast idle
    tiles.
  - A file goes once the NAS has it at the same size (each NAS folder listed once, sixteen at a
    time; a file the listing lacks asked about once more; a folder whose listing fails or is cut
    short, as a busy NAS's are, keeps its raw tiles that run and has each canopy square asked
    about alone). A canopy square the NAS lacks, or has at another size, is copied there first
    (whole and flushed), or kept; raw tiles it lacks are packed onto it first (an archive an area,
    none kept here), or kept (a tile at a time with a flush each, small files stall the NAS and
    every process waiting on it). The copies of its archives go each by its own use (a job marks
    one used when it opens it). A file that isn't whole itself (cut short, or temporary) is
    deleted, not kept.
  - The OSM pass counts those copies as room.
  - **After the build** (`room::trim`): once the build has nothing left to build and no job runs
    on the Mac (nor one an earlier agent left that couldn't be shown stopped), its agent, at home
    (through Tailscale it would take hours), empties those copies by the same rules, once, and
    again only after a job (not a daily one) has run there since: a helper all of them, the build
    Mac all but the canopy squares, which every pass's areas read again and which never change.
    "Nothing left to build" is the build Mac's forecast's (no work, no round under way, no machine
    busy), made within ten minutes and after the last job on the Mac ended; a helper reads it in
    the build Mac's heartbeat, which must have beaten within ten minutes and show no job of the
    build Mac's running, nor one beside it. Not down to the reserve: room-making makes that much
    room before each job, so the build ends with about that free (36 GB, with 39 GB of archive
    copies and copies of the records' files, once it was done on 2026-10-06), and a trim to it
    would free little or nothing, while that Mac's mirror copies nothing until 150 GB are free.
    It's logged, in the agent's status (`caches.trimmed`: when, what it freed by cache, what
    stayed) and, when it freed anything or what it keeps changed, in the history (a helper's, as
    the build Mac reads it in its status).
  - **On the owner's ask** (`room::clear`): Clear the Build's Caches in a Mac's menu bar item, or
    `scenic clean` there, after a confirmation that names what goes, cache by cache, and how each
    comes back (copied from the NAS at the measured 60 MB/s, 12 through Tailscale: the base packs
    in about 20 min; the heritage clip, an hour of osmium), writes an ask in that Mac's agent's
    folder (`clear-request.json`, as a pause is asked for, signed with the Mac's name as its
    agent goes by), which the agent takes up within seconds, renaming it aside as it does (an ask
    written meanwhile waits its turn): between jobs, once the build is done, it empties every
    cache a later job fills again from the NAS or makes again from it (§4, Caches), the canopy
    squares too, by the same rules, and says what it freed (`caches.cleared`); else it says why
    not (`caches.declined`, the last clear done kept apart); the ask goes either way. The menu
    shows the item with what it would free (`caches.clearable`, and cache by cache,
    `caches.each`), disabled with why while the build has work, a job runs there, its agent
    hasn't written its status for six minutes, or they hold nothing; "Clearing…" while the ask
    waits or is under way; and "Freed N GB" once it's done. `scenic clean` waits for the same
    answer.
  - A trim or a clear runs on a thread of the agent's own: its loop goes on beating, and no job
    starts on the Mac until it's done.
  - Nothing goes through a link: a folder or file of the caches that's a link, at any depth (the
    raw tiles' packer passes them over too), or a folder in the NAS's project folder by its real
    path, is left as it is (and not counted), so the NAS's own files never go.
  - Room-making, a trim and a clear each end early when the agent is asked to stop (the raw tiles'
    packing between tiles); an agent that exits waits a minute at most for a trim or a clear under
    way (a hung NAS call may not return), since nothing's lost left mid-way. The trim runs again
    under the next agent, and the ask stays for it.
- **Units run in map order** (by 10° square, then tile), so what one unit fetches serves the next.
- **Retries:** a failed job is retried after 10 minutes, doubling to 6 hours. The orphans of a crashed
  agent are stopped at start (only when their leader's start time proves them ours, or the leader is
  gone and every member started after the job).
- **The heartbeat:** the agent writes it locally with each loop (about every 20 s), and to the NAS
  (`state/status.json`) when it changes or every two minutes; the user's idle seconds don't count as
  a change, only whether they're at the Mac. It holds the job, its progress (from the job's `progress:` lines) with the time left, and a checklist
  of every step to the end, each saying what it does ("Choosing and drawing the landmarks"): its
  jobs left by name, in the order they'll run ("Measuring the peaks' prominence and isolation: 178
  areas", then the next), and for one with work left that isn't this
  Mac's job now, why: another Mac is on it, it waits for the home network or out a failure, or (the
  publishing) for the steps above, since a catalog follows each chain as it ends. A job of
  several parts says them as each begins (`parts: <i> [names]` in its log), and the status lists
  them under the job, done, under way and to come, the progress bar under the one under way
  (heritage's five: getting ready, details, outlines, fame, layers; the items' facts, articles and
  pageviews; summits, marks, route ends, the heritage sites, rail feeds, rail, labels; the OSM
  pass's ten, from the planet's copy to the roads' walk; a terrain run's three: its area's tiles
  fetched, shaded and written a z6 tile at a time, its zoomed-out terrain written to the NAS, the
  new raw tiles packed onto the NAS; slope's two an area, worked out (each z6 tile's pack written as
  it's done) then its zoomed-out pack written; the tree cover's two an area, worked out then written; the peaks' and the z8 terrain's, then
  their raw tiles packed; the map tiles' base packs got here, then the tiles drawn). A long part says
  how far it is: the pageview dumps by the bytes streamed, a part's steps one by one, a terrain run's
  tiles (every level's, counted first, each half done once it's here, from AWS or the NAS, and done
  once shaded), packs, then raw tiles packed and areas merged; slope by the tiles it works out; the
  pass's copies and uploads by their megabytes (an upload's reads, copy and read back all counted),
  its sets one by one, Planetiler by its phases. osmium says how far it is (`--progress`), as the
  step under way's share; a job's progress may count the item under way by how much of it is done
  (`progress: 2.4/6 areas`). A unit job's area counts by the stages it's through (its programs'
  and in-process parts', each weighted by about how long it takes on that Mac, learned as areas
  finish and kept with the caches: `unit-stages.json`), its elevations by the vertices that have a
  height, each area whose last steps are out with another worker by the stages before them; a
  map tile by its ways read, its tiles drawn, then written; a helper's task by its files fetched,
  its steps, what they wrote sent back. A part shows only what it has said itself (none at first),
  its time left from its own pace in its unit (a word in brackets after it may change: `steps
  (clipping…)` and `steps (filtering…)` are both steps), and when its progress last moved on (one
  stuck shows as such); what it said holds while its output since pushes the line out of the log's
  last 64 KB. The jobs' Python steps print straight to their logs (unbuffered); the unit's
  programs' progress lines reach the job's through its own (each program's errors are read as they
  come, into its unit's log).

- **The heartbeat's resources:** each Mac's memory and the share free, its cores and load, its disk's
  free space and the caches it may drop (counted every ten minutes on a thread of its own), how
  long the NAS took to answer and the NAS's free space. Beside them, its build caches (`caches`):
  what a clear would free (counted with them, and again after a trim or a clear), why they can't
  be cleared now, and the last trim and clear (Room on the disk).
- **The forecast** (`agent::forecast`), made with each plan (at most each minute) and in the
  heartbeat: the work left run through in the order the agent runs it. The build Mac takes the first
  it can (the pass's worldwide jobs, then a region at a time: its terrain, then its units; with
  none it can do now, the chains' work), its second job the first of its steps that fits beside it,
  each helper the far end of the first shared step with work it can do that fits its memory (terrain's
  near end): terrain at once, a unit once its region's terrain is built and the pass's heritage sites, reaches and
  roadside buildings are made (as the plan's units wait for), slope once its area's terrain is. A
  machine with nothing it can do waits for the next work to end or another machine to be free. The
  round under way goes first, with its own regions (their slope and tree cover left, then its chain
  as its steps take). A round goes out as the plan makes one: a region done that the map hasn't as
  it is now, an hour after the last round began (its slope and tree cover, which the build Mac makes
  while it waits, then the round's chain, as long as the last rounds took); its catalog carries the
  regions on the map that are rebuilt (a new pass) and done by then, their slope and tree cover too
  (they make no round of their own); after the last unit and terrain area, the
  slope and tree cover left, the last round (the roads' chain as it stands, less what the round
  under way still does, if longer; none when nothing's stale and no region waits to go out), then
  the overlays and a catalog. The trains' and
  the landmarks' chains run from the start, each step once what it reads is built (the candidates
  once the pass's hiking-route ends are made, the peaks once every candidate and the terrain are,
  the items' facts once every candidate is, the heritage chain once the heritage sites are, the
  landmark points once those four are, trains a day once their feeds are). Each
  target takes its last run's time at the build Mac's pace (one measured on a helper, over that
  helper's speed, asleep or not), else its step's mean, else what its jobs took here a target, else
  a first guess; each machine at its measured speed (a helper's: the build Mac's mean time a target
  over its own, for the shared steps both did, from the history; half until measured, said as a
  guess); each free once its job under way is done (its targets not yet done as they took last
  time, less what it's spent on the one under way; a job with no record, its step's time here, less
  what it's spent; if longer than its part's pace says; a helper's lease likewise). Run three times: as
  estimated, and for a range, the measured times a little off and the guessed much more. It says
  when each step, each region and everything will be done, when each region reaches the map, the
  rounds to come, what each machine does next (not what it's on) and its schedule to the end, and
  how much of the time was measured. A job under way is work left: its machine is busy until it's
  done, and one of the pass's worldwide jobs is on its schedule as it runs. No finish when nothing's
  left (no work, no round under way, no machine busy), when a new pass comes first, or while the
  units wait for the pass's heritage sites, reaches or buildings (the regions' work can't be
  listed): why instead.
- **The history** (`coord::history`): the coordinator keeps what happened, the last week's (50,000
  events at most), on the build Mac's disk (`coord/history.jsonl`, a line an event, numbered): each job the build Mac
  started and ended (what it does, what it finished, how long, how it ended), each lease a worker took, handed
  back, failed or let lapse, each task done or failed, the rounds begun (their regions), the
  catalogs (the regions they added), the
  pauses, the workers first heard from, the agents started, the build Mac's conditions changing
  (mains or battery, the NAS, home or away, a sleep), and each Mac's build caches trimmed or
  cleared, or an ask to clear them declined (what was freed, or why not; a helper's as the build
  Mac reads it in its status). Summed by the hour for the worker page (a job whose end went
  unsaid, its agent stopped, counted to the next agent's start), and the forecast's measure of the
  helpers' pace and of a round's time. Each cost the coordinator keeps says which worker measured
  it.

**Two Macs** (and any other worker: `docs/workers.md`). The build Mac's agent plans; it runs a
coordinator (`pipeline::coord`, port 8090) from which every other worker asks for work that fits it.
The M1's agent (`--helper`) plans nothing: it asks for the shared steps' jobs (it mounts the NAS)
and, when none fits it, units' last steps.
- **Shared steps** (`agent::claims::SHARED`, in this order of preference: what later steps wait on
  first): terrain, slope and tree cover (an area, a z3 tile, a job), units, and the landmarks'
  candidates and peaks. The rest stays the build Mac's: the pass, the worldwide sets, map tiles,
  indexing, trains, Wikidata and pageviews, heritage, publishing. The status marks each step a helper
  may take (⇄; the landmarks', its candidates and peaks).
- **What fits a helper:** each target is offered with the memory its job is expected to take: a
  unit's from its piece; another's what its last run took (the job notes, per target, the most its
  processes held together, sampled four times a second from the start of that target: a pool's
  workers summed, `SCENIC_COSTS`, "<step> <target>"), else candidates' their unit's (they read the
  same piece), else terrain by its area's size (it makes and writes a z6 tile at a time, holding
  that one's shaded hi tiles, up to 5,440 at ~270 KB, each z6 tile's z9 repairs and quarters, and
  its z12 repairs while its z11 is made, and the area's zoomed-out tiles: 3.3 to 4.6 GB, where
  holding the whole area's until they were written took 32.9 GB for 3/0/2; a measure from that way,
  `v` 0, counts for nothing now:
  `coord::cost_version`), else a first guess per step (tree cover 2.5 GB: its program held 1.05 GB
  on 14 threads for 3/2/2's 792 blocks, its measures from trees.py's workers, `v` 0, 12 to 36 GB,
  counting for nothing now; slope 2, holding a z6 tile's tiles at a time, its measures from when it
  held its whole area's, `v` 0, counting for nothing now); peaks 2.5. A helper asks only for the steps its
  disk has room for (a terrain run 55 GB free, tree cover 30, the others 15, a task 5, and a sixth
  more, counting what its caches can free: not its loose raw tiles, which only its own jobs pack),
  never while a newer app waits to start, and takes the earliest step with a target that fits, from
  the far end of the plan (terrain from the near end: the build Mac's next units wait on it), a
  job's worth (units: as many as it asks). A job it still has no room for
  once its caches are emptied goes back.
- **The same app:** a helper says which app it runs; on an older one than the build Mac's agent
  (its updater hasn't run yet) it gets nothing (409, why in words: its status shows it), since its
  work would be recorded under keys newer code made; on a newer one (the build Mac's agent finishing
  a job on the last) it builds, since a step the newer app changed is built again once the build
  Mac's keys say so.
- **The contact:** `state/coordinator.json`: the coordinator's addresses (Tailscale's, then the LAN
  name) and the token (kept on the build Mac) the agents' requests carry (a page helps with its
  device's own key: docs/workers.md §7); taken off the NAS when the agent stops. A worker reads it again when it can't reach the coordinator or its token is refused.
- **Leases:** work goes out on a lease (ten minutes, on the coordinator's own clock), renewed by a
  beat each minute while the work goes on, not while it's paused for its conditions (a helper beats
  through its client without the NAS too); a lapsed lease's work is offered again. While the build
  is paused, every lease is held, beats or not, and each has a whole ten minutes again as it goes
  on. The build Mac's own jobs hold leases too (a lapsed one is taken again if no one took its
  work), so a target is never built twice at once; each plan leaves out what's leased. Finished units
  aren't offered again before the plan shows them; a worker's failed unit isn't offered to it for an
  hour, doubling. The jobs' leases, the token and what units cost are kept on the build Mac's disk:
  its agent restarting (a new app) is a pause to workers.
- **One writer of the records:** the build Mac alone writes the manifest, its unverified uploads
  and the job keys (its agent, and its jobs). A helper's job saves its changes into an outbox folder
  per lease instead (`SCENIC_HANDOFF`, `pipeline::handoff`); when it ends, its agent sends them, merged
  in order, with the job's done record and what its units cost, as one hand-off, kept until the
  coordinator has it (across restarts). The coordinator takes it only for a lease it still holds
  (else 410: the work was offered again, and a late save could put an older build in the manifest)
  and only for the files its step saves for the lease's targets (a unit's base pack, road values,
  English and grids; candidates' and peaks' own; an area's lo pack and its z6 tiles' hi packs of
  terrain, slope or the tree layers), and journals it whole on the build Mac
  (`coord/journal/<worker>/`); the agent merges the journal before it plans, under its own lock (not
  while a paused job holds it), all of a hand-off or none, as it merged the NAS's hand-off files.
  Until they're merged, the agent plans with their done records on top of the keys.
- **Raw terrain tiles a helper fetches** (terrain, peaks): it packs them into archives and puts them
  on the NAS itself (content-named, written whole: as a unit's packs), keeping none, and hands the
  build Mac only their names (`Handoff::raw`: every loose tile in its cache, an earlier stopped job's
  too, each archive named for its area); merging the hand-off, the build Mac names them in the raw
  store's index, which it alone writes, each whose tiles are all its area's, and merges an area's
  archives when it next packs there. A hand-off it doesn't take (its lease gone, or refused) still
  has its archives named (journaled on their own, `coord/journal/raw-tiles/`), and so does a merge
  that couldn't name them then. An archive the index neither names nor lists to go, a day old (a
  hand-off that never came), is listed to go; and an index whose file is missing while archives are
  there is never written (one saved then would name none of them), unless the only archives are
  those being named (a helper's first). One that can't be read now is named with a later merge; a
  merge of nothing but those writes no records.
- **A helper's job that fails** hands off the targets it finished (they're kept), the rest held
  against it as a failure's; a hand-off refused from a helper on a newer app than the build Mac's
  (its step may save what the build Mac's doesn't know) isn't held against its targets.
- **For a helper on an older app** (one release): the build Mac still claims its own jobs' targets
  on the NAS (`state/build/claims/`), leaves out the targets such a helper claims, and merges the
  NAS's hand-off files (`state/build/handoff/<host>/`).
- **Enforced:** the build Mac's agent names its Mac in `state/build/writer` (every five minutes, by
  its name then), and its jobs carry `SCENIC_BUILD_MAC`; any other save on another Mac outside a
  helper's job (a step run there by hand) is refused.
- **Temporary names** are each Mac's own (kept results), and each process's too (uploads), so the
  two never write into the same one.

**Order:**
1. **The OSM pass**, when the NAS holds a newer planet than the newest pass.
2. **The pass's worldwide jobs:**
   - `pass-sets`;
   - hiking routes' ends;
   - the units' reach (`reach`);
   - `terrain-z8` (once);
   - roadside buildings (once per Overture release; the units wait for them);
   - summits;
   - labels.
3. **The regions' build,** a region at a time, each published as it's done (`agent::build::plan`):
   - heritage-sites (first, one job; not waited for by the rest);
   - a region at a time: the regions the map hasn't at all first (not in its catalog), then those it
     has (redrawn, or their units' keys changed: on the map as they were meanwhile); of each, the one
     with the fewest units left first, so regions are done as soon as they can be. For each, the
     terrain areas it reads that are stale (its own areas, and those of the z6 tiles within 30 km of
     its units), then its units whose terrain is built (a unit's key reads the terrain near it: one
     built first would be built again), neighbours together; a unit or area two regions share comes
     with the first. Then slope (each area once its terrain is built) and tree cover, after them for
     the build Mac. A helper takes the earliest shared step with work that fits it (terrain, slope,
     tree cover, units, …: what later steps wait on first), from the far end of all of that step's
     (the agent offers a step's targets together): the last regions', while the build Mac does the
     first's. Terrain it takes from the near end: the next region's, whose units the build Mac
     builds next.
   - **A round** when a region is done that the map hasn't as it is now, an hour after the last
     round began (`PUBLISH_EVERY_S`; before the agent kept rounds, after the last catalog went out
     or last started) while units or terrain are left, and at once after the last unit and terrain
     area. What it publishes is fixed as it begins (`agent::build::Round`, kept in the agent's
     folder, `round.json`): its regions, those done then, and the units as they were then, which
     its map tiles, road index, rail stops and catalog are made from (its jobs read them through
     `SCENIC_UNITS_AS_OF`, with when it began: a job of another round's fails). A region done or a
     unit built meanwhile waits for the next, and so does a region of it built again meanwhile (its
     outline redrawn). It's over once its catalog (or held catalog) is made, whatever changed in
     the meantime (that goes out with the next): it can't grow as the units go on being built, nor
     wait for work that keeps failing (its catalog goes out without that region). It begins once
     every helper's work done is merged (the units it counts as built are in its copy), and not
     while an edit is held; only the agent that runs the build's jobs keeps it (a dry run beside it
     plans with its own). Its work: the slope and tree cover of its regions' areas (slope's the z3 tiles
     within 20 km of it, tree cover's those it meets, as their targets go; after the last unit, all
     that's left), then a prune of what the coverage no longer builds (§5, Shrinking), the roads'
     chain, and a catalog. A region done waits for its round with its slope and tree cover made: the
     build Mac makes them as it's done, before more of the regions' work, so the round only draws.
     The units follow the round in the list (a helper's, and the build Mac's while the round's work
     waits out a failure). A round before the last draws only the map tiles that go out with it:
     those meeting a region it publishes, those no unit to build when it began is near (their 100 km
     halo), and those whose owners changed (Job keys, pack(T): their base packs go out as they are,
     which the tile must index); the others, a region's border tiles, would be drawn again in every
     round as their neighbours' units are built, and wait for a later one, or the last. A round
     with nothing to publish that isn't out already (after the last; with catalogs held, weighed
     against the last held one) is none.
4. **Three chains:**
   - **Roads**, in every round and after the last unit, its first stale step: a prune of map tiles
     no unit is near, road → units index, pack, lo, stations, ferries, terrain and slope roots.
     Stations and ferries drop the packs they no longer make.
   - **Rail service**, from the start (it reads no unit): `rail-feeds`, then `rail` (§6, Rail
     service). Nothing before the rail sources are seeded (`scenic-build rail-seed`), which the
     status says.
   - **Landmarks**, from the start, each step once what it reads is built
     (`agent::build::landmarks_work`): the candidates (once the pass's hiking-route ends are made);
     their peaks once every candidate is, the pass's summits are made and the terrain within 30 km
     of each is built (a unit's at a time); the items' facts and pageviews once every candidate is;
     the rest of the heritage chain on the heritage sites alone; the landmark points once those four
     are; the overlays (they read the built units) after the last unit.

   The trains' and the landmarks' work is listed after the regions' (the build Mac's own job takes
   it once the regions' work is done or waits): the second job takes it first, a helper the
   candidates and peaks.
5. **A catalog** once the roads chain is done, in a round: a new one whenever the served files
   change, or the regions it records (their recipes and the outline files they name), or which of
   them are done. It lists the units as they were when the round began. It records as built the
   regions done (`--ready <id>=<outline digest>,…`: every unit of theirs built as the coverage wants
   it, and their areas' slope and tree cover; one redrawn since the plan said so isn't; of those,
   the round's own and those on the map as they are), and the others as the last catalog had them,
   if it had them (on the map as they were); the Regions panel shows the rest as pending, or building with their areas
   counted. It waits while another worker builds a slope or tree cover area of a region it would
   publish (it would go out without the region, which would then wait an hour), and while a
   helper's hand-offs wait to be merged (their areas counted as built, their files not yet in the
   manifest). What the trains' and the landmarks' chains made goes out with the next round's
   catalog; after the last unit, a catalog follows any chain's change. While
   `inputs/hold-catalog` exists, it goes to `catalog-held/` instead (and the rounds go by the held
   ones; the first, by the served one).
6. **Daily:** backup and GC (not while a round is under way: the units it reads as they were may
   be in no catalog yet).

**Planned:**
- stale work after step-version bumps, done oldest first in idle time;
- registers, Overture and timetables fetched every ~6 months.

**Interruptions.** The build Mac may be asleep, away or unplugged at any time, or close its lid
mid-job. Nothing depends on it being available at a given time.
- **No deadlines.** Until work is done, the map serves the last catalog.
- **Conditions per step:**
  - Every job needs the NAS: at home, or through Tailscale away from home, except the whole-planet
    reads, which need home. CPU jobs run on mains power, or on battery down to 30 %.
  - When the NAS goes (or home, for a whole-planet job), the agent freezes the job (`SIGSTOP` to its
    process group) and lets it go on (`SIGCONT`) when it's back; on battery under 30 %, a CPU job
    stops at its next safe point instead (Pausing, above).
- **Sleep** suspends every process. Open SMB handles often don't survive it, so a job that touches the
  NAS is restarted after wake.
- **Kills** lose only the target under way: every job notes each target done as it's saved, and
  the agent records those whatever ends the job (a failure, sleep, a lapsed lease, its own stop, or
  after a crash, from the job's record).
  - A batch takes minutes to tens of minutes.
  - The OSM pass is a chain of stages with completion markers; its filter and basemap stages take
    an hour or more each.
  - The planet's copy and the mirror resume by byte range; uploads restart their `.tmp`.
- **Atomic writes** (§3): anything half-written is never referenced.
- **Caches are disposable:** they refill from the NAS and the original sources.

**Status.**
- `scenic status` (each Mac's build caches too); `scenic clean` clears this Mac's (Room on the
  disk, above).
- The app's status bar: the build Mac's state, the NAS, and new data or a new app in.
- The menu bar item. It asks the local server (`/api/build`): this Mac's agent's status when it runs
  here, else the NAS's copy. Each agent writes its status at least every two minutes; one not heard
  from for six is shown as out of touch (asleep, off, or stuck), not as it last was. It pauses and
  resumes the build (Pausing, above), as does the map's build panel. From this Mac's agent's own
  status, read in its folder (the build Mac's `status.json`, a helper's `helper.json`), it shows
  what that Mac's build caches hold and their last trim and clear, and offers Clear the Build's
  Caches, after a confirmation (Room on the disk, above).
- Each says what's waiting and why ("Build Mac last seen yesterday; Kanto waits for it to be plugged
  in at home").

**Determinism:** the same inputs give the same bytes, on any machine and in WebAssembly: sorted
outputs, no hash-map order, reductions that don't depend on the thread count, and every
transcendental function from one implementation (the `det` crate, over `libm`: the platforms' own
differ in the last bit). Checked by hand so far (terrain, slope, units, candidates, trains a day;
a dense unit's Rust steps natively on 1 and 14 threads and as WebAssembly, docs/workers.md; tree
cover's blocks the same way, `tools/check/trees-same.py`);
planned: a "build twice, compare hashes" test per step.

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
2. runs the crates' tests, the type check and the web build, and checks every Python step the agent
   runs loads from a copy of the app's `dem/` (in an environment made from its lock file for the
   check, then thrown away) and passes pyflakes;
3. smoke-tests a server on a spare port against the NAS's catalog;
4. writes `app/<version>/` and `app/current.json` with every file's SHA-256;
5. copies `tools/nas/fetch-planet.sh` to the NAS's `nas/` when it differs.

`publish.sh --rollback` swaps `current.json` and `previous.json`. The agent never fetches from git.

**Programs:**
- `scenic` is the user's command and the agent;
- `scenic-build` holds the build steps;
- `server` serves the map;
- `extract`, `tile` and `scenic-metrics` are today's steps, which units run (the rail job runs
  `extract` and `railfreq`, the tree cover job `trees`);
- the app also carries `dem/` (the Python steps), Scenic.app (the menu bar item), `web/` and
  `fonts/`. The Python steps run in `dem/.venv` beside them, which uv makes from the app's lock file
  (`uv.lock`) the first time a step runs on a Mac (from uv's own cache of the packages, else PyPI),
  the heritage chain's in its stand-in root too: nothing of it is on the NAS. (The agent finds uv,
  and Homebrew's osmium, zstd and Java 21, on the PATH `tools/app/install.sh` gives it.)

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
| roadside buildings (the world's, once per Overture release; ~2.5 billion boxes) | ~40 GB | the same |
| an app Mac's mirror | everything, ~200 GB (M1: budget-limited) | budget-limited |

A retired pass's sources go 14 days after the next pass completes, so the NAS holds about two
passes' sources at most.

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
     - terrain, slope and tree cover per z3 pack;
     - the worldwide z8 terrain;
     - the world's roadside buildings, once per Overture release;
     - grids inside the units.
   - The 2026-09-28 planet's pass is complete, with its worldwide jobs.
   - **Not built:** the sea mask.
4. **Per-unit pipeline and rankings: mostly done.**
   - **Built:**
     - base(U): the pilot (Northumberland and the Scottish Borders) matched today's data, with the
       same ways, elevations within 1.8 m and every scenic channel and flag;
     - heritage sites and flags;
     - labels, stations, ferries;
     - the landmark jobs: pois, peaks, items, marks;
     - the rest of the heritage chain and its consumers, checked against today's: 17 outputs and
       every overlay pack byte for byte; fame differs where today's was stale;
     - the rail service (trains a day), checked against today's on a scratch copy of the NAS's
       sources: every one of the 131 feeds with the same typical day, trips and duplicates, the
       same 38,445 stop pairs with the same trains, and 185,557 of today's 185,604 rail ways with
       the same trains a day (34 differ and 13 have none, where the same trains take a parallel
       track or OSM changed since); two runs give the same bytes.
   - **Not built:** names and descriptions todo, determinism tests, validation.
5. **Browser: done.**
   - Built:
     - landmarks, stations, ferries and overlays by view (In view answers equal to the legacy
       worker's in 163 views);
     - zoomed-out queries;
     - the Regions panel (add, rename, remove);
     - the status bar;
     - catalog switching.
   - **Not built:** drawing, splitting and merging regions; "Keep this view".
6. **Cutover: mostly done.**
   1. Done: today's 34 regions, as 34 recipes (`tools/cutover/regions`), built from the 2026-09-28
      pass into a held catalog and compared with today's map by `compare` (its report,
      `inputs/hold-catalog.compare-2026-10-04.md`: the same 181 units, their counts and
      distributions within 0.2 %); the hold released on 2026-10-04
      (`inputs/hold-catalog.released-2026-10-04`), so the map serves the pass's build.
   2. Done: the regions as 88 recipes by political unit (§5), every one built (284 units) and
      published (catalog 14, 2026-10-06).
   3. Left: deleting the converted legacy data, once nothing the map reads comes from it. The
      served catalog lists 38 of its files (`global/legacy/`), which the server reads: the popups'
      details (`details-*`), roads' English (`road-en`, under the units' own `global/roaden/<u>`),
      and the whole layer files the map fetches by name (`/api/layer/…`: ferries, stations,
      overlays) where the overlays job has no copy of its own (`global/heritage/`).
7. **Features,** each on its own.
   - Built: the terrain repair (`roadcore::grid::repair_terrain`, in the terrain job: voids
     filled, towers and spikes flattened, summits and ridges kept, with tests; §6).
   - Planned: 3D buildings (`docs/buildings3d.md`: designed, its sources on the NAS, its steps not
     built), then PLATEAU; building heights in horizons and the viewshed tool; sharper terrain from
     national DEMs.
8. **Builds anywhere: under way** (`docs/workers.md`). Done: the crates build for WebAssembly; one
   maths library on every target (outputs identical natively at any thread count and under WASI);
   the data plane's SSD copies and prefetch; the coordinator (leases, hand-offs over HTTP, learned
   memory, the shared steps' jobs for the M1); a unit's last steps as tasks for any worker, the web worker page and the M1 alike; the
   units' Python steps in Rust (`elev`, `landcover`, `areaflags`: the same bytes), and tree cover's
   (`trees`: the same pixels); the build page, a dashboard for anyone and HTTPS through `tailscale
   serve`, its devices let help by the build Mac's owner (`coord::devices`). Next: OPFS, ranged
   reads, journaled group commits, retiring the claim and hand-off files.

**Gaps:** the code falls short of the design here.
1. **The pool's core** (`crate::pool`, `crates/pipeline/src/pool/`; `docs/pool.md` §12), not wired
   into the agent: the open items of its review, to fix before the integration. The review's
   scenarios are in `docs/pool-review-scenarios.patch`: a unit test showing each finding, and knobs
   for the simulator (listing time, a week of journal folders, a Mac leaving, staleness, clock skew,
   development builds and rollbacks).
   - **H1, a lead re-asserts every loop when listings are slow** (sim.rs:852, 891; records.rs:274):
     a take-up lists the whole journal, so its loop runs past `GAP_S` (60 s), and the next loop
     re-asserts and takes up again. Fix: time a loop from the end of its take-up; re-assert only
     after wall-clock gaps. Then a take-up slower than `TAKE_UP_S` (120 s, handover.rs) still has
     its handover taken back: have B list the journal while Ready, and A count "term E+1 has a
     snapshot" as taken up.
   - **H2, the app rule can leave no working lead** (term.rs:54, 149): a lead restarted into an
     older or development app has its re-assertion refused, and its loop stops there every time (it
     should step down, an error stopping only the duty that met it); a handover to a Mac on a newer
     app that never takes up can't be taken back, nor taken over by force from an older app. Fix:
     exempt the take-back; give the owner an override for a downgrade.
   - **M1, setting an entry aside is final, and a stale lead can do it** (journal.rs:180–188,
     records.rs:195). Fix: let only a fenced lead set aside, or have later leads check again.
   - **M2, a corrupt entry's `at` panics every reader** (journal.rs:93). Fix: `checked_add`, and
     take such an entry as damaged.
   - **M3, a torn read (a hole of zeros) is refused for good** (journal.rs:150–157). Fix: write
     entries by a temporary name and rename them.
   - **M4, a create cut short disowns its own term** (nas.rs:84–91, term.rs:123): tried again, its
     maker reads the term as another's. Fix: have `create_new` report "made, bytes not written", for
     its maker to finish; compare the parsed term in `make`.
   - **M5, lease order holds only within one merge** (records.rs:179–215): an older lease's entry
     merged later wins. Fix: keep the last `LeaseId` per (step, target).
   - **M6, the driver that decides safety and liveness is test-only** (`sim::Mac`), with its one
     `now` per loop, a `settled` that passes after `SETTLE_S` (handover.rs) and a `ready_for` not
     tied to its offer. Fix: move it into the core behind an I/O trait, and fix those there.
   - **M7, a handover's `seq` covers the records only** (term.rs:39–43): nothing makes B read the
     coordinator's leases as A last wrote them.
   - **M8, keys forgotten after GC's week block settling for good** (records.rs:146, 199): an entry
     told again once its day is forgotten is in neither the records nor the journal, and waits. Fix:
     keep the forget horizon, and acknowledge older keys.
   - **Low:**
     - L1, a retried `take_up` restarts `seq` at 1 (records.rs:270–273), so two versions of a
       term's first snapshot can share a number;
     - L2, `BUSY_TRIES` (nas.rs:12) gives a busy rename 8 tries 250 ms apart,
       crate::whole::rename_over 20 retries;
     - L3, the member id is a plain file (mod.rs:69, `member_id`), which Migration Assistant copies
       to a new Mac: bind it to the Mac's IOPlatformUUID;
     - L4, an entry's key takes the day of its `at` (journal.rs:86–88), so one made again for a
       retry after midnight lands under another key: persist the `Entry` before the first try;
     - L5, term 1's first snapshot torn mid-file (a hole, not a short end) can't be read, nor term
       1 taken up (records.rs:247–250);
     - L6, no reader falls back to an older term's snapshot while the current term has none
       (records.rs:81, `Records::load`);
     - L7, a gone Mac's entries it never told a lead of wait for the next take-up, the only listing
       of the journal (records.rs:274).

## 11. Risks and checks

- **NAS throughput:**
  - writes ran at 12 MB/s through Tailscale's userspace networking until the LAN name;
  - a hung SMB session once stalled this Mac's reads, even a forced unmount, for minutes.
  - Planned: a rate limit on the pass's uploads, and remounting a hung mount (the breaker's probe
    times out) rather than waiting.
- **The build Mac's availability:**
  - work progresses only while the M4 is awake, reaches the NAS (away from home through Tailscale,
    at ~12 MB/s, the whole-planet and whole-world jobs waiting for home), and has power;
  - a closed lid stops building, and nothing is lost while it waits;
  - the M1 builds units too when it's open (§8, Two Macs); everything else waits for the M4.
- **Dense units:**
  - the densest (Kanto, a 3 GB base pack converted) is first built in the cutover;
  - if a unit or its 110 km halo doesn't fit in 48 GB, units split into z7 or z8 tiles, and pack(T)
    by z7.
- **Remote DEM servers** may be slow or change. Today's cache seeds the units, and their new samples
  are kept on the NAS (`cache/dem-units/`).
- **Version bumps at globe scale** would take days: today a bump makes every target stale in the
  normal order.
- **Way ids past u32** (2040s): the tile format is versioned.
- **Disk:** for 14 days after a pass completes, the NAS holds two passes' sources (~400 GB).

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

**Implementation decisions since v6 (2026-10-02 to 06), now in the sections above:**
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
- **Roads and landmarks build as two chains,** for the same reason; the rail service is a third
  (an operator's server down mustn't hold up the roads either).
- **The rail service reuses today's feeds:** the legacy build's zips, catalogue and checks seed its
  sources (the user asked, 2026-10-04, that what the research sessions collected be reused, and only
  what new coverage needs be fetched). A zip counts from the day it was fetched, so it's never
  fetched again just because time passed.
- **The rail feeds' countries come from the coverage** (§6, Rail service), so a region in another
  country has its own feeds.
- **Elevations are u16 decimetres from −500 m.** They were clamped at ±3,200 m, and roads in the
  Andes and the Himalaya reach 5,800 m.
- **Failures are never cached, and the breaker needs a failed probe.** A busy link slows reads
  without the NAS being gone.
- **Power: mains, or the battery down to 30 %** (asked for 2026-10-03); caffeinate per job.
- **The menu bar item,** with progress to the end (asked for 2026-10-03).
- **The cutover keeps today's 34 regions as 34 recipes,** with Gibraltar as an `osm:` relation.
  Keeping the legacy outlines makes the comparison like for like.
- **What the internet answered is kept on the NAS** (§4, Downloads): the items job's and the
  heritage chain's Wikidata and Wikipedia answers, a pass's at a time, as an archive each step
  writes as it starts and ends. They were on the build Mac alone, so another Mac leading the build
  (docs/pool.md) or a lost disk would have asked again (the 2026-09-28 pass's 59,000 items took
  about 37 minutes of queries), and the owner wants nothing fetched twice without a good reason.
  The heritage scripts run in the app's Python environment, as the other steps do: theirs was the
  only one kept in the cache.
