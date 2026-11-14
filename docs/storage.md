# Storage

NAS root: `personal/projects/scenic-roads/`, on the `personal` share. Deletions over SMB bypass its
Recycle Bin.

```
inputs/        the inputs folders (docs/inputs.md): regions/, languages/, translations/, descriptions/,
               heritage/, heritage-matches/, timetables/{feeds,gtfs}/, elevation/, sources/, extents/, each
               with how/ (how it's made); README.md; keys.env (KEY=value: the feeds' API keys, the
               NAS's SNMP keys NAS_SNMP_USER and NAS_SNMP_AUTH, its DSM account NAS_DSM_USER and
               NAS_DSM_PASS);
               _gate-test/ (the input check's test unit)
sources/       osm/<date>/ (planet, filtered, pieces/, sets/, roads/, outlines, reach, pass),
               basemap/ (Planetiler's jar and data), items/<date>/ (facts, pageviews, answers),
               pageviews/, buildings/<release>/,
               trees/, canopy/, aws-terrarium/, pyramids/ (GSI's tiles), rail/, canvec/, fetched/glo30/,
               inputs/<unit>/ (the input check's checked copies and accepted indexes: inputs.md §4.6)
base/          base packs, one per z6 tile
hidata/        per z6 tile: ways-here index, query parts, rail lines, zoomed-out summaries
markdata/      landmark points per z6 tile, in one bundle (all)
ovdata/        per z3 tile: area and park details
global/        worldwide files: road values per z6 tile, road → z6 tiles, rail frequencies, landmark
               totals, heritage/, spoken
layers/<layer>/  root, lo and hi packs, or one bundle (all); basemap/world-<date>.<hash>.pmtiles
work/          build intermediates, not served
cache/         what z6 tiles keep for later runs, shared by both Macs: scenic-units/
state/         build/ (the records, their brief, the lead's declaration),
               coord/ (the token, the history's archive (a tar a finished day: the history
               itself is the lead's, on its disk), the forecasts' and forecast inputs' archives),
               pool/nas-probe/ (the NAS budget's probe's own folders: it measures the share),
               live-archive/ (each Mac's live measures, a file a
               day), backups/;
               coord/history/ (the days an app before task #1119 wrote: GC packs each into the
               archive and removes it as it passes the week, the last on 19 Nov);
               build/term/<E>/ (old forecast inputs the forecast's backtest still reads:
               docs/coordination.md §9)
```

**Immutable, content-named files.**
- Every built file is `<logical>.<hash16>.<ext>`, never rewritten. A write goes to `<name>.tmp`, is
  read back past the client's cache (a fresh descriptor with `F_NOCACHE`, which the Mac's SMB
  client reads from the server; one it refuses fails the write) and checked against its hash, then
  renamed (`store::naming::write_atomic`). The temporary file is closed with the close's result
  checked (`store::sys::close`): the Mac's SMB client keeps a pageout's write error for the close
  alone (its fsync doesn't return it, and a dropped `File` ignores it), and on a soft mount a
  session reconnect fails every request in flight with ETIMEDOUT. A write lost to the share for a
  moment (ETIMEDOUT, EIO or a dropped session at its open, a write, its sync or its close:
  `store::whole::lost_write`, task #1142) or a read-back that differs, its source unchanged, is
  written again, three writes at most, each a line in the job's log; a source changed since it was
  hashed fails at once, and so does any other error (task #1140: on 9 and 11 Nov three packs of
  511-823 MB from the Mac mini, and on 24 Oct one of 185 MB from the M4, read back the same length
  but other bytes, and failed their jobs; the mini's SMB session had reconnected during the 11 Nov
  one's upload). Each write and read-back moves the whole file: the writes are counted as the
  NAS's bytes written, the read-backs as its bytes read (store::nasio's `ReadBytes`), and apart
  too (`ReadBack`), so a job's cost line says them as `read_back`, beside its `bytes` (what the
  step read), and its NAS weight counts them (docs/pool.md §7.7). Each of its calls on the share
  (the stat, the folder made, the open, each 4 MB written, the sync, the close, the read-back, the
  rename) is a store::deadline call, two minutes at most (a read-back counted from its last byte);
  one left in the kernel past it fails the upload, never written again (the temporary name held
  with it), and a late finish lands only in this process's temporary name or the same
  content-named bytes (docs/nas-access.md §6). Every other NAS writer closes its file checked too:
  `store::whole`'s writes and copies (a lost write of their temporary file written again, as
  here), a `NasFile` (`close`), an in-place write, a create-new, the server's I/O pool's writes,
  `store::sys::copy_data`, the NAS budget's probe and `bld-fetch`'s appends (a partial download
  whose close fails removed, fetched afresh). An unchanged file keeps its name, so a
  rebuild uploads only what changed; one already there is reused as it is, nothing written to it,
  and named in the job's hand-off (`Handoff::reused`), which the merge checks against GC's deletions
  (GC, below). One the manifest already names under the same logical name takes one look
  (`Out::put_file`, task #449: heritage-sites' slices, hundreds of small files nearly all
  unchanged): there at its size, it is taken; missing, put again; of another size, the put fails (a
  damaged file is removed by hand).
