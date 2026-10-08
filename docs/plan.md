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
- Zoomed out, nothing small leaves the map: every road is drawn at every zoom. The water zoomed out
  looks as the full-detail map would rendered and then shrunk to the screen (the owner's choice,
  2026-10-08): an island or lake smaller than a pixel is as faint as its share of the pixel, the
  same at every zoom, never exaggerated nor dropped, and keeps the coastal shading full detail would
  give it (§6, Water).
- `scenic status` shows what the build Mac is doing, and so does the menu bar item on both Macs
  (Scenic.app, `tools/status`). It shows the state as an icon: building, paused, waiting, nothing to
  build, a problem, or out of touch. A click opens the build page in a popover (the jobs, the
  machines, the schedule, every step to the end); a right-click opens its menu, which pauses and
  resumes the whole build, hands over the lead, sets the disk room and clears that Mac's build
  caches once the build is done (`scenic clean` too). It sends a notification for every change, on
  time even while a menu is open.
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
  - **Downloads on this Mac** (§4, Mirror, per Mac): nothing is copied to a Mac unless it's
    downloaded there, and nothing downloaded goes until it's removed.
    - **World, zoomed out**: a row of its own with its size and a Download or Remove button. Not
      downloaded, it says so: the map needs the NAS to show anything.
    - **Each region**: its size, and Download or Remove; downloaded, its state (downloaded, copying
      N %, next, waiting for room, away). A region's download brings the World too (away from the
      NAS it needs it), and the World can't be removed while a region or view is downloaded.
    - **Download this view** downloads the ground on screen the same way, named after the place
      search's most important place in it (renamed with ✎); it first says what the view takes and
      the free space above the reserve.
    - Remove asks once more, with the size that goes. A download that wouldn't fit (all that's
      downloaded and not here yet, more than the free space above the reserve) is refused, saying
      so with the numbers.
    - The Mac's own line: what's downloaded, the free space and the reserve; the copy under way,
      and when the build runs, that downloads keep to 20 MB/s.
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
- After each build the agent lists what still lacks English, or a description, in each folder's
  `todo/`, with the translators' and writers' briefs.

**Everything else is automatic:**
- **Building and refreshing:** whenever the build Mac is awake, reaches the NAS, and has power
  (plugged in, or on battery down to 30 %). Away from home it builds through Tailscale, slowly; the
  OpenStreetMap pass and the other jobs that move the whole planet or world through the NAS wait
  for home.
- Copying what's downloaded to each Mac, and keeping it current with each new catalog.
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
     routes, heritage-named objects, water);
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
   - the water's coverage at every zoom;
   - once ever, the worldwide z8 terrain for peaks.
