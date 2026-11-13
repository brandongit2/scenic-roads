# Inputs: every input through a standard shape and a checked inputs folder

Every input the map is built from has one written shape and one folder on the NAS, its **inputs folder**. The owner makes inputs however they like, to that shape, and adds them there. The build
checks each change before taking it in, keeps the last good version while a change is held, and
reads only checked, content-named copies, so builds stay deterministic. Per-source conversion
happens outside the app (`producers/`, §8); what the app must know about a source (where to fetch
it, its licence, its credit) is written in a declaration file in an inputs folder.

**The input check** (§4) checks every change to an inputs folder: first its shape, then the
input's own rules. A change that passes is copied into `sources/inputs/` and becomes the version the
build reads. A change with a problem is held: the app keeps the last version that passed, shows the
problem on the menu bar icon and the build page, and takes the change in once
it's fixed or, for a warning, once the owner accepts it. Each inputs folder is checked on its own and is
a **unit** of the input check. (In the code the mechanism keeps the older word "gate":
`gate_work`, `_gate-test`.)

Built: the input check and its units for regions, languages, translations, descriptions, heritage
registers and matches, timetable feeds and static GTFS, elevation datasets (the roads' and the 3D
terrain's), the remaining sources (§6.9); the guards (§7). Companions: `docs/steps.md` (pipeline, keys), `docs/building.md` (order), `docs/formats.md`
(files), `docs/pool.md` (lead, jobs, journal), `docs/phase5.md` (the heritage chain).

## 1. Constraints

- **Determinism** (docs/building.md §2): every input reaches a job's key by content (a content name or a digest
  of its bytes), never by a path, a size or a time. The one exception is a `range` source (§5.2),
  read in place a range at a time, which can't be pinned by content.
