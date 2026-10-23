# Storage

NAS root: `personal/projects/scenic-roads/`, on the `personal` share. Deletions over SMB bypass its
Recycle Bin.

```
inputs/        the drop boxes (docs/inputs.md): regions/, languages/, translations/, descriptions/,
               heritage/, heritage-matches/, timetables/{feeds,gtfs}/, elevation/, sources/, each
               with how/ (how it's made); README.md; keys.env (API keys, KEY=value); hold-catalog;
               _gate-test/ (the input check's test unit)
sources/       osm/<date>/ (planet, filtered, pieces/, sets/, roads/, outlines, reach, pass),
               basemap/ (Planetiler's jar and data), items/<date>/ (facts, pageviews, answers),
               pageviews/, buildings/<release>/,
               trees/, canopy/, aws-terrarium/, rail/, canvec/, fetched/glo30/, terrain-z8-v5-<pool>,
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
  rebuild uploads only what changed; one already there is reused and touched (GC's age rule). One
  the manifest already names under the same logical name takes one look (`Out::put_file`, task
  #449: heritage-sites' slices, hundreds of small files nearly all unchanged): there at its size, it
  is touched and taken; missing, put again; of another size, the put fails (a damaged file is
  removed by hand).
- **Uploads beside the work** (`Out::put_file_later`): slope and terrain z6 tiles, map tiles (`pack`)
  and the 3D buildings' tiles upload a target's files on a thread of their own while the job works
  on its next target, and a z6 tile its grids and base pack while it makes its road values. A file is
  named in the manifest only once it has landed; a target is saved and noted done only after its
  uploads have (`save_upto`); at a safe point with the build pausing, the target uploading is
  landed and noted done, then the job stops. Its cost line counts its own uploads' bytes. A put,
  set or removal of a name still uploading waits for that upload first. An upload that fails
  transiently is tried again after 2, 5 and 15 s; one that fails or doesn't land within a minute
  and a second per 2 MB fails the job, the target not noted done, and every later save with it.
- **Catalogs** list every file the map reads, with the coverage they were built for and the credits
  of their sources. Readers take the highest that decodes.
- **The build manifest** (`state/build/manifest.json`) lists everything built; **job keys**
  (`state/build/jobs.json`) say what each output was made from. These and the unverified uploads
  (`pending.json`) and the files' sizes (`sizes.json`) are written by the lead from its term's records
  (`state/build/term/<E>/records.json`, the truth: docs/building.md §2, The pool).

**Copies carry the bytes and permissions, nothing else** (`store::sys::copy_data`; `cp -X` in
scripts): the share refuses a copy that sets a `com.apple.provenance` attribute differing from its
folder's, so `std::fs::copy` or plain `cp` fail with "Permission denied".

**Packs, never one file per tile.** SMB manages about 80 random reads per second per file.
- **Our layers** use packs: a header and meta, the blobs (identical blobs stored once), then an index
  of (tile key, offset, length, raw length, 64-bit hash) sorted by key. Root: z0–2; lo: z3–8 per z3
  tile; hi: z9–14 per z6 tile. Hi tiles exist only near the coverage for the layers built per z6 tile;
  worldwide layers (labels, water) have them wherever they have tiles.
- **Bundles** (docs/formats.md): a small layer one job makes whole (each landmark kind's thinned
  tiles, each area overlay) is one pack of every tile, `layers/<layer>/all`, which the catalog names
  for each pack key it holds; the landmark points (a sectioned file per z6 tile) are one bundle,
  `markdata/all`. Each file made on the NAS costs a second or so of round trips: the marks and
  overlays jobs made about 450 and 380 a run before (task #449).
- **The basemap** is Planetiler's worldwide PMTiles file, one per pass, served tile by tile.
- **Base packs, hidata, markdata and ovdata** are sectioned files: mapped when local, read from the
  NAS in pages (docs/macs.md).

**Writers.** Every job uploads its files (content-named) and hands its record changes to the journal
on the NAS, which the lead merges; no step writes the records directly once a term exists
(`crate::out::check_writer`). Other writers: the owner's drop boxes, `tools/app/publish.sh`, and the
NAS's own planet fetch.

**GC** runs daily, in the agent, when the NAS is quiet: only beside leased jobs that leave its
measured weight room (pool.md §7.2, `Limits::fits_quiet`), or, held back 24 hours, as an aged job
of the lead's own (`QUIET_AGE`); running, it yields to a target held back for the budget.
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