3. **Heritage sites and designated areas:** one job over the coverage plus 20 km.
4. **Global-source layers near the coverage:** terrain, then slope, per z6 tile near the coverage,
   each z3 tile's zoomed-out terrain and slope assembled from them; tree cover per z6 tile the
   coverage meets, then each z3 tile's zoomed-out tree cover assembled from them (terrain
   with the first region that reads it; slope and tree cover as the regions in their area are
   published: §8, Order).
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
translations/  descriptions/  the user's drop-ins (descriptions/README.md); todo/: the agent's lists (§7)
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
               rail feeds: the catalogue, their zips, the MTR's lines; §6), terrain-z8-v3,
               copernicus-dem/ (GLO-30's 1° tiles north of 59.5°N), registers/ (the heritage
               registers' snapshot and its seeds: §6, Hand-made inputs)
base/          base packs, one per unit
hidata/        per z6 tile: the ways-here index, query parts, climbs, rail lines, zoomed-out summaries
markdata/      per z6 tile: landmark points
ovdata/        per z3 tile: area and park details
global/        worldwide files: road values per unit (roads/), road → units, rail frequencies,
               landmark totals, heritage/ (the overlays' summary and sources)
layers/<layer>/  root, lo and hi packs; basemap/world-<date>.<hash>.pmtiles
work/          build intermediates (not served; trees-mid/: tree cover's mids, §6)
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
- **Job keys** (`state/build/jobs.json`) say what each output was made from;
  `state/build/jobs.pre-rekey.json` keeps them as they were before the first re-keying (§8, A new
  key scheme).

**Deletions go through SMB.** They're permanent on this share. The build Mac can't use SSH
unattended, because 1Password asks to approve every new session.

**Copies carry the bytes and permissions, nothing else** (`store::sys::copy_data`; `cp -X` in
the scripts). macOS puts a provenance attribute (`com.apple.provenance`) on whatever an app writes,
and the share refuses a copy's attempt to set one that differs from its folder's. That makes
`std::fs::copy` or a plain `cp` fail with "Permission denied" (seen 2026-10-05: a file another
program wrote, copied into the backups).

**Packs, never one file per tile.** SMB manages about 80 random reads per second per file.
- **Small inputs read again and again** (the region recipes, the ferries' timetables): the agent
  and the map server keep each in memory and read it again only when its size or time changes
  (`pipeline::smallfiles`; a file changed in the last 5 s is always read). Opening and reading
  them took 1–11 s for each set under load (8 Oct 2026), stat'ing them a fraction of a second,
  and a plan read them twice.
- **Our layers** (roads, rails, terrain, slope, trees, labels, water, overlays,
  marks, stations, ferries) use packs.
  - A pack is a header and meta, then the blobs (identical blobs are stored once), then an index of
    (tile key, offset, length, raw length, 64-bit content hash) sorted by key.
  - **Root pack:** z0–2. **Lo packs:** z3–8, one per z3 tile (roads and rail z4–8). **Hi packs:**
    z9–14, one per z6 tile.
  - Hi tiles exist only near the coverage for the layers built by area (terrain and slope within
    20 km of it, the rest for the built units); the worldwide layers (labels, water) have
    them wherever they have tiles.
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
  the basemap's data, the DEM seed, the rail sources with the files they replaced), translations, descriptions, inputs, state, app and nas.

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
    starts when the NAS hasn't what the Mac has, and as it ends, finished or not, when it changed
    them (a step stopped, as the agent stops one, sends them at its next start): ~10 MB for the
    2026-09-28 pass. Which counts as a step starts: the Mac's while the NAS's archive is the one
    they last matched (a mark beside them says which), so what it fetched since goes up, and a
    file deleted there by hand (cut short, or facts to ask for again) stays deleted, the archive
    sent without it; the NAS's set when it isn't that one (another Mac ran the step since, or the
    Mac has none yet), made one with the Mac's: what only one side changed since, that side's; a
    file both changed, merged when it's answers kept by key (JSON lines by their item, a cache's
    JSON entries, the days fetched), else the NAS's set taken whole. As a step ends, the same,
    but never over another writer's archive (the heritage-sites and heritage jobs share one; once
    the lead can move, another Mac may run a step meanwhile): merged where it can be, else left,
    for the next start to take. An archive that doesn't read whole is moved aside
    (`<archive>.bad-<unix seconds>`), not written over. The build Mac's agent, as it starts,
    sends the pass's answers its cache has where the NAS lacks their archive. A new pass asks
    again (the items' everything, the heritage chain what the snapshot lacks).
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
    - copies of the records' files staging reads (`blobs/`), filled from the store;
    - copies of the pageview months' indexes (`items/months/`), filled from `sources/pageviews/`
      (one the NAS lacks stays, for pageviews.py to put there); none goes while an items or
      heritage job reads them.
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
  - **Kept:** the Wikidata and Wikipedia answers the items and heritage jobs keep (`items/`, but
    its pageview months, and the pass's copy of the registers' snapshot, which the heritage scripts
    add theirs to: `heritage-<date>-<id>/`), which the NAS keeps too (Downloads, above) but which
    free little (the items' 13 MB, the copy's own ~0.4 GB); the heritage scripts' old Python
    environment (`heritage-venv/`, which their next run removes: they run in the app's); the
    registers' snapshot,
    extracted (`registers-<id>/`: the pass's copy is an APFS clone of it, so deleting it would free
    next to nothing); the trains' stop pairs (`rail/`, under a MB); the unit stages' timings
    (`unit-stages.json`); and what a unit kept that isn't on the NAS yet (`dem-units/`,
    `scenic-units/`).
- **Its own map** is served from the NAS and what it has downloaded (§4, Mirror, per Mac); its
  downloads keep a 150 GB reserve so builds have room.
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
  is at it, all but two otherwise; each job started with its step's room free (15 GB, terrain's
  pieces too, a task 5), from the caches the NAS keeps; only work it can make that for is asked
  for, and a job it can't is given back.
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
  - It keeps the current version, its own, the agent's and one to roll back to, and removes the
    rest. A half-copied `<version>.tmp/` that nothing has written to for an hour goes too.
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
- **Offline start:** the last catalog and the pack indexes read so far stay local.
- **In use** means any request in the last ten minutes, except the status polls (`/api/catalog`,
  `/api/ping`, `/api/build`, the Regions panel's `/api/downloads`) and the place search's asks while
  its index is made (`poll=1`).
  - An idle server loads nothing; warming starts at the first request (starting isn't a use).
  - It checks the NAS for a newer catalog every 30 s while in use, every 10 minutes otherwise.
- **URLs and ETags:**
  - Data URLs carry a content version (`?v=`: a file's hash, a layer's packs' content names, plus
    the translations' version over every language for named data).
  - A versioned response is cached for good (`immutable`) while that version is current, else it's
    revalidated.
  - ETags are content hashes, combined for named tiles with the versions of the languages spoken
    within them and of the spoken-languages raster (§7). The basemap's are its archives' content names and the tile's position, known from the
    catalog, so its 304s read nothing.

**Mirror, per Mac** (`store::mirror`, `store::pieces`, `crates/server/src/downloads.rs`).
- **Only what's downloaded** (the owner's ask, 2026-10-06; decided 2026-10-08). Nothing is copied
  to a Mac by itself: the owner downloads it in the Regions panel (§1), and it stays until it's
  removed. The map reads a file from the Mac when it's there, else from the NAS: with nothing
  downloaded, the map works only while the NAS is reachable. What's downloaded is this Mac's alone
  (`downloads.json` in its home, never the NAS).
- **The World, zoomed out:** the essentials, and the basemap's zooms 0–10. The essentials are the
  build's worldwide files (`global/`: rail frequencies, the road → units index, landmark totals,
  heritage summaries, roads' English names, the spoken languages), every layer's root and lo packs (zooms 0–8), and the landmark points and area
  details (markdata, ovdata): 7.8 GB of catalog 14's 251 GB (2026-10-06). Not the pass's area
  outlines (2.7 GB, read only to make regions). The basemap's zooms 0–10 are 1.7 GB of its 28.5.
- **A region, or a view** (the ground on screen when it was downloaded): every layer's hi pack,
  and the base pack and road values, of each z6 tile within 2 km of it (outlines are simplified);
  the basemap's zooms 11–14 over those tiles; the terrain's and the grids' hi packs within 25 km
  (the viewshed's reach, so one from inside the area works); and the hi data of the z6 tiles
  within 50 km (what the lists of a view inside it read around it). With the World, that's what the
  map reads there, so a downloaded area works fully offline (but §10, Gaps). A region's files are
  worked out from the catalog's recorded coverage. Downloading a region or a view downloads the
  World too, and the World isn't removed while one is downloaded.
- **The basemap's pieces.** The basemap is one PMTiles file of 28.5 GB, so a download takes pieces
  of it, each a PMTiles archive of its own (`store::pieces`, `mirror/.basemap/<archive's
  hash>/`): `lo` (zooms 0–10, the World's) and `6-x-y` (a z6 tile's zooms 11–14, a region's, shared
  by every download that meets that tile). On PMTiles' Hilbert curve a z6 tile's tiles at each
  zoom are one run of tile ids, and the archive is clustered, so a piece is four runs of the
  directory and mostly sequential reads of the data: large ranged reads, never the whole file.
  - A piece is laid out from the archive's directory before any tile is read: its size is known
    ahead (each piece's size kept in `sizes.json`; the server works out every region's while it's
    idle: catalog 16's regions take 381, sized in 78 s by a development build on the M1), and a piece cut short
    resumes where it stopped. Each tile's bytes are copied as stored, its gzip checksum checked.
  - The server reads a basemap tile from the piece holding it when this Mac has it, else from the
    NAS's archive, as before (the water's deeper zooms, drawn from the basemap's z14, alike).
  - Why pieces rather than a sparse copy of the whole file: a piece is removed by deleting it (no
    holes punched in a sparse file, whose room APFS gives back unevenly), its size adds up as the
    packs' do, two regions share a z6 tile's piece, and the reader opens it as any archive.
- **Kept current.** Each new catalog's files that a download names are copied, and what no
  download names any more goes: the files it replaced, the pieces of a replaced basemap, a removed
  download's files. Nothing else goes, ever.
- **Copies:** one file or piece at a time, in large sequential reads through the I/O pool, in
  the order the downloads were made, the World first; within each, small worldwide files, root and
  lo packs, hi data and road values (and the per-tile records), base packs, hi packs, the 3D
  buildings' hi packs (a city's z6 tile is a few hundred MB: the roads and terrain first), then the
  basemap's pieces. While the build runs (the build Mac's heartbeat on the NAS says a job runs,
  as the copies' pause read it before downloads), copies keep to 20 MB/s, so the build's uploads
  have the NAS first, and the panel says why a download is slower; the rest of the time, at full
  speed. A copy is checked (a file against its content hash, a piece tile by tile) before it's
  moved into place.
- **Room.** The disk keeps a reserve free (50 GB; 150 GB on the build Mac: the server's
  `--reserve-gb`, in GB of 10⁹ bytes). A download that wouldn't fit (all that's downloaded and not
  here yet, more than the free space above the reserve) is refused, saying so. When the disk
  fills later, what doesn't fit waits ("waiting for room"), and nothing downloaded goes for it.
- **The build Mac:** its jobs don't need the mirror. The pack and lo jobs read a base pack from it
  where it has one (a downloaded region's), else from their own cache (`agent/cache/base`, copied
  from the NAS); the marks and overlays jobs read the heritage job's files from it where it has
  them, else from the NAS. Nothing is deleted from the mirror while that Mac's own agent runs
  a job, so a job never loses a file it's opening.
- **Pack indexes** are cached on a Mac as they're read (`idx/`), for offline starts; none is
  fetched ahead.
- **From before downloads** (the switch, 2026-10-08): a Mac's kept regions and views
  (`keep.json`) become downloads, with the World; the use times (`mirror/.uses`) go. At its first
  round the new server lets go of whatever no download names (with nothing downloaded, all of it:
  the M1's 7.3 GB of essentials, the build Mac's 1.8 GB) and copies the rest; what a download
  names and the Mac has stays as it is, never copied again. A whole basemap kept before goes too,
  its pieces copied from the NAS.
- The state, for the panel (`/api/downloads`): the World's and each region's size and how much of
  it is here, each download's state, the free space, the reserve, how much more room the downloads
  need, the copy under way and whether it's slowed, the last round. The menu bar says what's
  downloaded, and when nothing is, that the map needs the NAS.

**Devices: an iPhone, an iPad.** Either Mac's map opens on them, at home or away, over the tailnet.
- **Who's answered:** the server listens on every IPv4 address (and IPv6's loopback) but answers
  only this Mac, its LAN and the tailnet, also through a proxy on this Mac (`tailscale serve`) from
  those alone (`pipeline::net::reached`): anything else is refused (403), Tailscale Funnel's requests
  (the internet's, handed over from loopback) and a forwarded address that isn't one of those
  included. No key: a device it answers gets the map (the owner's choice, 2026-10-08).
- **A page elsewhere is never the map's,** in a browser on this Mac or on a device:
  - A request must name the map in its `Host`: an address, localhost or a name under it, or a name
    only a tailnet or a local network resolves (one label; `.local`, `.home`, `.lan`, `.internal`,
    `.ts.net`). A public name pointed at this Mac (DNS rebinding) is refused.
  - A request from a page (`Origin`) must come from the map's own page or from this Mac's.
  - Cross-origin reads (CORS) are allowed only to this Mac's own pages, whose downloads go to
    `roads.localhost` and the like.
  - So no site can read the map's data or change its regions through a browser that reaches it.
- **The address** to open on a device is `<home>/map-page` (`crates/server/src/remote.rs`), which
  the server rewrites when it changes: HTTPS where `tailscale serve` proxies the server's port at the
  root of an HTTPS port of its own (the app asks for `/api/…`, so not under a path: `tailscale serve
  --bg --https=8443 http://127.0.0.1:8080`), else the tailnet address. The status menu's Copy the
  Map's Address and `scenic status` give it.
  - Only a proxy that says it is one (`tailscale serve`'s HTTPS sets `X-Forwarded-For`) keeps the
    requests it hands over from passing for this Mac's own. A plain TCP forward on this Mac would let
    anything it forwards in.
  - The map's data stays on the owner's own devices (the licences): anyone on the LAN or the tailnet
    can open the map, so the LAN is a trusted one.
- **An app** (`web/public/manifest.webmanifest`, the icons): Share, then Add to Home Screen, full
  screen. Its service worker (`web/public/sw.js`) is registered only over HTTPS (so with `tailscale
  serve`), and never on this Mac's own address (localhost: the Macs have the data themselves). What
  it keeps for when the Mac is away:
  - the page, all of its scripts and styles (those it names and those they name: the map's
    workers) and the catalog's metadata, kept as it installs and again with each new page; scripts
    and styles a newer page no longer names go;
  - the fonts and icons, and the map's versioned data (`?v=`) that the Mac says never changes
    (`immutable`: its version still current), as it's looked at (the last 12,000 files kept); the
    3D buildings' tiles in a cache of their own (the last 2,000: a city's z14 tiles are 50–300 KB,
    a view's ~20), so a city's buildings don't crowd out its roads and terrain.
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
    coverage), the terrain packs, the tree cover per z6 tile, the landmark candidates and the
    heritage-sites job.
- **Shrinking:** what only the removed part built leaves the manifest once the units are built (a
  prune): the outputs of units no longer built, their candidates and peaks, and map tiles no unit's
  ways reach any more; pack and lo also drop what a tile no longer has (a tile without ways, a
  layer without tiles). The next catalog drops them, and GC frees their files. Terrain, slope and
  grid tiles stay, which is harmless; a z6 tile the coverage has left loses its tree cover (its hi
  packs and mid), and a z3 tile its zoomed-out tree cover.

**Today's set** (since 2026-10-05): 88 recipes in `inputs/regions/`, by political unit, every one
of them OpenStreetMap boundaries (`osm:` relations from the pass's outline set).
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
- the languages spoken there, for names (§7: CLDR's per territory, refined per subdivision in
  `names::spoken::REFINED`);
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
   - `pass-sets` makes a finished pass's missing sets from (a): copied to the SSD first when that
     leaves room for the sets and the agent's reserve (its size, 20 GB and 30: 111 GB for the
     2026-09-28 pass's 60.6 GB), else read from the NAS, two or three times a set. The build Mac
     (~40 GB free) reads it from the NAS: the water set took 65 min so (2026-10-06).
5. **Outlines:** administrative levels 2–8 and ISO 3166 areas, assembled into polygons. They go in a
   sectioned file, with simplified copies for the panel.
6. **Basemap:** Planetiler on (b), worldwide, giving `layers/basemap/world-<date>`. Its jar and data
   (Natural Earth, water polygons, lake centerlines) are pinned in `sources/basemap/`. The pass
   stops before drawing it with a jar of another version (its `buildinfo.properties`) than the one
   whose z14 water the water layer takes as full detail (`pipeline::water::PLANETILER_VERSION`,
   0.10.2), until the shoreline check (`tools/coastcheck`) has checked the new one's.
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

- **Terrain** is built in two steps (`pipeline::terrain_pack`), as tree cover is:
  - **terrain**, a piece per z6 tile near the coverage (`build_piece`; a helper takes them too):
    its z9–12, within the coverage plus 20 km (viewsheds see 15 km), zoom capped by latitude so
    pixels stay ≥ 15 m (z12 to 67°, z11 to 79°, z10 beyond): its hi pack; and its mid
    (`work/terrain-mid/6-x-y`, a sectioned file, never served: its z9 tiles' 2×2 means, as f32,
    which the stored tiles can't give back, and its lakes' levels).
  - **terrain-lo**, an assembly per z3 tile with a piece (`build_lo`, the build Mac's): the whole z3
    tile's z8 → z3, z8 and coarser made again from their children where those exist (a piece's z9
    tiles' means, from its mid), since AWS's coarse levels come from coarser sources: its lo pack.
    Not a piece's z8–z6 alone: a lake's level at z8 is gathered from the whole area's z8 tiles.

  Together they make the same packs, byte for byte, as the area's whole run (`build_q_with`, which
  `scenic-build terrain 3/x/y` runs by hand, is the pieces and the assembly in memory): checked
  against main's area run (TERRAIN_V 3) on synthetic tiles with GLO-30, a lake across two pieces and
  the walled patches' z9 tiles (`terrain_pack` tests), and on the build's own data (§10).
  - **The root (z0–2):** from the lo packs.
  - **Sources:** AWS's raw tiles, each downloaded once (64 at a time) into the build Mac's cache
    and packed onto the NAS (`sources/aws-terrarium/packs/`: §3 Downloads), which fills the cache
    when it lacks one; north of 60°N, Copernicus DEM GLO-30 (`sources/copernicus-dem/`, below); and
    the water of the latest pass's basemap (below). **The terrain depends on AWS's tiles, GLO-30's
    tiles, the basemap and the code**, each pinned in the terrain job's key (agent::build:
    `TERRAIN_V`, `terrain_pack::NORTH_PIN`, the basemap's content name), so a rebuild with the same
    key gives identical packs. A new pass makes a new basemap, whose content name changes every
    terrain piece's and assembly's key: the terrain is made again after each pass (every piece, the
    assemblies and the root, about 2 h), and the steps after it again where its tiles' bytes changed (slope everywhere; units and
    peaks where a tile they read changed). GLO-30's tiles don't change (the bucket's of May 2022);
    a tile the coverage newly wants is fetched into the store by the job.
  - **Repair** (`roadcore::grid::repair_terrain`, README "Terrain repair"): one pass that takes what
    is broken or undefined and nothing else. Voids are filled; a tower or a pit is a blob of the
    tile's component tree, taken whole and weighed against the ground it meets, as the map shows
    it: broken when it stands out more than 100 m, steeper over its footprint than terrain can be
    and towering over the ground around it; small spikes go too when two of three hold: walled, on
    flat ground, beside a blob taken or a ringing's pit under the sea; beside nothing, only one
    steeper than 63° over its width (buttes, plugs and islets aren't).
    Each stage judges the tile with what was found filled in, until one finds nothing in AWS's
    values and then in the tile as stored, so its own output has nothing left to repair
    (`terrain --scan` checked it over the coverage: §10, phase 7).
    It reads AWS's values, bathymetry and all, so stored tiles (at sea level) are never inputs.
  - **Seam spikes and walled patches** (`roadcore::grid::seam_spikes`, `walled_patches`, in
    `repair_terrain_with`, after the blobs' rules, in rounds until one changes nothing): where AWS's
    sources meet, a missing-data marker interpolated in leaves a tower 300 m and more out of the
    pixel beside it next to a pit to sea level (Maryland's 880 m tower at z9, Casco Bay's ±700 m),
    or a tower alone on the shore 1 km out of the pixels beside it (Yakutat's 6,097 m), or a needle
    2.5 pixel widths and 300 m out of every pixel beside it: the cluster is clamped into the middle
    half of the ground around it. A region of a z10–12 tile walled all
    round by sharp steps of one height, standing up out of ground that isn't water, that AWS's z9
    tile over it doesn't show (a patch of a source with another datum, or a coarse fill standing
    up off the shore) is moved down by its step. AWS's tiles alone. The whole repair runs once more
    on each tile as stored, after GLO-30 and the water (`terrain_pack::repair_stored`), so the
    stored tile is one it changes nothing in.
  - **North of 60°N, GLO-30** (crate::terrain_north): AWS mixes an ellipsoidal source (ArcticDEM's,
    most likely) with sea-level ones there, so lakes and patches stand the geoid's height (10–50 m)
    off the land around them (Kivalliq's lakes 47 m up, Ellesmere's terraces 12.4 m), its sea
    surface is 9 to 20 m up, and voids its coarse layer fills at sea level (Hans Island's top). Each
    terrain tile of z9 and finer (z8 and coarser are made from them) is resampled from GLO-30 (EGM2008 heights, 30 m, its water flattened; bilinear
    between its pixel centres, the mean of up to 6 × 6 samples over a wider pixel) and blended in by
    latitude, a smoothstep from 59.5°N (AWS's) to 60°N (GLO-30's): the band lies south of 60°,
    where AWS's sources are sea-level ones too, so it shows no seam. Where GLO-30 was filled from an
    ancillary DEM (its filling mask, 3 and up), AWS's repaired tile is taken, moved onto GLO-30's
    datum by the median difference over the tile's other pixels. GLO-30 is a surface model: the
    boreal forest reads 1.4–2.7 m over FABDEM's bare earth at the median, 4–8 m at p90, as soft
    patches; FABDEM draws blocky stair-steps over flat ground instead and stops at 80°N, so GLO-30
    it is (8 October). Licence: the Copernicus notice, in the map's credits.
  - **Water flattened** (crate::terrain_water): from the basemap's `water` layer at the tile's zoom
    (z6–12), rasterized at 4 × 4 samples a pixel. The sea (the pinned water polygons) goes to 0;
    each lake, pond, reservoir or dock to its level, the 10th percentile of its shore's dry pixels
    (the lowest of the shore is the outlet's level; the very lowest are pits and the next lake), or
    its own level where its source already flattened it and it's no higher (a forested shore would
    raise it); rivers (they slope), intermittent water and pools are left. One level a lake, by its
    OSM id, from all the tiles made together (a piece's levels, finest first; then its z3 pack's,
    starting from its pieces' levels, the first piece's, by column then row, for a lake two have);
    a lake across two z6 tiles may take a metre or two apart in each (§10). A pixel partly water is
    blended by its shares. AWS fills water its detailed source lacks from a coarse one whose cells
    mix the hills in (57 % of the sea off Yakutat above 20 m): that goes.
  - **Below zero:** values are clamped to 0, after the water. Planned: polders and depressions
    keeping their depth (the sea is the water polygons' now; land below zero is still clamped).
  - Deterministic: reruns give identical packs.
- **Slope:** z11 and coarser are stored, from transient z12 Horn slope. The server makes z12 on
  demand with the same encoder and an LRU (56 % of the full archive). Two steps
  (`pipeline::slope_pack`), as terrain's: **slope**, a piece per z6 tile near the coverage (its z9–11,
  its hi pack, and its mid, `work/slope-mid/6-x-y`: its z6–8 tiles), reading the terrain packs:
  Horn's method reads a pixel's border from the tiles west, east, north and south of each tile at
  its zoom (a missing one is its nearest ancestor's, up to eight levels up), so a piece's edge tiles
  read its neighbours' edge strips, in other areas' packs and the root too; and **slope-lo**, an
  assembly per z3 tile (its z6–8 tiles from its pieces' mids, or the lo pack's for a piece current
  without one; the area's other z6 tiles as the lo pack has them; z5–z3 from them). Together the
  area's whole run's packs, byte for byte (`slope_pack` tests: two areas whose border pieces read
  each other's terrain).
- **Worldwide z8 terrain** (`sources/terrain-z8-v3`, once, not served): every z8 tile, repaired, with
  each tile's maximum (AWS's tiles alone: not GLO-30 nor the water, §10). Peaks read it, so their prominence and isolation don't depend on coverage.
- **Grids (z11):** land cover, canopy and cover, for analysis only (not served).
  - Each unit's job makes the grid tiles its packs lack: `landcover --only`, and the scenic canopy
    step.
  - It uploads them as its own z6 tile's `grid-*` hi packs. They aren't in the units' keys: a grid
    read from its pack or made afresh is the same (from fixed datasets: WorldCover, Meta's canopy
    squares), and a unit writing its tile's would otherwise make it and its neighbours stale.
- **Trees** (cover, height, leaf type), zoom 4–12, clipped to the coverage (`pipeline::treepacks`,
  which runs the `trees` program, `pipeline::trees`), made for a region before it's published (§8,
  Order: once it's done, or earlier while the units wait for the pass's worldwide jobs) in two
  steps:
  - **trees**, a piece per z6 tile the coverage meets (`trees --z6`; a helper takes them too): its
    blocks' zoom 9–12 tiles, its hi packs, and its mid (`work/trees-mid/6-x-y`, a sectioned file:
    its blocks' zoom-8 tiles and zoom-8 values, which the tiles can't give back, rounded to whole
    steps; never served). A z6 tile the coverage has left drops its hi packs and mid.
  - **trees-lo**, an assembly per z3 tile with a piece (`trees --assemble-lo`, the build Mac's, in
    seconds): its zoom 8–4 from its pieces' mids, its lo packs. One with lo packs and no piece drops
    them.

  Together they make the same packs, byte for byte, as a z3 tile's whole run (`trees --z3`, which
  `scenic-build trees 3/x/y` runs by hand), from Meta's canopy squares (kept on the NAS,
  `sources/canopy/`, and copied into the agent's cache, where the units read them too; a square
  both want is downloaded once, under a `<file>.lock` in the store, the other waiting for it)
  and the leaf-type squares on the NAS (`sources/trees/leaf/`), each made whole once by
  `dem/leaftype.py` (the EEA's every chunk, a chunk without EEA data costing one small request;
  NALCMS's GeoTIFF kept beside them) and tagged complete.
  - The program is `dem/trees.py --z3` in Rust: the same tiles, to the pixel, in another lossless
    WebP encoder's bytes (`pipeline::webp`: about 1 % smaller than libwebp's on real tiles).
    `tools/check/trees-same.py` compares the two (2026-10-05: ten blocks of every kind, and the
    whole of 3/4/2 and 3/7/3, every tile the same); `SCENIC_TREES_PY=1` has a z3 tile's whole run
    use trees.py. trees.py made all the build's tree packs (2026-10-05, before the program took its
    place): the pieces make them again, the same pixels in the program's bytes (§8, A new key
    scheme).
  - A block runs on its own too (`trees --block`: a zoom-8 block's tiles and its zoom-8 values,
    the same bytes natively, on any thread count, and in WebAssembly, its squares read from a
    folder or through `pipeline::fetch`), and `trees --assemble` makes a z3 tile's archives from
    blocks. A row of a piece's blocks (one z8 row, up to four) runs together too (`trees
    --blocks`: each canopy strip, a row of its square's whole width, read and decoded once for the
    row; each block the same bytes as alone), and is a task (`treeblock`, docs/workers.md §3): a
    piece's run with the job's coordinator offers some of its last rows to pages and helpers,
    reading the squares where they lie, and takes their blocks into the piece in their turn, the
    same bytes (`tools/check/treeblock-same.mjs`: four real rows, natively and as WebAssembly, and
    the NAS's packs).
- **Area overlays:** see `docs/phase5.md`. The `overlays` job runs after marks, because it needs the
  World Heritage dots' ids.
- **3D buildings (phase 7, under way):** `docs/buildings3d.md`. Every building in the coverage, from
  the Overture release the roadside buildings read: its height measured or from its floors, else
  estimated from Microsoft's figure, its neighbours, GHSL or its size and kind; tiles z12–14 per z6
  tile. Built (B1): the steps `bldprep` and `bldtiles` (`pipeline::bld`, dem/bldprep.py), run by
  hand; the catalog's layer `buildings`, the server's `/tiles/buildings`, the map's layer and its
  settings. Built (B2, published 2026-10-08): the agent runs them for every tile, their sources'
  fetch a network job (§8, Order: the fourth chain), shared with helpers; the mirror's group, the
  iPad's budget, the credits. Built (B3, 2026-10-08, not yet published): a tile's z8 areas offered
  as tasks to pages and helpers (`bldtile`, the same bytes in WebAssembly: workers.md §3); on the
  map, walls on the terrain under each corner, the globe's 3D positions without float32's noise
  (the saw at the buildings' feet), fog, bridges and elevated rail drawn after the buildings, the
  camera kept above roofs, colour by height on the shared colour scale, heritage sites in their
  colour. National heights (PLATEAU, BD TOPO) later.

The server builds missing deeper terrain and slope tiles from their ancestors.

### Water (per pass, worldwide)

The basemap's water is Natural Earth's below z6, and from z6 simplified, with the small polygons
left out at each zoom (the sea's islands under 1 px² of a 256-px tile to z13; lakes, their islands
and the other inland water under 4 px² below z12). Drawn as vector fills, MapLibre's anti-aliasing
outline also pushes every shore out by 0.3 px. So zoomed out the shores were wrong, small islands
and lakes went missing, and a district of ponds disappeared. The water layer (`pipeline::water`)
gives each pixel its exact share of water at every zoom, sea and inland water apart. The map draws
it as a raster at the screen's density, so a tilted view, 3D terrain and the globe treat it as
they treat any raster.
- **The water** is the basemap's own at its deepest zoom, z14: simplified by 0.0625 px of a 256-px
  z14 tile (one unit of its 4,096, about 0.6 m at the equator), nothing over 1/256 px² left out. The sea is OpenMapTiles' class `ocean`
  (the pinned water polygons), the rest inland. Water in tunnels is left out, as the map leaves
  it out.
- **Coverage is exact:** a pixel gets the area of that water inside its square
  (`pipeline::watercov::Raster`: each edge adds its area to the pixels it crosses, as font
  renderers do). A pond a hundredth of a pixel across counts a hundredth.
- **Tiles:** 512 px, two channels (the sea's share, the inland water's). The bytes also say
  whether a pixel holds anything but sea, and any land, however little (`water::bytes`): a sea
  byte of 255 only for sea throughout, the two summing to 255 or more only for water throughout.
  So an island a millionth of a pixel is still land zoomed out, for the coastal shading (each
  coarser pixel holds what any of its four does; an island too small to round to a byte costs the
  water a 255th of its pixel).
  - **Stored for z0–9:** each z10 tile is drawn from its 256 z14 tiles, and each coarser tile is the
    mean of its four children's pixels, which is exact. A tile is stored only where it isn't one
    value throughout, as an 8-bit grey-and-alpha PNG (`layers/water/{root,lo,hi}`; docs/formats.md).
  - **Deeper zooms are drawn by the server when asked** (`crates/server/src/water.rs`), from the z14
    tiles under the tile (z10: 256, z13: 4) or the one over it (z15–18).
  - **Worldwide** (2026-09-28's basemap, 2026-10-08, on the build Mac, reading the basemap from the
    NAS): 25.4 million z14 tiles read, 4.5 min, 7.7 GB of memory at most. 75,683 tiles, 1.38 GB:
    - z0–8: 0.56 GB, the root and lo packs;
    - z9: 0.82 GB, 52,398 tiles, hi packs.
- **Served** at `/tiles/water/{z}/{x}/{y}`:
  - `?c=<sea>,<lake>[,<land>]` gives a PNG in those colours, alpha the water's share (the two
    shares summed, an overlap at most whole); with the land's colour, the alpha that mixes water
    and land as light mixes (`alpha_for`: the map blends the stored values, which gives the darker
    of the two more than its share, so half a pixel of dark water on light land looked three
    quarters water);
  - `?raw=1` gives the shares themselves (red the sea's, green the inland water's), for the coastal
    shading;
  - a tile not stored takes its stored ancestor's value over it;
  - a stored zoom whose pack can't be read (offline, and not downloaded) is drawn from the
    basemap at z9 (1,024 z14 tiles);
  - drawn tiles are kept (192, about 0.5 MB each); at most 4 are drawn at once, on 4 threads of
    their own (not the queries' pool), a draw the map stopped waiting for still kept, and stored
    tiles never wait for draws; the open sea's z14 tile, shared by thousands, is drawn once (at
    most 4,096 such kept, by archive and offset);
  - a tile drawn here reading the basemap from the NAS (2026-10-08, this Mac): z10 70–200 ms,
    z11 25–185 ms, z12 20–45 ms, z13 30–250 ms (the first in an area), z14–16 2–3 ms; one kept,
    2 ms.
- **Drawn** (`web/src/basemap.ts`) as a raster source of 256-CSS-px tiles to z18 (a texel a device
  pixel at 2×), linear resampling, no cross-fade (with 3D terrain a draped texture drawn mid-fade
  would keep it). Under the Water switch.
  - Never stretched: MapLibre is told the tiles are 256/√2 CSS px (`WATER_TILE_SIZE`), so it takes
    the finer level whenever the zoom isn't whole and a tile is drawn at 0.5–1× its texels (told
    256, it rounded, and just before each switch stretched the coarser level to 1.41×, softer than
    1:1: the owner saw it, 8 Oct). The coastal shading's tiles take the same levels.
  - Shrunk, the tiles are sampled between their two nearest mipmap levels (trilinear, a MapLibre
    patch in `web/vite.config.ts`): from the nearer level alone, the small lakes' look jumped at
    each half zoom.
  - The colours are in the tiles' URL (Settings → Map → Water, lakes a shade lighter), with the
    land's (the map's background, which no setting changes): a new colour asks for the tiles
    again, with no rebuild. The water and the land mix as light mixes over the background; over
    hill-shading, whose light differs pixel by pixel, the mix is the background's (in the default
    colours at most 3.7 L* off on the brightest lit slopes at a partly covered pixel, against 2.9
    with the plain share; 0.3 against 0.6 in the deepest shadow).
  - The coastal shading (`web/src/coast.ts`, at every zoom) measures its shore from the same
    tiles' shares (the sea's alone, or the inland water's too with Lakes & rivers), at their own
    density (2 texels a CSS px: its tiles are the water layer's tiles): every pixel holding any land
    is a shore, placed within the pixel by its share (`web/src/coastdist.ts`), so an island smaller
    than a pixel keeps its shore and its glow, as full detail would give them.
    - The distance is exact within 192 of a tile's pixels. Beyond that, land is "deep" (the ramp
      reaches only 0.7 CSS px into it) and water "far", except toward the poles. There a tile's
      pixels hold cos φ as many metres, so past 60° the water beyond comes from the tile's
      ancestor k levels up, enough that the window holds at least half the equator's metres
      (blended over its last quarter). A worker keeps the 12 most recent coarse tiles.
    - Stored in metres, MapLibre's 'custom' raster-DEM encoding (R·6553.6 + G·25.6 + B·0.1 −
      100 000; past 400 km 1/64 as steep): from −100 km of land to about 75 000 km of water, past
      any ramp's ends at every zoom. The ramp is in metres for the view centre's scale: on the
      globe a metre is as wide across the view as the sphere's foreshortening allows (at 84–85°
      within 1–5 % of the centre's), on flat Mercator the band widens toward the poles with the
      ground.
  - Without the layer in the catalog (until the agent first builds it), the basemap's water
    polygons are drawn as before.
  - Masking other layers by the water (the depth layers planned, any water overlay) takes the
    shares (`?raw=1`): the server multiplies a layer's tiles by them, or a layer samples both
    textures.
  - The water has no hover or click of its own. Its names are the labels' (the label tiles'
    `water`), which the change doesn't touch.
- **Checked** by the shoreline check (`tools/coastcheck`): the map in an eval mode (land white,
  water black, nothing else) against the same view drawn from the full detail (the water polygons
  and the pass's `water` set), pixel by pixel, over 137 views: zooms 2–16, pitches to 80°, 3D
  terrain, the globe and flat Mercator, nine places.
  - Visible difference (2026-10-08, with the trilinear mipmaps): 0.03 % of the pixels, against
    7.7 % with the small islands and lakes' dots of before; tilted 0.01 % against 13.1 %, z2–5
    0.02 % against 9.4 %.
  - Islands and lakes missing, extra or misplaced: 5,386 against 467,520. The shore's mean shift:
    0.00–0.02 px against +0.10 to +0.43 px.
  - Its README has the metric and the views.
- **Checked as the owner sees it** (`tools/coastcheck --screen`, 2026-10-08): the screen (the water,
  its coastal shading and the land, in the app's colours and in black and white) against the same
  view at full detail rendered and then shrunk in linear light, 70 views where small water is
  densest (northern Quebec, Hudson Bay's Belcher Islands, Saimaa, Maine, the Stockholm
  archipelago), flat at z4.4/4.6 … 8.4/8.6 either side of each half-zoom tile switch and tilted
  60°, the stored tiles of those places built with the bytes that keep any land.
  - Visibly different (3 L* in the app's colours): 0.15 % of the pixels, against 3.29 % with the
    shading measured from the pixels over half land at a texel a CSS px (Stockholm 9.1 %), the
    blend of the stored values and the nearer mipmap level; in black and white 0.22 % against
    8.26 %.
  - The jump between z.4 and z.6 beyond what the zoom explains: 0.04 % of the pixels in the app's
    colours against 2.23 % (Stockholm z8 8.6 %), 0.45 % against 0.82 % in black and white.
  - What each part did, on ten of the views: the shading's every-land shore 2.35 → 0.41 %, its 2
    texels a CSS px → 0.08 %; mixing as light, in black and white 10.3 → 0.19 % (in the app's
    dark colours under 0.05 % either way); trilinear mipmaps, the black-and-white jump 0.80 →
    0.50 %; the bytes' any land, Saimaa at z5.4 0.25 → 0.15 %. Always taking the finer tile level
    as well (never magnified) took the black-and-white jump to 0.01 %, at the cost of the water's
    tiles in view (12 → 30); first left out as changing nothing in the app's colours, then done
    (8 Oct) for sharpness, as above.
- **Checked toward the poles** (`--screen`, 2026-10-08, the reference's shading measured the same
  way, its water from this Mac's water tiles as a stand-in for the build Mac's store): 26 views,
  the whole globe at each pole and with a pole near the limb, Antarctica, the Arctic Ocean,
  Svalbard, East Greenland and the Antarctic Peninsula at z2–6.
  - Visibly different in the app's colours: 0.02 % of the pixels. Before, 25.5 %: capped in tile
    pixels and stored in metres, deep land and far water became a few metres toward the poles,
    land lit as the shoreline's edge and water as glow, the same at every longitude, stepping
    into rings where the globe's tile levels change.
  - The same caps lit land elsewhere: a strip at whole zooms and tilted views' near ground
    (since the finer level was always taken), and every continent on the whole globe
    (Terrain-RGB's −10 km floor). On the fade views 3.64 → 0.05 % (jump 0.24 → 0.003 %), in
    black and white 0.66 → 0.30 %. The 127 views of the coverage check are unchanged
    (0.006 %).
- **Cost on the map** (this Mac's Chrome, 1,180 × 820 CSS px at 2×, 2026-10-08): the densest
  views' water takes 24–55 MB of textures (17–39 tiles), against 10–22 MB of the polygons and dots
  of before. The frames' GPU and CPU times are the same within the runs' spread (GPU 2.6–8.5 ms a
  frame at rest against 2.5–8.2).
  - The coastal shading at 2 texels a CSS px (2026-10-08): about 3 times the tiles (9–20 in an
    800 × 600 view, against 4–6 at a texel), each 40–70 ms on its two workers and about 2 MB of
    texture and elevations. Its share tiles are the water layer's own, so it adds no server draws,
    and a pan's shading is in sooner than before: after a pan of half the view onto new ground
    (Stockholm z7.6, Maine z9.6, Hudson Bay z5.6; this Mac, its test server reading the NAS) the
    last shaded tile came 0.1–0.9 s after in a map of 488 × 536 CSS px (0.4–1.2 s before) and
    0.4–3.2 s in one of 1,488 × 1,036 (1.2–3.9 s).
  - Toward the poles (2026-10-08): the far field from coarser tiles makes a pan of eight steps
    over Svalbard at z5 3.3–3.5 s against 2.1–2.4 s, over Saimaa at z6 (61.75°) 2.2–2.5 s against
    1.7–2.0, Antarctica at z2 about the same; within 60° nothing changes. The coarse tiles kept
    take up to 12 MB a worker.

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
- Elevation smoothing uses a local continuation rule (`Net.cont`); it runs in base(U).

### Per unit: base(U)

The unit job runs the unit's programs on a unit folder, wiped at each run:
1. **extract:** on U's piece, U's ways that touch the coverage. Rail tracks without
   a route relation are kept by type.
2. **Elevations:** `elev` (`pipeline::dem`), DEMs by location, on U's slice of the per-vertex DEM
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
  (`dem/labels.py`, with each thing's own English, kana reading, OSM languages and OSM object:
  §7).
- **Water** (per pass, worldwide): above.
- **The languages spoken where** (per pass, worldwide: the `spoken` job): `global/spoken`, from the
  pass's outlines (§7).
- **Rail stops:** from the rail set, for the built units' tiles + 20 km.
- **Ferries:** worldwide, from the ferries set and `inputs/ferries/freq`.
- **Rail service:** trains a day on the coverage's rail ways (below).
- **Landmarks:** candidates and peaks per unit, then Wikidata facts and pageviews, marks, and
  overlays (`docs/phase5.md`).
- **The to-do lists** (`names-todo`, after each catalog the map serves): names to translate and
  descriptions to write (§7).

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
- **The rail sources** (`sources/rail/`, docs/formats.md): the jobs add the checks, zips and days;
  the catalogue and the MTR's lines are put there by hand (Hand-made inputs, below), and the chain
  waits for the catalogue.
- **Planned:**
  - Japan's ODPT and Taiwan's TDX feeds, behind the keys `inputs/keys.env` names for them
    (`ODPT_KEY`; `TDX_CLIENT_ID`, `TDX_CLIENT_SECRET`), each a keyed feed in `dem/railfeeds.py`
    once there are values to fetch with. ODPT's licence allows no redistribution of its raw feeds;
    TDX asks for a credit line (`pipeline::rules::CREDITS`);
  - the catalogue and the timetables fetched again every ~6 months (§8).

### Hand-made inputs

What no job makes, kept on the NAS with how it was made, so it can be made again. The scripts run in
the repository's `dem/` (`uv run python <script>`); `scenic-build` takes `--root <NAS project
folder> --scratch <local dir>` as for every step.

- **The heritage registers' snapshot** (`sources/registers/legacy`, a tar.zst; the heritage-sites
  and heritage jobs read it): a folder as `dem/heritage.py` reads it, without `osm/` (the pass's
  sets replace it). In it: the registers downloaded by hand (`fhd.xlsx`, Parks Canada's Directory of
  Federal Heritage Designations, open data; the provinces' and countries' registers under `qc/`,
  `on/`, `ns/`, `nb/`, `uk/`, `ie/`, `gg/`, `fr/`, `es/`, `pt/`, `ad/`, `jp/`, `tw/`, `hk/`,
  `sg/`, UNESCO's under `unesco/` and `whc.xml`), what heritage.py and the chain cached (`wd-*.csv`,
  `wd/`, `crhp/`), the descriptions written then (`desc/`), and what two scripts make from them:
  - `federal.py --dir <folder>`: `federal.json`, the federal designations located. It reads
    `fhd.xlsx`, `wd-canada.csv`, and the provinces and named places as GeoJSON lines in
    `<folder>/osm/`: `osmium tags-filter <planet> r/admin_level=4 -o prov.osm.pbf` then `osmium
    export prov.osm.pbf -f geojsonseq --geometry-types=polygon -o osm/prov.geojsonseq`, and the
    pass's `named` set exported as `osm/named.geojsonseq`.
  - `crhp.py --dir <folder>`: `crhp.json`, the Canadian Register of Historic Places' provincial and
    municipal places (its pages cached in `<folder>/crhp/`).
  - Then `scenic-build registers-import --from <folder>` puts it in as `sources/registers/legacy`;
    the heritage jobs' keys follow it.
- **Its seeds** (`sources/registers/legacy-seeds`): park facts (`areas/wikidata.json`), the
  pageview months the chain counted before the items job's cache (`pageviews/months/`), and the
  names table the heritage layers' English comes from (`names/english.json`, read by
  `dem/names.py` `english_at`). Nothing makes them again (§10).
- **Ferry timetables** (`inputs/ferries/freq/`, which the ferries job reads):
  - `gtfs-<feed>.json`, sailings counted from operators' GTFS feeds: `gtfs.py --feeds
    inputs/ferries/gtfs-feeds.json --lines <lines.json> --cache inputs/ferries/gtfs --out
    inputs/ferries/freq` (`--refresh` asks for newer zips). `gtfs-feeds.json` is the verified feed
    list (how it was found: `gtfs-feeds.md`, `operators.txt`); `gtfs/` the zips today's counts came
    from; `lines.json` the lines `ferries.py` writes into its `--src` folder (the ferries job's
    work folder: its osmium exports of the pass's ferries set, as `pipeline::ovconv::ferries_job`
    makes them).
  - `timetables-*.json`, sailings looked up by hand from operators' published timetables, each with
    its page: the researchers' brief, batches and not-found lists are in
    `inputs/ferries/research/` (`PROMPT.md`).
- **The rail catalogue** (`sources/rail/catalogue`): the Mobility Database's `feeds_v2.csv`
  (files.mobilitydatabase.org), downloaded once: `scenic-build put sources/rail/catalogue csv
  feeds_v2.csv`.
- **The MTR's lines** (`sources/rail/mtr`, `mtr.json`: researched by hand from MTR's published
  frequencies) and their stop pairs (`sources/rail/mtr-pairs`): `mtrpairs.py --mtr mtr.json
  --stations hk-stations.geojsonseq --out pairs-mtr.bin`, the stations from the pass's filtered
  planet (`osmium extract -b 113.8,22.1,114.5,22.6`, then `osmium tags-filter … n/railway=station,halt,stop,tram_stop
  n/public_transport=station w/railway=station` and `osmium export … -f geojsonseq`); then
  `scenic-build put sources/rail/mtr-pairs bin pairs-mtr.bin` (and `mtr.json` as `sources/rail/mtr`).
  The rail job's key follows them.
- **Translations' method** (§7): `translations/0-converted/conversion-log.txt`, how the earlier
  translation work became the tables, and the scripts that sized the work (`inputs/names/analysis/`:
  `latinwords.py`, `namecount.py`).
- **Descriptions' method** (§7): the writers' briefs (`inputs/descriptions/briefs-2026-09/`:
  `WRITERS.md`, `FIXERS.md`, `RESEARCH.md`); the lists to write and today's brief are the build
  Mac's (`descriptions/todo/`, tools/names/descriptions-todo.md).
- **Fonts** (`app/fonts/` on the NAS, `data/fonts` in a checkout): MapLibre's glyph ranges of three
  Noto Sans styles, `scripts/fonts.sh <folder>`.

### Job keys

A job's key is its step version plus what it reads, mostly by content name. The ones that cascade:
- **terrain (a piece, per z6 tile):** the coverage within 20 km of its tile (which decides its
  tiles), GLO-30 (`NORTH_PIN`) and the basemap its water comes from, by content name;
- **terrain-lo (an assembly, per z3 tile):** its pieces' mids by content ("-" for a piece without
  one: it can't be assembled until each has, but its key with every mid "-" is what the records of
  an area's whole run are read as: §8, A new key scheme) and the basemap; `TERRAIN_LO_V`;
- **slope (a piece, per z6 tile):** its terrain hi pack (which tiles it works out) and every
  terrain tile it can read, by content, from the packs' indexes (`agent::build::slope_piece_reads`):
  each tile it works out from its terrain (a z12 one, or one its children don't all cover) and the
  tiles west, east, north and south of it, each resolved as the job resolves it. A change in a
  neighbour's interior changes nothing here, one along its edge does;
- **slope-lo (an assembly, per z3 tile):** its pieces' mids by content and the area's terrain lo
  pack; `SLOPE_LO_V`;
- **trees (a tree cover piece, per z6 tile):** the coverage inside the tile ("none" once it has left
  a tile with tree packs or a mid: its run drops them). Its version, `TREES_V`, changes with any
  change to the bytes a piece writes, its hi packs' or its mid's, pixels or not: a piece made again
  as it is, its mid backfilled, must come out as its key made it (§8, Order), and one that never
  could would hold its z3 tile's assembly back;
- **trees-lo (an assembly, per z3 tile):** its pieces' mids by content ("-" for a piece without
  one: it can't be assembled until each has), so a piece made again to the same bytes changes
  nothing above it ("none" for a z3 tile with lo packs and no piece); its version, `TREES_LO_V`,
  changes likewise with the bytes of its lo packs;
- **heritage-sites:** the pass, its areas set, the registers snapshot, the coverage;
- **unit:** its piece, the pass's road values, the coverage as its ways meet it (inside its tile +
  20 km, and whether each long way touches it), the versions of the location rules where its ways
  go, its heritage slices, the roadside buildings' index, Taiwan's MOI DTM files where its ways meet
  Taiwan, and the terrain tiles it reads, by their contents (`agent::build::unit_terrain`: each
  tile's XXH3 from its pack's index, no tile read): of what it stages (terrain z0–12 over its tile
  + 30 km), every z11 tile (its grid: canopy, views, flags) and, at the points of the ways it owns
  (its owned box, and along its long ways' segments: `prep`'s drape at every vertex and its
  samples' ground), the z12 tile or the finest staged tile above it, down to z4. A tile staged but
  missing counts as such, so one appearing changes the key. A change elsewhere within the 30 km
  (the far side of a neighbouring z6 tile, a coast none of its ways reach) rebuilds nothing; one in
  a z5 tile under a long ferry of its does. Not the grids' packs (Global-source layers, Grids). A
  unit whose terrain can't be worked out (a terrain pack's index unreadable: the file missing or
  damaged on the NAS) has no key: it isn't built, nor counted as built, the status says so ("the
  terrain's indexes can't be read now for N of them", with the pack and why), and the read is tried
  again each time the agent plans. A pack that stays unreadable holds those units, and their
  regions' publishing, until it's made again: move the damaged file aside on the NAS (a
  content-named file is never written over), then run its area's terrain on the build Mac
  (`scenic-build terrain 3/x/y --root <the NAS project folder>`), which writes it again under its
  name;
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
  pairs, the pass's rail set and the coverage;
- **water:** its version and the pass's basemap, by content name;
- **bld-fetch** (the 3D buildings' sources, network): its version, the pinned Overture release and
  the whole coverage. Not the footers it reads and writes (`footers.json.gz`): a release's files
  never change, so the release names them, and a key on what it writes would run it again for its
  own sake;
- **bldprep (per z6 tile):** its version (`BLDPREP_V`), the release, and what it reads: each
  downloaded file with a row group meeting the tile (a parts file's: the tile grown by 0.02°), by
  name and ETag, with those row groups' indexes, and each GHSL tile meeting it by name and size,
  from the sources' indexes on the NAS (`crate::bld::sources`, read as `inputs/` is: by digest, in
  the plan's inputs, read again only when an index changes). A file fetched later (the coverage
  grew) changes the keys of the tiles it meets;
- **bldtiles (per z6 tile):** its version (`BUILDINGS_V`: the fill's rules, fits and defaults, and
  the tiles), the normalized files of the tile and its 8 neighbours by content name ("-" none), and
  the coverage's shapes over the tile grown by 1 km in the recipes' order with their countries
  (`Coverage::shapes_key`: a building's shape sets its country's fits).

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

**A thing's English, in order** (`names::display`):
1. **Its own, from a source about that thing.** It belongs to that thing alone, wins over its
   name's translation, and is never copied to other things with the same name. A citable source is
   shown even where it's wrong ("Leclerc tank" for one "Monument aux Morts"): the user prefers that
   to overwriting it. The sources, as the build carries them (`names::own`):
   - OSM's `name:en` (the basemap's `name:en`, not its `name_en`, which OpenMapTiles fills with the
     name where there's none; our tiles' and records' `en`);
   - its romanised name (`name:ja-Latn`, `name:ja_rm`, `name:zh-Latn-pinyin`…);
   - its kana reading (`name:ja-Hira`, `name:ja_kana`; the labels' `kana`), romanised by rule:
     modified Hepburn without macrons (`names::romaji`);
   - a heritage register's or UNESCO's English, or its English Wikipedia article's title, where the
     heritage and landmark jobs put it in a record's `en`.

   An "own English" that is the name itself (`same_name`) is none.
2. **Its name's translation:** the line for its name, kind and language. The lookup tries first the
   languages OSM gives the name (each `name:<language>` equal to `name`: a `name:br` equal to
   `name` makes it Breton; our labels' `l`), those spoken there first in the order they're spoken,
   then each language spoken where the thing is, in order.
   A name OSM tags Chinese is read in Cantonese where Cantonese is spoken and Mandarin isn't (Hong
   Kong, Macau).
3. **Else none:** the name alone, and the name goes on the to-do list.

**Translations are keyed by name, kind and language, the same way in every script.**
- **Kind:** road, settlement (city, town, village, hamlet, suburb, quarter, neighbourhood, isolated
  dwelling) or other. The same words can be a hamlet that keeps its name and a mill to translate
  ("Moulin"). A name is looked up in its own kind only (the converted lines hold for the other
  table's kinds too where that table had none: below).
  - Labels: a place by its class, states, water and parks other; the basemap: its `place` layer by
    class, `transportation_name` road, the rest other; roads (drives, climbs, way details) road;
    rail lines, ferries, stations, landmarks, area overlays, summits: other.
- **Language:** the language the name is read in (ISO 639, its first subtag: `zh_Hant` is `zh`).
  - A line holds for one language or several. A name that could be in any of the local languages,
    with the same English in each, is filed under all of them, so nothing claims a language it can't
    know.
  - 中山 has a Chinese line (Zhongshan, as Taiwan reads it), a Japanese one (Nakayama) and a
    Cantonese one (Chung Shan). "Lac Bleu" has one French line, used in France and Quebec alike.
- **The languages describe the name, not a region.** So a line serves every place where they're
  spoken, and stays valid when a region gains a language.
- **The cost:** the same name in two languages (a "Hotel Central" in Spain and another in Italy) is
  translated once in each.

**Languages spoken where a thing is** (`names::spoken`), in order:
- **Per territory:** CLDR's territory data (`crates/names/src/territory-languages.tsv`, made from
  CLDR 47's `territoryInfo.json` by `tools/names/cldr-languages.py`): the languages official or de
  facto official there, most spoken first, script subtags dropped. Languages official only in a
  region are left to the refinements.
- **Refined** (`spoken::REFINED`): Quebec French then English, New Brunswick English then French;
  Catalonia and the Balearics Catalan then Spanish, Valencia Spanish then Catalan, Galicia Galician
  then Spanish, the Basque Country and Navarre Spanish then Basque; Wales English then Welsh,
  Scotland English then Gaelic; Brittany French then Breton; Hong Kong Cantonese then English,
  Macau Cantonese then Portuguese (their names are read in Cantonese, the government's
  romanisation, not Mandarin's Pinyin).
- **Where:** the pass's outlines of ISO 3166-1 territories (Hong Kong's `CN-HK` is HK) and of the
  refined subdivisions, simplified (1 km for countries, 250 m for subdivisions), as a raster of
  1/128° cells (about 870 m at the equator), run-length coded by row, the smallest outline winning
  where they overlap (Hong Kong over China, Quebec over Canada). Beyond every territory (the high
  seas) no language is spoken: only OSM's tags lead to a line.
  - From the 2026-09-28 pass: 250 regions, 266,957 points, the raster 3.4 MB, made in 1–6 s
    (this Mac, busy); reading the outlines' records and simplified rings from the NAS took 20 s (five
    minutes while the NAS was busy).
  - **Made once per pass** by the `spoken` job (worldwide, keyed by the outlines and
    `spoken::RULES`): `global/spoken`, in the catalog and among the World download's files, so a Mac
    that has downloaded the World reads it in milliseconds, offline too.
  - **The server** reads the catalog's `global/spoken`. A catalog without it (made before the job
    first ran): the server makes the raster from the catalog's outlines on a thread of its own and
    keeps it in its home (`names/spoken-<outlines' content name>.bin`, its slashes as
    underscores), and offline uses the newest it kept.
  - **Names aren't loaded until it's there:** until then names show with their own English alone
    (as before the lines load), and `/api/catalog`'s `names.waiting` says what for. The
    `names-todo` job keeps its own copy in its scratch.
- It's the only place location enters: it sets the lookup order and which to-do list a name goes on.

**Files:** `translations/**/*.jsonl` (not `todo/`), one line per translation, wherever the file is:
`{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu", "sub": "Blue Lake",
"via": "agent:haiku"}`.
- **Fields:**
  - `n`: the name exactly as in OSM.
  - `kind`: road, settlement or other, or a list of them.
  - `langs`: the languages it holds for (one, or a list).
  - `main`, `sub`: the display (`main` null or empty: the name; `sub` null: nothing under main).
    The area tables' `en` stands for a missing `sub`.
  - `via`: how it was made (free text, for the record); "todo" and "skipped" mark lines not done,
    which are left out.
- Where lines share a name, kind and language, the later file (by path) wins, and within a file
  the later line.
- Lines without `kind` or `langs` (the area tables' format) are left out and counted, a file of
  nothing else passed over (no empty table kept), with one warning for them all. When the folder
  holds those and none by language, the server says so in its log and in `/api/catalog`'s
  `names.warning`.
- A file is read once its size and modification time have held for 10 s; an unfinished last line is
  ignored. The server copies the NAS folder to this Mac (`livefolder`), checking every minute while
  the map is in use and for ten minutes after it starts: a drop shows within about a minute and a
  half, with nothing rebuilt.
- Compiled per file into one arena with an index by name (`names::table`): the converted lines
  (2.78 M, 442 MB of JSON) load in about 1.4 s into 132 MB. Lookups read them, with the spoken
  languages, through one shared `Arc` (`names::Namer`): nothing is copied per lookup.
- **Versions:** each language's is a hash of the files holding lines in it (paths, sizes,
  modification times). A tile's ETag includes the versions of the languages spoken within it (a
  tile wider than 40°, or 1,000 raster rows, takes every language's) and the raster's. A line in a
  language not spoken in a tile, which only OSM's tag on a thing there leads to, shows once the
  tile is asked for again for another reason.

**Today's tables were converted once** (`tools/names/convert-tables.py`), from the translation
work's nine area tables (`place-translations`, branch `claude/exciting-cori-x46sfk`, `out/display/`,
copied 2026-10-02 into `translations/<area>/`), into `translations/0-converted/<language>.jsonl`
(the languages' English names: the area tables' loader, which reads area codes in paths, skips
them).
- **Lines made from the name itself became lines in the new format:** names kept as they are,
  rules on their words, Taiwan's romanisation of the characters, and agents' translations (`via`
  native, rule, agent).
  - Each holds for the languages spoken in its area: jp Japanese; tw Chinese; hk Cantonese; sg
    English, Chinese, Malay, Tamil; fr French; ib Spanish, Portuguese, Catalan, Galician, Basque;
    pt Portuguese; na English and French; gb English, Welsh, Irish, Gaelic.
  - Places' lines hold for settlements and other things; roads' lines for roads; and each for the
    other table's kinds too where that table has no line for the name in the language, as the old
    lookup fell back to it (a road read the places table, anything else the roads table).
  - Then the languages the old boxes reached besides an area's own (France's held northern Spain,
    Britain's the French coast north of 49.8° N, Iberia's southern Corsica, Hong Kong's Shenzhen's
    edge): France's lines hold for Spanish too, Britain's and Iberia's for French, Hong Kong's for
    Chinese, so "Playa de Cueva" keeps "Cave Beach". Where an area's own line for the language
    disagrees, it wins (198 lines, logged).
- **Lines taken from particular things' sources were dropped:** `via: osm`, the `name:en` most
  things with the name agreed on, Japan's romanisations of OSM's kana readings among them (197,595
  lines). Each of those things shows its own English from its own tags; things with the name but no
  English of their own go on the to-do list.
- **Lines not done** (`todo`, `skipped`) were dropped: 274,756.
- **Kept:** 2,814,841 lines, which became 2,780,879 (lines with the same name, kinds, display and
  languages merged; 442 MB).
- **Names whose lines disagreed** (96 lines: places in France and in Quebec, in mainland
  Portugal and on its islands, in Britain and North America) were settled by rule, each logged
  (`translations/0-converted/conversion-log.txt`): a name kept as it is in one area
  and translated in the other is split by kind, the kept line the settlement's (Mont-Blanc, a town
  in Quebec) and the translation other things' (Mont Blanc); else the language's home area wins.
- **Planned:** removing the per-area folders from the NAS once a published server shows the
  converted lines (§10).

**The build carries each thing's own English and its language tags:**
- **Labels** (`dem/labels.py`, `LABELS_V` 2): `en` (its `name:en`, else its romanised name),
  `kana` (its kana reading, when it has no English), `l` (the languages OSM gives its name), `o`
  (its OSM object).
- **The basemap:** `name:en` and `name:fr` in the 2026-09-28 pass's; from the next pass, Planetiler
  keeps `osmpass::BASEMAP_LANGUAGES` (the coverage's languages and its neighbours', `ja-Latn`,
  `ja_rm`, `ja-Hira`, `ja_kana`, `zh-Latn-pinyin`…).
- **Roads:** their own English is each unit's `global/roaden/<u>` (OSM's `name:en`). Roads carry no
  language tags: they're looked up in the languages spoken where they are.