- **No hand-kept figures:** a declaration states facts that can't be read from the data (licence,
  height reference, where it is); everything readable from the data (resolution, coverage, extent,
  counts, a feed's box) is read at check time. Where a declared fact can be checked against the
  data, it is, and a wrong one is a finding.
- **The NAS's limits** (docs/storage.md): no input is stored or read as thousands of small files, and
  nothing lists an inputs folder in a tight loop.

## 2. The three kinds of input

| Kind | What the owner adds | The app's side | Missing data means |
|---|---|---|---|
| **Declare** | the data itself, in the input's shape | reads it as given; the files are the whole input; never writes there | the map has none of it |
| **Request–fulfil** | answers to the app's requests | writes a to-do list and a brief into the inputs folder's `todo/` after building; reads the answers like a declare input | the map falls back, the request stays listed |
| **Source declaration** | a small file naming a standard format, where to get the data, its licence and credit (data may lie beside it) | fetches the data itself into `sources/`, content-named and pinned in the records | the source isn't used |

Not inputs: `inputs/keys.env` (secrets; their names enter keys, never their values), `todo/`
lists, and `how/` notes (§8).

## 3. The inputs folders on the NAS

Every inputs folder is a folder under `inputs/` in the NAS project folder (docs/storage.md):

```
inputs/
  regions/            declare: <id>.toml, <name>.geojson in any subfolders; special files _group.toml, _pins.toml, _rules.toml (§6.1.2)
  languages/          declare: territories.tsv, about.toml (§6.2)
  translations/       request–fulfil: **/*.jsonl; todo/ (§6.3)
  descriptions/       request–fulfil: **/*.jsonl, about.toml; todo/ (§6.4)
  heritage/           declare: <register>.geojson + <register>.toml (§6.5)
  heritage-matches/   declare: *.jsonl (§6.5.4)
  timetables/
    feeds/            source declarations: <feed>.toml (§6.7.1)
    gtfs/             declare: <name>.zip [+ <name>.toml]; todo/ (§6.7.2, §6.7.3)
  elevation/          source declarations: <dataset>.toml, indexes, <dataset>/ (§6.8)
  sources/            source declarations: <id>.toml, indexes (§6.9, the depth's §6.10)
  extents/            source declarations by theme: <theme>/<source>.toml + .geojson, <theme>/score.toml (§6.11)
  keys.env            not inputs
```

Each folder is one unit (`timetables/feeds` and `timetables/gtfs` are two units of one input).
Inside any inputs folder:
- `todo/` is the app's and `how/` the owner's notes (pointing to `producers/<input>/`, §8); neither
  is read as input.
- Names starting with `.`, `@`, `#` or `_` are ignored (`@eaDir`, `#recycle`, `.DS_Store`, drafts),
  except the regions' special files (`_*.toml`, §6.1.2).
- A file is taken only once its time is 10 s old, so a half-written file isn't checked.
- Any other file that isn't the inputs folder's shape is an error finding, never skipped silently
  (except in `regions/`, which ignores unknown files: §6.1.2).

A file added to the old NAS-root `translations/` or `descriptions/` is an error finding on the
new unit (`moved`); GC's never-swept list keeps those folders. The checked copies live under
`sources/inputs/` (§4.6) and are the app's, never the owner's to edit.

## 4. The input check

crate::inputs; `scenic-build inputs`; the agent's `gate_work` and `tend_gate`. Units are listed in
`inputs::UNITS`. A unit with nothing added and no records isn't checked or shown.

### 4.1 Terms

- **Candidate:** the inputs folder's files as they are now, once they've held still.
- **Finding:** an **error** (the file can't be used) or a **warning** (usable, likely a mistake),
  with a stable id (§4.4), the files it's about, a message, and a place when it has one.
- **Held:** a change with an unaccepted finding; the last accepted version of its files stays in use.
- **Accepted version:** the checked, content-named copies the build reads, listed by an index whose
  content name is the unit's version (§4.6).

### 4.2 Noticing a change

- The lead (pool.md §5) lists every inputs folder (recursively, skipping `todo/` and `how/` at the top)
  and the acceptances every two minutes, on a thread of its own (crate::inputs::watch). Members
  don't list. `scenic inputs check [<unit>]` asks for a listing now.
- A file whose time is within the last 10 s is left for the next listing. The check is given the
  listing its key was made from (`--listing`, `inputs::Planned`) and checks exactly it: a file
  changed since, or passed over as too recent, stays as accepted (or out, if new).
- The listing is only a trigger; change is decided by content. Only files whose size or time differ
  from the last check's listing (`sources/inputs/<unit>/@listed`) are hashed. A rewrite with the
  same size and time is missed until the daily full check (`inputs <unit> full`, or `scenic inputs
  check --full <unit>`), timed from the records' `full_at` so a new lead doesn't repeat it.

### 4.3 Checking: a job like any other

The step `inputs`, one target per unit (`inputs regions`, `inputs timetables/gtfs`):
- **Key** (`inputs::check_key`): the checker versions (`INPUTS_V` and each input's), the listing,
  the unit's acceptances, the current accepted index's content name, and the content names of
  whatever else its checks read (`Checks::context`: the pass's outlines, the accepted registers for
  the matches, check.py for translations, the regions' extract).
- **Where:** on the lead, either slot, never beside a step that runs alone, another heavy NAS reader
  (`bldprep`) or another check. It holds no region and no map.
- **What it does:**
  1. Hashes the files that may have changed (BLAKE3 hash16, formats.md, Names and hashes); a file
     with the accepted bytes is unchanged whatever its time.
  2. Runs the shape checks on each changed file, then the input's own checks (§6), which may read
     the whole candidate and the context.
  3. Gives each changed file a verdict: **clean** or **held**. A removal is a change. Files that
     only make sense together are held together (a register's `.geojson` and `.toml`, a region and
     the shapes it names, a feed and one that `replaces` it); a file naming something only a held
     file adds is held too.
  4. Assembles the next version: the accepted one with every clean change applied, held files as
     accepted before (a held new file is absent).
  5. **Checks that version whole.** If it raises a finding neither accepted nor raised by the
     current version, every changed file of the unit is held and the version stays; the banner says
     they were held together and why.
     What's derived from the version whole is made again, files unchanged, when the accepted
     index's `checks` isn't this app's (`unit` and the unit's `Checks::version`), or its
     `derived_from` isn't what the unit's checks read beyond the files now (`Checks::derived_from`:
     the elevation datasets' accepted regions and the pass's outlines, §6.8): a bump of a unit's
     version rolls its derived files out by itself, and so does a change of what they're derived
     from. While a file is kept as accepted (held), the index keeps the `derived_from` it had, so
     the check that lets the file go derives it again too.
     The regions keep a region's stored geometry while what it's resolved from is as it was
     (task #832): each region's `facts.resolved_from` is `inputs::regions::RESOLUTION` (their
     checks' version) and the digest of everything its resolution reads (its file, the pins, the
     shapes and regions it names, the pass, the datasets; a region with water: every file of the
     folder but the group files, `resolve::Outcome::keys`). A change that touches none of it (a
     `_group.toml`'s name, another region's file) derives it from nothing: its geometry, and the
     terrain keyed by it, stay. So `RESOLUTION` is bumped in the same commit as any change to the
     resolution's code that changes what an unchanged region resolves to; that, and only that,
     resolves every region again.
  6. Stores each file content-named under `sources/inputs/<unit>/` (uploaded and read back, docs/storage.md).
  7. Hands off the records: `sources/inputs/<unit>/@index` (when it differs; the replaced index is
     touched so GC keeps it, §4.9), `@listed`, and `@held` (the held report, or none). `@` starts
     no inputs-folder path component or unit name, so these never clash with copies or nested units.
- **Threads and progress.** A check lasts hours, one target, so it takes its job's grant as the grant
  changes rather than as a target begins: its agent keeps a file (`SCENIC_THREADS_FILE`,
  crate::control::THREADS_ENV, replaced whole by crate::control::grant, so a read never meets it
  emptied) with the threads its job may use now, which its lead re-decides on
  what the job uses and the cores and memory its Mac has (docs/pool.md §7.1, Threads re-granted:
  more as cores free up while the Mac's memory covers each extra thread at the step's measured
  peak over the threads it was measured at, the whole peak where those aren't recorded; fewer when
  the owner comes back or other work wants the cores), at most 8
  (`pace::MOST`; the regions' work ran four at once before). The file is read again within a
  second (a regions check once took 39 GB and panicked a 16 GB Mac: its memory guard stops one
  that overruns all the same). Tree cover jobs (`trees`) are paced the same way (their blocks'
  bands under permits, docs/steps.md §2).
  The check's pools are as wide as the cores (8 at most). The outermost parallel operation of a
  task takes a `pace::par_map` permit per task, waiting while as many run as the file says; an
  operation inside a task runs its items one after another on that task's thread, inside its
  permit's budget, so a permit's holder never waits for another permit (no deadlock) and nothing
  runs beyond what is allowed. Without the file (a hand run) every core is allowed. It reports
  its progress as six stages (the files read and checked, the candidate checked whole, the next
  version checked whole, the version in use checked whole when the next raised a finding that holds,
  what is derived from the next, the files stored), each counting the stages done before it (the
  files stored at 5/6), with how far within a stage where it knows (files; the regions' resolve
  by its steps, §6.1.4), never going back within a stage, as `progress:` lines like any job
  (docs/building.md); `scenic inputs` shows a checking unit's stage, share, time left and threads
  from the lead's status.
- **Derived facts** (a register's box and count, a GeoTIFF's resolution and extent) go in the index
  with each file, computed from content, so steps read the index rather than every file.
- **Deterministic:** the next version is a function of the candidate's bytes, the previous version
  and the acceptances.

### 4.4 Findings and their ids

- An id (`Finding::new`) is `<check>.<hash16>`: a hash of the check's name, what it's about by
  identity (region ids, register and entry id, feed id, path), and what was found: a place's
  geometry rounded to 100 m; a count's numbers and the two versions' content names (so accepting
  "46,210 → 39,100 entries" doesn't accept a later fall to 4,000); a file's flagged lines' content
  (so a new suspicious line is a new finding). Never the whole file's bytes, so an unrelated edit
  doesn't re-raise an accepted warning.
- **Errors** are faults of the data or shape (unparsable, a field missing or mistyped, an id twice,
  a reference to nothing, a declared fact the data contradicts). They can't be accepted.
- **Warnings** are likely mistakes the owner may mean (a register losing over 10 % of its entries,
  entries far outside their territory, a declaration no step uses).
- Findings about many lines of one file are one finding per file and check, listing the lines (50
  in the banner, all in the report).

### 4.5 Accepting a warning

- **How:** Accept and Accept All in the build page's banner, the menu bar
  item, or `scenic inputs accept <unit> <finding id>|--all`. Errors have no Accept.
- **What it writes:** `state/inputs/accepted/<unit>/<finding id>.json`, create-new (pool.md §2),
  holding who, when, how, and the message. The asking Mac writes it itself (the menu bar and map via
  their agent, the build page via the lead's coordinator), then tells the lead's coordinator
  (`/api/inputs`) so it lists at once; the check's key changes and the file is taken in.
- **Undoing:** `scenic inputs unaccept <unit> <finding id>`; the finding holds again from the next
  check (never the version already in).
- An acceptance outlives its finding; `scenic inputs` lists stale ones and `scenic inputs unaccept
  --stale` removes them. Acceptances are backed up with `inputs/`.

### 4.6 The accepted version and how steps read it

- **Files:** `sources/inputs/<unit>/<path>.<hash16>.<ext>`, the inputs-folder path content-named.
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
  `keyed` digests only what affects builds (a description's credit doesn't: §5.1, §4.8);
  `derived_from` (absent for most units) names what was derived from beyond the files (step 5);
  `accepted` maps the warnings the version was taken with to their files, kept while those files
  are unchanged. No file time is in it, so the same contents have the same version.
- **The records:** the manifest's `sources/inputs/<unit>/@index` names the index, which roots its
  files for GC. Only a check job's hand-off, merged by the lead, changes it.
- **Readers:** `inputs::open(root, manifest, unit)` gives a step a unit's accepted files by inputs-folder
  path, from the records it planned with. Python steps get the paths on their command line from the
  agent. No step reads an inputs folder (§7.1).
- **Keys:** a step names the index's content name when it reads the whole unit, or the `keyed`
  digests of the files it reads (heritage-sites names only the registers meeting its cover).
- **The map's server** reads the translations, descriptions, languages and the regions' index from
  the accepted copies the lead's manifest names, copied to the Mac (crates/server/src/accepted.rs,
  inputs::inputs::served), and from that copy away from the NAS. The Regions panel reads the
  region files read-only plus the accepted index and held report. `names-todo` reads the accepted
  translations and descriptions the agent names.
- Planned: the catalogue recording the accepted index of every unit it was built from.

### 4.7 Where it shows

- **The status** (the lead's agent's `status.json`, docs/building.md §2): `inputs`, one entry per unit
  (crate::inputs::view, the lead's):
  ```json
  {"unit": "heritage", "version": "sources/inputs/heritage/@index.7a…json", "state": "held",
   "checked": 1760100000, "held": ["fr-merimee.geojson"],
   "findings": [{"id": "h-loss.…", "level": "warning", "files": ["fr-merimee.geojson"],
                 "message": "fr-merimee: 46,210 → 39,100 entries (a loss of 15 %)"}],
   "accepted": [], "stale": []}
  ```
  `state` is `ok`, `checking` or `held` (with `checking: true` while re-checked); a finding carries
  its first 50 `lines` and `more`; `together` and `unread` appear when they apply.
- **The menu bar item** (`tools/status`): a warning badge on whatever icon the build has, a line per
  held unit with Accept (warnings, after confirming) and Show on the build page.
- **The build page** (`web/buildpage/dash.js`): a banner per held unit with its files, findings, places,
  Accept and Accept All; a quiet line while checking.
- **The Regions panel** marks a held region (§6.1.9).
- **`scenic status`** prints held or checking units; `scenic inputs` lists every unit's version,
  state, findings and acceptances.

### 4.8 Credits from the inputs

- The map's `credits` (formats.md, The catalogue) are made from the accepted descriptions (§5.1) by
  crate::inputs::credits, beside `inputs::rules::CREDITS` for sources with no description yet
  (descriptions win for a source both name), with one rule for which to list: those whose extent
  meets the coverage, 20 km around it, or a built z6 tile's ways (`rules::listed`). A description's
  extent is `facts.extent` in the index (a box, or `"world"`), derived from the data, never typed;
  one whose extent isn't known is listed nowhere, except the depth sources whose grids meet the
  depth's packs and the timetable feeds the trains and ferries count (§6.7.1), listed wherever they
  are.
- **Credit edits reach the map without rebuilding:** credits enter no step's key, but the
  map's key (`catalogue_key`) has the digest of every accepted description's credit fields
  (`credits::digest`). When descriptions can't be read now, the last digest read stands; with none
  read yet, the map update waits.
- **Popups name their source by register id** (`register`, `registers`), resolved in the map against
  the map's credits (web/src/overlays.ts `sourceOf`), so a credit edit reaches popups with the
  next map update.
- The map's own credits for what the app makes (colour ramps, `web/src/ui/strip.ts`) stay the app's.
  The server's fallback for a map without credits (`credits_of`, which reads `rules::CREDITS`)
  stays while such a map can be served.

### 4.9 GC and backups

- GC sweeps `sources/inputs/` (`gc::inputs_kept`) and no other part of `sources/` (`NEVER`). A file
  there goes once nothing names it and it's 14 days old (`gc::INPUTS_KEEP_S`; GC's inputs sweep,
  docs/storage.md, GC). Named: the
  manifest's and newest records' indexes, listings and held reports, every index touched within
  those 14 days, and the index each recent check
  state names as `replaced`. So a replaced version stays restorable for 14 days. If any folder can't
  be read, nothing is swept in that pass. The inputs folders and `state/inputs/` are never swept.
- Every accepted version of a file is kept whole, so the briefs say to add one answer file per
  batch rather than append to a big one.
- Backups (docs/storage.md; agent/backup.rs `FOLDERS`) cover `inputs/` and `state/inputs/accepted/`, not
  `todo/`.

### 4.10 The test unit and the tests

- **`inputs/_gate-test/`** (crate::inputs::gatetest) isn't real data; it's checked only while
  `state/inputs/gate-test` exists (`scenic inputs test on|off`; `off --forget` removes its
  acceptances and asks the lead to drop its records, the job `inputs _gate-test forget`). Shape:
  `.jsonl` lines `{"k": "<string>", "v": <number>}`; an error for a bad line, a warning for a
  negative `v` and for a removed file. No build step reads it. `tools/check/gate-two-macs.sh` is
  the procedure that exercises the input check on the NAS between two Macs (an update taken in, an
  error held, warnings accepted from each Mac, an unaccept, a touch, a removal, the lead moved
  mid-check).
- **Tests** (no wall clock, no real NAS): crate::inputs's tests (verdicts as a function of candidate,
  previous version and acceptances; stable ids; held new/edited/removed files; paired files; the
  whole-version check; only changed files read; the same index twice), the agent's
  (`the_gate_plans_a_check_first_and_shows_in_the_status`), GC's
  (`the_inputs_copies_go_once_nothing_names_them`) and `crates/pipeline/tests/inputs_accessor.rs`.

## 5. Shapes every input shares

### 5.1 The description file

Every declare input whose data reaches the map, and every source declaration, carries a description
in TOML: `about.toml` for a single-dataset input (languages, descriptions), `<name>.toml` beside each
dataset (registers, static GTFS), or the declaration itself (feeds, elevation).

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `name` | string | no | the dataset's own name |
| `what` | string | no | what the map shows from it, the credit's first part |
| `credit` | string | no | the source as its terms ask it be named (required) |
| `licence` | string | no | an SPDX id where there is one, else the terms' name (required) |
| `licence_url`, `source` | URL | no | the terms; the page it comes from |
| `retrieved` | date | no | the owner's record |
| `territory` | ISO 3166 codes, or `"world"` | yes | where it applies, checked against the data where possible |
| `redistribute` | bool, default true | yes | false: never in the World download |
| `notes` | string | no | free text |

Parsed by `inputs::credits::parse_description`. Keyed fields enter the file's `keyed` digest
(§4.6); the others change what the map says, not its data.

### 5.2 Source declarations

A declaration adds, under `[data]`:

| Field | Meaning |
|---|---|
| `format` | the standard format: `gtfs`, `mdb-catalogue`, `geotiff`, `tiles`, `geoid-grid`, … one generic reader each |
| `url` / `urls` | where to fetch; may be a template (`{z}`, `{x}`, `{y}`) |
| `index` | a file beside the declaration listing files and their footprints (GeoJSON), made by a producer (§8) |
| `files` | data added beside the declaration, in `<name>/` |
| `version` | a pinned release where the source has releases |
| `[data.fetch]` | `method`: `get` (default), `get-key-header` (`header`, `key` naming a key in `inputs/keys.env`), `get-key-json-link` (also `link`, the JSON path of the download link) |

Fetch methods are protocols, generic code used by any declaration, through `buildstore::fetch`, which
asks before any internet request. A range read (a GeoTIFF's tiles) is streamed, and one that breaks
off goes on from the bytes received with a new range request (`fetch::net`: only a request that
moves nothing counts against the retries). A server that gives no answer after its retries is noted
for its host and port in the z6 tile job's cache (`fetch-down/<host>`, with `_<port>` when the URL names one; `SCENIC_FETCH_DOWN`): requests to that server
fail at once for the next 30 minutes instead of each waiting out its retries, and a step that ends so
exits 69, which the `unit` job reads as the z6 tile's server being down: it sets that z6 tile aside, builds
its other z6 tiles, and fails at its end naming the z6 tiles set aside, which the agent's retry builds later. The data servers a worker's task may read through the coordinator
(docs/coordinator.md §3) are derived from the accepted declarations (`coord::hosts`): the hosts of the
roads' elevation datasets' web files (their `urls`, their index's URLs, their tile template; not a
dataset kept in the store, nor the terrain's) and of the land-cover source's. A dataset kept in a store (`[data] store`, §6.8) is
filled once by its own tool and only read by builds. Fetched data is content-named
and pinned in the records by the job that fetched it; a refresh is a new pin. Planned: an `s3-list` method; `range` sources (cloud
GeoTIFFs read in place, never whole) pinned by the server's ETag (else Last-Modified and length),
checked with `If-Match` on every read so a file changed mid-build fails rather than mixing versions.

### 5.3 Formats

- **GeoJSON:** RFC 7946, WGS 84, a FeatureCollection per file. Winding isn't required. Invalid
  geometry is an error naming the feature, never repaired silently.
- **JSON lines** (`.jsonl`): one UTF-8 object a line, ending in a newline.
- **TOML** for small hand-written files, **TSV** for tables, **GTFS** as zips; dates ISO 8601.

## 6. Each input

### 6.1 Regions

**Kind:** declare. **Inputs folder:** `inputs/regions/`, read recursively. **Unit:** `regions`. Code:
crate::inputs::regions (`file`, `shapes`, `geom`, `resolve`, `water`, `data`, `stored`, `extract`,
`read`).

#### 6.1.1 What a region is

A name for a piece of the coverage. The coverage is the union of every region; nothing is built per
region (docs/regions.md). Without `_rules.toml`, regions may overlap freely and the check validates each
region's definition, not how regions fit together. With `cover = "earth"` in `_rules.toml` (§6.1.3, "Land and water"), the regions must cover
the Earth once: the check fills land gaps and settles land overlaps by the folder-wide rules, and
any water in two regions, or in none, is an error.

#### 6.1.2 Files and folders

- One region per file, `<id>.toml`, anywhere under `inputs/regions/`. Ids are unique across folders,
  so moving a file changes no id, coverage or key.
- Custom shapes are `<name>.geojson`, anywhere, named by file name alone (unique across folders).
- **Folders are the Regions panel's groups:** a collapsible tree nested as on disk, each group with
  its count, all collapsed on every load (opened ones stay open for the session only). A group's name is its folder's, or the `name` of an
  optional `_group.toml`, with an optional integer `order` among siblings (those with one first,
  lower first; then by name). The check reads it (`rg-group`); the index carries it (`facts.group`)
  and the tree is made from the index alone (`regions::read::tree`). Empty folders aren't shown.
  Moving a region between folders changes its group, never its geometry or keys (the index, and so
  the map, is new).
- **Special files** are the `.toml` files whose name starts with `_`: never a region (an id has no
  `_`, so a region file can't be named so). Three are known, and any other `_*.toml` is an error
  (`rg-special`) naming them; so are the old names `pins.toml` and `rules.toml`, which say to
  rename them. A `_` name that isn't a `.toml` is ignored (drafts).
  - `_group.toml`, in any folder: its group (above).
  - `_pins.toml` (at the top) names the OSM pass the folder is written against (§6.1.5).
  - `_rules.toml` (optional, at the top) holds the rules for the whole folder: `cover = "earth"` and
    `sliver_km2` (default 50), §6.1.3 "Land and water". Unknown fields are an error (`rg-rules`).
- Every other file (a `.poly`, `.txt`, `.toml.removed`) is ignored: neither checked, read nor copied.

#### 6.1.3 The region file

| Field | Type | |
|---|---|---|
| `id` | `[a-z0-9-]`, 1–64 characters, the file's name without `.toml` | required |
| `name` | string, non-empty | required |
| `outline` | list of parts, their union | required, non-empty, unless there's `water` |
| `minus` | list of parts, their union taken away | optional |
| `intersect` | list of parts, the region kept only inside their union | optional |
| `water` | list of parts, the region's water: their union (`water = []`: none but its lakes) | optional; makes it a region of land and water |
| `hand_out` | a folder's path: each stretch of this region's water only one region of that folder borders goes to that region | optional, with `water` |
| `hand_out_all` | `true`: with `hand_out`, every stretch goes — to the region of the folder sharing its longest edge, or the nearest region with land if it touches none (the list's rule for land inside a split place's border but outside all its regions); the region then ends empty and is not stored; anything no region of the folder can take is an error (`rg-hand-out`) | optional, with `hand_out` |
| `disabled` | `true`: checked and stored like any region, but never built (below) | optional, default `false` |
| `[lines.<name>]` | a line, from `coords` or `ways` | optional |
| `[pins]` | as `_pins.toml`, overriding it for this file | optional |

Unknown fields are an error. The region is ((∪ `outline`) ∩ (∪ `intersect`)) − (∪ `minus`); a region
with `water` is its land, that cut back to the coastline's land, and its water ("Land and water").

**Disabled regions.** `disabled = true` keeps a region in the list without building it. The check
treats it like any other region: it is parsed, resolved, held and stored the same way, other regions
may name it (`region:`, `land:`, `group:`, `rest`, nearest-coast splits, hand-outs), and with
`cover = "earth"` it counts toward covering the Earth once. Its stored geometry is the same either
way. The index records it (`facts.disabled`, set only when true). The readers leave it out
(crate::inputs::regions::read: `recipes`, `enabled`, `accepted_coverage`), so it is in no coverage,
unit, map tile, catalogue or download, and it adds nothing to any forecast or size. The regions'
extract (§6.1.6) still fetches the relations and ways a disabled region names, because the check
needs them. Enabling or disabling a region changes only the index, which makes a new version. The
coverage changes over the region's area alone, so only jobs whose box it reaches run again. The panel
shows it greyed out with a "disabled" state and no Download button. `scenic-build regions-check`
reports it (`disabled`, per region and in all, and `enabled_km2`).

**Parts:**
- `"osm:<relation>"`: an OSM relation's area, from the pinned pass's outline set (administrative and
  ISO 3166 relations, docs/regions.md) or, for any other relation, the regions' extract (§6.1.6).
- `"shape:<name>.geojson"`: a Polygon or MultiPolygon (a Feature, geometry or FeatureCollection,
  unioned), exact.
- `"region:<id>"`: another region's stored geometry (in `outline`, `minus` and `intersect`, a
  region with water's land; in `water`, all of it).
- `"land:<id>"`: another region's land (its outline as resolved; a region with water may name its
  own in its `water`).
- `"group:<folder>"`: every region under that folder (`group:europe/portugal`), the region naming it
  left out.
- `"osm-way:<id>"`: a closed OSM way's area, from the regions' extract (§6.1.6).
- `"eez:<mrgid>"`, `"ts12:<mrgid>"`, `"ne-lake:<ne_id>"`: a 200-nautical-mile zone of Marine
  Regions' EEZ v12 with the internal waters Internal Waters v4 draws for it, a territorial sea of
  Territorial Seas v4, a lake of Natural Earth's 1:10m lakes (§6.1.7).
- `"box:<west>,<south>,<east>,<north>"`: degrees of longitude and latitude (west under east, so a
  box across ±180° is two); a slice of latitude is a box from −180 to 180.
- `"earth"`: the whole Earth. `"rest"`: only in `water`, and in one region: what no other region
  covers (the high seas).
- `{ buffer = <part or "line:<name>">, by = "<distance>" }`: everything within that ground distance
  (`m`, `km`, `nmi`; positive, at most 200 nmi).
- `{ union = [<parts>] }`, `{ minus = [<part>, <parts>] }` (the first less the others),
  `{ intersect = [<parts>] }`.
- `{ nearest = <part>, to = [<parts>], than = [<parts>], step = "<distance>" }`: the points of the
  part nearer the `to` parts than the `than` parts (each the nearest of their edges' points), decided
  on a grid of `step` (default 1 km; 100 m to 20 km): the split follows the median line within a
  cell. A split by nearest coast names every competing coast in `than`; the complementary split
  (`to` and `than` swapped) is exactly the rest (ties go the same way in both) when each coast is passed as its own part, one per region (`land:a`, `land:b`, not `{ union = ["land:a", "land:b"] }`): a point two coasts share goes to the coast that orders first (by box, point count, point sums, then rings), so the order is total only between coasts given separately.
- `{ cut = <part>, by = "line:<name>", keep = "left" | "right" }`: the part on one side of a line as
  drawn. Each piece the line crosses edge to edge is split; a piece it doesn't meet is kept if it
  lies on the `keep` side of the line extended straight from its ends. A line ending inside a piece
  is an error.

The operations nest.

**The coast tolerance.** `osm:` parts listed directly in `outline` of a region with no `minus` or
`intersect` are stored as the relation's rings with a 1 km tolerance (a vertex within 1 km counts as
inside: `OSM_BUFFER_M`), which takes in coasts drawn on the low-water side. Elsewhere an `osm:` part
is the relation's exact area; a region with `minus` or `intersect` grows its direct `osm:` outline
parts by a real 1 km buffer first, so subtracting a neighbour removes exactly its area.

**Lines**, used only to cut or buffer: `coords = [[<lon>, <lat>], …]` (at least two distinct
points), or `ways = [<way id>, …]` from the pinned pass (via the extract), strung end to end in
order, each reversed as needed; ways that don't share an end node are an error. A cut line may not
cross itself.

**Land and water.** A region with a `water` key is resolved in two steps, cell by cell (crate::inputs::
regions::tiles; §6.1.4). First its land: its outline exact (no coast tolerance), cut back to the coastline's land
(the pass's, §6.1.7) less the lakes over 100 km² (`regions-lakes`); those lakes inside its
outline are its water, unless another region's water names them. Then its water: the `water` parts,
less the coastline's land and every region without `water` (the land dominates the sea), in an
order where a region comes after those its water names whole (`region:`, `group:`). Then one
coastline, with or without `cover`: the outlines' coastal edges are taken from the coastline. What
lies in no region along the coastline, on either side of it — land where an outline's coast lay
inland of the coastline's, sea where the water's parts stopped short of it (the coastline: the
land's edge, the big lakes' shores included, never a cell's edge) — is joined across the cells'
edges into stretches; a stretch whose every point (its vertices and a point inside each piece) is
within `COAST_REACH_M` (1 km) of a region's land (a stretch of land) or water and lakes (a stretch
of sea) goes to the nearest such region, split at the median line where several are nearest (a
piece under 0.01 km², `SPECK_KM2`, whole to the region nearest a point inside it), each part to its
region if that has `water` (a region without `water` keeps its outline: its part stays
out). So a region's land and water meet on the coastline's coast (a `land:` part names the land
before this); a stretch reaching past 1 km (an island no outline has, a long inlet the zones stop
short of) stays out, for the cover check. A region's
`hand_out` folder then takes each stretch (a connected piece) of its water that one of its regions
alone borders, and a stretch under `sliver_km2` goes to the one sharing its longest edge. With
`cover = "earth"` in `_rules.toml`, then: a region's piece of water under `sliver_km2` touching
none of its land joins the region with water it shares the longest edge with; and, cell by cell
(2°), land in two regions is taken from those of them with water (a pair's over `LAND_OVERLAP_KM2`,
1 km² summed over the cells, a warning, `overlap-settled`: two outlines disagree, a disputed border
like Croatia's and Serbia's on the Danube). Then each piece still in no
region (its cells' pieces joined across the cells' edges, as the stretches are) within 100 m of land
up to `sliver_km2` joins the region with water it shares the longest edge with, as does one
touching none (a hole in the zones' water off any coast, where their source has a rock OSM hasn't).
A larger stretch is some country's where it is: the whole of one touching land (a lagoon, an
estuary, a creek the zones stop short of: the Ringkøbing Fjord, the Indus delta) or with a point
inside one of its pieces within `NEAR_LAND_M` (100 km) of land (a hole in the zones' water by a
coast: the Indus delta's tidal flats, New Caledonia's lagoon; the open sea beyond every zone lies
370 km or more from its coasts, so the Gulf of Mexico's and the Sea of Okhotsk's enclaves stay the
high seas), unless it reaches the open sea (a cell with no land in it, the rows above and below
or as many columns either side as make 111 km at its poleward edge, one up to 60°, all at the
poles' rows: at least 110 km from any coast, past a coast no zone has, as Antarctica's),
and each piece's part inside a 200-nmi zone (any of Marine Regions' zones with its internal
waters). That goes to the regions with water the stretch shares an edge with: to the one, or split
among several by nearest edge (`geom::nearest`, the median line; a piece with none of them in or
next to its cell to the one of them sharing the longest edge with the stretch). Within one country (the regions'
top folders, or a file at the top: a province and its country's waters are one), its shore's: the
regions with land it borders (its provinces, by their land or their own coastal water) take what
lies outside the zones, the one whole, several split among themselves alone, and the country's
regions of water alone (its waters) only the part inside a zone, so a fjord isn't cut by a median with the country's waters
(Avannaata's, Sakha's, Nunavut's). Across countries, all of them share it by nearest edge, a
warning (`unnamed`, naming them and their shares) when it's over `sliver_km2`: a place no region
names, likely disputed, whose owner a region's water should name. What's left of it, the open sea beyond every zone, stays. Land in no region
over `sliver_km2` (a stretch joined across the cells' edges) is an error (`rg-cover`), found at a
point inside its largest part of land and naming the regions with water that had any of it both
(taken from both); where most of such a stretch is land two regions with water both had, the error
is that land in two regions (`overlap`), at a point inside it, naming them. The region whose water is
`rest` takes what's left, the high seas; without one, water left is an error, as is water in two
regions. The rest's water inside a zone over `HAIR_KM2` (0.1 km²) in a cell is an error
(`rest-in-zone`: no region with water borders it). Then the regions as they end are measured cell by
cell (`tiles::Fit`): two regions both covering more than `HAIR_KM2` (summed over the cells), or a
cell's area in no region, the rest included, over it, is an error (`overlap-after`, `gap-after`):
one partition: every border two regions share, land or water, is one edge of both. A region
without `water` resolves as before, and no new rule changes it: a hand-out gives only to regions
with water (naming one without is `rg-ref`), and one without `water` may not name one with it
(`rg-ref`). So old and new files sit together, and `cover = "earth"` only holds once every
file is new. The 12-nautical-mile line is the `regions-territorial-seas` source,
named as `ts12:` parts; it divides nothing (§6.1.9).

**Examples:**
```toml
# europe/spain-mainland.toml: a country minus its islands
id = "spain-mainland"
name = "Mainland Spain"
outline = ["osm:1311341"]
minus = ["osm:349000", "region:canaries"]
```
```toml
# europe/spain-north.toml: north of a hand-drawn line
id = "spain-north"
name = "Northern Spain"
outline = [{ cut = "region:spain-mainland", by = "line:split", keep = "left" }]
[lines.split]
coords = [[-9.8, 42.0], [3.6, 41.6]]   # west to east: "left" is north
```
```toml
# europe/portugal/azores.toml: 12 nmi around an island group
id = "azores"
name = "Azores"
outline = [{ buffer = "osm:1629145", by = "12 nmi" }]
```
```toml
# cyprus.toml: the island's zone by nearest coast, the bases' 3 nmi their own
id = "cyprus"
name = "Cyprus"
outline = ["osm:307787"]
water = [{ minus = [{ nearest = "eez:8376", to = ["land:cyprus", "land:akrotiri-and-dhekelia"], than = ["land:northern-cyprus"] }, "region:akrotiri-and-dhekelia"] }]
```
```toml
# canada/canadian-waters.toml: the rest of a split place's zone
id = "canadian-waters"
name = "Canadian waters"
water = [{ minus = ["eez:8493", "group:canada"] }]
hand_out = "canada"
```
```toml
# high-seas.toml
id = "high-seas"
name = "High seas"
water = ["rest"]
```
`_pins.toml`: `osm = "2026-09-29"` (the pass, `sources/osm/<date>/`). A file's `[pins]` overrides it
and may record other datasets a shape came from (`gadm = "4.1"`, checked only as a non-empty string).

#### 6.1.4 Geometry: how it's computed and what's stored

- **Coordinates** are E7 integers (`[i32; 2]`, as `Shape::rings`); GeoJSON is rounded to E7 on read.
- **Set operations** use `i_overlay`'s 64-bit integer engine directly: exact predicates, no slivers
  on shared borders, the same output on every machine (single-threaded). Results are canonical:
  outer rings counter-clockwise, holes clockwise, each ring starting at its smallest vertex,
  repeated and collinear vertices removed, rings sorted, so equal geometry is equal bytes.
- **Buffers** are computed on the ground: parts densified to 10 km edges, each edge offset in a
  local equirectangular projection at its mid-latitude (under 0.1 % error at 200 km), round joins
  within 5 m of the arc, pieces unioned. Past 200 km the rings are first simplified within a
  thousandth of the radius and the joins drawn within a thousandth of it too. **Cuts** intersect the
  shape with each side's polygon. **Sizes** (for slivers, shared edges and the cover check) are on
  the Earth's mean sphere (`roadcore::geo::EARTH_R`); a shared edge's length is the other area within 100 m of the edge, over 100 m,
  measured in quarters while either area is large.
- **Cell by cell** (crate::inputs::regions::tiles): a region with water is resolved on a grid of 2°
  cells. Each part it names is cut into the cells by clipping its rings to boxes (no polygon
  operation on the whole); each expression is evaluated per cell, a buffer from the cells within
  its distance, a nearest-coast split against the coasts in the cells within 450 km, the reach doubled while a point's nearest coast lies past it (so no coast, however far, is passed over); the land, water
  and lakes are kept by cell, stretches (`hand_out`, slivers) joined across the cells' shared
  edges, and the cover check runs per cell, its gaps then joined across the cells' edges. So an operation holds a cell's geometry (a buffer's or
  a split's, its neighbourhood's), never a region's; what's kept is every region's cells (the
  outcome itself) and the datasets, read only once a region in the folder names them (the
  coastline's land by cell where asked, §6.1.7).
  A zone's internal waters are unioned with it. The datasets' rings are simplified within 20 m as
  read, the points two rings of the zones share kept (neighbours' borders the same on both sides). The regions with water resolve one at a time, their cells several at once (regions without
  water: as many at once as the check is allowed threads, §4.3). A region's parts naming other regions (`region:`, `land:`,
  `group:`) are read cell by cell where those are kept, never copied. With
  `SCENIC_REGIONS_TRACE` set, the check prints each step's and each region's water's footprint
  and its buffers' and nearest-coast splits' count, time and footprint, and the coast's stretches
  over 1 km² left out of reach; `SCENIC_REGIONS_GAPS` (cells' south-west corners in degrees,
  `-130,54;-64,44`) prints each such cell's land in no region (pieces, median, largest, total)
  before the coast step and at the end; `SCENIC_REGIONS_THREADS`
  sets how wide the pools are (the cores, 8 at most). A buffer of a cell reads its neighbours' pieces each buffered
  once, within 16 columns each way (25 in the north, enough for 200 nmi at 86°N) and not from the cells south of 80°S (Antarctica's inland ice) or
  north of 86°N (where no land is); a buffer of
  50 km or more is drawn as discs at the pieces' points a thirty-second of the distance apart
  (within a twentieth of the distance).
- **Progress:** a resolve counts its steps as it goes (`regions::progress`, task #1145): the regions
  without water, the land and the water (by region, the one under way by its cells), the lakes, the
  coast's cells, splits and fills, each region's area, the hand-outs and the slivers (by region, the
  one under way by its stretches), the 200-nmi zones, the cover check's every cell, the gaps' land and
  shares, the fills, the rest inside the zones, the fit's every cell, the regions stored. Each step
  is an equal share of the resolve (a step the folder doesn't take passed over), its items done of
  its total within it; every 5 s (`jobrt::jobs::saying`) the share goes out as the check's stage
  under way, what's under way in its line (`1.642/6 inputs check (the candidate checked whole: the
  cover check, 5200 of 16200 cells)`), so the slow and stuck flags judge it while it works. The share
  never goes back. (Planned, #1167: the steps are equal shares whatever they take, so the slow
  flag's rate in a long one, the cover check, is under the check's average.)
- **Snapping and the operations' watch:** the regions with water's operations snap with a growing
  radius (i_overlay's `Precision::HIGH`, from a unit; the others' stay at a unit, so they're byte
  for byte as before): with a fixed unit radius, cells whose edges ran nearly along each other
  (Australian and Indonesian waters) split over and over for half an hour; snapping, those
  subsets resolve in 11 s and 38 s. Every operation is watched: one past 30 s is said on stderr
  (the region, the cell, its points), one past `SCENIC_REGIONS_OP_BUDGET_S` (300 s by default;
  none measured over 30 s since) stops the check, naming the cell.
- **The memory guard:** the check stops its process (exit 3, saying so) if its physical footprint
  (`sys::footprint`: compressed and swapped memory included) passes `SCENIC_REGIONS_MAX_MB` (half the
  Mac's memory by default), looked at every 50 ms; `scenic-build regions-check --max-mb` sets it for a scratch
  check, and reports the most points one operation held.

**Stored per region** (beside the region files in the accepted version):
- `sources/inputs/regions/@geometry/<id>.<hash16>.rgeo`, what the coverage reads: uncompressed,
  magic `RGEO0001`, a JSON head, then each shape's rings as zigzag varint deltas of E7 points. Head:
  ```json
  {"fmt": 1, "id": "azores", "osm": "2026-09-29",
   "shapes": [{"from": "{buffer osm:1629145 12 nmi}", "country": "PT", "tolerance_m": 0, "rings": 3, "points": 2210}],
   "box": [-317000000, 366000000, -246000000, 401000000], "names": {"osm": [1629145], "ways": []}}
  ```
  No name, so renaming changes no key. `tolerance_m` is 1,000 for an `osm:` part with the coast
  tolerance, else 0; `country` is the relation's ISO code (a computed shape's first `osm:` part's).
- `sources/inputs/regions/@drawn/<id>.<hash16>.json`, the small outlines the map's catalogue
  records (its `coverage`: what a download reaches, the credits and the names' checks read; the map
  draws the region layer from the full geometry instead, docs/steps.md §15): each shape simplified
  for drawing (an `osm:` part as the pass simplified it, else by
  `draw_tolerance_m`), degrees to 5 decimals, keyed by `from`. A region resolved by cell
  (§6.1.3) is drawn with its cells' pieces dissolved into one area first, so no 2° cell edge is
  drawn inside it.

A region of only `osm:` and `shape:` parts in `outline` with no operation is stored as **one shape
per part, unchanged**; any operation gives one shape (two across the antimeridian); a region with
`water` gives two shapes, its land (the coastline's land, less the lakes over 100 km², of what it
ends with after hand-outs, slivers and fills) and its water (`"water": true` on the shape in the
head; `from` ends " (water)"), either left out when empty, their rings on both sides of ±180° where
they cross, and drawn as one. The coverage holds the water but makes only the depth's targets of it,
and a region of water alone is the depth's alone (docs/regions.md). Files from before #468 carry
`"water_only": true` in the head instead (every shape water): read, never written. The index names
each region's files in `facts.geometry` and `facts.drawn`, and its `keyed` digest is the geometry
file's content name, so an edit that doesn't change the result reruns nothing. A region that
resolved to nothing (a `hand_out_all` region, every stretch handed out) is stored with no files: its
facts name neither, and the readers list it (the Regions list shows it) but leave it out of the
regions built, the coverage and the drawing (no area to cover).

**Reading:** `accepted_coverage(root, accepted, cache)` (crate::inputs::regions::read) builds the
coverage from the stored shapes, regions in id order, shapes in stored order. The agent passes its
cache folder: the coverage is kept there by the accepted version and the file's format
(`coverage/<hash16>.rdcov007.cov`, `buildstore::coverage::kept_name`: an older app's file under its
own name, so an update's agent and the jobs an older one left never read each other's as
unreadable and make theirs in memory; the shapes with their grids and edge trees, tasks #807 and
#1109, so a read makes nothing again), so each Mac reads a version's geometry files off the NAS and grids them once, a
restart none; that first read runs on a thread of its own, the plan's region jobs waiting for it.
The file is read in place (`store::cachefile::map`, `buildstore::coverage::from_kept`, task #1109):
each array at a multiple of 8 bytes into it, the shapes' rings, grids and edge trees are views of
the mapped file, its structure checked as it's read (rings closed, every cell's and run's edges
there), so they are the file cache's pages, not the process's memory: no footprint, one copy for
the agent and every job on the Mac, dropped by macOS under pressure and read again as asked (the
world's 1,605 shapes, read into the process's own memory, were 1.68 GB of its footprint on the
plan bench of 11 Nov, 4 MB read in place; the file 1.62 GB with the edge trees, 1.41 without). A version
just made is read back from its file the same way, the one made let go. The answers are those of
the shapes made from the rings, bit for bit (the same values). `scenic-build`'s steps read the
same file, the agent naming the cache folder to its jobs (`SCENIC_CACHE`), a folder of the Mac's
own disk (a mapped NAS file faults with SIGBUS when the share drops: a cache on a network
filesystem isn't mapped, the coverage made in memory as with no cache). Each read in place there
(made or found) sweeps the folder: every other file no process holds is deleted
(`store::cachefile::try_remove`; task #1150), so another version's or an older format's file goes
once the last process mapping it lets go (mapping it holds it, its shared lock, while mapped: a job
of an older app or version keeps its file until it's done), as does a `<name>.making` lock whose
maker is gone and a temporary file whose writer died; a newer format's files are left
(`buildstore::coverage::kept_newer`: a newer app's, which the jobs an older app leaves running
after an update never take from it); room-making never empties the folder
(docs/macs.md). A version not kept yet is made by one process of the Mac at a time
(`store::cachefile::making`, `<name>.making` locked, taken again if a sweep deleted it meanwhile),
the others waiting, then reading it in place: since a job's memory
bound counts the coverage read in place, each making its own at once (a new version's first jobs
on a member) would have held the whole coverage beside the others', none of it booked (task
#1109's review). The one making it still holds it whole while it does (~1.7 GB for the world's,
unbooked), once a version and Mac; a process that can't write the file (its disk full) or map it
keeps the one it made, whole and unbooked, for its run (the memory guard stands behind it). Nothing rebuilds the coverage
from outlines, GeoJSON or region files. `scenic-build`'s `--regions <dir>` (a scratch folder, for
trying regions) runs the same check in memory and stores nothing. Tests build coverages with
`Coverage::fixture` and put accepted fixtures in their records (`inputs::regions::accept_fixture`).

#### 6.1.5 Pins

- A region is resolved only against the pass its pin names, and only if that's the build's current
  pass. The coverage changes only when a region file changes and passes, never because a pass came
  in.
- A pin naming another pass is an error (`rg-pin-mismatch`): the files are held and the last
  geometry kept. The finding lists, for each relation named, whether the new pass has it and whether
  its rings changed ("pinned to OSM 2026-09-29; the build's pass is 2026-12-28. Changed:
  osm:1629145. Unchanged: 2. Missing: none."). With one folder-wide pin, a new pass holds every
  region until `_pins.toml` is moved; the banner leads with that.
- While the current pass has no outline set yet, the check does nothing (the outline set's name is
  in its key).

#### 6.1.6 The extract

Relations outside the outline set, and every way a line names, come from **the regions' extract**:
`sources/osm/<date>/regions-extract.<hash16>.json`, `{"fmt": 1, "pass", "relations": {"<id>":
{"rings", "tags"} | null}, "ways": {"<id>": {"ends", "line"} | null}}`, `null` for an id the pass
lacks.
- A check needing an id the extract lacks holds the files (`rg-extract`) and lists what it needs in
  its held report (`needs`, the candidate's: step 5's check of the version without the held files
  doesn't replace them; a region with water's as a region without water's; a disabled region's
  too, since it is checked like the others); the lead plans `regions-extract <date>`, keyed by the pass and the
  sorted id set, named in the records as `sources/osm/<date>/regions-extract`. Its content name is
  in the check's key, so the check runs again once it's made. The extract never shrinks within a
  pass.
- It reads the pass's filtered planet, then the full planet only for ids the filtered one lacks,
  with `osmpbf` in three scans (relations, ways, nodes); it runs alone on its Mac.

#### 6.1.7 The datasets

`eez:`, `ts12:` and `ne-lake:` parts, and a region with `water`, read the regions' datasets
(crate::inputs::regions::data): declared sources (§6.9) of the roles `regions-sea` (Marine Regions'
EEZ v12, GeoJSON, by `mrgid`), `regions-internal-waters` (Internal Waters v4, by `mrgid_eez`, added
to its zone), `regions-territorial-seas` (Territorial Seas v4), `regions-lakes` (Natural Earth 1:10m
lakes 5.0.0, a shapefile zip, by `ne_id`), each a file in `sources/fetched/<id>/` named by its
`params.path`; and the coastline's land, the pinned pass's own. Their rings are simplified within
20 m as they're read (`data::SIMPLIFY_M`); the zones' (sea, internal waters, territorial seas) keep
every point two of their rings share, so a border two zones share stays the same on both sides. The
check reads the sources whole, and only once a region in the folder names them (or has `water`),
when the accepted sources declare `regions-sea`; until then such a region is held (`rg-data`).
Features of one id are unioned as they're read. Their declarations are staged in `producers/sources/`
(`marineregions-eez.toml`, `marineregions-internal-waters.toml`, `marineregions-territorial-seas.toml`,
`ne-lakes.toml`); Marine Regions' three are CC BY 4.0 and credited as their declarations say.

**The coastline's land** (crate::inputs::regions::land) is the pass's: the `regions-land` step
(lead, once per pass, alone, 8 GB) reads the pass's `coastline` set (`w/natural=coastline`, made by
pass-sets), joins the ways end to start into rings (land on their left, as OSM draws them),
turns round a chain drawn the wrong way (two chains start at its start and two end at its end; the
shortest first: the 2026-09-28 coastline had four at the Outer Banks), links the chains that don't
close end to start, each to the chain starting at its last node, else to the nearest chain start
within 10 km (the nearest pairs first; the 2026-09-28 coastline was broken by 8.0 and 5.5 km at the
Outer Banks, cutting the Americas' ring, and by 1.1 km in Scotland; the widest 20 gaps are in the
file's head), leaves out
what never comes round (counted, the largest named in the file's head), closes a ring round a pole
along its latitude, simplifies the rings within 20 m, adds the Earth south of 85°S (OSM closes
Antarctica along 85.0511°S) and keeps the land on disk by 2° cell: `sources/osm/<date>/regions-land.<hash16>.bin` (magic
`RLAND001`, a JSON head with its counts and each cell's offset and length, then each cell's rings
as zigzag varint deltas), keyed by the set and `land::LAND_V`. The check reads its head, then each
cell where it's asked for, by its offset (`Land::cell`; the last 256 cells kept decoded), never the
whole; a cell it can't read fails the check (an error, not findings, never a panic); its content name is in the check's context (`regions land`), so a new land re-checks the
regions. Without it a region with water is held (`rg-data`). `scenic-build regions-land --set <pbf>
--to <file>` makes it from a local coastline set into a local file.

`scenic-build regions-check --dir <folder> --declarations <folder> [--land <file>] [--geometry <dir>]` runs the check on a scratch folder in memory with the datasets those declarations name
(`--geometry`: each region's stored geometry written there as `<id>.rgeo`, to look at),
and reports each region's size (a region with water's land and water each read cell by cell and
summed, never one overlay of the whole: the high seas' would be ~200,000 points), and, with
`cover`, how they fit (`fit`, from `tiles::Fit`): each pair's land and water both had before the
cover check settled them, each stretch in no region (its area, its share inside a zone, its land,
whether it touches land, a point in it, the regions it borders and where it went), and after: each
pair's overlap, each cell's gap and the rest's water inside a zone.

#### 6.1.8 Checks

All errors but `rg-cover`'s `unnamed` and `overlap-settled`, warnings; ids `rg-<check>.<hash16>`.
Those two warnings' ids are their regions' (and `unnamed`'s place, to 0.1°), not their sizes, so a
new pass moving a coast a little doesn't hold an accepted one's files again.

| Check | Raised when |
|---|---|
| `rg-toml` | the TOML doesn't parse; a field unknown or mistyped |
| `rg-id` | `id` invalid or not the file's name; `name` or `outline` empty |
| `rg-part` | an unknown kind of part, a malformed `osm:` id, a bad distance, a bad `keep` |
| `rg-dup-id` | two files with one id, or two GeoJSON files with one name |
| `rg-osm-missing` | a relation or way the pinned pass doesn't have |
| `rg-osm-no-area` | a relation whose rings don't close |
| `rg-shape` | a GeoJSON missing, not a (Multi)Polygon, or invalid |
| `rg-line` | a line under two points, ways that don't join, a cut line crossing itself |
| `rg-cut` | a cut line ending inside a piece |
| `rg-ref` | a `region:`, `land:` or `line:` naming nothing (in `water` too) |
| `rg-loop` | a loop of `region:` references |
| `rg-empty` | a result with no area |
| `rg-pin` | no OSM pin, or one that isn't a date |
| `rg-pin-mismatch` | a pin naming another pass than the current one (§6.1.5) |
| `rg-special` | a `_*.toml` that isn't `_group.toml`, `_pins.toml` or `_rules.toml` (the last two at the top), or the old `pins.toml` or `rules.toml` |
| `rg-group` | a `_group.toml` that doesn't parse, has a field other than `name`/`order`, or an empty name |
| `rg-extract` | ids the extract lacks: held while it's made (§6.1.6) |
| `rg-data` | a region naming the datasets, or with `water`, while they aren't read (§6.1.7) |
| `rg-data-missing` | an `eez:`, `ts12:` or `ne-lake:` id the dataset hasn't |
| `rg-hand-out` | a `hand_out_all` region left with land or water no region of its folder could take |
| `rg-rest` | `rest` in more than one region's water |
| `rg-rules` | a `_rules.toml` that doesn't parse or has an unknown field or value |
| `rg-cover` | with `cover = "earth"`: water in two regions, or in none without a `rest` region; land in no region, or mostly in two regions with water, over `sliver_km2`; the rest's water inside a zone (`rest-in-zone`), two regions overlapping or area in none after the check (`overlap-after`, `gap-after`), each over `HAIR_KM2` (0.1 km²); a warning: a stretch in no region's parts over `sliver_km2` split among several countries' regions by nearest edge (`unnamed`), land two regions both had over `LAND_OVERLAP_KM2` (`overlap-settled`) |

A held region keeps its last geometry; a new held region isn't in the coverage yet. Deleting a
region file takes it out with the next version.

#### 6.1.9 What the owner sees

- **The Regions panel** (Settings → Regions), read only: the folder tree, each region with its parts
  and state (accepted, checking, "held: <finding>"; a disabled region greyed out with a "disabled" pill and no Download button). The server reads region files through the NAS
  I/O pool each minute, only those changed (all of them hourly), and keeps the listing with their states from the
  accepted index and held report (`GET /api/regions` answers from it at once). Nothing in the
  app writes a region.
- **Show regions on map** (a switch in Settings → Regions): every accepted region's borders as
  lines, from the region layer's tiles (docs/steps.md §15: the full geometry, each border two
  regions share simplified once per zoom, so neighbours' lines are one line), green (grey where
  both sides are disabled regions or none: a disabled region is checked even though it isn't
  built), with each region's name as a country's label at the point the server works out inside its
  largest land polygon (a region of water, or of waters with land under a thousandth of it: its largest water polygon). Only borders are drawn: never a cut along the antimeridian, a tile's edge or the
  Mercator tiles' ±85.05° (the polar caps draw no region lines); the lines' width and opacity are
  the boundaries'. A map from before the layer draws the names alone. The footer tells the region
  under the cursor (`/api/regions/at`, from the same tiles; the smallest by its box where regions
  overlap): its folders and name with their flags, its state, land and water and download.
- **The banner and menu bar** (§4.7).

#### 6.1.10 The switch

Every reader reads `accepted_coverage`, and only once the records hold
`sources/inputs/regions/@switched` (crate::inputs::regions::read `switched`); without it the readers
refuse, saying to do the migration's runbook first. The switch records the app it was made on, and
no Mac on an older app builds. The map records its regions' shapes inline
(formats.md, The catalogue `coverage`), so a map made before still draws. The tests (crate::inputs::regions's)
cover the operations on small fixtures: union, minus, intersect, cuts (past the shape, ending
inside, pieces missed), a 12 nmi buffer against a geodesic distance, ways that don't join, a loop,
duplicate ids, a pin mismatch, the antimeridian, byte-identical output twice.

### 6.2 Languages by territory

**Kind:** declare. **Inputs folder:** `inputs/languages/`. Code: crate::inputs::languages.

**Shape:** `territories.tsv`, one line `<code>\t<language>,<language>…`, most spoken first: an ISO
3166-1 alpha-2 code (`FR`) or 3166-2 subdivision (`CA-QC`), and ISO 639 base subtags (`fr`, `yue`).
A subdivision refines its country (the smallest outline wins); `#` lines are comments. Plus
`about.toml` (§5.1). Made by `producers/languages/cldr-languages.py` from CLDR's
`territoryInfo.json` (or `--base` an existing table), with its `REFINED` subdivisions folded in.

```
CA	en,fr
CA-QC	fr,en
HK	yue,en
```

The one rule kept in code, Chinese read as Cantonese where Cantonese is spoken and Mandarin isn't
(`READ_AS`, crates/names/src/spoken.rs), is a lookup rule, not territory data.

**Checks.** Errors: an unexpected file; a line not two tab-separated fields; a code not ISO 3166
syntax; a language not a BCP 47 primary subtag; a code twice; an empty list; a version without
`territories.tsv` or `about.toml`. Warnings: a code the newest pass's outlines lack
(`outlines::spoken_codes`, in the key); a subdivision whose country has no line. `keyed` is of the
lines as read, so comments rerun nothing.

**Ingesting.** The `spoken` job (docs/names.md) keys on the outlines, `spoken::RULES` and the accepted
index (`--languages-index`), and waits while there's none. `global/spoken` carries the table, which
`names-todo` reads; the server reads the accepted table (`accepted/languages/` in its home) only for
a map without `global/spoken`.

### 6.3 Translations

**Kind:** request–fulfil. **Inputs folder:** `inputs/translations/`. Code: crate::inputs::translations,
crate::inputs::lint; the server's crate::accepted; `names::Names::set_files`.

**Shape** (docs/names.md, Files): `**/*.jsonl` outside `todo/` and `how/`, one translation a line, read by
`names::line_shape` (the check and the server share it):

| Field | Type | |
|---|---|---|
| `n` | the name exactly as in OSM | required |
| `kind` | `road`, `settlement`, `other`, or a list | required |
| `langs` | an ISO 639 subtag, or a list | required |
| `main` | string or null (null or empty: the name itself) | required |
| `sub` | string or null | optional |
| `via` | how it was made; `todo` and `skipped` mark lines not done | optional |

```json
{"n": "Lac Bleu", "kind": "other", "langs": ["fr"], "main": "Lac Bleu", "sub": "Blue Lake", "via": "agent:haiku"}
```

Later files (by path) and later lines win for a name, kind and language.

**Checks.** Errors: a line that isn't JSON or lacks a required field; an invalid `kind` or language;
a file other than `.jsonl` (notes go in `how/`). Warnings (`tr-check`): `tools/names/check.py`'s
per-line `problem` rules and `drift`, one finding per file listing the lines, its id naming the
flagged lines' content (a drifted stretch is check.py's 100-line window). check.py runs as is, in
`--flagged` mode, through `uv run` in `dem/` (its pinned Python, so verdicts match on every Mac);
its text and Python version are in the key. If check.py fails to run, the job fails and is retried;
a file is never taken unchecked. Only changed files are read.

**Ingesting.** Nothing is rebuilt: the server loads the accepted files. A language's version is a
hash of the content names of the accepted files holding lines in it, so tiles' ETags change only with
content. `names-todo` keys on the accepted version.

### 6.4 Descriptions

**Kind:** request–fulfil. **Inputs folder:** `inputs/descriptions/`. Code: crate::inputs::descriptions.

**Shape** (docs/names.md, Descriptions): `**/*.jsonl` lines, and an optional `about.toml` (its credit;
without one the map keeps `rules::CREDITS`' "Heritage descriptions"; its extent is the world
when its `territory` is `"world"`, else unknown):

| Field | Type | |
|---|---|---|
| `qid` | `Q` and digits, no leading zero | one of `qid` and `id` |
| `id` | `n<digits>`, `w<digits>` or `r<digits>` | one of `qid` and `id` |
| `long` | string | required unless `drop` |
| `src` | string, or a list of `{"t", "u"}`: what it was written from, credited | optional |
| `drop` | true: removes the thing's description | optional |

**Checks.** Errors: neither or both of `qid` and `id`; either malformed; `long` empty without `drop`.
Warnings (one per file): `long` over the brief's 55 words; no `src`.

**Ingesting.** As translations: served with popups, nothing rebuilt; `names-todo` reads the accepted
version from the agent.

### 6.5 Heritage registers

**Kind:** declare. **Inputs folder:** `inputs/heritage/`, a pair per register: `<register>.geojson` and
`<register>.toml`, the id the file's stem (`fr-merimee`, `unesco-whc`; lower-case letters, digits,
`-`). Code: crate::inputs::heritage; `producers/heritage/` makes them; dem/heritage.py reads any
register.

**`<register>.toml`:** a description (§5.1) plus:

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `class` | `heritage`, `world-heritage`, `special` | yes | heritage sites and areas; UNESCO World Heritage; special places (biosphere reserves, geoparks, dark-sky places) |
| `territory` | ISO codes or `"world"` | yes | required; entries over 20 km outside are a warning |
| `wikidata_property` | `P<digits>` | yes | the property holding this register's ids, to match entries to Wikidata |
| `osm_ref_tags` | list of strings | yes | the OSM tags holding its ids, to link through OSM |

**`<register>.geojson`:** a FeatureCollection, any geometry, with properties:

| Property | Type | | Meaning |
|---|---|---|---|
| `id` | string | required, unique | the register's own reference, or `h` + 16 hex made by the producer from name, designation and place (with `ref: ""`); two entries sharing a reference take `~2`, `~3` |
| `ref` | string | optional | the reference when it isn't the `id`; `""`: none |
| `name` | string | optional (a warning without) | as the register gives it |
| `name_en` | string | optional | the register's own English (docs/names.md) |
| `level` | 1–5 | required for `heritage`, `world-heritage` | 1 World Heritage, 2 top national grade, 3 other national, 4 regional, 5 local |
| `designation` | string | required for `heritage` | the register's designation |
| `tier` | string | optional | the kind of designation the Heritage toggles group by (web/src/basemap.ts HERITAGE_TIERS); else by level |
| `kind` | string | required for `special` | `biosphere`, `geopark`, `dark_sky` |
| `site`, `site_area_km2` | string, number | `world-heritage` | the World Heritage site id, its inscribed area |
| `wikidata` | `Q<digits>` | optional | its item |
| `date`, `municipality`, `type`, `authority`, `url` | string | optional | shown in the popup |
| `approx` | bool | optional | located loosely |
| `area_km2`, `osm_name` | number, string | optional, `special` | the circle's area when OSM has no boundary; the boundary's name to look for |
| `dot` | bool | optional | whether a polygon or line also shows as a dot (default: points and lines yes, polygons no) |

Other properties pass through to the popup; `null` is absent. Every entry has a position when
dropped: locating is the producer's job.

```toml
name = "Mérimée, monuments historiques"
what = "France heritage"
credit = "Ministère de la Culture, base Mérimée (POP)"
licence = "etalab-2.0"
class = "heritage"
territory = ["FR", "GP", "MQ", "GF", "RE", "YT", "PM", "BL", "MF"]
wikidata_property = "P380"
osm_ref_tags = ["ref:mhs"]
```

**Checks.** Errors: an unexpected file; invalid GeoJSON or geometry; a coordinate out of range; an
`id` missing or twice; a property mistyped (a `level` outside 1–5, an unknown `tier` or `kind`, a bad
`wikidata`); what the class needs missing; a description missing, without credit or licence, or with
the licence `unverified` (`he-licence`: never read until someone checks it); a bad `class`,
`wikidata_property`, `osm_ref_tags` or `territory`; a `.geojson` without its `.toml` or the reverse.
Warnings: entries over 20 km outside the territory's outlines (one per register); a register losing
over 10 % of its entries, or removed; over half its entries `approx`; entries without a name.
Derived: `facts.box`, `facts.entries`, and `facts.extent` (the box) for the credits.

**Ingesting.**
- **One generic reader:** the heritage-sites job (docs/steps.md; docs/phase5.md) lays out every accepted
  register whose box meets one of its cover's z6 tiles (`landmarks::heritage::registers_meeting`)
  for dem/heritage.py, which reads them in id order: a point is a site; a polygon a heritage area
  (and a site at its representative point with `dot`); a line a site at its representative point; a
  `special` point is matched to the OSM boundary named `osm_name` (else its name) or drawn as a
  circle. Every feature carries `register`, a site its `ref`.
- **Merging is a build rule:** two entries are one place when the matches say so (§6.5.4), or when
  the lower-graded is local (level 5) and lies within 40 m of another register's entry of the same or
  higher level sharing a name word of three letters or more. Of two local entries, the larger
  register's wins (so a register growing past another can flip which is kept); a `same` match keeps
  the highest-graded entry (ties: first register by id) and pins a pair that matters. The place kept
  lists every register merged (`registers`).
- **Keys:** heritage-sites names the meeting registers' `keyed` digests (`registers_now`) and the
  matches' accepted version, and is given the indexes it planned with (`--heritage-index`,
  `--heritage-matches-index`). Without an accepted version it waits (and the z6 tiles, marks and
  overlays with it).
- **Credits and popups** by register (§4.8). A register's whole box decides where it's credited.
- The chain's caches (Wikidata, Wikipedia, World Heritage answers) are the pass's answers
  (`sources/items/<date>/heritage.tar.zst`, `agentcore::answers::heritage`), a new pass starting
  from the last one's. With no pass's archive yet, the first start seeds its working copy from the
  old snapshot `sources/registers/legacy`'s caches (read only) and the answers kept beside it
  (`agentcore::answers::heritage_start`); a snapshot named but unreadable fails the job.

#### 6.5.1 What stays generic in the heritage chain

The chain's scripts (docs/phase5.md) read the registers' fields, not per-register knowledge:
landmarks::heritagewd looks a site up by `ref` under its register's `wikidata_property` (a World Heritage
site by `site`) and links through OSM by `osm_ref_tags`; landmarks::whsshapes finds a site by `site`, its
components by `ref`, its area by `site_area_km2`, its outlines by OSM's World Heritage tags;
landmarks::filterprops takes `t` from `tier`. Names chosen per language (`lang_at`) are the producers'.

#### 6.5.2 The heritage chain's stand-in root

The chain runs in a stand-in root laid out as the old repository (`data/build`, `data/heritage`:
scenic-build's `heritage_root`). The registers and matches are given on heritage.py's command line
(`--registers`, `--matches`: folders of links to the accepted copies), and `data/heritage` is the
pass's working copy of the chain's answers (`cache/heritage-<date>/`).

#### 6.5.3 English names, park facts and pageviews

The old `sources/registers/legacy-seeds` archive is read by nothing:
- **English:** the heritage layers' `en` is the thing's own English only (`landmarks::heritage_layers`
  `english_at`: a site's UNESCO name or English Wikipedia title, an area's `name_en`). Other English
  is the server's, from the translations as it serves the layers (docs/names.md). A name lacking English
  goes onto the translations' to-do (`landmarks::namestodo` reads the landmarks' `en`).
- **The chain's Wikidata queries** (landmarks::heritagewd's sites, landmarks::areadetails' parks,
  and the items job's landmarks::items) ask each fact in a UNION branch holding the batch's items
  (or register IDs), so a query's cost is its batch's: QLever works an OPTIONAL or a branch out
  on its own before joining it, and one naming no item read every description or label in Wikidata
  (9-45 s for a single item). Language filters are written `STR(LANG(…))` (`landmarks::wikiask::fact_union`).
  The facts are put together in Rust, each picked the same way whatever order the service's rows
  come in: the least value where it has two (a description, an article), the earliest inception,
  the largest number, labels distinct and sorted; a register ID's items in QID order.
  heritagewd asks what details-heritage.jsonl and the layers read: sitelinks, inception,
  descriptions and articles in en, fr, es, pt and ca, the type's, style's and architect's English
  labels. items asks what the landmark popups and filters read (web/src/details.ts,
  landmarks::marksjob, landmarks::interest): sitelinks, the English description and article, water
  body, mountain range, inception, height, prominence, isolation, discharge, focal height; a cached
  record's other facts (an older query's `d_loc`, `elevation`) are left out of facts.json.
- **Park facts:** landmarks::areadetails' cache is the pass's answers (`areas-wikidata.json`,
  `agentcore::answers`, linked into the chain's stand-in root and written through the link after
  each batch), asking Wikidata only for parks it lacks. Each fact is picked the same way whatever
  order the rows come in: the English
  description and article (the least, where the service has two), the earliest inception, the
  largest area and visitors, the operators' English labels sorted. These are what the map's park
  popup reads (`enrichArea` in web/src/details.ts); a record in the answers that holds others (an
  older query's) is written to details-park.jsonl without them.
- **Pageviews:** the items job's months' indexes (`sources/pageviews/`, docs/steps.md).

#### 6.5.4 Heritage matches: the owner's judgment calls

**Kind:** declare. **Inputs folder:** `inputs/heritage-matches/`, `*.jsonl`. Code:
crate::inputs::heritage_matches; applied by dem/heritage.py.

| Field | Type | |
|---|---|---|
| `a`, `b` | `<register>:<id>` | required |
| `same` | true: one place; false: never merged | required |
| `note` | why | optional |

```json
{"a": "ca-dfhd:1234", "b": "ca-qc-rpcq:92847", "same": true, "note": "the same church; the federal entry is 300 m off"}
```

**Checks.** Errors: malformed lines; `a` equal to `b`; contradictions (a chain of `same` joining two
entries a `same: false` separates). Warning: a pair naming an entry no accepted register has (the
registers' version is in the key, so a register update re-checks the matches).

**Ingesting.** Applied before the rules: `same` pairs merge transitively (the highest-graded entry
kept); `same: false` pairs are never merged. Their accepted version is in heritage-sites' key.
Planned: a to-do of likely matches the rules didn't take.

### 6.7 Timetables

One input in two units: feeds the app fetches (source declarations) and static GTFS the owner
compiles by hand (declare). OSM decides what's drawn; timetables only add counts. Each mode has one
counter, whichever unit its timetables come from.

#### 6.7.0 The counters, per mode

- **Rail** (`landmarks::gtfs::railcount`): trains per stop pair on the typical day, as the PAIR records the
  `rail` job matches onto the tracks (`rail::PAIR`: two stops' positions, a mode byte, trains a
  day). A feed's typical day is its median-busy Tuesday–Thursday from 30 days before its zip's fetch
  day to 90 after; a train in two feeds counts once (keyed by first and last stops and times, the
  operator's feed first).
- **Ferries** (`landmarks::gtfs::ferrycount`, run by the `ferries` job's `landmarks::ferries`): per OSM ferry line,
  the record the map shows (`info["freq"]`): `per_day` (each way, on days it runs; under four days
  a week, weekly ÷ 7), `per_day_low` (the quietest month's), `headway` (median gap 07:00–19:00),
  `days`, `months`, `season` (`year`, `some-days`, `seasonal`), `overnight`, `source`, `url`,
  `checked`. A GTFS route matches a line when its trips call within 400 m (at most 30 % of the
  line's length) of both its ends. Feeds count from their fetch day, in id order, the first feed's
  count winning.
- **Precedence** for a ferry line (`landmarks::ferries`' `rank`): a feed's count, then a static file's (in
  the zips' order), then OSM's `interval` tag.

#### 6.7.1 Feeds

**Kind:** source declarations. **Inputs folder:** `inputs/timetables/feeds/`, unit `timetables/feeds`.
Code: crate::inputs::feeds; crate::railfeeds; crate::rail.

**A feed:** `<id>.toml` (letters, digits, `-`, `_`), a description (§5.1) and:

| Field | Type | Meaning |
|---|---|---|
| `[data] format` | `"gtfs"` | |
| `[data] url` | string | the zip's URL, or the API's for a keyed feed |
| `[data.fetch]` | table | §5.2; LTA's: `method = "get-key-json-link"`, `header = "AccountKey"`, `key = "LTA_ACCOUNT_KEY"`, `link = "value[0].link"` |
| `[data] refresh` | `days:<n>`, default `days:7` | a copy with no service in its window is fetched again once that old |
| `modes` | `"rail"`, `"ferry"` | the counters that take it; none: both (rail if `routes.txt` has rail routes) |
| `order` | whole number | its place among declared feeds (then by id); declared feeds come before the catalogue's |
| `replaces` | feed ids | feeds left out while this one has a zip |
| `ferry_routes` | route ids | routes counted as ferries whatever their `route_type` |
| `pin` | `{zip, day}` | a zip fetched before the app fetched the feed, and its day: used instead of fetching when the feed has no zip |
| `operator` | string | shown (not keyed) |

All fields are keyed but `operator` and the description's unkeyed ones; `name`, `licence` and
`source` are keyed too (they're in the rail feeds' list and the ferry records). `territory` says
which countries' coverage takes a rail feed. No box: a rail feed is used where its `stops.txt` is;
ferry feeds everywhere.

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

**The catalogue:** one `<id>.toml` with `format = "mdb-catalogue"` (the Mobility Database's
`feeds_v2.csv`), its `url` (fetched when `sources/rail/catalogue` has no copy), and corrections:
`exclude` (id → where its trains come from instead), `country` (id → ISO code, for feeds filed
under another country), `replaces` (id → ids), `own_code` (ISO 3166-2 → alpha-2 or `""`: Åland's
FI-01 is AX, Kosovo's RS-KM XK, Ceuta none). Its feeds are chosen by the countries the coverage is
in (`rail::countries`) and the feeds' boxes.

**Checks** (`inputs::feeds::Feeds`). Errors: a file not `<id>.toml`; no credit or licence; no
`[data]`, an unknown format or method, a non-http(s) `url`; a keyed method without `header`, a bad
`key` name, `get-key-json-link` without `link`; a bad `refresh`, `modes` or `order`; malformed ids in
`replaces`, `exclude`, `country`; a `country` or `own_code` value not ISO syntax; two catalogues.
Warnings: `replaces` or `exclude` naming a feed neither declared nor in the catalogue's copy; an
`own_code` key no outline has; two declarations with one URL. A declaration is held with those it
`replaces`. A `key` missing from `inputs/keys.env` isn't a finding (keys.env isn't an input): the
feed is left out until keys.env holds it, and the build page says so under Trains (the agent's
waiting line naming the key). What a fetch brings back is the fetching job's status, not a finding.

**Fetching.** `rail-feeds` (docs/steps.md) reads the accepted declarations (`--feeds-index`) and the
catalogue's copy and writes `sources/rail/feeds` and `sources/rail/ferry-feeds`, each zip pinned by
content in `sources/rail/gtfs/<id>` with its day (`sources/rail/fetched`). Its key names the
declarations' `keyed` digests (`inputs::feeds::declared`, not the index, so an edited note or credit
doesn't rerun it), the catalogue's copy, the outlines, which of the keys the declarations fetch
with `keys.env` holds, and the coverage as its run asked it (docs/steps.md §12: the feeds' boxes
and the countries; a region change elsewhere keeps it). `rail` keys on the rail list, the cover and
the coverage as it asked it (its stops and ways), `ferries` on the ferry list. With a catalogue copy but no accepted catalogue declaration,
rail-feeds fails rather than read it uncorrected. A declaration's `pin` is put in by rail-feeds the
first time it runs with no zip for that feed, so a pinned feed is never fetched before its pin.

**Credits.** A declared feed in the rail or ferry feeds' list with a zip is credited in the map
from its description, wherever it is (`inputs::feeds::credited`; the map update's key names those ids,
and the map update waits while they can't be read). The catalogue's own feeds are credited by the
table's line (`rules::CREDITS`). The catalogue's copy is fetched only when there is none: it isn't
refreshed yet (task #367).

#### 6.7.2 Static GTFS

**Kind:** declare (the fulfil half of §6.7.3's to-do). **Inputs folder:** `inputs/timetables/gtfs/`,
unit `timetables/gtfs`: `<name>.zip` (letters, digits, `-`, `_`), each a GTFS feed compiled by hand,
with an optional `<name>.toml` description (held together). The researchers' brief is in `how/`.
Code: crate::inputs::gtfs; crate::gtfs::statics; `producers/timetables/` (`gtfszip.py` and others).

**Shape:** GTFS with the required files (`agency.txt`, `stops.txt`, `routes.txt`, `trips.txt`,
`stop_times.txt`, `calendar.txt`; `feed_info.txt` naming the publisher) and the columns the counters
read (`trips.txt` and `stop_times.txt` may be headers only when every route is in `counts.txt`), and:
- `frequencies.txt` allowed;
- **seasons, no expiry:** `calendar.txt`'s dates are read as month and day over one reference year,
  the year ignored (`20000501`–`20001031` is May–October every year; a start after the end wraps
  the new year; 29 February reads as the 28th); `calendar_dates.txt` is ignored;
- `routes.txt`'s `route_url` (the source page) is required, with extra columns `checked`, `months`
  ("May–Oct") and an optional `osm` pin (`r14098916`, `w23471991`), the OSM line a route counts for;
- **`counts.txt`**, an extension table for services the source gives only as counts, so no time is
  invented: `route_id`; for rail `from_stop_id`, `to_stop_id` and `at_least` (1: a lower bound);
  then `per_day`, `per_week`, `per_day_low`, `headway` (minutes, or the source's words: its first
  number is read), `days`, `months`, `season`, `overnight` (1/0), `note`. Empty cells aren't given,
  `null` means the source gives none. A route with count rows has no trips; a counted ferry route
  needs its `osm` pin. Count rows are taken as given, never deduplicated against feeds: the
  catalogue's `exclude` keeps a feed's copy out where both would count (as for the MTR).

**The counters** read the accepted zips in path order (`inputs::gtfs::zips`; `--static`):
- **Rail:** trips as a feed's (frequencies expanded), the typical day the reference year's
  median-busy Tuesday–Thursday, deduplicated with the feeds by times. Count rows of rail routes go
  after the trains' pairs, in order: per row the trains each way (`per_day`, else `per_week` ÷ 7),
  A→B then B→A, with the lower-bound bit (0x80) for `at_least`.
- **Ferries:** a count row of a ferry route with an `osm` pin is that line's record as given
  (`source` the route's `route_long_name`, `url` its `route_url`, `kind` "timetable"). Trips are
  counted over the reference year (a break of a month or more makes a line seasonal); a pinned route
  counts for that line alone.

**Checks** (`inputs::gtfs::Gtfs`). Errors: an unexpected file; a description without credit or
licence; an unreadable zip (stored and deflated entries only, no Zip64); a required file or read
column missing; broken references (trip → route, service; stop time → trip, stop; frequency → trip;
count row → route and, for rail, its stops); a bad time, calendar date or `route_type`; a non-positive
`headway_secs` or numeric `headway`; a count that isn't a number or `null`; `overnight` or
`at_least` not 0/1; a route without `route_url`; a bad `osm` pin; a counted ferry route without a
pin; a route with both trips and count rows; stop coordinates out of range. Warnings: a route with
neither trips nor count rows; two zips sharing an agency's `route_id`. Whether a route matches the
map depends on OSM and is the report's (§6.7.3), not a finding.

**Ingesting.** `rail` and `ferries` read the accepted version the agent planned with
(`--gtfs-index`), key on its index, and wait while there's none ("waiting for the static
timetables' first accepted version").

#### 6.7.3 The to-do list and the report

Code: crate::timetables; the lead-only `timetables-todo` job. Both depend on OSM: neither is a
finding, and nothing waits on them.

- **The to-do:** `inputs/timetables/gtfs/todo/lines.jsonl`, rewritten after each rail and ferries
  run: OSM rail and ferry lines in the coverage no timetable covers, `{"osm": "r123", "kind":
  "rail"|"ferry", "name", "ref", "operator", "from", "to", "km", "priority"}`, highest first
  (`priority` is the length, a tenth of it for `passenger=no` or `service=freight`), beside
  `todo/README.md` (from `tools/timetables/todo.md`). Rail lines are the rail set's service relations
  grouped as the ferries' are (by network's or operator's `ref`, else the same ends), listed while
  under half their length has trains in `global/railfreq`. Ferry lines are listed with an end in
  the coverage and no count from a feed or static file.
- **The report** (`work/timetables/report`): timetable routes matching nothing, `{mode, source,
  static, route, name, agency, osm, why}`, on the build page (Details, Timetables) and in `scenic
  status`: rail routes none of whose stop pairs in the coverage found a track (railfreq's
  `rail-miss.bin`), static rail routes whose `osm` pin names no service relation or rail way; ferry
  routes no line took, and static ferry routes whose pin names no ferry line of this pass.

`rail` hands off `work/rail/lines` (the rail counter's routes, railfreq's misses, the service
relations); `ferries` hands off `work/ferries/lines`. `timetables-todo` (keyed on both, its version
and the coverage as its run asked it: the ferry lines' ends, docs/steps.md §12) merges them, writes the to-do into `todo/` outside the records (as `names-todo`
does), and hands off the report, which the lead reads for its status (`Status::timetables`).
Planned: matching rail routes to lines by their tracks (today a route with one matched pair isn't
reported).

### 6.8 Elevation

**Kind:** source declarations, one per dataset; a small dataset's files may lie beside it. **Inputs folder:** `inputs/elevation/`: `<dataset>.toml`, tile indexes (`<name>.geojson`) and `<dataset>/` for
local files. Code: crate::inputs::elevation (the check), crate::dem::generic and crate::dem::declared
(the sampler and blend), crate::dem::encoding, crate::dem::store_fetch (the store), crate::terrain_base
(the terrain's base), crate::dem::spacing (the densification), crate::dem::geoid (the heights brought
to EGM2008). Built for the roads' elevations (#143 step 1), the densification (step 2), the
heights' conversion (step 3) and the 3D terrain (#144). One pool: the roads and the terrain read
the same declarations.

**Shape:** a description (§5.1) and:

| Field | Type | Meaning |
|---|---|---|
| `surface` | `ground` or `surface` | a DTM, or a DSM with trees and buildings |
| `heights` | `EGM2008`, `EGM96`, `CGVD2013`, `NAVD88`, `JGD2011`, `TWVD2001`, `ellipsoid-WGS84` | the height reference, brought to EGM2008 before the blend (below) |
| `rank` | integer, default 0 | overrides the order by resolution (lower first), where datasets of one resolution differ in quality |
| `code` | 1–9 | the source byte (at most `roadcore::NDEM` − 1, where the server counts a way's sources) in the z6 tiles' outputs (`src.u8`), named on the map by `label` |
| `label` | string | the short name a road's details give its elevation source (`"GSI lidar (5 m)"`), else `name`; one label per `code` (an `el-decl` error otherwise); not keyed: it reaches the map through the map's `dem_labels` (formats.md, The catalogue), as a credit edit does (§4.8) |
| `regions` | region ids (§6.1) | optional: the map regions the dataset is used in at all, by the roads and the terrain alike; elsewhere the next dataset in the order serves both. Its coverage (`facts.measured.cover`) is cut at check time to the 1° cells meeting one of them (`declared::scope_cover`; the check's key names the accepted regions and the index records them, `derived_from`, so a regions change, no file changed, cuts every dataset again and writes a new index: from what was measured, `facts.measured.uncut`, no data read again); an id no accepted region has is an `el-data` error. Then it's cut to the regions' outlines inside those cells (`declared::scope_outline`, `facts.measured.outline`): their shapes, land and any water a region declares, with their buffer, as the coverage holds a point (§6.1). A cell they hold whole together is used whole; every other is an edge cell, where each point is tested against the outlines as the terrain's base samples (`declared::Cover::contains`, the regions' stored geometry read by the plan, `OutlineCut`); outside them the next dataset serves. The roads take their heights from the finished terrain, so they have the same cut. Not in its `keyed` digest (`Declaration::keyed_part`): a key names the cut only where it changed what the key's box reads (`cut_note`: the cells it cut there, and in each edge cell it meets only the outlines inside the key's box, grown by a 256th of it each way: a box the outlines hold whole there keeps its key, one wholly outside them counts the cell as cut, else the outlines there are named; the keys read the regions' stored geometry as the plans do), so the z6 tiles whose cells the outlines wholly hold keep their keys and those whose datasets change are built again |
| `exclude` | boxes `[w, s, e, n]` in whole degrees | optional: 1° cells the dataset isn't used in, by the roads and the terrain alike; elsewhere the next dataset in the order serves both. Cut from its coverage (and from its outline's edge cells) after its `regions`, at check time; not in its `keyed` digest, so as with `regions` only the keys whose box reads a cut cell change (`cut_note`). e.g. `[[153, 24, 155, 25]]`, Minamitorishima, for GSI's datasets (JGD2011): GSIGEO2011, their geoid grid, stops near 150° E |
| `territory` | ISO codes or `"world"` | required for a tile pyramid (its coverage is its territory's outlines' boxes, in the newest pass's outlines: the index records which, `derived_from`, so a new pass measures such a pyramid again); `"world"` makes any dataset's coverage the world |
| `[data] format` | `geotiff` or `tiles` | |
| `[data] url` / `urls` / `index` / `files` | | exactly one: GeoTIFFs by URL, a tile index (GeoJSON footprints with each file's `url` or `file`, and optionally its fill mask's URL, `mask`; made by `producers/elevation/indexes.py`), or files added by glob |
| `[data] store` | bool | the files (and fill masks) read from `sources/fetched/<dataset>/` alone, by their URLs' last part (after a `#`, the member's name); never fetched by a build: a file the store lacks is an error naming it, one noted `<name>.none` has no data. Filled once by `scenic-build store-fetch <id> --index <its index>` (crate::dem::store_fetch: every file of the index and its mask the store lacks, as published, by the build's fetcher: its User-Agent, 429 and 5xx answers waited out; each checked a whole GeoTIFF under a temporary name, then renamed; restartable) |
| `[data] encoding`, `url`, `zoom` | | for `tiles`: `terrarium`, `terrain-rgb`, `gsi-png`; the URL template; the zoom |
| `[data] every_zoom` | | for `tiles`, optional (default false): the pyramid is published at every zoom to `zoom`, so the terrain reads it at a tile's own zoom and takes the coarse relief outside every region from it alone (crate::terrain_zoom::relief); else it's read at `zoom` only, tile by tile. A terrain job with datasets accepted and none of them so declared fails, saying so (the relief would be sea level) |

```toml
name = "NRCan HRDEM, 2 m lidar mosaic"
what = "Road elevation, Canada"
credit = "Natural Resources Canada"
licence = "OGL-Canada-2.0"
surface = "ground"
heights = "CGVD2013"
[data]
format = "geotiff"
index = "hrdem-tiles.geojson"
```

**Read from the data at check time** (`facts.measured`, beside the parsed `facts.declaration`): a
GeoTIFF dataset's CRS and resolution (headers by range request: an index's first, last and seven
files between, or every file without an index; one CRS the app projects, the coarsest of those
files' full resolutions, at which every dataset is read, and the datasets' order) and its cell by
latitude band (`bands`, a geographic GeoTIFF's: each file read's latitudes and the coarser of its two
ground spacings, its longitude's at its equatorward edge, the coarsest of a band's files; none for a
pyramid or a projected GeoTIFF, whose `resolution` holds everywhere), which sets the terrain's finest
zoom there (crate::terrain_zoom), and its coverage (`cover`): the 1° cells its data meets, merged into boxes. Index
files meet by footprint; other files by where they have valid data at their coarsest overview, so a
file's empty margins cover nothing. And where its files really have data (`presence`, a GeoTIFF's
not holding the world: crate::inputs::elevation::presence_of): the z10 tiles of its cells (the
deep packs' zoom) one of whose points, no further apart than half the finest feature of the files
there, lies in a file's data, each file narrower than such a tile by its index footprint and every
other by its coarsest overview's valid pixels, every file of the dataset read so. The terrain's
finest zoom and the roads' spacing count a dataset only where it has data so
(crate::terrain_zoom::sources: a tile its cells merely touch, or whose files hold no data there,
isn't one of its places); measured once a version, a version measured before it counts its cells.
Of those, where a finer read finds no data (`hollow`, crate::inputs::elevation::hollow_of,
tasks #842, #1089): each file meeting a presence tile (a narrow one's footprint opened too) read
over the tile's box at the coarsest of its levels whose pixels are no wider than 1/128 of a z10
tile at the equator (~300 m; its full resolution where none is), its sparse blocks read as the
sampler reads them (nodata, else 0), eight files at once. A tile holds data there where a valid
pixel's centre lies inside it further than an overview's reach from its edge (3 of the level's
pixels, `OVERVIEW_REACH`: a resampling reaches past the pixel, and HRDEM's lidar mosaics' overviews
hold data up to 2 pixels from the patch beside a tile, whose runs then found nothing). Every other
tile is read again so at 1/1024 (~40 m), and holds data where a valid pixel lies so far inside it,
else where a valid pixel meeting the tile (any part of it, its edges included) has under it a
pixel of the full resolution with data meeting the tile (one of the full resolution itself, where
no level is finer). A pixel is valid as the sampler takes it (a value equal to the file's nodata,
whatever height it could be, is none to a run too). So a tile is hollow only where no pixel of the
full resolution with data meets it, as long as an overview's pixel is valid wherever one under it
is (an average's, or a wider kernel's ignoring nodata; a nearest-neighbour overview could miss an
island narrower than ~40 m). The plan reads it, and terrain-deep's and slope-deep's runs: a terrain-deep target whose every source reaching
past z12 is hollow there is never run, or, with an earlier run's outputs named, dropped once its
key moves (docs/steps.md §2, Terrain); a place's sources and every key stay `presence`'s, so the
hollow tiles alone move no key. A GeoTIFF dataset's measures are made in that order, each from those
before it (`Step`: its headers, giving its CRS, resolution, bands and coverage; its presence; its
hollow tiles), and a dataset taken as accepted (below) that a version of the check with fewer
measures measured is measured from the first step it lacks on, every step after it made again, what's
before it kept (task #1088): one accepted by checks version 5 has its hollow tiles read, nothing
else; a projected one accepted by version 4, its presence and hollow tiles; a geographic one, its
headers too, for its bands (so measured whole). A step lacks its measure too where it was made under
an older rule than the check's (`Step::rule`, raised with the checks' version when a step's rule
changes): each GeoTIFF dataset's entry names the rule each of its steps was made under (its `rules`
fact, `{"headers": 1, "presence": 1, "hollow": 2}`; an entry naming none, each step's first, 1), so
checks version 7, the hollow tiles' rule 2, reads every dataset's hollow tiles again and nothing
else (task #1089). What's kept and what's made are what a whole measurement of the same data
makes, so its keys don't move. A measure is lacking only where it isn't
none by design: a projected dataset's bands, or the world's presence and hollow tiles. A geographic
dataset holding the world (glo30) accepted without bands keeps its single `resolution` everywhere
and its headers aren't read again: bands there would move the keys of everything built from it. A
dataset whose declaration or data changed is measured whole: its memo key is its `keyed` with its
data's (`data_digest`: its index's content and its added files' names and contents, as the gate
holds them; a file named by URL counts by its URL, as the units' keys count it).
A tile pyramid's resolution comes from its zoom at its coverage's
mid-latitude, and a tile is read to check it's 256 px (`generic::TILE_PX`). The coverage is also the
credits' extent (§4.8).

**Checks.** Errors: a file not of the shape (`el-shape`); a description field wrong (`el-desc`); a
declaration's fields wrong: unknown `surface`, `heights` or encoding, no `code`, a template without
placeholders, not exactly one way of naming files (`el-decl`); an unparsable index (`el-index`); an
index or files missing (`el-missing`); unreadable or unmeasurable data: a header, a missing file, a
CRS the app can't project (EPSG 4326, 4269, 3979, NAD83 / UTM 26901–26923, polar stereographic 3031
and 3413: `inputs::dem::proj`,
`declared::known_crs`) or contradicting its index, a pyramid without a known territory or with
non-256 tiles (`el-data`); heights no chain of the accepted geoid sources brings to EGM2008
(step 3), or whose chain's grids can't be read (`el-geoid`: declare the geoid source first).
Warnings: heights whose chain's grids don't reach all of the dataset's coverage (`el-geoid`, by
each grid's box, against the coverage's 1° cells: the cells left out, merged into boxes, and the
area left out in km², reckoned at 0.05°; a grid's own voids inside its box are the sampler's
`unconverted`); a folder or index no declaration names (`el-orphan`); two
datasets of one rank, resolution and surface meeting (`el-tie`). A declaration is held with the
index it names. The checks run whole on every check, files changed or not (`holds_unchanged`), so
a finding on an unchanged dataset (an `el-geoid` the accepted sources or this check's version
newly raise) holds it as accepted and shows it held. A dataset whose declaration and data are as
accepted (its `keyed` with its data's) isn't measured again: its accepted `facts.measured`, before
the cut, is taken (`Checks::seed`), the measures it lacks made, and cut again; a territory's tile pyramid is, when the pass's
outlines aren't those the index names. The checks themselves (`el-geoid` among them) still run on
every check, on what's taken. Planned: files not meeting the declared territory.

**Projections** (crate::dem::proj, as PROJ 9.8 computes them, tested against pyproj): WGS 84 to
NAD83(CSRS) by EPSG:1946's Helmert, then Canada Atlas Lambert (EPSG:3979: HRDEM) or a NAD83 /
UTM north zone (EPSG:26901–26923: USGS's 1 m tiles; NAD83(2011) taken as NAD83(CSRS), so through
the same Helmert, not PROJ's null pick for EPSG:269zz, a metre off in the north-east), for the
input check, the plan (`declared::projected_cells`, `plan_converted`) and the sampler
(`proj::Projection`, in crate::dem::generic) and the deep terrain's reading ahead (`terrain_deep::plan_files`, `plan_items`).
The sampler projects a batch's points by `proj::Projector` (task #1098): a sampled tile's rows
share a latitude and its columns a longitude, so the sines and cosines of the geocentric point
(x = (n·cos φ)·cos λ, y = (n·cos φ)·sin λ, as the one expression multiplied them) are worked out
a row's and a column's, the rest a point's: the same bits as `Projection::project`.

**Ingesting.**
- **Order at a point:** the datasets covering it by `rank`, then resolution (finest first), then
  ground over surface, then id; the first with valid data gives the height and its `code`. A
  dataset's files are tried in index order, a point in those whose footprint holds it (min x ≤ x <
  max x, min y < y ≤ max y), sampled by crate::dem::sample. A file with a fill mask (the index's
  `mask`) has no data where its mask's nearest pixel is 3 or more (`declared::MASK_FILLED`:
  GLO-30's FLM, filled from another DEM): the next dataset serves there, as at any void.
- **Blended at edges:** where the next dataset with valid data (B) lies beneath the first (A), within
  a band of four of the coarser dataset's pixels of A's invalid area, h = B + (A − B)·s(d / band),
  s(t) = t²(3 − 2t). Past the band, A's height to the bit; the source byte is A's either way. The
  distance is looked for pixel by pixel only where one can be found: the points are grouped (by
  file and square of 256 pixels, a pyramid's by tile) and the groups measured in parallel; each
  block (or tile) read is noted clean when every pixel of it holds data
  (crate::dem::sample::Clean), and a group whose bands lie in clean blocks inside its file is at
  the band's full distance, no pixel read again. The others share a map of the invalid pixels
  (past a file's edge, the files holding them read once a group, `generic::Past`) with a count of
  them over any window (`generic::Invalid`): a point whose band holds none is at its full distance
  at once, the rest look at two pixels a row and only the rows nearer than the nearest invalid
  pixel found so far, what a scan of the whole band finds, to the bit (task #1095). The decoded blocks of every file share one cache (`generic::CACHE_BYTES`, 512 MB),
  so a unit opening tens of files holds that much, not a cache's worth per file. A block several
  threads ask for at once is read and decoded once (`Tiff::block`), the others waiting for it.
  A GeoTIFF dataset's blocks are read ahead of their sampling (`generic::read_ahead`, on
  store::ahead's threads, task #1096), a fetched dataset's over the network and a stored one's read
  in place on the NAS alike (a whole copy on this Mac's disk is read as sampled): its files first
  opened together (`GeoSet::blocks_ahead`), then each block's bytes as stored (`Tiff::fetch`, held
  in the cache within its one budget, a quarter of it at most, decoded blocks let go to make room;
  a level read whole, `read_level_whole`, has nothing to read ahead), at most `GEO_AHEAD` (128)
  blocks past the chunk being sampled and no more than the list's share of that quarter (the chunks
  sampled at once share it, `BlockCache::begin_list`, so none of their blocks is let go before it's
  decoded, to be read twice), decoded by the sampling's own threads as they come to them. How many reads are in flight
  is each source's limit, found as TCP finds its window (`store::aimd`, task #1096), whatever
  number of chunks a process samples at once: the NAS's one a process (starting at 2; decided once
  a window, on the median of its reads' latencies over their size class's floor (the least their
  latency smoothed over about eight reads was in the last 30–60 s, never under 2 ms; an answer of
  this Mac's own cache, under 2 ms, no sample): doubled until the first halving and one more after
  while near the best, halved past three times it or when a read times out; 1 to 16), a host's one a host (the fetcher's, below); so the sampler doesn't wait on a chunk's few
  blocks one round trip after another, and its threads never sit in a read its points foresaw. A tile pyramid's
  tiles likewise (`TileSet::rolling`): `TILE_FETCHES` (64) kept in flight, whatever the threads
  the step was granted (task #665: a terrain job fetched GSI's tiles only on its three sampling
  threads, one at a time each, about 2 tiles a second where the host answers 15 at once); the
  terrain pool's own Terrarium tiles, read from this Mac's cache or the NAS's archives
  (`terrain_base::RawFetch`), are read only as many at once as the job has threads, as before.
  The terrain's jobs read a pyramid's tiles from its store on the NAS first
  (`sources/pyramids/`, crate::pyramids: GSI's, filled by the lead's `pyramids-fill` step, docs/macs.md),
  fetching only those it lacks.
  Only each point's
  first file (the first whose footprint holds it) is read ahead; a point that file has no data for
  reads the next file's block when it's sampled. The reading ahead goes no further than a quarter
  of the cache holds (`generic::ahead_for`), and it stops with the sampling: a read of it waiting
  out its retries gives up at once (`fetch::cancellable`), and every retry checks its host's
  down note first (`SCENIC_FETCH_DOWN`), so a host that stopped answering doesn't hold the step
  through each thread's retries. The fetcher holds
  a process's requests to one host to that host's limit (`fetch::net::HostSlot`, store::aimd),
  whatever its readers keep in flight: starting at 2 (`HOST_START`), up while answers come near
  their best (decided as the NAS's, once a window on its median answer; the floor never under 20 ms), halved when they slow to three times it, on a 429 or a 5xx, never past the host's ceiling
  (`fetch::host_ceiling`): Amazon S3's 64 (it states 5,500 GETs a second a prefix), every other
  host's 16 (`HOST_DEFAULT`, every host's fixed cap before): the hosts read live are free services
  that publish no limit (GSI's among them, task #665 review).
  A fetched file's bytes are got in 16 KB blocks, held in memory (16 MB a file), and a run of
  blocks one thread is getting is waited for by another wanting any of them, not fetched again
  (that thread failing, the other gets them itself);
  what the network gave is kept on this Mac's disk (`blobs/net/` in the agent's cache, by the
  file's version as its server names it, only past the room target: buildstore::fetch::kept, task
  #1099) and read there by a later read of it, in the job or a later one here.
- **Heights brought to EGM2008** (step 3, crate::dem::geoid), each dataset's before the blend (A's
  and B's alike): the conversions are declared sources (§6.9, role `geoid`, any number), each
  linking two references by `params.from` and `to`: a separation grid (`geoid-grid`: one GeoTIFF in
  degrees, its value added to a height in `from` to make it one in `to`, an integer grid scaled by
  its GDAL metadata; `params.store`: read from `sources/fetched/<id>/` by its URL's last part) or a
  Helmert between two frames (`helmert`: `translation_m`, `rotation_mas`, `scale_ppb`,
  `convention`, optionally `rates`, `epoch` and `at`; the change of a point's ellipsoidal height on
  GRS 80). A dataset's chain is the shortest from its `heights` to EGM2008 (a link walked from `to`
  to `from` subtracts; references by name where chains tie); a link declared by several sources (a
  grid a region) takes, at a point, the first by id with a value there. Grids are read bilinearly
  between pixel centres in f64 (no data where a neighbour has none). A point no source of a link
  covers keeps its height as sampled, counted (the sampler's `unconverted`; a blended one whose
  first dataset was converted and whose dataset beneath had no value, `unconverted_seam`). The
  input check holds each dataset to its chain (`el-geoid`, below, against the accepted sources: the
  check's key names them): no chain is an error, and a chain whose grids' boxes miss part of the
  dataset's coverage is a warning, the dataset accepted only with it (§4.5). A dataset already
  accepted that the sources then leave without a chain is held, its accepted version still read
  (its heights then left as they are, said on the agent's stderr).
  `producers/geoid/fetch.sh` downloads the grids (published data: no robots.txt, §6.9). Only
  EGM2008 is named in code. `scenic-build geoid-offsets --sources <folder>
  --from <reference> --at <lon,lat>[;…]` prints a chain's offsets, hop by hop.
  - **The grids** (staged in `producers/geoid/declarations/`, fetched by `producers/geoid/fetch.sh
    <root>` into the store; PROJ-data's GeoTIFFs on cdn.proj.org): NAVD88 → NAD83 by GEOID18
    (`geoid18-conus`, 16.7 MB, public domain) and GEOID12B over Alaska (`geoid12b-alaska`, 11.3 MB,
    public domain); CGVD2013 → NAD83(CSRS) by CGG2013a (`cgg2013a`, 11.0 MB, OGL-Canada-2.0); NAD83 →
    WGS 84 by NGS's ITRF2008 → NAD83(2011) Helmert at 2010.0 (`nad83-itrf2008`, no file; NAD83(2011)
    and NAD83(CSRS) taken as one frame, WGS 84 as ITRF2008); JGD2011 → WGS 84 by GSIGEO2011
    (`gsigeo2011`, 0.4 MB, CC-BY-4.0); EGM2008 → WGS 84 by its 2.5′ grid (`egm2008`, 80.6 MB,
    public domain). Sampled (18 Oct): NAVD88 → EGM2008 −0.42 m at Washington, −0.71 m at Denver,
    −1.22 m at Seattle, −1.08 m at Anchorage; CGVD2013 → EGM2008 −0.01 to −0.05 m across Canada;
    JGD2011 → EGM2008 +0.13 m at Sapporo to +0.33 m at Fukuoka (+0.95 m on Okinawa). 3DEP's tiles
    off the conterminous US and Alaska (Hawaii, Puerto Rico, the islands) aren't on NAVD88 and no
    grid covers them: their heights stay as sampled, counted.
  - **Keys:** a converted dataset's line in a z6 tile's key names its chain's digest (the sources' ids and `keyed`, `Conversion::digest`):
    `<id> <keyed> to EGM2008 <digest>`, so declaring or editing a grid rebuilds just the z6 tiles
    meeting a dataset it converts.
- **The worldwide datasets**, ranked last (staged in `producers/elevation/`: `glo30.toml`,
  `terrarium.toml`): Copernicus GLO-30 (`rank = 1`, `code = 3`, a surface model, EGM2008; its index
  from the bucket's tile list with each tile's fill mask, `indexes.py glo30`: 26,450 1° tiles;
  `store = true`, kept in `sources/fetched/glo30/`, filled by `scenic-build store-fetch glo30 --index glo30-tiles.geojson`), then
  AWS's Terrarium tiles (`rank = 2`, `code = 4`, z12) where GLO-30 has none or a void. Terrarium is
  declared EGM2008, which most of its sources are; north of 60°N some are ellipsoidal (10–50 m off),
  but GLO-30 covers that land, so the tiles serve there only in its voids.
- **Keys:** a z6 tile names the `keyed` digests of the datasets whose coverage meets its ways (its
  `elevation` input, `agent::idents`), and the terrain tiles its roads' heights read
  (`agent::build::unit_deep_tiles`, by XXH3); a dataset's `keyed` includes its index's content and
  added files, so adding a tile reruns the z6 tiles it meets.
- **The switch:** the check records `sources/inputs/elevation/@switched` (the accepted index and the
  published app that recorded it) once a version of the declarations is accepted; a development
  build records none. Until then `elev` and the z6 tile job don't run and the z6 tiles wait (the agent
  says why). From then on no older app builds or is granted work
  (`regions::read::switch_app`, the coordinator's `min_app`).
- **The roads' heights** (crate::road_terrain, `elev`): a vertex's height is the finished
  terrain's there, bilinear between pixel centres at the finest zoom stored there, its native zoom
  (a deep tile, z13–16, where one is; else z12, or the latitude's cap), so a road and the ground
  under it come from one terrain and one dataset, already brought to EGM2008. The z6 tile job stages
  into the z6 tile's folder the terrain's tiles in one archive (`terrain.tiles`, crate::stage: z0–12
  over its box, and the deep tiles under its vertices, from the deep packs
  `layers/terrain/deep/10-x-y`: `road_terrain::deep_tiles`) and the
  datasets' codes by coverage (`codes.json`, `road_terrain::Codes`: a vertex's `src.u8` is the
  first dataset in the order whose measured coverage, in 1° cells, holds it: a label, so in a cell
  a dataset covers only in part it names that dataset where the terrain may have taken the next one
  through its voids). `elev` reads those and `verts.bin` alone (no
  NAS, no server). A z6 tile waits for the stale terrain targets under
  its box, the fine ones (z8) as well.
- **Densification** (crate::dem::spacing, `extract`): a way's points are put as far apart as the
  terrain's pixel in the 1° cell of the way's first node (the finest zoom stored there: its place's,
  crate::terrain_zoom, by the datasets' measured cells (`bands`, else `resolution`) and `cover`),
  never closer than 4 m (`FLOOR_M`); ferries not at all. A cell
  is the point's degrees floored: its west and south edges are its own. The z6 tile job writes the
  fine cells into the z6 tile's folder (`spacing.json`), which `extract` reads; the reach tests the
  z6 tiles' long ways at the same spacing (`reach::LongWay::touches`). The other steps' `extract` runs
  (the links, rail and candidates) read the same file (`Fine::of_records`); the rail and candidates keys name its whole digest, read by the planner from the accepted index (`spacing::digest_read`, cached by version; unreadable, they wait), the links are the pass's (made once a pass).
  - **Keys:** each z6 tile's reach records the cells its ways start in (`Reach::cells`); its key
    names those of them that are fine, with their zooms and the floor (`Fine::key`), so a dataset added, dropped or measured
    again rebuilds just the z6 tiles with a way starting where the spacing changes.
- NRCan's tile squares, the HRDEM index producer's input, are `producers/elevation/hrdem-squares.geojson`.
- **The 1 m datasets** (#594), staged in `producers/elevation/1m/` and in the inputs folder: NRCan's
  HRDEM 1 m mosaic (`hrdem-1m`: EPSG:3979, the 2 m mosaic's 56 squares, `indexes.py hrdem --res 1`)
  and USGS 3DEP's 1 m tiles (`usgs-3dep-1m-17`, `-18`, `-19`: one declaration a UTM zone, as a dataset
  is in one CRS; `indexes.py 3dep-1m --bbox -79.8,40.4,-66.8,47.5`), each with `regions` the ten
  eastern regions. Ranked before the 2 m and 10 m datasets (HRDEM 1 m −1, the 3DEP zones −2 to −4). Where zones overlap, their ranks decide (zone 19 before 18 before
  17): an index's files are newest first within its zone alone, not across zones: by survey (the
  end of each tile's lidar collection in USGS's work-unit metadata, WESM; then the newest published,
  task #960), not by publication alone (NY_FEMAR2_Central_2018_D19, published 2024 from a survey
  ending April 2019, had come before NY_3County_2019_A19, surveyed after it, across Niagara). HRDEM 1 m shares HRDEM 2 m's code and label (the same lidar, read
  alike); 3DEP 1 m takes code 8. Their heights reach EGM2008 by the existing
  chains (CGVD2013 by CGG2013a, NAVD88 by GEOID18).
- **REMA** (#601, the owner's approval of 25 Oct; staged in `producers/elevation/rema.toml`, not yet
  in the inputs folder): PGC's Reference Elevation Model of Antarctica, its 32 m mosaic v2.0, cloud-optimized
  GeoTIFFs in EPSG:3031 on AWS's open data (`pgc-opendata-dems`, CC-BY-4.0; PGC's acknowledgement
  and the mosaic's citation in its credit). Ranked after GLO-30 and the Terrarium tiles (`rank = 3`,
  `code = 9`, `roadcore::NDEM` raised to 10 for it), so it serves only in GLO-30's voids where the
  Terrarium tiles, which end at 85.05° S, have nothing: the polar caps (docs/polar.md §2). Heights
  above the WGS 84 ellipsoid, brought to EGM2008 by the `egm2008` grid. Its index is made by
  `indexes.py rema --south-of -87` (the mosaic's STAC items: each 100 km tile's `proj:bbox`): the
  tiles wholly south of 87° S, so its coverage (1° cells and their neighbours, to 86° S) meets no
  Web Mercator tile and changes no existing key; 20 tiles, 0.24 GB in the store
  (`sources/fetched/rema/`, `scenic-build store-fetch rema --index rema-tiles.geojson`). The mosaic
  has no data within about 150 km of the pole (no tile there); the caps take GLO-30's own filled
  values where nothing lies beneath (docs/polar.md §2). EPSG:3031 (and 3413) are projected by
  `roadcore::polar` (`dem::proj::Projection::Polar`).

**The 3D terrain** reads the same pool (crate::terrain_base; docs/steps.md §2, Terrain): a tile at
its place's finest zoom is sampled from these datasets in this order, blended as the roads' are, so
the roads and the terrain use one dataset at every point; the Terrarium tiles through the NAS's
archive of them (`sources/aws-terrarium/`, docs/macs.md). Each place's finest zoom is the finest of
the datasets meeting it there, by their cells (`bands`; crate::terrain_zoom), past z12 in the
terrain-deep step (a z10 tile a target: crate::terrain_deep); every dataset is read at most at its
own detail, the next in the order filling its gaps. The roads' heights are read from the finished
terrain (above).
- **Keys:** a terrain target names the datasets meeting it by their lines, as the roads' keys do
  (`agent::tiles::TerrainTiles::terrain_datasets`), and each place's sources there with the zoom each
  reaches (`terrain_zoom::key_lines`); a terrain-deep z10 tile likewise
  (`terrain_deep::target_key_lines`); while they can't be read
  its key is unknown and the terrain waits.

### 6.9 The remaining sources

**Kind:** source declarations. **Inputs folder:** `inputs/sources/`: `<id>.toml` each, and a source's
index (`<name>.geojson`) beside it. Code: crate::inputs::sources (the check, and `Sources`, the
accepted declarations by role, which the steps read). Built for the sources in the table marked
built (#145); the declarations themselves are kept in `producers/sources/`, with the producers of
their indexes.

**Shape:** a description (§5.1) and `[data]`:

| Field | Meaning |
|---|---|
| `role` | the slot the app reads it in, one source a role: `osm-planet` (the pass's planet: its `version` the planet's date, `params.md5` its MD5's URL), `land-cover`, `canopy`, `leaf-type-europe`, `leaf-type-north-america`, `wikidata`, `wikipedia`, `pageviews`, `lake-fill`, `buildings`, `building-heights`, `regions-sea`, `regions-internal-waters`, `regions-territorial-seas`, `regions-lakes`, `regions-land` (§6.1.7); and `depth` (§6.10) and `geoid` (§6.8, step 3), which any number of sources take |
| `format` | one of its role's: `osm-pbf`, `geotiff`, `geojson`, `image-service`, `zip-geotiff`, `zip-shapefile`, `sparql`, `mediawiki-api`, `pageviews-dump`, `geoparquet`, `depth-grid`, `geoid-grid`, `helmert` |
| `kind` | required: what fetching it is (`fetch::Kind`): `api` (a documented API: a query service, a WFS or WCS, MediaWiki's API, an ArcGIS REST service), `data` (a published download: a dump, an extract, a dataset's files) or `page` (a web page, an undocumented URL, links followed or scraped), refused: no step's fetcher reads robots.txt, so pages are a producer script's (`FETCHES = "page"`). Kept out of keys and the accepted index |
| `url` / `urls` / `index` | exactly one; a template's placeholders only its role's (`{tile}` for land cover, `{y}` `{m}` for the pageviews); an index is a GeoJSON beside it, a feature per file with its `url` and footprint |
| `version` | the pinned release |
| `refresh` | `pinned` (default: a new release is the owner's edit) or `live` (a query service asked each run) |
| `[data.params]` | the format's own fields (a legend, a box, a layer and the values kept) |

**Checks.** Errors: a file not of the shape (`src-shape`); a description field wrong (`src-desc`);
a declaration's fields wrong: an unknown role, a format not its role's, no `kind` or an unknown one, not exactly one way of
naming where it is, a placeholder its role doesn't fill, an unknown `refresh` (`src-decl`); an
unparsable index (`src-index`); an index named that isn't there (`src-missing`); two declarations
of one role (`src-role`; for `depth`, two of one `code`). Warning: an index no declaration names (`src-orphan`). Derived: each
description's extent for the credits (§4.8), the world or its territory's polygons' boxes in the
newest pass's outlines.

**robots.txt is for crawlers.** Only a page's host's robots.txt is read first, in a
request of its own, and obeyed (one that's missing, refused or unreadable allows everything). An
`api` or `data` source follows its provider's usage policy instead: the User-Agent
"scenic-roads/0.1 (personal offline map)" (`fetch::USER_AGENT`), the pool's rate limits (the
budget's proxy, `crate::budget`), a back-off on 429, and its credit. Every producer script that
fetches says what it fetches (`FETCHES = "api, data"`; a shell script's `# FETCHES: data`), and
one fetching pages reads robots.txt and tells its agent's proxy what it found (`/robots`, shown
in the build page's Activity tab): `fetch::tests::every_fetching_script_declares_its_kinds`.
The Rust fetchers (`crate::fetch`'s `Fetcher`, `crate::wikiask`) fetch only APIs and data.

**How the steps read them.** The planet by `crate::planet` (`declared`, `next`): the lead's plan
fetches the declared planet when it's newer than the last complete pass and the NAS lacks it
(`planet-fetch`, its job the lead's own, a fetch the pool's downloads cap counts and the NAS
budget weighs), then plans the pass of it (docs/steps.md §1); a declared planet older than the last
pass is said waiting (passes only go forward), and with none declared no new pass is planned. Rust steps through `inputs::sources::open`/`read` (one read per
accepted version); a unit's programs from `sources.json` in its folder (`unit::UnitSources`: the
land cover, the canopy files' URLs); a tree cover run from `sources.json` beside its coverage
(`trees::RunSources`, passed to the trees program as `--sources`, which makes its leaf-type
squares from them: `leaftype`). The jobs that read them (the z6 tiles, tree cover, items, the
heritage chain) wait until a version is accepted (the agent's `hold_for_sources`); the lake fill,
and the water, terrain and catalogue that read it, wait too (`lakefill::run::planned_source`); so do
the roadside buildings' job and the z6 tiles' keys (the release, `buildtiles::release_of`, held by
`agent::tiles` for the keys) and the 3D buildings' chain (no "bld-key" in `agent::theplan::input_digests`).

**Keys.** A key names a declared source only where it named the source in code before: the
pass's key is its planet's path, `sources/osm/<version>/planet.osm.pbf`, and its target the date,
so the declared version enters the pass's key, and through the pass's date every key after it; the
declaration's URLs and MD5 enter none, and its description's credit only the map's (§4.8), so
declaring the planet the build has (2026-09-28) is a map update and no other new key
(`planet::tests::the_staged_planet_is_the_one_the_build_has`,
`agent::tests::osm_pass_waits_for_space_or_runs`: the pass's job and key as before;
`agent::tiles::tests::declaring_the_planet_changes_no_key_but_the_catalogues`); the lake
fill's key names its zips' pin, and the declaration only once it names other than the code had
(`Source::key_extra` against `inputs::sources::declared_before::LAKE_FILL`, the staged declaration's
digest); the roadside buildings' (the z6 tiles', the buildings job's) and the 3D buildings' (bld-fetch,
bldprep) keys name the declared release, and the declarations only once they name other than the
code had (`bld::sources::declared`: `Source::key_part` against `declared_before::BUILDINGS`, and
`key_extra` against `BUILDING_HEIGHTS`), so while they're as staged every key is the one it was, and
the stores keep their names (`sources/buildings/<release>/`, `sources/overture/<release>/`,
`sources/ghsl/<version>/`, its version `R2023A`). The land cover, canopy and leaf types entered no
key in code; a z6 tile's key names the land-cover and canopy declarations, a tree cover z6 tile's the
canopy and leaf-type ones, only once they aren't as staged (`inputs::sources::unit_extra`,
`trees_extra`, against `declared_before::LAND_COVER`, `CANOPY`, `LEAF_EUROPE`,
`LEAF_NORTH_AMERICA`), so a changed legend or index builds them again rather than leaving them mixed
(`agent::build::tests::a_changed_land_cover_canopy_or_leaf_declaration_is_a_new_key`). The
Wikimedia sources are asked live. So nothing is built again for the move. Proofs: `landcover::tests::the_declared_source_is_what_the_code_read` (the same URLs and class
table), `trees::tests::the_declared_squares_are_where_the_code_read_them` (every indexed canopy
URL one the code asked for; the leaf sources, boxes and zip member the same),
`inputs::sources::tests::the_python_steps_sources` (the Python steps' URLs the same),
`lakefill::run::tests::the_declared_source_fills_as_the_code_did`.

| Source | Outcome | Declaration |
|---|---|---|
| OSM planet | built: `osm-planet.toml`, the `osm-planet` role, its `version` the planet's date (`crate::planet`); fetched once by the `planet-fetch` step into `sources/osm/<version>/planet.osm.pbf`, then read by the pass | `osm-pbf`: the mirrors' and the main site's URLs (`urls`, tried in turn) and its published MD5 (`params.md5`) |
| Planetiler and its data | kept in code: `water::PLANETILER_VERSION` is a code contract (the water layer's simplification was checked against that version's z14 water, `osmpass::check_planetiler`), and Natural Earth, the water polygons and lake centrelines are the jar's own downloads (`--download`), named by it, not by the app | — |
| ESA WorldCover | built: `esa-worldcover.toml`, the `land-cover` role | `geotiff`, URL template (`{tile}`), version; its class table is its legend (`params.legend`) |
| Meta & WRI canopy height | built: `meta-canopy.toml`, the `canopy` role, its index `canopy-squares.geojson` (`producers/sources/canopy.py`, from the bucket's listing: the equator row's `lat=-0.0` read there); a file the index doesn't list is none there | `geotiff` with an index: a file's `url`, `kind` and 10° square |
| Leaf type (EEA HRL, NALCMS) | built: `eea-leaf-type.toml`, `nalcms.toml`, the `leaf-type-europe` and `leaf-type-north-america` roles; the build still makes the squares where a run needs them (`trees::leaftype`, given the sources), since the coverage decides which | `image-service` and `zip-geotiff`: URL, the box each covers, NALCMS's zip member |
| Overture buildings | built: `overture-buildings.toml`, the `buildings` role (the release its `version`; `buildtiles` and `bld::fetch` read the bucket by role); `bld/fill.rs`'s `ESTIMATES` stays in code, the fill's rule 2 (its tables were fitted with it) | `geoparquet`: the bucket, release, its `s3://` name (the roadside scan's parts list names files by it) and region, the types read |
| GHSL building heights | built: `ghsl-built-h.toml`, the `building-heights` role, its index `ghsl-tiles.geojson` (`producers/sources/ghsl.py`, from the tiles folder's listing: the tiles' grid read there) | `zip-geotiff` by an index of its tiles, its version naming its folder |
| Wikidata, Wikipedia's API | built: `wikidata-qlever.toml`, `wikipedia-api.toml`, the `wikidata` and `wikipedia` roles | endpoints (`sparql`, `mediawiki-api`, `refresh = "live"`) |
| Wikipedia pageviews | built: `wikipedia-pageviews.toml`, the `pageviews` role | URL template (`{y}`, `{m}`); the months stay the items job's rule |
| Geoid grids | staged, not yet added: `producers/geoid/declarations/` (the `geoid` role, any number; the grids by `producers/geoid/fetch.sh`), read by the elevation step (§6.8, step 3) | `geoid-grid` (a URL, its two references, `store`) and `helmert` (its parameters), pinned |
| The regions' datasets (Marine Regions EEZ v12, Internal Waters v4, Territorial Seas v4; Natural Earth lakes) | staged, not yet added: `producers/sources/` (§6.1.7); the files fetched into `sources/fetched/marineregions-eez-v12/`, `marineregions-internal-waters-v4/`, `marineregions-territorial-seas-v4/`, `natural-earth-lakes-10m/` on 2026-10-18 (the coastline's land is the pass's own: §6.1.7) | `geojson` and `zip-shapefile`, their file (`params.path`), id field and layer |
| Canada's lake fill (CanVec) | built: `canvec-lakes.toml`, the `lake-fill` role; its zips kept by hand in `sources/fetched/canvec-lakes/` | `zip-shapefile`: where they come from, its layer, field, values kept, the intermittent ones and the confirming share (`params`) |

Also kept: fonts (`scripts/fonts.sh`, the app's own assets). The depth's sources are §6.10's, NCEI's
grids on the NAS (`sources/ncei-crm/`, `sources/ncei-greatlakes/`) among them.

**The rollout:**
1. Both Macs run the app that reads the declarations. Until a version is accepted, the z6 tiles, tree
   cover, items, heritage chain, lake fill (and the water, terrain and catalogue that read it), the
   roadside buildings' job and the 3D buildings wait, and say so.
2. CanVec's zips are copied with their times kept (`cp -p`) from `sources/canvec/50k/` to
   `sources/fetched/canvec-lakes/`, and each copy's modification time checked to match its
   original's to the second (`stat`): over SMB a time not kept changes the fill's pin, and its key,
   and the fill is made again. A declared lake-fill source whose folder isn't there holds the fill
   and what reads it (`lakefill::run::planned_source`), so the copy comes before the update.
3. The files of `producers/sources/` (the `.toml` declarations, `canopy-squares.geojson` and
   `ghsl-tiles.geojson`) are added to `inputs/sources/`. Once accepted, everything goes on under
   the keys it had.
4. `sources/canvec/50k/` is deleted only after both Macs run the new app (an older app still reads
   it).
5. The planet (#1120): once both Macs run the app that reads its declaration, `osm-planet.toml` is
   added to `inputs/sources/` (until it's accepted no new pass is planned, and the plan says so;
   accepted, its planet is on the NAS and its pass complete: no fetch, no pass, a map update for
   its credit), and DSM's daily task that ran the NAS's own fetch is deleted, with what it left on the NAS
   (`nas/`, `state/logs/fetch-planet.log`).

### 6.10 Depth

**Kind:** source declarations of the `depth` role, any number (crate::inputs::sources `MANY`), in
`inputs/sources/` beside the others, each with its index of depth grids, or made by the pool
(`format = "lake-estimate"`, below). Code: crate::depth::grids (the build's reading), crate::depth
(the steps, docs/steps.md §3). The declarations are staged in `producers/depth/declarations/`; the
producers that make the indexed grids in `producers/depth/`.

**The standard: depth grids** (`format = "depth-grid"`). A source is a set of GeoTIFFs, each on the
global Web Mercator pixel grid of one zoom (EPSG:3857; its pixels `roadcore::geo::MERC_C` (2π × 6,378,137 m) / 256 / 2^zoom m,
their edges on that grid, its own edges on its zoom's tile edges where its data allows), so the build
reads them by pixel arithmetic, never reprojecting; every cell shows at its grid's zoom:
- `file`: float32 depth below the water's surface in metres, negative (the sea's mean level; a lake's
  mean surface), drying heights positive, NaN where there's none; tiled, Deflate with the
  floating-point predictor, overviews by averaging;
- `low_file` (optional): float32, the chart's low water (MLLW, LAT, chart datum) below MSL, m ≥ 0, for
  the "dries at low tide" flag.

Its index (`<name>.geojson`, a feature per grid): its footprint, `url` (where the data came from),
`file`, `zoom`, `low_file`, `hash`, and where it's packed: `pack` and `at`. The files
are kept in `sources/fetched/<id>/` (the producer writes them; they're copied there by tar over SSH).

**Packs** (task #452; producers/depth/packs.py): a source's grids are kept in packs, not a file a
grid (the NAS seeks between small files: tens of thousands of grids made it seek-bound). A pack is
a plain tar (GNU format, its headers' times, owners and modes fixed, so the same grids make the same
bytes) of whole GeoTIFFs, holding a source's grids of one z6 tile (the tile of each footprint's
centre; a tile can have several packs, one per producer run or chunk), named
`<6-x-y>.<hash16>.tar` (SHA-256 of its bytes). A packed grid's feature names its `pack` and its
files' byte ranges in it and their hashes as packed, `at`: `{"file": [offset, length, hash16],
"low_file": […]}` (SHA-256's first 16 digits; a range alone is read too). The
build opens a pack once a job and reads each GeoTIFF in place by its range (crate::depth::grids), so
its reading and output are a loose file's; `pack` and `at` are left out of the source's digest in the
depth's keys (`grids::unpacked_digest`), so packing builds nothing again; so is `zoom_file`, a
property the build doesn't read (`grids::UNREAD_PROPS`). So a source's pin names
its grids' depth files by `hash` alone, packed or loose: a low-water file changed with its
depth file unchanged changes no key (the loose index never named their contents, so naming the
packed hashes would rebuild every depth target at adoption). A producer that changes them changes
the depth file's `hash` too, or a new key is forced by the declaration's `version`. A feature without `pack`
is a loose file, read as before. The producers pack as they write the index (`grid.Writer`).

**Made by the pool** (`format = "lake-estimate"`, task #959; crate::depth::lakes): a source whose
grids the lake-depth step makes (docs/steps.md §3, The lakes' estimates) for every z6 tile near the
coverage, from what its `params.method` says: `globathy`, GLOBathy's maximum depths on HydroLAKES
v1.0's outlines, from the zips kept in its folder `sources/fetched/<id>/` (GLOBathy's basic
parameters, the one holding `(ALL_LAKES).csv`, and `HydroLAKES_polys_v10_shp.zip`); `land-slope`,
the basemap's small lakes from the land's slope, leaving out the lakes under the HydroLAKES
polygons of the source `params.outlines` names. It has no index, and names where its files came
from by `url` or nothing (made from the map's own inputs). Its grids are that step's packs
(`work/lake-depth/<id>/6-x-y`, docs/formats.md), in this standard. The keys name its declaration
(its `version` pins the zips: a new copy comes with a new version) and each pack read by content.

**`[data.params]`:** `code` (1–15, the source byte the class tiles carry, one a source), `water`
(`sea` or `lake`: where it's drawn: a sea source on the sea, a lake source in lakes),
`label` (the map's name for it), `datum` (what its depths are below, in words), optionally `band_m`
(ground metres it's feathered in over where it ends in the water over another source's data; with
none beneath, its edge is its own), `intertidal` (its positive values are drying heights), `datum_exact`
(false: not corrected to the others' datum, so it fills gaps only, hard-edged), `rivers` (drawn in the basemap's rivers too: its depths there are below the river's own
surface, not a fixed level's), `lakes` (drawn in the basemap's lakes too: its depths there are
below the lake's own surface), `estimated` (every cell an estimate:
classed so, painted after every survey and filling only what none has) and `notice` (a
notice its licence asks every map showing its data to carry: shown in full in the map's © Credits,
under a "Depth:" line naming every source's `label` and "Not for navigation."; nothing floats over
the map).

**Checks** (§6.9's, and): a depth declaration's params (`src-decl`: a `lake-estimate` one with no
index, a known `method`, and `outlines` for `land-slope`; those two a `lake-estimate` one's alone);
its index's features each naming a `.tif` `file` and a `zoom` 0–18, and a packed one a `.tar`
`pack` in the source's folder and a range in `at` for each file it names (`src-index`); two of one
`code` (`src-role`).

**The sources** (`producers/depth/declarations/`; the owner's decisions of 6 and 17 October 2026):

| id | what | code | water | producer | detail |
|---|---|---|---|---|---|
| `gebco-2026` | the yearly global 15″ grid and its type identifiers | 1 | sea | `gebco.py`, from the owner's two zips in `sources/gebco/2026/` | z8 |
| `bluetopo` | NOAA's US coastal grids, 4–16 m | 2 | sea | `bluetopo.py`'s steps, from the public bucket by its tile scheme (checksums checked), a chunk of tiles at a time (`chunked.py bluetopo`); NAVD88 → MSL and MLLW by NOAA VDatum (`vdatum.py`, a point per 0.25° cell; `stations.py noaa` gives the same table from the tide stations) | z15 (4 m) to z13 (16 m) |
| `nonna-100`, `nonna-10` | CHS's 100 m and 10 m grids (licence accepted; never redistributed; its notice names the owner as the maker) | 3, 4 | sea | `nonna.py`, the keyless coverage service (NONNA-10 a chunk of blocks at a time, where NONNA-100 has water: `chunked.py nonna10`); chart datum → mean water level by CHS's stations (`stations.py chs`) | z10, z13 |
| `emodnet-dtm` | EMODnet's DTM 2024, MSL version | 5 | sea | `emodnet.py`: its MSL ASCII tiles one at a time (a WGS 84 `.prj` given each), `raster.py`'s produce | gaps only (`datum_exact = false`) |
| `shom-homonim-100`, `shom-homonim-20` | SHOM's façade and coastal DEMs, MSL versions | 6, 7 | sea | `raster.py`, the low water SHOM's own separation (`--low-raster`) | z10, z13 |
| `gblq` | surveyed lakes (Québec) | 11 | lake | `raster.py` (metres, positive down) | z16 |
| `ncei-crm` | NCEI's Coastal Relief Models, 1″ (the owner's download, `sources/ncei-crm/`; volume 6 absent) | 12 | sea | `ncei.py crm`; volumes 1–5, 7, 8 (EGM2008) → MSL by a datum surface a volume from `ncei.py egm2008`'s table (CO-OPS' stations' MSL − NAVD88, `stations.py noaa`, plus NAVD88 → EGM2008 by the declared geoid grids through `scenic-build geoid-offsets`; a station more than 2 km from the volume's sea left out), land cut by MSL; 9 and 10 on local MSL as they come | z12 |
| `ncei-greatlakes` | NCEI's Great Lakes bathymetry, 3″ (the owner's download, `sources/ncei-greatlakes/`) | 13 | lake | `ncei.py greatlakes`: from each lake's low water datum (CO-OPS' station datums) to its long-term mean level (GLERL's coordinated lake-wide monthly means, 1918 on), both fetched as it runs | z10 |
| `globathy` | GLOBathy's maximum depths (CC0) on HydroLAKES v1.0's outlines (CC BY 4.0), every lake of 10 ha and more: estimates | 14 | lake | the lake-depth step (`lake-estimate`, `method = "globathy"`), from GLOBathy's and HydroLAKES' zips in `sources/fetched/globathy/` | z11; `estimated`, gaps only (`datum_exact = false`): a survey wins |
| `smalllakes` | the basemap's (OpenStreetMap's, ODbL) lakes and ponds under 10 ha that HydroLAKES lacks, each one's maximum depth estimated from the land's slope around it: estimates | 15 | lake | the lake-depth step (`lake-estimate`, `method = "land-slope"`, `outlines = "globathy"`) | z14; `estimated`, gaps only (`datum_exact = false`): a survey and GLOBathy win |

`rivers = true` is staged for `nonna-100`, `nonna-10` (mean water level by CHS's stations, which
follow the St. Lawrence up to Montréal: NONNA-100 has values there), `bluetopo` (local mean sea
level by VDatum, which covers only the tidal reaches) and the SHOM grids (mean sea level, in the
estuaries' tidal reaches). The others' depths inland are heights on a fixed level (GEBCO, CRM,
EMODnet) or a lake's own, so they stay out of rivers. `lakes = true` is staged for `nonna-100` and
`nonna-10` alone: their stations' mean water level is the lake's own surface where CHS charts a
lake (the St. Lawrence's fluvial lakes, Lac Saint-Louis and Lac Saint-Pierre among them; the Great
Lakes, its level within a metre of GLERL's means), so a survey there wins over GLOBathy's
estimated bowl. BlueTopo's and SHOM's mean sea level isn't a lake's surface (their topo-bathymetry
gives a coastal pond's bed as a height above the sea), so they stay out of lakes.

A chunked run (`chunked.py`) fetches a chunk, produces it, packs it, tars its packs over SSH into the NAS's
`sources/fetched/<id>/`, compares each file's size there and deletes the chunk here; each chunk's
index part is kept, so the same command resumes. A dropped or refused connection, a timeout, 429 or a
5xx is tried again (5 tries); any other failure stops the run before the chunk's part is written (a
block with no data comes as a TIFF of nodata, making no grid). Free disk is logged a chunk;
`--count` estimates a chunk's bytes from files already in its cache. Each chunk's datum surfaces are
its own. A datum surface (`datums.py`) is solved between the stations over the water bodies holding
one (a station counts within 25 km of its water); a body with no station within 25 km of a solved
cell gets nodata, and over 5% of the water so, or one body over 2%, stops the run. NONNA-10's run
alone (BlueTopo, `raster.py` and the VDatum surfaces keep the rule above) extrapolates open sea: NONNA
charts the sea only, so every such body, of any size or place (sparse Arctic survey tracks leave
thousands of tiny disconnected ones), is sea when it lies on none of Natural Earth's
10 m lakes (`--lakes`: `sources/basemap/natural_earth_vector.sqlite.zip`, unzipped; Great Slave,
Great Bear, Winnipeg and the Great Lakes among them), and takes the nearest solved cell's value, or with
no station within 25 km of any water the nearest station's, however far (the log gives the cells and
the farthest km); the stop counts the water still without a value. Extrapolated cells carry no flag
in the output (one band). (`producers/depth/test_chunked.py`: `uv run --project .. python -m unittest test_chunked`.)

On macOS the producers trust the system's root certificates as well as OpenSSL's (`grid.urlopen`,
read from its keychain): uv's Python lacks some roots the hosts use (CHS's station service's chain ends at Sectigo's
Public Server Authentication Root R46). `SSL_CERT_FILE`, when set, is used alone.

Every request carries the User-Agent "scenic-roads/0.1 (personal offline map)"; what they fetch
is APIs and published data, so no robots.txt is read (§6.9).

**Keys** (docs/steps.md §12): a depth target names each source meeting it by its id and
`Source::digest` (its `[data]` and its index's features, their hashes among them), so a new grid
re-keys the tiles it meets; a `lake-estimate` source by its declaration's digest and the
lake-depth packs it reads by content, so a coverage change re-keys only the targets whose packs
changed.

**The rollout:** docs/steps.md, Depth.

### 6.11 Source extents

Where each known data source really has data, and how good it is there: the input of the map's
"data available" layers (docs/steps.md §14), which colour every place by the best data any source
has for it. The map's regions play no part in it, and no source is assumed to cover the planet: the
worldwide models are sources like any other, and a place no declared source covers has no data.
One format serves three **themes**, each with its own metrics and score: `terrain` (built here),
and `bathymetry` and `buildings` (planned, when their research is done).

**Status.** Built: the producer, its tests, the declarations and the pieces of the sources in the
table at this section's end (`producers/extents/`), and the unit `extents` that checks them
(crate::inputs::extents, the checks below; task #1346); North America's and Europe's sources
(task #1350); the step `available` that draws the pieces' tiles, and the server's score, layers
and click (docs/steps.md §14; task #1347); the app's layers (task #1348); the surface models of
North America and Europe (task #1370). Planned: Asia's and the
rest of the world's sources (task #1350's later runs) and `piece.dataset` (extents from the build's
own data; task #1349). Task #1334.

**Kind:** source declarations whose data is an extent, not the source's data. **Inputs folder:**
`inputs/extents/`, a folder per theme:

```
inputs/extents/
  terrain/
    score.toml          the theme's score: its metrics, their scales and weights, and how tied pieces rank (below)
    <source>.toml       a source: its description, availability, and how each piece's extent is found
    <source>.geojson    its pieces, made by producers/extents/extents.py from the declaration
```

**A piece** is a source's extent with one set of metrics: the area where the source's data has the
same metrics (for the terrain: one grid spacing, one kind, one survey). A source has as many as its
data has (a national lidar programme: one per project; a worldwide model: one). The `.geojson` is a
FeatureCollection, a feature per piece (RFC 7946: WGS 84, exteriors counter-clockwise and holes
clockwise, cut at the antimeridian, a polygon holding a pole closed along its 90° parallel), its
geometry a MultiPolygon, or `null` for a piece whose extent isn't known (drawn nowhere, so on no
click; named in the sources' list). Its properties:

| Property | Meaning |
|---|---|
| `piece` | its id within the source: the declaration's piece name, then the values it was grouped by |
| `name` | what the publisher calls it (a project's name), where it has one |
| `extent` | how its extent was found, the **extent class** (below) |
| `availability` | `open` (downloads with no further step), `login` (free, after a login or a request), `paid`, `closed` (exists, can't be had) |
| the theme's metrics | the metrics its `score.toml` names; terrain: `spacing_m` (the grid's spacing in metres; a point cloud with no grid: 1/√density, its nominal point spacing), `surface` (`ground`: bare earth, a DTM; `surface`: with trees and buildings, a DSM; the elevation declarations' words, §6.8), `year` (the survey's last year), `vertical_rmse_m` (the vertical accuracy the publisher states, as an RMSE in metres). A metric the source doesn't state is absent, never guessed |
| `stated` | where a metric's value is the publisher's class and not a number (a quality level), the class as published |

The collection's own members: `source` (the declaration's id), `theme`, and `read`: what the
producer read to make it, per piece block: the method, the address, the requests, bytes and
features read, the date the newest of those answers was fetched on (a cached answer's: when it was
fetched, so a run again from the cache writes the same bytes), the simplification's
`tolerance_m`, and where the extent itself is no finer than a cell, its `grain_m`: a listing's tile
side, a raster's mask pixel. So every extent says what it stands on and how fine it is.

**Extent classes**, best first; a declaration takes the best its source allows, and a piece
records which it got:

| Class | The extent is | For |
|---|---|---|
| `mask` | where the build's own copy of the data has valid values | data the build has downloaded (§6.8's datasets): every built-in piece, and a source's piece naming the dataset that holds its files (`dataset`), there (planned: docs/steps.md §14) |
| `files` | the files' own bounds and valid-data masks, read from their headers and coarsest overviews by range request | a source that downloads directly, whose files are few enough to open each |
| `index` | the publisher's list of its files: a tile index, STAC items, a directory listing of georeferenced names | a source that downloads directly, with too many files to open |
| `service` | the publisher's footprint service: project or survey outlines | a source that downloads directly, with no per-file index |
| `claimed` | the coverage the publisher states (an administrative area, a map on a page), not checked against data | a source that doesn't download directly (`login`, `paid`, `closed`), or publishes no footprint |
| `unknown` | not known: no geometry | a source whose extent couldn't be had; listed, not drawn |

The owner's order (12 Nov) is mask; then the files' own headers and overviews, or the
publisher's per-file index (`files` and `index`, `files` first as it's the data's own); then a
footprint service; then a claim. Never a whole download to learn an extent, unless the source is
small.

**What is declared** (the owner, 13 Nov 2026): every elevation product is declared, surface models
included (image-matched DSMs, lidar DSMs, point clouds), each with its own spacing and its
`surface` metric stated truthfully (a point cloud, holding both, states none); none is left
out because it would outrank a ground model: the finest grid wins, whatever it models. **What is
claimed:** a whole country or region only where the publisher's product covers it or states that
area; otherwise the piece is `none`, listed and not drawn.

**A declaration** is a description (§5.1: `name`, `credit`, `licence`, `licence_url`, `source`,
`notes`) and:

| Field | Meaning |
|---|---|
| `theme` | `terrain` |
| `availability` | the source's, as above; a piece block may say its own |
| `[[piece]]` | one or more blocks, each a way of finding extents and the metrics of what it finds |
| `piece.id` | the block's name, the first part of its pieces' ids |
| `piece.method` | the producer's generic reader (below) |
| `piece.class` | the extent class its extents are: `index` or `service` for a reader that can be either (`arcgis`, `wfs`, `file`), or `claimed` where the outline read is a stated coverage and not the data's (an administrative area's); fixed by the method otherwise. A piece that doesn't download directly (its availability not `open`) is `claimed` whatever lists it |
| `piece.url` and the method's own fields | where to read: a layer, a collection, a file, a listing |
| `piece.where`, `piece.filter`, `piece.exclude` | a filter in an ArcGIS service's own language; `{field = [values]}`, applied to what any reader read: `filter` keeps the features whose field is one of the values, `exclude` leaves them out (`""` for a field empty or missing) |
| `piece.latest` | `{field, pattern, order}`: of the features naming one tile (the match of `pattern` in `field`), only the one greatest in `order` is read, where a collection keeps each tile's earlier versions beside its newest (swissALTI3D's); a feature naming none is kept |
| `piece.by` | fields a piece is told apart by: features sharing their values, and every metric's reading, are dissolved into one piece (none: a piece per distinct reading of the metrics) |
| `piece.name`, `piece.label` | the field a piece's `name` is read from (one of `by`, so the features of a piece share it), or the name itself |
| `piece.tolerance_m` | the simplification, default 50 |
| `piece.timeout_s` | a request's time limit, default 180: a server that makes its answer on request and takes longer (Bulgaria's WCS, 2 to 5 minutes) |
| `[piece.metrics]` | each metric a constant (`surface = "ground"`), or read from a field: `{field = "…"}`, with `scale` (a factor to the metric's unit), `map` (the publisher's classes to values, the class kept as `stated`), `index` (the place in a list-valued field), `text` (a number written with its unit, `"2 metre"`, `"10 pts/m2"`, `"0.35m (spec)"`: the unit's word, a remark in brackets after it left out, the value unknown and kept as `stated` when another unit is written), `date` (how a date is written: `epoch-ms`, `iso`, `year`, `text`, the last year a name holds, alone or opening a compact date, `20240315`, or ending a span in two digits, `2011-12`, not a date's month, or `yymmdd`, six-digit dates of the 2000s alone or as a span, the last taken; the year is taken), `density = true` (a point density per m², taken as its nominal spacing 1/√density, the Lidar Base Specification's NPS); or a list of readings, tried in turn, the first giving a value (`[{field = "id", date = "text"}, {field = "datetime", date = "iso"}]`: a field the publisher fills only sometimes, then another), its last one may be a constant, the value where none gives one (PST-A's 1 m grid's tiles stating no cell: `[{field = "risoluzion"}, 1.0]`). Features of one value but different classes (QL 1 and QL 2 both 0.10 m) are pieces apart, each stating its own |
| `piece.dataset` | planned (docs/steps.md §14): the elevation dataset (§6.8) holding this piece's files, whose mask the step takes for the piece's extent where it's accepted |

**The producer's readers** (`producers/extents/extents.py`, one generic reader a kind of service;
a new source of a kind it knows is a new declaration, no code):

| `method` | Reads | Class |
|---|---|---|
| `arcgis` | an ArcGIS REST layer's features (FeatureServer or MapServer `query`) or an ImageServer's catalogue (its items, a file each), paged by the layer's object id (each page the ids after the last one's, in their order: no deep offsets, which a catalogue of 100,000 items times out on), the pages coming to the count the service gives first (`returnCountOnly`), so none is skipped or read twice at a page's edge: a layer whose rows share an id (a join), on a page or across its edge, is refused, in WGS 84, generalized by the service to a quarter of the piece's tolerance; as GeoJSON where the layer offers it (a position's z and m dropped), as Esri JSON otherwise (an ImageServer offers only that; its rings taken by the even-odd rule) | `index` or `service` |
| `stac` | a STAC API or OGC API Features collection's items (paged by its `next` links up to the first page with no items, a link without the page size asked for given it back, a link back to a page already read stopping the run), their footprints and properties, or with `geometry` the GeoJSON file of each item's asset of that name (a finer outline than the item's own, where the publisher gives one) | `index` |
| `atom` | an ATOM download feed (an INSPIRE download service's; `follow = true` reads the feeds its entries link too, each once, by their `alternate` links: never `up` or `self`, back to the service's every dataset): each file it links with its own `bbox` (`latfirst = true` where a feed writes it latitude first), else each entry's GeoRSS polygon or box; a file's properties its entry's `title`, `updated` and `id` and its own `href`, `title` and `length` | `index` |
| `wfs` | a WFS feature type (GetFeature, paged by `startIndex`, every feature counted, those without geometry too; as GeoJSON, or GML where the server answers only that, the `format` asked for; `version` 2.0.0 or a 1.x's; `latfirst = true` where the answers put latitude first, which servers naming one CRS do differently) | `index` or `service` |
| `file` | a published index file fetched whole, its size checked first: GeoJSON or GML (read as `wfs`'s answers are; a position's z and m dropped), KML (its Placemarks' polygons), a GeoPackage (its features table, `layer` naming one of several; its CRS from the package's own table), or a zip holding a shapefile (its text in `encoding`, else its .cpg's, else UTF-8) or one of those (a KMZ; `member`, a part of its name, picking one of several) | `index` or `service` |
| `listing` | a bucket's listing, a page's links (`href` in any case), a text file of names, a zip's member names (its central directory by range request, none of its members read) or a spreadsheet's rows (xlsx, cells joined by `;`, a date-formatted cell as its ISO date; `sheet`) (`list`), of `url` or of each of `urls` in turn, each name georeferenced by a declared `pattern`: its groups `x`, `y`, and `ew`, `ns` (`unit`, `size`, `corner`, `crs`); or `ref`, a national grid's lettered reference (`grid`: `osgb`, `irish`; NS16NE, NG2004: its square and side read from it); or a nested sheet nomenclature's digits (`digits`, each a group's steps in x and y from `origin`, `cols` for a quadrant's digit). A tile listed twice is one; one tile's files of different readings (a survey and its successor) are two | `index` |
| `raster` | each file's header and an overview by range request (`urls`, a listing's names as `listing` reads them that `pattern` matches, or a STAC collection's items' `asset`): its bounds, its cell (the reading `res_m`) and where it has data, at the coarsest overview or, with `mask_m`, the coarsest whose pixels are no wider (the finest it has where none is that fine, never the full resolution: a file with no overviews stops the run, its mask a whole download, for `pattern` to leave out, unless its own cells are no finer than `mask_m`); the overview's pixel is the piece's `grain_m`. A coverage service's answer at a coarse scale (a WCS GetCoverage with a small `SCALEFACTOR`, its spacing then declared as a constant) is such a file, read whole where the server ignores ranges | `files` |
| `none` | nothing: a piece with no extent, and `why` | `unknown` |

Each reads through one fetcher: the app's User-Agent, one request at a time with a pause, TLS
verified against Mozilla's trust store (certifi's: the system's file lacks roots some publishers chain
to, Quebec's; a server sending its certificate without its intermediate, the CNIG's and the IDEE's,
has its chain completed once a run from the certificate's AIA, only where its issuer wasn't found:
the issuers fetched, up to three, over plain http to a public address only, up to three redirects
followed (D-Trust's AIA answers a 302 to a host of its own), each held to the same rules,
64 KiB in 30 s at most, a root never loaded, are added with partial chains refused, so each is a
link to one of the store's roots, never trusted by itself, the host's name checked as ever; a
certificate refused otherwise, expired or misnamed, is refused at once, not retried; revocation
isn't checked), the provider's 429s and 5xx retried (four tries, waiting longer each time), a
request's time limit (180 s; a block's `timeout_s`, for a server slower to make its answer), an
answer kept in the `--cache` only once what read it went on (one the run stopped on, an error page
or a service's error sent as a 200, is fetched afresh next run), and a limit on what one source's reads come to (`--max-gb`, 5,
the owner's of 13 Nov; cached answers count: a source over it stops the run, never a silent bulk
download). Headers, indexes and overviews come first; a source's data is read whole only where
that's the only way to its extent and the source is small. No fetch waits for a go-ahead; a run
reading a large index goes to the Mac mini (tools/dev/agent-rules.md). It
then makes each piece: its features repaired where invalid (counted in `read`), each generalized to
a quarter of the tolerance (as `arcgis` asks its service to: RGE ALTI's 290,932 source polygons of
some 120 million positions took hours to dissolve unsimplified), dissolved in their
own CRS (a tile index's squares become their outline), brought to longitude and latitude with
their edges densified, cut at the antimeridian and closed along 90° round a pole
(`extents.ring_region`), gaps narrower than half the tolerance closed (a quarter's buffer out and
back: the service's generalization of neighbouring features apart leaves no wider ones; a narrow
gap of the publisher's own that the generalization widens in places can close only at its ends,
leaving a hole: two, of 0.4 km² in all, in the BEV's Styrian flight areas of 2010-11),
simplified to the tolerance with the topology kept, holes and specks smaller than the tolerance
squared dropped, coordinates to 5 decimals (about a metre). So an outline is within one and three quarters
tolerances of its features' (the tolerance taken in degrees of latitude: east-west it's tighter
by the cosine of the latitude), plus the grain where there is one. British Columbia's 98,964 map
sheets become 17 outlines, 246 KB. Tests: `producers/extents/test_extents.py`.

**The score** is declared in the theme's `score.toml`, the one place its formula's numbers live
(the server reads it, `Score::of`, as `score.py` does; docs/steps.md §14): its `primary` metric, the
factor `within`, the others' `share`, and a table per metric with its `label`, `unit`, scale and
(but the primary, which takes none) `weight`, the metric's value from 0 at `worst` to 1 at `best`
(`scale = "log"`: by the logarithm; `"linear"`), clamped, or a `values` table instead, a word's
value from 0 to 1. The primary's scale is `log` (the guarantee below is a factor). The primary metric decides,
the others only break near-ties (the owner, 13 Nov: for the terrain, a finer grid always wins,
whatever its kind or age):

    score = (1 − share) × value(primary) + share × rest
    rest  = Σ weight × value / Σ weight, over the other metrics the piece states

A metric a piece doesn't state is unknown: it leaves both of the rest's sums, so its weight goes
to the piece's other metrics in proportion; it's never a zero. A piece stating none of the others
has its primary's value for the rest (they move it neither way); a piece not stating the primary
has no score. Availability is not a term of the score: it ranks tied pieces (below), and the
layers' switch filters by it. A metric of weight 0 has its layer and its line on a click and takes
no part in the score.

**The guarantee:** a piece whose primary is better than another's by more than the factor
`within` scores higher, whatever either states of the rest (both between the scale's `best` and
`worst`; outside it the values clamp). It holds when (1 − share) × k > share, k = ln(within) ÷
ln(worst ÷ best), the primary's least step that counts: `score.py` refuses a `score.toml` where it
doesn't, and `test_extents.py` tries it on 20,000 random pairs of pieces and at its edge. The
terrain's, as proposed (the owner's to tune):

| | Value |
|---|---|
| `primary` | `spacing_m`, log: 1000 m → 0, 0.1 m → 1 (every terrain source's spacing inside it) |
| `within` | 1.25: spacings closer than a quarter are one grid in different units or products (1 m and 3 ft, 0.9144 m; 0.5 m and 2 ft, 0.6096 m; 10 m and 1/3 arc-second, 10.3 m), where the rest should decide |
| `share` | 0.02 (the bound for these numbers is 0.0237) |
| `surface` | weight 0.4: `ground` 1, `surface` 0 |
| `vertical_rmse_m` | weight 0.4, log: 10 m → 0, 0.05 m → 1 |
| `year` | weight 0.2, linear: 2000 → 0, 2026 → 1 |

What it does (`producers/extents/score.py <score.toml> --examples` prints them):

| Pieces | Scores | Best |
|---|---|---|
| a 2 m surface model; a 5 m bare-earth one (both 2020, accuracy not stated) | 0.666; 0.582 | the 2 m surface model: finer |
| a 1.2 m surface model; a 1.25 m bare-earth one (both 2020) | 0.721; 0.730 | the bare-earth one: within the factor, the kind decides |
| a 3 ft (0.9144 m) surface model of 2010; a 1 m bare-earth one of 2024 | 0.747; 0.754 | the 1 m bare earth: within the factor |
| 1 m bare earth of 2005; 2 m bare earth of 2024 (accuracy not stated) | 0.750; 0.681 | the old fine one |
| the same, 0.30 m and 0.10 m RMSE stated | 0.749; 0.680 | the old fine one |
| 1 m bare earth of 2005; 5 m bare earth of 2024 | 0.750; 0.583 | the old fine one |
| a 30 m surface model of 2013 at 2 m RMSE; a 10 m bare-earth one, nothing else stated | 0.378; 0.510 | the 10 m one |
| 1 m bare earth of 2024 stating a 0.5 m RMSE; the same stating none | 0.751; 0.754 | the one stating none, narrowly |

The last row is what redistributing an unknown metric's weight does: the rest is scored as if what
a piece doesn't state were as good as what it does, so stating a middling accuracy can rank a
piece just below the same piece silent about it; it never crosses a spacing step larger than the
factor.

**The best piece at a place** (`Score::best`, `score.py`'s `best`; the layers' piece and the
click's order, docs/steps.md §14): of the pieces there, those whose primary ties the finest's
(within the factor `within` of it); of them, those of the best availability in the order `score.toml`'s
`availability` lists (each of `open`, `login`, `paid`, `closed` once, best first); of those, the
best score; of equals, the first by id. So a grid finer by more than `within` always wins, whatever
its availability, and among tied grids an open source wins before the score's tie-break (the
owner's rule, 13 Nov: in Berlin the BKG's paid 1 m DGM of 2022 scores above Brandenburg and
Berlin's open 1 m of 2021, which is drawn). The order: the best, then the best of the rest, and so
on. A `score.toml` without `availability` ranks by the score alone. The terrain's: `availability =
["open", "login", "paid", "closed"]`. Its `label` names the score's layer in the app ("Best data";
none: the server's, "Best data"): the best piece drawn at its own primary value, the grid spacing,
which so has no layer of its own beside it. The layers' colours are the app's (its colour maps,
docs/steps.md §14); a `score.toml` still holding the fields that drew them as classes (`ramp`,
`shade`, a metric's `classes` and `colours`) is read as if it didn't, and the check warns.

**Checks** (the unit `extents`, crate::inputs::extents; its themes the app's list, `terrain` so
far). Errors:
- a file not of the shape (`<theme>/<source>.toml`, `<theme>/<source>.geojson`,
  `<theme>/score.toml`, ids lower-case letters, digits, `-` and `_`); a theme folder the app doesn't
  know, or a file's `theme` not its folder's (`ex-shape`, `ex-theme`);
- a declaration's description fields wrong (§5.1, `ex-desc`), or its own (`ex-decl`): an unknown
  method or availability (a block's own, or the declaration's: one of them is required); a class a
  block's method can't have (a method that fixes its class takes that or `claimed`; `arcgis`,
  `wfs` and `file` say `index`, `service` or `claimed`; `none`'s is `unknown`, with its `why`); no
  `[[piece]]` block, a block without its `id`, two of one id; `by` not a list, `name` not one of
  `by`; `tolerance_m` not positive; `filter` not a table of lists; a metric's reading not of its
  shape (a `field`; `scale` positive, `index` from 0, `date` one of `epoch-ms`, `iso`, `year`,
  `text`; no other key). A method's own fields (`url`, `pattern` …) are the producer's to read and
  check;
- a `.geojson` that isn't a FeatureCollection, whose `source` isn't its stem, or whose `read`
  isn't a list of readings each naming its block's `piece` and `method` (`ex-pieces`); pieces
  without `piece`, `extent` (a class) and `availability`, a `name` not a string, a `stated` not a
  table of strings, a geometry that isn't a MultiPolygon, or a geometry where the class is
  `unknown` and none where it isn't (`ex-piece`); invalid geometry (`ex-geom`): a position not two
  numbers, a longitude outside ±180° or a latitude outside ±90°, a ring not closed, of fewer than
  four positions, of no area or crossing itself, an exterior wound clockwise or a hole
  counter-clockwise (RFC 7946), an edge stepping more than half a turn of longitude (not cut at
  the antimeridian) but one from −180° to 180° along a parallel (a piece spanning the turn, or
  closed along a pole); a piece's id twice (`ex-twice`). Each finding
  lists its pieces. Whether a polygon's holes lie inside it, or its polygons overlap, isn't checked;
- a `.geojson` without its declaration (`ex-pair`), or not made from it as it is: the blocks its
  `read` names aren't the declaration's, a piece's id doesn't start with one of them, or a
  piece isn't of its block's class (its method's or the one it says; `claimed` where it says so or
  its availability isn't `open`), availability or constant metrics (`ex-made`);
- a `score.toml` not of its shape (above: an unknown field, no `primary` among its metrics, the
  primary not on a log scale or with a weight, `within` not above 1, `share` not from 0 to under 1
  or breaking the guarantee, a metric without its `label`, a scale whose `worst` = `best` or a log
  scale's not positive, a `values` table not of numbers from 0 to 1, a weight missing or
  negative, a `label` not a name, an `availability` not each of the four ids once: `ex-score`); a
  theme with no `score.toml` (`ex-noscore`);
- a metric the theme's score doesn't name, in a declaration's block or a piece (its `stated`
  too), or a value off its scale's kind: a positive number on a log scale, a number on a linear
  one, one of the `values` words (a declaration's constants and its `map`s' values alike;
  `ex-metric`, held with the score).

Warnings: a declaration with no `.geojson` yet (`ex-nopieces`); pieces of class `unknown`
(`ex-unknown`); a `score.toml` holding fields no longer read (`ramp`, `shade`, a metric's `classes`
or `colours`: `ex-retired`). A source's two files are held together. Facts read at check time: a `.geojson`'s
pieces, by class and availability, and its box (`pieces`, `classes`, `availability`, `box`); a
declaration's `theme`, `availability` and `blocks`; a score's `primary` and `metrics`. Tests:
crate::inputs::extents's, with the real files of `producers/extents/terrain/` taken with no
error.

**The sources so far** (terrain; `producers/extents/terrain/`, read 13 Nov 2026; the worldwide
models, the polar mosaics, BC, Wales, France and Luxembourg the same outlines as on 12 Nov; North
America read on 13 Nov, its large indexes on the Mac mini; Europe below):

| Declaration | Pieces | Class | Read by |
|---|---|---|---|
| `cop-dem` | 2: GLO-30, GLO-90 | `index` | `listing`: each bucket's list of its 1° tiles (GLO-30 withholds Armenia's and Azerbaijan's) |
| `pgc-arcticdem` | 1: the 2 m mosaic | `index` | `file`: the centre's tile index (a zipped shapefile, EPSG:3413) |
| `pgc-rema` | 1: the 2 m mosaic | `index` | `file`: the same, EPSG:3031; its hole round the South Pole kept (no data south of ~89° S) |
| `jaxa-aw3d30` | 1 | `claimed` (a free account) | `listing`: the agency's public list of 1° tiles |
| `nrcan-hrdem` | 951: 950 lidar projects, the ArcticDEM-derived mosaic | `index`; `files` | `stac`: each project's outline (its `extent` asset: the items' own footprints, hulls for many, summed 5.07 million km²; the extent files 2.29 million, against the 2,230,605 km² NRCan states; 4.5 MB, their 1 km tiles' staircase edges, which a 200 m tolerance cuts by 6 % only), cell and year (from its name, or its item's `datetime` for the 145 names holding none); `raster`: the mosaic's 36 squares, each's mask at its overview of ~1 km pixels (`grain_m` 1,024) |
| `geobc-lidar` | 17: the DEM sheets by cell and year | `index` | `arcgis`: the portal's two sheet indexes, each sheet's cell as stated (1,659 sheets of 2016 are 2 m) |
| `wales-lidar` | 8: the 1 km tiles by year flown, the DTM's and the DSM's | `index` | `wfs`: the tile catalogue (EPSG:27700) |
| `ign-lidar-hd` | 6: the 1 km tiles by the year their flights ended (2021 to 2026) | `index` | `wfs`: the programme's tile index, 524,687 tiles (metropolitan France and the overseas tiles so far): 105 requests, 155 MB, 3 min 9 s and 477 MB of memory on the M1, to 174 KB; each year's area within 0.3 % of its tiles' count in km² |
| `luxembourg-lidar` | 1 | `files` | `raster`: the one GeoTIFF's header and its mask at an overview of 64 m pixels |
| `usgs-3dep` | 1,050: the 1 m DEM by project and year (1,021), the 1/9 arc-second DEM and Alaska's 5 m IfSAR DTM by year, the 1/3, 1 and 2 arc-second layers | `index` | `arcgis`: the bare-earth DEM service's catalogue (an ImageServer, Esri JSON), 143,630 items, 81 requests, 306 MB, 16 min on the mini, 429 MB of memory, to 1.1 MB; the 1 m DEM's 126,836 10 km tiles sum 12.67 million km² (their count × 100 km²: within 0.1 %), its pieces within 0.001 % of the tiles' unions by project. The index service with the 0.5 m source DEMs refuses every feature query |
| `usiei` | 1,015: the US inventory's surveys by spacing, year and availability | `service`; `claimed` (paid, closed) | `arcgis`: NOAA's and the USGS's interagency inventory (topographic and topobathymetric lidar, 3,523 surveys complete or partly delivered, of every agency, state and city): each survey's stated point spacing and year; 18 MB read; 8.9 MB at a 500 m tolerance (408,165 positions: the surveys' own outlines, a piece per spacing, year and availability; at 1 km 5.0 MB and four small surveys dropped as specks, at 2 km 2.8 MB and sixteen) |
| `quebec-lidar` | 200: the point clouds by project, density and year; the 1 m DTM | `index` | `wfs`: the province's 614,801 latest 1 km tiles, 62 requests, 408 MB, 6 min 43 s on the mini, to 199 pieces summing 615,301 km² (the tiles' count, about 1 km² each: within 0.1 %); `file`: the DTM's 2,630 sheets |
| `ontario-lidar` | 51: the DTM by project, cell and year; the forest inventory's single-photon DTM | `index` | `arcgis`: the province's two lidar image services' catalogues, 203,904 and 419,331 1 km tiles, 626 requests, 161 MB, 12 min 45 s on the mini, 484 MB of memory (a run from the cache), to 518 KB; the pieces' areas within 0.03 % of the tiles' own unions (187,791 and 411,748 km²) |
| `saskatchewan-lidar` | 2 | `index` | `arcgis`: the province's lidar image service's catalogue (964 rasters, Consul and the Great Sand Hills) |
| `yukon-lidar` | 84: communities and corridors | `index` | `arcgis`: the territory's terrain-model image service's lidar items (96) |
| `geonb-lidar` | 5 | `index` | `arcgis`: the province's 1 m DEM image service's catalogue (158 blocks, their outlines over the sea too) |
| `novascotia-lidar` | 100: the projects | `service` | `arcgis`: the province's 100 project areas, each its required pulse spacing |
| `inegi-mde` | 3: the 5 m and 1.5 m sheets, the 15 m CEM | `index`; `claimed` | `file`: the institute's sheet indexes (26,333 and 1,843 sheets: 1,027,835 and 73,357 km², within 0.3 % of its own figures), and its national outline for the CEM |
| `haiti-dtm` | 2: the 1.5 m DTM's tiles; the southern peninsula | `files`; `claimed` | `raster`: OpenTopography's bucket listing, 357 10 km tiles, each's header and its mask at its overview of 24 m pixels (675 requests, 44 MB, 16,259 km²); `file`: the four departments its 12 southern 50 km tiles span (7,807 km²), those tiles having no overviews (2.4 GB, a mask of theirs the whole file) |
| 18 claimed sources | 22 | `claimed` | `file`: the areas the publishers state their data covers, as Natural Earth's 1:10m admin-1 areas (v5.1.2, public domain) draw them, filtered: El Salvador, Honduras, the Cayman Islands, Turks and Caicos, Antigua and Barbuda, Saint Lucia, Grenada, Barbados, Trinidad, Saint Vincent, Bonaire, Saba, Sint Eustatius, Greenland (GHM, at a 1 km tolerance), Bermuda, Saint-Pierre-et-Miquelon, Martinique, Saint-Barthélemy, Saint-Martin, Nuevo León, Connecticut, the District of Columbia |
| `belize-crip`, `dominicanrep-sgn`, `dominica-dvrp`, `guatemala-ign`, `stkitts-lidar` | 1 each | `unknown` | `none`: where they lie isn't stated (Belize's 17 %, Saint Kitts and Nevis's 2024 survey, Guatemala's elevation model, Dominica's lidar, seen only village by village), or their area would overstate them (Greater Santo Domingo's 380 km², in administrative areas of some 1,400 km²) |

**Not obtained** (North America, 13 Nov): 3DEP's 0.5 m source DEMs (its index service refuses
every feature query, by `where`, object ids, `find` and `identify`; it has no FeatureServer; its WESM
GeoPackage is 3.8 GB: the US inventory, `usiei`, has the same surveys at their point spacing);
NRCan's per-project valid-data polygons (coverage.gpkg, 8.4 GB in all, over a source's 5 GB, and no
year: the extent files are read instead); Nova Scotia's tile and 10K-sheet layers (joins sharing
their object ids); Manitoba's own project DEMs (13 to 14 GB zips with no index read; NRCan's HRDEM
holds the province's projects); Newfoundland's 5 m DEM (one 4 GB zip, its Labrador coverage not
stated); Anegada's USGS 1 m DEM (Natural Earth draws the British Virgin Islands as one area);
Belize's, Dominica's and Saint Kitts and Nevis's lidar, Guatemala's elevation model and Greater Santo Domingo's lidar (no outline, or their areas would overstate them: listed, `unknown`); North Carolina's 2024 Phase 3 counties, Indiana's 2026
block and the Mexican states' and municipalities' local lidar (Jalisco, Querétaro, Quintana Roo,
Tabasco), whose areas aren't stated or have no outline to claim; NOAA Digital Coast's per-dataset tile
indexes (its surveys are in `usiei`, at the inventory's outlines); Haiti's 12 southern 50 km tiles' masks (no overviews: 2.4 GB read whole; claimed by department instead).

**Europe** (read 13 Nov 2026; England's, swissALTI3D's, PST-A's, mainland Portugal's, RGE ALTI's
and the BKG's indexes on the Mac mini, the rest on the M1): 119 declarations, 2,600 pieces, 2,567
with an extent. Areas are checked in EPSG:3035 against the publisher's figure or the tiles' count
times their area.

| Declaration | Pieces | Class | Read by |
|---|---|---|---|
| `gb-eng-lidar` | 97: the National LIDAR Programme's tiles by year, the composite DTMs' coverages (1 m, 2 m) by year, the time-stamped surveys' 50 and 25 cm | `index`; `service` | `wfs` ×5: the Environment Agency's indexes (EPSG:27700; the 50 and 25 cm blocks with a `CQL_FILTER` in their `url`, the server timing out on their filters as fields); 108 requests, 308 MB, 9.3 min from the cache on the mini, a peak of 3.7 GB; 1.54 MB at 100 and 200 m tolerances; the programme's 7,139 5 km tiles 178,475 km², its pieces 177,472; the composites' 1 m 138,593 km² (England is 130,279: the tiles over the sea) |
| `gb-sct-lidar`, `gb-sct-hes-lidar` | 11; 7: the portal's phases (I to VI, the National LiDAR Programme, Outer Hebrides, Orkney, the coast) and Historic Environment Scotland's sets | `index` | `listing` (s3): the portal's bucket, each file's lettered grid reference (NS16NE, NG2004) its square; each piece within 0.1 % of its tiles' count × area. The phases' extents are their point clouds' tiles: the DTM sheets hold corridors in 10 km sheets (Phase I's 382 sheets span 38,215 km² against its stated 11,845) |
| `gb-nir-osni-dtm`, `gb-nir-river-basin-lidar` | 1; 77 | `index` | `file`: OSNI's 10 m DTM's quarter-sheets (1,057 × 15.36 km²); DfI's 1 km tiles by basin, cell (1 or 0.5 m) and survey date, 3,721 km² against 3,722 tiles |
| `ie-gsi-lidar` | 189: the GSI's, TII's and the OPW's surveys, each lidar survey's DTM and DSM | `index` | `arcgis` ×9: each survey's 2 km tiles, their `RESOLUTION`, last capture year and `RMS_ERROR` ("Within +/- 150mm" taken as 0.15 m, the class kept as `stated`); photogrammetric surveys `surface` |
| `de-*-dgm1` (14 Länder), `de-bw-dgm025` | 1 to 82 each, 199 in all | `index`; Bayern's `service` | each Land's own index: Berlin's, Mecklenburg-Vorpommern's and Schleswig-Holstein's ATOM feeds' tile names (`listing`), Brandenburg's OGC API and Niedersachsen's STAC (`stac`; Niedersachsen's every edition, so its lots overlap where re-flown), Bremen's, Hamburg's, Sachsen-Anhalt's and Saarland's archives' members (`listing` zip), Hessen's tile table (`listing` xlsx), Nordrhein-Westfalen's and Rheinland-Pfalz's folder listings (`listing`; Rheinland-Pfalz's 2022 tiles and their 2026 successors both), Sachsen's ArcGIS layer, Bayern's 82 flight lots (`file`), Baden-Württemberg's two WFS (0.25 m: ALS_2 by flight year, and ALS_3's re-flown tiles); each within 0.5 % of its tiles' count × area; years where the index states them (none for Mecklenburg-Vorpommern and Sachsen-Anhalt) |
| `ch-swissalti3d` | 7: by year (2019, 2021 to 2026) | `index` | `stac` with `latest`: 88,775 items for 43,650 tiles, each tile's newest; 888 requests, 248 MB, 17 min on the mini; 43,646 km² (Liechtenstein's whole) |
| `ch-ge-mnt`, `ch-zh-dtm`, `ch-lu-dtm` | 1 each | `files`; `index` | Geneva's 0.2 m COG (`raster`, a 12.8 m overview), Zürich's 0.25 m tile links (`listing`, 7,784 × 0.25 km²), Luzern's 0.25 m STAC (1,804 sheets) |
| `at-bev-als-dgm`, `at-ooe-dgm`, `at-tirol-dgm` | 59; 12; 7 | `service` | the BEV's metadata shapefile (its 1 m mosaic by flight area and year, 83,975 km² against Austria's 83,879), Oberösterreich's flight-year sheets and Tirol's flight areas (0.5 m) |
| `it-pst-lidar` | 51: the national PST-A lidar's 1 m, 2 m and land-sea grids by tile cell and year | `index` | `arcgis` ×3: the ministry's download grids (the empty cells left out, `pst <> ' '`), 164,063 1 m tiles uniting to 147,796 km²; 93 requests, 51 MB on the mini; 842 KB |
| `it-tn-lidar`, `it-er-lidar`, `it-veneto-lidar` | 1; 6; 1 | `index` | Trento's 0.5 m tiles (`listing`, 25,201 × 0.25 km²), Emilia-Romagna's campaigns' 0.5 m tiles (`arcgis`; 2023-24's 11,276 km² against the 9,240 stated: its index holds January 2025's flights too), Veneto's 5 m mosaic's outline (`wfs`) |
| `nl-ahn` | 5: AHN4, AHN5 (2023, 2024), AHN6 (2025, 2026) | `index` | `atom` (AHN4's 1,373 sheets, 42,971 km² against 1,373 × 31.25) and `stac` (AHN5's and AHN6's DTM sheets, each within 0.1 %) |
| `be-wal-mnt`, `be-bru-urbis` | 3; 1 | `index` | Wallonia's 0.5 m tiles by year (`arcgis`, 69,438 × 0.25 km²); Brussels' 1 km tiles (`atom`, its bboxes latitude first) |
| `fr-rgealti` | 8: RGE ALTI's 1 m grid by the class of its source (lidar, radar, photogrammetry, the topo-bathy lidar …), each its source's stated cell and RMSE (Emq) class | `service` | `wfs`: the source mask (`source_fra`), 290,932 polygons, 2.23 GB, 6.5 min and a peak of 13.9 GB on the mini; 2.4 MB at a 1 km tolerance (16 MB at 200 m, 5.9 MB at 500 m: the mask is pixel-edged; each class within 6 % of its 200 m outline), 651,700 km² |
| `fr-litto3d`, `fr-craig-lidar`, `fr-strasbourg-lidar`, `fr-lyon-lidar` | 5; 28; 1; 1 | `index`; `files` | Litto3D's campaigns' 5 km packages (`wfs`), the CRAIG's 2021 tiles by zone (`file`, a GeoPackage), Strasbourg's 2025 and Lyon's 2023 0.5 m COGs (`raster`, 64 m overviews) |
| `es-pnoa` | 18: the MDT 0.5 m sheets by UTM zone and year (16); MDT02 and MDT05 | `index`; `claimed` | `file`: the CNIG's auxiliary zip of 16 sheet shapefiles, 29,494 sheets, 248,078 km² (each piece within 0.1 % of its sheets' union); the 2 m and 5 m products over Spain as their records state ("cobertura completa") |
| `es-ct-icgc`, `es-an-rediam`, `es-na-mde`, `es-md-madrid` | 2; 1; 3; 2 | `files`; `index`; `service` | Catalonia's 0.5 m and Barcelona's 0.25 m COGs and Andalucía's 1 m (a 203 GB COG, its 256 m overview) by `raster`; Navarre's 0.5 m by its 1:10,000 sheets (`file`); Madrid's 0.1 m surface model's tiles and its 2019 DTM's KMZ (`file`) |
| `no-hoydedata` | 1,547: Norway's projects by cell (0.25, 0.5, 1 m) and year, 463 restricted (closed: claimed) | `index`; `claimed` | `arcgis`: hoydedata.no's DTM ImageServer catalogue (its fill items left out), 62 MB, 4 min on the M1; its DTM blocks 4.1 MB at a 200 m tolerance (8.0 MB at 50 m: 1,547 irregular block outlines; a coarser tolerance drops corridor projects as specks); 430,716 km² against the items' stated 432,284 |
| `se-markhojdmodell`, `se-goteborg-hojdmodell` | 17; 1 | `claimed` (login); `index` | Lantmäteriet's STAC (`dtm-cog`, 5,004 10 km tiles by year, 473,195 km²; its files download after a free login); Gothenburg's 0.5 m tile index (a zipped shapefile) |
| `fi-nls-km2`, `fi-nls-lidar` | 18; 7 | `index`; `claimed` | the NLS's OGC API collections: the 2 m model's sheets by year (Åland's whole), the paid 5 p lidar's areas with products by year |
| `cz-cuzk-dmr4g`, `sk-lls` | 1; 58 | `index`; `service` | the ČÚZK's ATOM feed (16,301 5 km² sheets of DMR 4G, 5 m); Slovakia's lot outlines (`file`, cp1250 shapefiles), cycle 1's 1 m and cycle 2's 0.5 m lots by year, each within 0.3 % of its stated area |
| `lv-lgia-lidar` | 2: the point clouds' 1 km sheets; the open 20 m DTM | `index`; `claimed` | `listing` of the LAS names by their nested sheet numbers (`digits`), 65,941 km²; the country |
| `si-clss`, `cy-dls` | 19; 3 | `index`; `claimed` | Slovenia's 2023–25 scan's 1 km tiles by block and year (`wfs`, 23,038 tiles; 0.5 m, RMSE 0.10 m as tendered); Cyprus's 2019 and 2014 1 m DTMs' 2.5 km tiles (`listing` of the INSPIRE folder) and its 1963 DEM claimed over the island |
| `md-agcc`, `bg-mzh` | 4; 6 | `files` | each coverage's own mask, read from the WCS at a scale factor of 0.01 to 0.05 (`raster`): Moldova's 5 m models of 2007, 2020 and 2021 and a 0.5 m reserve; Bulgaria's 1 m DTMs of 2023 to 2025 and 4 m DSMs of 2020 to 2022 (110,900 km², the country 110,994). The server takes 2 to 5 minutes for a 1 m coverage: those blocks wait 600 s (`timeout_s`) |
| 46 claimed sources | 72 | `claimed` | `file`: the areas the publishers state their data covers, as Natural Earth's 1:10m admin-1 areas draw them: Ireland (Tailte Éireann, sold), the Isle of Man (sold), Thüringen (its index unreachable), Germany (the BKG's DGM1, sold; drawn from the Länder's tiles, `wfs`), Basel-Landschaft, Burgenland, Vorarlberg, Andorra, Flanders, TINITALY, Bolzano, Valle d'Aosta, Lombardia, Piemonte, Liguria, Friuli-Venezia Giulia, Sicilia, Sardegna, Campania, Puglia, Lazio, Marche, Malta, Valencia's provinces, the Basque Country, La Rioja, the Balearics, Cantabria, Castilla y León, mainland Portugal (login; its STAC's 91,202 tiles), the Azores (login; non-commercial kept), Madeira, Svalbard's DTM 20, Denmark (login), the Faroes, Iceland, Poland, Estonia, Lithuania (login), Hungary (paid; its 25 m free after a login), Prague, Slovenia's 2011–15 survey, Croatia (paid), Bosnia and Herzegovina (paid), Albania (closed; no grid stated, so no score), Sofia |
| 23 sources listed only | 33 | `unknown` | `none`, each with its `why`: Kärnten's 2022–29 campaign, Bolzano's 0.5 m and Adige 0.2 m, Valle d'Aosta's 0.5 m corridors, Sardegna's 1 m, Toscana's lidar (public bodies only), Navarre's Pamplona 12.5 cm, Northern Ireland's coastal, heritage and roads surveys, Tailte Éireann's 2005–14 lidar, Svalbard's DTM 5 and Jan Mayen's, Stockholm's (sold), Helsinki's (the WCS's rectangle only), Brno's and Riga's, the Federation of Bosnia and Herzegovina's river-corridor lidar, Republika Srpska's, Serbia's, North Macedonia's, Greece's, Turkey's, Romania's and Russia's |

**Not obtained** (Europe, 13 Nov): Thüringen's tile index (its ATOM host answers an outage page,
its new portal a CAPTCHA; claimed); Mecklenburg-Vorpommern's and Sachsen-Anhalt's tile years (only
in a page's own application, a PDF and per-tile files); Kärnten's sheet indexes (its host timed
out from both Macs); Steiermark's 0.5 m campaign (no download stated; its finished blocks are in the
BEV's 1 m); Basel-Landschaft's, Burgenland's and Vorarlberg's indexes (an order form, a PDF, an SFTP
login: claimed); Flanders' DHMV II tiles (its WFS redirects to an error, its OGC API is 404, its
download API's 43 NGI sheets would double the region: claimed); Poland's NMT sheet indexes (every
GUGiK host times out from both Macs: claimed); Estonia's and Lithuania's indexes (a sheet at a
time, or none published: claimed); the Czech update years (its metadata layer returns no
geometry); Brno's DTM (its ImageServer has no catalogue, its zip answers 403); Spain's MDT02 and
MDT05 lot outlines (RAR files; the download centre's search answers 403 without its session:
claimed); Valencia's, the Basque Country's, La Rioja's, the Balearics' and Cantabria's grids'
indexes (sheets holding data only in their province, a WCS envelope, 403s, null footprints, a map
service only: claimed); Pamplona's quarter-sheets (no outline); Toscana's sheet index (only in its
Cartoteca app); Emilia-Romagna's restricted surveys (WCS 401); Bolzano's and Valle d'Aosta's finer
models (WCS only, a token: listed); Basilicata's 5 m (no record found) and Abruzzo's model (grid
and vintage unread); Svalbard's DTM 5 cells (codes with no geometry) and Jan Mayen's (one zip, no
index); the Faroes' and Iceland's files (no overviews: masks would be whole downloads; claimed);
Helsinki's DTM (the WCS describes a rectangle with the sea); Stockholm's (Natural Earth has only
its county); Slovenia's 2011–15 index (ARSO's lidar pages answer 503: claimed); Romania's (the
ANCPI's geoportal is down after an outage; Mapterhorn's file list names 325 LAKI II units by code);
Turkey's, Greece's and Russia's coverage (unstated or unreachable: listed); Jersey, Guernsey and
Gibraltar (nothing finer than 30 m published).

**Surface models** (read 13 Nov 2026, task #1370; England's, swissSURFACE3D's, PST-A's, mainland
Portugal's, Sweden's Ythöjdmodell, 3DEP's, NRCan's and Laserdata Skog on the Mac mini, the rest, Ontario's among them, on the M1): the
rows above count the ground models and point clouds as first read; these are the surface models
and point clouds declared beside them, each a block of its source's declaration (a declaration of
its own, marked "own" below, where it has no ground model here), `surface = "surface"` (a point
cloud, holding both, states none; its spacing 1/√density), its own spacing and year as its records
state them.
Image-matched (photogrammetric, correlation) models say so in their declarations' notes. Areas in
EPSG:3035, against the tiles' count times their area or the publisher's figure.

| Declaration: blocks | Product (method), spacing | Class | Read by; area |
|---|---|---|---|
| `nrcan-hrdem`: `lidar-dsm`, `cdsm` | each lidar project's DSM (its item's `dsm` asset, the DTM's cell and extent: every item has both); the Canadian Digital Surface Model (from the SRTM's 1 arc-second DSM of 2000), 16 m | `index`; `files` | the same STAC items as the DTM's; the CDSM's one GeoTIFF's mask at an overview. 8.7 MB (2,683 requests, 112 MB, 2.5 min on the mini, 453 MB of memory, from the cache but the CDSM): the 950 project outlines twice, the DTM's and the DSM's |
| `ontario-lidar`: `dsm`, `fri-spl-dsm`, `dsm-imagery`, `dem-imagery` | the lidar DSMs (0.5 m, 1 m; the forest inventory's single-photon DSM, 0.5 m); the image-matched DSM (0.5 m) and DEM (2 m, "both surface and ground features") of the photography projects | `index` | `arcgis`: the four image services' catalogues, 1 km tiles: 194,149, 419,333, 87,164 and 73,138 tiles, the pieces summing 193,547, 412,998, 87,032 and 72,984 km² (the DTM blocks' 1,413 requests with them: 390 MB, 35 min and 584 MB of memory on the M1); 1.1 MB |
| `usgs-3dep`: `ifsar-dsm` | Alaska's IfSAR DSM, 5 m (radar) | `index` | `file`: its Full Extent Spatial Metadata (FESM_DSM_PROJ.gpkg, 26 MB, 429 cells), by year: 1,784,420 km², the IfSAR DTM's items 1,594,813 (its cells reach further over the sea) |
| `iowa-geotree`: `dsm-2020` (own) | Iowa's statewide lidar DSM, 1 m | `files` | `raster`: the 272 GB COG's mask at a 256 m overview, 147,746 km² (the state 145,746) |
| `de-*-dgm1`, `de-bw-dgm025`, `de-bkg-dgm1` | the Länder's lidar DOM1 (1 m: Berlin, Bremen and Bremerhaven, Niedersachsen, Nordrhein-Westfalen, Rheinland-Pfalz, Sachsen, Baden-Württemberg; Mecklenburg-Vorpommern's and Sachsen-Anhalt's image-matched; Saarland's, its method not stated; Thüringen's claimed) and image-matched bDOM (Mecklenburg-Vorpommern 0.1 m, Brandenburg, Niedersachsen, Rheinland-Pfalz, Sachsen-Anhalt, Schleswig-Holstein and Bayern 0.2 m, Nordrhein-Westfalen 0.5 m, Berlin's and Hamburg's 1 m); the BKG's national DOM1 (paid: claimed) | `index`; Bayern's `service`; `claimed` | each Land's DGM index's twin (the same feed, collection, folder or archive's DOM entries), Bayern's DOP flight areas (a GeoPackage), Mecklenburg-Vorpommern's yearly overview shapefiles, Schleswig-Holstein's mass-download GeoJSON (17,626 km² against 17,614 tiles), the BKG's WFS; each within 0.5 % of its tiles' count × area. Baden-Württemberg's bDOM claimed (paid by order); Bremen's and Thüringen's bDOM listed (`none`) |
| `gb-eng-lidar`: 7 | the National LIDAR Programme's DSM (1 m), the composites' last-return and first-return DSMs (1 m, 2 m, RMSE 0.15 m), the time-stamped DSMs (50 and 25 cm) | `index`; `service` | the same WFS's `_DSM_` layers; each within 0.4 % of its DTM's union (the first-return composite 134,537 km²). 3.4 MB: its DSM pieces repeat the DTM's outlines |
| `gb-sct-lidar`, `gb-sct-hes-lidar`, `gb-nir-river-basin-lidar`: 11; 7; 1 | each phase's and survey's DSM, its DTM's spacing and year | `index` | the same bucket's `dsm/` names (the NLP's 5,994 tiles: 5,994 km²); DfI's grid index; Northern Ireland's coastal DSMs listed (`none`: tile services only) |
| `nl-ahn`: `ahn4-dsm`, `ahn5-dsm`, `ahn6-dsm` | AHN's 0.5 m DSM | `index` | PDOK's dsm_05m ATOM (42,971 km²); the AHN5 and AHN6 collections, their DSM field |
| `ch-swisssurface3d` (own): `dsm`, `points-2024`, `points` | swissSURFACE3D Raster, 0.5 m; swissSURFACE3D's point clouds (15 and 5 pts/m²) | `index` | `stac` with `latest`: the raster's newest tiles 43,688 km² (swissALTI3D's 43,646), the point clouds' 2024–26 tiles 15,632 km² and the earlier ones' 26,091 (1,781 requests, 304 MB, 21 min on the mini) |
| `ch-zh-dtm`, `ch-lu-dtm`, `ch-ge-mnt`: `dom`; `dom`, `dom-points`; `mns` | Zürich's 0.25 m DOM (1,946 km²), Luzern's 0.25 m DOM and its point cloud (1,691 km²), Geneva's 0.2 m MNS | `index`; `files` | tile links, STAC, Geneva's COG at its coarsest overview, 1.6 m (329 km²; 7,339 range requests of 64 KiB, 481 MB, 91 min on the M1) |
| `ign-lidar-hd`: `mns`; `fr-craig-lidar`, `fr-lyon-lidar`: `mns`; `fr-mns-correl` (own) | LiDAR HD's 0.5 m MNS (524,956 km², every tile); the CRAIG's and Lyon's 0.5 m MNS; IGN's image-matched MNS Correl, 0.5 m | `index`; `files`; `claimed` | the same WFS index, GeoPackage and COG; MNS Correl claimed over France with Saint-Pierre-et-Miquelon, Saint-Barthélemy and Saint-Martin (no index read; no year or accuracy stated) |
| `be-wal-mnt`: `mns`, `mns-photo-2023`; `be-vlg-dhmv`: `dsm`; `luxembourg-lidar`: `mns` | Wallonia's lidar MNS 0.5 m and its image-matched 2023 MNS (login: claimed, 16,988 km²); Flanders' DHMV II DSM 1 m (claimed); Luxembourg's 0.5 m MNS (2,633 km²) | `index`; `claimed`; `files` | the same tile index; the region; the GeoTIFF's mask |
| `es-pnoa`: 10 `mds50cm-*`, `mds02`, `mds05`; `es-na-mde`: `mds-2024`, `mds-2017`; `es-ct-icgc`: 3; `es-ib-mdt`: `mds-2m-2014` | PNOA's MDS 0.5 m (14,337 sheets, 121,418 km²; its MDS02 and MDS05 listed, no coverage stated); Navarre's MDS (401 sheets each); Catalonia's 1 m DSM, Barcelona's 0.25 m and the ICGC's image-matched 1 m of 2024; the Balearics' 2 m (claimed) | `index`; `files`; `claimed` | the CNIG's MDS50cm auxiliary zip; Navarre's sheet file; the ICGC's COGs at 100 m masks |
| `pt-dgt-lidar`: `mds-50cm`; `pt-ac-lidar`: `mds-1m` | Portugal's 0.5 m MDS (login: claimed on its STAC's tiles, 90,601 km²); the Azores' 1 m MDS (2,912 km²) | `claimed`; `index` | the DGT's and the Azores' STAC collections |
| `it-pst-lidar`: `dsm1`, `dsm2`; `it-fvg-lidar`, `it-bz-dtm`, `mt-pa-dtm` | PST-A's DSM grids (1 m 147,796 km², 2 m 17,023 km², as its DTM's); Friuli-Venezia Giulia's 0.5 m and Bolzano's 2.5 m DSM, Malta's 1 m (claimed); Bolzano's 0.5 m, Adige 0.2 m and glaciers', Toscana's and Sardegna's DSMs listed | `index`; `claimed`; `unknown` | the ministry's download grids; the regions |
| `im-lidar`: `dsm` | the Isle of Man's 0.5 m DSM (sold: claimed) | `claimed` | the island |
| `no-hoydedata`: `dom`, `dom-restricted` | hoydedata's DOM projects (lidar, and 424 image-matched "Bilde" projects with no DTM), 0.25 to 1 m | `index`; `claimed` | the Prosjekt_DOM ImageServer's catalogue: 978,350 km² against its items' 981,192 (projects overlap). 9.1 MB: 3,520 irregular outlines at 200 m (a coarser tolerance drops corridor projects as specks) |
| `se-ythojdmodell` (own) | Lantmäteriet's image-matched Ythöjdmodell, 0.25 and 0.5 m (paid: claimed) | `claimed` | `stac` (dsm-cog): 149,614 items, 997 MB, 54 min on the mini, 240 MB of memory; 935,353 km² against items × 6.25 km² |
| `se-markhojdmodell`: `laserdata-skog` | Laserdata Skog's point clouds, by stated density (login: claimed) | `claimed` | `stac` (dsm-skoglig-copc): 81,031 items of 2.5 km, 485 MB, 21 min on the mini, 179 MB of memory; nominal spacings 0.69 to 0.95 m; the pieces sum 506,503 km² against the items × 6.25 km², 506,444 (re-scanned tiles overlap: their union 409,918) |
| `fi-nls-km2`: `lidar05p` | the open 0.5 p point clouds the 2 m model is made from (1.414 m nominal) | `claimed` | the model's sheets (no index of their own) |
| `dk-dhm`: `dsm`; `ee-maaruum-dtm`: `dsm`; `pl-gugik-nmt`: `nmpt`, `nmpt-urban`; `al-asig`: `dsm` | DHM/Overflade 0.4 m (RMSE 0.06 m), Estonia's 1 m, Poland's NMPT 1 m (its 0.5 m urban one listed), Albania's (closed) | `claimed`; `unknown` | the countries |
| `at-bev-als-dgm`: `dom`, `dom-luftbild`; `at-ooe-dgm`, `at-tirol-dgm`, `at-bgld-dgm`, `at-vbg-dgm`: `dom` | the BEV's ALS-DOM 1 m (84,011 km²) and its image-matched DOM 0.5 m (paid: claimed, 13,828 sheets); the Länder's 0.5 m DOMs (Burgenland's and Vorarlberg's claimed) | `service`; `claimed` | the same metadata shapefile and flight-year layers; the BEV's actuality shapefile |
| `cz-cuzk-dmr4g`: `dmpok`, `dmpok-points`; `sk-lls`: 9 `dmp*`; `si-clss`: `dmp`, `gkot` | Czechia's image-matched DMP OK 0.5 m and its point grid (16,301 sheets); Slovakia's DMP 1 m and 0.5 m by lot and year; Slovenia's first-return and classified point clouds (23,034 km²) | `index`; `service` | the ČÚZK's ATOM feeds; the same lot shapefiles; the same WFS tiles |
| `cy-dls`: `dsm-2019`, `dsm-2014`; `bg-mzh`: `dsm-alternative` | Cyprus's 0.5 m DSMs (1,068 and 1,063 tiles of 6.25 km²); Bulgaria's Dem_Alternative 4 m (110,095 km²) | `index`; `files` | the INSPIRE folder's listing; the WCS at a coarse scale |
| `lv-lgia-lidar`, `mk-akn`: listed | Latvia's 1 m DTM and DSM (public bodies only), North Macedonia's DSM | `unknown` | `none` |

**Not obtained** (surface models, 13 Nov): Saxony's DOM2 (its DOM1, coarser), Baden-Württemberg's
nDOM (heights above ground), Bavaria's DOM mesh (not a grid), Brandenburg's TDOM (no download);
Schleswig-Holstein's DOM1 (no feed found); Hessen publishes no bDOM; Trento's, Emilia-Romagna's (its
WCS answers 401), Tailte Éireann's, Madeira's, Andorra's, Strasbourg's (nDSMs only) and the Litto3D
surveys' (none published); La Rioja's (403s) and Aberdeen's 2025 (listed, not in the bucket); the
Basque Country's MDS (listed: its WCS answers 0 outside the Community with no nodata stated, which
`raster` can't take as no data); Bern's, Schaffhausen's, Fribourg's and Florence's city DSMs (not
read); the other point clouds, not yet declared, though the rule above declares them (a later run's:
LiDAR HD's, Scotland's, Portugal's, AHN's, Luxembourg's and Brussels' LAZ, and those the Länder's,
Tirol's and Slovakia's notes name as their grids' source); Czechia's DMP 1G (a TIN, no spacing
stated, withdrawn from 2027); Finland's (no DSM published), Croatia's, Hungary's, Moldova's and
Lithuania's (none or no grid stated); Vorarlberg's 2024 re-flight (no outline); Kärnten's (its host
unreachable); the older editions of a model whose newer one covers its area (the ICGC's 2018–23
correlation models, Geneva's 2023 MNS). In North America: Quebec publishes no DSM (its lidar
products are the DTM, a canopy height model and the point clouds, declared); the US inventory's
IfSAR surveys are 3DEP's Alaska IfSAR (its DSM declared); the states' own DSMs (Illinois's,
Montana's, Utah's, Hawaii's, the District of Columbia's) and NEON's sites, of the same surveys as
3DEP's 1 m DEM, are for a later run; NRCan's CDEM and MRDEM-30 (ground models, 20 and 30 m) aren't
declared.

The outlines are the publishers': an index's tiles or a catalogue's blocks reach over the sea
where they do (GeoNB's blocks: 106,396 km² against the province's 72,908; Nova Scotia's project
areas, 63,636 against 55,284; GLO-30's 1° tiles round every coast), and the layers draw them as
they are, not cut to the land.

**Bathymetry and buildings** (planned) take the same folder, declaration, piece properties and
score, with their own metrics in their own `score.toml`. Nothing in the format is the terrain's
but its metrics' names: a piece is an outline, an availability, an extent class, metrics and what
was read. A bathymetry piece (a survey, or a class of a composite grid's source grid):

    {"piece": "tid/multibeam", "extent": "files", "availability": "open", "spacing_m": 15,
     "method": "multibeam", "density": 0.9, "year": 2019}

with `method` a word on a `values` scale (multibeam, lidar, single-beam, satellite-derived,
interpolated), `density` the survey's share of cells holding a sounding (a `linear` scale), and
`spacing_m` the grid's, which for a composite says little and weighs little. A buildings piece:

    {"piece": "city/2023", "extent": "mask", "availability": "open", "detail": "LoD2",
     "completeness": 0.97, "year": 2023}

with `detail` on a `values` scale (footprints, heights, LoD1, LoD2, LoD3, textured mesh). What
they'll need that the terrain doesn't (planned with their research): a `raster` reading by value,
a piece per class of a source-identifier grid (GEBCO's type identifier grid, EMODnet's source
references), where the terrain's reader takes one valid-data mask a file; and, for the buildings
the build already downloads (Overture's, docs/steps.md §7), extents of class `mask` from the
build's own data, per source the features name.

## 7. The guards

Built on the pattern of `crates/pipeline/tests/cache_accessor.rs`: one accessor the code must go
through, a test that finds every way around it, and an exemption list (item, anchor, reason) whose
every entry must match a line (`every_exemption_matches_a_line`). Both run before every publish,
first, before the build (`tools/app/guards.sh`, which `tools/app/publish.sh` runs; a failure stops
publishing; the tests' whole output in `~/Library/Logs/scenic/guards-<time>.log`).

### 7.1 Inputs folders only through the accessor

- **Rust:** `inputs::open` (§4.6) and the input check's job are the only code given inputs-folder paths.
  `Out::path` (the project root type) panics, naming its caller, when asked for a content name under
  `inputs/`, `translations/` or `descriptions/`. A step gets its inputs' paths from the agent.
- **The test** (`crates/pipeline/tests/inputs_accessor.rs`): every non-test item in `crates/` (a
  function by its block, a `const` or `static`, or the file's top level) whose code names an inputs folder
  (`"inputs`, `join("inputs")`, `/inputs/`, `"translations`, `}/translations`, `"descriptions`,
  `}/descriptions`, each unit's name; `sources/inputs` aside) must call an accessor (`inputs::open(`,
  `inputs::gate::run(`) or be exempt by item name and line anchor with a reason. No reader is
  exempt: only the Regions panel's writes, the backups, GC's never-swept list, the keys
  (`inputs/keys.env`) and words that aren't paths (the `inputs` step's name, a
  task spec's "inputs", log lines). The scanner's own rules have a test.
- **Python:** steps receive input paths on their command line. `dem/inputs.py`'s `arg(path)` refuses
  a path not under `sources/inputs/`, `sources/fetched/` or the job's scratch, and the same test
  scans `dem/**/*.py` (functions by indentation) by the same rules.
- **The stand-in root:** `tools/app/guards.sh` loads every Python step the agent runs from the
  app's `dem/` (its tracked files alone) placed in a stand-in NAS root whose inputs folders
  (`inputs/`, `inputs/sources/`, `inputs/elevation/`, `translations/`, `descriptions/`) hold a file
  each and can't be read, then checks them with pyflakes: a step that reads an inputs folder as it loads,
  by a path the scanner doesn't see, fails there.

### 7.2 No source names in the app's code

`crates/pipeline/tests/source_names.rs` fails when a data source's address or name is in the code
or strings of `crates/` (tests aside: `#[cfg(test)]` items, `tests.rs` files, `tests/`), `dem/`,
`web/src/`, `tools/` or `scripts/`: their Rust, Python, TypeScript and
JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`), shell, Swift, C, CSS, HTML, JSON, TOML and YAML files.
Comments aren't scanned (nor Python's docstrings and `test_…` functions). `producers/` is exempt: the
declarations are made and staged there.

- **Markers, derived from the declarations staged in `producers/`** (each `.toml` but
  `pyproject.toml`), so a new declaration adds its own: the hosts of its `source`, `url`/`urls` and
  its staged index's files' URLs (a path-style S3 URL's host with its bucket, the host being
  shared); its `name` and `credit`, whole, and up to their first `,` `(` `:` `;` when that's two
  words or more; its name's first two words in any case (as written when both are plain
  capitalized words, "Terrain Tiles", common in lower case); and its id, the file's first word, as
  the name writes it and as a whole word, when it's a word of the name, five letters or more and no
  role's, format's or encoding's word (no `[data]` value of any declaration): today `Overture`,
  `CanVec`, `NALCMS`. A host is found where it isn't part of a longer word (Geneva's `ge.ch` isn't in
  `heritage.checked`). And any other URL, but this machine's, the tailnet's (`100.64.0.0/10`), the
  documentation names (`example.*`, `.test`, `.invalid`, `192.0.2.*`) and a host of one label; a host
  behind a template (`${x}.host`) is judged by what follows it.
- **Allowed:** the W3C's namespaces (a format's); OpenStreetMap (its tags are the map's schema; its
  links open a feature or a place there); Wikidata's and Wikipedia's pages opened from an item;
  IRIs in angle brackets (a SPARQL query's vocabulary, and the sites it names as nodes).
- **Exempt**, by file, item and anchor, with why (`EXEMPT`):

| Where | Why |
|---|---|
| `rules::CREDITS` (the declared sources' entries, the CanVec licence's notice) | the credits' fallback (§4.8): a declared source's entry is replaced by its declaration's credit once one is accepted, and stands in until then, so a map never lacks a licence's notice |
| `lakefill::run::fill`'s metadata | part of the fill's content name: changed, the fill and the water, terrain and catalogue reading it would be made again for nothing |
| `leaftype`'s square tags, and `trees::squares::complete` | a made square's GeoTIFF `source`, as every square already made says it (`trees::squares::complete` reads the tags) |
| `trees::META` | the tree tiles' metadata, part of their content: changed, every tree tile would be made again for nothing |
| `leaftype`'s `fetch_nalcms` and `nalcms_centre` | the reader of NALCMS's zip and its GeoTIFF's projection, made for their layout; their messages name what they read |
| `bld::overture`'s `row_group`, `bbox_spans` and `boxes`, `bld::fetch`'s `overture_jobs` | readers of Overture's schema and release folders, made for their layout; their messages name what they read |
| timing phases (`scenic-build`'s lake fill, `leaftype`'s NALCMS download and squares) | a phase's name is its key in the timings kept |
| `budget::QUERY_HOSTS`, `QUERY_DOMAINS` | the polite-use budget is by host (docs/pool.md §7.7): the query services' limits hold for any request to them |
| `web/src/main.ts`'s Google Maps link | opens the place elsewhere |
| `web/src/flags/index.json`'s pages and authors | each flag's credit is its file's page on Wikimedia Commons (the Credits link it) and its author as Commons gives it; their maker is a producer (`producers/flags/fetch.py`, §8) |
| `tools/check/pool-two-macs.sh`'s budget test | a request the budgets' proxy must meter, not a read |
| `scripts/fonts.sh` | the app's own assets (§6.9, also kept) |
| `tools/app/install.sh`'s uv link | documentation, in a message |

Of what §6.9 keeps in code, Planetiler (a version, `water::PLANETILER_VERSION`) and
`bld::fill::ESTIMATES` (Overture's dataset name the fill's rule matches) name no marker, so neither needs an exemption. The 3D buildings'
details name their source from the map's credits (`web/src/buildings.ts`, `setBuildingsCredit`).
Short names other than the ids (`Terrarium`, an encoding; `GHSL`, `EEA`) and store folders
(`sources/overture/`) aren't markers.

**What it can't see:** a name or address built at run time (split and joined, `"transl" +
"ations"`, or read from a file the code ships), a short name or abbreviation the declarations
don't give, and anything in a comment. It's a check on what's written, not a proof. The scanner's own
rules have a test (`the_scanner_finds_sources_by_their_declarations`).

## 8. How each input is made

- **Producers live in the repository** under `producers/<input>/` (`heritage/`, `timetables/`,
  `elevation/`, `languages/`, `sources/`, `depth/`, `extents/`, and `flags/`, which writes the app's bundled flags, `web/src/flags/`): per-source code and staged declarations kept
  out of the app, with their history and review. They're outside what `publish.sh` ships and
  outside the guards' scans; the app never runs them. Each runs on its own (`uv run --project
  producers python producers/heritage/registers.py --data <folder> --out <folder>`), writing the
  inputs folder's shape, checked when added. Their environment is their own (`producers/pyproject.toml`).
- **Each inputs folder's `how/`** on the NAS says how its input is made and points to its producers; it
  holds what isn't code: the briefs and models of agent-made inputs (translations: Haiku, the
  earlier work's brief and launch prompt; descriptions: Sonnet, the September rounds' briefs; the
  ferry researchers' `PROMPT.md` and batches; the ferry feeds' search) and what was done by hand.
  Its README is kept in the repository, `producers/how/<box>/README.md`, and copied there with any
  briefs; the input check never reads `how/` (§3), so a copy changes no accepted version.
- **`inputs/README.md`** on the NAS describes the inputs folders, the kinds and the input check
  (`producers/how/inputs-README.md`).
- **What can't be remade exactly:** the regions and heritage matches (they are the owner's
  decisions); the converted translations (the earlier work's rules and later brief are in its own
  repository, not here); the descriptions written so far (their targets' rule wasn't kept); the
  ferry and MTR lookups (the sources must be read again; the researchers' model wasn't recorded).

## 9. What still names a source in code

What could still move out of the code: names the guard (§7.2) doesn't take for a source's (short
names, store paths, help text) or exempts. **D:** becomes a declaration
field. **P:** moves to a producer. Configuration and generic code (formats, protocols, road network
rules, storey heights per country, OSM name heuristics, the Wikidata properties asked for, the user
agent) stay.

- **Credits:** `inputs::rules::CREDITS` and its area boxes (D: each source's description, extents
  derived), and the server's `credits_of` fallback; the tree tiles' meta source string (D); source lines in web/src's layer help and popups
  (trees.ts, ferries.ts, overlays.ts, details.ts: D, from the map's credits). The World
  Heritage outlines' popup (overlays.ts, "site: UNESCO World Heritage Centre"): planned, the
  outlines carrying their register's id (`register`) so the popup reads its credit from the map
  (`sourceOf`); the overlays step can't add it alone (`ovconv`'s outlines know their dot's id and
  place, not its register), so it waits for the heritage job's next change to its outlines'
  file (`layer-whs-shapes`).
- **Kept by design, credits:** the 3D buildings' height-source legend and hover help
  (buildings.ts `SOURCES`, ui/buildings.ts: "Microsoft", "GHSL", "Esri"): they name the method
  that found a building's height (its `s`, docs/buildings3d.md §2.3), not a credit; the
  buildings' credit itself comes from the map (`setBuildingsCredit`).
- **Terrain:** the raw tiles' store path (`sources/aws-terrarium/`, named for its source) and the
  credit in `rules::CREDITS` the base's declaration takes over once accepted.
- **Trees and land cover:** the canopy and leaf-type squares' resolutions (D, read from data).
- **Buildings:** `bld/prep.rs`'s `ghsl` meta (D). `rules::CREDITS`'
  Overture and GHSL entries, which their declarations' credits replace once accepted (§4.8). `bld/fill.rs`'s `ESTIMATES` stays
  the fill's rule (§6.9).
- **Kept by design:** `budget`'s metered hosts (the polite-use budget, by
  host: §7.2); `markconv.rs`'s id schemes (`whc:`, `reg:dfhd:`, `legacy:`: ids must not
  change, docs/phase5.md, Ids); `codecs::legacy` (a name for live code).
