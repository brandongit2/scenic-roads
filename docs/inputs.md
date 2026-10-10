# Inputs: every input through a standard shape and a checked drop box

**Status: planned. Nothing in this document is built.** It is the spec for the input
standardisation series on the task board (#133 is this document; #134–#147 build it). Where it
describes today's code or today's NAS, it says "today" and cites the file; everything else is the
design. Companions: `docs/plan.md` (the pipeline, keys and order, which these tasks change),
`docs/formats.md` (files), `docs/pool.md` (the lead, jobs, the journal), `docs/phase5.md` (the
heritage chain).

**The idea in one line:** every input the map is built from has one written shape and one folder on
the NAS (its drop box). The owner makes inputs however they like, to that shape, and drops them
there. The build checks each change before taking it in, keeps the last good version while a
change is held, and reads only checked copies, named by their content, so builds stay deterministic.
No data source is named in the app's code: per-source conversion happens outside the app, and what
the app must know about a source (where to fetch it, its licence, its credit) is written in a
declaration file in a drop box.

## 1. Requirements

The owner's decisions of 9 October 2026 (the board's notes for #133–#147):
1. **A standard shape and a drop box per input.** The owner produces inputs to the shape and drops
   them in; the agent ingests them; after any building they show on the map.
2. **Every change is checked first:** its shape, then the input's own checks (thin gaps between
   regions, for instance). An error or a warning pauses ingesting it, with a warning on the menu bar
   icon and a banner on the build page. The last good version stays in use. A warning the owner
   agrees with can be accepted.
3. **Three kinds of input:** request–fulfil, declare, and source declarations (§2).
4. **Per input** (§6): regions as a file each, custom shapes in GeoJSON, thin gaps warned of; one
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
  content (a content name or a digest of its bytes), never by a path, a size or a time.