- **Uploads beside the work** (`Out::put_file_later`): slope and terrain z6 tiles, slope's and terrain's
  z10 tiles, terrain's z3 tiles (task #1097), map tiles (`pack`)
  and the 3D buildings' tiles upload a target's files on a thread of their own while the job works
  on its next target, and a z6 tile its grids and base pack while it makes its road values. A file is
  named in the manifest only once it has landed; a target is saved and noted done only after its
  uploads have (`save_upto`); at a safe point with the build pausing, the target uploading is
  landed and noted done, then the job stops. Its cost line counts its own uploads' bytes. A put,
  set or removal of a name still uploading waits for that upload first. An upload that fails
  transiently is tried again after 2, 5 and 15 s. An upload has no limit on its whole time: one
  that fails, or moves no byte (hashed, written to the share past the client's cache, or read
  back) for two minutes (`out::UPLOAD_STALL`), fails the job, the target not noted done, and every
  later save with it; a slow share that still moves lands.
- **The map** names every file the map server reads, with the coverage it was built for and the
  credits of its sources. It and the log of its updates are the lead's, on its own disk, served by
  its coordinator; the NAS holds no copy (docs/formats.md, The map and its log).
- **The build manifest** lists everything built; **job keys** say what each output was made from.
  These, the unverified uploads and the files' sizes are the build's records
  (`state/build/records.json`, the truth, kept first on the lead's disk: docs/building.md §2, The
  pool; docs/pool.md §6.2; its manifest and keys alone in `brief.json` beside it, for the readers
  needing no more). Before the pool they were `state/build/manifest.json`, `jobs.json`,
  `pending.json` and `sizes.json`, which a new pool's first take-up starts from and no lead writes
  (docs/pool.md §6.2).
