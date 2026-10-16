# Inputs: every input through a standard shape and a checked drop box

**Status: the input check (§4) is built (#134: crate::inputs; its two-Mac trial of §4.10 ran on
2026-10-10), with the regions, the languages, the translations and descriptions, and the heritage
registers and matches its units (#135–#138, §6.1–§6.5: built); the accessor's guard (§7.1) is
built and growing with each input; the rest is planned.** It is the spec for the input standardisation series on the task board (#133 is this
document; #134–#147 build it). Each section says what of it is built; where it describes today's
code or today's NAS, it says "today" and cites the file; everything else is the design.
Companions: `docs/plan.md` (the pipeline, keys and order, which these tasks change),
`docs/formats.md` (files), `docs/pool.md` (the lead, jobs, the journal), `docs/phase5.md` (the
heritage chain).

**The idea in one line:** every input the map is built from has one written shape and one folder on
the NAS (its drop box). The owner makes inputs however they like, to that shape, and drops them
there. The build checks each change before taking it in, keeps the last good version while a
change is held, and reads only checked copies, named by their content, so builds stay deterministic.
No data source is named in the app's code: per-source conversion happens outside the app, and what
the app must know about a source (where to fetch it, its licence, its credit) is written in a
declaration file in a drop box.

**The input check** (this document's name for the mechanism of §4). Every change dropped into a
drop box is checked before the app uses it: first its shape, then the input's own rules. A change
that passes is copied into the app's own store of checked files (`sources/inputs/`) and becomes
the version the build reads. A change with a problem is held back: the app keeps using the last
version that passed, shows the problem on the menu bar icon, the build page and the map's build
panel, and takes the change in once it is fixed or, for a warning the owner agrees with, once the
owner accepts it. Each drop box is checked on its own and is called a **unit** of the input check.
(In the code the mechanism's identifiers keep the older word "gate": `gate_work`, `_gate-test`.)

## 1. Requirements

The owner's decisions (the board's notes for #133–#147; the regions' as revised on 14 October 2026):
1. **A standard shape and a drop box per input.** The owner produces inputs to the shape and drops
   them in; the agent ingests them; after any building they show on the map.
2. **Every change is checked first:** its shape, then the input's own checks (a region naming
   an OSM relation that doesn't exist, for instance). An error or a warning pauses ingesting it, with a warning on the menu bar
   icon and a banner on the build page. The last good version stays in use. A warning the owner
   agrees with can be accepted.
3. **Three kinds of input:** request–fulfil, declare, and source declarations (§2).
4. **Per input** (§6): regions as a file each, built from OSM relations and GeoJSON
   shapes with set operations, lines and buffers, pinned to an OSM snapshot; one
   timetable input worldwide (declared GTFS feeds the app fetches, and static GTFS the owner
   compiles by hand); heritage registers as one GeoJSON file each plus a description file, located
   before they're dropped, with merging by rule in the build and the owner's same/not-same pairs as
   an input; elevation as a declaration per dataset, any resolution, ranked and blended, FABDEM the
   worldwide fallback; languages by territory as a declared input.
5. **No source hard-coded in the app.** Per-source code and hand lists move out to the producing
   side. Credits and licences come from the inputs' description files into the catalog's credits.
6. **The legacy pipeline** (`make run`, `make dev`) is removed (§10: it already is; what's left of
   it is listed there).

Standing constraints this design keeps:
- **Determinism** (plan §8): the same inputs give the same bytes. Every input reaches a job's key by
  content (a content name or a digest of its bytes), never by a path, a size or a time. One
  exception can't be closed: sources read in place, a range at a time, and never downloaded whole
  (3DEP's `current/` GeoTIFFs and HRDEM's cloud-optimised mosaic: `range` sources, §5.2) can change
  under the same URL, and their bytes aren't all read, so they can't be pinned by content. Their
  keys name what the server says of each file read, its ETag (else its Last-Modified and length),
  recorded when it's first read and checked on every read after (a file whose ETag changed is read
  as a new version, and the steps reading it are stale). That's as close to a pin as the source
  allows, and it's said so wherever such a source is declared.
- **No hand-kept figures that go stale** (the owner's rule of 10 October): a declaration states
  facts about its source that can't be read from the data (its licence, its height reference,
  where it is), and everything that can be read from the data (resolution, coverage, extent,
  feature counts, a feed's box) is read from it, at check time. Where a declared fact can be
  checked against the data, the input check does so, and a wrong declaration fails loudly as a finding;
  it never corrupts output silently. A hand-entered value may at most be a hint that costs speed
  when wrong, never correctness.
- **Docs say what's true** (this file was planned throughout; each task marks its part built as it
  lands, and plan.md, formats.md and the diagram follow, #147).
- **The NAS's limits** (plan §3; pool.md §3): listings take 3–33 s a folder under load and small-file
  creates manage about 20–55 a second over SMB, so no input is stored or read as thousands of small
  files, and nothing lists a drop box in a tight loop.

## 2. The three kinds of input

| Kind | What the owner drops | Who decides what's needed | The app's side | Missing data means |
|---|---|---|---|---|
| **Declare** | the data itself, in the input's shape | the owner | reads it as given; the set of files is the whole input | nothing there: the map has none of it (no region, no register) |
| **Request–fulfil** | answers to the app's requests | the app, by a to-do list it writes into the drop box's `todo/`, with a brief | reads the answers; lists again what's still missing after each build | the map falls back (a name without English shows alone) and the request stays on the list |
| **Source declaration** | a small file saying where the data is and in what standard format, its licence and credit | the owner | fetches the data itself, keeps each fetched version content-named on the NAS, refreshes it on its rule | the source isn't used (a feed not declared isn't fetched) |

Precisely:
- **Declare.** The drop box's files are the input's whole truth. Adding a file adds data, editing
  it changes data, removing it removes data (once the removal passes the input check). The app never
  writes in a declare drop box.
- **Request–fulfil.** Two halves. The request: a to-do list the app writes after building (in the
  drop box's `todo/`, never an input itself), each entry with what's needed and why, sorted by
  priority, beside a brief (`todo/README.md`) saying how to answer. The fulfilment: answer files the
  owner drops, which the app reads like a declare input; an answer may answer an entry no list has
  any more, and that's fine. Translations and descriptions work this way today (plan §7). The
  timetables' to-do (§6.7.3) is answered in a declare box (hand-compiled GTFS): its request half
  is a to-do list, its fulfil half is the static GTFS input.
- **Source declaration.** The drop box holds declarations, not data. A declaration names a
  standard format (GTFS, GeoTIFF, a tile pyramid in a named encoding, GeoParquet, …), where to get
  it (URLs, a URL template, a bucket prefix, or files beside the declaration), and its description
  (licence, credit). The app fetches what it needs, when its rule says, into `sources/`
  content-named, and the version a build used is pinned in the records, so a rebuild with the same
  pins is byte-identical. A declaration may carry its data with it (files in a folder beside it):
  a small elevation dataset dropped as GeoTIFFs is a declaration whose data is local, checked and
  stored the same way.

What's not an input: `inputs/keys.env` (secrets: the API keys feeds use; their names enter keys,
never their values, as today), `inputs/hold-catalog` (a control), the to-do lists, and the notes on
how each input is made (`how/`, §8).

## 3. The drop boxes on the NAS

The NAS project folder is `personal/projects/scenic-roads/` (plan §3). Every drop box is a folder
under `inputs/`:

```
inputs/
  README.md                  how the drop boxes work, and each one's shape in brief (#147)
  regions/                   declare: <id>.toml and <name>.geojson, in any subfolders; pins.toml (§6.1)
  languages/                 declare: territories.tsv, about.toml (§6.2)
  translations/              request–fulfil: **/*.jsonl; todo/ (the app's) (§6.3)
  descriptions/              request–fulfil: **/*.jsonl; todo/ (the app's) (§6.4)
  heritage/                  declare: <register>.geojson + <register>.toml, one pair per register (§6.5)
  heritage-matches/          declare: *.jsonl, the owner's same/not-same pairs (§6.5.4)
  timetables/
    feeds/                   source declarations: <feed>.toml, <catalogue>.toml (§6.7.1)
    gtfs/                    declare: <name>.zip, hand-compiled static GTFS; todo/ (the app's: lines.jsonl, README.md) (§6.7.2, §6.7.3)
  elevation/                 source declarations: <dataset>.toml, its files (if local) in <dataset>/ (§6.8)
  sources/                   source declarations for everything else the build reads (§6.9)
  keys.env                   not an input (secrets)
  hold-catalog               not an input (a control)
```

Each of these folders is one **unit** of the input check: checked, held and accepted on its own (timetables'
two folders are two units of one input). Inside any drop box:
- `todo/` is the app's (requests), `how/` is the owner's notes on how the input is made, pointing to
  its producers in the repository (`producers/<input>/`, §8); neither is read as input.
- Names starting with `.`, `@`, `#` or `_` are ignored (the Mac's and the NAS's own files: `@eaDir`,
  `#recycle`, `.DS_Store`; and the owner's drafts, `_draft.toml`).
- A file is taken only once its size and time have held for 10 s (as translations are today), so a
  file being written isn't checked half-written.
- Anything else that isn't the drop box's shape (a stray `.csv` in `heritage/`) is an error
  finding, not silently skipped: a file the owner meant as input must never be ignored quietly.

What moves (each in its task; §11):
- `translations/` and `descriptions/` move from the NAS root into `inputs/` (#137; decided by
  default, §13). Their `todo/` lists move with them. `inputs/descriptions/` exists today, holding
  only `briefs-2026-09/` (the writers' briefs): that folder moves to `inputs/descriptions/how/`
  first, so the drop box starts with nothing but answers. The move is made while no translator or
  writer is running (they're run on request: plan §7), checked by listing the old folder for files
  newer than the move's start; and from then on a file appearing in the old `translations/` or
  `descriptions/` is an error finding on the new unit ("dropped into the old folder: move it to
  inputs/translations/"), never silently ignored. The old folders are then left empty, not removed,
  so a drop there is seen.
- `inputs/outlines/` (`.poly` files, Geofabrik's and three of our own) goes, each file converted
  to GeoJSON in `inputs/regions/` first: no recipe uses one today (§6.1).
- `inputs/ferries/` became timetable declarations and hand-compiled GTFS (#140, #141); its
  research notes and feed-finding notes move to `inputs/timetables/*/how/`.
- `inputs/names/analysis/` (how earlier translation work was sized) moves to
  `inputs/translations/how/` (#137).
- Per-source code moving out of the app goes to `producers/<input>/` in the repository (§8), outside
  what `publish.sh` ships and outside the guards' scans (§7).
- `sources/registers/legacy` became heritage registers in `inputs/heritage/` (#138);
  `legacy-seeds` is made by the build instead (§6.5.3, #139). Both archives are the PM's to delete
  from the NAS.
- The checked copies live under `sources/inputs/` (§4.6); they're the app's, never the owner's to
  edit.

## 4. The input check (#134)

**Built** (crate::inputs; `scenic-build inputs`; the agent's `gate_work` and `tend_gate`), checking
the languages (§6.2), the translations and the descriptions (§6.3, §6.4), the regions (§6.1), the
timetable feeds and the static timetables (§6.7.1, §6.7.2), the heritage registers and matches
(§6.5, §6.5.4), the elevation datasets (§6.8) and, while it's on, the test unit `_gate-test`
(§4.10); each other input joins `inputs::UNITS` in its own task (#142, #144, #145). A unit with
nothing dropped yet and no records isn't checked nor shown. What of this section isn't built yet is
marked planned.

### 4.1 Terms

- **Drop box:** the owner's folder. The build never reads it directly once its input goes through
  the input check; it reads the accepted version.
- **Candidate:** the drop box's files as they are now, once they've held still.
- **Finding:** what a check says about a candidate: an **error** (the file can't be used: a shape
  broken, a field missing, a reference to nothing) or a **warning** (usable, but likely a mistake:
  a register losing a fifth of its entries). Each has a stable id
  (§4.4), the files it's about, a message in plain words, and where it is when that's a place.
- **Held:** a change with an unaccepted finding. It isn't taken in; the last accepted version of
  the files it touches stays in use.
- **Accepted version:** the files the build reads: checked copies, content-named, listed by an
  index whose content name is the input's version (§4.6).

### 4.2 Noticing a change

- The lead (pool.md §5) lists every drop box (recursively for regions, translations and descriptions,
  skipping `todo/` and `how/`) and its acceptances every two minutes, off its loop (a thread of its
  own, crate::inputs::watch, as its journal listings are: a listing can take half a minute under
  load), and keeps the last listing: each file's path, size and modification time. (A Mac working
  as a member doesn't list.)
- `scenic inputs check [<unit>]` lists now (an ask to this Mac's agent, `inputs-asks/` in its
  folder, as an acceptance asked of it is).
- A file whose time is within the last 10 s is left for the next listing (listed again 11 s
  later), by its time alone (the same size and time in two listings a few seconds apart don't prove
  it whole). The check is given the listing and acceptances its key was made from (`--listing`,
  `inputs::Planned`), and checks exactly them: a file whose size or time is no longer the listing's
  (changed since) is left as accepted, or out while new, and so is one the listing passed over as too
  recent (never taken as removed). So the key recorded never names a file the check didn't take as
  listed, and the next listing, which differs, checks it again.
- The listing is only a trigger. Whether anything changed is decided by content (§4.3): touching a
  file, or rewriting it with the same bytes, changes nothing downstream.
- **What gets hashed:** only the files whose size or time differ from those the last check listed
  (the check's own state, `sources/inputs/<unit>/@listed.<hash16>.json`, named in the records as
  `sources/inputs/<unit>/@listed`, apart from the index so the version's name is its contents'
  alone: a held change's file keeps the accepted version's time, so it's read again; a file only
  touched takes its new time, so it's read once; and the index the last change of version
  replaced, `replaced`, which GC keeps while this state is recent, §4.9). A file whose size and
  time are as they were is
  taken as unchanged without reading it, so a check of
  translations (808 MB on the NAS on 2026-10-10) reads only the files dropped or edited since. A
  file rewritten in place with the same size and time is the one case this misses; `scenic inputs
  check --full <unit>` hashes every file, and the lead runs a unit's check in full once a day as a
  backstop (a job `inputs <unit> full`, its key the normal one's with "full": the normal check runs
  once after it, reading only what changed). The day counts from the last full check's time in the
  records (the check's state, `full_at`), whichever Mac led then, so a new lead doesn't repeat one
  less than a day old.

### 4.3 Checking: a job like any other

Checking runs as a build step, `inputs`, one target per unit (`inputs regions`, `inputs
timetables/gtfs`, …), so it fits the pool unchanged (pool.md §2, principle 4: every job hands off):
- **Its key** (`inputs::check_key`): the step's version and the unit's checker version
  (`INPUTS_V`, and a version per input's checks, bumped with any change to them), the listing
  (names, sizes, times: a trigger), the acceptances for the unit (§4.5, by file name), the current
  accepted index's content name, and the content names of whatever else its checks read
  (`Checks::context`: the pass's outlines for regions, languages and the registers' territories;
  the accepted heritage registers for the matches). When the key changes, the target
  (`inputs/<unit>`, kept with the lo keys) is stale and the lead runs it. (A check that takes a
  change in names a new index, so the key changes once more: the check after it reads nothing and
  changes nothing.)
- **Where it runs:** the lead's two slots, by the steps table's row: its first slot, first in the
  plan's order, or its second slot beside the first slot's job, the second job's first choice
  (`beside`), whenever the table lets a second job run beside that one: never beside a step that
  runs alone (the OSM pass and the pass's other worldwide steps, the water, GC: a check waits for
  those to end), nor beside another step of its group of heavy NAS readers (the 3D buildings'
  `bldprep`), nor beside another check. Other members taking it is planned: the coordinator's
  offers are made with the pass's region work today (crate::agent::region_work), which a check
  can't wait for. It holds no region and no map update.
- **What it does:**
  1. Hashes the files the listing says may have changed (§4.2; BLAKE3's hash16, as content names
     are made: formats.md, Names and hashes). A file whose bytes are those of the accepted version
     is unchanged, whatever its time.
  2. Runs the shape checks on each changed file (parse, required fields, types, references), then
     the input's own checks (§6), which may read the whole candidate and the context inputs.
  3. Works out each changed file's verdict: **clean**, or **held** by its unaccepted findings
     (a finding about several files holds the changed ones among them; a removal is a change too).
     Files that only make sense together are held together: a register's `.geojson` with its
     `.toml` (§6.5), a recipe with a shape file it names (§6.1), a feed declaration with one that
     `replaces` it. A file naming something that's only in a held file is held too (a `replaces` or
     a heritage match naming a feed or entry that only a held file adds).
  4. Assembles the next accepted version: the previous accepted version with every clean change
     applied (new and edited files in, removed files out), the held files as they were accepted
     before (a held new file is simply absent).
  5. **Checks that version whole:** runs the unit's checks again on exactly the files it would be.
     Per-file holding can combine an old file with a new one in a way neither candidate nor the old
     version had (a heritage match kept old beside a register's new version that no
     longer has the entry it names). If the whole
     version raises a finding neither accepted nor raised by the accepted version as it stands (that
     one holds no change: the version already in is never held), every changed file of the unit is
     held and the version stays as it was; the banner says the changes were held together and why.
     If it's clean, it's the next accepted version.
  6. Stores each file content-named under `sources/inputs/<unit>/` (§4.6), uploaded as every output
     is (read back, checked against its hash, renamed into place: plan §3).
  7. Writes the candidate's report (its findings, which files are held, and every finding id it
     raised) and hands off the records changes: `sources/inputs/<unit>/@index` → the new accepted
     index (only when it differs; the index it replaces is touched, so GC keeps that version
     restorable, §4.9), `sources/inputs/<unit>/@listed` → the sizes and times listed (only when they
     differ), and `sources/inputs/<unit>/@held` → the report
     (`sources/inputs/<unit>/@held.<hash16>.json`), or none when nothing is held. (A `@` starts no
     component of a drop-box path, nor of a unit's name, so a unit's records are never confused
     with a copy, nor with a nested unit's, `timetables/gtfs` among them. Those names, not
     `inputs/<unit>`: a records entry's content name is its logical name's, which the lead checks of
     every journal entry it merges, pool.md §7.3.)
- **Derived facts** go in the index with each file (a register's box and entry count, a GeoTIFF's
  resolution and extent, a feed declaration's derived box once fetched; the test unit's line count):
  computed from the content at check time, so the steps that need them read the index instead of every file, and a fact is a
  function of the content, never typed by hand.
- **Deterministic:** the next accepted version is a function of the candidate's bytes, the
  previous accepted version and the acceptances. A re-run gives the same index.

### 4.4 Findings and their ids

- A finding's id (`Finding::new`) is `<check>.<hash16>`: the check's name, then a hash of the
  check's name, what it's about by identity (the region ids, the register and entry id, the feed
  id, a file's path), and what was found (a dot, not a colon: the id names the acceptance's file on
  the share):
  - for a place, its geometry rounded to 100 m;
  - for a count (a register losing entries, a share of `approx` entries), the counts themselves and
    the content names of the two versions compared, so accepting "fr-merimee: 46,210 → 39,100
    entries" doesn't accept a later truncation to 4,000;
  - for lines of a file (a translation file's suspicious lines), the flagged lines' own content, so
    an accepted warning covers those lines exactly, and a new suspicious line in the same file (or a
    flagged line edited) is a new finding.
  Not of the whole file's bytes: an unrelated edit elsewhere in a file doesn't re-raise a warning
  already accepted, and the same finding seen again has the same id.
- **Errors** are the data's or the shape's faults: the file can't be read in the shape, a required
  field is missing or of the wrong type, an id repeats, a reference names nothing (an OSM
  relation the pinned snapshot doesn't have, a shape file that isn't there), a declared fact the data
  contradicts in a way that can't be meant (a declared height reference the app doesn't know, a
  GeoTIFF whose CRS contradicts its declared format). Errors can't be accepted; the file is fixed or
  removed.
- **Warnings** are likely mistakes the owner may mean: a register
  losing more than 10 % of its entries since its accepted version, entries far outside their
  register's declared territory (NRHP lists the American Legation in Tangier), a declaration no
  step will use.
- Findings about many lines of one file (a translation file's suspicious lines) are one finding
  per file and check, listing the lines (the first 50 in the banner, all in the report).

### 4.5 Accepting a warning

- **How:** the build page's banner has Accept beside each warning and Accept All for the unit's
  warnings (§4.7); the map's build panel, the menu bar item and `scenic inputs accept <unit>
  <finding id>|--all` do the same. Errors have no Accept. Built.
- **What it writes:** `state/inputs/accepted/<unit>/<finding id>.json`, made with create-new (pool.md
  §2, principle 3: written once, never changed), holding who accepted it (member id, host label),
  when, how it was asked, and the finding's message as it was. The asking member writes it itself
  (`scenic inputs` directly; the menu bar and the map through an ask to their Mac's agent, the
  build page through the lead's coordinator to the lead's agent); no round trip to the lead before
  it's written. A member's agent then tells the lead's coordinator (`/work/inputs {unit, check}`),
  so the lead lists the drop boxes and acceptances at once rather than at its next listing (if it
  can't be reached, within two minutes all the same); the listing changes the check's key, the
  check runs again, and the file is no longer held.
- **Undoing:** `scenic inputs unaccept <unit> <finding id>` removes the file; the finding holds its
  file again from the next check (if the file has moved on since, it holds the new change, never
  the version already in).
- Acceptances are backed up with `inputs/` (plan §3, Backups).
- An acceptance outlives the finding: if the same finding appears again later with the same id, it's
  already accepted. `scenic inputs` lists acceptances whose finding no candidate raises any more,
  and `scenic inputs unaccept --stale` removes them.

### 4.6 The accepted version and how steps read it

- **Files:** each accepted file is `sources/inputs/<unit>/<path>.<hash16>.<ext>` (its drop-box path,
  slashes kept, content-named as every built file: plan §3), so the same bytes are stored once
  however often they're dropped.
- **The index:** `sources/inputs/<unit>/@index.<hash16>.json`:
  ```json
  {"fmt": 1, "unit": "heritage", "checks": "heritage 1",
   "files": {"fr-merimee.geojson": {"file": "sources/inputs/heritage/fr-merimee.4b1e…a0.geojson",
                                   "size": 81234567, "keyed": "9c03…e1",
                                   "facts": {"box": [-5.14, 41.37, 9.56, 51.09], "entries": 46210}},
             "fr-merimee.toml": {"file": "sources/inputs/heritage/fr-merimee.77d2…10.toml", "size": 512,
                                 "keyed": "-", "facts": {}}},
   "accepted": {"h-loss.9e41…c2": ["fr-merimee.geojson"]}}
  ```
  `keyed` is the digest of what affects builds (§5.1: a description file's credit and licence
  don't, so editing a credit reruns no build step; it reaches the catalog through the credits'
  digest, §4.8); `accepted` maps the warnings the version was taken with to the files they're about,
  each kept while those files are in the version unchanged (a file edited or removed drops its
  warnings: their acceptances are then stale, §4.5); a warning that let a change in counts while
  the version it was taken with is current, a removal's too, its file gone with it. No file time is in it (§4.2: `listed`), so the
  same contents always have the same version.
- **The records:** the manifest's `sources/inputs/<unit>/@index` names the index, whose file
  entries root the files for GC (§4.9). It changes only by a check job's hand-off, merged by the
  lead (pool.md §5): the lead stays the one writer of the records, and the input check adds no
  other writer.
- **Readers:** one function in the pipeline (`inputs::open(root, manifest, unit)`, built) gives a
  step the accepted files of a unit, by their drop-box paths, from the records it planned with. No
  step reads a drop box (the accessor and the guard enforce it, §7). The Python steps get the
  files' paths on their command line, from the agent, as they get every input today. (No step reads
  a unit yet: each input's task moves its readers here.)
- **Every read of a drop box today** moves to it in its input's task. Besides the regions' readers
  (§6.1), the ones that would bypass the input check if missed: `scenic-build`'s `coverage_of`
  (crates/pipeline/src/bin/scenic-build.rs:3352, defaulting `--regions` to `inputs/regions`); the outline files read by the agent's
  regions digest and coverage key (agent/mod.rs:652, :4213); the backups'
  folders (agent/backup.rs: `inputs` and `state/inputs/accepted`); GC's `NEVER` (agent/gc.rs, §4.9); `dem/bldfetch.py:248–250`
  (`inputs/outlines`). Each is in the guard's exemptions (§7.1), naming its task.
- **Keys:** a step names the content of the input files it reads: the index's content name when it
  reads the whole unit, or the `keyed` digests of the files it reads when it reads some (the
  heritage-sites job names the registers whose box meets its cover, so a register on another
  continent changing reruns nothing there: built, #138, §6.5). Planned for the other inputs, with
  their readers. This replaces today's digests of `inputs/` beside the
  manifest (`agent::input_digests`, crates/pipeline/src/agent/mod.rs:4638, which digests the
  recipes and the outline files they name by size and time: `regions_digest`, :4671).
- **The map's server** reads its inputs the same way: the translations and descriptions from the
  accepted files the lead's manifest (`state/build/manifest.json`) names, copied to the Mac
  (crates/server/src/accepted.rs, pipeline::inputs::served: built, #137), and away from the NAS
  from that copy; the regions (the Regions panel's list and states) from the drop box's region
  files, read only, and the accepted version and held report (built, #135). `names-todo` reads the translations' and
  descriptions' accepted versions the agent names (their indexes, in its key; built, #137).
- **The catalog** records the accepted index of every unit its data was built from
  (`"inputs": {"<unit>": "<index content name>"}`), so what a published map was made from can be
  read back (planned: with the first unit whose data reaches the map, #135).

### 4.7 Where it shows

- **The status** (`state/status.json`, plan §8, Status): `inputs`, one entry per unit:
  ```json
  {"unit": "heritage", "version": "sources/inputs/heritage/index.7a…json", "state": "held",
   "checked": 1760100000, "held": ["fr-merimee.geojson"],
   "findings": [{"id": "h-loss.…", "level": "warning", "files": ["fr-merimee.geojson"],
                 "message": "fr-merimee: 46,210 → 39,100 entries (a loss of 15 %)"}],
   "accepted": [], "stale": []}
  ```
  `state` is `ok`, `checking` (a check is planned or running, nothing held) or `held` (also while
  it's checked again: `checking: true` beside it, its findings still shown, until the check says;
  the menu bar notifies "taken in" only once the state is `ok`); `checked` is the time of
  the held report's (or the index's) file on the NAS; a finding shows its first 50 flagged lines
  (`lines`) and how many more the report has (`more`); `accepted` lists the unit's acceptances and
  `stale` those no check raises any more (`together`, why every change was held together, and
  `unread`, why the unit can't be read now, when they apply). Built (crate::inputs::view), the
  lead's.
- **The menu bar item** (`tools/status`): today one icon shows the build's state, and a problem shows
  only when nothing builds (tools/status/main.swift). An input held adds a badge (a small warning
  triangle) to whatever icon the build's state has, so a held input is visible while the build
  goes on with the last good version. Its menu has a line per held unit ("heritage: 1 warning held
  (fr-merimee.geojson)"), with Accept (warnings only, after a confirmation naming them: an ask to this
  Mac's agent, which writes the acceptances) and Show on the Build Page. It notifies when a unit
  becomes held and when it's taken in. Built.
- **The build page** (`web/work/dash.js`): a banner per held unit at the top: the unit, the held
  files, each finding with its message, where it is for a finding with a place (its coordinates:
  the page doesn't know the map's address), Accept per warning and Accept All (`/work/inputs`, to
  the lead's coordinator, whose agent writes the acceptance); errors say what to fix. While a unit
  is checking, a quiet line says so. Built.
- **The map's build panel** (`web/src/ui/buildstatus.ts`): the same banners, compact, a link to
  the place on the map, and a warning sign on the build item (`/api/build/inputs`, an ask to this
  Mac's agent). Built. The Regions panel marks a held region "held: <finding>" (built, #135: the
  panel's states, §6.1.9).
- **`scenic status`** prints a line per unit held or checking; `scenic inputs` lists every unit's
  version, state, findings and acceptances (stale ones marked). Built.

### 4.8 Credits from the inputs

- Every input whose data reaches the map has a description (§5.1) with its `credit` and `licence`.
  The catalog's `credits` (formats.md, Catalog) are made from the accepted descriptions instead of
  `pipeline::rules::CREDITS` (crates/pipeline/src/rules.rs), with the same rule for which to
  list: those whose extent meets the coverage, 20 km around it, or a built unit's ways
  (`rules::listed`). Built beside `CREDITS` (crate::inputs::credits: a description in the index
  is a file's `facts.description`, its extent `facts.extent`, a box or `"world"`; one whose extent
  isn't known yet is listed nowhere); no unit has descriptions yet, so the catalogs' credits are
  the table's until the inputs' tasks add theirs.
- **The extent is derived,** never typed: a register's box from its entries, a GeoTIFF dataset's
  from its files, a feed's from its stops, a tile pyramid's from its territory's outline, a source
  declared for the whole world (`territory = "world"`) as anywhere. Today's credit boxes
  (rules.rs:129–175) go.
- **Credit edits reach the catalog.** Credits enter no build step's key, but the catalog's key
  (`catalog_key`, crates/pipeline/src/agent/build.rs: the served files, the regions, the ready
  list) gains the digest of the accepted descriptions' credit fields (`credits::digest`), so a
  corrected credit gets a new catalog without rebuilding anything. Built, with one difference: the
  digest is of every accepted description, not only those the catalog would list (which needs the
  built units' extents, a plan doesn't read), so an edit to a description listed elsewhere remakes
  a catalog listing the same credits. With no description there's no line, so the key is as it was.
  When the descriptions can't be read now, the last digest the agent read stands (a failed read
  never changes the key), and with none read yet the catalog waits; `scenic-build catalog` fails
  then too, rather than publish credits without them.
- **Popups name their source by register, not by a copied string** (built, #138). The heritage
  outputs carry each feature's register id (`register`; `registers`, for a place merged from
  several), and the map resolves it against the catalog's credits, which list each register's
  credit by id (`register`, crate::inputs::credits `Described::credit`; web/src/overlays.ts
  `sourceOf`), so popups follow a credit edit with the next catalog, no heritage job rerun.
- The map's own credits for what the app makes (its colour ramps, `web/src/ui/strip.ts:330`) stay
  the app's.
- The map server's fallback for catalogs without credits (`credits_of`,
  crates/server/src/main.rs:765, which reads `rules::CREDITS`) goes once no served catalog lacks
  them (GC keeps catalogs 14 days).

### 4.9 GC and backups

- GC never swept `sources/` at all, outside retired passes (`NEVER`,
  crates/pipeline/src/agent/gc.rs). #134 changes that for `sources/inputs/` alone (built:
  `gc::inputs_kept`): a file there goes once nothing names it (the manifest, and with the pool on
  the newest records' own; the accepted indexes, listings and held reports they name, and those of
  any journal entry the newest records don't reflect yet: pool.md §7.3; every index whose time is
  within `keep_days` (`scenic gc --days`, default 14), the check touching the index it replaces;
  and the index each check's state made within `keep_days` names as `replaced`, with its files,
  whatever that index's own time, so a touch that failed loses nothing) and it's older than
  `keep_days` (plan §3, GC). So with the default a version replaced is kept whole, restorable by
  hand, for 14 days after it's replaced, however long ago its files were accepted, and a step
  reading the version it planned with finds its files. GC walks every folder under
  `sources/inputs/`, a unit's name nesting (`timetables/gtfs`). When any of those can't be read,
  nothing there is swept that day. The rest of
  `sources/` stays unswept, and so do the drop boxes and `state/inputs/`.
- **Appending to a big file costs a copy.** Every accepted version of a file is kept whole for 14
  days, so appending a batch to an 80 MB `.jsonl` keeps another 80 MB copy each time. The briefs
  (and `inputs/README.md`) say to drop one answer file per batch instead, which costs only the
  batch.
- Backups (plan §3) cover `inputs/` (every drop box, so translations and descriptions once moved)
  and `state/inputs/accepted/` (agent/backup.rs's `FOLDERS`: built). `todo/` isn't backed up (it's
  remade).

### 4.10 What #134 builds and how it's tried

- The `inputs` step, the listing thread, the index, the records entries, `inputs::open`, the
  acceptances, the status entry, the menu bar badge and lines, the build page's and the map's
  banners, `scenic inputs`; the accessor and its test (§7.1); GC's sweep of `sources/inputs/` and
  the backups' folders (§4.9); the catalog's credits from descriptions beside `rules::CREDITS`, and
  the credits' digest in the catalog's key (§4.8).
  All built.
- **A test input first:** `inputs/_gate-test/` isn't real data (a name starting with `_` is
  ignored everywhere else; the input check is told of it by a flag, `state/inputs/gate-test`: `scenic
  inputs test on|off`; off, it's no longer shown, and `test off --forget` removes its acceptances
  and asks the lead to remove its records, a job of the lead's, `inputs _gate-test forget`, its
  copies going with GC). Its shape: JSON lines `{"k": "<string>", "v": <number>}` in `.jsonl`
  files; its checks (crate::inputs::gatetest): an error for a line that doesn't parse or isn't
  that shape, a warning for a negative `v`, and a warning for a file removed (naming the version
  removed), so a removal can be held and accepted. No build step reads it; its accepted version
  shows in the status. With it, #134 shows on the NAS, between two Macs: a drop taken in; an error
  held with the last good version kept; a warning accepted from the build page and from the other
  Mac's menu; an unaccept; a touch changing nothing; a removal held and accepted; the lead handed
  over mid-check (the procedure: `tools/check/gate-two-macs.sh`; run on 2026-10-10 on the NAS
  between the two Macs, what it showed in its header). Then it's removed.
- **Tests** (plan §8, Tests: no wall clock, no real NAS): the verdicts as a function of (candidate,
  previous version, acceptances); finding ids stable across unrelated edits, and new when a count or
  a flagged line changes; a held new file absent, a held edit keeping the old bytes, a held removal
  keeping the file; paired files held together; a combination of an old held file and a new clean
  one that raises a finding holding every change (the whole-version check); only files whose size
  or time changed being read; the index the same twice; the accessor refusing a drop-box path from
  any other caller (§7.1). Built: crate::inputs's tests (with a test unit of paired files for the
  cross-file rules), the agent's (`the_gate_plans_a_check_first_and_shows_in_the_status`), GC's
  (`the_inputs_copies_go_once_nothing_names_them`) and `crates/pipeline/tests/inputs_accessor.rs`.

### 4.11 Older apps during the changes

Each task changes what an app reads, so for a while two apps may meet:
- **An older lead and the new records.** The records keep the steps' records an app doesn't know
  (`build::Keys::other`, since the app after the 3D buildings: pool.md §6.1, "The 3D buildings and
  older apps"; an app from before them drops what it doesn't know as it saves; the checks' done
  records are kept with the lo keys, which every app keeps). An older lead keeps the
  `sources/inputs/*` entries it doesn't know (the manifest is a map it passes on whole) but plans
  with its own code, reading the drop boxes directly as today: it bypasses the input check for as
  long as it leads.
  Its member's hand-offs of an `inputs` job are refused (not a step it shares), as for the
  buildings.
- **The same-app rule limits the window.** A member gets jobs only from a lead on its own app
  (plan §8, The same app; pool.md §7.1), and a lead that sees a newer app installed restarts into it
  (pool.md §6.1). So mixed apps last from a publish until each Mac's updater has run: minutes while
  the Macs are awake. An owner's downgrade past #134 (`scenic lead take --force --downgrade`) puts
  the build back on the drop boxes until a newer app leads; nothing is lost, as the drop boxes still
  hold everything.
- **An older map server during a move.** A server reads translations and descriptions from where its
  own app says. So each move (#137's folders) goes in two publishes: first an app whose server reads
  both the new accepted copies and, while those don't exist yet, the old folder; then, once every
  Mac's server runs it (the menu bar item's and `/api/catalog`'s app versions say so), the move
  itself. An older server never sees an empty folder where its data was.
- **Content-named copies outlive both apps.** `sources/inputs/` is only ever added to, so an older
  app reading the drop boxes and a newer one reading the copies never conflict; GC's sweep of it
  (§4.9) keeps whatever any record names.

## 5. Shapes every input shares

### 5.1 The description file

Every declare input whose data reaches the map, and every source declaration, carries a description:
`about.toml` for an input that is one dataset (languages), `<name>.toml` beside each dataset for an
input of several (one per register), or the declaration file itself (feeds, elevation, sources).
TOML, as the region recipes are, since the owner writes them by hand.

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `name` | string | no | the dataset's own name ("Répertoire du patrimoine culturel du Québec") |
| `what` | string | no | what the map shows from it, the credit's first part ("Québec heritage") |
| `credit` | string | no | the source as its terms ask it be named |
| `licence` | string | no | an SPDX id where there is one (`CC-BY-4.0`, `ODbL-1.0`, `OGL-UK-3.0`), else the terms' name |
| `licence_url` | string | no | the terms |
| `source` | string (URL) | no | the page it comes from (for a declaration, the page about it; where to fetch is in `[data]`) |
| `retrieved` | date | no | when it was downloaded or checked (the owner's record; nothing ages on it) |
| `territory` | list of ISO 3166 codes, or `"world"` | yes | where it applies, checked against the data (§4.4); `"world"` for a source with no territory (UNESCO's list, OSM, FABDEM). The one field for this in every description and declaration |
| `redistribute` | bool, default true | yes | false: its data never goes into the catalog's World download nor to a worker page (ODPT's raw feeds, plan §6) |
| `notes` | string | no | free text |

`credit` and `licence` are required (an error without them): a source without its credit would
break its terms. (The parser and its checks are built, `inputs::credits::parse_description`; no
unit uses it yet.) "Keyed" fields enter the file's `keyed` digest (§4.6); the others change what the
map says, not its data, as credits enter no key today (rules.rs:12).

### 5.2 Source declarations

A declaration adds, under `[data]`:

| Field | Type | Meaning |
|---|---|---|
| `format` | string | the standard format: `gtfs`, `mdb-catalogue` (the Mobility Database's catalogue CSV), `geotiff`, `tiles`, `geoparquet`, `pmtiles`, `planet-pbf`, `jar`, `zip`… one generic reader each |
| `url` / `urls` | string / list | where to fetch; may be a template with `{z}`, `{x}`, `{y}`, `{tile}` |
| `index` | string | a file beside the declaration listing tiles or files and their footprints (GeoJSON), when the set is large; made by a producer from the source's own listing (§8), checked to parse and to name URLs on the declared host |
| `files` | list of globs | data dropped beside the declaration, in `<name>/` |
| `version` | string | a pinned release where the source has releases (`2026-09-23.1`); the fetch is pinned by content in the records either way |
| `[data.fetch]` | table | how: `method = "get"` (default), `"get-key-header"` (`header`, `key`: a name in `inputs/keys.env`), `"get-key-json-link"` (also `link`: the JSON path of the download link in the answer, as LTA's signed links), `"s3-list"` (a public bucket's prefix, listed), `"range"` (read in ranges where it lies, never downloaded whole: cloud GeoTIFFs) |

The fetch methods are protocols, not sources: generic code, one implementation each, used by any
declaration. Every fetch goes through the existing fetcher (`pipeline::fetch`, which asks before any
internet request: `every_internet_request_asks_first`, crates/pipeline/tests/cache_accessor.rs:195),
and the worker pages' proxy allow-list (`coord::WEB_HOSTS`, crates/pipeline/src/coord/mod.rs:446,
five hosts by name today) is derived from the declarations' hosts.

**Fetched data** goes to `sources/fetched/<declaration id>/…`, content-named, each version pinned in
the records by the job that fetched it; steps' keys name the pins (as the terrain's raw tiles are
pinned). A refresh is a new pin, never a silent change. (The timetable feeds' zips are pinned where
the rail sources keep them, `sources/rail/gtfs/<declaration id>`: §6.7.1.)

**`range` sources can't be pinned by content** (§1): their files are read a range at a time where
they lie, never whole, and may change under the same URL (3DEP's `current/` folder is replaced as
USGS updates it). For each file read, the reading job records the server's ETag (else its
Last-Modified and length) in the records, and the keys of the steps reading it name those; every
range read sends `If-Match` with the recorded ETag, so a file changed mid-build fails the read
rather than mixing versions, and the next plan sees the new ETag and makes those steps stale.

### 5.3 Formats

- **GeoJSON:** RFC 7946, WGS 84 longitude-latitude, a FeatureCollection per file (`.geojson`).
  Polygons' winding isn't required (it's normalised). Invalid geometry (an unclosed ring, a ring
  crossing itself) is an error naming the feature; it's never repaired silently.
- **JSON lines** (`.jsonl`): one object a line, UTF-8; the last line ends with a newline (a last line
  without one that doesn't parse is an error, not ignored as today's server does: the 10 s quiet
  rule keeps half-written files out).
- **TOML** for hand-written small files; **TSV** for tables; **GTFS** as zips.
- Dates are ISO 8601 (`2026-10-09`).

## 6. Each input

Each section gives the input's kind and drop box, its shape, its checks, how ingesting and
rebuilding follow, and how today's data moves into it. **Every migration proves the map unchanged
before the old path is deleted:** the steps reading the new shape give the same output content
names as the old path (by re-keying where the new key names the same inputs, `agent::rekey`, plan
§8, A new key scheme; or by running the step both ways in a scratch root and comparing bytes,
`--expect-same`), or each difference is explained in the task's report. Only then is the old path
deleted.

### 6.1 Regions (#135)

**Status: built (#135, both publishes)**, reviewed by the owner on 15 October 2026 (the decisions in
§6.1.11, three of them against the picks: unknown files ignored, the antimeridian supported, the
app writing no region). Built: the check (crate::inputs::regions: `file`,
`shapes`, `geom`, `resolve`, `stored`, `extract`), the stored geometry, the readers of the accepted
version alone (crate::inputs::regions::read, `Coverage::accepted`), the `regions-extract` job,
folders as the Regions list's groups, the server's panel states and region layer
(`/api/regions`, `/api/regions/layer`) and the map's. The old kinds of part (`geofabrik:`, `poly:`,
`place:`) and the drop box's path for the build are gone (§6.1.10).

**Kind:** declare. **Drop box:** `inputs/regions/`, read recursively. **Unit:** `regions`.

#### 6.1.1 What a region is

A name on the map for a piece of the coverage. The coverage is the union of every region; nothing is
built per region (plan §5), so regions may overlap freely and an overlap costs nothing. The app
checks each region's definition, not how regions fit together: no partition report, no gap warning
(§13 item 19). The map's region layer (§6.1.9) shows overlaps and gaps by eye.

#### 6.1.2 Files and folders

- One region per file, `<id>.toml`, anywhere under `inputs/regions/`. Subfolders group
  (`europe/portugal/azores.toml`): the build ignores them, the Regions panel shows them (below). An id is unique across all
  folders, so moving a file between folders changes nothing (not its id, the coverage or a key).
- Custom shapes are GeoJSON files, `<name>.geojson`, anywhere under `inputs/regions/`, named by
  their file name alone (unique across all folders, for the same reason).
- **Folders are the Regions list's groups** (the owner's requirement, 15 October): the map's
  Regions panel shows the folders as a collapsible tree, nested as on disk, each region inside its
  folder's group and those at the top at the top level, with each group's count; its open or closed
  state is remembered per viewer (in the browser). A group's name is its folder's, or the `name` of
  an optional `_group.toml` in it (for names a folder can't hold, `/` or `:`), with an optional
  integer `order` among its siblings (those with one first, lower first; then by name). It's the one
  `_`-named file the regions take (every other is passed over, §3); a region's id can't start with
  `_`, so it never collides. The check reads it (a TOML that doesn't parse, an unknown field or an
  empty name is an error, `rg-group`); the accepted version carries it (its `facts.group`), and the
  tree is made from the index alone (crate::inputs::regions::read::tree), so the server and the map
  never read the drop box for it. A folder with no region at any depth isn't shown. The grouping is
  the index's alone: moving a region to another folder changes its group, never its id, its stored
  geometry or any key (its geometry file is the same), though the regions' index, and so the
  catalog, is new.
- `inputs/regions/pins.toml` names the OSM snapshot the folder is written against (§6.1.5).
- `how/`, `todo/` and names starting with `.`, `@`, `#` or `_` are ignored, as in every drop box
  (§3). Any other file (a `.poly`, a `.txt`, a `.toml.bak`, a `.toml.removed`) is ignored too: neither checked nor read, nor
  copied into the accepted version (decided by the owner, 15 October: §6.1.11, 5). Only
  `<id>.toml`, `<name>.geojson` and `pins.toml` are the input.

#### 6.1.3 The region file

| Field | Type | |
|---|---|---|
| `id` | string: `[a-z0-9-]`, 1–64 characters, the file's name without `.toml` | required |
| `name` | string, non-empty | required |
| `outline` | list of parts, their union | required, non-empty |
| `minus` | list of parts, their union taken away | optional |
| `intersect` | list of parts, the region kept only inside their union | optional |
| `[lines.<name>]` | a line, from `coords` or `ways`; `<name>` as an id | optional |
| `[pins]` | as `pins.toml`, overriding it for this file | optional |

Unknown fields are an error, so a typo (`minsu`) can't silently do nothing. The region is
((∪ `outline`) ∩ (∪ `intersect`)) − (∪ `minus`), in that order; an empty `intersect` is none.

**Parts.** Each entry of `outline`, `minus` or `intersect` is one of:
- `"osm:<relation>"`: an OSM relation's area (preferred: neighbours share borders exactly). Its
  geometry comes from the pinned pass: its outline set (administrative and ISO 3166 relations,
  plan §5) or, for any other relation, the regions' extract (§6.1.6).
- `"shape:<name>.geojson"`: a Polygon or MultiPolygon in a GeoJSON file of the folder (a Feature, a
  geometry, or a FeatureCollection whose union is taken; RFC 7946, longitude first, WGS 84). No
  tolerance: the shape is exactly what it says.
- `"region:<id>"`: another region's area, as its stored geometry (§6.1.4) has it.
- `{ buffer = <part or "line:<name>">, by = "<distance>" }`: everything within that distance of a
  shape (its inside included) or of a line. Distances: a number and `m`, `km` or `nmi` (1,852 m),
  measured on the ground; positive, at most 200 km.
- `{ cut = <part>, by = "line:<name>", keep = "left" | "right" }`: the part of a shape on one side of
  a line (left or right as the line is drawn, first point to last). Each separate piece the line
  crosses from edge to edge is split and its `keep` side kept; where the line runs on past a piece,
  the extra is ignored; a piece the line doesn't meet is kept whole if it lies on the `keep` side
  (judged by the side of the line, extended straight from both its ends, it lies on), else dropped. A line ending inside a
  piece can't cut it: an error.

`buffer` and `cut` take any part, so they nest (`{ cut = { buffer = "osm:…", by = "12 nmi" }, … }`).

**The coast tolerance.** Today a vertex within 1 km of an `osm:` outline counts as inside
(`OSM_BUFFER_M`, coverage.rs), which takes in coasts drawn on the low-water side. That stays for
`osm:` parts listed directly in `outline` of a region with no `minus` or `intersect`: they're stored
as the relation's rings with a 1 km tolerance, exactly as today (what keeps every key unchanged,
§6.1.10). Anywhere else (in `minus` or `intersect`, under `buffer` or `cut`) an `osm:` part is the
relation's exact area; and a region with `minus` or `intersect` has its direct `osm:` outline parts
grown by a real 1 km buffer before the operations, so its coast is as generous as today's while
subtracting a neighbour removes exactly its area (§6.1.11, 2).

**Lines** are named in the file and used only to cut or buffer, never as outlines:
- `coords = [[<lon>, <lat>], …]`: drawn by hand, at least two distinct points;
- `ways = [<way id>, …]`: OSM ways from the pinned pass (through the extract, §6.1.6) strung together
  end to end in the order given, each reversed if that's what meets the previous one's end; ways
  that don't share an end node are an error naming the pair.

A line used to cut may not cross itself (an error); a buffered one may.

**Examples** (the relation and way ids are illustrative):
```toml
# inputs/regions/europe/portugal/portugal.toml: a country
id = "portugal"
name = "Portugal"
outline = ["osm:295480"]
```
```toml
# inputs/regions/europe/spain-mainland.toml: a country minus its islands
id = "spain-mainland"
name = "Mainland Spain"
outline = ["osm:1311341"]
minus = ["osm:349000", "region:canaries"]        # the Balearic Islands, and the Canaries region
```
```toml
# inputs/regions/europe/portugal/azores.toml: 12 nmi around an island group
id = "azores"
name = "Azores"
outline = [{ buffer = "osm:1629145", by = "12 nmi" }]
```
```toml
# inputs/regions/europe/spain-north.toml: mainland Spain north of a hand-drawn line
id = "spain-north"
name = "Northern Spain"
outline = [{ cut = "region:spain-mainland", by = "line:split", keep = "left" }]

[lines.split]
coords = [[-9.8, 42.0], [3.6, 41.6]]   # west to east, past both coasts: "left" is north
```
```toml
# inputs/regions/europe/portugal/tagus-banks.toml: a band along a river, from OSM ways
id = "tagus-banks"
name = "Tagus banks"
outline = [{ buffer = "line:river", by = "3 km" }]
intersect = ["osm:295480"]

[lines.river]
ways = [24046327, 24046330, 155883810]
```
A `shape:` file:
```json
{"type": "Feature", "properties": {"source": "drawn by hand"},
 "geometry": {"type": "Polygon", "coordinates": [[[-8.2, 41.1], [-8.0, 41.1], [-8.0, 41.3], [-8.2, 41.1]]]}}
```
`pins.toml`:
```toml
osm = "2026-09-29"            # the OSM pass (sources/osm/<date>/) every file here is written against
```
A file's own `[pins]` overrides it (`osm = "…"`), and may name other datasets a `shape:` was derived
from (`gadm = "4.1"`) as a record: the app checks only that such a value is a non-empty string.

#### 6.1.4 Geometry: how it's computed and what's stored

**Coordinates** are integers in E7 degrees (1e-7°, about 1 cm), as every ring in the pipeline
(`Shape::rings`, `[i32; 2]`). GeoJSON is rounded to E7 when read; the pass's outlines and the
extract are E7 already. All set operations run on these integers.

**Set operations** (`outline`'s union, `minus`, `intersect`, a `cut`'s split): the `i_overlay`
crate's boolean operations on its 64-bit integer engine, called directly on the E7 integers (the
`geo` crate's are the same engine behind floating-point coordinates, which would only add a
rounding step). `i_overlay` works in integer coordinates with exact predicates, so it doesn't fail
or leave slivers on the near-coincident edges of shared borders, and the same input gives the same
output on every machine (built single-threaded: no feature that parallelises it). Its 64-bit engine
takes coordinates up to 2⁶² in size, so E7 longitudes shifted by whole turns (below) fit with room
to spare. Results are E7 and put in a canonical form: outer rings
counter-clockwise, holes clockwise, each ring starting at its smallest vertex (longitude, then
latitude), repeated and collinear vertices removed, rings sorted by their first vertex. Equal
geometry is then equal bytes, so the fingerprints that key jobs are stable.

**Buffers** are computed on the ground, not in degrees: each part is densified (no edge longer than
10 km), every edge offset in a local equirectangular projection scaled at its mid-latitude (error
under 0.1 % of the distance up to the 200 km maximum), with round joins and ends whose chords stay
within 5 m of the true arc, and the pieces unioned as above. **Cuts** test sides against the line
extended straight from its ends (§6.1.3) and split by intersecting the shape with each side's
polygon (the line and its extensions closed round a box around the shape).

**The antimeridian** (decided by the owner, 15 October: §6.1.11, 6): regions crossing ±180°
(Fiji, Chukotka, the Aleutians, Kiribati) are supported, and no reader of the coverage has to know:
- **Every stored ring lies within −180°…180°.** A region's result crossing the line is stored split
  at it, its pieces east of the line (longitudes up to 180°) and west of it (from −180°) each a
  shape of its own (`from` ending ` (east of 180°)` or ` (west of 180°)`), so each shape's box
  stays tight (one box spanning the world would make every reader that walks a shape's box walk
  the world). The seam is a plain edge along ±180° in both pieces. So `Shape`, its grid, the
  fingerprints, `shapes_key`, the catalog's drawn regions, the map's layer and `dem/bldfetch.py`
  read plain lon/lat rings as they always have.
- **OSM's relations are split at ±180° already** (OSM draws a boundary crossing the line as ways
  along it on both sides), so an `osm:` part's rings are plain rings, as the outline set holds them,
  and a region of only such parts (Alaska today, `osm:1116270`, the Aleutians beyond 180°) is
  stored unchanged like any other.
- **Computing in a region's own frame.** Before its operations, every part of a region is put in
  one longitude frame: a ring or line whose consecutive points differ by more than 180° in
  longitude crosses the line, and is unwrapped there (the later points shifted by a whole turn, so
  the edge is the short way round); then the region's frame is chosen from all its parts' points
  as the turn that leaves the widest empty band of longitudes outside it (its middle the frame's
  edge), each part shifted by whole turns into it. A region nowhere near the line has the plain
  frame (−180°…180°) and nothing moves. All set operations, buffers and cuts run in that frame
  (64-bit, so a frame past 180° fits), and the result is split at each odd multiple of 180° it
  crosses and every piece shifted back into −180°…180°.
- **Buffers across the line** grow in the frame like any other (the 200 km maximum is far less
  than any empty band a region could leave), then are split as above: a 12 nmi buffer of an island
  at 179.9° E reaches past 180° and is stored as two shapes.
- **GeoJSON** (`shape:`): RFC 7946 asks for geometry crossing the line to be split there; both a
  split shape and one drawn across it (an edge from 179° to −179°) are read, the latter unwrapped as
  above. Hand-drawn lines (`coords`) likewise.
- **Lines** used to cut work in the frame too, so a cut line drawn across ±180° cuts as drawn.

The tests carry fixtures crossing the line: an island group astride it buffered by 12 nmi (two
shapes, each within ±180°, each point within the distance in or out as it should be); a GeoJSON
polygon drawn across it and the same polygon split at it giving the same stored geometry; a
`minus` and a `cut` whose parts lie on both sides; a region whose `osm:` relation is split at the
line stored unchanged; and the coverage's point tests, fingerprints and `shapes_key` on both sides
of the seam.

**The stored geometry** (the accepted version, §4.6): the check resolves every region it accepts and
stores, besides the region files, two content-named files per region:
- `sources/inputs/regions/@geometry/<id>.<hash16>.rgeo`, what the coverage reads: uncompressed (so the
  same geometry is the same bytes whatever compression library a Mac has), the magic `RGEO0001`, a JSON head, then each shape's rings as zigzag varint deltas of E7 points
  (binary because a region's rings are the outline set's at full detail: Nunavut's coast alone is
  millions of points, which JSON would make hundreds of megabytes). The head:
  ```json
  {"fmt": 1, "id": "azores", "osm": "2026-09-29",
   "shapes": [{"from": "{buffer osm:1629145 12 nmi}", "country": "PT", "tolerance_m": 0, "rings": 3, "points": 2210}],
   "box": [-317000000, 366000000, -246000000, 401000000], "names": {"osm": [1629145], "ways": []}}
  ```
  (No name: renaming a region changes no geometry file, so no key.)
- `sources/inputs/regions/@drawn/<id>.<hash16>.json`, what the catalog and the map's region layer
  draw: each shape's polygons simplified for drawing as the catalog's are today (an `osm:` part's
  as the pass simplified it, `Outlines::simple_polygons`; a computed shape by
  `draw_tolerance_m`), GeoJSON coordinates in degrees to 5 decimals, by `from`
  (`{"fmt": 1, "id": "azores", "name": "Azores", "shapes": {"<from>": [[[[lon, lat], …]]]}}`).

`shapes` is in the order the coverage reads them (what `shape_at` and `shapes_key` depend on);
`tolerance_m` is 1,000 for an `osm:` outline part kept with today's coast tolerance, else 0;
`country` as today's `Shape::country` (the relation's ISO code from the pass's outlines; for a
computed shape, its first `osm:` part's, or none); rings E7, unclosed (the last point joins the
first, as `Shape::rings` has them). A region with only `osm:` and `shape:` parts in `outline` and no
operation is stored as **one shape per part, unchanged** (the relation's rings as the outline set
holds them; a `shape:` part's as its file has them, rounded to E7): exactly today's
`Coverage::from_recipes`. Any operation gives one shape, the region's result in canonical form (two
when it crosses the antimeridian, above). The index (§4.6) names each region's files in its
`facts.geometry` and `facts.drawn`, and its `keyed` digest is the geometry file's content name, so an
edit that doesn't change a region's result (a comment, a line renamed) reruns nothing.

**Reading it:** `Coverage::accepted(root, accepted)` (built: crate::inputs::regions::read) reads the index through
`inputs::open(root, manifest, "regions")` and builds the coverage from the stored shapes, regions in
id order, each region's shapes in stored order (today's order is the recipes' file-name order, which
is id order). No reader makes the coverage again from the pass's outlines, a GeoJSON or a region file.

#### 6.1.5 Pins

- The check resolves a region only against the OSM pass its pin names, and only if that is the
  build's current pass. The build reads the stored geometry, so the coverage changes only when a
  region file changes and passes the check, never because a pass came in.
- A pin naming another pass is an error (`rg-pin-mismatch`): those files are held and the map
  keeps the last accepted geometry; nothing rebuilds because of the new pass. The finding lists, for
  every relation the file names (through `region:` too) in the outline set, whether the new pass
  still has it and whether its rings changed (compared point for point with the pinned pass's
  outline set, while the records still name it; "not compared" once that pass is retired), so the
  owner can check the ids and move the pin: "pinned to OSM 2026-09-29; the build's pass is
  2026-12-28. Changed: osm:1629145. Unchanged: 2. Missing: none." (Relations and ways from the
  extract are listed by id, not compared: the new pass's extract is made only once the pin moves.)
- With one folder-wide pin, each new pass holds every region at once until `pins.toml` is edited:
  meanwhile the map keeps its coverage and no new or edited region is taken in either (they fail the
  same check). Moving the pin is the first thing to do after a pass, and the banner says so.
- A held region holds the files naming it (`region:`), §4.3 step 3.

#### 6.1.6 The extract (§13 item 21)

The pass's outline set holds only administrative and ISO 3166 relations (plan §5). Any other
relation a region names, and every way a line names, comes from **the regions' extract**: a small
file of exactly those ids' geometry, made from the pinned pass.

- **What:** `sources/osm/<date>/regions-extract.<hash16>.json`: `{"fmt": 1, "pass": "<date>",
  "relations": {"<id>": {"rings": […], "tags": {…}} | null}, "ways": {"<id>": {"ends": [<node>,
  <node>], "line": […]} | null}}`, `null` for an id the pass doesn't have (so "missing" is a fact of
  the extract, not a failure to make it). Relations' rings are assembled as the outline set's are
  (crates/pipeline/src/outlines.rs).
- **When:** a check needing an id the current extract lacks holds the files naming it
  (`rg-extract`) and says which ids it needs in its held report (`needs`: `r<id>` and `w<id>`,
  with the pass); the lead, reading the report, plans an extract job (`regions-extract <date>`,
  keyed by the pass's date and the sorted id set: those needed and those already extracted; a
  hand-off like every job, pool.md §5) naming the extract in the records as
  `sources/osm/<date>/regions-extract`. The extract's content name is in the check's key
  (`Checks::context`), so the check runs again once it's made. The extract covers every id the drop box names
  and never shrinks within a pass, so removing and restoring a region doesn't remake it.
- **From:** the pass's planet PBF in `sources/osm/<date>/`, read with the pipeline's PBF reader
  (`osmpbf`) in three scans: relations (their member ways), ways (their nodes), nodes.
- **Which file, size and time:** kilobytes per relation or way. The job reads the pass's filtered
  planet first (60 GB for 2026-09-28; it keeps the relations and ways of the pipeline's tags with
  their members, not every relation), and the planet itself (95 GB; GC sweeps a pass's planet only
  once the pass is retired) only for the ids the filtered one lacks: three sequential scans of each
  it reads, a long whole-file read, so the step runs alone on its Mac. Its time is the step's
  measured timing (none yet: no region needs an extract today). Today no region needs one: all 88 recipes name relations in the
  outline set (§6.1.10).

#### 6.1.7 Readers moved to the accepted version

All moved (#135): the table lists them as they were before. Every reader of `inputs/regions` and `inputs/outlines` then (each in the accessor guard's
exemptions, crates/pipeline/tests/inputs_accessor.rs, naming #135). Each moves to
`Coverage::accepted` or the index, and its exemption goes:

| Where | Function | Reads today |
|---|---|---|
| crates/pipeline/src/coverage.rs:516 | `Coverage::load` (its cache, :387) | `recipes::load`, then `from_recipes` |
| crates/pipeline/src/coverage.rs:542, :361–380 | `from_recipes`, `circle`, `file_rings` | `inputs/outlines/*.poly`, `place:` |
| crates/pipeline/src/coverage.rs:698 | `drawn` (the catalog's regions) | recipes and outlines |
| crates/pipeline/src/agent/mod.rs:2232, :2239 | `step` | `recipes::load` |
| crates/pipeline/src/agent/mod.rs:4184 | `region_jobs` | `recipes::load` |
| crates/pipeline/src/agent/mod.rs:4843 | `rekey_records` | `recipes::load` |
| crates/pipeline/src/agent/mod.rs:4886 | `as_read` | `recipes::load` |
| crates/pipeline/src/agent/mod.rs:4936–4954 | `coverage` (the agent's coverages) | `inputs/outlines`, `from_recipes` |
| crates/pipeline/src/agent/mod.rs:5964–5975 | `regions_digest` (in `input_digests`) | recipes and their outline files, by size and time |
| crates/pipeline/src/agent/build.rs:2667 | `catalog_key` | the regions' digest |
| crates/pipeline/src/terrain_task.rs:942 | `live_coverage` | `recipes::load`, `from_recipes` |
| crates/pipeline/src/treepacks.rs:629 | `p5_trees_real` (a proof) | same |
| crates/pipeline/src/bin/terrain.rs:306 | `scan` | same |
| crates/pipeline/src/bin/scenic-build.rs:1222 | `catalog_coverage` | recipes, `drawn` |
| crates/pipeline/src/bin/scenic-build.rs:1388 | `rekey_check` | `recipes::load`, `from_recipes` |
| crates/pipeline/src/bin/scenic-build.rs:1546 | `p5_check_terrain` | same |
| crates/pipeline/src/bin/scenic-build.rs:1682 | `water_key_check` | same |
| crates/pipeline/src/bin/scenic-build.rs:1835 | `p5_check` | same |
| crates/pipeline/src/bin/scenic-build.rs:1938 | `pois_step` (`--regions`' default) | same |
| crates/pipeline/src/bin/scenic-build.rs:2788 | `pass_and_coverage` (`--regions`' default) | same |
| crates/pipeline/src/bin/scenic-build.rs:3476 | `coverage_of` (`--regions`' default) | `Coverage::load` |
| crates/pipeline/src/bld/job_holdout.rs:171 | `holdout` (a test, `$REGIONS`) | `recipes::load` |
| crates/server/src/regions.rs:663 | `coverage_from_recipes` | recipes, `inputs/outlines` |
| crates/server/src/regions.rs:613 | `recorded_features` | `parse_outline` of a catalog's entries |
| dem/bldfetch.py:248–250 | `coverage` | `inputs/outlines/*.poly` |

The `scenic-build` subcommands' `--regions <dir>` (a scratch folder of region files, for proofs)
stays: it runs the same check code in memory on that folder and builds the coverage from the result,
storing nothing. The tests that built a coverage from `place:` or `poly:` recipes (about thirty, in
agent/build.rs, agent/rekey.rs, agent/mod.rs, terrain_pack.rs, slope_pack.rs, treepacks.rs,
rail.rs, reach.rs, candidates.rs, bld/job.rs, bld/task.rs, coverage.rs) use a test-only helper,
`Coverage::fixture`, keeping that notation for their fixtures (a circle of the same 64 points the
old `circle` gave, so their expected values stand; a `.poly` square); the agent's tests put an
accepted version of fixtures in their records (`inputs::regions::accept_fixture`).

#### 6.1.8 Checks

All errors (regions have no warnings, §13 item 19); ids `rg-<check>.<hash16>` (§4.4: a finding id has one dot, as the other units' `la-`, `tr-`), the hash
over the files and ids named.

| Check | Raised when |
|---|---|
| `rg-toml` | the TOML doesn't parse; a field unknown or of the wrong type |
| `rg-id` | `id` invalid or not the file's name; `name` or `outline` empty |
| `rg-part` | a part of an unknown kind (`geofabrik:`, `poly:`, `place:` among them), a malformed `osm:` id, a distance that doesn't parse or is out of range, `keep` not `left`/`right` |
| `rg-dup-id` | two region files with one id, or two GeoJSON files with one name (naming every path) |
| `rg-osm-missing` | a relation or way the pinned pass doesn't have (outline set and extract) |
| `rg-osm-no-area` | a relation whose rings don't close into an area |
| `rg-shape` | a GeoJSON missing, not GeoJSON, not a (Multi)Polygon, or invalid (§5.3: self-crossing, fewer than four points, unclosed) |
| `rg-line` | a line with fewer than two points, ways that don't join (naming the pair), a cut line crossing itself |
| `rg-cut` | a cut line ending inside a piece it meets |
| `rg-ref` | a `region:` or `line:` naming nothing |
| `rg-loop` | a loop of `region:` references, naming every region in it |
| `rg-empty` | a result with no area (a `minus` taking everything, an `intersect` meeting nothing) |
| `rg-pin` | no OSM pin (neither `pins.toml` nor `[pins]`), or one that isn't a date |
| `rg-pin-mismatch` | a pin naming another pass than the build's current one (§6.1.5) |
| `rg-group` | a `_group.toml` that doesn't parse, has a field other than `name` and `order`, or an empty name |
| `rg-extract` | a relation or way not in the pass's outline set that the current extract lacks: the files wait (held) while the extract is made (§6.1.6), then are checked again |

A held region keeps its last accepted geometry; a region new since the accepted version and held is
not in the coverage yet. Removing a region (its file deleted, or renamed to a name the regions
ignore) takes it out with the next accepted version.

#### 6.1.9 What the owner sees

- **The Regions panel** (Settings → Regions) is read only (decided by the owner, 15 October:
  §6.1.11, 11): it lists the regions by folder (the tree above), each with its parts and its state,
  accepted, checking, or "held: <finding>", and shows them on the map. The server reads the region
  files through the NAS I/O pool (`GET /api/regions`; away from the NAS, the last list read) and
  their states from the accepted index and the held report. Nothing in the app writes a region:
  regions change only as files in `inputs/regions/`, which the input check takes in.
- **The region layer** (a switch in Settings → Regions, beside "Coverage on the map"): every
  region's stored outline with a light fill in a colour from its id's hash (so a region keeps its
  colour) and its name as a label, so overlaps (fills mixing) and gaps (no fill) show by eye; a held
  region draws its accepted geometry with a dashed outline. The server reads the drawn files
  (§6.1.4) the lead's manifest's regions index names, copied to the Mac as the translations are
  (crates/server/src/accepted.rs): already simplified, as the catalog's are.
- **The banner and the menu bar** (§4.7): each held region with its finding; after a new pass, one
  finding per file, the banner leading with "move the pin in inputs/regions/pins.toml".

#### 6.1.10 Migration, in two publishes, with a runbook

Today all 88 recipes use `osm:` entries alone, 122 of them, all in the pass's outline set (checked
on the NAS, 2026-10-15: `inputs/regions/*.toml`; two more are `.toml.removed`), and none uses an
operation. So
every region is stored as its parts unchanged (§6.1.4): the build reads the same shapes, in the same
order, with the same tolerance, and so the same `Coverage::fingerprint` and `shapes_key` everywhere.

**Publish 1** (the check, the stored geometry, both read paths):
- The `regions` unit goes through the input check. Every reader in §6.1.7 reads `Coverage::accepted`
  once **the records say the readers switched** (`sources/inputs/regions/@switched`, written once by
  the regions' check job when the migration's proof, runbook step 2, says the same of exactly the
  accepted version the records name then), and **today's path otherwise** (the drop box through
  `recipes::load`), so the coverage is never empty while no version is accepted. The switch is a
  records entry, merged like any other, so no reader decides it from a file it may fail to read;
  the proof's own file is read by the check alone, and only its absence means "not proved" (a read
  that fails or a file that doesn't parse fails the check). The server's region layer reads its
  Mac's accepted copy once it has it, and the catalog's regions until then.
- **Not ready:** while the current pass has no outline set yet (its outlines step pending, #201),
  the check does nothing: no finding, no extract asked for, nothing taken in; the outline set's
  name is in its key, so it runs once it's made.
- `regions_digest` and `catalog_key` take the index's content name once readers use it.
- The check refuses `geofabrik:`, `poly:` and `place:` (`rg-part`); the fallback still reads them.
- The operations, subfolders, lines, `region:`, pins, the extract and the region layer ship here
  and change nothing until a file uses them.
- Older apps (§4.11, minutes): an older lead reads `inputs/regions/*.toml` alone, not subfolders,
  and lists `pins.toml` among its bad recipes without dropping anything.

**Runbook, with publish 1 on every Mac** (the menu bar item's and `/api/catalog`'s app versions):
1. Write `inputs/regions/pins.toml` naming the current pass's date. The check runs; expect no
   findings (any is a recipe today's path takes and the new check doesn't: fix the file or report).
   From here until step 2's switch, **no edits to `inputs/regions/`**: the switch is only for the
   version proved.
2. Proof (publish 1's proof tool, removed with publish 2; it refused while any pool
   member's heartbeat names an app older than its own): build the coverage both ways (`recipes::load` +
   `from_recipes` on the pass's outlines; `Coverage::accepted`) and compare `fingerprint` and
   `shapes_key` on every z6 tile either meets and every job box in the records (`rekey_records`'
   boxes), then plan dry: no build step changes key except the catalog's (its regions digest). It writes `state/inputs/regions-proof.json`
   with the accepted index's name and `same: true`; readers switch then, for good (later versions
   are the owner's edits taken in by the check); a difference writes `same: false`, keeps today's
   path and stops the migration. It compares the coverages' fingerprints and shapes keys on every
   z6 tile either meets and every tile-named target of the records' keys, then the unit keys and a
   dry plan made with each coverage (all other inputs the same), and the catalog's drawn regions
   both ways. Then `scenic inputs check --full regions`: the check records the switch
   (`sources/inputs/regions/@switched`) if the accepted version is still the one proved; `scenic
   inputs` shows it. If an edit got in between, run the proof again. The switch records the app
   the proof ran on: from then on no Mac on an older app leads or is handed the lead (the pool's
   views say "app too old"), and a lead on one builds nothing and says why. **No rollback past
   publish 1 after this step** (`scenic lead take --force --downgrade` to an older app would build
   from the drop box again).
3. Let the lead plan with it: no build step reruns; the catalog is remade once (its key names the
   regions' index now) and lists the same regions with the same drawn shapes (look at the map's
   coverage once, by eye, beside the region layer). **From the switch on, the coverage follows
   `pins.toml`, not the newest pass:** a new pass changes nothing until the pin is moved (§6.1.5).
4. Convert every `.poly` to GeoJSON (publish 1's converter, removed with publish 2): ours
   (`inputs/outlines/azores.poly`, `madeira.poly`, `portugal-mainland.poly`) to
   `inputs/regions/shapes/<name>.geojson`, Geofabrik's 31 (`inputs/outlines/geofabrik/`) to
   `inputs/regions/shapes/geofabrik/<id, / as ->.geojson`, each a Feature naming its `.poly` in its
   properties. Proof, for each: the GeoJSON read back has the same E7 rings as the `.poly` through
   `read_poly`, and a coverage of `shape:<name>` has the same `fingerprint` as one of `poly:<file>`
   on every z6 tile it meets. No region names them, so the map doesn't change; the check validates
   them as shapes.
5. Move `inputs/outlines/` to `inputs/regions/how/old-2026-10/outlines/` (ignored by the check), the
   owner's to delete.

**Publish 2** (once the runbook is done and both Macs' servers have their accepted copy): every
reader reads the accepted version alone. Deleted, with their guard exemptions: the fallback,
`recipes::load` as a coverage path, `Outline::Geofabrik`, `Poly`, `Place` and their parsing,
`file_rings`, `circle`, `regions_digest`, the server's `coverage_from_recipes` and its labels for the
old kinds, and `dem/bldfetch.py`'s read of `inputs/outlines` (the agent's job gives it the coverage
as a GeoJSON file, `--coverage`, made from the stored geometry). With them went the migration's own
tools (the proof and the `.poly` converter), and the proof's record
(`state/inputs/regions-proof.json`, read no more; the records' `sources/inputs/regions/@switched` stays: the readers refuse to read the regions without it, an app installed before the runbook was done saying to do it first, and its proved app still bars older Macs from leading). Built (this app). Catalogs record their regions' shapes inline (formats.md, Catalog
`coverage`), so an old catalog naming `geofabrik:` still draws. The tests move to GeoJSON fixtures
(§6.1.7), and the operations are tested on small fixtures: two touching squares unioned; a minus and
an intersect; a cut by a line running past the shape, by one ending inside it (an error), of a shape
in pieces some of which the line misses; a 12 nmi buffer at 60° N against a geodesic distance; a
buffer of a line from ways, and ways that don't join; a loop of three; an id in two folders; a pin
mismatch holding the files while the coverage stays as it was; the same input giving the same bytes
twice; `region:` of a region with the coast tolerance.

**Done when** every reader in §6.1.7 reads the accepted version's stored geometry through
`inputs::open`, the old kinds and `inputs/outlines/` are gone, the operations are tested as above,
and the runbook's proofs passed (fingerprints and shapes keys unchanged, no build step rerun, the
catalog remade once; each converted `.poly` selecting the same coverage).

#### 6.1.11 Decisions (the owner, 15 October 2026)

The owner reviewed the open questions of this section on 15 October and agreed with every pick but
three (5, 6 and 11), decided otherwise; the section above is written to the decisions.

1. **Geometry library: `i_overlay` (decided)**, over GEOS (a C library to build on both Macs,
   robust only through snapping) or a hand-written clipper. Integer coordinates match the
   pipeline's E7 rings and give byte-identical results, which stable keys need. (Picked as `geo` on
   `i_overlay`; `i_overlay`'s own integer engine is called directly, `geo` adding only a conversion
   to floating point and back: §6.1.4.)
2. **The 1 km coast tolerance with operations (decided):** today's tolerance for plain regions (so
   nothing rebuilds); a real 1 km buffer of the direct `osm:` outline parts of a region with `minus`
   or `intersect`, so a subtracted neighbour is removed exactly. The alternative, no tolerance in
   such regions, trims coasts drawn on the low-water side.
3. **Buffers per edge in a local projection (decided)**, under 0.1 % error at 200 km, over one
   projection per region (wrong for regions spanning many degrees) or exact geodesic offsets (no
   maintained Rust crate does them).
4. **An empty result is an error (decided):** almost always a mistake (a minus of the wrong
   relation). A warning would let the owner keep a placeholder region.
5. **Unknown files are ignored (decided by the owner, against the pick):** a file in
   `inputs/regions/` that's neither a region, a shape nor `pins.toml` is neither checked nor read
   (§6.1.2). The pick was an error (`regions.unknown-file`), so a stray `.poly` or a `.tml` wouldn't
   pass unnoticed; the owner prefers to keep notes and leftovers beside the regions.
6. **The antimeridian is supported now (decided by the owner, against the pick):** regions crossing
   ±180° (Fiji, Chukotka, the Aleutians, Kiribati) work: each region's operations run in its own
   longitude frame and the result is stored split at the line, so every stored ring lies within
   −180°…180° and no reader of the coverage changes (§6.1.4). The pick was to refuse them
   (`regions.antimeridian`) until a task of its own.
7. **One shape per part for plain regions, one per region with operations (decided):** the first
   keeps every key unchanged. One shape per region always would be simpler but changes `shapes_key`
   and reruns the buildings and terrain everywhere.
8. **Stored geometry per region, content-named (decided)**, over one file for the folder: an edit
   stores and copies one small file, and the region layer and `region:` read only what they need.
9. **The extract never shrinks within a pass (decided)**, so removing and restoring a region doesn't
   cost three planet scans; a new pass starts it afresh.
10. **`--regions <dir>` keeps working on a scratch folder (decided)**, through the check's code in
    memory, so a proof can try a region before it's dropped.
11. **The app writes no region (decided by the owner, against the pick):** regions change only as
    files in the drop box, which the input check takes in; the Regions panel lists them, their
    states and their outlines on the map, read only (§6.1.9). The pick was a panel editing names
    and part lists.
12. **Converted shapes in `inputs/regions/shapes/` and `shapes/geofabrik/` (decided)**, plain
    grouping folders like any other.
13. **A colour per region from its id's hash (decided)**, stable everywhere, over a palette assigned
    in order (prettier, but colours shift when a region is added).
14. **The 200 km buffer limit (decided)**: past 12 nmi and any coastal band asked for so far; it
    bounds the buffer's error and the time one buffer can take.

### 6.2 Languages by territory (#136)

**Kind:** declare, generated once and rarely touched. **Drop box:** `inputs/languages/`.

**Built** (#136: crate::inputs::languages, on the input check; the `spoken` job reading the
accepted version). The app holds no table of its own.

**Shape:** `territories.tsv`, one territory a line, `<code>\t<language>,<language>…`, most spoken
first:
- `<code>`: an ISO 3166-1 alpha-2 code (`FR`) or an ISO 3166-2 subdivision (`CA-QC`);
- languages: ISO 639 base subtags (`fr`, `yue`), as `names::spoken::Lang` parses them.
- A subdivision's line refines its country's where they overlap (the producer's `REFINED`); the smallest
  outline wins as today.
- Lines starting with `#` are comments.

Plus `about.toml` (§5.1): CLDR's version, the Unicode licence, its credit. The one rule kept in
code, Chinese read as Cantonese where Cantonese is spoken and Mandarin isn't (`READ_AS`,
crates/names/src/spoken.rs), is a rule of the names' lookup, not data about a territory: it stays.

Both are made by `producers/languages/cldr-languages.py` (§8): from CLDR's `territoryInfo.json`, or
(`--base`) from a `territories.tsv` already made, with the refinements (its `REFINED`: Quebec,
New Brunswick, Spain's co-official languages, Wales, Scotland, Brittany, Hong Kong, Macau) folded in
as lines of their own.

```
# CLDR 47 territoryInfo, official and de facto official languages, then the refinements
CA	en,fr
CA-QC	fr,en
ES-CT	ca,es
HK	yue,en
```

**Checks** (crate::inputs::languages). Errors: a file other than those two; a line that isn't two
tab-separated fields; a code that isn't ISO 3166 syntax (`FR`, `CA-QC`: only the syntax is checked,
not that the code exists); a language that isn't a BCP 47 primary subtag (two or three lower-case
letters, `fr`, `yue`); a code twice; an empty list (one finding per check listing the lines); a version without
`territories.tsv` or without `about.toml` (so removing the table is an error, never accepted).
Warnings: a code the newest pass's outlines don't have, which covers a well-formed code that doesn't
exist (it would have no effect; the check job reads the outlines' codes, `outlines::spoken_codes`,
and their content name is in the check's key, so a new pass checks the table again; outlines that
can't be read are logged and the check goes on without this warning); a subdivision whose country has no line. The table's `keyed`
digest is of its lines as read, so a comment edited reruns nothing.

**Ingesting and rebuilding.** The `spoken` job (plan §7) is keyed on the outlines, `spoken::RULES`
(the raster's code and format) and the languages' accepted index, which the agent hands it
(`--languages-index`); it reads `territories.tsv` from the accepted version, and waits while there
is none. `global/spoken` carries the table in its head, so `names-todo` reads the catalog's
`global/spoken`, and the map's server needs the input only for its fallback (a catalog without
`global/spoken`), which reads the accepted table, kept on the Mac as the translations are
(`accepted/languages/` in its home).

### 6.3 Translations (#137)

**Built** (crate::inputs::translations, crate::inputs::lint; the server's crate::accepted and
pipeline::inputs::served; `names::Names::set_files`), with these choices where this section left
room:
- A line's shape is `names::line_shape`'s, the tables' own reading, so the check and the server
  can't disagree; `main` must be there (a string or null), `sub` a string or null when there.
- check.py runs as it is (one checker for the translators and the check): `uv run` in the app's
  `dem/` (the Python `dem/.python-version` pins, so its Unicode tables and verdicts are the same on
  every Mac), in its `--flagged` mode, on each new or changed file in the shape. Its per-line rules
  (`problem`) and `drift` are the warnings (`tr-check`); its batch-level null count isn't run (the
  converted tables are native names, mostly null by design). A finding's id names the flagged
  lines' content only (not their numbers nor check.py's words); a drifted stretch is check.py's
  window of 100 lines, named by all of its lines, so an edit above it that shifts the windows makes
  it a new finding. check.py failing to run fails the check job (tried again), never takes a file
  unchecked. check.py's text and the pinned Python's version are in the check's key
  (`Checks::context`): a change to either checks the unit again, reading the held and changed
  files under it; files already taken in stay as they are (§4.2).
- A file other than `.jsonl` (`conversion-log.txt`) is a shape error: notes go in `how/`.

**Kind:** request–fulfil. **Drop box:** `inputs/translations/` (moved from `translations/`).

**Shape:** as today (plan §7, Files): `**/*.jsonl`, outside `todo/` and `how/`, one translation a line:

| Field | Type | |
|---|---|---|
| `n` | string: the name exactly as in OSM | required |
| `kind` | `road`, `settlement`, `other`, or a list of them | required |
| `langs` | an ISO 639 subtag, or a list | required |
| `main` | string or null (null or empty: the name itself) | required |
| `sub` | string or null | optional |
| `via` | string, how it was made; `todo` and `skipped` mark lines not done | optional |

```json
{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu", "sub": "Blue Lake", "via": "agent:haiku"}
```

Later files (by path) win over earlier ones for a name, kind and language, and later lines within a
file, as today.

**Checks.** Errors: a line that isn't JSON or lacks `n`, `kind`, `langs` or `main`; a `kind` or
language that isn't valid; a file that isn't JSON lines. Lines in the area tables' old format
(without `kind` or `langs`) are an error too (today they're skipped with a warning: the converted
tables replaced them). Warnings: `tools/names/check.py`'s rules on the lines (its `problem` and
`drift` checks: romanisation that isn't Hepburn, English that drifts from the name's words…), one
finding per file listing its lines. Each such finding's id names the flagged lines' content
(§4.4): accepting it accepts those lines, and check.py goes on checking every other line of the
file, and any line added later. Only changed files are read (§4.2): checking a new 2,000-line
batch reads that batch, not the 808 MB already in.

**Ingesting and rebuilding.** Nothing is rebuilt, as today: the map's server loads the accepted
files (§4.6). A language's version becomes a hash of the content names of the accepted files holding
lines in it (today: paths, sizes and modification times, plan §7, Versions), so tiles' ETags change
only with content. `names-todo` names the translations' accepted version in its key, and gets the
accepted files from the agent (today `scenic-build`'s `--translations` defaults to the NAS folder,
crates/pipeline/src/bin/scenic-build.rs:209: the default goes). The time from
a drop to the map grows from about a minute and a half to the next listing plus a check (a few
minutes; §12).

**Migration.**
1. The code's side is built, in two publishes (#137). The first app's server reads the old folder
   (never losing a file of its copy) until the copy on its Mac is the accepted version the records
   name and the old folder holds no `.jsonl` file, then the accepted copy alone. This app (the
   second) reads only the accepted copies, and removes an earlier app's copy of the old folder once
   its own copy is the records' version. A file dropped into the old folder (but an older app's
   `todo/`) is an error finding (`moved`) saying where to move it. The move was made on 15 October
   2026, by #137's runbook, with the first app on every Mac: the files copied (never moved) into
   `inputs/translations/` while no translator ran (§3), the check's warnings accepted, each Mac's
   accepted copy confirmed as the records' version, and only then the old folder emptied (its
   contents, the old area tables among them, in `inputs/translations/how/old-2026-10/`, the
   owner's to delete); the second app was published once both Macs had their accepted copies.
2. The input check on today's files: the converted tables (2.78 M lines) raised check.py warnings,
   one a file (7 findings, 34,067 lines, listed in #137's report), each accepted once, each
   acceptance covering exactly the lines it lists (§4.4).
3. Proof (run for #137 on a copy of today's files): the compiled tables hold the same lines (every
   name of every file looked up in every kind and language, the same lookups; the same count per
   language).
4. The server's direct read of the NAS folder and the old-format skipping: deleted (this app).

### 6.4 Descriptions (#137)

**Built** (crate::inputs::descriptions), with these choices: `src` is a string or a list (today's
files carry lists of `{"t", "u"}`, as the server credits them); a `qid` is `Q` and digits without a
leading zero; `about.toml` is optional (without it the catalog keeps `rules::CREDITS`' "Heritage
descriptions"); its extent is the world when its `territory` is `"world"`, else unknown (listed
nowhere) until the things' places are read.

**Kind:** request–fulfil. **Drop box:** `inputs/descriptions/` (moved from `descriptions/`).

**Shape:** as today (plan §7, Descriptions): `**/*.jsonl` lines:

| Field | Type | |
|---|---|---|
| `qid` | string `Q<digits>` | one of `qid` and `id` |
| `id` | string `n<digits>`, `w<digits>` or `r<digits>` (an OSM object) | one of `qid` and `id` |
| `long` | string | required unless `drop` |
| `src` | string: what it was written from, credited | optional |
| `drop` | true: removes the thing's description | optional |

```json
{"qid": "Q1368", "long": "A fortified medieval town on the Aude, its double walls restored by Viollet-le-Duc.", "src": "Wikipedia"}
```

**Checks.** Errors: a line without `qid` or `id`, or both; a malformed `qid` or `id`; `long` empty
without `drop`. Warnings (one per file, listing lines): `long` over the brief's 55 words; no `src`.

**Ingesting and rebuilding.** As translations: served over the popups, nothing rebuilt; the
descriptions' credit comes from `inputs/descriptions/about.toml` (today the "Heritage descriptions"
credit, rules.rs:391).

`names-todo` reads the accepted descriptions from the agent too (today `namestodo::run` reads the
NAS's `descriptions/`, crates/pipeline/src/namestodo.rs:607).

**Migration.** As translations: the code's side built in the same two publishes (#137: the direct
reads gone, the proof run, every popup's description the same for the things in the files); the
move made on 15 October 2026 by the same runbook: `briefs-2026-09/` to `how/` first (§3), today's
`descriptions/heritage/` files copied (never moved) into the drop box and checked, the old folder
emptied only after each Mac's gate (its contents in `inputs/descriptions/how/old-2026-10/`, the
owner's to delete).

### 6.5 Heritage registers (#138)

**Kind:** declare. **Drop box:** `inputs/heritage/`, one pair of files per register:
`<register>.geojson` and `<register>.toml`. The register's id is the file's stem (`fr-merimee`,
`ca-dfhd`, `unesco-whc`): lower-case letters, digits and `-`.

**Built** (#138: crate::inputs::heritage on the input check; `producers/heritage/` making today's
registers; dem/heritage.py reading any register; the heritage-sites job reading the accepted
version), with these choices where this section left room:
- **Ids.** An entry's `id` is the register's own reference where the register has one the producer
  can read (Mérimée's `PA…`, the NHLE's list entry, the NRHP's reference number, a UNESCO
  component's `1221-001`, the IAPH's code for Andalusia …); where it has none (Northern Ireland's,
  Hong Kong's …), the producer makes one from the entry's name, designation and place (`h` and 16
  hex digits), stable while those don't change, and says so with `ref: ""`. A made id changes when
  the register renames an entry or moves or reshapes it: a match naming the old id (§6.5.4) then
  warns that no accepted register has it, and the owner writes it again with the new id. Two entries sharing a reference (a
  designation the register lists at two places) take `~2`, `~3` … in their ids, `ref` keeping the
  reference.
- **What the chain looks entries up by** is the shape's (`ref`, else the `id`; `site` for a
  `world-heritage` register), with the register's `wikidata_property`; a register without one is
  linked through OSM only (heritagewd.py, §6.5.1).
- **The kind of designation** the map's Heritage sites toggles group by is the producer's (`tier`,
  from today's patterns on designations, `producers/heritage/tiers.py`), a key the map knows
  (web/src/basemap.ts HERITAGE_TIERS); without one, it's by level (World Heritage by its category).
- **Other properties** pass through to the popup as they are (`category`, `criteria`, `notice`,
  `built`, `dfhd_id` …), as today's outputs carry them; `null` is absent.
- **`name`** is optional: an entry without one is a warning (`he-unnamed`), shown as its
  designation; Andalusia's register has none today (its reader doesn't find IAPH's name field),
  accepted as it is.
- **Registers by source:** one register per credit string today's readers carried, so each
  credit is a description of its own: France's Mérimée and its sites patrimoniaux remarquables;
  Spain's eight regions; Ireland's NIAH and SMR; Wales's listed buildings, scheduled monuments and
  parks (each with its own Wikidata property); Nova Scotia's and Halifax's; Singapore's three
  datasets; the special places by certifier (`unesco-mab`, `unesco-iggp`, `darksky`, `rasc-dsp`):
  40 registers.

**Shape: `<register>.toml`,** a description (§5.1), plus:

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `class` | `heritage`, `world-heritage`, `special` | yes | how the map shows its entries: heritage sites and areas; UNESCO's World Heritage (dots, and outlines from OSM); special places (biosphere reserves, geoparks, dark-sky places) |
| `territory` | list of ISO 3166 codes, or `"world"` | yes | required (§5.1); entries further than 20 km outside are a warning (§4.4) |
| `wikidata_property` | string `P<digits>` | yes | the Wikidata property holding this register's ids, by which entries are matched to Wikidata items |
| `osm_ref_tags` | list of strings | yes | the OSM tags holding its ids (`ref:mhs`, `HE_ref`…), by which heritagewd.py links an entry through OSM |

**Shape: `<register>.geojson`,** a FeatureCollection; each feature any geometry (Point, LineString,
Polygon, or their Multi forms), with properties:

| Property | Type | | Meaning |
|---|---|---|---|
| `id` | string | required, unique in the register | the register's own reference (stable across downloads), or one the producer made |
| `ref` | string | optional | the register's reference when it isn't the `id` (an id with `~2`); `""`: the register gives it none |
| `name` | string | optional (a warning without) | as the register gives it, in its language, tidied by the producer |
| `name_en` | string | optional | the register's own English: a source of the thing's own English (plan §7) |
| `level` | integer 1–5 | required for `heritage` and `world-heritage` | 1 World Heritage, 2 highest national grade, 3 other national grades, 4 regional, 5 local |
| `designation` | string | required for `heritage` | the register's designation ("Monument historique classé") |
| `tier` | string | optional, `heritage` | the kind of designation (web/src/basemap.ts HERITAGE_TIERS keys) |
| `kind` | string | required for `special` | `biosphere`, `geopark`, `dark_sky` |
| `site`, `site_area_km2` | string, number | `world-heritage` | the World Heritage site's id (a component's `id` is its UNESCO reference), its inscribed area |
| `wikidata` | string `Q<digits>` | optional | its item, where the register or the producer knows it |
| `date` | string | optional | designated |
| `municipality`, `type`, `authority`, `url` | string | optional | shown in the popup |
| `approx` | bool | optional | located loosely (by name) |
| `area_km2` | number | optional, `special` | its designated area, for the circle drawn when OSM has no boundary |
| `osm_name` | string | optional, `special` | the name of the OSM boundary to look for (default: its name) |
| `dot` | bool | optional | whether a polygon or line also shows as a dot (default: true for points and lines, false for polygons) |

```json
{"type": "FeatureCollection", "features": [
  {"type": "Feature", "geometry": {"type": "Point", "coordinates": [2.3376, 48.8606]},
   "properties": {"id": "PA00088801", "name": "Palais du Louvre", "level": 2,
                  "designation": "Monument historique classé", "tier": "n.top",
                  "municipality": "Paris", "url": "https://www.pop.culture.gouv.fr/notice/merimee/PA00088801"}}]}
```
```toml
name = "Mérimée, monuments historiques"
what = "France heritage"
credit = "Ministère de la Culture, base Mérimée (POP)"
licence = "etalab-2.0"
source = "https://www.pop.culture.gouv.fr/"
retrieved = 2026-09-28
class = "heritage"
territory = ["FR", "GP", "MQ", "GF", "RE", "YT", "PM", "BL", "MF"]
wikidata_property = "P380"
osm_ref_tags = ["ref:mhs"]
```

Every entry has a position when it's dropped: locating (Canada's federal designations, by
`producers/heritage/federal.py`) is the producer's job, never the app's.

**Checks** (crate::inputs::heritage).
- Errors: a file that isn't a register's `.geojson` or `.toml`; invalid GeoJSON or geometry (a ring
  of fewer than four positions, a line of fewer than two); a coordinate out of range; an `id`
  missing or twice; a property of the wrong type (a `level` outside 1–5, a `tier` the map doesn't
  know, a `kind` not one of the three, a malformed `wikidata`); entries missing what their class
  needs (`level`; and `designation` for `heritage`; `kind` for `special`); a description missing,
  or without its credit or licence, or with the licence `unverified` (`he-licence`: a licence
  nobody checked never reaches the credits or popups); an unknown `class`; a malformed `wikidata_property` or
  `osm_ref_tags`; a `territory` missing, or that isn't ISO codes or `"world"`; a `.geojson`
  without its `.toml` or the reverse (the two are held and taken together, §4.3). One finding per
  file and check, listing the entries (their feature number and id).
- Warnings: entries more than 20 km outside the declared territory's outlines (the newest pass's
  simplified outlines of those codes; one finding per register, its id naming the entries: NRHP's
  legation in Tangier is real); a register losing more than 10 % of its entries since its accepted
  version (its id naming the counts and both versions), or removed; more than half its entries
  `approx`; entries without a name.
- Derived: a `.geojson`'s box and entry count (`facts.box`, `facts.entries`), and its description's
  extent, the same box (`facts.extent`), which the catalog's credits list it by (§4.8).

Today's registers checked (the check run on the converted files, #138): five warnings, each to be
accepted once: Mérimée's 8 entries outside France's outlines (in Belgium and Germany: Mérimée lists
them), the NRHP's one off the Pacific coast, Nova Scotia's one with its coordinates swapped,
Andalusia's 3,929 entries without a name, and Singapore's 4 conservation areas without one.

**Ingesting and rebuilding.**
- **One generic reader** replaces the function per register: the heritage-sites job (plan §6, Job
  keys; docs/phase5.md, Heritage and area flags) lays out every accepted register whose box meets
  its cover (`pipeline::heritage::registers_meeting`: the box meets one of the cover's z6 tiles)
  for dem/heritage.py, which reads them in the order of their ids: a point is a site; a polygon a
  heritage area (flagging roads as today), and a site too at its representative point with `dot`; a
  line a site at its representative point (no layer draws lines); a `special` register's point is
  matched to the OSM boundary named `osm_name` (else its name) as today, or drawn as a circle, and
  its polygon is its boundary. Every site, area and special place carries its register's id
  (`register`), and a site its reference (`ref`).
- **Merging across registers stays a build step, by rule** (the owner's line: rules in the build,
  judgment as an input): two entries are one place when the matches say so (§6.5.4), or when the
  lower-graded one is local (level 5) and lies within 40 m of another register's entry of the same
  or a higher level whose name shares a word of three letters or more (today's rule). Of two local
  entries, the one of the register with fewer entries gives way (a city's list to the provincial
  register), whichever is read first, so the result doesn't depend on the order. That order is the
  registers' sizes now: a register that grows past the other (or shrinks below it) in an update
  flips which of their colocated local entries is kept, its id, register and properties with it; a
  `same` match (§6.5.4) keeps the highest-graded entry (among equals the first register by id)
  whatever the sizes, and is the way to pin a pair that matters. The place kept
  lists every register it was merged from (`registers`), its level the highest.
- **Keys:** heritage-sites names the registers meeting its cover by their `keyed` digests (the
  agent reads them from the accepted index each loop, `pipeline::heritage::registers_now`), the
  matches' accepted version and its version (4), and is given the two indexes it planned with
  (`--heritage-index`, `--heritage-matches-index`). A register on another continent changing reruns
  nothing. Without an accepted version, the heritage sites wait (and the units, the marks and the
  overlays with them). The heritage job keys on heritage-sites' outputs, as before.
- **Credits** from each register's description, for the registers whose box meets what the catalog
  serves (§4.8): a register's whole box, so France's overseas departments stretch Mérimée's across
  the Atlantic, and it's credited by a catalog in Canada too (a line too many, never one too few).
- **Popups' sources** come from each feature's `register` (and `registers`, for a merged place),
  resolved in the map against the catalog's credits, which name each register's credit by id
  (`register`, §4.8; web/src/overlays.ts `sourceOf`): "Source: <credit> (<licence>)".

**Migration.**
1. **Producers outside the app** (`producers/heritage/`, §8): `registers.py --data <folder> --out
   <folder> [<register> …]` writes each register's pair, its readers moved from the app (`eu.py`,
   from dem/heritage_eu.py; `na.py`, from dem/heritage.py's per-register functions and the special
   places' local names; `tiers.py`, from dem/heritagetiers.py; `federal.py` and `crhp.py`, which make
   `federal.json` and `crhp.json` in the data folder as before). The data folder is laid out as the
   registers' snapshot was (`sources/registers/legacy` extracted), so today's registers convert from
   it without a download. Their credits and licences, Wikidata properties and OSM tags are in
   `REGISTERS`. Six of Spain's regional registers (Catalonia, Castilla y León, Aragón, the
   Comunitat Valenciana, Extremadura, Andalusia) have the licence `unverified`: the old app called
   them "free use with credit", which nobody checked. Their descriptions' `notes` say what the
   owner must check (each portal's legal notice or reuse conditions: an open licence or Spain's
   default reuse terms, whether commercial reuse and redistribution in a map are allowed, the
   attribution asked for) before setting the licence. Until then the input check holds them (an
   error: `he-licence`), so they're never read; heritage-sites reads the other accepted registers,
   and Spain's regions have no heritage sites from them meanwhile.
2. The heritage-sites job reads the registers generically, the chain reads the registers' fields
   heritage-sites lists (`heritage-sources.json`'s `registers`), and the chain's own caches (its
   Wikidata, Wikipedia and World Heritage answers, which lived in the snapshot) are the pass's
   answers alone (`sources/items/<date>/heritage.tar.zst`, `pipeline::answers::heritage`), a new
   pass starting from the last pass's. With no pass's archive yet (the first run since the
   registers became an input), the job starts its working copy from the snapshot's caches (wd/,
   desc/ and whs/ of `sources/registers/legacy`, read only) with the pass's answers kept beside it
   (`sources/items/<date>/heritage-<id>.tar.zst`) laid over them, less what the chain makes again
   each run (`pipeline::answers::heritage_start`); its keep then writes the pass's archive under
   the answers' lease, as every run's does. Without the snapshot, the pass's answers alone; a
   snapshot the manifest names that can't be read (the NAS away) fails the job, tried again, so an
   empty copy is never kept as the pass's answers.
3. **Proof** (run for #138, the old app and this one on the same pass, cover and answers, seven
   regions: Moncton, Halifax, Mont-Mégantic, Paris, Barcelona, Edinburgh, Kyoto; the new side's
   answers made as the job's first start makes them, from the snapshot's caches and the pass's
   answers, so the chain asked nothing): the sites'
   position slices and World Heritage parts the same bytes; the 11,769 sites the same with every
   property but `source` (now `register`, `ref`, `t`, and `site` for World Heritage); the area
   slices of the cover's tiles the same, the heritage areas those of the registers meeting the cover
   (the old job took every register's, 418 more here: Ontario's districts, Singapore's areas, two of
   Quebec's without coordinates, which the producer leaves out); the special places the same; the
   chain's details and props the same but for the source; seven colocated pairs in Moncton (a
   Moncton and a CRHP entry metres apart, which the rule doesn't merge) swap their zoom of
   appearance, interest.py's tie-break following the order the registers are read in; the marks'
   heritage points the same ids at the same places (`work/marks/heritage-dots`); the overlays the
   same areas within the cover, with the register in place of the source.
4. **Deleted:** the per-register code (dem/heritage_eu.py, dem/federal.py, dem/crhp.py,
   dem/heritagetiers.py, dem/heritage.py's register functions, `STATES`, the register order),
   heritagewd.py's `RULES` and NRHP case, whsshapes.py's reading of UNESCO's list, the registers'
   credits and territory boxes in rules.rs, `registers-import`, the snapshot's readers
   (`sources/registers/legacy`), and the old build's heritage fallbacks (`markconv::LEGACY`,
   `agent::build::heritage_src`, `overlays_key`'s branch). The snapshot's archive on the NAS is the
   PM's to retire once a pass's heritage archive (`sources/items/<date>/heritage.tar.zst`) exists:
   until then the job's first start reads its caches (§11).

#### 6.5.1 What stays generic in the heritage chain

The chain's other scripts (heritagewd.py, heritagedetails.py, areadetails.py, whsshapes.py,
filterprops.py, pageviews.py, interest.py, layers.py: docs/phase5.md) are generic over OSM,
Wikidata and the registers, reading the registers' fields instead of per-register knowledge:
heritagewd.py looks a site up by its `ref` under its register's `wikidata_property`, a World
Heritage site by its `site`, and a site whose record links its Wikidata item by that item, and
links through OSM by the tags the registers declare (`osm_ref_tags`); whsshapes.py finds a site by
its `site`, its components' items by their `ref`s and its inscribed area by `site_area_km2`, and
its outlines by OSM's own World Heritage tags (`ref:whc`, `heritage:operator=whc`: OSM's schema);
filterprops.py takes `t` from the register's `tier`. The languages that pick a UNESCO site's or a
special place's name (`lang_at`) are the producers' now (`producers/heritage/eu.py`), as those
names are chosen there; dem/names.py's region boxes went with the English table (§6.5.3).

#### 6.5.2 The heritage chain's stand-in root

The chain runs in a stand-in root laid out as the old repository (`data/build`, `data/heritage`:
scenic-build's `heritage_root`; docs/phase5.md). The registers are given on heritage.py's command
line (`--registers`, a folder of links to the accepted copies; `--matches` likewise), and
`data/heritage` is the pass's working copy of the chain's answers (`cache/heritage-<date>/`), an
empty folder for heritage-sites. The rest of the stand-in (a scratch folder per run) stays until
the scripts take paths for everything, which is cleanup, not this series.

#### 6.5.3 The frozen seeds retired (#139)

`sources/registers/legacy-seeds` held three things nothing could make again. None is an input; the
heritage job reads none of them now (built, #139), and the archive is the PM's to delete from the
NAS:
- **The English names table** (`names/english.json`): the heritage layers' `en` is the things' own
  English only (`dem/names.py` `english_at`: a site's UNESCO name or its English Wikipedia title, an
  area's `name_en`). Any other English is the map's server's, from the translations' live tables
  as it serves the layers (the same lookup as for every other name, plan §7: `en` is the thing's own,
  shown first; else a line in a language spoken where it is). A name only the old table had goes
  onto the translations' to-do list (`pipeline::namestodo`, which reads the landmarks' `en`).
- **Park facts** (`areas/wikidata.json`): areadetails.py's cache is the pass's answers
  (`areas-wikidata.json`, `pipeline::answers`), started from the earlier pass's archive or, for the
  pass the registers became an input, from the answers kept beside it (§6.5, Migration);
  areadetails.py asks Wikidata only for the parks it lacks.
- **Pageview months** (`pageviews/months/`): the items job's months' indexes (`sources/pageviews/`,
  plan §6), which pageviews.py reads before any older counts; the seeds' counts were never read
  while the indexes covered the months.

Proof (pass 2026-09-28, the published layers against the seeds and the accepted translations):
- park facts: the pass's answers have all 7,952 of the seeds' parks, each the same, and 6,223
  more;
- pageviews: the four months' indexes count every language pageviews.py asks for, so the seeds'
  months were never read;
- English: of 232,716 features, 11,001 lose an `en` that came only from the old table (10,947
  heritage sites, 23 heritage areas, 2 special places, 29 Indigenous lands). The server shows the
  same English for 4,446 of them (a live line gives it; for generic names such as Église, Château it
  shows the English as the main line), 3 more with the same English under a script's name, other
  English from a live line for 152 (e.g. 琵琶湖疏水施設: "Lake Biwa Canal Facilities", the table
  "Lake Biwa Canal"), and none for 6,400, which go onto the to-do list. The `en` of every other
  feature is unchanged (14 differences in a reconstruction from the published files were the
  reconstruction's: no table line was involved).

#### 6.5.4 Heritage matches: the owner's judgment calls

**Kind:** declare. **Drop box:** `inputs/heritage-matches/`, `*.jsonl` lines:

| Field | Type | |
|---|---|---|
| `a`, `b` | string `<register>:<id>` | required |
| `same` | bool: true, one place; false, never merged | required |
| `note` | string: why | optional |

```json
{"a": "ca-dfhd:1234", "b": "ca-qc-rpcq:92847", "same": true, "note": "the same church; the federal entry is 300 m off"}
```

**Checks.** Errors: malformed lines; `a` equal to `b`; contradictions (a chain of `same` pairs
joining two entries a `same: false` pair separates). Warnings: a pair naming an entry no accepted
register has (a register's update dropped it).

**Built** (#138: crate::inputs::heritage_matches on the input check; dem/heritage.py applying
them). The nowhere warning reads the registers' accepted version (their content name is in the
check's key, `Checks::context`, so a register's update checks the matches again).

**Ingesting.** Applied before the rules: `same` pairs merge (transitively, the highest-graded
entry kept), `same: false` pairs are never merged by any rule. The matches enter heritage-sites'
key by their accepted version (and so the heritage job's, which keys on heritage-sites' outputs).
Today there are none; the to-do list of likely matches the rules didn't take (near, a similar name,
different registers) is a later request–fulfil addition, not in this series.

### 6.6 (reserved)

### 6.7 Timetables (#140–#142)

One input worldwide, in two units of the input check: feeds the app fetches (source declarations) and static GTFS
the owner compiles by hand (declare). OSM decides what's drawn and where; timetables only add
counts. Each mode has one counter, whichever unit its timetables came from, and each counter keeps
what its mode shows today.

#### 6.7.0 The counters, per mode

- **Rail** (`dem/railgtfs.py`): trains per stop pair on the typical day, as the PAIR records
  the `rail` job matches onto the tracks (`rail::PAIR`, crates/pipeline/src/rail.rs:57: the two
  stops' positions, a mode byte, trains a day). The typical day is a feed's median-busy Tuesday to
  Thursday from 30 days before the day its zip was fetched to 90 days after; a train in two feeds
  counts once (trips keyed by their first and last stops and their times, the operator's feed
  first). The static files' trains and count rows: §6.7.2.
- **Ferries** (`dem/ferrygtfs.py`, run by the `ferries` job's `dem/ferries.py`, built #140): per
  OSM ferry line, the record the map shows (`ferries.py`'s `info["freq"]`): `per_day` (each way, both
  directions averaged, on the days it runs; for a line sailing fewer than four days a week, the
  weekly count over 7), `per_day_low` (the quietest month's, for a line running all year with a
  lower winter), `headway` (the typical day's median gap, 07:00–19:00), `days` ("daily", "Mo–Fr"),
  `months` and `season` (`year`, `some-days`, `seasonal`), `overnight`, `source`, `url`, `checked`.
  A GTFS route is matched to a line by its stops: trips calling within 400 m (at most 30 % of the
  line's length) of each of its two ends. Generic GTFS code, run on the ferry feeds the `rail-feeds`
  job fetched (§6.7.1), each from its reference day: the day its zip was fetched, as rail's, so a zip
  gives the same counts whenever it's read. The feeds are read in id order, the first feed's count
  for a line winning. The static files (§6.7.2) are counted by the same counter over one reference
  year, and their count rows taken as given.
- **Precedence** for a ferry line (`ferries.py`'s `rank`): a feed's count, then a looked-up
  timetable's (a static file's, in the zips' order), then OSM's `interval` tag.

#### 6.7.1 Feeds (#140)

**Built** (crate::inputs::feeds; `dem/railfeeds.py`; crate::rail). **Kind:** source declarations.
**Drop box:** `inputs/timetables/feeds/`, the unit `timetables/feeds`; how the feeds were found is
in its `how/` (`gtfs-feeds.md`, `operators.txt`).

**A feed:** `<id>.toml` (an id of letters, digits, `-` and `_`), a description (§5.1) and:

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `[data] format` | `"gtfs"` | yes | |
| `[data] url` | string | yes | the zip's URL, or the API's for a keyed feed |
| `[data.fetch]` | table | yes | `method` and its fields (§5.2): `get` (default); `get-key-header` (`header`, `key`); `get-key-json-link` (also `link`, the download link's JSON path in the answer): `header = "AccountKey"`, `key = "LTA_ACCOUNT_KEY"`, `link = "value[0].link"` for LTA's |
| `[data] refresh` | string | yes | `days:7` by default: a copy out of date when fetched (no service in its window) is fetched again once it's that many days old |
| `modes` | list of `"rail"`, `"ferry"` | yes | the counters that take it; none: both, a rail feed by its `routes.txt` having rail routes |
| `order` | whole number | yes | where it comes among the declared feeds (then by id); the declared come before the catalogue's, so a train they share with an aggregate counts as theirs |
| `replaces` | list of feed ids | yes | feeds left out while this one has a zip (an operator's own over copies) |
| `ferry_routes` | list of route ids | yes | routes counted as ferries though their `route_type` says otherwise |
| `pin` | table `{zip, day}` | yes | a zip fetched before the app fetched the feed (its path from the project folder) and the day it was fetched: rail-feeds pins it, counting from that day, when the feed has no zip yet, instead of fetching |
| `operator` | string | no | shown |

`territory` (§5.1) and `name`, `licence` and `source` are keyed too: `territory` says which
countries' coverage takes a rail feed; `name` and `licence` are in the rail feeds' list; a ferry
record's `source` is "`<name or agencies>` timetable (GTFS)" and its `url` the declaration's
`source`. No box: a rail feed is used where its stops are (the box of its zip's `stops.txt`, read by
rail-feeds once the zip is there), the ferry feeds everywhere (the ferries are mapped worldwide).

```toml
name = "SNCF Voyageurs (TGV INOUI, OUIGO, Intercités, TER)"
what = "Rail timetables"
credit = "SNCF Voyageurs"
licence = "Licence Ouverte 2.0"
source = "https://ressources.data.sncf.com/"
territory = ["FR"]
modes = ["rail"]
order = 1
[data]
format = "gtfs"
url = "https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip"
```

**The catalogue:** `<id>.toml` with `format = "mdb-catalogue"` (the Mobility Database's
`feeds_v2.csv` columns, a published format read generically), its `url` (fetched when the rail
sources have no copy, `sources/rail/catalogue`), and the corrections to it:

| Field | Type | Meaning |
|---|---|---|
| `exclude` | table id → where its trains come from | feeds left out (the MTR's lines, which come from the static file `mtr.zip`, §6.7.2: its status says "replaced by" that) |
| `country` | table id → ISO code | feeds the catalogue files under another country (two of Singapore's, under Malaysia) |
| `replaces` | table id → ids | copies of one timetable, each listed left out while the feed listing it has a zip |
| `own_code` | table ISO 3166-2 code → alpha-2 code, or `""` for none | territories with an ISO 3166-1 code of their own that isn't their code's end, and two whose ends are other countries' codes (Åland's FI-01 is AX, Guadeloupe's FR-971 GP, Kosovo's RS-KM XK; Ceuta's ES-CE and Melilla's ES-ML none) |

The catalogue's feeds are chosen by a generic rule over its fields: the countries the coverage is in
(by the pass's outlines and `own_code`: `rail::countries`), the feeds' boxes, no key needed (plan
§6).

**Checks** (`inputs::feeds::Feeds`). Errors: a file that isn't `<id>.toml`; a description without
its credit and licence; no `[data]`, an unknown format or fetch method, a `url` not http(s); a
keyed method without its `header`, a `key` that isn't a key's name, a `get-key-json-link` without
`link`; a `refresh` not `days:<n>`; `modes` other than `rail` and `ferry`; an `order` not a whole
number; `replaces`, `exclude` or `country` naming a malformed id; a `country` or `own_code` value
that isn't ISO 3166 syntax; two catalogue declarations. Warnings: `replaces` or `exclude` naming a
feed neither declared nor in the catalogue's copy (when there is one); an `own_code` key no outline
of the pass has; two declarations with one URL. A declaration that `replaces` declared feeds is
held with them: a held `<id>.toml` holds a feed declaration whose `replaces` names it, and the
catalogue's declaration when its `replaces` names it. (A `key` that `inputs/keys.env` doesn't hold
isn't a finding: the feed waits. Plan §10.)

What a fetch brings back (a zip that isn't one, a feed without rail) isn't a finding of the input
check: the data isn't the owner's drop. It's the fetching job's status (a refusal leaves the feed
out, an old copy is kept: the last good version already).

**Fetching and rebuilding.** `rail-feeds` (plan §6) reads the accepted declarations (the index the
agent names: `--feeds-index`) and the catalogue's pinned copy, and writes two lists: the rail feeds
(`sources/rail/feeds`) and the ferry feeds (`sources/rail/ferry-feeds`), each feed's zip pinned by
content in `sources/rail/gtfs/<id>` with the day it was fetched (`sources/rail/fetched`). Its key
names the declarations' accepted index, the catalogue's copy, the coverage, the pass's outlines and
which of the keys the declarations fetch with `inputs/keys.env` holds (their names). `rail` names the rail list (the zips by content);
`ferries` names the ferry list (the zips by content and their days) and waits for it. While the
rail sources have a catalogue's copy and the accepted version has no catalogue declaration (held),
rail-feeds fails rather than read the catalogue without its corrections. A declaration's `pin` is
put in by rail-feeds itself, handed off as its downloads are, the first time it runs with that
declaration accepted and no zip for the feed: whatever the order of the drop and the publish, a
pinned feed is never fetched before its pin is in.

**Rollout** (#140): (1) the declarations (the ferry feeds' with their `pin`s, naming
`inputs/ferries/gtfs/<id>.zip` and 2026-09-28) dropped in `inputs/timetables/feeds/`, the feed
finders' notes in its `how/`, before or after the publish; (2) the publish; (3) the catalogue's
declaration's `own_code` warning (seven territories no outline of the pass has) accepted: till
then it's held, rail-feeds fails and the ferries wait, their outputs as they were; (4) after the
first rail-feeds and ferries, the `gtfs-*.json` files, `gtfs-feeds.json` and (the pins being in
the sources) `inputs/ferries/gtfs/` removed, and the `pin`s may go from the declarations.

**Migration** (done in #140; the proof in its report):
1. **Rail:** declarations written from the lists railfeeds.py had (SNCF, two of Renfe's, Great
   Britain's, Hong Kong's trams; LTA's behind its key), and the catalogue's with its corrections and
   `own_code`; their zips and the catalogue's copy stay the pins they were. Proof: railfeeds.py
   with the lists and with the declarations, on the same inputs, write the same feed list byte for
   byte, so `rail`'s key and `global/railfreq` are the same.
2. **Ferries:** the 40 verified feeds of the old hand-run counter's list became declarations
   (`modes = ["ferry"]`), their first pins the zips its counts were made from, each counting from
   the day it counted them (2026-09-28). Proof: the counter in the app, on those zips and that day,
   gives every record of the `gtfs-<feed>.json` files field for field, but two whose lines OSM has
   moved since (the routes they match are named otherwise, which the map doesn't show).
3. Deleted: the lists in railfeeds.py, `rail::FEED_KEYS` and `OWN_CODE`, the hand-run counter and
   its feed list; the `gtfs-*.json` files on the NAS go (the ferries job reads only
   `timetables-*.json` there now).

#### 6.7.2 Static GTFS (#141)

**Built** (crate::inputs::gtfs; `dem/staticgtfs.py`, read by the counters; the producers in
`producers/timetables/`). **Kind:** declare (and the fulfil half of §6.7.3's to-do). **Drop box:**
`inputs/timetables/gtfs/`, the unit `timetables/gtfs`: `<name>.zip` (a name of letters, digits, `-`
and `_`), each a GTFS feed compiled by hand from timetables found anywhere, with a `<name>.toml`
description (§5.1) beside it when its sources need a credit line beyond each route's page (the two
are held together). The researchers' brief, how to answer in static GTFS, is in its `how/`.

**Shape:** GTFS, with the reference's required files (`agency.txt`, `stops.txt`, `routes.txt`,
`trips.txt`, `stop_times.txt`, `calendar.txt`; `feed_info.txt` naming the publisher), each with at
least the columns the counters read (`trips.txt` and `stop_times.txt` may hold their header alone
when every route is counted in `counts.txt`), and:
- `frequencies.txt` allowed: a trip plus "every 600 s from 06:00 to 23:00" instead of each departure;
- **no expiry, but seasons:** static files are counted without years. `calendar.txt`'s start and
  end dates are read as month and day over one reference year, the year written ignored
  (`20000501`–`20001031` is May to October every year; a start after the end wraps the new year;
  `0101`–`1231` is all year; 29 February is read as the 28th), and its weekday columns give the
  days. `calendar_dates.txt` is ignored. So a service never expires, and a seasonal one is seasonal
  every year;
- **each route's source page** in `routes.txt`'s `route_url` (required here, an error without it),
  and extra columns (GTFS lets producers add columns, which consumers ignore): `checked` (when the
  source was read), `months` (the season as the source words it, "May–Oct"), and an optional `osm`
  pin (`r14098916`, `w23471991`), the OSM line a route is counted for;
- **counts where the source gives no times: `counts.txt`,** an extension table (ignored by other
  GTFS tools) for services the source describes only by counts ("about 10 sailings a day"), so no
  departure time is ever invented. A row: `route_id`; for rail, `from_stop_id` and `to_stop_id`
  (trains each way between the two stops) and `at_least` (1: a lower bound, shown "at least"); then
  the mode's fields as the counter outputs them: `per_day`, `per_week`, `per_day_low`, `headway`
  (minutes), `days`, `months`, `season`, `overnight` (1 or 0), `note`. An empty cell isn't given;
  a number is read as written (`10` a whole number, `1.5` not), `null` where the source was read and
  gives none; a `headway` may be the source's words ("about every 20 minutes"), which the ferries
  read by its first number. A route with count rows has no trips; a ferry route counted so needs
  its `osm` pin (it has no stops to match by). Count rows are taken as given: they're never matched
  against other feeds' trips by times (they have none), so a service shouldn't be both in a feed and
  in counts; the catalogue's `exclude` keeps a feed's copy out where that would happen (as it does
  the MTR's).

**The counters** read the accepted zips in their paths' order (`inputs::gtfs::zips`; `--static` of
`dem/railgtfs.py` and `dem/ferries.py`):
- **Rail:** a static file's trips as a feed's (frequencies expanded), its typical day the
  median-busy Tuesday to Thursday of the reference year, its trains deduplicated with the feeds' by
  times. Its count rows of rail routes (by `route_type`, as a feed's) go apart, in their order, to
  the stop pairs read after the trains': per row the trains each way (`per_day`, else `per_week` ÷
  7), A to B then B to A, at the stops' positions as f32, the mode by the route's type, with the
  lower-bound bit (0x80) for `at_least`. Their stops beyond the coverage are marked as the feeds'.
- **Ferries:** a count row of a ferry route with an `osm` pin is the record of that line: the row's
  fields as given, `source` the route's `route_long_name`, `url` its `route_url`, `checked` its
  `checked` (`kind` "timetable"). Trips are counted by the ferry counter over the reference year (a
  break of a month or more, the new year's included, makes the line seasonal; otherwise it's
  year-round or some days), each record's source the first matched route's; a route pinned to a
  line counts for that line alone, its ports the calls nearest the line's ends.

**Checks** (`inputs::gtfs::Gtfs`). Errors: a file that isn't `<name>.zip` or `<name>.toml`; a
description without its credit and licence; a zip that can't be read (only stored and deflated
entries, no Zip64); a required file or a column the counters read missing; references broken (a
trip's route or service, a stop time's trip or stop, a frequency's trip, a count row's route or, for
a rail route, its two stops); a time that doesn't parse (`H:MM:SS`); a calendar date that isn't a
valid month and day; a `route_type` GTFS doesn't define; a `headway_secs` or a numeric count
`headway` not positive; a count row's number that isn't one (nor `null`), an `overnight` or
`at_least` not 0 or 1; a route without `route_url`; an `osm` pin that isn't `w<id>` or `r<id>`; a
ferry route with count rows and no `osm` pin; a route with both trips and count rows; a stop's
coordinates out of range. Warnings: a route with no trips and no count rows; two zips with one
`route_id` of one agency.

Whether a route matches something on the map isn't a finding of the input check: it depends on
OSM, which changes each pass, and a warning there would hold the file again every pass. It's #142's
report.

**Ingesting.** `rail` and `ferries` read the accepted version the agent planned with
(`--gtfs-index`), name its index in their keys (each zip by content), and wait while the unit has no
accepted version, rather than count without the static files: their outputs stay as they were
(the agent shows "waiting for the static timetables' first accepted version"). Neither reads
`inputs/` itself.

**Migration** (done in #141):
1. **The MTR:** `sources/rail/mtr` (`mtr.json`, researched by hand) became `mtr.zip`
   (`producers/timetables/mtr.py`, which holds the MTR-specific parsing of the research: the
   service hours at the slowest published weekday off-peak headway as a lower bound, the LOHAS Park
   shuttle off-peak): its 160 stations as stops at the positions the old pairs had them (read back
   from `sources/rail/mtr-pairs`, `--pairs`; `--stations` places them from OSM by name, as
   `dem/mtrpairs.py` did), a route per line or branch with a count (23; the High Speed Rail's one
   station makes no row), and a count row per pair of consecutive stations at two places (314). Proof
   (`tools/check/static-gtfs-same.py`): the rail counter's count rows for it are the 628 records of
   `sources/rail/mtr-pairs` byte for byte, and they're railfreq's last input as the pairs were, the
   feeds' pairs before them unchanged (the counter with and without the static files, on six feeds
   with Hong Kong's, wrote the same bytes), so `global/railfreq` is the same.
2. **The ferries' looked-up timetables:** the 24 `inputs/ferries/freq/timetables-*.json` files (658
   lines) became 24 zips of count rows (`producers/timetables/ferry_lookups.py`,
   `timetables-batch-01.json` → `ferries-batch-01.zip`, so in the files' order): a route per line
   (`route_id` and `osm` the record's `line`, `route_long_name` its `source`, `route_url` its `url`,
   `checked` its `checked`, `route_type` 4) and a count row with exactly the record's other fields
   (the one `per_day` given as null written `null`). Proof: the 658 records the zips give equal the
   files' field for field, each value of the same JSON type; and `ferries.py` on the 2026-09-28
   pass's ferries set, without feeds, wrote `ferries.json` and `ferry-lines.json` byte for byte the
   same from the files and from the zips (25,287 lines, 1,005 with a frequency).
3. Deleted: `dem/mtrpairs.py`, `rail::MTR` and `MTR_PAIRS` and the rail job's merging of the
   pairs, the `timetables-*.json` path in `ferries.py`, the ferries job's and the agent's reads of
   `inputs/ferries/freq` (with `pipeline::smallfiles`, kept for them alone).

**Rollout** (#141), each state's output said:
1. Before the publish, the PM drops the 25 zips into `inputs/timetables/gtfs/` together (the app
   published before #141 doesn't know the unit: nothing reads them), and into its `how/` the
   rewritten brief (`PROMPT.md`), `inputs/ferries/research/`'s batches and not-found lists (as
   `how/research/`) and the MTR's research (`sources/rail/mtr`'s file, as `how/mtr.json`).
2. The publish. Until the input check takes the zips in, rail and the ferries wait, their outputs as
   they were. The zips have no findings (checked against the converted files), so the first check
   takes all 25 in one version; rail and the ferries then run once (their keys and `RAIL_V`,
   `FERRIES_V` changed) and write the same `global/railfreq` and ferry layers. (A zip dropped after
   the first check is a second version: rail and the ferries would run on the first without it,
   which is why they go in together.)
3. After the first rail and ferries runs: `sources/rail/mtr` and `mtr-pairs` are read by nothing
   (the PM removes their records and files), and `inputs/ferries/freq/` and `inputs/ferries/research/`
   (now in `how/`) can be removed from the NAS.

#### 6.7.3 The to-do list and the report (#142)

**Built** (crate::timetables; the rail and ferries jobs' pieces; the lead-only `timetables-todo`
job). Both depend on OSM, which changes each pass: neither is a finding of the input check, and
nothing waits on them.

- **The to-do:** `inputs/timetables/gtfs/todo/lines.jsonl`, rewritten whole after each rail and
  ferries run: OSM's rail and ferry lines in the coverage that no timetable covers, each
  `{"osm": "r123", "kind": "rail"|"ferry", "name", "ref", "operator", "from", "to", "km", "priority"}`,
  highest priority first (`priority`: the length, a tenth of it for a line whose tags say it carries
  no passengers: `passenger=no`, `service=freight`), beside a brief (`todo/README.md`,
  `tools/timetables/todo.md`: how to compile static GTFS for a line, what to cite, the `osm` pin).
  The owner answers with zips in `inputs/timetables/gtfs/` (§6.7.2).
  - *Rail lines:* the rail set's service relations (`route` train, subway, tram, light_rail,
    monorail, funicular; not `railway`, the track) grouped into lines as the ferries' are (one
    network's or operator's `ref`, else the same two ends, else the relation alone; the first
    relation's id names it), measured over their ways in the coverage; on the list while less than
    half that length has trains in `global/railfreq`.
  - *Ferry lines:* ferries.py's lines with an end in the coverage and no count from a feed or a
    static file (an OSM `interval` tag isn't a timetable).
- **The report** (`work/timetables/report`): timetable routes that match nothing on the map, each
  `{mode, source (the feed's id or the zip's name), static, route, name, agency, osm, why}`, listed
  on the build page (Details, Timetables: the first 40) and in `scenic status`, never drawn:
  - a rail route with stop pairs in the coverage none of which found a track (railfreq's
    `rail-miss.bin`): a line OSM lacks, or one that closed; a static rail route whose `osm` pin
    names neither a service relation nor a rail way of the pass in the coverage;
  - a ferry route sailing in its counting window, between two stops, that no line took (its ports
    match no OSM ferry line); a static ferry route, timed or counted, whose `osm` pin names no ferry
    line of this pass (a pin naming a direction variant other than the line's first relation is
    one).

**How it's made.** `rail` (railgtfs.py's `--routes`: each route's stop pairs, as the pairs files
hold them; railfreq's misses; the rail set's service relations, `osmium tags-filter` of the
clipped set) hands off `work/rail/lines`; `ferries` (ferries.py's `timetables.json`) hands off
`work/ferries/lines` (`RAIL_V` 3, `FERRIES_V` 3: the layers and `global/railfreq` as they were).
`timetables-todo` (lead-only, keyed on the two pieces, the coverage and its version) merges them,
writes the to-do and its brief into the drop box's `todo/` itself, outside the records (as
`names-todo` writes the translations' and descriptions' to-dos), and hands off the report. The
input check never lists `todo/` (`inputs::list` skips `todo/` and `how/` at a drop box's top, for
every unit), so the to-do's rewrites are no drop-box change, and `todo/` isn't backed up. The lead
reads the report again when its content name changes, for its status (`Status::timetables`).

**Rollout:** the publish; rail and ferries run once more (their versions), then `timetables-todo`
once. Nothing is deleted.
### 6.8 Elevation (#143, #144)

**Status:** #143's first step is built as its publish 1 (the migration below): the declarations on
the input check (crate::inputs::elevation), the one generic sampler with its blend
(crate::dem::generic, crate::dem::declared), the switch, the units' keys and DEM cache by the
datasets once switched, and the proof (`scenic-build elevation-proof`). Its publish 2 deletes the
per-source code and the DEM rules and credits. Steps 2 (densification from the data) and 3 (heights
converted) are planned. What isn't built yet is marked planned.

**Kind:** source declarations, a dataset each; a small dataset's files may be dropped beside its
declaration. **Drop box:** `inputs/elevation/`: `<dataset>.toml`, a tile index `<name>.geojson`
beside the declaration that names it, and `<dataset>/` for local files.

**Shape:** a description (§5.1) and:

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `surface` | `ground` or `surface` | yes | bare earth (a DTM), or with trees and buildings (a DSM) |
| `heights` | string | yes | the height reference: `EGM2008`, `EGM96`, `CGVD2013`, `NAVD88`, `JGD2011`, `TWVD2001`, `ellipsoid-WGS84` (recorded; converted in step 3, planned); `mixed` is refused here (the terrain's alone, #144) |
| `rank` | integer | yes | optional: overrides the order by resolution (lower first); a dataset without one ranks 0; needed where datasets of one resolution differ in quality (GSI's lidar over its photogrammetry) |
| `code` | integer 1–255 | yes | the source byte its heights carry in the units' outputs (`src.u8`), which the map's server labels by (`roadcore::DemSource`): today's codes kept, so the outputs stay the same (§12, the DEM cache, plans the byte as a position instead) |
| `territory` | ISO codes, or `"world"` (§5.1) | yes | required for a tile pyramid (its coverage can't be read from a header: tiles are asked for only inside it; its coverage is its territory's outlines' boxes in the newest pass); `"world"` makes any dataset's coverage the world (FABDEM); otherwise optional for GeoTIFFs (checking it against their extents is planned) |
| `[data] format` | `geotiff` or `tiles` | yes | |
| `[data] url` / `urls` / `index` / `files` | | yes | one of: GeoTIFFs by URL (`<zip URL>#<member>` for a member of a zip, read in place), a tile index (GeoJSON of footprints with each file's `url`, or its `file` in `<dataset>/`, in the index's CRS, made by a producer from the source's listing: `producers/elevation/indexes.py`), or files dropped in `<dataset>/` by glob. (ETags pinning `range` sources, §5.2, are planned.) |
| `[data] store` | bool | yes | each file downloaded whole once, kept in `sources/fetched/<dataset>/` as a compressed GeoTIFF (FABDEM's, as `sources/fabdem/` kept them); read in place where that can't be written |
| `[data] encoding` | `terrarium`, `terrain-rgb`, `gsi-png` | yes | for `tiles`: the encodings are formats, decoded by generic code (crate::dem::encoding) |
| `[data] url`, `zoom` | template, integer | yes | for `tiles` |
| `[data] read_resolution` | metres | yes | optional: read a finer dataset at its overview nearest this (HRDEM's 2 m read at its 8 m overview) |
| `[data] valid` | GeoJSON file beside it | yes | planned (#144): where the dataset is trusted, when that isn't all of it (AWS's tiles south of 60°N: north of it they mix an ellipsoidal source, plan §6) |

Read from the data at check time, never declared (`facts.measured` in the index, beside the
declaration as parsed, `facts.declaration`): a GeoTIFF dataset's CRS and resolution (by range
requests, the headers of an index's first file, its last and seven between, and of every file
without an index, URLs and dropped files alike: all in one CRS the app projects, its resolution the
coarsest of theirs at the overview read), and its coverage (`cover`): the 1° cells its data meets,
merged into boxes. An index's files meet by their footprints; any other file by where it has valid
data, read at its coarsest overview (one with none under 16 M pixels, by its bounds), so a file
whose rectangle reaches past its data covers only its data (MRDEM's bounds hold Greenland and
Iceland, its data neither). A projected CRS's cells are found by projecting each cell's 5 × 5
points, with the cells around. A tile pyramid's resolution comes from its zoom at its coverage's
mid-latitude, and one of its tiles is read for its size: the sampler reads 256-pixel tiles
(`generic::TILE_PX`). The coverage is also the description's
extent for the credits (§4.8: a list of boxes, `"world"` for none).

```toml
name = "NRCan HRDEM, 2 m lidar mosaic"
what = "Road elevation, Canada"
credit = "Natural Resources Canada"
licence = "OGL-Canada-2.0"
source = "https://open.canada.ca/data/en/dataset/0fe65119-e96e-4a57-8bfe-9d9245fba06b"
surface = "ground"
heights = "CGVD2013"
[data]
format = "geotiff"
index = "hrdem-tiles.geojson"
read_resolution = 8
```

**Checks** (built: crate::inputs::elevation). Errors: a file not of the drop box's shape
(`el-shape`); a description's field missing or of the wrong type (`el-desc`, §5.1); a declaration's
own fields wrong: an unknown `surface`, `heights` (or `mixed`) or encoding, a `code` missing, a tile
template without its placeholders, not exactly one way of naming files (`el-decl`); an index that
doesn't parse (`el-index`); an index or files named that aren't there (`el-missing`); data that
can't be read or measured: a header that can't be read, a file the server hasn't, a CRS the app
can't project (EPSG 4326, 4269 and 3979 today: `pipeline::dem::proj`) or that contradicts its
index's, a tile pyramid without a territory or whose territory the pass's outlines lack, or whose
tiles aren't 256 × 256 (`el-data`; the sampler refuses such a tile too).
Warnings: a dataset's folder or an index no declaration names (`el-orphan`); two datasets of one
rank, resolution and surface meeting (`el-tie`: their order would fall to their ids). Planned: a
dataset whose files don't meet its declared territory. A declaration is held with the index it
names.

**Ingesting.**
- **The order at a point:** the datasets whose coverage holds it, by `rank` (none: 0), then
  resolution, finest first, then ground over surface, then id; the first with valid data there gives
  the height and its `code`. A GeoTIFF dataset's files are tried in its index's order, a point in
  those whose footprint holds it (min x ≤ x < max x, min y < y ≤ max y in the files' CRS: today's
  HRDEM and 3DEP choice exactly), sampled as today (crate::dem::sample).
- **Blended at the edges** (built: crate::dem::generic): where the next dataset with valid data
  there (B) lies beneath the first (A), and the point is within a band of four of the coarser one's
  pixels of A's invalid area (the nearest pixel cell where A has no data, or past its files' edges,
  measured on A's own pixels, a neighbouring file's where the band crosses into one), the two blend
  by a smoothstep of the distance d: h = B + (A − B)·s(d / band), s(t) = t²(3 − 2t), as GLO-30
  blends into AWS's tiles by latitude today (plan §6), so a seam shows no step. Past the band the
  height is A's to the bit, and the source byte is A's either way. B is the first dataset after A
  that has valid data at the point; when none does (the next ones are invalid there too), there's
  no blend: A's height to the bit.
- **Heights are converted** to one reference, EGM2008 (FABDEM's and GLO-30's), through each
  reference's geoid grid: a height H in a reference with geoid N_ref over its ellipsoid is the
  ellipsoidal height h = H + N_ref, moved to WGS 84's frame where the reference's frame differs, and
  then H₂₀₀₈ = h − N₂₀₀₈. What each needs:

  | `heights` | Grid (a source declaration, §6.9) | Frame step | Today's datasets |
  |---|---|---|---|
  | `EGM2008` | none | none | FABDEM, GLO-30 |
  | `EGM96` | EGM96's and EGM2008's (NGA) | none (WGS 84) | none today |
  | `CGVD2013` | CGG2013a (NRCan) and EGM2008's | NAD83(CSRS) to WGS 84 | HRDEM, MRDEM |
  | `NAVD88` | GEOID18 (NOAA) and EGM2008's | NAD83(2011) to WGS 84 | 3DEP |
  | `JGD2011` (Tokyo Peil heights) | GSIGEO2011 (GSI) and EGM2008's | none (JGD2011 is GRS80 on ITRF, within centimetres) | GSI's layers |
  | `TWVD2001` | Taiwan's geoid model, once declared | none | none on the NAS |
  | `ellipsoid-WGS84` | EGM2008's | none | |

  The grids are data, declared like any source (`format = "geoid-grid"`: GeoTIFF or GTX); the frame
  steps are published transformations, generic code beside today's projections
  (`pipeline::dem::proj`, which already holds WGS 84's and GRS 80's ellipsoids). A reference whose
  grid isn't declared, or that the app doesn't know, is an error, never a guess. AWS's tiles mix
  sources and references (plan §6) and have no single reference: their declaration says so
  (`heights = "mixed"`), allowed only for the terrain (#144), never for the roads' elevations.
- **Ground over surface** at equal rank and resolution; a surface model ranks over a ground one only
  by `rank` (GLO-30 over AWS's tiles north of 60°N, where those are wrong).
- **FABDEM** is declared with `territory = "world"` and the coarsest rank: the worldwide fallback.
- **Keys** (built, once switched): a unit names the `keyed` digests of the datasets whose coverage
  meets its ways' extent (`agent::build::unit_key`, the datasets held by `agent::tiles`), in place
  of the DEM rules' versions by area (`rules::versions_meeting_but_dem`). A dataset's `keyed` is its
  declaration's fields with its data's digest: its index's content and its dropped files' names and
  contents (set when the version is derived), so adding a tile to an index or a file to a folder
  reruns the units it meets, as adding or editing a dataset does. Planned: the fetched files' pins for datasets downloaded whole.
- **The unit's plan:** the unit job writes `elevation.json` into the unit's folder (the accepted
  datasets in their order, each with its files whose footprints meet the unit's vertices, a few km
  around: `dem::declared::plan`), which `elev` reads (natively or as a task's WebAssembly: it's one
  of the tail's files); without one `elev` runs today's code (publish 1 only).
- **The units' DEM cache** (`sources/dem-cache/`, `cache/dem-units/`): before the switch a vertex's
  cached height stays valid while the versions of the DEM rules it depends on hold (`dem_valid_in_box`,
  through `rules::dem_rules_of`). Built for the switch, coarser than below (`unit::DemValidity`): a
  units' samples file made from the declarations records their digest (`made`: `u32::MAX`, 0, the
  digest's halves, in the rules' versions' place); its samples are valid while the declarations'
  digest is the same, and otherwise none are. Any other sample (the seed's, those made under the
  rules) is valid only where a single dataset's coverage holds its vertex and it's that dataset's
  code (no blend could apply there, and nothing finer covers it). Planned, finer: each cached height
  records the datasets at or above its source's rank that meet the vertex, by their `keyed` digests
  (as an index into a small table of such lists kept with the cache, so a vertex costs a few bytes);
  it's valid while the datasets at or above that rank meeting the vertex are still exactly those. A
  finer dataset declared over it, or its own dataset changed, makes it stale and the vertex is
  sampled again; a coarser dataset added changes nothing. The source byte (`DemSource`,
  crates/roadcore/src/lib.rs:145) becomes the dataset's position in that list.
- **Densification** follows the data: today 8 m in North America and Japan and 15 m elsewhere by
  boxes (crates/pipeline/src/bin/extract.rs:1083–1087, duplicated in reach.rs:57), because those are
  where finer DEMs are; it becomes 8 m where a dataset of 10 m or finer meets the way, else 15 m. This
  changes Canadian ways outside HRDEM's coverage (8 m today, 15 m after), so it's its own step after
  the proof below, not part of it.

**#143, the roads' elevations.** Declarations for HRDEM, 3DEP, MRDEM, GSI's five layers and FABDEM,
written from today's code (dem/mod.rs:40–58, dem/gsi.rs:11–15, dem/fabdem.rs:16, and the HRDEM tile
list and index embedded from `dem/hrdem_2m_tiles.txt` and `dem/hrdem_tile_index.geojson`,
dem/mod.rs:46–47, which become HRDEM's `index`, made by a producer from NRCan's listing). The `elev`
program reads the declarations through one generic sampler; the per-source order (dem/mod.rs:364–471)
goes. In three steps, each its own rebuild:
1. **The declarations and the blend,** heights and spacing as today. Proof, mechanical: every
   unit's vertices the same, and every vertex whose elevation differs lies in a blend band (within
   four of the coarser dataset's pixels of a finer one's valid edge); a check over every unit lists
   any that doesn't, and one is a failure, not an explanation.
2. **Densification derived:** the units whose ways change spacing, and only those, rebuilt; their
   vertices differ by construction, so the proof is that no other unit's key changed.
3. **Heights converted:** every vertex of a converted dataset moves by its grids' difference; the
   proof is that each vertex's change equals N_ref − N₂₀₀₈ (and the frame step) at its position, to
   a centimetre, checked over every unit.
Then deleted: the per-source code and the DEM
rules and credits in rules.rs (:187–204). Taiwan's MOI DTM code is already gone; its stale
mention (crates/pipeline/src/bin/elev.rs:6) and rule (`dem-taiwan`, `meets_taiwan`, rules.rs:38,
:92) go with the DEM rules, the re-keying of old keys (agent/rekey.rs) keeping what it needs.

**Step 1's migration, in two publishes.** The declarations are written (`data/scratch/inputs/elevation/`
in #143's worktree: nine, `hrdem`, `usgs-3dep`, `mrdem`, `gsi-1a`, `gsi-5a`, `gsi-5b`, `gsi-5c`,
`gsi-10`, `fabdem`, with the indexes `producers/elevation/indexes.py` made from the sources'
listings: HRDEM's 56 squares, the same as `dem/hrdem_2m_tiles.txt` names; 3DEP's 1,449 tiles;
FABDEM's tiles in Bristol's 356 zips).

*Publish 1* (the check, the sampler, both read paths). It reads FABDEM's kept tiles from
`sources/fetched/fabdem/` (the generic store's place), and a tile not there from `sources/fabdem/`,
read only, when it's whole there (`SCENIC_STORE_EARLIER`, crate::dem::kept), so nothing is
downloaded or stored twice before the rename. The rollout, in order:
1. *Publish 1 on both Macs.* Nothing changes (the unit isn't shown; every key is as it was; `elev`
   without a plan runs today's code).
2. *FABDEM's store moved:* pause the units (`scenic pause`), rename `sources/fabdem/` to
   `sources/fetched/fabdem/` (one rename on the share), resume. Paused, so no unit job writes a tile
   into either place mid-rename.
3. *Declarations dropped* (the nine `.toml` and three indexes, together: a declaration is held with
   the index it names): staged in a folder beside the drop box, then one rename into
   `inputs/elevation/` (the check takes in at once any file with no findings, so a half-copied set
   would be taken in half). The check reads their headers (a few dozen range requests, and MRDEM's
   coarsest overview) and takes them in when clean. Live: the catalog's credits are the
   declarations' (the three DEM entries of `rules::CREDITS` replaced by their `what`, MRDEM's and
   3DEP's lines beside HRDEM's, GSI's five layers one line), so the catalog is remade once; no build
   step reruns (nothing reads the datasets yet).
4. *The counts:* `scenic-build elevation-proof --root <nas> --out <scratch> --count`: of the units
   recorded, those built again at the switch (more than one dataset meets them) and those re-keyed
   (one alone). Measured on 16 Oct from the declarations in #143's worktree: 242 built again, 43
   re-keyed.
5. *The proof:* `scenic-build elevation-proof --root <nas> --out <scratch> --store <nas>/sources
   --write` (`--write` needs `--store`): each unit's vertices sampled by today's code and by the
   generic sampler; every difference must lie in a blend band with the same source byte. It refuses
   up front when any unit the records list has no kept samples but has roads (`base/<u>` or
   `global/roads/<u>` in the manifest): it would be unproved. A unit with neither has no road
   vertices (the unit run samples and keeps none for it) and counts as proved, listed as such. Only
   when every recorded unit passed does it write `state/inputs/elevation-proof.json`: the accepted
   index proved, the app, the sampler's version (`generic::SAMPLER_V`), the units proved (the
   roadless among them, also listed as `roadless`) and recorded.
6. *The switch:* the next check records it (`sources/inputs/elevation/@switched`) when the proof
   names the accepted index, the sampler this app has, and as many units proved as recorded; a
   declaration edited between, or a newer sampler, makes the proof stale, so it runs again. From the
   switch on, no app older than the one it was proved on leads, is handed the lead, or is granted
   work by the coordinator, a page's work included (its code the lead's): their keys and code are
   today's (`regions::read::switch_app` reads both switches, the coordinator's `min_app`). The
   units' keys then name the datasets meeting them: those a single dataset meets (FABDEM's alone:
   everywhere outside North America and Japan) are re-keyed, not built (`agent::rekey`, their
   heights the same by the proof); those more than one meets are built again with the blends,
   their kept samples reused where one dataset alone covers a vertex. **No rollback past the
   switch** but by `scenic lead take --force --downgrade`.
7. *Publish 2* (once switched): the per-source code (`dem::run`, `dem::gsi`, `dem::fabdem`, the
   HRDEM tile list and index), the DEM rules and their credits in rules.rs, `DemValidity::Rules`,
   the fallback to today's code, and the read of the store's earlier place go; `elev` and the unit
   job refuse to run without the datasets' plan.

**#144, the 3D terrain.** AWS's Terrarium tiles (`terrain_pack::URL`, terrain_pack.rs:15) and GLO-30
north of 60°N (`terrain_north::BUCKET`, terrain_north.rs:102; `NORTH_PIN`, terrain_pack.rs:93)
declared; the terrain's key names their declarations and pins instead of `NORTH_PIN`. The repair
(`roadcore::grid::repair_terrain`), the water flattening and the blend stay the terrain's own code:
they're how the app processes elevation, not knowledge of a source. Proof: the terrain and slope
packs the same (their keys re-keyed: the declarations name what the old pins named). Finer terrain
zooms from the national datasets are a later build question, not an input change.

### 6.9 The remaining sources (#145)

**Kind:** source declarations. **Drop box:** `inputs/sources/`, one `<id>.toml` per source, the
shape of §5.2. Each is made a declaration or kept in code with a written reason:

| Source | Today | Declaration |
|---|---|---|
| OSM planet | `tools/nas/fetch-planet.sh:11–12` (list and mirrors) | `osm-planet.toml`: `format = "planet-pbf"`, the mirrors as `urls`; the NAS's script reads them (`key = value` lines, readable in sh) |
| Planetiler and its data | `sources/basemap/` (jar, Natural Earth, water polygons, lake centrelines), pinned by `water::PLANETILER_VERSION` (crates/pipeline/src/water.rs:54) | `planetiler.toml` (`format = "jar"`, `version`), `natural-earth.toml`, `water-polygons.toml`, `lake-centerlines.toml`: pinned (`version`), files as kept today |
| ESA WorldCover | `landcover.rs:22` | `format = "geotiff"`, `url` template, `version = "v200 2021"`; its class table (landcover.rs:26) is the format's legend, generic |
| Meta & WRI canopy height | `trees/mod.rs:54` (`CHM10_URL`), the `lat=-0.0` naming quirk (:122) | `format = "geotiff"`, an `index` made by a producer from the bucket's listing, so no naming rule lives in code |
| Leaf type, Europe (EEA HRL) and North America (NALCMS) | `dem/leaftype.py:48–63` | two declarations; their reading (an ArcGIS export, a zip read in ranges at a fixed offset, leaftype.py:51–52) moves to a producer that makes the squares (`sources/trees/leaf/`), declared as files |
| Overture buildings | `buildtiles::RELEASE` (crates/pipeline/src/buildtiles.rs:22), `dem/buildings.py:68`, `dem/bldfetch.py:78` | `format = "geoparquet"`, `version = "2026-09-23.1"`, the bucket prefix |
| GHSL building heights | `bld/sources.rs:19`, `dem/bldfetch.py:80–85` | `format = "geotiff"`, its tile grid as an `index` |
| Microsoft's estimated heights | `bld/fill.rs:28` (`ESTIMATES`, a source name inside Overture's data) | a field of the Overture declaration: `estimated_sources = ["Microsoft ML Buildings"]` |
| Wikidata | `dem/heritagewd.py:51` (`WDQS`, QLever), the items job | `wikidata.toml`: the endpoint |
| Wikipedia's API | `dem/heritagewd.py:132` (English Wikipedia's `api.php`, for short descriptions) | `wikipedia.toml`: the API's URL template by language |
| Geoid grids | none today (heights aren't converted) | `egm2008.toml`, `egm96.toml`, `cgg2013a.toml`, `geoid18.toml`, `gsigeo2011.toml`: `format = "geoid-grid"`, pinned (§6.8) |
| Wikipedia pageviews | `dem/pageviews.py:56` (`DUMP`) | `pageviews.toml`: the URL template; the months chosen stay the items job's rule (pageviews.py:55, :61) |
| Fonts | `scripts/fonts.sh:6` (Protomaps' assets) | kept: the app's own assets, built into it, not map data |
| Canada's lake fill (#124, CanVec) | `lakefill::run::SOURCE_DIR` (`sources/canvec/50k/` on the NAS, the provinces' zips downloaded by hand) and `FillOpts::canvec` (its layer, fields and values) | planned: `canvec-hydro.toml`, `format = "zip"` of shapefiles, the zips as its `files`; its layer and values move to the declaration |
| GEBCO, NCEI's coastal relief and Great Lakes bathymetry | on the NAS (`sources/gebco`, `ncei-crm`, `ncei-greatlakes`), nothing on `main` reads them | none until a step reads them |

Each with its proof (the step's outputs the same) and the code's constant deleted.

## 7. The guards

Two guards, both run before every publish (`tools/app/publish.sh` runs the pipeline's whole-crate
checks with `--test`; a failure stops publishing), built on the pattern of
`crates/pipeline/tests/cache_accessor.rs`: one accessor that the code must go through, a test that
finds every way around it, an exemption list (file, function, reason) whose every entry must name a
function that exists (`every_exemption_names_a_function_there`).

### 7.1 Drop boxes only through the accessor (#134, growing with each input)

**Built** (#134): `Out::path`'s refusal, the test (`crates/pipeline/tests/inputs_accessor.rs`,
run before publishing with the others) and `dem/inputs.py`'s `arg`, with today's readers exempt by name,
each naming the task that moves it; the command-line defaults into the drop boxes go with their
readers (#135, #137), and the publish check with a stand-in root whose drop boxes are unreadable is
planned.

String markers alone would miss paths built with `join`, defaults on a command line, and Python. So
the drop boxes are reached only through an accessor that refuses everything else:
- **In Rust:** `inputs::open` (§4.6) and the input check's job are the only code given the drop
  boxes' paths. The NAS root type the steps receive (`Out`, the project root) refuses, at run time,
  a content name under `inputs/`, `translations/` or `descriptions/` asked of it (`Out::path`
  panics, naming its caller: the input check and `inputs::open` never ask it for one), so a reader that
  takes a drop-box path from the records or builds one as a content name fails its first run. It
  doesn't cover a path joined onto the root itself (`Out::root()`, or a `root` a function is given:
  most of today's readers), which the test below is for. Command-line defaults that point into a drop box (today
  `--regions`, scenic-build.rs:3352; `--translations` went in #137) are removed: a step gets its
  inputs' paths from the agent, which takes them from the accessor.
- **The test:** every item in `crates/` outside `#[cfg(test)]` code (a function, `const fn`
  included, by its block; a `const` or `static`; a line outside any is the file's top level) whose
  code (not its comments) names a drop box (`"inputs`, `join("inputs")`, `/inputs/`,
  `"translations`, `}/translations`, `"descriptions`, `}/descriptions`, and each unit's name; a
  line's `sources/inputs` aside) must make an accessor call (`inputs::open(`, `inputs::gate::run(`;
  one named in a string doesn't count) or be exempt, by the item's name and an anchor in the line
  (what it says: so a common name like `new` exempts that line alone), with a reason (the
  backups, GC's never-swept list, the records' names, today's
  readers with their tasks). An exemption that matches no line fails the test too, so the list
  can't outlive what it exempts (`every_exemption_matches_a_line`), and the scanner's own rules have
  a test.
- **In Python:** the steps never build a drop-box path; they receive input paths on their command
  line, from the agent. `dem/` gets a small helper every step uses to open an input argument
  (`inputs.arg(path)`), which refuses a path under a drop box (anything not under
  `sources/inputs/`, `sources/fetched/` or the job's scratch); the same test scans `dem/**/*.py`
  (a function by its indentation, a top-level statement by its name) for drop-box names in code
  outside a function calling `inputs.arg(`, by the same rules (single-quoted forms too; exemptions
  by name and anchor, each matching a line), and
  `publish.sh`'s check that every Python step loads from a copy of `dem/` (plan §8, App
  publishing) runs with a stand-in NAS root whose drop boxes are unreadable, so a step that opens
  one fails there.
- Each input's task adds its readers to the accessor and removes their exemptions; #146 checks that
  none is left.

### 7.2 No source names in the app's code (#146)

A test that fails when a data source's name or URL appears in the app's code:
- **What it scans:** `crates/**/*.rs` (outside tests), `dem/**/*.py`, `web/src/**/*.ts`, `tools/`
  and `scripts/`; not `tools/taskboard/` (the board's) nor `producers/` (§8: per-source code
  belongs there).
- **Markers:** URLs (`http://`, `https://`, `s3://`) except localhost, tailnet and documentation
  hosts (`example.org`) and links to the formats' specifications in comments; the hosts and names of
  every source this inventory lists (§9), and those of every declaration in the drop boxes (read
  from the NAS when it's there, from a list committed beside the test otherwise).
- **Allowed:** formats and protocols (GTFS, GeoTIFF, Terrarium, PMTiles, the Mobility Database's
  catalogue columns), OSM itself (its tags are the map's schema, and OSM links on the map open the
  object: web/src/details.ts:114), links that open a place in another viewer (the G, M and O keys:
  web/src/main.ts:1725–1734) and Wikipedia and Wikidata links built from an item (they open what the
  data names, web/src/details.ts:102, :260).
- **A last sweep** with it: every hit fixed or exempted with its reason.

## 8. How each input is made (#147)

The second half of the owner's request: how each input is made, kept so the map's inputs can be
made again from scratch.
- **Producers live in the repository,** under `producers/<input>/` (`producers/heritage/`,
  `producers/timetables/`, `producers/elevation/`, `producers/languages/`, …): the per-source code
  this series moves out of the app (§9's P rows, about 3,000 lines of per-register Python among
  them), with its history, review and tests. They're outside what `publish.sh` ships and outside the
  guards' scans (§7): the app never runs them. Each runs on its own
  (`uv run python producers/heritage/fr_merimee.py --out <folder>`), writing the drop box's shape,
  checked by the input check when its output is dropped. Their Python environment is their own
  (`producers/pyproject.toml`), not the app's `dem/`.
- **Each drop box's `how/`** (on the NAS) says how its input is made and points to its producers in
  the repository; it holds what isn't code: the briefs and models of agent-made inputs
  (translations: Haiku; descriptions: Sonnet; the ferries' researchers' `PROMPT.md`, with how a
  batch is piloted and checked), and what the owner did by hand (the MTR's research, the registers
  downloaded by hand).
- Then `inputs/README.md` (the drop boxes, the kinds, the input check), plan.md (§3's tree, §6's Hand-made
  inputs replaced by a pointer here, §7's folders), formats.md (the index, the status's `inputs`, the
  catalog's `inputs` and credits by id) and the diagram's curated inputs brought up to the drop
  boxes.

## 9. The inventory: every place the code names a specific source

Every line was found by search (`https?://`, hosts, source and dataset names, credits, hand lists)
across `crates/`, `dem/`, `web/src/`, `tools/` and `scripts/` on 2026-10-10, and each `file:line`
was checked. **D:** becomes a declaration field. **P:** moves out to a producer. **G:** stays as
generic code (a format, a protocol, a rule over declared data). **C:** configuration or tuning, not
a source (stays; out of the guard's scope). The task that changes it is in brackets.

### 9.1 Credits (all D, from descriptions; §4.8) [#134, then each input's task]

- `crates/pipeline/src/rules.rs:179` `CREDITS`, 42 entries, each `what` at: 181 OSM; 187 road
  elevation, North America; 193 GSI; 199 FABDEM; 205 AWS terrain; 211 Meta & WRI canopy; 217 EEA leaf
  type; 223 NALCMS; 229 ESA WorldCover; 235 UNESCO World Heritage; 241 France; 247 Andorra; 253 Parks
  Canada; 259 US designations; 265 Québec; 271 Ontario; 277 Nova Scotia; 283 CRHP provinces; 289
  biosphere reserves, geoparks, dark-sky places; 295 England; 301 Scotland; 307 Wales; 313 Northern
  Ireland; 319 Guernsey; 325 Ireland; 331 Spain; 337 Portugal; 343 Hong Kong; 349 Japan; 355 Taiwan;
  361 Singapore; 367 Wikidata local names; 373 OSM rail; 379 stops and sights details (Wikidata via
  QLever); 385 peak prominence (the app's, from OSM and the terrain: G, kept as the app's own); 391
  heritage descriptions; 397 park and area details; 404 rail service frequency (names a dozen
  operators: each feed's own credit instead); 410 OSM ferries; 416 ferry sailings (likewise); 422
  roadside buildings; 428 3D buildings (repeats the Overture pin); 434 GHSL.
- `rules.rs:129–175`: the credits' area boxes (`EEA_LEAF`, `FRANCE`, `ENGLAND`, `SCOTLAND`, `WALES`,
  `NORTHERN_IRELAND`, `GUERNSEY`, `IRELAND`, `SPAIN`, `PORTUGAL`, `CANADA`, `QUEBEC`, `ONTARIO`,
  `NOVA_SCOTIA`, `CRHP_PROVINCES`, `NRHP_STATES`): go, extents derived (§4.8).
- `rules.rs:449` `catalog_credits`: G (reads the descriptions).
- `crates/server/src/main.rs:765` `credits_of`'s fallback to `rules::CREDITS`: goes (§4.8).
- `crates/pipeline/src/trees/mod.rs:52` the tree tiles' meta source string: D (from the
  declarations). `crates/roadcore/src/lib.rs:145–172` `DemSource` and its labels (the per-way source
  shares, crates/server/src/ways.rs:18): D (the datasets' names and their order) [#143].
- `web/src/basemap.ts:397`, `:456`: MapLibre attribution strings naming AWS, Copernicus, OSM,
  OpenMapTiles, NRCan, USGS; MapLibre's control is off (web/src/main.ts:135), so they never show:
  delete [#146].
- `web/src/trees.ts:25`, `:27` (layer help naming Meta/WRI, Copernicus HRL, NALCMS),
  `web/src/buildings.ts:834` and `web/src/ui/buildings.ts:84` (Overture's line),
  `web/src/ferries.ts:559` ("Route: OpenStreetMap"), `web/src/overlays.ts:751`, `:776`, `:781`,
  `:867`, `:883`, `:896`, `:903` (source lines in popups), `web/src/details.ts:270–276` (a
  description's Wikipedia credit): D, from the catalog's credits or the record's own `source` (as
  overlays.ts:54, :860 already do) [#146].
- `web/src/ui/strip.ts:330` `APP_CREDITS` (the colour ramps): G, the app's own.

### 9.2 Elevation and terrain [#143, #144]

- `crates/pipeline/src/dem/mod.rs:1–19` the priority order (doc), `:40` `NRCAN`, `:43`
  `NA_WEST_OF`, `:46–47` the embedded HRDEM index and tile list (`dem/hrdem_tile_index.geojson`, 66
  tiles; `dem/hrdem_2m_tiles.txt`, 56), `:50` HRDEM's path, `:54` MRDEM's, `:58` 3DEP's, `:61`
  `in_japan`, `:94` 3DEP's tile naming, `:126–136` per-source stats, `:364–471` the per-source order:
  D (declarations; the index by a producer: P).
- `dem/gsi.rs:11` `HOST`, `:15` `LAYERS` (five layers, zooms, codes): D; `:21–38` the PNG decoding:
  G (the `gsi-png` encoding).
- `dem/fabdem.rs:16` `BASE`, `:43`, `:47` the zip and member naming: D (an `index`); the store
  logic: G.
- `dem/proj.rs`: G (projections, chosen by a GeoTIFF's CRS).
- `rules.rs:25–41` the areas and DEM rules, `:60` `DEM_RULES`, `:65` `dem_rules_of`, `:92`
  `meets_taiwan`: go (keys name declarations).
- `crates/roadcore/src/lib.rs:145` `DemSource` codes 0–7, `:161` `NDEM`: D (see §12, the DEM cache).
- `crates/pipeline/src/bin/extract.rs:1083–1087` and `reach.rs:57–58` densification by boxes: G,
  derived from the datasets' resolutions.
- `crates/pipeline/src/bin/elev.rs:6` the stale `SCENIC_MOI_DTM` doc: delete.
- `crates/pipeline/src/terrain_pack.rs:15` `URL` (AWS Terrarium), `:34`, `:909` its template, `:93`
  `NORTH_PIN`, `:107` and `:1709` (and terrain_task.rs:966, handoff.rs:213, rawpack.rs:6) the stores'
  paths: D. `terrain_north.rs:102` `BUCKET` (GLO-30), `:99` its naming, `:114–128` its tile list:
  D; its blend band and fallback rule (`:1–21`): G. `terrain_z8.rs:20` `V`: C.
- `crates/pipeline/src/coord/mod.rs:446` `WEB_HOSTS`: derived from declarations.

### 9.3 Trees, land cover [#145]

- `crates/pipeline/src/landcover.rs:22` WorldCover's URL: D; `:26` its class table: G (the legend
  of the format); `:38` its tile naming: D (an index).
- `trees/mod.rs:54` `CHM10_URL`, `:113–127` its names and the `lat=-0.0` quirk, `:60–61` the
  resolutions (read from the data instead), `:131` leaf squares' names: D.
- `trees/squares.rs:248` runs `dem/leaftype.py --make`: P (the squares made by a producer).
- `dem/leaftype.py:48` EEA's export URL, `:50` NALCMS's zip, `:51–52` its member offset and size,
  `:54–58` NALCMS's classes, `:62–63` the boxes: P (the producer) and D (its declarations).
- `dem/trees.py:246`, `:257–264`, `:508`: the Python fallback (`SCENIC_TREES_PY=1`), D as above, or
  delete with the fallback.
- `rules.rs:129` `EEA_LEAF`: goes.

### 9.4 Buildings [#145]

- `crates/pipeline/src/buildtiles.rs:22` `RELEASE`, `:38`, `:43` its store: D. `bld/sources.rs:19`
  `GHSL_DIR`, `:22–24` Overture's store: D; `:26+` the GeoParquet row groups: G.
- `bld/fill.rs:28` `ESTIMATES`: D (§6.9); `:57–92` storey heights and sizes per country (fitted):
  C; `:94–99` Overture's class lists: G (Overture's schema, a format).
- `bld/prep.rs:644` meta `ghsl`: D; `:397` Overture's OSM id form: G (its schema).
- `dem/buildings.py:68`, `dem/bldfetch.py:78–85`, `:367`, `:422`, `:674`, `:760–774`: D (the
  declarations), the GHSL grid an index (P). `dem/bldprep.py:54–57`, `:83`: G (Overture's schema).
  `dem/bldmeasure.py`: a one-off analysis, hand-run: P (`producers/buildings/`).

### 9.5 Heritage [#138, #139]

Done in #138 (the registers an input, §6.5): the functions per register, their credit strings,
`STATES`, the register order and `SOURCES` moved to `producers/heritage/` (P; credits D, in the
registers' descriptions); dem/federal.py and dem/crhp.py likewise; `dedupe` and `sparql` stay
generic (dem/heritage.py's `merge`); the credit copied into each feature replaced by its register's
id (§4.8); heritagewd.py's `RULES`, `OSM_REF` and NRHP case replaced by the registers' fields
(`wikidata_property`, `osm_ref_tags`, `ref`), its `WDQS` and English Wikipedia's API left for §6.9;
heritagetiers.py's patterns the producers' `tier`; whsshapes.py's reading of UNESCO's list replaced
by the `unesco-whc` register's `site`, `site_area_km2` and `ref`; `lang_at` moved with the producers
(the names it picks are chosen there); the snapshot's lines in the agent and scenic-build gone.
Left:
- `crates/pipeline/src/markconv.rs` the UNESCO site id from its URL and the id schemes (`whc:`,
  `reg:dfhd:` from the federal entries' `dfhd_id`, `legacy:`): kept as they are, as ids must not
  change (docs/phase5.md, Ids).

### 9.6 Timetables [#140, #141, #142]

- `dem/railfeeds.py`: the catalogue's columns: G (the `mdb-catalogue` format); the declared feeds,
  the catalogue's corrections and the fetch methods (`get-key-json-link`, `get-key-header`): G,
  from the declarations (#140); user agent and retry tuning: C.
- `dem/railgtfs.py`: G (the counter). `:47` `mode_of`: G (GTFS route types).
- `dem/mtrpairs.py` (whole, hand-run): P (`producers/timetables/mtr.py`), deleted (#141).
- `dem/ferrygtfs.py` (the ferry counter, run by the ferries job): G (§6.7.0, #140); `dem/staticgtfs.py`
  (the static files' calendar and count rows): G (§6.7.2, #141).
- `dem/ferries.py:10–14`, `:326`, `:510` the sources' precedence and reading: G; `:49` `URBAN_NETS`,
  `:58`: C.
- `crates/pipeline/src/rail.rs` the rail sources' logical paths (catalogue; the MTR's and its
  pairs gone, #141): D; the territories' own codes: D (the catalogue declaration's `own_code`, #140).
- `bin/scenic-build.rs` `rail_step`: G; the MTR's pairs merged: gone, the static files' count rows
  read instead (#141).
- `crates/pipeline/src/ovconv.rs` the ferries job reading `inputs/ferries/freq`: reads the static
  files' accepted version instead (#141).
- `crates/pipeline/src/stations.rs:24` `INTERCITY_WORDS` (brands in OSM names): C.

### 9.7 Names and languages [#136, #137]

- The languages by territory: D, the languages input (#136). `spoken.rs` `READ_AS`: G (a lookup
  rule). `producers/languages/cldr-languages.py`: P.
- `tools/names/convert-tables.py` (the one-off conversion, its per-area tables at :32–42): P
  (`producers/translations/`, history). `tools/names/check.py`: G (the translations' checks, run by
  the input check).
- `crates/pipeline/src/osmpass.rs:147` `BASEMAP_LANGUAGES`: C (derivable from the languages input
  later).

### 9.8 Basemap, OSM, Wikidata, pageviews, fonts [#145]

- `tools/nas/fetch-planet.sh:11–12` the planet's list and mirrors: D.
- `crates/pipeline/src/water.rs:54` `PLANETILER_VERSION`, `agent/mod.rs:3457` and
  `bin/scenic-build.rs:233` the jar's path: D. `osmpass.rs:618–633` Planetiler's arguments
  (`--download` fetching its own data): G, with the data declared and given as files instead.
- `dem/pageviews.py:56` `DUMP`: D; `:51–55`, `:61` languages and months: C.
- `dem/items.py:65` the Wikidata properties asked for: C (Wikidata's schema).
- `scripts/fonts.sh:6`: C (the app's assets).
- The user agent string, repeated in about a dozen scripts (heritage.py:67, heritage_eu.py:73,
  railfeeds.py:69, leaftype.py:44, trees.py:247, bldfetch.py:77, pageviews.py:50, buildings.py:28 …)
  and terrain_pack.rs:24: C, one shared setting.

### 9.9 Regions [#135]

- `crates/pipeline/src/agent/recipes.rs:26–56` `Outline::Geofabrik`, `Poly`, `Place` and their
  parsing, `coverage.rs:374` `file_rings`, their rings from `inputs/outlines/`, `circle` (:361), `agent/mod.rs:5964` `regions_digest`'s
  outline files, `server/src/regions.rs:663` `coverage_from_recipes`: go (§6.1.10; every reader, §6.1.7). `dem/bldfetch.py:248–250` reads
  `inputs/outlines`: goes.

### 9.10 Direct reads of drop boxes (each moved behind the accessor, §7.1)

Not source names, but every place that would bypass the input check:
- Regions: the readers listed in §6.1, `scenic-build`'s `coverage_of` (bin/scenic-build.rs:3352),
  the outline files at agent/mod.rs:652 and :4213 [#135].
- Translations: `scenic-build`'s `--translations` default and the server's `livefolder` copy:
  gone (#137; the server reads the accepted copies, crates/server/src/accepted.rs).
- Descriptions: `namestodo::run`'s read of `descriptions/` and the server's copy: gone (#137).
- Ferries: `input_digests`' read of `inputs/ferries/freq`'s looked-up timetables (agent/mod.rs),
  the ferries job's (ovconv.rs): gone, the static files' accepted version read instead [#141].
- Keyed feeds: `key_names` of `inputs/keys.env` (agent/mod.rs:4658): stays (keys.env isn't an input),
  exempt with its reason.
- The never-swept and backed-up folders: `agent/gc.rs` (`NEVER`, which also holds all of
  `sources/` but `sources/inputs/`, swept since #134: §4.9) and `agent/backup.rs` (`FOLDERS`, with
  `state/inputs/accepted` since #134; the old `translations/` and `descriptions/` out of it since
  #137). GC's `NEVER` keeps naming the two old folders, so a file dropped there is flagged by the
  input check, never swept.
- The catalog's key, `catalog_key` (agent/build.rs): its regions digest becomes the accepted index
  [#135]; it gained the credits' digest (§4.8) [#134, built].

### 9.11 Not sources (checked, left as they are)

- Road network codes by country (`bin/extract.rs:393–523` `network_code`, `roadcore::network`,
  `web/src/mapschemes.ts:88–114` their colours and legend): C, rules on OSM's `ref` tags, versioned
  by area in `rules::RULES` (`networks-*`).
- Map scheme names ("OSM Carto", "Michelin", …, `web/src/mapschemes.ts:40–61`): C, palettes.
- Links that open a place elsewhere (`web/src/main.ts:1725–1734`, `web/src/details.ts:102`, `:114`,
  `:260`): G.
- `tools/taskboard/`: the board's own (Google Fonts in its page).

**Size:** about 75 places in the Rust crates, 110 in `dem/`, 40 in `web/src`, `tools/` and
`scripts/` (counting each line or block cited above once). By kind: about 95 become declaration
fields (42 of them credits, 16 credit boxes that go, about 30 URLs and pins, the rest hand lists and
dataset facts); about 45 move to producers (31 functions per register or source, the HRDEM index,
the leaf-type squares, the MTR's converter, the CLDR generator); about 55 stay generic (formats,
protocols, the merging rule, the counters, gtfs.py's among them); about 35 are configuration. Plus
the dozen direct reads of drop boxes of §9.10.

## 10. The legacy pipeline: what's left

`make run` and `make dev` are gone: the Makefile and the Python steps only it ran were deleted
(commit 2992c71), and nothing in the repository names them. What remains (plan §10, phase 6,
"Cutover: done"), each checked in the code on 2026-10-10:
- **The old build's heritage files** (`global/legacy/`, still on the NAS): their readers, the
  fallbacks `markconv::LEGACY`, `agent::build::heritage_src` and `overlays_key`'s branch, were
  removed in #138 (the marks and overlays wait for the pass's heritage job instead); the files and
  `layers/basemap/legacy-*.pmtiles` are to be deleted from the NAS once a published app has no
  readers. (`sources/legacy/` is already gone: the owner deleted it on 9 October.)
- **The registers' legacy names:** `sources/registers/legacy`'s only reader since #138 is the heritage
  job's first start, for its caches (`pipeline::answers::heritage_start`, while no pass has a
  heritage archive); its archive is the PM's to retire once one has. `legacy-seeds` has no reader
  since #139 (§6.5.3); its archive is the PM's to delete.
- **The `terrain` and `slope` programs' build-folder modes** (crates/pipeline/src/bin/terrain.rs:66,
  bin/slope.rs:185, defaulting to `data/build`): `terrain --scan` is live; the folder modes go in
  #144.
- **The heritage chain's stand-in root** (`data/build` in a scratch root, §6.5.2): its
  `data/heritage` is the chain's answers since #138; the rest stays.
- **Names that say legacy for live code:** `pipeline::legacy` (the units' tile and base packs: a
  rename, not a removal, out of this series), and the `legacy:` id prefixes (kept: ids don't change).
- **Real-data tests and an example reading a checkout's `data/build`**
  (crates/names/tests/real_data.rs, crates/names/examples/load.rs:35,
  crates/store/src/pmtiles.rs:812): `#[ignore]`d, opt-in; out of this series.

## 11. Order and proofs

The board's order, with what each needs and proves:

| # | Task | Needs | Proves before deleting |
|---|---|---|---|
| 133 | this spec | | |
| 134 | the input check (§4), on `_gate-test` (built; its trial ran on 2026-10-10) | 133 | its behaviour on the NAS between two Macs (§4.10: `tools/check/gate-two-macs.sh`) |
| 135 | regions (§6.1), after 136 and 137 | 134 | coverage fingerprints unchanged: no build step reruns; the catalog is remade once (its key names the regions' digest); every `.poly` converted selects the same coverage |
| 136 | languages (§6.2) | 134 | `global/spoken`'s content name unchanged |
| 137 | translations, descriptions (§6.3, §6.4; built) | 134 | the served tables and descriptions unchanged |
| 138 | heritage registers (§6.5) | 134, 136 (the chain's languages) | heritage-sites, heritage, marks, overlays unchanged or explained |
| 139 | the seeds retired (§6.5.3) | 138, 137 | the heritage layers' English and park facts unchanged or explained |
| 140 | feeds (§6.7.1) | 134 | the feed list, zips and `global/railfreq` unchanged; every `gtfs-<feed>.json` record reproduced field for field by the app's ferry counter on the same zips and days |
| 141 | static GTFS (§6.7.2) | 140 (the counters) | the MTR's pairs byte for byte; the 658 looked-up ferry records field for field |
| 142 | to-do and report (§6.7.3) | 141 | (new output; nothing deleted) |
| 143 | elevation, roads (§6.8) | 134 | step 1: every differing vertex in a blend band (checked, not explained); step 2: only the respaced units' keys change; step 3: each vertex moves by its grids' difference |
| 144 | elevation, terrain (§6.8) | 143 | terrain and slope packs unchanged |
| 145 | the remaining sources (§6.9) | 134 | each consumer's outputs unchanged |
| 146 | the guards (§7) | 135–145 (§7.1 from 134 on) | both pass; publish runs them first |
| 147 | how each input is made; docs (§8) | 135–145 | |

Each migration task, in order: write the producer or the declarations; drop today's data in its
shape; run the input check on it (accepting today's warnings, each listed in the task's report); switch
the readers to the accepted version, re-keying where the new keys name what the old pinned; prove;
publish; only then delete the old path and its NAS files (after GC's 14 days for content-named ones).

## 12. Risks

- **Every step's reads move.** Regions alone are read in about twenty places today (§6.1), and
  §9.10 lists a dozen more direct reads. A reader missed would read the drop box and bypass the
  input check; the accessor (§7.1, from #134) refuses such a read at run time and its test finds it in the
  code, Python included, so a missed reader fails instead of bypassing quietly.
- **The input check needs a lead.** Checks run as jobs the lead grants: with no lead (every Mac asleep or
  away), a drop waits. Translations go from about a minute and a half to the next listing plus a
  check, a few minutes; if that matters, the map's server can show a dropped file's lines as
  "pending" before they're taken in (not planned).
- **Big checks.** A heritage register of a few hundred thousand entries takes tens of seconds to a
  minute to check. Only files whose size or time changed are read (§4.2): translations' 808 MB are
  read whole once, at migration, then only the batches dropped. A same-size, same-time rewrite is
  missed until the daily full hash (§4.2). The whole-version re-check (§4.3) reruns the unit's
  cross-file checks (matches, references), not every file's shape checks.
- **Today's warnings at migration.** check.py on the converted translations, the registers' `approx`
  shares: each migration accepts what's there once, listed, so the input check starts clean
  without hiding anything.
- **Heritage's merge order.** Reading registers in id order instead of today's hand order may change
  which register a merged place names first; positions shouldn't change. Shown and explained in
  #138.
- **Elevation's three changes.** Blending changes elevations near datasets' edges, the derived
  densification respaces Canadian ways outside HRDEM, and converting heights moves every vertex of
  HRDEM, MRDEM, 3DEP and GSI by up to a metre or two. Each is its own step with a mechanical proof
  (§6.8), so none hides in another's. Converting means every unit in North America and Japan is
  sampled again (its cache entries' datasets unchanged, but their heights converted): the cache
  stores converted heights, so it's remade once in step 3, at about the cost of those units' elev
  runs.
- **The DEM cache's new validity** (§6.8): today's cache is valid by rule versions; its first
  conversion to dataset lists (each vertex's datasets at or above its rank) is made from today's
  source codes and the declarations' extents, once, in #143.
- **Geoid grids and frame steps** are new code and data: a wrong grid or sign would move every
  vertex. Step 3's proof checks each vertex's move against the grids at its position, and the grids'
  values at published test points (each agency publishes some) are checked when they're declared.
- **Feeds' derived boxes.** A declared feed's box comes from its `stops.txt`, read once fetched; a
  feed is fetched once before it's known whether it's near the coverage. That costs a download per
  new declaration, never a wrong count.
- **Ferry pins.** OSM ids pinned in static GTFS (every counted ferry route has one: the 658
  migrated records) go stale when OSM changes a ferry; the report (§6.7.3) lists each pin naming
  nothing in the pass, every pass.
- **Rail routes matched by pairs.** A rail route is reported when none of its stop pairs in the
  coverage found a track; one with a single pair matched isn't, so a route running partly on a line
  OSM lacks goes unreported. Matching routes to lines by their tracks is planned.
- **The ferry counter's reference day.** Counting from a zip's fetch day instead of the day the
  counter runs makes counts deterministic, but a feed whose copy is old counts its seasons from then;
  the feeds' weekly refresh (`days:7`) keeps that within a week, as for rail.
- **Older apps.** An older lead plans from the drop boxes and an older server reads the old folders
  (§4.11); the same-app rule and two-publish moves keep that window to the updaters' minutes.
- **Credits in transition.** Until every input has its description, the catalog's credits come
  partly from descriptions and partly from `rules::CREDITS`; #134 merges both, the descriptions
  winning for a source both name, so no credit is lost.
- **SMB.** The input check adds a listing of about a dozen folders every two minutes and a few content-named
  writes per change: well within the share's limits (plan §3), none in a hot path.

## 13. Open questions, decided by default

Points the owner hasn't decided, each with the option most consistent with their decisions and
principles. Each is easy to reverse before its task. Those marked **decided** were settled at the
spec's review (10 October 2026, by the project manager the owner delegated approval to).

1. **Validation runs as a job, not in the lead's loop.** The owner asked that the agent check every
   change; the pool's principle is that every job hands off and the lead alone merges. A check job
   keeps both: any member checks, the lead's merge takes it in, and nothing new writes the records.
2. **Holding is per file, with the whole version checked (decided).** "An error or a warning pauses
   ingesting it": "it" is the changed file (or the files a cross-file finding names, and the files
   paired with them), so one bad translation file doesn't hold a hundred good ones; and the version
   that would result is checked whole, every change of the unit held together if it raises a
   finding (§4.3). The banner says which files are held and why.
3. **Acceptances live in `state/inputs/accepted/`, not in the drop box.** A file in the drop box
   would itself be a change to check; create-new files in `state/` keep one writer per file and are
   backed up with `inputs/`.
4. **Translations and descriptions move under `inputs/`.** One root for every drop box (one listing,
   one backup rule, one place to look). The cost is a one-time move in two publishes (§4.11), and a
   finding for anything dropped in the old folders afterwards (§3).
5. **Two timetable units, one input.** Feeds (declarations) and static GTFS (data) have different
   checks, so they're checked apart, but counted by the same counters and shown as one input.
6. **Static GTFS carries seasons; one counter per mode, the ferries' keeping their fields
   (decided).** Calendar ranges are read as month and day, the year ignored (no expiry, seasons
   kept); `months`, `checked` and the `osm` pin are extra columns and `counts.txt` an extension table
   for services known only by counts, so no departure time is invented; sources in `route_url`
   (required). Rail's counter gives trains per stop pair, the ferries' today's ferry record
   (§6.7.0).
7. **Timetable matching problems are a report, not a warning of the input check:** they depend on OSM, which
   changes each pass; a warning would hold the owner's file again every pass.
8. **Feeds carry no country or box.** The box is read from the data (`stops.txt`), so declaring it
   would be a hand figure that goes stale; which countries' coverage takes a rail feed is its
   description's `territory` (§5.1); the catalogue's country rule stays, since it reads the
   catalogue's own fields.
9. **Heritage geometry:** points are sites, polygons heritage areas, lines lines; a polygon shows a
   dot only with `dot: true`, which reproduces today's map (Québec's perimeters and Ontario's
   districts have no dot of their own).
10. **Heritage merging keeps today's rule exactly** (the same Wikidata item, or a local entry within
    40 m of a same-or-higher one sharing a name word), plus the owner's pairs; registers read in id
    order for determinism.
11. **Territories are ISO codes or `"world"`, one field everywhere; entries outside are a warning
    (decided).** No boxes. A register's credit extent is its entries' box. NRHP's legation in
    Tangier is why it's a warning.
12. **Elevation:** finest resolution first unless `rank` says otherwise; ground over surface at a
    tie; a blend band four coarser pixels wide. **Decided:** heights are converted to EGM2008 through
    each reference's declared geoid grid (and frame step), unknown or grid-less references refused;
    densification derived from the datasets (8 m where one of 10 m or finer meets the way); each its
    own step after the blend's proof (§6.8).
13. **Tile pyramids declare a territory** (ISO codes) because their coverage can't be read from a
    header and asking for tiles worldwide isn't practical; it's a fact about the dataset, and tiles
    that answer with no data outside it are simply none.
14. **What's configuration, not a source,** stays in code: road network rules, storey heights fitted
    per country, OSM name heuristics (intercity brands, urban ferry networks), Wikidata properties
    asked for, the pageview months, the user agent. The guards don't flag them.
15. **Producers stay in the repository (decided),** under `producers/<input>/`, outside what publish
    ships and outside the guards' scans, each drop box's `how/` pointing to them: their history,
    review and tests (about 3,000 lines of per-register Python) are kept.
16. **The test input is removed after #134's trial,** its tests staying in the suite.
17. **`OWN_CODE` becomes a declaration (decided):** the catalogue's `own_code` (§6.7.1).
18. **Every `.poly` becomes GeoJSON in #135 (decided, 14 October):** our three and Geofabrik's 31,
    each proved to select the same coverage; `poly:`, `geofabrik:` and `place:` are deleted (§6.1).
19. **Regions have no gap or partition checks (decided, 14 October):** overlaps are harmless and
    gaps are the owner's to see on the map's region layer (§6.1), not a warning.
20. **Popups name their source by register id (decided),** resolved against the catalog's credits,
    and the catalog's key carries the credits' digest, so a credit edit reaches the map with the next
    catalog and nothing else reruns (§4.8).
21. **Where `osm:` parts and lines' ways come from (decided by the owner, 14 Oct).** The pass's outline
    set holds administrative and ISO 3166 relations only (plan §5). A region naming another
    relation, or a line naming ways, needs their geometry from the same pass: a small
    extract of exactly the ids the regions name, made from the pinned pass's planet once per pass
    and per set of ids, content-named, which the check reads. A region naming a relation or way
    not yet in the extract makes the check run the extract again against the pinned pass first.
22. **The regions' open questions (decided by the owner, 15 Oct):** every pick of §6.1.11 kept but
    three: a file in `inputs/regions/` that's neither a region, a shape nor `pins.toml` is ignored,
    not an error; regions crossing the antimeridian are supported now (each region computed in its
    own longitude frame, stored split at ±180°); the app writes no region, the Regions panel
    listing them read only.