- **No hand-kept figures that go stale** (the owner's rule of 10 October): a declaration states
  facts about its source that can't be read from the data (its licence, its height reference,
  where it is), and everything that can be read from the data (resolution, coverage, extent,
  feature counts, a feed's box) is read from it, at check time. Where a declared fact can be
  checked against the data, the gate checks it, and a wrong declaration fails loudly as a finding;
  it never corrupts output silently. A hand-entered value may at most be a hint that costs speed
  when wrong, never correctness.
- **Docs say what's true** (this file is planned throughout; each task marks its part built as it
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
  it changes data, removing it removes data (once the removal passes the gate). The app never
  writes in a declare drop box, except where a tool of the owner's writes on their behalf (the
  Regions panel, `scenic add`/`remove`, as today).
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
  regions/                   declare: <id>.toml, and custom outlines as <name>.geojson (§6.1)
  languages/                 declare: territories.tsv, about.toml (§6.2)
  translations/              request–fulfil: **/*.jsonl; todo/ (the app's) (§6.3)
  descriptions/              request–fulfil: **/*.jsonl; todo/ (the app's) (§6.4)
  heritage/                  declare: <register>.geojson + <register>.toml, one pair per register (§6.5)
  heritage-matches/          declare: *.jsonl, the owner's same/not-same pairs (§6.5.4)
  timetables/
    feeds/                   source declarations: <feed>.toml, <catalogue>.toml (§6.7.1)
    gtfs/                    declare: <name>.zip, hand-compiled static GTFS; todo/ (the app's) (§6.7.2, §6.7.3)
  elevation/                 source declarations: <dataset>.toml, its files (if local) in <dataset>/ (§6.8)
  sources/                   source declarations for everything else the build reads (§6.9)
  keys.env                   not an input (secrets)
  hold-catalog               not an input (a control)
```

Each of these folders is one **gate unit**: checked, held and accepted on its own (timetables'
two folders are two units of one input). Inside any drop box:
- `todo/` is the app's (requests), `how/` is the owner's notes and producer scripts (§8); neither
  is read as input.
- Names starting with `.`, `@`, `#` or `_` are ignored (the Mac's and the NAS's own files: `@eaDir`,
  `#recycle`, `.DS_Store`; and the owner's drafts, `_draft.toml`).
- A file is taken only once its size and time have held for 10 s (as translations are today), so a
  file being written isn't checked half-written.
- Anything else that isn't the drop box's shape (a stray `.csv` in `heritage/`) is an error
  finding, not silently skipped: a file the owner meant as input must never be ignored quietly.

What moves (each in its task; §11):
- `translations/` and `descriptions/` move from the NAS root into `inputs/` (#137; decided by
  default, §13). Their `todo/` lists move with them.
- `inputs/outlines/` (`.poly` files, Geofabrik's and three of our own) goes: no recipe uses one
  today (§6.1).
- `inputs/ferries/` becomes timetable declarations and hand-compiled GTFS (#140, #141); its
  research notes and feed-finding notes move to `inputs/timetables/*/how/`.
- `inputs/names/analysis/` and `inputs/descriptions/briefs-2026-09/` (how earlier work was made)
  move to the `how/` of translations and descriptions (#147).
- `sources/registers/legacy` and `legacy-seeds` become heritage registers in `inputs/heritage/`
  (#138) and go with #139.
- The checked copies live under `sources/inputs/` (§4.6); they're the app's, never the owner's to
  edit.

## 4. The gate (#134)

### 4.1 Terms

- **Drop box:** the owner's folder. The build never reads it directly once its input is on the
  gate; it reads the accepted version.
- **Candidate:** the drop box's files as they are now, once they've held still.
- **Finding:** what a check says about a candidate: an **error** (the file can't be used: a shape
  broken, a field missing, a reference to nothing) or a **warning** (usable, but likely a mistake:
  a thin gap between regions, a register losing a fifth of its entries). Each has a stable id
  (§4.4), the files it's about, a message in plain words, and where it is when that's a place.
- **Held:** a change with an unaccepted finding. It isn't taken in; the last accepted version of
  the files it touches stays in use.
- **Accepted version:** the files the build reads: checked copies, content-named, listed by an
  index whose content name is the input's version (§4.6).

### 4.2 Noticing a change

- The lead (pool.md §5) lists every drop box (recursively for translations and descriptions,
  skipping `todo/` and `how/`) every two minutes, off its loop (a thread of its own, as its journal
  listings are: a listing can take half a minute under load), and keeps the last listing: each
  file's path, size and modification time.
- `scenic inputs check [<input>]` lists now (and the Regions panel's own writes ask for a listing
  at once, so a new region reaches the gate in seconds).
- A file whose size or time changed in the last 10 s is left for the next listing.
- The listing is only a trigger. Whether anything changed is decided by content (§4.3): touching a
  file, or rewriting it with the same bytes, changes nothing downstream.

### 4.3 Checking: a job like any other

Checking runs as a build step, `inputs`, one target per gate unit (`inputs regions`, `inputs
timetables/gtfs`, …), so it fits the pool unchanged (pool.md §2, principle 4: every job hands off):
- **Its key:** the step's version and the unit's checker version (`INPUTS_V`, and a version per
  input's checks, bumped with any change to them), the listing (names, sizes, times: a trigger),
  the acceptances for the unit (§4.5, by file name), the current accepted index's content name, and
  the content names of whatever else its checks read (the pass's outlines for regions, languages
  and the registers' territories; the accepted heritage registers for the matches). When the key
  changes, the target is stale and the lead grants it.
- **Where it runs:** any member (the steps table, pool.md §7.2: it needs the NAS and little
  memory; power on battery as any light job). The lead's own slot takes it when nothing else does.
  It's ordered before every other step, so a change is checked before the plan builds with stale
  inputs, and it holds no region and no round.
- **What it does:**
  1. Reads each file of the drop box and hashes it (XXH3, as content names are made: formats.md,
     Names and hashes). A file whose bytes are those of the accepted version is unchanged, whatever
     its time.
  2. Runs the shape checks on each changed file (parse, required fields, types, references), then
     the input's own checks (§6), which may read the whole candidate and the context inputs.
  3. Works out each changed file's verdict: **clean**, or **held** by its unaccepted findings
     (a finding about several files holds the changed ones among them; a removal is a change too).
  4. Writes the next accepted version: the previous accepted version with every clean change
     applied (new and edited files in, removed files out), the held files as they were accepted
     before (a held new file is simply absent). Each file is stored content-named under
     `sources/inputs/<unit>/` (§4.6) and uploaded as every output is (read back, checked against its
     hash, renamed into place: plan §3).
  5. Writes the candidate's report (its findings, and which files are held) and hands off both as
     records changes: `inputs/<unit>` → the new accepted index (only when it differs), and
     `inputs-held/<unit>` → the report, or none when nothing is held.
- **Derived facts** go in the index with each file (a register's box and entry count, a GeoTIFF's
  resolution and extent, a feed declaration's derived box once fetched): computed from the content
  at check time, so the steps that need them read the index instead of every file, and a fact is a
  function of the content, never typed by hand.
- **Deterministic:** the next accepted version is a function of the candidate's bytes, the
  previous accepted version and the acceptances. A re-run gives the same index.

### 4.4 Findings and their ids

- A finding's id is a hash of the check's name, what it's about by identity (the region ids, the
  register and entry id, the feed id), and, for a place, its geometry rounded to 100 m. Not of the
  whole file's bytes: an unrelated edit elsewhere in a file doesn't re-raise a warning already
  accepted, and the same gap seen again has the same id.
- **Errors** are the data's or the shape's faults: the file can't be read in the shape, a required
  field is missing or of the wrong type, an id repeats, a reference names nothing (an outline
  relation the pass doesn't have, a shape file that isn't there), a declared fact the data
  contradicts (a register's entries outside its declared territory by more than 20 km, a
  declared height reference the app doesn't know). Errors can't be accepted; the file is fixed or
  removed.
- **Warnings** are likely mistakes the owner may mean: a thin gap between regions, a register
  losing more than 10 % of its entries since its accepted version, a declaration no step will use.
- Findings about many lines of one file (a translation file's suspicious lines) are one finding
  per file and check, listing the lines (the first 50 in the banner, all in the report).

### 4.5 Accepting a warning

- **How:** the build page's banner has Accept beside each warning and Accept All for the unit's
  warnings (§4.7); the menu bar item and `scenic inputs accept <unit> <finding id>|--all` do the
  same. Errors have no Accept.
- **What it writes:** `state/inputs/accepted/<unit>/<finding id>.json`, made with create-new (pool.md
  §2, principle 3: written once, never changed), holding who accepted it (member id, host label),
  when, and the finding's message as it was. The asking member writes it itself; no round trip to
  the lead. The next listing changes the check's key, the check runs again, and the file is no
  longer held.
- **Undoing:** `scenic inputs unaccept <unit> <finding id>` removes the file; the finding holds its
  file again from the next check (if the file has moved on since, it holds the new change, never
  the version already in).
- Acceptances are backed up with `inputs/` (plan §3, Backups).
- An acceptance outlives the finding: if the same gap appears again later with the same id, it's
  already accepted. `scenic inputs` lists acceptances whose finding no candidate raises any more,
  and `scenic inputs unaccept --stale` removes them.

### 4.6 The accepted version and how steps read it

- **Files:** each accepted file is `sources/inputs/<unit>/<path>.<hash16>.<ext>` (its drop-box path,
  slashes kept, content-named as every built file: plan §3), so the same bytes are stored once
  however often they're dropped.
- **The index:** `sources/inputs/<unit>/index.<hash16>.json`:
  ```json
  {"fmt": 1, "unit": "heritage", "checks": "heritage 1",
   "files": {"fr-merimee.geojson": {"file": "sources/inputs/heritage/fr-merimee.4b1e…a0.geojson",
                                   "size": 81234567, "keyed": "9c03…e1",
                                   "facts": {"box": [-5.14, 41.37, 9.56, 51.09], "entries": 46210}},
             "fr-merimee.toml": {"file": "sources/inputs/heritage/fr-merimee.77d2…10.toml", "size": 512,
                                 "keyed": "-", "facts": {}}},
   "accepted": ["h-entries-dropped:fr-merimee"]}
  ```
  `keyed` is the digest of what affects builds (§5.1: a description file's credit and licence
  don't, so editing a credit reruns nothing); `accepted` lists the warnings the version was taken
  with.
- **The records:** the manifest's `inputs/<unit>` names the index (and each file's entry roots it
  for GC). It changes only by a check job's hand-off, merged by the lead (pool.md §5): the lead
  stays the one writer of the records, and the gate adds no other writer.
- **Readers:** one function in the pipeline (`inputs::open(root, records, unit)`) gives a step the
  accepted files of a unit, by their drop-box paths, from the records it planned with. No step reads
  a drop box (the guard of #146 enforces it, §7). The Python steps get the files' paths on their
  command line, as they get every input today.
- **Keys:** a step names the content of the input files it reads: the index's content name when it
  reads the whole unit, or the `keyed` digests of the files it reads when it reads some (the
  heritage-sites job names the registers whose box meets its cover, so a register on another
  continent changing reruns nothing there). This replaces today's digests of `inputs/` beside the
  manifest (`agent::input_digests`, crates/pipeline/src/agent/mod.rs:4638, which digests the
  recipes and the outline files they name by size and time: `regions_digest`, :4671).
- **The map's server** reads its inputs the same way: the translations and descriptions from the
  accepted files the lead's manifest (`state/build/manifest.json`) names, copied to the Mac as
  `livefolder` copies the folders today (crates/server/src/livefolder.rs), and away from the NAS
  from that copy; the regions' recipes (for the Regions panel's pending and held states) from both
  the accepted version and the drop box.
- **The catalog** records the accepted index of every unit its data was built from
  (`"inputs": {"<unit>": "<index content name>"}`), so what a published map was made from can be
  read back.

### 4.7 Where it shows

- **The status** (`state/status.json`, plan §8, Status): `inputs`, one entry per gate unit:
  ```json
  {"unit": "regions", "version": "sources/inputs/regions/index.7a…json", "state": "held",
   "checked": 1760100000, "held": ["wales.toml"],
   "findings": [{"id": "r-gap:…", "level": "warning", "files": ["wales.toml", "england.toml"],
                 "message": "A gap between Wales and England up to 340 m wide (2.1 km²): its roads would be left off",
                 "at": [-3.07, 52.31]}]}
  ```
  `state` is `ok`, `checking` (a check is granted or running) or `held`.
- **The menu bar item** (`tools/status`): today one icon shows the build's state, and a problem shows
  only when nothing builds (tools/status/main.swift:532). An input held adds a badge (a small
  warning triangle) to whatever icon the build's state has, so a held input is visible while the
  build goes on with the last good version. Its menu has a line per held unit ("Regions: 1 warning
  held"), with Accept (warnings only, after a confirmation naming them) and Show on the Build Page.
  It notifies when a unit becomes held and when it's taken in.
- **The build page** (`web/work/dash.js`): a banner per held unit at the top: the unit, the held
  files, each finding with its message, a link to the place on the map for a finding with one,
  Accept per warning and Accept All; errors say what to fix. While a unit is checking, a quiet line
  says so.
- **The map's build panel** (`web/src/ui/buildstatus.ts`): the same banners, compact. The Regions
  panel marks a held recipe "held: <finding>".
- **`scenic status`** prints a line per held unit; `scenic inputs` lists every unit's version,
  state, findings and acceptances.

### 4.8 Credits from the inputs

- Every input whose data reaches the map has a description (§5.1) with its `credit` and `licence`.
  The catalog's `credits` (formats.md, Catalog) are made from the accepted descriptions instead of
  `pipeline::rules::CREDITS` (crates/pipeline/src/rules.rs:179), with the same rule for which to
  list: those whose extent meets the coverage, 20 km around it, or a built unit's ways
  (`catalog_credits`, :449).
- **The extent is derived,** never typed: a register's box from its entries, a GeoTIFF dataset's
  from its files, a feed's from its stops, a tile pyramid's from its territory's outline, a source
  declared for the whole world (`extent = "world"`) as anywhere. Today's credit boxes
  (rules.rs:129–175) go.
- The map's own credits for what the app makes (its colour ramps, `web/src/ui/strip.ts:330`) stay
  the app's.
- The map server's fallback for catalogs without credits (`credits_of`,
  crates/server/src/main.rs:765, which reads `rules::CREDITS`) goes once no served catalog lacks
  them (GC keeps catalogs 14 days).

### 4.9 GC and backups

- Accepted versions are rooted by the manifest; an older accepted file goes 14 days after nothing
  names it (plan §3, GC), so a version from last week can be restored by hand.
- `sources/inputs/` is swept like other content-named folders; drop boxes and `state/inputs/` are
  never swept.
- Backups (plan §3) cover `inputs/` (every drop box, so translations and descriptions once moved)
  and `state/inputs/accepted/`. `todo/` isn't backed up (it's remade).

### 4.10 What #134 builds and how it's tried

- The `inputs` step, the listing thread, the index, the records entries, `inputs::open`, the
  acceptances, the status entry, the menu bar badge and lines, the build page's and the map's
  banners, `scenic inputs`.
- **A test input first:** `inputs/_gate-test/` isn't real data (a name starting with `_` is
  ignored everywhere else; the gate is told of it by a flag). Its shape: JSON lines
  `{"k": "<string>", "v": <number>}`; its checks: an error for a line that doesn't parse, a warning
  for a negative `v`. No build step reads it; its accepted version shows in the status. With it,
  #134 shows on the NAS, between two Macs: a drop taken in; an error held with the last good version
  kept; a warning accepted from the build page and from the other Mac's menu; an unaccept; a touch
  changing nothing; a removal held and accepted; the lead handed over mid-check. Then it's removed.
- **Tests** (plan §8, Tests: no wall clock, no real NAS): the verdicts as a function of (candidate,
  previous version, acceptances); finding ids stable across unrelated edits; a held new file absent,
  a held edit keeping the old bytes, a held removal keeping the file; the index the same twice.

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
| `territory` | list of ISO 3166 codes | yes | where it applies: checked against the data (§4.4) |
| `redistribute` | bool, default true | yes | false: its data never goes into the catalog's World download nor to a worker page (ODPT's raw feeds, plan §6) |
| `notes` | string | no | free text |

`credit` and `licence` are required (an error without them): a source without its credit would
break its terms. "Keyed" fields enter the file's `keyed` digest (§4.6); the others change what the
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
| `refresh` | string | `never` (pinned), `days:<n>` (fetched again once the copy is that old, as rail feeds are today), or `manual` (by `scenic sources refresh <id>`) |
| `[data.fetch]` | table | how: `method = "get"` (default), `"get-key-header"` (`header`, `key`: a name in `inputs/keys.env`), `"get-key-json-link"` (also `link`: the JSON path of the download link in the answer, as LTA's signed links), `"s3-list"` (a public bucket's prefix, listed), `"range"` (read in ranges where it lies, never downloaded whole: cloud GeoTIFFs) |

The fetch methods are protocols, not sources: generic code, one implementation each, used by any
declaration. Every fetch goes through the existing fetcher (`pipeline::fetch`, which asks before any
internet request: `every_internet_request_asks_first`, crates/pipeline/tests/cache_accessor.rs:195),
and the worker pages' proxy allow-list (`coord::WEB_HOSTS`, crates/pipeline/src/coord/mod.rs:446,
five hosts by name today) is derived from the declarations' hosts.

**Fetched data** goes to `sources/fetched/<declaration id>/…`, content-named, each version pinned in
the records by the job that fetched it; steps' keys name the pins (as the rail feeds' zips and the
terrain's raw tiles are pinned today). A refresh is a new pin, never a silent change.

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

**Kind:** declare. **Drop box:** `inputs/regions/` (as today).

**Shape:** a recipe per region, `<id>.toml`, as today (crates/pipeline/src/agent/recipes.rs), with
custom outlines in GeoJSON instead of `.poly`:

| Field | Type | |
|---|---|---|
| `id` | string: lower-case letters, digits, dashes, ≤ 64, the file's name | required |
| `name` | string | required |
| `outline` | list of entries, their union the region | required, non-empty |

Outline entries:
- `osm:<relation>`: an administrative or ISO 3166 area of the pass's outlines (preferred:
  neighbours share edges exactly, and a 1 km buffer covers coasts: plan §5).
- `shape:<file>.geojson`: a Polygon or MultiPolygon (a Feature, or a FeatureCollection whose union
  is taken) in `inputs/regions/`. Replaces `poly:` and `geofabrik:`.
- `place:<lon>,<lat>,<km>`: a circle up to 500 km (as today).

```toml
id = "lake-district"
name = "Lake District"
outline = ["osm:2654634", "shape:lake-district-fells.geojson"]
```

**Checks.**
- Errors: the TOML doesn't parse; `id` isn't the file's name or isn't valid; `name` or `outline`
  empty; an entry of an unknown kind; an `osm:` relation not in the pass's outlines; a shape file
  missing, not GeoJSON, not a polygon, or invalid; a circle out of range; two recipes with one id.
- Warnings:
  - **a thin gap between neighbouring regions:** the coverage's closing at 1 km (grown by 1 km,
    then shrunk by 1 km) less the coverage, each piece that touches two or more regions' shapes
    and is at least 0.01 km²: a sliver whose roads would be left off. Overlaps are harmless (the
    coverage is a union; nothing is built per region) and aren't checked. A hole inside one region
    (touching no other) isn't a gap: it's that region's own outline;
  - a `.geojson` in the folder that no recipe names.
- A `.toml.removed` file is a removal the owner made (the panel's), not a finding.

**Ingesting and rebuilding.** The accepted recipes are the coverage. Jobs are keyed by the
coverage's geometry inside what they read (`Coverage::fingerprint`, plan §5), so nothing about keys
changes; what changes is that the coverage comes from the accepted version (`Coverage::load`,
crates/pipeline/src/coverage.rs:514, reads `inputs/regions` today, as do about twenty other call
sites: agent/mod.rs:1947, :1954, :3534, :4119, :4162, :5859; terrain_task.rs:942; treepacks.rs:629;
scenic-build.rs:1130, :1283, :1441, :1577, :1730, :1834, :2672; bin/terrain.rs:306), and a shape
file enters by its content (a `shape:` entry's key is its geometry, as `osm:` entries' are). The
regions' wait for more edits (plan §8, Regions: a quarter of an hour, an hour at most) counts from
when a change is taken in. A bad recipe today is dropped from the coverage and reported
(`recipes::load`'s second list, the status's `bad_recipes`, agent/mod.rs:373); on the gate it's
held instead, its accepted version kept, so a typo can't shrink the map.

**Migration.** Today all 88 recipes use `osm:` entries alone (checked on the NAS, 2026-10-10:
`inputs/regions/*.toml`; two more are `.toml.removed`), so no outline is converted:
1. Put the regions on the gate, the readers above on `inputs::open`. Proof: the coverage's
   fingerprint and shapes key (`Coverage::shapes_key`) of the accepted version equal today's, so
   every job's key is unchanged and nothing rebuilds.
2. Delete `poly:` and `geofabrik:` (recipes.rs:28–55, coverage.rs's `file_rings`, `regions_digest`'s
   outline files, server/src/regions.rs:686) and move `inputs/outlines/` (31 Geofabrik `.poly`
   files and three of ours: `azores`, `madeira`, `portugal-mainland`) to `inputs/regions/how/` as
   history; catalogs record their outlines' shapes inline (formats.md, Catalog `coverage`), so an
   old catalog naming `geofabrik:` still draws.
3. The Regions panel's planned drawing (plan §1) saves `shape:` GeoJSON.
4. Run the thin-gap check over today's recipes: no gap is expected (OSM neighbours share edges), and
   any found is the first real use of warning-and-accept.

### 6.2 Languages by territory (#136)

**Kind:** declare, generated once and rarely touched. **Drop box:** `inputs/languages/`.

**Shape:** `territories.tsv`, one territory a line, `<code>\t<language>,<language>…`, most spoken
first:
- `<code>`: an ISO 3166-1 alpha-2 code (`FR`) or an ISO 3166-2 subdivision (`CA-QC`);
- languages: ISO 639 base subtags (`fr`, `yue`), as `names::spoken::Lang` parses them.
- A subdivision's line refines its country's where they overlap (today's `REFINED`); the smallest
  outline wins as today.
- Lines starting with `#` are comments.

Plus `about.toml` (§5.1): CLDR's version, the Unicode licence, its credit. The one rule kept in
code, Chinese read as Cantonese where Cantonese is spoken and Mandarin isn't (`READ_AS`,
crates/names/src/spoken.rs:93), is a rule of the names' lookup, not data about a territory: it stays.

```
# CLDR 47 territoryInfo, official and de facto official languages, then the refinements
CA	en,fr
CA-QC	fr,en
ES-CT	ca,es
HK	yue,en
```

**Checks.** Errors: a line that isn't two tab-separated fields; a code that isn't ISO 3166 syntax; a
language that isn't a valid subtag; a code twice; an empty list. Warnings: a code the pass's
outlines don't have (it would have no effect); a subdivision whose country has no line.

**Ingesting and rebuilding.** The `spoken` job (plan §7) is keyed on the outlines and
`spoken::RULES` (crates/names/src/spoken.rs:28) today; its key names the accepted table instead of
the table's part of RULES (RULES stays, for the raster's code and format). `global/spoken` already
carries the table in its head (spoken.rs:331), so the map's server needs the input only for its
fallback (a catalog without `global/spoken`), which reads the accepted table. `names-todo` follows
through `global/spoken`.

**Migration.** Write `territories.tsv` from today's `crates/names/src/territory-languages.tsv`
(`include_str!`, spoken.rs:98) and `REFINED` (spoken.rs:69); proof: `global/spoken` comes out with
the same content name. Then delete the embedded table, `REFINED` and the `is_refined` test that
reads it (spoken.rs:158, made table-driven), and move `tools/names/cldr-languages.py` to
`inputs/languages/how/`.

### 6.3 Translations (#137)

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
finding per file listing its lines.

**Ingesting and rebuilding.** Nothing is rebuilt, as today: the map's server loads the accepted
files (§4.6). A language's version becomes a hash of the content names of the accepted files holding
lines in it (today: paths, sizes and modification times, plan §7, Versions), so tiles' ETags change
only with content. `names-todo` names the translations' accepted version in its key. The time from
a drop to the map grows from about a minute and a half to the next listing plus a check (a few
minutes; §12).

**Migration.**
1. Move `translations/` to `inputs/translations/` (one rename on the share; the server reads both for
   one published app).
2. Check today's files on the gate. The converted tables (2.78 M lines) are expected to raise
   check.py warnings; they're accepted once, at migration, each listed in the task's report.
3. Proof: the server's compiled tables (`names::table`) hold the same lines (the same lookups for
   every name of a sample of tiles, and the same count per language).
4. Delete the server's direct read of the NAS folder and the old-format skipping.

### 6.4 Descriptions (#137)

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

**Migration.** As translations: the move, the check (today's `descriptions/heritage/` files), the
proof (every popup's description the same for the things in the files), then the direct read goes.

### 6.5 Heritage registers (#138)

**Kind:** declare. **Drop box:** `inputs/heritage/`, one pair of files per register:
`<register>.geojson` and `<register>.toml`. The register's id is the file's stem (`fr-merimee`,
`ca-dfhd`, `unesco-whc`).

**Shape: `<register>.toml`,** a description (§5.1), plus:

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `class` | `heritage`, `world-heritage`, `special` | yes | how the map shows its entries: heritage sites and areas; UNESCO's World Heritage (dots, and outlines from OSM); special places (biosphere reserves, geoparks, dark-sky places) |
| `territory` | list of ISO 3166 codes | yes | required; entries further than 20 km outside are an error (§4.4) |
| `wikidata_property` | string `P<digits>` | yes | the Wikidata property holding this register's ids, by which entries are matched to Wikidata items (today `heritagewd.RULES`, dem/heritagewd.py:59) |
| `osm_ref_tags` | list of strings | yes | the OSM tags holding its ids (today `heritagewd.OSM_REF`, dem/heritagewd.py:168: `ref:mhs`, `HE_ref`…) |

**Shape: `<register>.geojson`,** a FeatureCollection; each feature any geometry (Point, LineString,
Polygon, or their Multi forms), with properties:

| Property | Type | | Meaning |
|---|---|---|---|
| `id` | string | required, unique in the register | the register's own reference (stable across downloads) |
| `name` | string | required | as the register gives it, in its language, tidied by the producer |
| `name_en` | string | optional | the register's own English: a source of the thing's own English (plan §7) |
| `level` | integer 1–5 | required for `heritage` | 1 World Heritage, 2 highest national grade, 3 other national grades, 4 regional, 5 local (today's levels, dem/heritage.py:5) |
| `designation` | string | required for `heritage` | the register's designation ("Monument historique classé") |
| `kind` | string | required for `special` | `biosphere`, `geopark`, `dark_sky` |
| `wikidata` | string `Q<digits>` | optional | its item, where the register or the producer knows it |
| `date` | date | optional | designated |
| `municipality`, `type`, `authority`, `url` | string | optional | shown in the popup |
| `approx` | bool | optional | located loosely (by name, or a circle for a special place without a mapped boundary) |
| `area_km2` | number | optional, `special` | its designated area, for the circle drawn when OSM has no boundary |
| `dot` | bool | optional | whether a polygon or line also shows as a dot (default: true for points, false otherwise) |

```json
{"type": "FeatureCollection", "features": [
  {"type": "Feature", "geometry": {"type": "Point", "coordinates": [2.3376, 48.8606]},
   "properties": {"id": "PA00088801", "name": "Palais du Louvre", "level": 2,
                  "designation": "Monument historique classé", "wikidata": "Q19675",
                  "municipality": "Paris", "url": "https://www.pop.culture.gouv.fr/notice/merimee/PA00088801"}}]}
```
```toml
name = "Mérimée, monuments historiques"
what = "France heritage"
credit = "Ministère de la Culture, base Mérimée (POP)"
licence = "etalab-2.0"
source = "https://www.pop.culture.gouv.fr/"
retrieved = 2026-09-14
class = "heritage"
territory = ["FR"]
wikidata_property = "P380"
osm_ref_tags = ["ref:mhs"]
```

Every entry has a position when it's dropped: locating (Canada's federal designations, by
`dem/federal.py` today) is the producer's job, never the app's.

**Checks.**
- Errors: invalid GeoJSON or geometry; a coordinate out of range; a required property missing or of
  the wrong type; an `id` twice; a `level` outside 1–5; a description missing, or without its
  credit or licence; an unknown `class`; a malformed `wikidata` or `wikidata_property`; entries more
  than 20 km outside the declared territory's outline (from the pass's outlines); a `.geojson`
  without its `.toml` or the reverse.
- Warnings: a register losing more than 10 % of its entries since its accepted version; more than
  half its entries `approx`.

**Ingesting and rebuilding.**
- **One generic reader** replaces the function per register: the heritage-sites job (plan §6, Job
  keys; docs/phase5.md, Heritage and area flags) reads every accepted register whose box meets its
  cover, takes points as sites, polygons as heritage areas (flagging roads as today), lines as
  lines, each register's `class` deciding which layer.
- **Merging across registers stays a build step, by rule** (the owner's line: rules in the build,
  judgment as an input): two entries are one place when they share a `wikidata` item, or when the
  lower-graded one is local (level 5) and lies within 40 m of another register's entry of the same
  or a higher level whose name shares a word of three letters or more (today's `dedupe`,
  dem/heritage.py:319, unchanged). The merged place keeps every register it came from (for credits
  and the popup), its level the highest. Registers are read in the order of their ids, so the result
  doesn't depend on the drop box's listing order.
- **The judgment calls** come from the matches input (§6.5.4), applied before the rules.
- **Keys:** heritage-sites names the `keyed` digests of the registers meeting its cover and the
  matches' accepted version (today it names the whole snapshot, `sources/registers/legacy`,
  crates/pipeline/src/agent/build.rs:609); the heritage job likewise. The units follow through
  their heritage slices, as today.
- **Credits** from each register's description, for the registers whose entries meet what the
  catalog serves (§4.8).

**Migration.**
1. **Producers outside the app** convert each of today's registers from the snapshot
   (`sources/registers/legacy`, the folder `dem/heritage.py` reads) into its pair of files: the
   function per register in `dem/heritage.py` (`federal`, :156; `nrhp`, :166; `quebec`, :197;
   `ontario`, :244; `nova_scotia`, :277; `new_brunswick_moncton`, :301; `crhp`, :314; the special
   places, :352 and :392) and in `dem/heritage_eu.py` (`unesco`, :270; `france`, :325; `andorra`,
   :372; `england`, :414; `scotland`, :476; `wales`, :506; `northern_ireland`, :547; `guernsey`,
   :575; `ireland`, :701; `spain`, :735, split into its eight regional registers; `portugal`, :885;
   `hong_kong`, :918; `japan`, :996; `taiwan`, :1072; `singapore`, :1137), with `federal.py` and
   `crhp.py`, move to `inputs/heritage/how/`, each writing the shape. Their credit strings
   (`QC_SRC`, `WHC_SRC`, … and `rules::CREDITS`' heritage entries, rules.rs:235–361) become the
   descriptions; `heritagewd.RULES` and `OSM_REF` become `wikidata_property` and `osm_ref_tags`;
   `heritagetiers.py`'s patterns on designation strings (dem/heritagetiers.py:15–36) become the
   producers' `level`s.
2. The heritage-sites and heritage jobs read the registers generically.
3. **Proof:** the heritage-sites outputs (`work/heritage/<d>/base/*`, the `pos/` and `areas/` slices),
   the heritage job's outputs, the marks and the overlays the same, or every difference explained
   (expected: none in positions; the merge's order may change which register a merged place's popup
   names first, to be shown and explained).
4. **Then deleted:** the per-register code (the functions above, `heritage_eu.SOURCES`, the
   register order at dem/heritage.py:561, `STATES`, the snapshot's paths), the registers' credits and
   territory boxes in rules.rs (:131–175, :235–361), `registers-import` (scenic-build.rs:2440) and
   `sources/registers/legacy` (after GC's window).

#### 6.5.1 What stays generic in the heritage chain

The chain's other scripts (heritagewd.py, heritagedetails.py, areadetails.py, whsshapes.py,
filterprops.py, pageviews.py, interest.py, layers.py: docs/phase5.md) are generic over OSM,
Wikidata and the registers once they read the registers' fields instead of per-register knowledge:
whsshapes.py matches World Heritage outlines by the `unesco-whc` register's `wikidata_property`
(P757) and `osm_ref_tags`; heritagewd.py's NRHP special case (dem/heritagewd.py:293–310, reading
`nrhp.json` for reference numbers) becomes the register's own `id`s. The languages they guess by
location (`heritage_eu.lang_at`, dem/heritage_eu.py:178; `names.REGIONS`, dem/names.py:36) read the
languages input's raster (`global/spoken`) instead.

#### 6.5.2 The heritage chain's stand-in root

The chain runs in a stand-in root laid out as the old repository (`data/build`, `data/heritage`:
scenic-build.rs:2573, :2636; docs/phase5.md). With the registers given on the command line, the
layout's `data/heritage` goes; the rest of the stand-in (a scratch folder per run) stays until the
scripts take paths for everything, which is cleanup, not this series.

#### 6.5.3 The frozen seeds retired (#139)

`sources/registers/legacy-seeds` (scenic-build.rs:2232) holds three things nothing can make again
(plan §6, Hand-made inputs; §10). None is an input; each becomes made:
- **The English names table** (`names/english.json`, read by `dem/names.py` `english_at`): the
  heritage layers' English comes from the live translation tables and the things' own English (the
  registers' `name_en`, Wikipedia titles), through the same lookup the server uses (plan §7). Proof:
  the layers' `en` the same, or each difference explained (an older table's line no longer there is
  expected; it goes on the to-do list instead).
- **Park facts** (`areas/wikidata.json`, areadetails.py's cache): asked by the heritage job from
  Wikidata like its other answers, kept as a pass's answers (`pipeline::answers`, as the items job's).
- **Pageview months** (`pageviews/months/`): the items job's months (they already are the
  chain's source for new months, plan §6).
Then the seeds' extraction (scenic-build.rs:2232, :2290–2311) and the archive go.

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

**Ingesting.** Applied before the rules: `same` pairs merge (transitively), `same: false` pairs are
never merged by any rule. The matches enter heritage-sites' and heritage's keys by their accepted
version. Today there are none; the to-do list of likely matches the rules didn't take (near, a
similar name, different registers) is a later request–fulfil addition, not in this series.

### 6.6 (reserved)

### 6.7 Timetables (#140–#142)

One input worldwide, in two gate units: feeds the app fetches (source declarations) and static GTFS
the owner compiles by hand (declare). OSM decides what's drawn and where; timetables only add
counts. Rail and ferries are counted by one counter from GTFS, whichever unit it came from.

#### 6.7.1 Feeds (#140)

**Kind:** source declarations. **Drop box:** `inputs/timetables/feeds/`.

**A feed:** `<id>.toml`, a description (§5.1) and:

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `[data] format` | `"gtfs"` | yes | |
| `[data] url` | string | yes | the zip's URL, or the API's for a keyed feed |
| `[data.fetch]` | table | yes | `method` and its fields (§5.2): `get-key-json-link` with `header = "AccountKey"`, `key = "LTA_ACCOUNT_KEY"`, `link = "value[0].link"` for LTA's |
| `[data] refresh` | string | yes | `days:7` by default: fetched again once the copy is a week old and out of date, as today (plan §6, Rail service) |
| `replaces` | list of feed ids | yes | feeds left out while this one has a zip (an operator's own over copies) |
| `operator` | string | no | shown |

No country and no box: a feed is used where its stops are (its box read from `stops.txt` by range
requests, as `routes.txt` is read today, and kept in the index's facts once fetched).

```toml
name = "SNCF Voyageurs (TGV INOUI, OUIGO, Intercités, TER)"
what = "Rail service frequency"
credit = "SNCF Voyageurs"
licence = "Licence Ouverte 2.0"
source = "https://ressources.data.sncf.com/"
[data]
format = "gtfs"
url = "https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip"
```

**A catalogue:** `<id>.toml` with `format = "mdb-catalogue"` (the Mobility Database's
`feeds_v2.csv` columns, a published format read generically), its `url`, `refresh = "manual"` (today
it was downloaded once by hand; the planned six-monthly refresh, plan §8, is `days:180`), and the
corrections that today are hand lists in `dem/railfeeds.py`:

| Field | Type | Meaning |
|---|---|---|
| `exclude` | table id → reason | feeds left out (today `ELSEWHERE`, railfeeds.py:98: the MTR's lines, which come from static GTFS) |
| `country` | table id → ISO code | feeds the catalogue files under another country (today `COUNTRY`, :110) |
| `replaces` | table id → ids | copies of one timetable (today `REPLACES`, :115) |

The catalogue's feeds are chosen as today (the countries the coverage is in, by the pass's outlines;
the feeds' boxes; no key needed: plan §6), a generic rule over the catalogue's fields.

**Checks.** Errors: a declaration that doesn't parse; an unknown format or fetch method; a `key`
that isn't a valid name; `replaces`, `exclude` or `country` naming a malformed id; a catalogue's
`country` value that isn't ISO 3166. Warnings: `replaces` or `exclude` naming a feed neither
declared nor in the catalogue's fetched copy; a `key` that `inputs/keys.env` doesn't hold (the feed
waits, as keyed feeds do today); two declarations with one URL.

What a fetch brings back (a zip that isn't one, a feed without rail) isn't a gate finding: the data
isn't the owner's drop. It's the rail-feeds job's status, as today (a refusal leaves the feed out,
an old copy is kept: the last good version already).

**Ingesting and rebuilding.** `rail-feeds` (plan §6) reads the accepted declarations and the
catalogue's pinned copy; its key names the declarations' `keyed` digests instead of the code's
lists, plus what it names today (the coverage, the outlines, the key names held: agent/build.rs:1696).
`rail` and `ferries` name the fetched zips by content, as `rail` does today.

**Ferries' feeds** go the same way: `inputs/ferries/gtfs-feeds.json`'s 40 verified feeds become
declarations, and their sailings are counted by the same counter as rail (today `dem/gtfs.py` counts
them by hand into `inputs/ferries/freq/gtfs-*.json`, which the ferries job reads,
crates/pipeline/src/ovconv.rs:384). Where today's counts pin a GTFS route to an OSM line by hand
(`gtfs_routes` in those files), the declaration carries the pins: `[match] "<route_id>" = "r14098916"`
(OSM ids, checked by #142's report each pass, §6.7.3).

**Counting** (one counter, generic GTFS): a feed's trips on its typical weekday, the median-busy
Tuesday to Thursday from 30 days before the day its zip was fetched to 90 days after, as rail counts
today (`dem/railgtfs.py`); a service that runs no Tuesday to Thursday counts on the days it runs,
with the days shown (ferries' weekend lines, which today's ferry counts give as `days`). A trip in
two feeds counts once.

**Migration.**
1. Declarations written from `railfeeds.py`'s `EXTRA` (:79: SNCF, two of Renfe's, Great Britain's,
   Hong Kong's trams), `KEYED` (:102: LTA), and the catalogue's corrections; the catalogue's pinned
   copy (`sources/rail/catalogue`) becomes its first fetched pin; `rail::FEED_KEYS`
   (crates/pipeline/src/rail.rs:54) is derived from the declarations' keys. The ferries' 40 feeds.
2. Proof: rail-feeds' feed list and fetched zips the same, `global/railfreq` the same; the ferries'
   sailings per line the same, or explained (the shared counter's weekday rule against `gtfs.py`'s).
3. Then deleted: the lists in railfeeds.py (:79–115) and its `keyed_link` LTA branch (:417), the
   test reading them (rail.rs:459), `FEED_KEYS`, the `gtfs-*.json` files and `gtfs.py` (moved to
   `how/` if still useful for finding feeds).

#### 6.7.2 Static GTFS (#141)

**Kind:** declare (and the fulfil half of §6.7.3's to-do). **Drop box:** `inputs/timetables/gtfs/`,
`<name>.zip`, each a GTFS feed compiled by hand from timetables found anywhere.

**Shape:** GTFS (the reference's required files: `agency.txt`, `stops.txt`, `routes.txt`,
`trips.txt`, `stop_times.txt`, `calendar.txt`; `feed_info.txt` naming the publisher), with:
- `frequencies.txt` allowed: a trip plus "every 600 s from 06:00 to 23:00" instead of each departure;
- **no expiry:** static files are counted without dates. A service's days come from `calendar.txt`'s
  weekday columns; its start and end dates and `calendar_dates.txt` are ignored. (Give a wide range,
  20000101–20991231: GTFS requires one.)
- **each route's source page** in `routes.txt`'s `route_url` (required here, an error without it),
  and when it was checked in an extra column, `checked` (GTFS lets producers add columns, which
  consumers ignore);
- **an optional OSM pin** per route, an extra column `osm` (`r14098916`, `w23471991`), for a route
  that matching by stops wouldn't find (a ferry whose terminals are far from OSM's ends). The
  description (§5.1) is `feed_info.txt`'s publisher plus `<name>.toml` beside the zip when the
  sources need a credit line beyond each route's page.

**Checks.** Errors: not a zip; a required file missing; references broken (a trip's route, a stop
time's stop or trip, a frequency's trip); times that don't parse; a `route_type` GTFS doesn't define;
a headway not positive; a route without `route_url`; a stop's coordinates out of range. Warnings: a
route with no trips on any day; two zips with one `route_id` and agency.

Whether a route matches something on the map isn't a gate finding: it depends on OSM, which changes
each pass, and a warning there would hold the file again every pass. It's #142's report.

**Ingesting.** `rail` and `ferries` count the accepted zips with the same counter, and name them by
content in their keys.

**Migration.**
1. **The MTR:** `sources/rail/mtr` (`mtr.json`, researched by hand) becomes `mtr.zip`: its stations'
   positions from OSM (as `dem/mtrpairs.py` takes them today), a route per line, `frequencies.txt`
   from its frequencies. The catalogue's `exclude` keeps Just Use Wheels' copy out, as today.
2. **The ferries' looked-up timetables:** `inputs/ferries/freq/timetables-*.json` (21 batches, each
   line `{"line": "w23471991", "per_day": 10, "days": "daily", "source", "url", "checked"}`) become
   zips with each line's `osm` pin, its terminals as stops, its sailings as trips on its days, its
   page as `route_url`.
3. Proof: trains a day on the MTR's lines and sailings on every ferry line the same, or explained.
4. Then deleted: `dem/mtrpairs.py`, `sources/rail/mtr` and `mtr-pairs` (rail.rs:37–42), the rail
   job's merging of the pairs (scenic-build.rs:3980), the `timetables-*.json` path in `ferries.py`;
   `inputs/ferries/research/` moves to `inputs/timetables/gtfs/how/`.

#### 6.7.3 The to-do list and the report (#142)

- **The to-do:** `inputs/timetables/gtfs/todo/lines.jsonl`, written after each build: OSM's rail and
  ferry lines in the coverage that no timetable covers, each
  `{"osm": "r123", "kind": "rail"|"ferry", "name", "ref", "operator", "from", "to", "km", "priority"}`,
  by priority (length, and whether it's a passenger line by its tags), beside a brief
  (`todo/README.md`: how to compile static GTFS for a line, what to cite). The owner answers with
  zips in `inputs/timetables/gtfs/`.
- **The report:** timetable routes that match nothing on the map (a feed's line OSM lacks, or a line
  that closed; a static route whose pin names no line of this pass), listed on the build page and in
  `scenic status`, never drawn. It isn't a held input: nothing waits on it.

### 6.8 Elevation (#143, #144)

**Kind:** source declarations, a dataset each; a small dataset's files may be dropped beside its
declaration. **Drop box:** `inputs/elevation/`: `<dataset>.toml`, and `<dataset>/` for local files.

**Shape:** a description (§5.1) and:

| Field | Type | Keyed | Meaning |
|---|---|---|---|
| `surface` | `ground` or `surface` | yes | bare earth (a DTM), or with trees and buildings (a DSM) |
| `heights` | string | yes | the height reference: `EGM2008`, `EGM96`, `CGVD2013`, `NAVD88`, `JGD2011`, `TWVD2001`, `ellipsoid-WGS84`… |
| `rank` | integer | yes | optional: overrides the order by resolution (lower first); needed where datasets of one resolution differ in quality (GSI's lidar over its photogrammetry) |
| `territory` | ISO codes | yes | required for a tile pyramid (its coverage can't be read from a header: tiles are asked for only inside it); optional for GeoTIFFs, checked against their extents |
| `[data] format` | `geotiff` or `tiles` | yes | |
| `[data] urls` / `index` / `files` | | yes | GeoTIFFs by URL (cloud-optimised ones read in ranges), a tile index (GeoJSON of footprints with URLs, made by a producer from the source's listing), or files dropped beside it |
| `[data] encoding` | `terrarium`, `terrain-rgb`, `gsi-png` | yes | for `tiles`: the encodings are formats, decoded by generic code (today Terrarium in `roadcore::grid`, GSI's PNG in `pipeline::dem::gsi`) |
| `[data] url`, `zoom` | template, integer | yes | for `tiles` |
| `[data] read_resolution` | metres | yes | optional: read a finer dataset at a coarser overview (HRDEM's 2 m read at its 8 m overview today) |
| `[data] valid` | GeoJSON file beside it | yes | optional: where the dataset is trusted, when that isn't all of it (AWS's tiles south of 60°N: north of it they mix an ellipsoidal source, plan §6) |

Read from the data at check time, never declared: each GeoTIFF's resolution, extent, CRS and no-data
value (its header, by range requests); a tile pyramid's resolution from its zoom and latitude.

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

**Checks.** Errors: an unknown `surface`, `heights` or encoding; a GeoTIFF whose header can't be read
or whose CRS the app can't project (today's projections: `pipeline::dem::proj`); a tile template
without its placeholders; files named that aren't there; an index that doesn't parse. Warnings: a
dataset whose files don't meet its declared territory; two datasets of one resolution and no `rank`
between them meeting (their order would fall to their ids).

**Ingesting.**
- **The order at a point:** the datasets with valid data there, by `rank` where given, else
  resolution, finest first, ties by id.
- **Blended at the edges:** within a band of four of the coarser dataset's pixels inside the finer
  one's valid area (its no-data or its coverage's edge), the two blend by a smoothstep of the
  distance, as GLO-30 blends into AWS's tiles by latitude today (plan §6), so a seam shows no step.
- **Heights:** the references the app knows are converted to EGM2008: the geoid-based ones above
  within a metre of it (EGM96, CGVD2013, NAVD88, JGD2011, TWVD2001) as they are, the blend hiding
  the offset; ellipsoidal heights through a geoid grid declared as its own source (§6.9). A
  reference the app doesn't know is an error, not a guess.
- **Ground over surface** at equal rank and resolution; a surface model ranks over a ground one only
  by `rank` (GLO-30 over AWS's tiles north of 60°N, where those are wrong).
- **FABDEM** is declared with `extent = "world"` and the coarsest rank: the worldwide fallback.
- **Keys:** a unit names the `keyed` digests of the datasets whose extent meets its reach (and the
  fetched files' pins for datasets downloaded whole, as FABDEM's tiles are kept today), replacing
  the DEM rules' versions by area (`rules::RULES`' `dem-*`, crates/pipeline/src/rules.rs:33–41,
  `DEM_RULES`, :60, `dem_rules_of`, :65). Adding a dataset reruns exactly the units it meets.
- **Densification** follows the data: today 8 m in North America and Japan and 15 m elsewhere by
  boxes (crates/pipeline/src/bin/extract.rs:1083–1087, duplicated in reach.rs:57), because those are
  where finer DEMs are; it becomes 8 m where a dataset of 10 m or finer meets the way, else 15 m.

**#143, the roads' elevations.** Declarations for HRDEM, 3DEP, MRDEM, GSI's five layers and FABDEM,
written from today's code (dem/mod.rs:40–58, dem/gsi.rs:11–15, dem/fabdem.rs:16, and the HRDEM tile
list and index embedded from `dem/hrdem_2m_tiles.txt` and `dem/hrdem_tile_index.geojson`,
dem/mod.rs:46–47, which become HRDEM's `index`, made by a producer from NRCan's listing). The `elev`
program reads the declarations through one generic sampler; the per-source order (dem/mod.rs:364–471)
goes. Proof: every unit's per-vertex elevations the same away from the datasets' edges, and the
differences at the edges (now blended) and from densification (Canadian units outside HRDEM's
coverage go from 8 m to 15 m) shown and explained. Then deleted: the per-source code and the DEM
rules and credits in rules.rs (:187–204). Taiwan's MOI DTM code is already gone; its stale
mention (crates/pipeline/src/bin/elev.rs:6) and rule (`dem-taiwan`, `meets_taiwan`, rules.rs:38,
:92) go with the DEM rules, the re-keying of old keys (agent/rekey.rs) keeping what it needs.

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
| Planetiler and its data | `sources/basemap/` (jar, Natural Earth, water polygons, lake centrelines), pinned by `water::PLANETILER_VERSION` (crates/pipeline/src/water.rs:54) | `planetiler.toml` (`format = "jar"`, `version`), `natural-earth.toml`, `water-polygons.toml`, `lake-centerlines.toml`: pinned (`refresh = "never"`), files as kept today |
| ESA WorldCover | `landcover.rs:22` | `format = "geotiff"`, `url` template, `version = "v200 2021"`; its class table (landcover.rs:26) is the format's legend, generic |
| Meta & WRI canopy height | `trees/mod.rs:54` (`CHM10_URL`), the `lat=-0.0` naming quirk (:122) | `format = "geotiff"`, an `index` made by a producer from the bucket's listing, so no naming rule lives in code |
| Leaf type, Europe (EEA HRL) and North America (NALCMS) | `dem/leaftype.py:48–63` | two declarations; their reading (an ArcGIS export, a zip read in ranges at a fixed offset, leaftype.py:51–52) moves to a producer that makes the squares (`sources/trees/leaf/`), declared as files |
| Overture buildings | `buildtiles::RELEASE` (crates/pipeline/src/buildtiles.rs:22), `dem/buildings.py:68`, `dem/bldfetch.py:78` | `format = "geoparquet"`, `version = "2026-09-23.1"`, the bucket prefix |
| GHSL building heights | `bld/sources.rs:19`, `dem/bldfetch.py:80–85` | `format = "geotiff"`, its tile grid as an `index` |
| Microsoft's estimated heights | `bld/fill.rs:28` (`ESTIMATES`, a source name inside Overture's data) | a field of the Overture declaration: `estimated_sources = ["Microsoft ML Buildings"]` |
| Wikidata | `dem/heritagewd.py:51` (`WDQS`, QLever), the items job | `wikidata.toml`: the endpoint |
| Wikipedia pageviews | `dem/pageviews.py:56` (`DUMP`) | `pageviews.toml`: the URL template; the months chosen stay the items job's rule (pageviews.py:55, :61) |
| Fonts | `scripts/fonts.sh:6` (Protomaps' assets) | kept: the app's own assets, built into it, not map data |
| Canada's lake fill (#124, CanVec) | not on `main` (an experiment; `sources/canvec/` on the NAS) | declared when #124 lands |
| GEBCO, NCEI's coastal relief and Great Lakes bathymetry | on the NAS (`sources/gebco`, `ncei-crm`, `ncei-greatlakes`), nothing on `main` reads them | none until a step reads them |

Each with its proof (the step's outputs the same) and the code's constant deleted.

## 7. The guard (#146)

A test that fails when a data source's name or URL appears in the app's code, gating publish as the
cache accessor's does:
- **Modelled on `crates/pipeline/tests/cache_accessor.rs`:** a marker list, an exemption list (file,
  function, reason), each exemption required to name a function that exists
  (`every_exemption_names_a_function_there`), and `tools/app/publish.sh:36–38` running it with
  `--test`, so a failure stops publishing.
- **What it scans:** `crates/**/*.rs` (outside tests), `dem/**/*.py`, `web/src/**/*.ts`, `tools/`
  and `scripts/` (outside `tools/taskboard/`, which is the board's, not the map's).
- **Markers:** URLs (`http://`, `https://`, `s3://`) except localhost, tailnet and documentation
  hosts (`example.org`) and links to the formats' specifications in comments; the hosts and names of
  every source this inventory lists (§9), and those of any declaration in the drop boxes (read from
  the NAS when it's there, from a list committed beside the test otherwise).
- **Allowed:** formats and protocols (GTFS, GeoTIFF, Terrarium, PMTiles, the Mobility Database's
  catalogue columns), OSM itself (its tags are the map's schema, and OSM links on the map open the
  object: web/src/details.ts:114), links that open a place in another viewer (the G, M and O keys:
  web/src/main.ts:1725–1734) and Wikipedia and Wikidata links built from an item (they open what the
  data names, web/src/details.ts:102, :260).
- **A last sweep** with it: every hit fixed or exempted with its reason.

## 8. How each input is made (#147)

The second half of the owner's request: each drop box's `how/` holds how its input is made, so the
map's inputs can be made again from scratch:
- **Producers:** the per-source scripts this series moves out (§9's P rows), each runnable on its own
  (`uv run python how/<script>.py --out …`), writing the drop box's shape.
- **Agent-made inputs:** the briefs and the models used (translations: Haiku; descriptions: Sonnet;
  the ferries' researchers' `PROMPT.md`), with how a batch is piloted and checked.
- **Hand-made inputs:** what the owner did (the MTR's research, the registers downloaded by hand).
- Then `inputs/README.md` (the drop boxes, the kinds, the gate), plan.md (§3's tree, §6's Hand-made
  inputs replaced by a pointer here, §7's folders), formats.md (the index, the status's `inputs`, the
  catalog's `inputs`) and the diagram's curated inputs brought up to the drop boxes.

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
  `dem/bldmeasure.py`: a one-off analysis, hand-run: P (`how/`).

### 9.5 Heritage [#138, #139]

- `dem/heritage.py:69` `NRHP`, `:70` `STATES`, `:156–314` the function per register, `:193`
  `QC_SRC`, `:241` `ON_SRC`, `:352` `special_wikidata`, `:381` `LOCAL_NAMES`, `:392`
  `special_official`, `:561–564` the register order: P (producers), credits D.
- `dem/heritage.py:319` `dedupe`: G (the merging rule); `:101` `sparql`: G.
- `dem/heritage_eu.py:270–1137` the functions per register (listed in §6.5), `:593`, `:1156`
  `SOURCES`, and the `*_SRC` credit strings beside each: P, credits D. `:77`, `:101`, `:149`,
  `:610–684` (fetching, ArcGIS paging, WFS, shapefiles, ODS): P (move with the producers).
  `:178` `lang_at`: G (reads the languages input).
- `dem/federal.py` (whole; `:42` `TYPES`, `:49` `DFHD_URL`) and `dem/crhp.py` (whole; `:36` `BASE`,
  `:38` `PROVINCES`, `:50` `CREDIT`), both run by hand: P.
- `dem/heritagewd.py:51` `WDQS`: D (§6.9); `:59` `RULES`, `:168` `OSM_REF`: D (the registers'
  fields); `:293–310` the NRHP case: P.
- `dem/heritagetiers.py:15–36` designation patterns per register: P (the producers' `level`s).
- `dem/whsshapes.py:82`, `:122–125` UNESCO's list URL and id pattern: D (the `unesco-whc`
  register); the rest G.
- `dem/names.py:19`, `:36` (`REGIONS` boxes): G (the languages input); the English table: retired
  (#139).
- `crates/pipeline/src/agent/build.rs:609` (heritage-sites' key on the snapshot), `bin/scenic-build.rs:2232`
  (the seeds), `:2290–2311` (the seeds laid out), `:2440–2459` (`registers-import`), `:2502` (the
  snapshot), `:2573`, `:2636` (the stand-in root): G once they read the accepted registers; the
  seeds' and snapshot's lines go.
- `crates/pipeline/src/markconv.rs:88–92` the UNESCO site id from its URL, `:137–144` id schemes
  (`reg:dfhd:`, `legacy:`): G (ids from the registers' `id`s; the `legacy:` prefixes stay, as ids
  must not change: docs/phase5.md, Ids).

### 9.6 Timetables [#140, #141, #142]

- `dem/railfeeds.py:7`, `:391–414` the catalogue's columns: G (the `mdb-catalogue` format). `:79`
  `EXTRA` (SNCF, Renfe ×2, GB national rail via Catenary Transit, Hong Kong's trams), `:98`
  `ELSEWHERE`, `:102` `KEYED` (LTA), `:110` `COUNTRY`, `:115` `REPLACES`: D. `:417` `keyed_link`'s
  LTA branch: G (the `get-key-json-link` method). `:69–76` user agent and retry tuning: C.
- `dem/railgtfs.py`: G (the counter). `:47` `mode_of`: G (GTFS route types).
- `dem/mtrpairs.py` (whole, hand-run): P, then deleted (#141).
- `dem/gtfs.py` (hand-run, writes the ferries' counts): P, replaced by the shared counter.
- `dem/ferries.py:10–14`, `:326`, `:510` the sources' precedence and reading: G; `:49` `URBAN_NETS`,
  `:58`: C.
- `crates/pipeline/src/rail.rs:37–42` the rail sources' logical paths (catalogue, MTR, its pairs):
  D/P; `:54` `FEED_KEYS`: derived; `:137` `OWN_CODE` (ISO 3166-2 territories with their own
  alpha-2): G (ISO reference data, not a source); `:459` the test reading railfeeds.py: goes.
- `bin/scenic-build.rs:3877–3895`, `:3946–3950`: G; `:3980–3983` the MTR's pairs merged: goes.
- `crates/pipeline/src/ovconv.rs:384–397` the ferries job reading `inputs/ferries/freq`: reads the
  accepted units instead.
- `crates/pipeline/src/stations.rs:24` `INTERCITY_WORDS` (brands in OSM names): C.

### 9.7 Names and languages [#136, #137]

- `crates/names/src/territory-languages.tsv` (CLDR 47, 251 lines) and its `include_str!`
  (spoken.rs:98), `spoken.rs:69` `REFINED`: D (the languages input). `:93` `READ_AS`: G (a lookup
  rule). `tools/names/cldr-languages.py`: P (`inputs/languages/how/`).
- `tools/names/convert-tables.py` (the one-off conversion, its per-area tables at :32–42): P
  (`how/`, history). `tools/names/check.py`: G (the translations' checks, run by the gate).
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

- `crates/pipeline/src/agent/recipes.rs:28–55` `Outline::Geofabrik`, `Poly` and their parsing,
  `coverage.rs:374` `file_rings`, their rings from `inputs/outlines/`, `agent/mod.rs:4671` `regions_digest`'s
  outline files, `server/src/regions.rs:686`: go (§6.1). `dem/bldfetch.py:248–250` reads
  `inputs/outlines`: goes.

### 9.10 Not sources (checked, left as they are)

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
the leaf-type squares, the MTR and ferries' converters, the CLDR generator); about 55 stay generic
(formats, protocols, the merging rule, the counter); about 35 are configuration.

## 10. The legacy pipeline: what's left

`make run` and `make dev` are gone: the Makefile and the Python steps only it ran were deleted
(commit 2992c71), and nothing in the repository names them. What remains (plan §10, phase 6,
"Cutover: done"), each checked in the code on 2026-10-10:
- **Fallbacks to the old build's heritage files** (`global/legacy/`, still on the NAS):
  `markconv::LEGACY` and `heritage_source` (crates/pipeline/src/markconv.rs:20–35),
  `agent::build::heritage_src` (agent/build.rs:694–700) and `overlays_key`'s branch for it
  (agent/build.rs:1944–1950). Never taken while a registers snapshot exists. **Removed in #138**
  (the registers on the gate make the snapshot's absence impossible: the last good version stays),
  then `global/legacy/` and `layers/basemap/legacy-*.pmtiles` deleted from the NAS once a published
  app has no readers. (`sources/legacy/` is already gone: the owner deleted it on 9 October.)
- **The registers' legacy names** (`sources/registers/legacy`, `legacy-seeds`): go with #138 and
  #139.
- **The `terrain` and `slope` programs' build-folder modes** (crates/pipeline/src/bin/terrain.rs:66,
  bin/slope.rs:185, defaulting to `data/build`): `terrain --scan` is live; the folder modes go in
  #144.
- **The heritage chain's stand-in root** (`data/build` in a scratch root, §6.5.2): partly in #138.
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
| 134 | the gate (§4), on `_gate-test` | 133 | the gate's behaviour on the NAS between two Macs (§4.10) |
| 135 | regions (§6.1) | 134 | coverage fingerprints unchanged: nothing rebuilds |
| 136 | languages (§6.2) | 134 | `global/spoken`'s content name unchanged |
| 137 | translations, descriptions (§6.3, §6.4) | 134 | the served tables and descriptions unchanged |
| 138 | heritage registers (§6.5) | 134, 136 (the chain's languages) | heritage-sites, heritage, marks, overlays unchanged or explained |
| 139 | the seeds retired (§6.5.3) | 138, 137 | the heritage layers' English and park facts unchanged or explained |
| 140 | feeds (§6.7.1) | 134 | the feed list, zips, `global/railfreq`, ferry sailings unchanged or explained |
| 141 | static GTFS (§6.7.2) | 140 (the shared counter) | the MTR's trains and the looked-up sailings unchanged or explained |
| 142 | to-do and report (§6.7.3) | 141 | (new output; nothing deleted) |
| 143 | elevation, roads (§6.8) | 134 | per-vertex elevations unchanged away from edges; edges and spacing explained |
| 144 | elevation, terrain (§6.8) | 143 | terrain and slope packs unchanged |
| 145 | the remaining sources (§6.9) | 134 | each consumer's outputs unchanged |
| 146 | the guard (§7) | 135–145 | passes; publish gated on it |
| 147 | how each input is made; docs (§8) | 135–145 | |

Each migration task, in order: write the producer or the declarations; drop today's data in its
shape; check it on the gate (accepting today's warnings, each listed in the task's report); switch
the readers to the accepted version, re-keying where the new keys name what the old pinned; prove;
publish; only then delete the old path and its NAS files (after GC's 14 days for content-named ones).

## 12. Risks

- **Every step's reads move.** Regions alone are read in about twenty places today (§6.1). A reader
  missed keeps reading the drop box and bypasses the gate; the guard (#146) adds a marker for drop-box
  paths outside `inputs::open`, and #134 lands that marker first, so each later task is checked as it
  goes.
- **The gate needs a lead.** Checks run as jobs the lead grants: with no lead (every Mac asleep or
  away), a drop waits. Translations go from about a minute and a half to the next listing plus a
  check, a few minutes; if that matters, the map's server can show a dropped file's lines as
  "pending" before they're taken in (not planned).
- **Thin-gap false alarms.** The closing at 1 km may flag deliberate notches (a bay between two
  regions' custom shapes). Accepting is the answer; the finding's id keeps an accepted gap accepted.
- **Big checks.** A heritage register of a few hundred thousand entries, or 2.78 M translation lines,
  takes tens of seconds to minutes to check. The check job is ordered first but holds nothing else
  up; only the changed files are read in full.
- **Today's warnings at migration.** check.py on the converted translations, the registers' `approx`
  shares, likely gaps: each migration accepts what's there once, listed, so the gate starts clean
  without hiding anything.
- **Heritage's merge order.** Reading registers in id order instead of today's hand order may change
  which register a merged place names first; positions shouldn't change. Shown and explained in
  #138.
- **Elevation's edges and the DEM cache.** Blending changes elevations near datasets' edges, and the
  derived densification changes vertices where today's boxes and the data disagree (Canada outside
  HRDEM); both are explained differences, not regressions. The units' per-vertex DEM cache
  (`sources/dem-cache/`, `cache/dem-units/`) records a source code per vertex (`DemSource`, 0–7):
  the codes become positions in the ranked declarations, recorded with the cache, and the cache is
  re-keyed once in #143 rather than every unit sampled again.
- **Height references.** Treating the geoid-based references as equal to EGM2008 leaves offsets up
  to about a metre (NAVD88 against EGM2008 in parts of the US), smoothed by the blend. A declared
  geoid grid for each would remove them, at the cost of a grid per reference.
- **Feeds' derived boxes.** A declared feed's box comes from its `stops.txt`, read once fetched; a
  feed is fetched once before it's known whether it's near the coverage. That costs a download per
  new declaration, never a wrong count.
- **Ferry pins.** OSM ids pinned in declarations and static GTFS go stale when OSM changes a ferry;
  #142's report lists each pin naming nothing in the pass, every pass.
- **Credits in transition.** Until every input has its description, the catalog's credits come
  partly from descriptions and partly from `rules::CREDITS`; #134 merges both, the descriptions
  winning for a source both name, so no credit is lost.
- **SMB.** The gate adds a listing of about a dozen folders every two minutes and a few content-named
  writes per change: well within the share's limits (plan §3), none in a hot path.

## 13. Open questions, decided by default

Points the owner hasn't decided, each with the option most consistent with their decisions and
principles. Each is easy to reverse before its task.

1. **Validation runs as a job, not in the lead's loop.** The owner asked that the agent check every
   change; the pool's principle is that every job hands off and the lead alone merges. A check job
   keeps both: any member checks, the lead's merge takes it in, and nothing new writes the records.
2. **Holding is per file, not per drop box.** "An error or a warning pauses ingesting it": "it" is
   taken as the changed file (or the files a cross-file finding names), so one bad translation file
   doesn't hold a hundred good ones. The banner says which files are held.
3. **Acceptances live in `state/inputs/accepted/`, not in the drop box.** A file in the drop box
   would itself be a change to check; create-new files in `state/` keep one writer per file and are
   backed up with `inputs/`.
4. **Translations and descriptions move under `inputs/`.** One root for every drop box (one listing,
   one backup rule, one place to look). The cost is a one-time move and a server reading both paths
   for one app version.
5. **Two timetable units, one input.** Feeds (declarations) and static GTFS (data) have different
   checks, so they're gated apart, but counted by one counter and shown as one input.
6. **Static GTFS uses GTFS's own fields where it can:** sources in `route_url`, the OSM pin and the
   checked date as extra columns (the spec lets producers add columns), so the files stay valid GTFS
   any tool opens. `route_url` is required.
7. **Timetable matching problems are a report, not a gate warning:** they depend on OSM, which
   changes each pass; a warning would hold the owner's file again every pass.
8. **Feeds carry no country or box.** Both can be read from the data (`stops.txt`), so declaring them
   would be a hand figure that goes stale; the catalogue's country rule stays, since it reads the
   catalogue's own fields.
9. **Heritage geometry:** points are sites, polygons heritage areas, lines lines; a polygon shows a
   dot only with `dot: true`, which reproduces today's map (Québec's perimeters and Ontario's
   districts have no dot of their own).
10. **Heritage merging keeps today's rule exactly** (the same Wikidata item, or a local entry within
    40 m of a same-or-higher one sharing a name word), plus the owner's pairs; registers read in id
    order for determinism.
11. **Register territories are ISO codes, checked against the entries;** no boxes. A register's
    credit extent is its entries' box.
12. **Elevation:** finest resolution first unless `rank` says otherwise; ground over surface at a
    tie; a blend band four coarser pixels wide; the geoid-based references taken as EGM2008 within a
    metre, ellipsoidal ones only with a declared geoid grid, unknown ones refused. Densification
    derived from the datasets (8 m where one of 10 m or finer meets the way). Each is a change the
    owner may tune; none changes the input's shape.
13. **Tile pyramids declare a territory** (ISO codes) because their coverage can't be read from a
    header and asking for tiles worldwide isn't practical; it's a fact about the dataset, and tiles
    that answer with no data outside it are simply none.
14. **What's configuration, not a source,** stays in code: road network rules, storey heights fitted
    per country, OSM name heuristics (intercity brands, urban ferry networks), Wikidata properties
    asked for, the pageview months, the user agent. The guard doesn't flag them.
15. **Producers live in each drop box's `how/`,** on the NAS (backed up with `inputs/`), not in the
    repository: they're the owner's side of the line, and the app's publish never carries them.
    (Their history in git ends with this series' deletions.)
16. **The test input is removed after #134's trial,** its tests staying in the suite.