- **Unverified uploads** (content name → its SHA-256, recorded as it lands): a file a job copied to
  the NAS is read back past the client's cache as it lands and checked against its hash, and stays
  pending until checked on the NAS itself; one a job found there and reused is pending too (nothing
  touches it, so its check is what finds it gone). The records hold pending files as they hold the
  manifest's. GC's daily duties drain them (`agent::gc`): first every pending upload the build
  doesn't read (neither a manifest name nor a file an accepted inputs' index it names lists:
  replaced or dropped since) leaves the pending unchecked, its file then GC's queue's as any the
  records stop naming; then every one it reads is checked on the NAS over SSH to be there at the
  records' size (`out::verify`: `wc -c` there, a file's size from its metadata and its last block
  read at most; one the records hold no size of, written before they kept sizes, to be there). Its
  bytes aren't hashed again: the read-back checked them as they landed, and the NAS's file system
  (Btrfs, data checksums on: no `C` attribute on the share) checks every block it reads, so a
  reader of a damaged file gets an error, never other bytes. What is left is whether the file is
  still there, whole (task #1001: hashed on the NAS, 57,444 pending uploads of 2.2 TB or more,
  most of a week's writes, kept its disks at the whole NAS budget for hours). 500 a call at most, fewer when their quoted names would pass 96 KB (`out::NAS_NAMES_MAX`: the NAS's shell takes the command as one argument, 128 KB at most), each held
  to a deadline of 3 minutes (past it the call is stopped, SIGTERM, SIGKILL 10 s later, and the run
  ends there, said in GC's warnings, the rest left to the next duties' pass); each call's result
  saved and a safe point after it, so a run stopped midway keeps what it checked; each file looked
  up counts as one of the job's NAS stats, its weight. One there at its size leaves the pending; one
  of another size or not there stays pending, nothing removed, and the job's warnings name it (its
  target is done under its key, so nothing builds it again but `scenic redo` of it). Both are the
  `gc` job's change through `Out`: in the pool its hand-off's `checked`, which the lead takes out of
  its records' pending. A root not on the NAS's share (a test's) is never checked. `scenic gc
  --dry-run` counts both, writing nothing.

**Copies carry the bytes and permissions, nothing else** (`store::sys::copy_data`; `cp -X` in
scripts): the share refuses a copy that sets a `com.apple.provenance` attribute differing from its
folder's, so `std::fs::copy` or plain `cp` fail with "Permission denied".

**Packs, never one file per tile.** SMB manages about 80 random reads per second per file.
- **Our layers** use packs: a header and meta, the blobs (identical blobs stored once), then an index
  of (tile key, offset, length, raw length, 64-bit hash) sorted by key. Root: z0–2; lo: z3–8 per z3
  tile; hi: z9–14 per z6 tile. Hi tiles exist only near the coverage for the layers built per z6 tile;
  worldwide layers (labels, water) have them wherever they have tiles.
- **Bundles** (docs/formats.md): a small layer one job makes whole (each landmark kind's thinned
  tiles, each area overlay) is one pack of every tile, `layers/<layer>/all`, which the map names
  for each pack key it holds; the landmark points (a sectioned file per z6 tile) are one bundle,
  `markdata/all`. Each file made on the NAS costs a second or so of round trips: the marks and
  overlays jobs made about 450 and 380 a run before (task #449).
- **Small files are written one by one, not staged and batched** (task #683, measured 26 Oct from
  the lead's timings, the last 20 runs of each step: 85 h of jobs). Uploads took 5.5 % of job time,
  nearly all of it bytes of large packs; the jobs made 12,959 files, which at the NAS's measured 28
  creates/s is 0.15 % of job time. Only heritage-sites (about 720 slices a run, 76 s), stations (230,
  58 s) and water (1,628, 174 s, one run) make hundreds, and none is on the build's path: the pool was
  core-bound with the NAS budget at 1–3 %. Staging and batching would buy nothing; measure again if a
  step starts making thousands of files a run.
- **The basemap** is Planetiler's worldwide PMTiles file, one per pass, served tile by tile.
- **Base packs, hidata, markdata and ovdata** are sectioned files: mapped when local, read from the
  NAS in pages (docs/macs.md).

**Writers.** Every job uploads its files (content-named) and hands its record changes off to the
lead by HTTP (docs/coordination.md §5.3), which merges them; no step
writes the records directly once a lead is declared
(`crate::out::check_writer`); the planet is fetched so too, by the lead's `planet-fetch` job (docs/steps.md
§1). Other writers: the lead's agent, under `state/` (the records' snapshot and brief, the forecasts' and their
inputs' archives), each Mac's agent (its live measures' days, `state/live-archive/`), the lead's coordinator's NAS
probes (`state/pool/nas-probe/`), the first agent to find no token (`state/coord/token`), and the owner's inputs
folders. The map's server writes nothing on the NAS.

**GC** (`agent::gc`, the queue `store::map::gc` and `agent::gcqueue`) never looks for garbage: the
lead queues each file as it stops being held, and the `gc` job removes what's due, a chunk at a
time. The job is the lead's own (`LEAD_ONLY`), planned whenever GC's queue has a chunk for it or one
of its sweeps is due (`Planner::gc_work`), and run when the NAS is quiet: only beside leased jobs
that leave its measured weight room (pool.md §7.2, `Limits::fits_quiet`; unmeasured the way it runs
now, weighing the whole NAS beside no job weighing the whole, as the lead's own unmeasured NAS steps
go, so its first run measures it), or, held back 24 hours, as an aged job of the lead's own
(`QUIET_AGE`); running, it yields at its next safe point (between
two removals, or two folders) to a target held back for the budget, and the next run goes on where
it stopped. So there is no run to finish: the queue's length is what's left.
- **What holds a file.** A content-named file on the NAS is held while any of these names it:
  the map at any `seq` a client still holds (a cursor heard within `CURSOR_TTL_S`, 6 hours:
  docs/formats.md, Cursors); the records (the manifest and the pending uploads), which the lead
  alone saves; and a hand-off the merge has taken, until the records after it are seen (its changes,
  its uploads and the files its job reused, `Handoff::reused`). The lead sees every change to the
  first two as it makes it, and the third as it takes it.
- **The queue** (`agent/maplog/gc.jsonl` on the lead's disk, beside the map's log): JSON lines,
  appended and `fsync`ed once per change (a batch, a records save, a removal), rewritten whole once
  it has grown to four times what it holds; it survives a lead restart. When a batch of the map's
  log frees a content name (`Batch::freed`) or a records save drops one, and the other holder
  doesn't name it, the lead queues it with the `seq` and time it was freed at; a name named again
  leaves the queue. The holders' index is memory alone: the map's names from the log's map at the
  open and each batch, the records' from each save the lead's agent makes; nothing is due until
  the records have been seen. A name is **due** when:
  - every client's cursor is past the `seq` it was freed at;
  - an hour has passed since (`GRACE_S`): a margin for readers outside the cursor set (a request
    in flight as a map server's cursor moves on; an ad-hoc script). Each day of grace would hold a
    day's freed files (2,000 to 9,000 names, some 26 MB each: 50 to 240 GB), and no window to look at a bad map
    update is wanted: updates are small and many a day, a bad one is fixed by the next, and the
    map is never set back to an earlier `seq`;
  - every live lease was granted after it was freed (a job running since then may have looked it
    up by name);
  - and neither the map nor the records name it, checked again as it goes.
- **By the queue:** each run first takes the due names from the lead's coordinator 200 at a time
  (`POST /gc/chunk`; `gc::CHUNK_NAMES`): the lead checks each against its holders as of then and
  marks the rest deleted in its queue, the newest lease granted then theirs, on its disk before it
  answers; the job removes them one by one over SMB, one call a file, nothing listed (a file not
  there counts as gone while the share answers, its `inputs/` folder there; a lost mount fails the
  run), then says them gone (`POST /gc/gone`, the marks' lease renewed to the newest then). With the
  records not seen yet the lead answers 503 and the run removes nothing. A chunk's names are
  refused to every hand-off while they're being removed: some 24 s of the share's deletions at the
  median of the NAS budget's probes (8.4 a second, 6 and 7 Nov), 80 s at the tenth percentile; its
  two asks of the lead and one `fsync` there are noise beside 200 SMB calls (12.5 ms, measured on
  the M4 over a lead's coordinator's HTTP).
  Killed mid-chunk, the next run's first chunk is the same,
  told again (one held by then kept out of it, still marked); yielding mid-chunk, the job says what
  went and lets the rest go back to the queue, unmarked (but a name told more than once, which the
  killed job may have removed, stays marked). A chunk takes a name in a folder the map indexes, a removed
  step's, or a source of a pass older than the newest complete one (`gc::by_the_queue`); the
  gate's copies are the inputs sweep's below, and the rest of `sources/` is never GC's. The lead
  not reached, or not able to put its marks on its disk, nothing is removed and the job fails. The
  job's card says how many names it has; the queue knows no sizes.
- **Reuse without a touch.** A job that finds its file already on the NAS takes it as it is and
  names it in its hand-off (`Handoff::reused`); nothing writes a reused file, and nothing goes by a
  file's time but the sweeps' age rules. The merge refuses a hand-off bringing a name being removed,
  or one removed since its lease was granted, by the leases' job ids (pool.md §7.5), no clocks: its
  targets are leased again, a redo that uploads the file afresh (`gcqueue::refusal`). With the
  third rule of "due" this can only happen to a name freed before the lease, found by the job, and
  removed during it, or to a name freed during a long job, due once its lease ends and removed
  before its hand-off is merged: a redo, never a loss. A removal's mark is kept while any lease
  as old may still be handed off: each lease granted to a member is owed by it (a `Granted` line
  in the queue's file) until a beat of its lists that lease neither held nor handed off; the oldest
  lease owed, or out now, is the floor, and a mark whose lease is older goes. So a member away for
  weeks keeps every removal since its oldest lease out (about 70 bytes a name in the queue's file,
  150 in the lead's memory), until it comes back or the owner releases it (`scenic pool release`).
  One the merge takes holds its names from every chunk until the records after it are seen. A
  reused file's check is pending as an upload's, so the uploads' check finds one missing too.
- **What the queue can't see.** A lead whose log is lost starts an empty queue: it refuses nothing
  the old queue removed (a reused file missing is then found only by the uploads' check, said, not
  mended), and the orphan sweep queues what the lost queue held. A file uploaded and never handed
  off (a job killed after its upload, a refused hand-off) was never held, so never queued: the
  orphan sweep's too.
- **The sweeps**, each a pass with its own cursor in the lead's map log folder
  (`agent/maplog/gc-sweeps.json`, `gc::Sweeps`: the folder each walk reached, so a run stopped
  goes on after it), run after the queue's chunks:
  - **The duties**, once a day: the names to drop (below), the history's and the forecasts'
    archives, the forecast inputs' old days, and the pending
    uploads' check (above), last.
  - **The orphan sweep**, a pass every 14 days (`ORPHAN_AGE_S`): it lists,
    a folder at a time, the top folders of what the map and the records name, the removed steps'
    folders and the retired passes' (`sources/osm/<date>/`, `sources/items/<date>/` older than the
    newest complete pass), and sends the lead each content-named file at least 14 days old (no job
    runs that long between its upload and its hand-off), which queues those nothing holds, freed
    as it finds them (so a lease older than that holds them, and they wait their grace). A pass
    begins 14 days after the last ended, so an orphan goes within twice its age. It removes stray `.tmp`
    files after 2 days, and a retired pass's other files (the planet download) once the newer
    pass has been complete 14 days, with its folders left empty.
  - **The inputs sweep**, a pass every 14 days: the input check's checked copies (`sources/inputs/`,
    inputs.md §4.9) are kept while the records' manifest, an accepted index or
    an index of the last 14 days names them (a hand-off not merged yet bringing one removed is
    refused by the merge, its job's work done again); an index names its files' copies and every content
    name under `sources/inputs/` in their facts, nested ones too (`FileEntry::stored`: a region's
    `.rgeo`, and the regions' `.rgeo` an elevation dataset's `measured.outline` was cut by, which
    its plans read while that index is accepted). The rest, 14 days old, is told the lead 200 at a
    time (`POST /gc/removing`), which answers those something holds now (kept) and marks the rest,
    removed then as a chunk is. What keeps them is read as the pass starts and again before a
    chunk once 30 minutes old (`gc::KEEP_FRESH_S`, half the grace for which the lead holds a copy a
    merge took). None is
    swept while what keeps them can't be read (said).
  No sweep goes through a link (a file reached by one has a path no holder names).
- **Removed steps' data** (`agent::gc::RETIRED`, logical-name prefixes; today `sources/terrain-z8-`,
  the worldwide z8 terrain removed by task #670, `global/legacy/`, the old pipeline's converted
  data, read by nothing since, and `work/terrain-lakes/`): the duties first drop every manifest
  name under one (their uploads still pending then leave the pending with every other one the
  manifest doesn't name, above), as the `gc` job's own change through `Out` (its write-set: these
  removals and the depth packs' below, `agent::steps`): in the pool a hand-off the lead merges into
  its records in one save, as any job's (pool.md §7.3). With nothing left to drop it writes
  nothing. Their files then go as GC's queue frees them, and those named before the queue was by
  the orphan sweep. A step whose code is deleted adds its outputs' prefix there, so "removed"
  means its data deleted too; no step may make a name under one.
- **Depth packs no target names** (task #679): in the same drop, every depth pack (the `depth`,
  `depth-class` and `depth-shade` layers' z6 hi and z3 lo packs) whose tile no current depth
  target names: the z6 tiles and z3 tiles the coverage and the declared depth sources make
  targets (`build::depth_kept`, as the planner's `depth_tiles`, whatever their water's keys), and
  those leased now. The agent writes them to its home (`gc-depth-keep.json`) as each plan plans
  the `gc` job and passes them (`scenic gc --depth-keep`); none found (sources or coverage
  unreadable, no source declared, an empty coverage), or the file over an hour old
  (`DEPTH_KEEP_FRESH_S`), no depth name is dropped that run. That covers the targets the coverage
  left and those the depth's rules no longer make (a z6 tile no source finer than z8 meets).
  Their files then go as GC's queue frees them. `scenic gc --dry-run` counts them, writing nothing.
- `scenic gc --dry-run` says what's due by the queue, what each sweep would find and drop, and
  marks, sends and removes nothing; without `--sweeps` every sweep runs whole.
- Never swept: the newest pass, a planet waiting for its pass, the rest of `sources/`, inputs and
  state. The packed tile stores (`sources/aws-terrarium/`, `sources/pyramids/`), whose
  archives no manifest names, are kept by their own indexes (macs.md, Downloads), never by GC.

**Backups.** Daily, the lead's `backup` job (agentcore::agent::backup) copies `inputs/`, the input
check's acceptances (`state/inputs/accepted/`) and the map's settings profiles (`profiles.json` in
the app's folder on the lead, the map server's Mac: crates/server/src/profiles.rs; listed as
`mac/profiles.json`) into a content-addressed store under `state/backups/`, with a dated list on
days something changed (kept 30 days), mirrored to the lead's own disk (`<home>/agent/backups/`);
`todo/` isn't backed up, and no link is followed or backed up (#1004: a link to its own folder,
`inputs/inputs`, failed the backup on 7 Nov). Everything else can be regenerated. A laptop's server
on demand keeps a list of profiles of its own, in its own app folder, not backed up.

**Settings profiles.** The web app reads a profile's settings only in the shape it writes, and fills
a setting the profile lacks from `web/default-profile.json` (README, Using it: Settings): a profile
saved before a setting was renamed or reshaped would follow the seed for it. So a change that renames
or reshapes a stored setting converts the stored lists at its rollout, each Mac's (the map server's,
`~/Library/Application Support/scenic/profiles.json`, and each laptop's own), with
`tools/app/profiles-reshape.mjs` (its header), `--app` a checkout of the app before the change:
1. Try it on a copy (`GET /api/profiles` saved to a file, or the file copied): `profiles-reshape.mjs
   --app <before> <copy> <out.json>` prints each profile's changes and refuses unless the app before
   and after the change read the result as the app before read the original; read the diff.
2. With the owner's go, before the change is published: `profiles-reshape.mjs --app <before>
   --apply <profiles.json>` on each Mac. It keeps the original beside the file,
   `profiles.json.before-reshape`, and refuses while one is there or when the file changed as it ran.
3. Undo if needed: `--restore <profiles.json>` puts the backup back.
4. Last, delete the backup: `--drop-backup <profiles.json>` on each Mac, once the owner has opened
   each converted profile on the published map and confirmed it, and at the latest 7 days after
   step 2 (the daily backup above keeps the map server's list for 30 days).

**Links.** None belongs in the project folder. A link made from a Mac is stored on the NAS as an
XSym file and lists as a file over SMB; only its stat says it's a link. No walk of the NAS's folders
goes through one: each goes down by the listing's type or by an entry's stat, which doesn't follow a
link (`NasEntry::symlink_metadata`). A link made on the NAS itself, which the share may show as the
folder it leads to, can't be told from a Mac: the PM's tools/dev/health.sh asks the NAS over SSH
for any link (of either kind) under `inputs/`, `state/` and `sources/inputs/`, and a top folder's
link to itself, at most every 6 hours, and flags each.