- **Not carried yet:** Wikidata's English labels (the landmark jobs keep the English Wikipedia
  title, `w_en`, in the popup records, not as the thing's English), roads' language tags (§10).

**To-do** (`pipeline::namestodo`, the `names-todo` job, after each catalog the map serves; 1,070 s
for catalog 15, read from the NAS on this Mac while busy, its estimate):
- **`translations/todo/<language>.jsonl`:** a name, when something in the coverage has it, no
  English of its own and no line in any language spoken there (or OSM gives it).
  - **Read:** labels (the labels layer's zoom-12 tiles in the units' z6 tiles, the labels inside
    the coverage the catalog records), roads and rail lines
    (each unit's base pack read first, then each named way placed once by its box's centre from
    the hidata's ways-here index; their own English from `global/roaden/<u>`), landmarks (the markdata). Not yet: the basemap's things
    (rivers' names), stations, ferries, the area overlays' names.
  - Names already in English aren't listed: where English is spoken, a name without another
    candidate language's signs (accents, the coverage's generic words and articles) counts as
    English; elsewhere one with more English words than another language's ("Hiraizumi – Temples,
    Gardens and Archaeological Sites…"); a name in Chinese or Japanese script with an English part (Hong
    Kong's "文武廟 Man Mo Temple Compound") carries its English. Nor are names without letters, nor
    settlements in Latin script, which keep their own name by the brief's rule (a well-known
    English one would be their own English, OSM's `name:en`).
  - **Each entry:** the name and its kind; the candidate languages: those spoken where the things
    lacking English are, narrowed by OSM's language tags and the name's script (kana Japanese, other
    CJK the CJK languages spoken there, Latin the others); how many things lack English, and one of
    them (its OSM id and position); a priority (labels: places by class and population, about 70 a
    city, 50 a village, 30 a hamlet; states 50 and up, water and parks 30 and up; landmarks 30 + 8 ×
    fame; roads by class, 45 a motorway to 20 a residential street; plus log₁₀ of the count).
  - Entries are sorted by priority. Each goes on its first candidate's list, and the answer says
    which candidates it holds for. Entries carry no other thing's sourced English.
  - **The brief:** `translations/todo/README.md` (`tools/names/translations-todo.md`): the entry
    and answer formats and the conventions (title case; settlements keep their own name unless they
    have a well-known English one; Hepburn without macrons in Japan, Hanyu Pinyin in Taiwan, the
    Hong Kong government's romanisation), from the translation work's brief. `check.py` beside it
    (`tools/names/check.py`, the translation work's checker on the new lines) checks an answer
    file.
- **`descriptions/todo/`:** `landmarks.jsonl` (the markdata's points of every kind) and
  `areas.jsonl` (the ovdata's parks and protected areas) with an English Wikipedia article or a
  register entry and no description (in `descriptions/`, or the record's own), by fame
  (landmarks: the map's fame; areas: Wikidata's sitelinks, else the log of the area), so how far
  down to go is a choice; `README.md` (`tools/names/descriptions-todo.md`) is the writers' brief: a
  noun phrase opening on the claim to fame, at most 55 words, mundane places not inflated, sources
  credited. Not yet listed: heritage areas, Indigenous lands, special areas and World Heritage
  outlines (their records hold no article or register entry).
- The lists are the NAS's alone (not the translation work's repository). Running the translators
  and writers (Haiku for names, Sonnet for descriptions, a pilot, then checks) is done on request.

**Descriptions.**
- Descriptions are `descriptions/**/*.jsonl`, lines `{"qid", "long", "src"}`, or `{"id":
  "n123"|"w123"|"r123", "long"}` for things without a Wikidata item. `"drop": true` removes one.
- They're laid over the popups of details, landmarks and area overlays when serving: matched by the
  record's `qid`, else its first `wikidata`, else its `osm`. Later file names win, and `src` is
  credited.
- Today's written descriptions are in `descriptions/heritage/`, prefixed 1–4 to keep their order.
- They arrive as translations do.

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
- **A new key scheme** (`agent::rekey`): when a step's keys change what they name, its targets'
  records are re-keyed rather than built again. One whose recorded key is what the old scheme
  computes now (it's current) is recorded under its new key when every input the new key names is
  pinned by the old key's (the same input, a function of what it named, of immutable sources, or
  of what's never been rewritten since); otherwise its record goes and it's built again. Any other
  record stays as it is: stale under the old scheme, it's stale under the new. Safe because a
  target's outputs are a function of the inputs its new key names (Determinism), and those are
  what they were when it was built. The build Mac's agent re-keys after merging hand-offs: it
  reads what that takes first (the indexes, the coverage, the reaches, the files' times) and works
  the re-keying out, and only when that changes the records does it take the build lock, as the
  merge takes it (a job saving holds it), re-key the records as they are then and write
  `state/build/jobs.json` whole. Before its first such write it keeps them as they were, in
  `state/build/jobs.pre-rekey.json` (written once, never over one there: the way back to an app
  from before the units' keys, README; one from before tree cover's pieces needs no copy: it drops
  the assemblies' records and makes the tree cover again by z3 tiles). The plan and the status
  re-key the keys they read in memory, so a dry run, or a loop that couldn't take the lock, plans
  as the records will be. A second pass finds nothing (a new key is never an old one), and a
  record of an older app's job merged later is re-keyed then (tree cover's as below).
  `scenic-build rekey-check` says what it would do, reading only. The old scheme's keys
  (`agent::rekey::v1`) are kept for those late records.
  - **The units' terrain** (§6, Job keys: they named the terrain hi packs within 30 km): their
    z9–12 tiles are pinned by the hi packs the old keys named, and z8–z6 tiles by the hi pack of a
    z6 tile near the coverage in an area whose terrain is current (a z8 tile is made from its raw
    tile and its z9 children alone). Not z5 and z4 tiles (made from z6 tiles the old keys mostly
    didn't name: the far reaches of long ways), nor z8–z6 tiles of a stale hi pack's z6 tile
    (gap 4, §10): the coverage left the z6 tile, or the hi pack is over an hour older than its
    area's lo pack by the files' times (a run writes its pieces' hi packs, then its lo pack, so an
    older one was left by an earlier run: the last made no hi tiles for the piece, and its z8–z6
    from the raw tiles alone). A unit reading those is built again, unless it has no outputs (the
    terrain doesn't decide which ways it keeps); one whose packs' times can't be read now waits
    for the next pass. The z8–z6 tiles of a z6 tile without a hi pack are pinned unchecked, on an
    assumption: that the unit was built after its area's first lo pack the build made (the
    converted legacy ones differ). `rekey-check` shows the times it rests on: on 2026-10-06 each of
    the 108 units with outputs reading an area's zoomed-out terrain was built over an hour after
    the build first made it; and the review compared each re-keyed unit's zoomed-out tiles with
    those of the lo pack it was built from (by its run's start in the agents' logs): none of
    12,054 differed. On 2026-10-06 `rekey-check` found 261 of the 284 units re-keyed (20 of them
    without outputs) and 23 to build again (12 reading z5–z4 tiles, 13 a stale hi pack's; two
    both).
  - **Terrain and slope** (§6: a z3 tile's whole run became a piece per z6 tile and an assembly per
    z3 tile, 2026-10-08), unlike the schemes before, are never re-keyed in the records: the pool's
    records change only by a job's save. Every reader reads them through one function instead
    (`agent::rekey::as_read`: the plan, the checklist, the regions' state, a job's
    `--expect-same`, `p5-check`), which derives in memory (`rekey::derive`), the same each time, a
    record a job saved always winning over a derived one: a z3 tile's terrain record current under
    the old key (its z6 tiles, the coverage within 20 km of the area, GLO-30, the basemap) is read
    as its pieces under their keys (their inputs follow from the old key's, and the area's run is
    its pieces and assembly, byte for byte) and its assembly under its key with every mid "-" (a
    mid made since makes it stale: it's made again from all its pieces' mids). A z3 tile's slope
    record current under the old key (the area's terrain lo pack and its pieces' hi packs) is read
    as its pieces whose every terrain tile read is in the area's packs; those reading another
    area's terrain or the root, which the old key didn't name (and an area's slope run could read a
    neighbour's terrain before it was made again), are made again, and the assembly after them.
    Each piece current only by being derived has no mid: its mid is made in idle time, expected the
    same (Order), and its job's record then holds. A record of the old scheme stale under it, or an
    area's whole run merged late (byte for byte what its pieces make), changes nothing derived but
    the pieces without records. `scenic-build p5-check terrain` says what it would do, reading only.
  - **Tree cover** (§6, Trees: a z3 tile's whole run, keyed on the coverage inside it, became a
    piece per z6 tile and an assembly per z3 tile): a z3 tile current under the old key pins its
    pieces' inputs (the coverage inside a z6 tile follows from the coverage inside its z3 tile: the
    edges crossing the box, and whether a point is inside), and when the trees program made its
    packs, its pieces and assembly make them again byte for byte: they're recorded under their keys
    (the assembly's naming no mids yet, "-"), and the pieces' mids are made in idle time, expected
    the same (Order). Packs trees.py made (written before the program took its place, 2026-10-06
    07:11 UTC, by their files' times: `rekey::TREES_PROGRAM_SINCE`) are the same pixels in other
    bytes, so nothing pins the pieces' bytes: the z3 tile's record goes, and its pieces and
    assembly are made again. Every z3 record goes: a stale one is made again as pieces either way,
    and one of "none" has nothing left to build. A z3 record merged after the switch (a lease
    granted before it, or an older app's, rolled back to: README) comes with its whole run's packs,
    written over those of the z3 tile's pieces and assembly made since. Current and the program's,
    it's re-keyed as above, but for a piece recorded since under another key, or with a mid and no
    record (its mid isn't of this coverage: made again); otherwise it goes with the records of the
    z3 tile's pieces and assembly, all made again. `scenic-build p5-check trees` says what it would
    do, reading only: on 2026-10-06 all 18 z3 tiles were current and trees.py's, so none is
    re-keyed and the switch makes the 380 pieces and 18 assemblies again (2.3 h by trees.py's last
    runs, less with the program: a z3 tile's run of it took 5 to 24 s on the build Mac for 3/7/2,
    3/7/3, 3/4/2 and 3/3/2, the canopy squares local), 14.6 GB of packs uploaded again, then one
    catalog. The same four areas' pieces and assemblies made the program's z3 runs' packs byte for
    byte (and every one of their 129,455 tiles has trees.py's pixels).
- **A job** is one step over a batch of stale targets: terrain's and slope's pieces 8 (a z6 tile's
  about 25 s and 10 s), their assemblies 4, lo 2, tree cover's pieces and assemblies 4, unit 6, the 3D buildings' bldprep 8 and bldtiles 16 (z6 tiles: a dense
  one's bldprep about a minute, its bldtiles about 10 s; 2 and 4 while the regions' terrain,
  slope, tree cover or units are left, so theirs come back within minutes), peaks 12, pack 16,
  pois 24, the worldwide steps all. So a failure
  or a new app costs one batch.
- **Order:** the agent starts the first job that can run, in plan order. It plans when a job could
  start (its second slot's: each minute), when one ends, and otherwise every five minutes for the
  heartbeat (planning reads the manifest, the keys, a dozen NAS folders and the terrain packs'
  indexes, which the units' keys read: each read once by its pack's content name, two range reads,
  and kept in the agent's `pack-idx/`, so after the first plan only a terrain job's new packs;
  13–64 s for the build's 529 on 2026-10-06, and a unit's terrain worked out again only when a pack
  it reads or its reach changes); other workers' hand-offs are merged each loop while it waits, every
  two minutes while a job runs.
- **Two jobs at once** (`agent::SECOND`): beside the first job, the build Mac runs a second, the
  plan's first job of these steps, in this order: the trains', the landmarks' and the 3D buildings'
  steps that mostly wait on the internet (the heritage chain, the items' facts, the rail feeds and
  trains a day, the 3D buildings' sources, the landmark points and overlays), then the candidates
  and peaks, then units and slope, then the 3D buildings' bldprep and bldtiles (they hold up
  neither the roads nor the terrain). A unit spent
  380 of its 860 s writing to the NAS and reading the caches (6/17/25, 2026-10-05): two at once build
  more. A second job:
  - never runs beside a job that runs alone (the OSM pass, the pass's worldwide jobs, GC), nor
    beside a job of the same step unless it's a shared one (its targets are held apart, as a
    helper's are), nor a reader of the raw terrain tiles beside another (terrain, peaks, the roots),
    nor the items' facts beside the heritage chain (both ask Wikidata, each paced as if alone),
    nor a bldprep beside another (each reads up to ~3 GB of the NAS's parquet a tile);
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
  less free than the job needs (30 GB; the water's 35 GB, for its tiles' archive and packs, 1.4 GB
  each worldwide, written here before they go up; an area's whole terrain run 55 GB, for its
  area's raw tiles held twice while they're packed onto the NAS, and on a run again the area's
  archives copied here and merged, those copies spared; terrain's pieces and assemblies 35 GB, a
  job's z6 tiles' or z3 tile's archives copied here and spared; the OSM pass, its own 80 GB less
  the pack cache it clears; the M1's helper, 15 GB), the local copies of what the NAS
  keeps (Meta's canopy squares, AWS's raw terrain tiles, the copies of the records' files staging
  reads, `blobs/`, and of the pageview months' indexes, `items/months/`) lose files until it has a
  sixth more (the OSM pass: what it needs), so the next jobs start without deleting again.
  - **Mid-job, safely by design** (`store::cachefile`, `dem/cachefile.py`):
    room-making, a trim, a clear and the freeing toward the target may delete any cache file no
    running job uses, at any moment, with jobs running. Every read of the caches goes through one
    accessor, which gets the file or fills it from where that cache fills from (the NAS's stores,
    else the source it fills from first) when it's missing, so a wrong deletion costs a fetch,
    never a failure. A job holds each cache file it uses with a shared `flock`, taken as it opens
    it and kept to its end (a unit job lets each area's go once the next is under way; a raw tile,
    read whole at once, only while it's read); children that open it by name (osmium, the Python
    steps, scenic-metrics) are covered, the lock being on the file. A file is deleted only under an
    exclusive lock taken without waiting, its name checked to still be that file, and unlinked
    while it's held; one in use is passed over (deleting an open file would free nothing anyway).
    Nothing in the caches is renamed over a file: a file is made under a temporary name of its
    own, held from its making (so a half-made one is never taken), and named only if the name is
    free. So a job that opened a file a deleter then took finds it has no name left once it has
    its lock, and fills it again: it never holds a name that's gone. A guard test fails on any new
    code that names a cache path outside the accessor (`tests/cache_accessor.rs`), and chaos tests
    run real steps (a terrain area, tree cover's z3 run, the DEM seed's slices, the records' copies,
    pageviews.py's lookups) while every cache file no job holds is deleted as fast as it can be:
    the outputs come out byte-identical.
    - What can't be filled again stays while it can't: the DEM seed while the NAS hasn't it whole,
      the heritage clip while the pass's filtered planet isn't there, raw tiles and pageview
      months the NAS lacks (rules below).
    - What the jobs queued and running read (`room::Hints`: a terrain run's area's archives, a
      unit's canopy squares and the copies of its packs, tree cover's squares, the pageview months
      for an items or heritage job) goes last, so it isn't fetched twice; only a hint: a stale one
      costs a fetch.
    - Not while a job an earlier agent left runs on the Mac (its programs may be an app's from
      before the locks): until it ends, nothing of the caches is deleted while it runs.
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
    (whole and flushed), or kept; a pageview month it lacks stays (pageviews.py puts it there when
    it next reads it), as do the counts from before the indexes; raw tiles it lacks are packed
    onto it first (an archive an area, none kept here), or kept (a tile at a time with a flush
    each, small files stall the NAS and every process waiting on it). The copies of its archives go
    each by its own use (a job marks
    one used when it opens it). A file that isn't whole itself (cut short, or temporary) is
    deleted, not kept.
  - The OSM pass counts those copies as room.
  - **After the build** (`room::trim`): once the build has nothing left to build (and no job an
    earlier agent left runs on the Mac, nor one that couldn't be shown stopped), its agent, at home
    (through Tailscale it would take hours), empties those copies by the same rules, once, and
    again only after a job (not a daily one) has run there since: a helper all of them, the build
    Mac all but the canopy squares, which every pass's areas read again and which never change.
    "Nothing left to build" is the build Mac's forecast's (no work, no round under way, no machine
    busy), made within ten minutes and after the last job on the Mac ended; a helper reads it in
    the build Mac's heartbeat, which must have beaten within ten minutes and show no job of the
    build Mac's running, nor one beside it. Not down to the reserve: room-making makes that much
    room before each job, so the build ends with about that free (36 GB, with 39 GB of archive
    copies and copies of the records' files, once it was done on 2026-10-06), and a trim to it
    would free little or nothing, while that Mac's downloads copy nothing past a 150 GB reserve.
    It's logged, in the agent's status (`caches.trimmed`: when, what it freed by cache, what
    stayed) and, when it freed anything or what it keeps changed, in the history (a helper's, as
    the build Mac reads it in its status).
  - **On the owner's ask** (`room::clear`): Clear the Build's Caches in a Mac's menu bar item, or
    `scenic clean` there, after a confirmation that names what goes, cache by cache, and how each
    comes back (copied from the NAS at the measured 60 MB/s, 12 through Tailscale: the base packs
    in about 20 min; the heritage clip, an hour of osmium), writes an ask in that Mac's agent's
    folder (`clear-request.json`, as a pause is asked for, signed with the Mac's name as its
    agent goes by), which the agent takes up within seconds, renaming it aside as it does (an ask
    written meanwhile waits its turn): once the build is done (jobs running keep what they use),
    it empties every
    cache a later job fills again from the NAS or makes again from it (§4, Caches), the canopy
    squares too, by the same rules (the DEM seed only while the NAS has it whole; the heritage
    jobs' planet clip only while the pass's filtered planet it's clipped from is in the NAS's
    manifest and there, else it stays, said as kept: a safety the owner accepted on 2026-10-08),
    and says what it freed (`caches.cleared`); else it says why
    not (`caches.declined`, the last clear done kept apart); the ask goes either way. The menu
    shows the item with what it would free (`caches.clearable`, and cache by cache,
    `caches.each`), disabled with why while the build has work, a job an earlier agent left runs
    there, its agent
    hasn't written its status for six minutes, or they hold nothing; "Clearing…" while the ask
    waits or is under way; and "Freed N GB" once it's done. `scenic clean` waits for the same
    answer.
  - **The owner's room target** (`room::Target`, `room::toward`): set at any moment, right before
    something that needs disk, with `scenic room <GB>` (1 GB or more, and less than the disk;
    `scenic room` shows it and how it stands, `scenic room off` clears it) or the menu bar item's
    Disk Room (the free space and the target, with presets of 10 to 300 GB the disk can hold, and
    Off). It's per Mac, in its agent's folder (`room-target.json`; a hand-edited one past 1 PB
    counts as 1 PB), since each Mac's disk is its own.
    - It keeps that floor: a job starts only with the target free past its own room (room-making
      makes both, a sixth of the job's room past them; a job beside another needs both free
      already), so what a job copies back never crosses it. The daily backup and GC keep no
      target (they read no cache; the NAS's backup doesn't stop for this Mac's disk). A job that
      can't have it waits, saying why in the status's `waiting`; the jobs after it that need as
      much room wait with it, unsaid, and one that needs less is tried. Room-making isn't tried
      again for those for ten minutes, or until a job ends, or the caches have been freed toward
      them. A helper asks the build Mac for no work its disk can't fit past the target; a lease it
      can't start for the target alone it gives back without a failure held against its targets,
      and asks for none for ten minutes (or until a job ends, or the disk has the room).
    - Whenever the disk is short of the target, or of the least room a job waiting for it needs
      past it (jobs running or not; not while a job an earlier agent left runs), the agent frees its caches
      toward that, whether or not the build has work left, on a thread of its own as a trim's: the
      copies by room-making's rules and order (the canopy squares too), then the others a clear
      empties, the cheapest to fill again first: the copies of the NAS's files, the base packs a
      file at a time (the least recently used first), the DEM seed whole (only while the NAS has
      it whole), the heritage clip last (an hour of osmium to make again; only while the pass's
      filtered planet it's clipped from is on the NAS, which a clear checks too). Each only as
      far as needed: the free space is measured again as they go, and the goal read again, so a
      target lowered or cleared midway stops it.
    - A freeing isn't tried again toward as much room or less (the target, or a held job's room
      past it: the two don't take turns) until a job has ended since, ten minutes have passed, or
      the target changes; toward more (a job held for more room), it is. Its record says the
      target and, apart, the room past it it freed toward (`goal`). Short of the target with nothing more to free (what the NAS hasn't, kept), the
      status says so (`caches.room.short`, and a line in `waiting`), as it does while the NAS
      isn't reachable (what goes must be kept there); it's logged and, when it freed anything, in
      the history. Until the target is lowered or off, nothing else refills the caches.
    - It never deletes what a job uses: what a job holds stays (Mid-job, above).
    - **With the mirror's reserve** (`--reserve-gb` in `tools/app/install.sh`: 50 GB on the M1,
      150 GB on the build Mac): the server's mirror copies downloads only while that much stays
      free (nothing downloaded goes for it). The two floors are apart: the mirror never frees for
      the agent's target, and the agent never touches the mirror. A target at or under the
      mirror's reserve is the agent's alone to make. Above it, a download may fill the room between
      its reserve and the target that the agent freed; the agent then stays short of the target,
      with nothing more of its own to free, and says so (§10, Gaps).
  - A trim, a clear or a freeing toward the target runs on a thread of the agent's own: its loop
    goes on beating, and jobs start and run meanwhile (room-making before a job isn't tried while
    one runs: it frees already).
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
  it's done) then its zoomed-out pack written; a tree cover piece's two, worked out then written; the peaks' and the z8 terrain's, then
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
  be cleared now, and the last trim and clear, and the owner's room target with the disk's free
  space (`caches.room`; Room on the disk).
- **The forecast** (`agent::forecast`), made with each plan (at most each minute) and in the
  heartbeat: the work left run through in the order the agent runs it. The build Mac takes the first
  it can (the pass's worldwide jobs, then a region at a time: its terrain, then its units; with
  none it can do now, the chains' work), its second job the first of its steps that fits beside it,
  each helper the far end of the first shared step with work it can do that fits its memory (terrain's
  near end): terrain at once, a unit once its region's terrain is built and the pass's heritage sites, reaches and
  roadside buildings are made (as the plan's units wait for), slope once its area's terrain is.
  Units built now but reading stale terrain (`RegionLeft::expected`) go stale or not only once that
  terrain is built (their keys name its tiles' contents), so a round share of them,
  `forecast::EXPECTED_STALE` (0.6), is counted as work to come, after their terrain. (Without it
  the 8 Oct terrain rebuild's forecast counted 32 units at 17:00 and 149 by 19:00.) A
  machine with nothing it can do waits for the next work to end or another machine to be free. The
  round under way goes first, with its own regions (their slope and tree cover left, then its chain
  as its steps take). A round goes out as the plan makes one: a region done that the map hasn't as
  it is now, an hour after the last round began (its slope and tree cover, which the build Mac makes
  while it waits, then the round's chain, as long as the last rounds took); its catalog carries the
  regions on the map that are rebuilt (a new pass) and done by then, their slope and tree cover too
  (they make no round of their own); after the last unit and terrain area, the
  slope and tree cover left, the last round (the roads' chain as it stands, less what the round
  under way still does, if longer; none when nothing's stale and no region waits to go out), then
  the overlays and a catalog, then the tree cover mids made in idle time. A z3 tile's tree cover
  assembly waits for its pieces, the build Mac's alone. The trains' and
  the landmarks' chains run from the start, each step once what it reads is built (the candidates
  once the pass's hiking-route ends are made, the peaks once every candidate and the terrain are,
  the items' facts once every candidate is, the heritage chain once the heritage sites are, the
  landmark points once those four are, trains a day once their feeds are, the 3D buildings' tiles
  once the normalized files are). Each
  target takes its last run's time at the build Mac's pace (one measured on a helper, over that
  helper's speed, asleep or not), else its step's mean, else what its jobs took here a target, else
  a first guess; each machine at its measured speed (a helper's: the build Mac's mean time a target
  over its own, for the shared steps both did, from the history; half until measured, said as a
  guess); each free once its job under way is done (its targets not yet done as they took last
  time, less what it's spent on the one under way; a job with no record, its step's time here, less
  what it's spent; if longer than its part's pace says; a helper's lease likewise); with a worker
  around that takes tails, spares what one takes typically and is measured faster than the build
  Mac at them, each of the build Mac's units 30 s more, the moment its job gives that worker to
  take its tail (docs/workers.md §3; a tree cover piece nothing more for a worker that takes its
  rows of blocks: its run makes its other blocks meanwhile). Run three times: as
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
- **Timings** (`pipeline::timings`; its Python twin `dem/timings.py`; a page's task in
  `web/work/worker.js`): every job's run is broken into named phases, each with its wall time, its
  CPU time (user and system, the process's and its finished children's, from `getrusage`; none on
  a page), its class (what it waits on: `nas-read`, `nas-write`, `disk`, `net`, `compute`, `wait`
  for a lock, a thread or another worker, or `mixed` for a phase whose sub-phases split it), how
  many spans it had, and the bytes and files it moved where the code knows them. A phase may have
  sub-phases, one level only; a child program run within a phase (a Python step, `elev`, the
  trees program) says its own phases, which come in as that phase's sub-phases
  (`SCENIC_PHASES_TO`). Every kind follows one rule:
  - every stage that moves data in bulk is its own phase of one class (the NAS read or written, the
    network, the local disk), every distinct compute stage and every external program too;
  - every job gets the records read and saved (`Out::open`, `Out::save`) as phases for free;
  - a loop over areas, tiles or files is never a phase an iteration: each of its stages is one
    phase, its spans added up (a unit's "DEM cache slice" over every area of the job);
  - sub-phases only to split a phase that's a large share of a run and mixes classes (the DEM
    cache slice: the seed's samples, the units' kept samples read, merged and written), or for a
    child program's phases;
  - nothing timed per tile or feature in a hot loop: there only counters are added to;
  - what's left untimed (the run's wall time less its main thread's top-level phases) is under
    about 5 % of a typical run; a kind past that is split further.
  A phase on another thread (the unit's copying ahead of the next area) is marked `background` and
  isn't counted against the untimed time; a phase while another thread's runs is marked
  `overlapped`, its CPU (the whole process's) then an approximation. The unit's per-area lines
  ("  DEM cache slice: 34s") are its stages' phases; every job's log ends with the same table of
  its phases (`timings: …` lines: each phase's time, share, class, CPU, spans and bytes, then the
  untimed share). The run's record (docs/formats.md, Timings) goes where the agent says
  (`SCENIC_TIMINGS`), however the job ends (a job killed leaves none); the agent keeps it in its
  `timings.jsonl`, and the build Mac's coordinator keeps every worker's in `coord/timings.jsonl`:
  its own jobs' directly, a helper's job's and task's and a page's task's with its done (an older
  worker sends none; an older lead ignores it). `scenic timings [kind] [--last N] [--host mac]`
  prints each kind's phases over its last runs (20 by default) with their totals, shares, CPU over
  wall (marked `~` where approximate), class and bytes: on the build Mac from its coordinator's
  log, on another Mac by asking the coordinator (`/work/timings`), or `--here` this Mac's own jobs.

**Two Macs** (and any other worker: `docs/workers.md`). The build Mac's agent plans; it runs a
coordinator (`pipeline::coord`, port 8090) from which every other worker asks for work that fits it.
The M1's agent (`--helper`) plans nothing: it asks for the shared steps' jobs (it mounts the NAS)
and, when none fits it, units' last steps.
- **Shared steps** (`agent::claims::SHARED`, in this order of preference: what later steps wait on
  first): terrain's and slope's pieces (eight z6 tiles a job), tree cover's pieces (four), units, the
  landmarks' candidates and peaks, and the 3D buildings' bldprep and bldtiles (8 and 16 z6 tiles a
  job; bldprep reads the NAS, which every helper mounts). The rest stays the build Mac's: the
  pass, the worldwide sets, terrain's, slope's and tree cover's assemblies, map tiles, indexing, trains, Wikidata and
  pageviews, heritage, the 3D buildings' sources, publishing. The status marks each step a helper
  may take (⇄; the landmarks', its candidates and peaks; terrain's, slope's and tree cover's, their tiles; the 3D
  buildings', its sources read and tiles).
- **What fits a helper:** each target is offered with the memory its job is expected to take: a
  unit's from its piece; another's what its last run took (the job notes, per target, the most its
  processes held together, sampled four times a second from the start of that target: a pool's
  workers summed, `SCENIC_COSTS`, "<step> <target>"), else candidates' their unit's (they read the
  same piece), else terrain's piece by a z6 tile's (its shaded hi tiles, up to 5,440 at ~270 KB,
  its z12 repairs while its z11 is made: 3.3 GB; its assembly 1.2 GB, the area's zoomed-out tiles
  and its pieces' z9 means; a measure from an area's whole run, `v` 2 and before, counts for nothing
  now: `coord::cost_version`), else a first guess per step (a tree cover piece 1 GB: the program held
  1.05 GB on 14 threads for a z3 tile's 792 blocks, a piece has 16 at most; its measures from a z3
  tile's whole run, `v` 1, and from trees.py's workers, `v` 0, 12 to 36 GB, counting for nothing
  now; slope's piece 1.5, a z6 tile's tiles, its assembly 1, its measures from an area's whole
  run, `v` 2 and before, counting for nothing now); peaks 2.5; the 3D buildings' by the rows of the row
  groups a z6 tile's bldprep reads (`agent::bld_peak`, from `crate::bld::sources`): bldprep 0.3 GB
  and 160 B a row, bldtiles 0.25 GB and 280 B for two fifths of them (its largest z8 area's
  buildings; B1's six tiles each within 0.16 GB of it). A helper asks only for the steps its disk
  has room for (15 GB: a terrain piece copies its z6 tile's raw tiles' archive, a tree cover piece
  the one to four canopy squares its blocks touch; a task 5, a row of tree cover blocks 1 (it reads
  the squares where they lie), and a sixth more, counting what its caches can free:
  not its loose raw tiles, which only its own jobs pack),
  never while a newer app waits to start, and takes the earliest step with a target that fits, from
  the far end of the plan (terrain from the near end: the build Mac's next units wait on it), a
  job's worth (units: as many as it asks). A job it still has no room for
  once its caches are emptied goes back.
- **The same app** (with the pool on, its app rule: docs/pool.md §6.1; an app from before the 3D
  buildings drops their records and refuses their hand-offs, so both Macs run one with them before
  the lead may move; from it on, the records keep the steps' they don't know, `build::Keys::other`):
  a helper says which app it runs; on an older one than the build Mac's agent
  (its updater hasn't run yet) it gets nothing (409, why in words: its status shows it), since its
  work would be recorded under keys newer code made; on a newer one (the build Mac's agent finishing
  a job on the last) it builds, since a step the newer app changed is built again once the build
  Mac's keys say so.
- **The contact:** `state/coordinator.json`: the coordinator's addresses (Tailscale's, then the LAN
  name) and the token (kept on the build Mac) the agents' requests carry (a page helps with no key:
  docs/workers.md §7); taken off the NAS when the agent stops. A worker reads it again when it can't reach the coordinator or its token is refused.
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
  English and grids; candidates' and peaks' own; a tree cover, terrain or slope piece's hi packs
  and mid; an area's lo pack and its z6 tiles' hi packs of terrain or slope, or of the tree layers for a z3 tile's
  whole run, a lease of the scheme before pieces), and journals it whole on the build Mac
  (`coord/journal/<worker>/`); the agent merges the journal before it plans, under its own lock (not
  while a paused job holds it), all of a hand-off or none, as it merged the NAS's hand-off files.
  Until they're merged, the agent plans with their done records on top of the keys, and a tree
  cover piece's mid one saves counts as made: the piece isn't made again for it, and its z3 tile's
  assembly waits for the merge.
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

**The pool** (`docs/pool.md`, phase 1, `crate::agent::pool`): built, and switched on since 8 Oct 2026. While
`state/pool/enabled` isn't on the NAS, all of the above holds; a change of it restarts each agent
between jobs into the other way. On:
- **Who leads** is the terms' (`state/build/terms/`): the Mac `state/build/writer` names makes term
  1, and leads it as the build Mac did (it plans, its coordinator grants); any other Mac's agent works
  as a helper did, `--helper` or not. A lead restarting, waking from a sleep, or before GC makes the
  next term naming itself; one on an app older than its term's stands down, and another member able
  to lead takes over after two minutes. A process whose part changes restarts into it between jobs.
- **Every job hands off**, the lead's too: under a lease `<term>-<id>` of the lead's coordinator, its
  saves into `agent/pool/jobs/<term>-<id>/` (`SCENIC_HANDOFF`), then, as an entry, to the journal on
  the NAS (`state/journal/<day>/<term>-<id>.json`), which the lead merges into its term's records
  (`state/build/term/<E>/records.json`) and writes into today's three files for their readers. No
  agent records keys, merges hand-offs or names the writer; no job carries `SCENIC_BUILD_MAC`; every
  save outside a job's hand-off is refused.
- **A catalog** goes out only once the lead's records reflect the journal (a listing of every day,
  under a day old, merged, nothing told or listed waiting); **GC** only on a step that re-asserted
  the lead's term, so caught up.
- **The coordinator's state** (leases, costs, failures, the pause) is the term's, on the NAS
  (`state/coord/term/<E>/state.json`), loaded by the next lead; the workers' token and accepted
  devices are the pool's (`state/coord/`), so pages keep working whoever leads; its history is
  each lead's own file per day (`state/coord/history/`).
- **As it's switched on**, what waits from before is drained into the journal: the build Mac's
  coordinator's journal, the NAS's hand-off folders, a helper's outbox.
- **A shadow run** (`state/pool/shadow`, or `scenic pool-shadow` beside an agent) runs the pool's
  driver beside today's coordination, writing only under `state/pool-shadow/`, and logs what it would
  decide.

**Order:**
1. **The OSM pass**, when the NAS holds a newer planet than the newest pass.
2. **The pass's worldwide jobs:**
   - `pass-sets`;
   - hiking routes' ends;
   - the units' reach (`reach`);
   - `terrain-z8` (once);
   - roadside buildings (once per Overture release; the units wait for them);
   - summits;
   - labels;
   - the water.
3. **The regions' build,** a region at a time, each published as it's done (`agent::build::plan`):
   - heritage-sites (first, one job; not waited for by the rest);
   - a region at a time: the regions the map hasn't at all first (not in its catalog), then those it
     has (redrawn, or their units' keys changed: on the map as they were meanwhile); of each, the one
     with the fewest units left first, so regions are done as soon as they can be. For each, the
     terrain areas it reads that are stale (its own areas, and those of the z6 tiles within 30 km of
     its units): their pieces to make, then each area's assembly once every piece of it is current
     with its mid (a piece current but without one comes with them, made again as it is); then its
     units whose terrain is built (a unit's key reads the terrain near it: one built first would be
     built again), neighbours together; a unit or area two regions share comes with the first. Then
     slope (a piece once the terrain of its area and of its edge neighbours' areas is built, an
     area's assembly once its pieces are) and tree cover (its pieces, then each z3 tile's assembly
     once every piece of it has its mid: a piece current but without one comes with them, made
     again as it is), after them for the build Mac. A helper takes the
     earliest shared step with work that fits it (terrain, slope, tree cover's pieces, units, …:
     what later steps wait on first), from the far end of all of that step's (the agent offers a
     step's targets together): the last regions', while the build Mac does the first's. Terrain it
     takes from the near end: the next region's, whose units the build Mac builds next.
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
     within 20 km of it, tree cover's those it meets: their pieces and assemblies; after the last
     unit, all that's left), then a prune of what the coverage no longer builds (§5, Shrinking), the
     roads' chain, and a catalog. A region done waits for its round with its slope and tree cover made: the
     build Mac makes them as it's done, before more of the regions' work, so the round only draws.
     The units follow the round in the list (a helper's, and the build Mac's while the round's work
     waits out a failure). A round before the last draws only the map tiles that go out with it:
     those meeting a region it publishes, those no unit to build when it began is near (their 100 km
     halo), and those whose owners changed (Job keys, pack(T): their base packs go out as they are,
     which the tile must index); the others, a region's border tiles, would be drawn again in every
     round as their neighbours' units are built, and wait for a later one, or the last. A round
     with nothing to publish that isn't out already (after the last; with catalogs held, weighed
     against the last held one) is none.
4. **Four chains:**
   - **Roads**, in every round and after the last unit, its first stale step: a prune of map tiles
     no unit is near, road → units index, pack, lo, stations, ferries, terrain and slope roots.
     Stations and ferries drop the packs they no longer make.
   - **Rail service**, from the start (it reads no unit): `rail-feeds`, then `rail` (§6, Rail
     service). Nothing before the rail catalogue is on the NAS (§6, Hand-made inputs), which
     the status says.
   - **Landmarks**, from the start, each step once what it reads is built
     (`agent::build::landmarks_work`): the candidates (once the pass's hiking-route ends are made);
     their peaks once every candidate is, the pass's summits are made and the terrain within 30 km
     of each is built (a unit's at a time); the items' facts and pageviews once every candidate is;
     the rest of the heritage chain on the heritage sites alone; the landmark points once those four
     are; the overlays (they read the built units) after the last unit.
   - **3D buildings** (`docs/buildings3d.md` §3.3), from the start (it reads no unit and no terrain:
     `agent::build::bld_work`): the sources' fetch (`bld-fetch`, a network job: the release's files
     and GHSL's tiles meeting the coverage, what's there skipped) when the release or the coverage
     changed; what no target has any more pruned; each z6 tile's normalized file (`bldprep`, the
     tiles within 1 km of the coverage that read a downloaded row group or GHSL tile: beside the
     fetch, a file fetched later changing the keys of the tiles it meets); each tile's 3D buildings
     (`bldtiles`, the tiles meeting the coverage) once it and its 8 neighbours are prepared, and
     once the sources are here. Its tiles in the regions' order (those of the region built first,
     first), each step's together.

   The trains', the landmarks' and the 3D buildings' work is listed after the regions' (the build
   Mac's own job takes it once the regions' work is done or waits), the 3D buildings' last: the
   second job takes it first (the buildings after its other steps), a helper the candidates and
   peaks, and the 3D buildings' tiles. Last of all, in idle time: the mids of tree cover's,
   terrain's and slope's pieces current without one (those a key scheme's switch recorded, or
   derived: §8, A new key scheme; terrain's in an area whose assembly won't run), each made again as
   it is and expected the same (`scenic-build trees|terrain|slope --expect-same`: a pack coming out other than the manifest
   has it fails the job, nothing uploaded; so a change to a piece's bytes alone bumps `TREES_V`:
   §6, Job keys); a helper takes them from the far end too.
5. **A catalog** once the roads chain is done, in a round: a new one whenever the served files
   change, or the regions it records (their recipes and the outline files they name), or which of
   them are done. It lists the units as they were when the round began. It records as built the
   regions done (`--ready <id>=<outline digest>,…`: every unit of theirs built as the coverage wants
   it, and their areas' slope and tree cover: its pieces and their z3 tiles' assemblies; one redrawn
   since the plan said so isn't; of those,
   the round's own and those on the map as they are), and the others as the last catalog had them,
   if it had them (on the map as they were); the Regions panel shows the rest as pending, or building with their areas
   counted. It waits while another worker builds a slope area or a tree cover piece of a region it
   would publish (it would go out without the region, which would then wait an hour), and while a
   helper's hand-offs wait to be merged (their areas counted as built, their files not yet in the
   manifest). What the trains', the landmarks' and the 3D buildings' chains made goes out with the
   next round's catalog; after the last unit, a catalog follows any chain's change, but while the
   3D buildings' chain has work left (their sources' fetch aside: it changes nothing served, and
   one failing for good would hold every catalog to the hour), at most an hour after the last
   round began (not a catalog for each of their jobs), unless a region waits to go out: the
   buildings never hold a region back. A round fixes the 3D buildings' packs as it begins, with
   the units (`out::AS_OF_OUTPUTS`): those made meanwhile go out with the next, so its catalog isn't
   made again as their jobs end. While
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
- `scenic status` (each Mac's build caches too); `scenic clean` clears this Mac's, and `scenic room`
  sets its disk room target (Room on the disk, above).
- The app's status bar: the build Mac's state, the NAS, and new data or a new app in.
- The menu bar item. It asks the local server (`/api/build`): this Mac's agent's status when it runs
  here, else the NAS's copy. Each agent writes its status at least every two minutes; one not heard
  from for six is shown as out of touch (asleep, off, or stuck), not as it last was. It pauses and
  resumes the build (Pausing, above), as does the map's build panel. From this Mac's agent's own
  status, read in its folder (the build Mac's `status.json`, a helper's `helper.json`), it shows
  what that Mac's build caches hold and their last trim and clear, and offers Clear the Build's
  Caches, after a confirmation, and Disk Room: the disk's free space and the room target, set
  from a few presets or Off (Room on the disk, above).
  - **Its panel:** a click on the icon opens the build page in a popover, the lead's: `/api/build`'s
    `pages`, the lead's own coordinator first on the lead, then from its contact on the NAS (their
    URLs alone, never its key) the page over HTTPS when `tailscale serve` proxies it, then the
    lead's addresses, the first that answers. (The web view's App Transport Security loads no plain
    HTTP to a tailnet address, only the LAN's `.local`; host names are compared without case.) Loaded with
    `?view` in a web view that keeps nothing, so it only watches, never helps (docs/workers.md §7);
    made as the popover opens and dropped as it closes. As tall as the page, within the screen;
    light or dark with the system; links out of the page open in the browser. While the page can't
    be shown, the status's first lines (with this Mac's downloads) and Retry.
  - **Its menu** (a right-click, or the panel's "⋯"): the state in a line, this Mac's downloads and
    build caches; Pause Building (Option: Pause Building Now) or Resume Building, and an ask under
    way; the pool's lead items (docs/pool.md §11); Clear the Build's Caches; Disk Room ▸; Open the
    Build Log (on the build Mac, while a job runs); Open the Map; Copy the Map's Address; Open the
    Build Page in the Browser; Copy the Build Page's Address.
  - Its timers run in the run loop's common modes: the polls, the icon and the notifications go on
    while a menu is open (`scenic-status --menu-proof` shows it).
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
- `extract`, `elev`, `areaflags`, `landcover`, `tile` and `scenic-metrics` are the unit's programs (the rail job runs
  `extract` and `railfreq`, the tree cover job `trees`);
- the app also carries `dem/` (the Python steps), Scenic.app (the menu bar item), `web/` and
  `fonts/`. The Python steps run in `dem/.venv` beside them, which uv makes from the app's lock file
  (`uv.lock`) the first time a step runs on a Mac (from uv's own cache of the packages, else PyPI),
  the heritage chain's in its stand-in root too: nothing of it is on the NAS (where it's made is a
  gap: §10). (The agent finds uv, and Homebrew's osmium, zstd and Java 21, on the PATH
  `tools/app/install.sh` gives it.)

## 9. Sizes

**Measured** (the 2026-09-28 planet's pass):

| | |
|---|---|
| planet | 95.1 GB |
| filtered (a) | 60.6 GB (64 % of the planet) |
| sets | 10.9 GB |
| outlines | 2.7 GB |
| OSM pieces (all land) | ~58 GB (measured as the cut finished) |
| basemap (worldwide) | 28.6 GB (its input 16.3 GB; Planetiler needs ~6× its input while it runs; 46 min) |
| water (worldwide) | 75,683 tiles, 1.38 GB (z0–8 0.56 GB, z9 0.82 GB), from 25.4 million z14 tiles of 2026-09-28's basemap in 4.5 min on the build Mac, the basemap read from the NAS (2026-10-08); deeper zooms drawn by the server |
| NAS | 8.5 TB free of 35 TB |

**Estimated:**

| | today's coverage | whole world |
|---|---|---|
| road values | ~8 GB (all land) | ~8 GB |
| base packs | ~30 GB | ~300 GB |
| our layers (terrain and slope for roadless coverage too) | ~150 GB | ~1.2 TB, with buildings |
| sources kept per pass | ~200 GB | the same |
| roadside buildings (the world's, once per Overture release; ~2.5 billion boxes) | ~40 GB | the same |
| 3D buildings (`docs/buildings3d.md` §2.5): their sources (62 GB, on the NAS since 2026-10-06), the normalized files (~440 M buildings in the coverage's 380 z6 tiles, at the 38–50 B each B1 and B2 measured) and the packs (~342 M, 13.5–20 B each) | 62 + 17–22 + 4.6–6.9 GB; a mirror the packs alone | not planned |
| an app Mac's mirror | what's downloaded: the World ~9.5 GB (the essentials 7.8, the basemap's zooms 0–10 1.7), a region from tens of MB to ~20 GB | as downloaded |

A retired pass's sources go 14 days after the next pass completes, so the NAS holds about two
passes' sources at most.

## 10. Phases and status

At each phase's end an Opus agent reviews the work against this plan.

1. **Foundations: done.**
   - The `store` crate: packs, catalogs, content naming, the I/O pool, mounting, the mirror.
   - The server serves from packs, base packs and catalogs: lazy, paged, by id plus location,
     offline start, names attached. The client follows.
2. **Agent and moves: done.**
   - The agent (recipes, heartbeat, conditions with the battery rule, batches, progress and
     checklist, backups, GC).
   - Both Macs' data moved to the NAS, the local copies deleted.
   - Descriptions moved to `descriptions/`. The menu bar item.
3. **The OSM pass and global-source layers: mostly done.**
   - **Built:**
     - the pass (filter, sets, outlines, the worldwide basemap, the cut, road values);
     - terrain, slope and tree cover per z6 tile (pieces) and their z3 tiles' assemblies (terrain's
       and slope's 2026-10-08, not yet published);
     - the worldwide z8 terrain;
     - the world's roadside buildings, once per Overture release;
     - grids inside the units;
     - the water's coverage at every zoom (per pass).
   - The 2026-09-28 planet's pass is complete, with its worldwide jobs; the water's runs once an
     app with it is published.
   - **Not built:** the sea mask.
4. **Per-unit pipeline and rankings: mostly done.**
   - **Built:**
     - base(U);
     - heritage sites and flags;
     - labels, stations, ferries;
     - the landmark jobs: pois, peaks, items, marks;
     - the rest of the heritage chain and its consumers;
     - the rail service (trains a day); two runs give the same bytes.
   - **Built:** the names and descriptions to-do lists (§7).
   - **Not built:** determinism tests, validation.
5. **Browser: done.**
   - Built:
     - landmarks, stations, ferries and overlays by view;
     - zoomed-out queries;
     - the Regions panel (add, rename, remove; regions and the view kept on each Mac);
     - the status bar;
     - catalog switching;
     - the water's coverage at every zoom, and the shoreline check.
   - **Not built:** drawing, splitting and merging regions.
6. **Cutover: done.** The regions as 88 recipes by political unit (§5), every one built (284 units)
   and published (catalog 14, 2026-10-06). The build before the agent and its conversion are
   deleted from the code.
   - **Left:**
     - the NAS's files from that build, which nothing reads now: `global/legacy/`,
       `layers/basemap/legacy-*`, `sources/legacy/` (deleted once an app without their readers is
       published);
     - the fallbacks to them in `agent::build` (`heritage_src`, `overlays_key`) and
       `markconv::heritage_source` (`markconv::LEGACY`), unused while the registers' snapshot is
       there;
     - names that still say legacy for live code and data: `pipeline::legacy` (the units' tile and
       their base packs) and `sources/registers/legacy{,-seeds}` (to be `inputs/heritage/registers`);
     - the `terrain` and `slope` programs' build-folder modes (`terrain --scan` is live);
     - the registers' seeds, which nothing makes again (§6, Hand-made inputs).
7. **Features,** each on its own.
   - Built: the terrain repair (`roadcore::grid::repair_terrain`, in the terrain job and the
     worldwide z8: §6, README "Terrain repair"). One pass that takes broken towers and pits whole,
     as blobs of the tile's component tree judged against the ground they meet, fills them and the
     voids from the clean ground around them, and leaves real relief; repairing its own output
     changes nothing.
   - Checked over the whole coverage (`terrain --scan`, 7 October): its output repaired again
     changes nothing at any zoom; the sharpest summits unchanged; 9 of OSM's summit pixels move,
     inside clusters of AWS's towers and pits. Planned: single-pixel towers of 100–300 m left at
     z7–z9, where a tile alone can't tell them from islands, judged against the finer level the
     terrain job already has (its 2×2 means).
   - Under way: 3D buildings (`docs/buildings3d.md`: its sources on the NAS; B1 done, the steps
     built and piloted by hand on six z6 tiles, the layer and the map's side built, measured on the
     iPad; B2, the agent running them for every tile as a fourth chain, the mirror's group, the
     iPad's budget and the credits, published 2026-10-08, the tiles building; B3's sharing with
     pages and the map's polish built, not yet published), then PLATEAU.
   - Built (8 October): the terrain fix (§6, Terrain): GLO-30 north of 60°N, the water flattened
     from the basemap, the seam spikes' and walled patches' rules. Terrain now depends on AWS's
     tiles, GLO-30's and the pass's basemap (pinned in its key: a new pass makes it again). Gaps: a
     lake across two z6 tiles may take two levels a metre or two apart; the worldwide z8 and the
     peaks' z12 outside the packs take the new rules but not GLO-30 nor the water; rivers are left
     as AWS has them.
   - Planned: building heights in horizons and the viewshed tool; sharper terrain from national
     DEMs.
8. **Builds anywhere: under way** (`docs/workers.md`). Done: the crates build for WebAssembly; one
   maths library on every target (outputs identical natively at any thread count and under WASI);
   the data plane's SSD copies and prefetch; the coordinator (leases, hand-offs over HTTP, learned
   memory, the shared steps' jobs for the M1); a unit's last steps as tasks for any worker, the web worker page and the M1 alike; the
   units' Python steps in Rust (`elev`, `landcover`, `areaflags`: the same bytes), and tree cover's
   (`trees`: the same pixels); the build page, a dashboard for anyone and HTTPS through `tailscale
   serve`, any page on the LAN or the tailnet helping, with no key. Next: OPFS, ranged
   reads, journaled group commits, retiring the claim and hand-off files.

**Gaps:** the code falls short of the design here.
1. **The pool** (`crate::pool`, `crates/pipeline/src/pool/`; `docs/pool.md` §12): phase 1 is built
   and switched on since 8 Oct 2026 (`state/pool/enabled`, §8, The pool), the agent's part with it
   (`crate::agent::pool`), phase 3's controls (`crate::agent::lead`: the owner's asks to hand the
   lead over or take it, the menu bar, the build page, the map's panel, `scenic lead`), and a shadow
   run beside today's agents (`crate::agent::shadow`). What they leave open:
   - the build page is served by the lead's coordinator alone (any member serving it is phase 4),
     so its "Take it" can't reach a Mac while the lead is gone: the menu bar and `scenic lead take`
     do;
   - members joining, leaving and coming back aren't in the history (the terms are);
   - a process whose member takes a term up, or steps down, restarts into its new part between
     jobs: the lead's own jobs run in its process until phase 2;
   - the records' readers read today's three files, which the lead writes from its records after
     each save; the snapshot itself is read by no reader yet;
   - with the pool on, nothing re-keys the records (`agent::rekey`): a new key scheme builds its
     targets again, but terrain's and slope's pieces, which every reader derives in memory from an
     area's whole run's record (§8, A new key scheme);
   - the raw tiles' archives the lead names stay in its records (nothing takes them off);
   - the coordinator's state per term is written on the loop after a grant, not in it;
   - the members' messages go by mailbox on the NAS, not the pool's API (pool.md §9);
   - switched off again, the pool's terms, records and journal stay on the NAS, and the records go
     on in today's files without them: switched on again, the pool would take up its newest
     snapshot, older than today's files. `scenic pool off` moves its files aside once the agents
     have left it (pool.md §12, Switching it off).
   What the core leaves open, by design:
   - create-new between two Macs is unchecked on the real share (pool.md §3, ◻), and invariant 1
     (one lead a term) rests on it;
   - a create whose answer was lost before its bytes landed, or whose maker stopped for good between
     its create and its bytes, can't be told from another Mac's still being written: its term has
     no lead until the owner forces past it (`term::make`);
   - an older lease's entry for targets of which some were set by a newer lease's is passed over
     whole: its other targets are built again (`Records::apply`; an entry's manifest changes aren't
     by target);
   - a lead's sweeps list the journal's last two days, and it lists every day daily: an older
     entry whose member never told a current lead of it, the member gone since, waits a day at
     most (`RELIST_S`);
   - a handover's new lead must read its old lead's last snapshot within two minutes: how long the
     share keeps reads stale is unchecked (pool.md §3), and longer staleness has handovers taken
     back (nothing lost). The hour rule rests on it too (an entry read not whole for an hour
     awake is refused: `UNREADABLE_S`), and so does term 1's take-up from today's files (once
     term 1 has had no snapshot that reads whole for ten minutes: `STALE_S`);
   - with reads lagging minutes, a lead settles few handovers: it settles none while entries wait
     to be read (pool.md §7.3), kept so until the share's lag is measured;
   - a refusal's note whose write fails isn't tried again: only the owner's loss, the records naming
     the refusal (`journal::note_refusal`);
   - GC (removing the journal's old days, forgetting them) isn't built: the forget horizon is
     checked by the modules' tests alone. It must respect `caught_up`'s bound too: caught up, the
     records reflect what a listing under a day old found (`Out::listed_at`), and an entry written
     since by a member gone before telling, which may name uploads made days before (its Mac away),
     is merged only by the next daily listing. GC must check the journal's entries itself before it
     removes an upload, or keep uploads longer than an entry can wait unwritten, and a day more;
   - a records snapshot, saved whole after each merge, holds the lease that last set each target
     (one per step and target, as the job keys are: about as large as the keys again) and every
     entry's key since the pool began: until GC is built nothing moves the forget horizon, so the
     records, each member's `Mine` and each of its tells keep every key. It grows with the build's
     targets and jobs, unmeasured at the build's size yet.
2. **The Python steps' environment is made inside the app's version folder**
   (`app/<version>/dem/.venv`, by uv at a Mac's first Python step: §8, Programs), which every
   Python step shares: the published copy isn't left as published (the updater checks a version's
   files only as it copies them), and each version makes its own (~380 MB, kept with its version).
   Fix: one environment per lock file beside the app, made as an app is installed.

3. **Downloaded areas offline** (§4, Mirror, per Mac): a whole road (`/api/road`) leaving a downloaded area
   reads the base packs of every unit it crosses, and fails away from the NAS when one of them
   isn't on the Mac; and a way that starts more than 2 km outside the area, in a unit whose tile
   doesn't meet it, has no way info there away from the NAS. Fix: give a road leaving the area as
   far as it's here (marked as cut), and keep the owners of the ways the area's hi data list (their
   `here` records say).

4. **Stale terrain hi packs** (§5, Shrinking): a z6 tile the coverage has left keeps its hi pack
   (98 of the 496 on 2026-10-06), and so does a piece that made no hi tiles
   (`terrain_pack::build_piece` writes none and keeps the earlier pack; 6/21/18 on 2026-10-06),
   while the area's assembly makes those z6 tiles' z8–z6 from the raw tiles alone: the map serves, and
   the units near them stage, hi tiles from an earlier run above zoomed-out ones made otherwise
   (the units' keys see any change there).

5. **Meta's canopy squares on the equator row, kept as "none"** (§6, Global-source layers): Meta
   names most of that row's files `lat=-0.0`, which the canopy downloads ask for since 2026-10-06
   (`trees::chm_urls`), but before, under `lat=0.0` alone, two squares were found missing and kept
   as empty files, Meta's "none", which is remembered for good: `sources/canopy/` holds
   `meta_chm_lat=0.0_lon=100.0_{cover5m,p95,median}.tif` and `…_lon=-60.0_…` (Singapore's and
   French Guiana's units asked, their grids reaching past the equator; each Mac's canopy cache has
   copies). Their roads are 130 km and more north of it, beyond what any road's values read of
   the canopy, so no road's values differ. Fix: remove the six files, on the NAS and in both Macs'
   caches, before any region reaches south of the equator.

6. **The water's deeper zooms need the basemap** (§6, Water): z10 and deeper are drawn by the
   server from the basemap's z14 tiles, so a Mac away from the NAS draws them only where it has
   the basemap's pieces of zooms 11–14 (a downloaded area's z6 tiles, §4 Mirror). Elsewhere, the
   server answers with the nearest stored zoom's water over the tile, scaled up (z8 from the
   World's lo packs, z9 where a downloaded area's hi packs are), not to be cached, so nothing fails and nothing is asked for again;
   the coastal shading leaves out a neighbouring tile it can't have. The shores are then z8's or
   z9's, blurred close up. Fix, if it's wanted: keep the basemap's water polygons where it's away.
7. **The pass's `water` set serves only the shoreline check** (§6, Water): about 6 GB on the NAS a
   pass and its share of `pass-sets` (65 min over the LAN for this set), for a reference read apart
   from the basemap. Dropping it from `osmpass::SETS` would leave the check to build its store from
   the basemap's own z14 tiles (then not an independent reference).
8. **The water near the camera in steep 3D terrain, before the view first moves** (§6, Water):
   MapLibre works out a source's tiles as the camera moves, culling with the elevations it has
   then; tilted at z14–15 over the Highlands with 3D terrain, the water's tiles on the slopes
   nearest the camera were left out and not asked for until the camera moved (a no-op `jumpTo`
   brings them) (a loch at the bottom of the view missing: 1.9 % of the pixels in the
   shoreline check's Scotland z15 view; the basemap's coarser polygon tiles covered it). Fix:
   have the sources' tiles worked out again once the terrain under the camera has loaded.
9. **The mirror doesn't know the room target** (§8, Room on the disk): a disk room target above the
   mirror's reserve (50 GB on the M1, 150 GB on the build Mac) can be filled by a download's copies
   once the agent frees toward it, leaving the agent short of it with nothing of its own to free
   (it says so). Fix: the server reads the agent's `room-target.json` and keeps its mirror's
   reserve at the larger of the two while one is set.
10. **Names by language** (§7), what's short of the design:
   - **The area tables are still on the NAS** (`translations/<area>/`, 342 MB): each server copies
     them and leaves their lines out. Fix: delete the nine folders once a published server shows
     the converted lines (`translations/0-converted/`).
   - **The converted lines hold for their area's languages,** not those spoken where their names'
     things are (the area tables kept no places): Iberia's lines hold for Spanish, Portuguese,
     Catalan, Galician and Basque alike.
   - **Roads carry no language tags** (only the spoken languages lead to their lines). Planned: the
     unit job writing them beside `global/roaden/<u>`, which rebuilds every unit, so with the next
     pass.
   - **The basemap** carries `name:en` and `name:fr` alone until the next pass
     (`osmpass::BASEMAP_LANGUAGES`).
   - **Wikidata's English labels** aren't carried; a landmark's English Wikipedia title (`w_en`) is
     in its popup record, not its English.
   - **The to-do lists** don't read the basemap's things (rivers' names), stations, ferries or the
     area overlays' names; the descriptions' list lacks heritage areas, Indigenous lands, special
     areas and World Heritage outlines (their records hold no article or register entry).
   - **The refinements** cover the coverage's subdivisions only: Switzerland's cantons aren't
     (Geneva reads German first), and Åland comes out Finnish (OSM's outline of it has no ISO
     3166-1 code).
   - **Near a border** the raster is as good as the simplified outlines (1 km for countries): where
     two overlap the smaller wins, so a thing within about a kilometre of a border may take its
     neighbour's languages (the ruins of Wasigenstein, in Alsace 300 m from the border, read German).
     Fix, if it matters: the outlines' full rings near borders.
11. **A worker's tail in a one-unit job only frees the build Mac's cores** (docs/workers.md §3):
   the job waits on a worker holding its tail only while its measured pace says it'll be back
   before the build Mac's own run would end, so a worker slower than the build Mac is raced at once
   (and kept to finish only to measure it), and the build Mac runs the tail anyway. The paces live
   in the coordinator alone: a restart measures each worker again. Fix: the job ends while a worker holds its tail,
   its slot free for new work, and the agent commits the unit when the tail is back (or runs it
   then, past its time): planned with the pool's phase 4.

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
  - the densest (Kanto, a 3 GB base pack);
  - if a unit or its 110 km halo doesn't fit in 48 GB, units split into z7 or z8 tiles, and pack(T)
    by z7.
- **Remote DEM servers** may be slow or change. Today's cache seeds the units, and their new samples
  are kept on the NAS (`cache/dem-units/`).
- **Version bumps at globe scale** would take days: today a bump makes every target stale in the
  normal order.
- **Way ids past u32** (2040s): the tile format is versioned.
- **Disk:** for 14 days after a pass completes, the NAS holds two passes' sources (~400 GB).

## 12. Changes

**Downloads (2026-10-08):** the mirror copies only what the owner downloads (§1, §4 Mirror),
by the owner's ask: the World, zoomed out (the essentials and the basemap's zooms 0–10), and each
region or view (its packs and the basemap's zooms 11–14 over it). Before, each Mac copied the
whole catalog as its room allowed, the essentials first, then kept areas, then the rest by recent
use, and let files go for room. Why the rest of the design: the basemap is split into pieces by z6
tile rather than copied whole (28.5 GB for any area) or kept as a sparse copy (§4); a region needs
the World offline, so it brings it; copies keep to 20 MB/s while the build runs rather than
pausing, which with the pool on would hold an owner's download off for hours.

**v7, names as built (2026-10-08):** §7's design built, with these choices:
- Hong Kong's and Macau's names are read in Cantonese (`yue`), a language of its own: Chinese lines
  carry Mandarin's Pinyin (Taiwan's readings), which Hong Kong's romanisation isn't.
- A name is looked up in its own kind only (road, settlement, other), with no fallback to another
  kind's line: the kind is the point of the key. The converted lines keep the old lookup's reach
  instead: each holds for the other table's kinds where that table had no line, and for the
  languages the old boxes reached besides an area's own (the owner's choice, 2026-10-08).
- Roads' own English is the units' `global/roaden/<u>` alone: one road's English isn't copied to
  every road of its name.
- The converted lines hold for their area's languages (the area tables kept no places).

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
- **Terrain always from AWS's raw tiles.** The repair reads AWS's own values, bathymetry and all
  (stored tiles are at sea level), and the same raw tiles give the same bytes.
- **The terrain repair weighs blobs, not pixels** (2026-10-06). The first repair judged each pixel
  against a ring two to three pixels out, so a cluster's inner pixels hid behind its outer ones and
  a pass took only the outer layer (on 1 October a third pass still changed 304 tiles, by up to
  2,466 m). Repeating it until nothing changed was turned down as crude. Each tower or pit is now a
  component of the tile's level sets, taken whole whatever its size and judged against the ground
  it meets; a stage after the first only judges what the first revealed (a lesser tower that stood
  on a greater one's flank, a lobe of its ringing) on the tile with what was found filled in. What's
  broken is judged by steepness and by context (it towers over flat ground), so a summit AWS drew
  too sharp, among rough ground, stays.
- **base(U) runs the unit's programs on a unit folder,** staged from the build manifest.
- **Landmarks, stations and ferries have jobs of their own,** and pack(T) writes no landmark tiles
  (`docs/phase5.md`). Otherwise every road pack would depend on worldwide rankings.
- **Heritage sites and flags come before the units, in their own job;** the rest of the heritage chain
  comes after the landmark candidates. Wikidata and pageview outages mustn't hold up the roads.
- **Roads and landmarks build as two chains,** for the same reason; the rail service is a third
  (an operator's server down mustn't hold up the roads either).
- **The rail service keeps the feeds it has** (the user asked, 2026-10-04, that what was collected
  be reused, and only what new coverage needs be fetched). A zip counts from the day it was fetched, so it's never
  fetched again just because time passed.
- **The rail feeds' countries come from the coverage** (§6, Rail service), so a region in another
  country has its own feeds.
- **Elevations are u16 decimetres from −500 m.** They were clamped at ±3,200 m, and roads in the
  Andes and the Himalaya reach 5,800 m.
- **Failures are never cached, and the breaker needs a failed probe.** A busy link slows reads
  without the NAS being gone.
- **Power: mains, or the battery down to 30 %** (asked for 2026-10-03); caffeinate per job.
- **The menu bar item,** with progress to the end (asked for 2026-10-03).
- **What the internet answered is kept on the NAS** (§4, Downloads): the items job's and the
  heritage chain's Wikidata and Wikipedia answers, a pass's at a time, as an archive each step
  writes as it starts and ends. They were on the build Mac alone, so another Mac leading the build
  (docs/pool.md) or a lost disk would have asked again (the 2026-09-28 pass's 59,000 items took
  about 37 minutes of queries), and the owner wants nothing fetched twice without a good reason.
  After its review: a file deleted by hand stays deleted (one cut short would otherwise come back
  from the NAS, the job failing again), a step's end never writes over another writer's archive
  (merged by key where it can be: once the lead moves, two Macs' runs can overlap), and the build
  Mac's agent sends what the NAS lacks as it starts (the next run may be the next pass's, months
  away). The heritage scripts run in the app's Python environment, as the other steps do: theirs
  was the only one kept in the cache.
- **A unit's key names the terrain tiles it reads, by their contents** (§6, Job keys; 2026-10-06),
  not the terrain hi packs within 30 km. A terrain run that changed part of a z6 tile rebuilt every
  unit within 30 km of it, while the hi packs' names couldn't see the zoomed-out tiles a long way
  reads, nor a stale hi pack's z6 tile made again from the raw tiles alone.
- **A change of key scheme re-keys the records** (§8, A new key scheme): a change of keys
  mustn't build again what would come out the same, and the units' alone would have rebuilt all
  284 (some seven hours of the build Mac).
- **Tree cover a z6 tile at a time, each z3 tile's zoomed-out tree cover assembled from mids**
  (§6, Trees; 2026-10-06). A z3 tile's whole run took up to half an hour and 30 GB of canopy squares
  on a helper's disk, and a change anywhere in it made it all again; a piece copies its z6 tile's
  one to four squares and takes about a minute, and a change makes its pieces and its z3 tile's
  assembly (seconds) again. Its switch makes the whole tree cover again once, with the program:
  trees.py had made every pack, in bytes the pieces don't reproduce (§8, A new key scheme). Byte
  identity is the build's rule, so its packs are made again rather than taken as the same by their
  pixels.
- **Terrain and slope a z6 tile at a time, each z3 tile's zoomed-out levels assembled from mids**
  (§6, Global-source layers; 2026-10-08). An area's run made every z6 tile of it again for a
  change in one, and held the build Mac for up to 15 minutes, where a piece takes seconds and a
  helper can take it; a slope piece's key names the terrain tiles it reads, so a change in one
  piece makes its slope and its edge neighbours' again, not the area's. Terrain's mid holds its z9
  tiles' means and its lakes' levels, not its z8–z6 tiles: a lake's level at z8 comes from the
  whole area's z8 tiles (the water, 2026-10-08), so the assembly makes the area's z8–z3. The
  switch makes nothing again (the area's run is its pieces and assembly, byte for byte) but the
  slope pieces whose border reads another area's terrain; the records aren't re-keyed but read so
  by every reader (§8, A new key scheme): with the pool's records, a job's save is the only writer.
- **The water as exact coverage, drawn as a raster** (2026-10-08, §6, Water): it replaced the small
  islands and lakes' dots and outlines over the basemap's water polygons. Against the full detail
  (`tools/coastcheck`), the dots, Natural Earth below z6 and the vector fills' anti-aliasing left
  7.7 % of the pixels visibly different (13.1 % tilted, where the faked dots speckled far lake
  districts); coverage tiles at a texel a device pixel leave 0.04 %, and, tried on 27 views, at a
  texel a CSS pixel 1.1 % (against 0.10 % there). Vectors can't reach it: a polygon under a pixel draws nothing or too much, and a fill's
  anti-aliasing isn't its area. The pass's `water` set stays, as the check's reference, read apart
  from the basemap.
- **Zoomed out, the water is full detail shrunk** (2026-10-08, the owner's choice, §6 Water): no
  visibility floor and no exaggeration, small islands and lakes as faint as their share of a pixel,
  with the coastal shading full detail gives them, the same at every zoom. Measured on the screen
  in the app's colours against full detail rendered and shrunk in linear light, the shading
  measured from coarse pixels over half land, the stored values' blend and the nearer mipmap level
  left 3.29 % of the pixels visibly different and a jump at each half zoom on 2.23 %; the
  shading from every pixel holding any land at the water's density, light's mix, trilinear
  mipmaps and bytes that keep any land leave 0.15 % and 0.04 %.
