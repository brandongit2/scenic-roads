# Storage

NAS root: `personal/projects/scenic-roads/`, on the `personal` share. Deletions over SMB bypass its
Recycle Bin.

```
inputs/        the inputs folders (docs/inputs.md): regions/, languages/, translations/, descriptions/,
               heritage/, heritage-matches/, timetables/{feeds,gtfs}/, elevation/, sources/, each
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
map/           <seq>.json.zst: the map's snapshots that apps before task #1119 wrote (hourly, the
               newest three) and stat to tell the NAS answers; written and read by no app since.
               Removed once every Mac's beat says `nas-check-inputs` (coord::beats::NAS_CHECK):
               the map is the lead's log's alone (docs/maplog.md §4.4)
app/           releases/: the live app version's and the one before's, each one archive, <version>.tgz;
               current.json (the live one's manifest: {version, sha256, archive_sha256}),
               previous.json (docs/building.md, The NAS's layout of the app); publish.lock
settings/      profiles.json: read and written by no app since task #1119 (an older map server's): the
               map's server keeps its settings profiles on its own Mac (Backups, below); the PM's to
               remove once every Mac runs a newer app (docs/coordination.md §9)
state/         build/ (the records, their brief, the lead's declaration),
               coord/ (the token, the history's archive (a tar a finished day: the history
               itself is the lead's, on its disk), the forecasts' and forecast inputs' archives),
               pool/nas-probe/ (the NAS budget's probe's own folders: it measures the share),
               live-archive/ (each Mac's live measures, a file a
               day), backups/;
               coord/history/ (the days an app before task #1119 wrote: GC packs each into the
               archive and removes it as it passes the week, the last on 19 Nov);
               coordinator.json (the lead's contact, written for the apps before task #1119 alone,
               read by no app since: docs/coordination.md §9);
               lead/store.json (the lead's store's old backup, which an older lead reads at a take-up
               only to pass its own over: the PM's to remove once the lead runs an app since task
               #1183, which writes it no more, docs/coordination.md §9);
               what no app since task #1119 reads or writes, the PM's to remove once every Mac runs
               one (§9): status.json, pool/nas-probes.json, tmp/ (empty), logs/ (the
               NAS's planet fetch's and an old bld-fetch's logs, of 2 and 6 Oct); taskboard/ (copies
               of the task board's database from its moves, 21 Oct and 5 Nov: the owner's); and the
               old coordination files no app writes (journal/, the terms' folders: §9)
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
  (docs/maplog.md §7.4). One the manifest already names under the same logical name takes one look
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
- **Catalogues** list every file the map reads, with the coverage they were built for and the credits
  of their sources. Readers take the highest that decodes.
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
  tiles, each area overlay) is one pack of every tile, `layers/<layer>/all`, which the catalogue names
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
(`crate::out::check_writer`). Other writers: the owner's inputs folders, `tools/app/publish.sh`, and the
NAS's own planet fetch. The map's server writes nothing on the NAS.

**GC** (`agent::gc`, docs/maplog.md §7) is the lead's `gc` job, planned whenever GC's queue has a
chunk for it or one of its sweeps is due, and run when the NAS is quiet: only beside leased jobs
that leave its measured weight room (pool.md §7.2, `Limits::fits_quiet`; unmeasured the way it runs
now, weighing the whole NAS beside no job weighing the whole, as the lead's own unmeasured NAS steps
go, so its first run measures it), or, held back 24 hours, as an aged job of the lead's own
(`QUIET_AGE`); running, it yields at its next safe point (between
two removals, or two folders) to a target held back for the budget, and the next run goes on where
it stopped.
- **What holds a file:** the map at any `seq` a client still holds, the records (the manifest and
  the pending uploads), and a hand-off the merge has taken until the records after it are seen
  (its changes, its uploads and the files its job reused, `Handoff::reused`). The lead's agent
  queues each content name the map's log or the records stop naming (`agent::gcqueue`,
  `agent/maplog/gc.jsonl`); a name is due once every client's cursor is past the map that freed it,
  an hour has passed (`GRACE_S`) and every live lease was granted after it was freed.
- **By the queue:** each run first takes the due names from the lead's coordinator 200 at a time
  (`POST /gc/chunk`; `gc::CHUNK_NAMES`): the lead checks each against its holders as of then and
  marks the rest deleted in its queue, the newest lease granted then theirs, on its disk before it
  answers; the job removes them one by one over SMB (a file not there counts as gone while the share
  answers, its `map/` folder there; a lost mount fails the run), then says
  them gone (`POST /gc/gone`, the marks' lease renewed to the newest then). The merge refuses a
  hand-off bringing a name being removed, or one removed after its lease was granted: its targets
  are done again (docs/maplog.md §7.4). A removal's mark is kept while any lease as old may still
  be handed off (owed by its member until a beat of its lists it neither held nor handed off),
  however long a member is away. Killed mid-chunk, the next run's first chunk is the same,
  told again (one held by then kept out of it, still marked); yielding mid-chunk, the job says what
  went and lets the rest go back to the queue, unmarked (but a name told more than once, which the
  killed job may have removed, stays marked). Nothing is listed for it. A chunk takes a name in a folder the map indexes, a removed
  step's, or a source of a pass older than the newest complete one (`gc::by_the_queue`); the
  gate's copies are the inputs sweep's below, and the rest of `sources/` is never GC's. The lead
  not reached, or not able to put its marks on its disk, nothing is removed and the job fails.
- **The sweeps**, each a pass with its own cursor in the lead's map log folder
  (`agent/maplog/gc-sweeps.json`, `gc::Sweeps`: the folder each walk reached, so a run stopped
  goes on after it), run after the queue's chunks:
  - **The duties**, once a day: the names to drop (below), the history's and the forecasts'
    archives, the forecast inputs' old days, and the pending
    uploads' check (above), last.
  - **The orphan sweep** (docs/maplog.md §7.5), a pass every 14 days (`ORPHAN_AGE_S`): it lists,
    a folder at a time, the top folders of what the map and the records name, the removed steps'
    folders and the retired passes' (`sources/osm/<date>/`, `sources/items/<date>/` older than the
    newest complete pass), and sends the lead each content-named file at least 14 days old (no job
    runs that long between its upload and its hand-off), which queues those nothing holds, freed
    as it finds them (so a lease older than that holds them, and they wait their grace). So what
    a lost queue held, or a job uploaded and never handed off, goes too. It removes stray `.tmp`
    files after 2 days, and a retired pass's other files (the planet download) once the newer
    pass has been complete 14 days, with its folders left empty.
  - **The inputs sweep**, a pass every 14 days: the input check's checked copies (`sources/inputs/`,
    inputs.md §4.9) are kept while the records' manifest, an accepted index or
    an index of the last 14 days names them (a hand-off not merged yet bringing one removed is
    refused by the merge, its job's work done again: the lead's GC queue, docs/maplog.md §7.4); an index names its files' copies and every content
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
- Never swept: the newest pass, a planet waiting for its pass, the rest of `sources/`, inputs,
  state, app and nas. The packed tile stores (`sources/aws-terrarium/`, `sources/pyramids/`), whose
  archives no manifest names, are kept by their own indexes (macs.md, Downloads), never by GC. A
  tombstone the GC before step 8b left (a file renamed `….gc-tomb` by a run stopped between its
  rename and its removal) is never taken for a content-named file and stays.

**Backups.** Daily, the lead's `backup` job (agentcore::agent::backup) copies `inputs/`, the input
check's acceptances (`state/inputs/accepted/`) and the map's settings profiles (`profiles.json` in
the app's folder on the lead, the map server's Mac: crates/server/src/profiles.rs; listed as
`mac/profiles.json`) into a content-addressed store under `state/backups/`, with a dated list on
days something changed (kept 30 days), mirrored to the lead's own disk (`<home>/agent/backups/`);
`todo/` isn't backed up, and no link is followed or backed up (#1004: a link to its own folder,
`inputs/inputs`, failed the backup on 7 Nov). Everything else can be regenerated. A laptop's server
on demand keeps a list of profiles of its own, in its own app folder, not backed up.

**Links.** None belongs in the project folder. A link made from a Mac is stored on the NAS as an
XSym file and lists as a file over SMB; only its stat says it's a link. No walk of the NAS's folders
goes through one: each goes down by the listing's type or by an entry's stat, which doesn't follow a
link (`NasEntry::symlink_metadata`). A link made on the NAS itself, which the share may show as the
folder it leads to, can't be told from a Mac: the PM's tools/dev/health.sh asks the NAS over SSH
for any link (of either kind) under `inputs/`, `state/` and `sources/inputs/`, and a top folder's
link to itself, at most every 6 hours, and flags each.
