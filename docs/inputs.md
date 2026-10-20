# Inputs: every input through a standard shape and a checked drop box

Every input the map is built from has one written shape and one folder on the NAS, its **drop
box**. The owner makes inputs however they like, to that shape, and drops them there. The build
checks each change before taking it in, keeps the last good version while a change is held, and
reads only checked, content-named copies, so builds stay deterministic. Per-source conversion
happens outside the app (`producers/`, §8); what the app must know about a source (where to fetch
it, its licence, its credit) is written in a declaration file in a drop box.

**The input check** (§4) checks every change dropped into a drop box: first its shape, then the
input's own rules. A change that passes is copied into `sources/inputs/` and becomes the version the
build reads. A change with a problem is held: the app keeps the last version that passed, shows the
problem on the menu bar icon and the build page, and takes the change in once
it's fixed or, for a warning, once the owner accepts it. Each drop box is checked on its own and is
a **unit** of the input check. (In the code the mechanism keeps the older word "gate":
`gate_work`, `_gate-test`.)

Built: the input check and its units for regions, languages, translations, descriptions, heritage
registers and matches, timetable feeds and static GTFS, elevation datasets (the roads' and the 3D
terrain's), the remaining sources (§6.9); the guards (§7). Companions: `docs/plan.md` (pipeline, keys, order), `docs/formats.md`
(files), `docs/pool.md` (lead, jobs, journal), `docs/phase5.md` (the heritage chain).

## 1. Constraints

- **Determinism** (plan §8): every input reaches a job's key by content (a content name or a digest
  of its bytes), never by a path, a size or a time. The one exception is a `range` source (§5.2),
  read in place a range at a time, which can't be pinned by content.
- **No hand-kept figures:** a declaration states facts that can't be read from the data (licence,
  height reference, where it is); everything readable from the data (resolution, coverage, extent,
  counts, a feed's box) is read at check time. Where a declared fact can be checked against the
  data, it is, and a wrong one is a finding.
- **The NAS's limits** (plan §3): no input is stored or read as thousands of small files, and
  nothing lists a drop box in a tight loop.

## 2. The three kinds of input

| Kind | What the owner drops | The app's side | Missing data means |
|---|---|---|---|
| **Declare** | the data itself, in the input's shape | reads it as given; the files are the whole input; never writes there | the map has none of it |
| **Request–fulfil** | answers to the app's requests | writes a to-do list and a brief into the drop box's `todo/` after building; reads the answers like a declare input | the map falls back, the request stays listed |
| **Source declaration** | a small file naming a standard format, where to get the data, its licence and credit (data may lie beside it) | fetches the data itself into `sources/`, content-named and pinned in the records | the source isn't used |

Not inputs: `inputs/keys.env` (secrets; their names enter keys, never their values),
`inputs/hold-catalog` (a control), `todo/` lists, and `how/` notes (§8).

## 3. The drop boxes on the NAS

Every drop box is a folder under `inputs/` in the NAS project folder (plan §3):

```
inputs/
  regions/            declare: <id>.toml, <name>.geojson, _group.toml in any subfolders; pins.toml, rules.toml (§6.1)
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
  keys.env, hold-catalog   not inputs
```

Each folder is one unit (`timetables/feeds` and `timetables/gtfs` are two units of one input).
Inside any drop box:
- `todo/` is the app's and `how/` the owner's notes (pointing to `producers/<input>/`, §8); neither
  is read as input.
- Names starting with `.`, `@`, `#` or `_` are ignored (`@eaDir`, `#recycle`, `.DS_Store`, drafts),
  except the regions' `_group.toml`.
- A file is taken only once its time is 10 s old, so a half-written file isn't checked.
- Any other file that isn't the drop box's shape is an error finding, never skipped silently
  (except in `regions/`, which ignores unknown files: §6.1.2).

A file dropped into the old NAS-root `translations/` or `descriptions/` is an error finding on the
new unit (`moved`); GC's never-swept list keeps those folders. The checked copies live under
`sources/inputs/` (§4.6) and are the app's, never the owner's to edit.

## 4. The input check

crate::inputs; `scenic-build inputs`; the agent's `gate_work` and `tend_gate`. Units are listed in
`inputs::UNITS`. A unit with nothing dropped and no records isn't checked or shown.

### 4.1 Terms

- **Candidate:** the drop box's files as they are now, once they've held still.
- **Finding:** an **error** (the file can't be used) or a **warning** (usable, likely a mistake),
  with a stable id (§4.4), the files it's about, a message, and a place when it has one.
- **Held:** a change with an unaccepted finding; the last accepted version of its files stays in use.
- **Accepted version:** the checked, content-named copies the build reads, listed by an index whose
  content name is the unit's version (§4.6).

### 4.2 Noticing a change

- The lead (pool.md §5) lists every drop box (recursively, skipping `todo/` and `how/` at the top)
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
  (`bldprep`) or another check. It holds no region and no catalog.
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
  6. Stores each file content-named under `sources/inputs/<unit>/` (uploaded and read back, plan §3).
  7. Hands off the records: `sources/inputs/<unit>/@index` (when it differs; the replaced index is
     touched so GC keeps it, §4.9), `@listed`, and `@held` (the held report, or none). `@` starts
     no drop-box path component or unit name, so these never clash with copies or nested units.
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
  (`/work/inputs`) so it lists at once; the check's key changes and the file is taken in.
- **Undoing:** `scenic inputs unaccept <unit> <finding id>`; the finding holds again from the next
  check (never the version already in).
- An acceptance outlives its finding; `scenic inputs` lists stale ones and `scenic inputs unaccept
  --stale` removes them. Acceptances are backed up with `inputs/`.

### 4.6 The accepted version and how steps read it

- **Files:** `sources/inputs/<unit>/<path>.<hash16>.<ext>`, the drop-box path content-named.
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
  `accepted` maps the warnings the version was taken with to their files, kept while those files
  are unchanged. No file time is in it, so the same contents have the same version.
- **The records:** the manifest's `sources/inputs/<unit>/@index` names the index, which roots its
  files for GC. Only a check job's hand-off, merged by the lead, changes it.
- **Readers:** `inputs::open(root, manifest, unit)` gives a step a unit's accepted files by drop-box
  path, from the records it planned with. Python steps get the paths on their command line from the
  agent. No step reads a drop box (§7.1).
- **Keys:** a step names the index's content name when it reads the whole unit, or the `keyed`
  digests of the files it reads (heritage-sites names only the registers meeting its cover).
- **The map's server** reads the translations, descriptions, languages and regions' drawn files from
  the accepted copies the lead's manifest names, copied to the Mac (crates/server/src/accepted.rs,
  pipeline::inputs::served), and from that copy away from the NAS. The Regions panel reads the
  region files read-only plus the accepted index and held report. `names-todo` reads the accepted
  translations and descriptions the agent names.
- Planned: the catalog recording the accepted index of every unit it was built from.

### 4.7 Where it shows

- **The status** (`state/status.json`, plan §8): `inputs`, one entry per unit
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
  held unit with Accept (warnings, after confirming) and Show on the Build Page; notifies when a unit
  is held and when it's taken in.
- **The build page** (`web/work/dash.js`): a banner per held unit with its files, findings, places,
  Accept and Accept All; a quiet line while checking.
- **The Regions panel** marks a held region (§6.1.9).
- **`scenic status`** prints held or checking units; `scenic inputs` lists every unit's version,
  state, findings and acceptances.

### 4.8 Credits from the inputs

- The catalog's `credits` (formats.md, Catalog) are made from the accepted descriptions (§5.1) by
  crate::inputs::credits, beside `pipeline::rules::CREDITS` for sources with no description yet
  (descriptions win for a source both name), with one rule for which to list: those whose extent
  meets the coverage, 20 km around it, or a built unit's ways (`rules::listed`). A description's
  extent is `facts.extent` in the index (a box, or `"world"`), derived from the data, never typed;
  one whose extent isn't known is listed nowhere.
- **Credit edits reach the catalog without rebuilding:** credits enter no step's key, but the
  catalog's key (`catalog_key`) has the digest of every accepted description's credit fields
  (`credits::digest`). When descriptions can't be read now, the last digest read stands; with none
  read yet, the catalog waits.
- **Popups name their source by register id** (`register`, `registers`), resolved in the map against
  the catalog's credits (web/src/overlays.ts `sourceOf`), so a credit edit reaches popups with the
  next catalog.
- The map's own credits for what the app makes (colour ramps, `web/src/ui/strip.ts`) stay the app's.
  The server's fallback for catalogs without credits (`credits_of`, which reads `rules::CREDITS`)
  stays while such catalogs are served.

### 4.9 GC and backups

- GC sweeps `sources/inputs/` (`gc::inputs_kept`) and no other part of `sources/` (`NEVER`). A file
  there goes once nothing names it and it's older than `keep_days` (default 14). Named: the
  manifest's and newest records' indexes, listings and held reports, those of journal entries not
  yet merged (pool.md §7.3), every index touched within `keep_days`, and the index each recent check
  state names as `replaced`. So a replaced version stays restorable for 14 days. If any folder can't
  be read, nothing is swept that day. Drop boxes and `state/inputs/` are never swept.
- Every accepted version of a file is kept whole, so the briefs say to drop one answer file per
  batch rather than append to a big one.
- Backups (plan §3; agent/backup.rs `FOLDERS`) cover `inputs/` and `state/inputs/accepted/`, not
  `todo/`.

### 4.10 The test unit and the tests

- **`inputs/_gate-test/`** (crate::inputs::gatetest) isn't real data; it's checked only while
  `state/inputs/gate-test` exists (`scenic inputs test on|off`; `off --forget` removes its
  acceptances and asks the lead to drop its records, the job `inputs _gate-test forget`). Shape:
  `.jsonl` lines `{"k": "<string>", "v": <number>}`; an error for a bad line, a warning for a
  negative `v` and for a removed file. No build step reads it. `tools/check/gate-two-macs.sh` is
  the procedure that exercises the input check on the NAS between two Macs (a drop taken in, an
  error held, warnings accepted from each Mac, an unaccept, a touch, a removal, a lead handover
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
| `redistribute` | bool, default true | yes | false: never in the World download nor sent to a worker page |
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
| `files` | data dropped beside the declaration, in `<name>/` |
| `version` | a pinned release where the source has releases |
| `[data.fetch]` | `method`: `get` (default), `get-key-header` (`header`, `key` naming a key in `inputs/keys.env`), `get-key-json-link` (also `link`, the JSON path of the download link) |

Fetch methods are protocols, generic code used by any declaration, through `pipeline::fetch`, which
asks before any internet request. A range read (a GeoTIFF's tiles) is streamed, and one that breaks
off goes on from the bytes received with a new range request (`fetch::net`: only a request that
moves nothing counts against the retries). A server that gives no answer after its retries is noted
for its host and port in the unit job's cache (`fetch-down/<host>`, with `_<port>` when the URL names one; `SCENIC_FETCH_DOWN`): requests to that server
fail at once for the next 30 minutes instead of each waiting out its retries, and a step that ends so
exits 69, which the unit job reads as the area's server being down: it sets that area aside, builds
its other areas, and fails at its end naming the areas set aside, which the agent's retry builds later. The data servers a worker's task may read through the coordinator
(docs/workers.md §3) are derived from the accepted declarations (`coord::hosts`): the hosts of the
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

**Kind:** declare. **Drop box:** `inputs/regions/`, read recursively. **Unit:** `regions`. Code:
crate::inputs::regions (`file`, `shapes`, `geom`, `resolve`, `water`, `data`, `stored`, `extract`,
`read`).

#### 6.1.1 What a region is

A name for a piece of the coverage. The coverage is the union of every region; nothing is built per
region (plan §5). Without `rules.toml`, regions may overlap freely and the check validates each
region's definition, not how regions fit together; the map's region layer shows overlaps and gaps
by eye. With `cover = "earth"` in `rules.toml` (§6.1.3, "Land and water"), the regions must cover
the Earth once: the check fills land gaps and settles land overlaps by the folder-wide rules, and
any water in two regions, or in none, is an error.

#### 6.1.2 Files and folders

- One region per file, `<id>.toml`, anywhere under `inputs/regions/`. Ids are unique across folders,
  so moving a file changes no id, coverage or key.
- Custom shapes are `<name>.geojson`, anywhere, named by file name alone (unique across folders).
- **Folders are the Regions panel's groups:** a collapsible tree nested as on disk, each group with
  its count, open state remembered per browser. A group's name is its folder's, or the `name` of an
  optional `_group.toml`, with an optional integer `order` among siblings (those with one first,
  lower first; then by name). The check reads it (`rg-group`); the index carries it (`facts.group`)
  and the tree is made from the index alone (`regions::read::tree`). Empty folders aren't shown.
  Moving a region between folders changes its group, never its geometry or keys (the index, and so
  the catalog, is new).
- `pins.toml` names the OSM pass the folder is written against (§6.1.5).
- `rules.toml` (optional, at the top) holds the rules for the whole folder: `cover = "earth"` and
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
| `[lines.<name>]` | a line, from `coords` or `ways` | optional |
| `[pins]` | as `pins.toml`, overriding it for this file | optional |

Unknown fields are an error. The region is ((∪ `outline`) ∩ (∪ `intersect`)) − (∪ `minus`); a region
with `water` is its land, that cut back to the coastline's land, and its water ("Land and water").

**Parts:**
- `"osm:<relation>"`: an OSM relation's area, from the pinned pass's outline set (administrative and
  ISO 3166 relations, plan §5) or, for any other relation, the regions' extract (§6.1.6).
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
  (`to` and `than` swapped) is exactly the rest (ties go the same way in both).
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
(the `regions-land` source) less the lakes over 100 km² (`regions-lakes`); those lakes inside its
outline are its water, unless another region's water names them. Then its water: the `water` parts,
less the coastline's land and every region without `water` (the land dominates the sea), in an
order where a region comes after those its water names whole (`region:`, `group:`). A region's
`hand_out` folder then takes each stretch (a connected piece) of its water that one of its regions
alone borders, and a stretch under `sliver_km2` goes to the one sharing its longest edge. With
`cover = "earth"` in `rules.toml`, then: a region's piece of water under `sliver_km2` touching
none of its land joins the region with water it shares the longest edge with; and, cell by cell
(2°), land in two regions is taken from those of them with water, and each piece in no region
within 100 m of land (land in no outline, the sea between the zones' coastline and OSM's) up to
`sliver_km2` joins the region with water it shares the longest edge with; a piece touching none
stays. Land in no region over `sliver_km2` is an error (`rg-cover`). The region whose water is
`rest` takes what's left; without one, water left is an error, as is water in two regions. A region
without `water` resolves as before, and no new rule changes it: a hand-out gives only to regions
with water (naming one without is `rg-ref`), and one without `water` may not name one with it
(`rg-ref`). So old and new files sit together, and `cover = "earth"` only holds once every
file is new. The 12-nautical-mile line is the `regions-territorial-seas` source,
named only as `ts12:` parts; it divides nothing.

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
`pins.toml`: `osm = "2026-09-29"` (the pass, `sources/osm/<date>/`). A file's `[pins]` overrides it
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
  the authalic sphere; a shared edge's length is the other area within 100 m of the edge, over 100 m,
  measured in quarters while either area is large.
- **Cell by cell** (crate::inputs::regions::tiles): a region with water is resolved on a grid of 2°
  cells. Each part it names is cut into the cells by clipping its rings to boxes (no polygon
  operation on the whole); each expression is evaluated per cell, a buffer from the cells within
  its distance, a nearest-coast split against the coasts in the cells within 450 km; the land, water
  and lakes are kept by cell, stretches (`hand_out`, slivers) joined across the cells' shared
  edges, and the cover check runs per cell. So an operation holds a cell's geometry (a buffer's or
  a split's, its neighbourhood's), never a region's; what's kept is every region's cells (the
  outcome itself) and the datasets, read only once a region in the folder names them (about 1 GB).
  A zone's internal waters are unioned with it. The datasets' rings are simplified within 20 m as
  read, the points two rings of the zones share kept (neighbours' borders the same on both sides). The regions with water resolve one at a time, their cells several at once (regions without
  water: at most four at once). A region's parts naming other regions (`region:`, `land:`,
  `group:`) are read cell by cell where those are kept, never copied. With
  `SCENIC_REGIONS_TRACE` set, the check prints each step's and each region's water's footprint
  and its buffers' and nearest-coast splits' count, time and footprint; `SCENIC_REGIONS_THREADS`
  sets how many cells at once (4). A buffer of a cell reads its neighbours' pieces each buffered
  once, within 16 columns each way and not from the cells south of 80°S (Antarctica's inland ice) or
  north of 86°N (where no land is); a buffer of
  50 km or more is drawn as discs at the pieces' points a thirty-second of the distance apart
  (within a twentieth of the distance).
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
- `sources/inputs/regions/@drawn/<id>.<hash16>.json`, what the catalog and the region layer draw:
  each shape simplified for drawing (an `osm:` part as the pass simplified it, else by
  `draw_tolerance_m`), degrees to 5 decimals, keyed by `from`.

A region of only `osm:` and `shape:` parts in `outline` with no operation is stored as **one shape
per part, unchanged**; any operation gives one shape (two across the antimeridian); a region with
`water` gives one shape, its land and water together, its rings on both sides of ±180° where it
crosses. A region with water and no land is stored `"water_only": true` (in the head) and the
coverage keeps it apart, for the depth's targets alone (plan §5). The index names
each region's files in `facts.geometry` and `facts.drawn`, and its `keyed` digest is the geometry
file's content name, so an edit that doesn't change the result reruns nothing.

**Reading:** `Coverage::accepted(root, accepted)` (crate::inputs::regions::read) builds the coverage
from the stored shapes, regions in id order, shapes in stored order. Nothing rebuilds the coverage
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
  region until `pins.toml` is moved; the banner leads with that.
- While the current pass has no outline set yet, the check does nothing (the outline set's name is
  in its key).

#### 6.1.6 The extract

Relations outside the outline set, and every way a line names, come from **the regions' extract**:
`sources/osm/<date>/regions-extract.<hash16>.json`, `{"fmt": 1, "pass", "relations": {"<id>":
{"rings", "tags"} | null}, "ways": {"<id>": {"ends", "line"} | null}}`, `null` for an id the pass
lacks.
- A check needing an id the extract lacks holds the files (`rg-extract`) and lists what it needs in
  its held report (`needs`); the lead plans `regions-extract <date>`, keyed by the pass and the
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
lakes 5.0.0, a shapefile zip, by `ne_id`) and `regions-land` (OSM's land polygons, split, in
degrees), each a file in `sources/fetched/<id>/` named by its `params.path`. Their rings are simplified within
20 m as they're read (`data::SIMPLIFY_M`); the zones' (sea, internal waters, territorial seas) keep
every point two of their rings share, so a border two zones share stays the same on both sides. The check reads them
whole, and only once a region in the folder names them (or has `water`), when the accepted sources
declare `regions-sea`; until then such a region is held (`rg-data`). Features of one id are unioned
as they're read. Their declarations are staged in `producers/sources/`
(`marineregions-eez.toml`, `marineregions-internal-waters.toml`, `marineregions-territorial-seas.toml`,
`ne-lakes.toml`, `osm-land-polygons.toml`); Marine Regions' three are CC BY 4.0 and
credited as their declarations say. `scenic-build regions-check --dir <folder> --declarations
<folder>` runs the check on a scratch folder in memory with the datasets those declarations name,
and reports each region's size.

#### 6.1.8 Checks

All errors; ids `rg-<check>.<hash16>`.

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
| `rg-ref` | a `region:` or `line:` naming nothing |
| `rg-loop` | a loop of `region:` references |
| `rg-empty` | a result with no area |
| `rg-pin` | no OSM pin, or one that isn't a date |
| `rg-pin-mismatch` | a pin naming another pass than the current one (§6.1.5) |
| `rg-group` | a `_group.toml` that doesn't parse, has a field other than `name`/`order`, or an empty name |
| `rg-extract` | ids the extract lacks: held while it's made (§6.1.6) |
| `rg-data` | a region naming the datasets, or with `water`, while they aren't read (§6.1.7) |
| `rg-data-missing` | an `eez:`, `ts12:` or `ne-lake:` id the dataset hasn't |
| `rg-rest` | `rest` in more than one region's water |
| `rg-rules` | a `rules.toml` that doesn't parse or has an unknown field or value |
| `rg-cover` | with `cover = "earth"`: water in two regions, or in none without a `rest` region |

A held region keeps its last geometry; a new held region isn't in the coverage yet. Deleting a
region file takes it out with the next version.

#### 6.1.9 What the owner sees

- **The Regions panel** (Settings → Regions), read only: the folder tree, each region with its parts
  and state (accepted, checking, "held: <finding>"). The server reads region files through the NAS
  I/O pool (`GET /api/regions`) and states from the accepted index and held report. Nothing in the
  app writes a region.
- **The region layer** (a switch in Settings → Regions, `/api/regions/layer`): each region's stored
  outline, a light fill coloured by its id's hash, its name as a label; a held region drawn dashed.
  Read from the drawn files, copied to the Mac as the translations are.
- **The banner and menu bar** (§4.7).

#### 6.1.10 The switch

Every reader reads `Coverage::accepted`, and only once the records hold
`sources/inputs/regions/@switched` (crate::inputs::regions::read `switched`); without it the readers
refuse, saying to do the migration's runbook first. The switch records the app it was made on, and
no Mac on an older app leads or is handed the lead. Catalogs record their regions' shapes inline
(formats.md, Catalog `coverage`), so old catalogs still draw. The tests (crate::inputs::regions's)
cover the operations on small fixtures: union, minus, intersect, cuts (past the shape, ending
inside, pieces missed), a 12 nmi buffer against a geodesic distance, ways that don't join, a loop,
duplicate ids, a pin mismatch, the antimeridian, byte-identical output twice.

### 6.2 Languages by territory

**Kind:** declare. **Drop box:** `inputs/languages/`. Code: crate::inputs::languages.

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

**Ingesting.** The `spoken` job (plan §7) keys on the outlines, `spoken::RULES` and the accepted
index (`--languages-index`), and waits while there's none. `global/spoken` carries the table, which
`names-todo` reads; the server reads the accepted table (`accepted/languages/` in its home) only for
catalogs without `global/spoken`.

### 6.3 Translations

**Kind:** request–fulfil. **Drop box:** `inputs/translations/`. Code: crate::inputs::translations,
crate::inputs::lint; the server's crate::accepted; `names::Names::set_files`.

**Shape** (plan §7, Files): `**/*.jsonl` outside `todo/` and `how/`, one translation a line, read by
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

**Kind:** request–fulfil. **Drop box:** `inputs/descriptions/`. Code: crate::inputs::descriptions.

**Shape** (plan §7, Descriptions): `**/*.jsonl` lines, and an optional `about.toml` (its credit;
without one the catalog keeps `rules::CREDITS`' "Heritage descriptions"; its extent is the world
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

**Kind:** declare. **Drop box:** `inputs/heritage/`, a pair per register: `<register>.geojson` and
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
| `name_en` | string | optional | the register's own English (plan §7) |
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
- **One generic reader:** the heritage-sites job (plan §6; docs/phase5.md) lays out every accepted
  register whose box meets one of its cover's z6 tiles (`pipeline::heritage::registers_meeting`)
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
  `--heritage-matches-index`). Without an accepted version it waits (and the units, marks and
  overlays with it).
- **Credits and popups** by register (§4.8). A register's whole box decides where it's credited.
- The chain's caches (Wikidata, Wikipedia, World Heritage answers) are the pass's answers
  (`sources/items/<date>/heritage.tar.zst`, `pipeline::answers::heritage`), a new pass starting
  from the last one's. With no pass's archive yet, the first start seeds its working copy from the
  old snapshot `sources/registers/legacy`'s caches (read only) and the answers kept beside it
  (`pipeline::answers::heritage_start`); a snapshot named but unreadable fails the job.

#### 6.5.1 What stays generic in the heritage chain

The chain's scripts (docs/phase5.md) read the registers' fields, not per-register knowledge:
heritagewd.py looks a site up by `ref` under its register's `wikidata_property` (a World Heritage
site by `site`) and links through OSM by `osm_ref_tags`; whsshapes.py finds a site by `site`, its
components by `ref`, its area by `site_area_km2`, its outlines by OSM's World Heritage tags;
filterprops.py takes `t` from `tier`. Names chosen per language (`lang_at`) are the producers'.

#### 6.5.2 The heritage chain's stand-in root

The chain runs in a stand-in root laid out as the old repository (`data/build`, `data/heritage`:
scenic-build's `heritage_root`). The registers and matches are given on heritage.py's command line
(`--registers`, `--matches`: folders of links to the accepted copies), and `data/heritage` is the
pass's working copy of the chain's answers (`cache/heritage-<date>/`).

#### 6.5.3 English names, park facts and pageviews

The old `sources/registers/legacy-seeds` archive is read by nothing:
- **English:** the heritage layers' `en` is the thing's own English only (`dem/names.py`
  `english_at`: a site's UNESCO name or English Wikipedia title, an area's `name_en`). Other English
  is the server's, from the translations as it serves the layers (plan §7). A name lacking English
  goes onto the translations' to-do (`pipeline::namestodo` reads the landmarks' `en`).
- **Park facts:** areadetails.py's cache is the pass's answers (`areas-wikidata.json`,
  `pipeline::answers`), asking Wikidata only for parks it lacks.
- **Pageviews:** the items job's months' indexes (`sources/pageviews/`, plan §6).

#### 6.5.4 Heritage matches: the owner's judgment calls

**Kind:** declare. **Drop box:** `inputs/heritage-matches/`, `*.jsonl`. Code:
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

- **Rail** (`dem/railgtfs.py`): trains per stop pair on the typical day, as the PAIR records the
  `rail` job matches onto the tracks (`rail::PAIR`: two stops' positions, a mode byte, trains a
  day). A feed's typical day is its median-busy Tuesday–Thursday from 30 days before its zip's fetch
  day to 90 after; a train in two feeds counts once (keyed by first and last stops and times, the
  operator's feed first).
- **Ferries** (`dem/ferrygtfs.py`, run by the `ferries` job's `dem/ferries.py`): per OSM ferry line,
  the record the map shows (`info["freq"]`): `per_day` (each way, on days it runs; under four days
  a week, weekly ÷ 7), `per_day_low` (the quietest month's), `headway` (median gap 07:00–19:00),
  `days`, `months`, `season` (`year`, `some-days`, `seasonal`), `overnight`, `source`, `url`,
  `checked`. A GTFS route matches a line when its trips call within 400 m (at most 30 % of the
  line's length) of both its ends. Feeds count from their fetch day, in id order, the first feed's
  count winning.
- **Precedence** for a ferry line (`ferries.py`'s `rank`): a feed's count, then a static file's (in
  the zips' order), then OSM's `interval` tag.

#### 6.7.1 Feeds

**Kind:** source declarations. **Drop box:** `inputs/timetables/feeds/`, unit `timetables/feeds`.
Code: crate::inputs::feeds; `dem/railfeeds.py`; crate::rail.

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
`replaces`. A `key` missing from `inputs/keys.env` isn't a finding: the feed waits. What a fetch
brings back is the fetching job's status, not a finding.

**Fetching.** `rail-feeds` (plan §6) reads the accepted declarations (`--feeds-index`) and the
catalogue's copy and writes `sources/rail/feeds` and `sources/rail/ferry-feeds`, each zip pinned by
content in `sources/rail/gtfs/<id>` with its day (`sources/rail/fetched`). Its key names the index,
the catalogue's copy, the coverage, the outlines and which keys `keys.env` holds. `rail` keys on the
rail list, `ferries` on the ferry list. With a catalogue copy but no accepted catalogue declaration,
rail-feeds fails rather than read it uncorrected. A declaration's `pin` is put in by rail-feeds the
first time it runs with no zip for that feed, so a pinned feed is never fetched before its pin.

#### 6.7.2 Static GTFS

**Kind:** declare (the fulfil half of §6.7.3's to-do). **Drop box:** `inputs/timetables/gtfs/`,
unit `timetables/gtfs`: `<name>.zip` (letters, digits, `-`, `_`), each a GTFS feed compiled by hand,
with an optional `<name>.toml` description (held together). The researchers' brief is in `how/`.
Code: crate::inputs::gtfs; `dem/staticgtfs.py`; `producers/timetables/` (`gtfszip.py` and others).

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

`rail` hands off `work/rail/lines` (railgtfs.py's `--routes`, railfreq's misses, the service
relations); `ferries` hands off `work/ferries/lines`. `timetables-todo` (keyed on both, the coverage
and its version) merges them, writes the to-do into `todo/` outside the records (as `names-todo`
does), and hands off the report, which the lead reads for its status (`Status::timetables`).
Planned: matching rail routes to lines by their tracks (today a route with one matched pair isn't
reported).

### 6.8 Elevation

**Kind:** source declarations, one per dataset; a small dataset's files may lie beside it. **Drop
box:** `inputs/elevation/`: `<dataset>.toml`, tile indexes (`<name>.geojson`) and `<dataset>/` for
local files. Code: crate::inputs::elevation (the check), crate::dem::generic and crate::dem::declared
(the sampler and blend), crate::dem::encoding, crate::dem::kept and crate::dem::fabdem_fetch (the store), crate::terrain_sources
(the terrain's), crate::dem::spacing (the densification), crate::dem::geoid (the heights brought
to EGM2008). Built for the roads' elevations (#143 step 1), the densification (step 2), the
heights' conversion (step 3) and the 3D terrain (#144).

**Shape:** a description (§5.1) and:

| Field | Type | Meaning |
|---|---|---|
| `surface` | `ground` or `surface` | a DTM, or a DSM with trees and buildings |
| `heights` | `EGM2008`, `EGM96`, `CGVD2013`, `NAVD88`, `JGD2011`, `TWVD2001`, `ellipsoid-WGS84` | the height reference, brought to EGM2008 before the blend (below); `mixed` allowed for the terrain's alone |
| `rank` | integer, default 0 | overrides the order by resolution (lower first), where datasets of one resolution differ in quality |
| `code` | 1–8 | the source byte (at most `roadcore::NDEM` − 1, where the server counts a way's sources) in the units' outputs (`src.u8`), named on the map by `label`; none for the terrain's |
| `label` | string | the short name a road's details give its elevation source (`"GSI lidar (5 m)"`), else `name`; one label per `code` (an `el-decl` error otherwise); not keyed: it reaches the map through the catalog's `dem_labels` (formats.md, Catalog), as a credit edit does (§4.8) |
| `terrain` | bool, default false | read by the 3D terrain, never by the roads' elevations (below) |
| `territory` | ISO codes or `"world"` | required for a tile pyramid (its coverage is its territory's outlines' boxes); `"world"` makes any dataset's coverage the world (FABDEM) |
| `[data] format` | `geotiff` or `tiles` | |
| `[data] url` / `urls` / `index` / `files` | | exactly one: GeoTIFFs by URL, a tile index (GeoJSON footprints with each file's `url` or `file`, and optionally its fill mask's URL, `mask`; made by `producers/elevation/indexes.py`), or dropped files by glob |
| `[data] store` | bool | the files read from `sources/fetched/<dataset>/` alone, by their URLs' last part (after a `#`, the member's name); never fetched by a build: a file the store lacks is an error naming it, one noted `<name>.none` has no data |
| `[data] encoding`, `url`, `zoom` | | for `tiles`: `terrarium`, `terrain-rgb`, `gsi-png`; the URL template; the zoom |
| `[data] read_resolution` | metres | read a finer dataset at its nearest overview (HRDEM's 2 m at 8 m) |

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
read_resolution = 8
```

**Read from the data at check time** (`facts.measured`, beside the parsed `facts.declaration`): a
GeoTIFF dataset's CRS and resolution (headers by range request: an index's first, last and seven
files between, or every file without an index; one CRS the app projects, the coarsest resolution at
the overview read) and its coverage (`cover`): the 1° cells its data meets, merged into boxes. Index
files meet by footprint; other files by where they have valid data at their coarsest overview, so a
file's empty margins cover nothing. A tile pyramid's resolution comes from its zoom at its coverage's
mid-latitude, and a tile is read to check it's 256 px (`generic::TILE_PX`). The coverage is also the
credits' extent (§4.8).

**Checks.** Errors: a file not of the shape (`el-shape`); a description field wrong (`el-desc`); a
declaration's fields wrong: unknown `surface`, `heights` or encoding, no `code`, a template without
placeholders, not exactly one way of naming files (`el-decl`); an unparsable index (`el-index`); an
index or files missing (`el-missing`); unreadable or unmeasurable data: a header, a missing file, a
CRS the app can't project (EPSG 4326, 4269, 3979: `pipeline::dem::proj`) or contradicting its index,
a pyramid without a known territory or with non-256 tiles (`el-data`). Warnings: a folder or index
no declaration names (`el-orphan`); two datasets of one rank, resolution and surface meeting
(`el-tie`). A declaration is held with the index it names. Planned: files not meeting the declared
territory.

**Ingesting.**
- **Order at a point:** the datasets covering it by `rank`, then resolution (finest first), then
  ground over surface, then id; the first with valid data gives the height and its `code`. A
  dataset's files are tried in index order, a point in those whose footprint holds it (min x ≤ x <
  max x, min y < y ≤ max y), sampled by crate::dem::sample.
- **Blended at edges:** where the next dataset with valid data (B) lies beneath the first (A), within
  a band of four of the coarser dataset's pixels of A's invalid area, h = B + (A − B)·s(d / band),
  s(t) = t²(3 − 2t). Past the band, A's height to the bit; the source byte is A's either way. The
  distance is looked for pixel by pixel only where one can be found: each block (or tile) read for
  sampling is noted clean when every pixel of it holds data (crate::dem::sample::Clean), and a
  point whose band lies in clean blocks inside its file is at the band's full distance, no pixel
  read again. The decoded blocks of every file share one cache (`generic::CACHE_BYTES`, 512 MB),
  so a unit opening tens of files holds that much, not a cache's worth per file. A block several
  threads ask for at once is read and decoded once (`Tiff::block`), the others waiting for it.
  A GeoTIFF dataset's blocks are read ahead of their sampling (`generic::read_ahead`, natively):
  `GEO_FETCHES` (32) at once, at most `GEO_AHEAD` (128) blocks past the chunk being sampled, so
  the sampler doesn't wait on a chunk's few blocks one round trip after another. Only each point's
  first file (the first whose footprint holds it) is read ahead; a point that file has no data for
  reads the next file's block when it's sampled. The reading ahead goes no further than a quarter
  of the cache holds (`generic::ahead_for`), and it stops with the sampling: a read of it waiting
  out its retries gives up at once (`fetch::cancellable`), and every retry checks its host's
  down note first (`SCENIC_FETCH_DOWN`), so a host that stopped answering doesn't hold the step
  through each thread's retries. The fetcher holds
  a process's requests to one host to `fetch::net::HOST_REQUESTS` (64) at once.
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
  covers keeps its height as sampled, counted (`dem-stats.json`'s `unconverted`; a blended one whose
  first dataset was converted and whose dataset beneath had no value, `unconverted_seam`: plan.md
  §10); a dataset no chain reaches is left as it is, said by the agent (the status's `notices`,
  apart from the waits: an info line on the build page and "note: elevation heights — not
  converted to EGM2008: …" in scenic status) and by the unit job (`elevation: <id>: its heights …
  left as they are`); a unit job given no accepted sources says it converts nothing.
  `producers/geoid/fetch.sh` reads each host's robots.txt first, alone, judged by
  `producers/geoid/robots.py` (RFC 9309 groups, our token's group over `*`'s, Allow and Disallow by
  longest match with `*` and `$`, percent-encoding normalised; `scenic-roads/…` counts as ours; an
  error running it stops); unreadable counts as allow (redirects followed, up to 5). Only
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
  - **Keys:** a converted dataset's line in a unit's key (and the plan's `tag`, by which kept DEM
    samples are judged) names its chain's digest (the sources' ids and `keyed`, `Conversion::digest`):
    `<id> <keyed> to EGM2008 <digest>`, so declaring or editing a grid rebuilds just the units
    meeting a dataset it converts. A sample made under the DEM rules (the seed's) is never valid
    for a converted dataset (`DemValidity`: those weren't converted). With the six declarations
    accepted, the units meeting 3DEP, HRDEM, MRDEM or GSI's datasets are built again: 241 of the 285
    keyed units (by their z6 tiles, 18 Oct); FABDEM's alone keep their keys.
- **FABDEM** is declared with `territory = "world"` and the coarsest rank: the worldwide fallback.
  It's `store = true`: all of FABDEM V1-2 (19,011 tiles, about 297 GiB) is kept once in
  `sources/fetched/fabdem/`, and area builds only read it. Its index still names each tile by its
  Bristol zip member (`<zip URL>#<tile>.tif`): the URL names the tile and its stored file, and the
  index is unchanged so no unit's key changes. The store is filled by `scenic-build fabdem-fetch
  --root <nas> --mirror https://huggingface.co/datasets/pavelzaitsau/fabdem-v12 --work <local dir>
  [--first <square>,…]` (crate::dem::fabdem_fetch), from that Hugging Face mirror (the same pixels,
  two tiles compared; Bristol's server resets our requests):
  every tile of the mirror's listing the store lacks, one at a time, the 10° squares named first;
  each downloaded into the local work folder (resumed by range with `If-Range` and the ETag compared,
  so a changed file starts again; retried with backoff; one fetcher a folder, by a lock), its size
  and SHA-256 checked against the listing's (its LFS oid; a mismatch is fetched once more), decoded and written as every stored tile is (`dem::kept::store`:
  rewritten, read back and compared under a temporary name, then renamed). It skips stored tiles,
  so a stopped run is started again as it is. The input check measures a stored dataset's sampled
  files from the store too, so fetch the tiles first, then change FABDEM's declaration.
- **Keys:** a unit names the `keyed` digests of the datasets whose coverage meets its ways
  (`agent::build::unit_key`); a dataset's `keyed` includes its index's content and dropped files, so
  adding a tile reruns the units it meets. `agent::rekey` keeps what keys made before the
  declarations need (`dem_rules_meeting`, `pre_elevation_unit_key`).
- **The switch:** the check records `sources/inputs/elevation/@switched` (the accepted index and the
  published app that recorded it) once a version of the declarations is accepted; a development
  build records none. Until then `elev` and the unit job don't run and the units wait (the agent
  says why). From then on no older app leads, is handed the lead or is granted work
  (`regions::read::switch_app`, the coordinator's `min_app`).
- **The unit's plan:** the unit job writes `elevation.json` into the unit's folder (the datasets in
  order, each with its files meeting the unit's vertices: `dem::declared::plan`), which `elev` reads,
  natively or as WebAssembly; without one `elev` refuses to run.
- **The DEM cache** (`sources/dem-cache/`, `cache/dem-units/`, `unit::DemValidity`): a samples file
  made from the declarations records their digest and is valid while it holds. Older samples are
  valid only where a single dataset covers the vertex and the sample carries its code. The seed
  (`sources/dem-cache/`, made under the DEM rules) is neither copied to a Mac nor read for a unit
  when two datasets or more cover every 1° cell its vertices meet (`DemValidity::seed_serves`):
  none of its samples could be valid there. It's copied, once a Mac, by the first unit it can serve.
- **Densification** (crate::dem::spacing, `extract`): a way's points are put 8 m apart (`FINE_M`)
  where a roads' dataset finer than 15 m (`COARSE_M`, its measured `resolution`) has data in the 1°
  cell of the way's first node (its measured `cover`), 15 m elsewhere; ferries not at all. A cell
  is the point's degrees floored: its west and south edges are its own. The unit job writes the
  fine cells into the unit's folder (`spacing.json`), which `extract` reads; the reach tests the
  units' long ways at the same spacing (`reach::LongWay::touches`). The other steps' `extract` runs
  (the links, rail and candidates) read the same file (`Fine::of_records`); the rail and candidates keys name its whole digest, read by the planner from the accepted index (`spacing::digest_read`, cached by version; unreadable, they wait), the links are the pass's (made once a pass). Today's datasets make HRDEM,
  3DEP (10.3 m, 1/3″) and GSI's fine, MRDEM and FABDEM (30 m) not.
  - **Keys:** each unit's reach records the cells its ways start in (`Reach::cells`); its key
    names those of them that are fine (`Fine::key`), so a dataset added, dropped or measured
    again rebuilds just the units with a way starting where the spacing changes.
  - **Re-keyed** (`agent::rekey::rekey_spacing`): the spacing was 8 m west of 40° W and in a box
    over Japan, 15 m elsewhere (`rekey::Before`). A unit current under its key from before whose
    every cell is wholly fine or wholly coarse under that rule and the same now is recorded under
    its key now: `extract` puts the same points. The others (the units of Mexico, Central and
    South America, the Caribbean, Greenland, French Guiana, the Northwestern Hawaiian Islands and
    the parts of Canada and Alaska without fine data, which go to 15 m; Guam and the western
    Aleutians, which go to 8 m; and any unit with a way starting in a cell the rule split, at 122.5° E or
    46.5° N) are built again.
- NRCan's tile squares, the HRDEM index producer's input, are `producers/elevation/hrdem-squares.geojson`.

**The 3D terrain's datasets** (`terrain = true`; crate::terrain_sources): left out of the roads'
order, keys, plan, cache and ties. Two are read: a tile pyramid in the `terrarium` encoding, the
terrain's base (AWS's Terrarium tiles, `heights = "mixed"`: its sources mix references), fetched
raw by its template and packed onto the NAS (`sources/aws-terrarium/`, plan.md §4); and at most one
GeoTIFF dataset of 1° tiles by an index in EPSG:4326, each tile with its fill mask (`mask`),
blended in north of 60°N (Copernicus GLO-30: the index from the bucket's tile list,
`indexes.py glo30`), its files kept in `sources/fetched/<id>/` by their URLs' names. The repair,
the water's flattening, the blend by latitude and the mask's reading (3 and up: filled from another
source) are the terrain's code. The declarations themselves are kept in
`producers/elevation/terrain/` (`terrarium.toml`, `glo30.toml`).
- **Keys:** a terrain piece names the terrain's declarations by their ids and `keyed` digests
  (`terrain_sources::pin`, in `agent::build::terrain_piece_key`); with none accepted (or one that
  can't be read) its key is unknown and the terrain waits. The roads' keys and plan don't change
  when a terrain declaration does.
- **Re-keyed** (`agent::rekey::derive_sources`): a piece recorded under the key that named its
  sources as code had them (`rekey::SOURCES_BEFORE`) is read under its key now when the declarations
  name the same: the base's template, the northern tiles' and masks' origin and the set of 1° cells
  match what the code read (`rekey::named_before`, by digests; the cells' is `rekey::cells_digest`
  of the tile list it read, `NORTH_CELLS_BEFORE`). The packs are then the same bytes
  (`terrain_pack::tests::the_declared_sources_make_the_same_terrain_and_slope_packs`), and slope,
  keyed on the terrain packs' contents, is unchanged with them. Other sources: every piece made again.
- **The rollout:** first both Macs run the app that reads the declarations; then the northern
  dataset's files are copied (not moved) from `sources/copernicus-dem/` to `sources/fetched/glo30/`
  (the same names), so an older app mid-build still reads the old folder, which is deleted later;
  then the two declarations and `glo30-tiles.geojson` are dropped into `inputs/elevation/`. Until
  they're accepted the terrain waits.

**Planned:**
- **A finer cache validity:** each cached height recording the datasets at or above its rank that
  meet the vertex, so a finer dataset added restales only those vertices; the source byte becoming
  the dataset's position in that list.
- `[data] valid` (a GeoJSON of where a dataset is trusted) and ETag pins for `range` sources (§5.2).

### 6.9 The remaining sources

**Kind:** source declarations. **Drop box:** `inputs/sources/`: `<id>.toml` each, and a source's
index (`<name>.geojson`) beside it. Code: crate::inputs::sources (the check, and `Sources`, the
accepted declarations by role, which the steps read). Built for the sources in the table marked
built (#145); the declarations themselves are kept in `producers/sources/`, with the producers of
their indexes.

**Shape:** a description (§5.1) and `[data]`:

| Field | Meaning |
|---|---|
| `role` | the slot the app reads it in, one source a role: `land-cover`, `canopy`, `leaf-type-europe`, `leaf-type-north-america`, `wikidata`, `wikidata-live`, `wikipedia`, `pageviews`, `lake-fill`, `buildings`, `building-heights`, `regions-sea`, `regions-internal-waters`, `regions-territorial-seas`, `regions-lakes`, `regions-land` (§6.1.7); and `depth` (§6.10) and `geoid` (§6.8, step 3), which any number of sources take |
| `format` | one of its role's: `geotiff`, `geojson`, `image-service`, `zip-geotiff`, `zip-shapefile`, `sparql`, `mediawiki-api`, `pageviews-dump`, `geoparquet`, `depth-grid`, `geoid-grid`, `helmert` |
| `url` / `urls` / `index` | exactly one; a template's placeholders only its role's (`{tile}` for land cover, `{y}` `{m}` for the pageviews); an index is a GeoJSON beside it, a feature per file with its `url` and footprint |
| `version` | the pinned release |
| `refresh` | `pinned` (default: a new release is the owner's edit) or `live` (a query service asked each run) |
| `[data.params]` | the format's own fields (a legend, a box, a layer and the values kept) |

**Checks.** Errors: a file not of the shape (`src-shape`); a description field wrong (`src-desc`);
a declaration's fields wrong: an unknown role, a format not its role's, not exactly one way of
naming where it is, a placeholder its role doesn't fill, an unknown `refresh` (`src-decl`); an
unparsable index (`src-index`); an index named that isn't there (`src-missing`); two declarations
of one role (`src-role`; for `depth`, two of one `code`). Warning: an index no declaration names (`src-orphan`). Derived: each
description's extent for the credits (§4.8), the world or its territory's polygons' boxes in the
newest pass's outlines.

**How the steps read them.** Rust steps through `inputs::sources::open`/`read` (one read per
accepted version); a unit's programs from `sources.json` in its folder (`unit::UnitSources`: the
land cover, the canopy files' URLs); a tree cover run from `sources.json` beside its coverage
(`trees::RunSources`, passed to the trees program, `trees.py` and `leaftype.py` as `--sources`);
Python steps by role from the file `SCENIC_SOURCES` names (`dem/declared.py`, written by
`inputs::sources::write_for_python`). The jobs that read them (the units, tree cover, items, the
heritage chain) wait until a version is accepted (the agent's `hold_for_sources`); the lake fill,
and the water, terrain and catalog that read it, wait too (`lakefill::run::planned_source`); so do
the roadside buildings' job and the units' keys (the release, `buildtiles::release_of`, held by
`agent::tiles` for the keys) and the 3D buildings' chain (no "bld-key" in `agent::input_digests`).

**Keys.** A key names a declared source only where it named the source in code before: the lake
fill's key names its zips' pin, and the declaration only once it names other than the code had
(`Source::key_extra` against `agent::rekey::declared_before::LAKE_FILL`, the staged declaration's
digest); the roadside buildings' (the units', the buildings job's) and the 3D buildings' (bld-fetch,
bldprep) keys name the declared release, and the declarations only once they name other than the
code had (`bld::sources::declared`: `Source::key_part` against `declared_before::BUILDINGS`, and
`key_extra` against `BUILDING_HEIGHTS`), so while they're as staged every key is the one it was, and
the stores keep their names (`sources/buildings/<release>/`, `sources/overture/<release>/`,
`sources/ghsl/<version>/`, its version `R2023A`). The land cover, canopy and leaf types entered no
key in code; a unit's key names the land-cover and canopy declarations, a tree cover piece's the
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
| OSM planet | kept in code: `tools/nas/fetch-planet.sh` runs on the NAS under DSM's Task Scheduler, outside the app, and can't read the accepted version through the accessor (§7.1) | — |
| Planetiler and its data | kept in code: `water::PLANETILER_VERSION` is a code contract (the water layer's simplification was checked against that version's z14 water, `osmpass::check_planetiler`), and Natural Earth, the water polygons and lake centrelines are the jar's own downloads (`--download`), named by it, not by the app | — |
| ESA WorldCover | built: `esa-worldcover.toml`, the `land-cover` role | `geotiff`, URL template (`{tile}`), version; its class table is its legend (`params.legend`) |
| Meta & WRI canopy height | built: `meta-canopy.toml`, the `canopy` role, its index `canopy-squares.geojson` (`producers/sources/canopy.py`, from the bucket's listing: the equator row's `lat=-0.0` read there); a file the index doesn't list is none there | `geotiff` with an index: a file's `url`, `kind` and 10° square |
| Leaf type (EEA HRL, NALCMS) | built: `eea-leaf-type.toml`, `nalcms.toml`, the `leaf-type-europe` and `leaf-type-north-america` roles; the build still makes the squares where a run needs them (`dem/leaftype.py`, given the sources), since the coverage decides which | `image-service` and `zip-geotiff`: URL, the box each covers, NALCMS's zip member |
| Overture buildings | built: `overture-buildings.toml`, the `buildings` role (the release its `version`; `dem/buildings.py`, `dem/bldfetch.py` read the bucket by role); `bld/fill.rs`'s `ESTIMATES` stays in code, the fill's rule 2 (its tables were fitted with it) | `geoparquet`: the bucket, release, its DuckDB path and region, the types read |
| GHSL building heights | built: `ghsl-built-h.toml`, the `building-heights` role, its index `ghsl-tiles.geojson` (`producers/sources/ghsl.py`, from the tiles folder's listing: the tiles' grid read there) | `zip-geotiff` by an index of its tiles, its version naming its folder |
| Wikidata, Wikipedia's API | built: `wikidata-qlever.toml`, `wikidata-query-service.toml`, `wikipedia-api.toml`, the `wikidata`, `wikidata-live` and `wikipedia` roles | endpoints (`sparql`, `mediawiki-api`, `refresh = "live"`) |
| Wikipedia pageviews | built: `wikipedia-pageviews.toml`, the `pageviews` role | URL template (`{y}`, `{m}`); the months stay the items job's rule |
| Geoid grids | staged, not yet dropped: `producers/geoid/declarations/` (the `geoid` role, any number; the grids by `producers/geoid/fetch.sh`), read by the elevation step (§6.8, step 3) | `geoid-grid` (a URL, its two references, `store`) and `helmert` (its parameters), pinned |
| The regions' datasets (Marine Regions EEZ v12, Internal Waters v4, Territorial Seas v4; Natural Earth lakes; OSM land polygons) | staged, not yet dropped: `producers/sources/` (§6.1.7); the files fetched into `sources/fetched/marineregions-eez-v12/`, `marineregions-internal-waters-v4/`, `marineregions-territorial-seas-v4/`, `natural-earth-lakes-10m/`, `osm-land-polygons/` on 2026-10-18 | `geojson` and `zip-shapefile`, their file (`params.path`), id field and layer |
| Canada's lake fill (CanVec) | built: `canvec-lakes.toml`, the `lake-fill` role; its zips kept by hand in `sources/fetched/canvec-lakes/` | `zip-shapefile`: where they come from, its layer, field, values kept, the intermittent ones and the confirming share (`params`) |

Also kept: fonts (`scripts/fonts.sh`, the app's own assets). The depth's sources are §6.10's, NCEI's
grids on the NAS (`sources/ncei-crm/`, `sources/ncei-greatlakes/`) among them.

**The rollout:**
1. Both Macs run the app that reads the declarations. Until a version is accepted, the units, tree
   cover, items, heritage chain, lake fill (and the water, terrain and catalog that read it), the
   roadside buildings' job and the 3D buildings wait, and say so.
2. CanVec's zips are copied with their times kept (`cp -p`) from `sources/canvec/50k/` to
   `sources/fetched/canvec-lakes/`, and each copy's modification time checked to match its
   original's to the second (`stat`): over SMB a time not kept changes the fill's pin, and its key,
   and the fill is made again. A declared lake-fill source whose folder isn't there holds the fill
   and what reads it (`lakefill::run::planned_source`), so the copy comes before the drop.
3. The files of `producers/sources/` (the `.toml` declarations, `canopy-squares.geojson` and
   `ghsl-tiles.geojson`) are dropped into `inputs/sources/`. Once accepted, everything goes on under
   the keys it had.
4. `sources/canvec/50k/` is deleted only after both Macs run the new app (an older app still reads
   it).

### 6.10 Depth

**Kind:** source declarations of the `depth` role, any number (crate::inputs::sources `MANY`), in
`inputs/sources/` beside the others, each with its index of depth grids. Code: crate::depth::grids
(the build's reading), crate::depth (the steps, docs/plan.md §6, Depth). The declarations are staged in
`producers/depth/declarations/`; the producers that make their grids in `producers/depth/`.

**The standard: depth grids** (`format = "depth-grid"`). A source is a set of GeoTIFFs, each on the
global Web Mercator pixel grid of one zoom (EPSG:3857; its pixels 40,075,016.686 / 256 / 2^zoom m,
their edges on that grid, its own edges on its zoom's tile edges where its data allows), so the build
reads them by pixel arithmetic, never reprojecting:
- `file`: float32 depth below the water's surface in metres, negative (the sea's mean level; a lake's
  mean surface), drying heights positive, NaN where there's none; tiled, Deflate with the
  floating-point predictor, overviews by averaging;
- `zoom_file` (optional): uint8, the zoom each cell's data supports: its own where measured, coarser
  where the source's flags say interpolated or predicted; overviews nearest;
- `low_file` (optional): float32, the chart's low water (MLLW, LAT, chart datum) below MSL, m ≥ 0, for
  the "dries at low tide" flag.

Its index (`<name>.geojson`, a feature per grid): its footprint, `url` (where the data came from),
`file`, `zoom`, `zoom_file`, `low_file`, `hash`. The files are kept in `sources/fetched/<id>/` (the
producer writes them; the owner copies them there).

**`[data.params]`:** `code` (1–15, the source byte the class tiles carry, one a source), `water`
(`sea` or `lake`: where it's drawn: a sea source stays off the lakes, a lake source in them),
`label` (the map's name for it), `datum` (what its depths are below, in words), optionally `band_m`
(ground metres it's feathered in over where it ends in the water), `intertidal` (its positive values
are drying heights), `datum_exact` (false: not corrected to the others' datum, so it fills gaps only,
hard-edged), `rivers` (drawn in the basemap's rivers too: its depths there are below the river's own
surface, not a fixed level's), `estimated` (every cell an estimate: classed so) and `notice` (a
notice its licence asks every map showing its data to carry: shown in full in the map's © Credits,
under a "Depth:" line naming every source's `label` and "Not for navigation."; nothing floats over
the map).

**Checks** (§6.9's, and): a depth declaration's params (`src-decl`); its index's features each naming
a `.tif` `file` and a `zoom` 0–18 (`src-index`); two of one `code` (`src-role`).

**The sources** (`producers/depth/declarations/`; the owner's decisions of 6 and 17 October 2026):

| id | what | code | water | producer | detail |
|---|---|---|---|---|---|
| `gebco-2026` | the yearly global 15″ grid and its type identifiers | 1 | sea | `gebco.py`, from the owner's two zips in `sources/gebco/2026/` (CEDA's robots.txt bars programs) | z8; cells the TID calls interpolated or estimated (40–48, 71, 72) at z7 |
| `bluetopo` | NOAA's US coastal grids, 4–16 m | 2 | sea | `bluetopo.py`'s steps, from the public bucket by its tile scheme (checksums checked), a chunk of tiles at a time (`chunked.py bluetopo`); NAVD88 → MSL and MLLW by NOAA VDatum (`vdatum.py`, a point per 0.25° cell; `stations.py noaa` gives the same table from the tide stations) | z15 (4 m) to z13 (16 m); interpolated contributors at z12 |
| `nonna-100`, `nonna-10` | CHS's 100 m and 10 m grids (licence accepted; never redistributed; its notice names the owner as the maker) | 3, 4 | sea | `nonna.py`, the keyless coverage service (NONNA-10 a chunk of blocks at a time, where NONNA-100 has water: `chunked.py nonna10`); chart datum → mean water level by CHS's stations (`stations.py chs`) | z10, z13 |
| `emodnet-dtm` | EMODnet's DTM 2024, MSL version | 5 | sea | `emodnet.py`: its MSL ASCII tiles one at a time (a WGS 84 `.prj` given each), `raster.py`'s produce; its interpolation flag not applied (plan §10) | gaps only (`datum_exact = false`) |
| `shom-homonim-100`, `shom-homonim-20` | SHOM's façade and coastal DEMs, MSL versions | 6, 7 | sea | `raster.py`, the low water SHOM's own separation (`--low-raster`) | z10, z13 |
| `gttm` | NIED's 2″ grid of Japan | 8 | sea | `raster.py` over windows of it | z11; coarse meshes at z9 |
| `masswildlife-ponds`, `mn-dnr-lakes`, `gblq` | surveyed lakes (Massachusetts, Minnesota, Québec) | 9–11 | lake | `raster.py` (feet or metres, sign as each says) | z16, z15, z16 |
| `ncei-crm` | NCEI's Coastal Relief Models, 1″ (the owner's download, `sources/ncei-crm/`; volume 6 absent) | 12 | sea | `ncei.py crm`; volumes on EGM2008 taken as MSL, uncorrected | z12 |
| `ncei-greatlakes` | NCEI's Great Lakes bathymetry, 3″ (the owner's download, `sources/ncei-greatlakes/`) | 13 | lake | `ncei.py greatlakes`: from each lake's low water datum (CO-OPS' station datums) to its long-term mean level (GLERL's coordinated lake-wide monthly means, 1918 on), both fetched as it runs | z10 |
| `globathy` | GLOBathy's maximum depths (CC0) on HydroLAKES v1.0's outlines (CC BY 4.0), every lake of 10 ha and more: estimates | 14 | lake | `globathy.py` (below) | z11; `estimated`, gaps only (`datum_exact = false`): a survey wins |
| `smalllakes` | the basemap's (OpenStreetMap's, ODbL) lakes and ponds under 10 ha that HydroLAKES lacks, each one's maximum depth estimated from the land's slope around it: estimates | 15 | lake | `smalllakes.py` (below) | z14; `estimated`, gaps only (`datum_exact = false`): a survey and GLOBathy win |

`rivers = true` is staged for `nonna-100`, `nonna-10` (mean water level by CHS's stations, which
follow the St. Lawrence up to Montréal: NONNA-100 has values there), `bluetopo` (local mean sea
level by VDatum, which covers only the tidal reaches) and the SHOM grids (mean sea level, in the
estuaries' tidal reaches). The others' depths inland are heights on a fixed level (GEBCO, CRM,
EMODnet, GtTM) or a lake's own, so they stay out of rivers.

A chunked run (`chunked.py`) fetches a chunk, produces it, tars its grids over SSH into the NAS's
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

Every request carries the User-Agent "scenic-roads/0.1 (personal offline map)" and asks the host's
robots.txt first, in a request of its own (`grid.robots_allow`); a path it bars stops the producer,
which says to download the file by hand.

`globathy.py` (`test_globathy.py`) reads GLOBathy's basic-parameters zip (fetched into `--cache`, or
`--params`) and the owner's HydroLAKES v1.0 polygons zip (`--hydrolakes`; data.hydrosheds.org's
robots.txt bars programs, so it's never fetched), and makes a z11 grid a z8 tile: each lake a bowl
D = Dmax · (l / L)^0.75 on its outline (l the distance to shore smoothed by L/8, the depth lab's
fit), several `--box`es a run, each owning the tiles whose north-west corner is in it.

`smalllakes.py` (`test_smalllakes.py`) reads the basemap's z14 water and the z12 terrain from this
machine's app server (`--server`, default `http://127.0.0.1:7311`; nothing from the internet) and
the owner's HydroLAKES v1.0 polygons zip (`--hydrolakes`), and makes a z14 grid a z10 tile. Its
lakes: class lake or pond, merged by OSM id over the z10 tile's z14 tiles and a ring of one more (a
lake reaching past them is skipped and counted), under 10 ha, not intermittent, not within 3 m of
the sea, and not a fifth or more covered by a HydroLAKES polygon (GLOBathy's). Each one's maximum
depth by the depth lab's fit (its validation.md §3): ln Dmax = −3.820 + 0.363 ln S + 1.983 ln L −
0.1324 (ln L)², ln L held at its peak (L ≈ 1,790 m) beyond it, S the mean land slope 30–100 m from
the shore (other lakes and terrain at or below 0.3 m left out), L the largest distance to shore;
under 3 land pixels in the ring or S < 0.003, the area rule Dmax = 3.04 · A^0.228 (A in ha). On the
lab's 85 measured lakes under 10 ha: median error 48 %, half within ±50 % (the area rule: 53 %).
Each lake is `globathy.py`'s bowl at z14. Several `--box`es a run, each owning the z10 tiles whose
north-west corner is in it; the index is written after each box, and a stopped run resumes (the
tiles finished are listed in `<index>.done`). A failed request is tried again after 2, 4, 8 and 16 s;
a 404 reads as no tile, the server's answers are logged a box, and a box whose every basemap tile
was a 404 fails. `smalllakes-run.sh` is the full run, on the M4: its own server instance (own port
and `--home`, `--root` on the NAS's project folder), one call a z6 tile of the coverage spread over
3–4 workers, each box's grids tarred over SSH into the NAS's `sources/fetched/smalllakes/` and
checked there by size, the indexes merged at the end; under `caffeinate`, a box starting only on
mains power or at 30 % battery or more.

**Keys** (docs/plan.md §6, Job keys): a depth target names each source meeting it by its id and
`Source::digest` (its `[data]` and its index's features, their hashes among them), so a new grid
re-keys the tiles it meets.

**The rollout:** docs/plan.md §6, Depth.

## 7. The guards

Built on the pattern of `crates/pipeline/tests/cache_accessor.rs`: one accessor the code must go
through, a test that finds every way around it, and an exemption list (item, anchor, reason) whose
every entry must match a line (`every_exemption_matches_a_line`). Both run before every publish,
first, before the build (`tools/app/guards.sh`, which `tools/app/publish.sh` runs; a failure stops
publishing; the tests' whole output in `~/Library/Logs/scenic/guards-<time>.log`).

### 7.1 Drop boxes only through the accessor

- **Rust:** `inputs::open` (§4.6) and the input check's job are the only code given drop-box paths.
  `Out::path` (the project root type) panics, naming its caller, when asked for a content name under
  `inputs/`, `translations/` or `descriptions/`. A step gets its inputs' paths from the agent.
- **The test** (`crates/pipeline/tests/inputs_accessor.rs`): every non-test item in `crates/` (a
  function by its block, a `const` or `static`, or the file's top level) whose code names a drop box
  (`"inputs`, `join("inputs")`, `/inputs/`, `"translations`, `}/translations`, `"descriptions`,
  `}/descriptions`, each unit's name; `sources/inputs` aside) must call an accessor (`inputs::open(`,
  `inputs::gate::run(`) or be exempt by item name and line anchor with a reason. No reader is
  exempt: only the Regions panel's writes, the backups, GC's never-swept list, the controls
  (`inputs/hold-catalog`, `inputs/keys.env`) and words that aren't paths (the `inputs` step's name, a
  task spec's "inputs", log lines). The scanner's own rules have a test.
- **Python:** steps receive input paths on their command line. `dem/inputs.py`'s `arg(path)` refuses
  a path not under `sources/inputs/`, `sources/fetched/` or the job's scratch, and the same test
  scans `dem/**/*.py` (functions by indentation) by the same rules.
- **The stand-in root:** `tools/app/guards.sh` loads every Python step the agent runs from the
  app's `dem/` (its tracked files alone) placed in a stand-in NAS root whose drop boxes
  (`inputs/`, `inputs/sources/`, `inputs/elevation/`, `translations/`, `descriptions/`) hold a file
  each and can't be read, then checks them with pyflakes: a step that reads a drop box as it loads,
  by a path the scanner doesn't see, fails there.

### 7.2 No source names in the app's code

`crates/pipeline/tests/source_names.rs` fails when a data source's address or name is in the code
or strings of `crates/` (tests aside: `#[cfg(test)]` items, `tests.rs` files, `tests/`), `dem/`,
`web/src/`, `tools/` (not `tools/taskboard/`) or `scripts/`: their Rust, Python, TypeScript and
JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`), shell, Swift, C, CSS, HTML, JSON, TOML and YAML files.
Comments aren't scanned (nor Python's docstrings and `test_…` functions). `producers/` is exempt: the
declarations are made and staged there.

- **Markers, derived from the declarations staged in `producers/`** (each `.toml` but
  `pyproject.toml`), so a new declaration adds its own: the hosts of its `source`, `url`/`urls` and
  its staged index's files' URLs (a path-style S3 URL's host with its bucket, the host being
  shared); its `name` and `credit`, whole, and up to their first `,` `(` `:` `;` when that's two
  words or more; its name's first two words in any case (as written when both are plain
  capitalised words, "Terrain Tiles", common in lower case); and its id, the file's first word, as
  the name writes it and as a whole word, when it's a word of the name, five letters or more and no
  role's, format's or encoding's word (no `[data]` value of any declaration): today `Overture`,
  `CanVec`, `NALCMS`. And any other URL, but this machine's, the tailnet's (`100.64.0.0/10`), the
  documentation names (`example.*`, `.test`, `.invalid`, `192.0.2.*`) and a host of one label; a host
  behind a template (`${x}.host`) is judged by what follows it.
- **Allowed:** the W3C's namespaces (a format's); OpenStreetMap (its tags are the map's schema; its
  links open a feature or a place there); Wikidata's and Wikipedia's pages opened from an item;
  IRIs in angle brackets (a SPARQL query's vocabulary, and the sites it names as nodes).
- **Exempt**, by file, item and anchor, with why (`EXEMPT`):

| Where | Why |
|---|---|
| `rules::CREDITS` (the declared sources' entries, the CanVec licence's notice) | the credits' fallback (§4.8): a declared source's entry is replaced by its declaration's credit once one is accepted, and stands in until then, so a catalog never lacks a licence's notice |
| `lakefill::run::fill`'s metadata | part of the fill's content name: changed, the fill and the water, terrain and catalog reading it would be made again for nothing |
| `dem/leaftype.py`'s square tags, and `complete` there and in `trees::squares` | a made square's GeoTIFF `source`, as every square already made says it (`trees::squares::complete` reads the tags) |
| `trees::META`, `dem/trees.py`'s meta | the tree tiles' metadata, part of their content: changed, every tree tile would be made again for nothing |
| `dem/leaftype.py`'s `fetch_nalcms` | the reader of NALCMS's zip, made for its layout; its messages name what they read |
| `dem/bldprep.py`'s `read_rg`, `dem/bldfetch.py`'s `overture_jobs` | readers of Overture's schema and release folders, made for their layout; their messages name what they read |
| timing phases (`scenic-build`'s lake fill, `dem/leaftype.py`'s NALCMS squares) | a phase's name is its key in the timings kept |
| `budget::QUERY_HOSTS`, `QUERY_DOMAINS`; `dem/budget.py`'s | the polite-use budget is by host (docs/pool.md §7.7): the query services' limits hold for any request to them |
| `dem/bldfetch.py`'s S3 listing namespace | the protocol's |
| `web/src/main.ts`'s Google Maps link | opens the place elsewhere |
| `tools/check/pool-two-macs.sh`'s budget test | a request the budgets' proxy must meter, not a read |
| `tools/nas/fetch-planet.sh`'s mirrors | kept in code (§6.9, OSM planet): the test checks the row says so (`KEPT`) |
| `scripts/fonts.sh` | the app's own assets (§6.9, also kept) |
| `tools/app/install.sh`'s uv link | documentation, in a message |

Of what §6.9 keeps in code, Planetiler (a version, `water::PLANETILER_VERSION`),
`bld::fill::ESTIMATES` (Overture's dataset name the fill's rule matches) and
`dem/bldmeasure.py`'s store folders name no marker, so none needs an exemption. The 3D buildings'
details name their source from the catalog's credits (`web/src/buildings.ts`, `setBuildingsCredit`).
Short names other than the ids (`Terrarium`, an encoding; `GHSL`, `EEA`) and store folders
(`sources/overture/`) aren't markers.

**What it can't see:** a name or address built at run time (split and joined, `"transl" +
"ations"`, or read from a file the code ships), a short name or abbreviation the declarations
don't give, and anything in a comment. It's a check on what's written, not a proof. The scanner's own
rules have a test (`the_scanner_finds_sources_by_their_declarations`).

## 8. How each input is made

- **Producers live in the repository** under `producers/<input>/` (`heritage/`, `timetables/`,
  `elevation/`, `languages/`, `sources/`, `depth/`): per-source code and staged declarations kept
  out of the app, with their history and review. They're outside what `publish.sh` ships and
  outside the guards' scans; the app never runs them. Each runs on its own (`uv run --project
  producers python producers/heritage/registers.py --data <folder> --out <folder>`), writing the
  drop box's shape, checked when dropped. Their environment is their own (`producers/pyproject.toml`).
- **Each drop box's `how/`** on the NAS says how its input is made and points to its producers; it
  holds what isn't code: the briefs and models of agent-made inputs (translations: Haiku, the
  earlier work's brief and launch prompt; descriptions: Sonnet, the September rounds' briefs; the
  ferry researchers' `PROMPT.md` and batches; the ferry feeds' search) and what was done by hand.
  Its README is kept in the repository, `producers/how/<box>/README.md`, and copied there with any
  briefs; the input check never reads `how/` (§3), so a copy changes no accepted version.
- **`inputs/README.md`** on the NAS describes the drop boxes, the kinds and the input check
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

- **Credits:** `pipeline::rules::CREDITS` and its area boxes (D: each source's description, extents
  derived), and the server's `credits_of` fallback; the tree tiles' meta source string (D); source lines in web/src's layer help and popups
  (trees.ts, ferries.ts, overlays.ts, details.ts: D, from the catalog's credits). The World
  Heritage outlines' popup (overlays.ts, "site: UNESCO World Heritage Centre"): planned, the
  outlines carrying their register's id (`register`) so the popup reads its credit from the catalog
  (`sourceOf`); the overlays step can't add it alone (`ovconv`'s outlines know their dot's id and
  place, not its register), so it waits for the heritage job's next change to its outlines'
  file (`layer-whs-shapes`).
- **Kept by design, credits:** the 3D buildings' height-source legend and hover help
  (buildings.ts `SOURCES`, ui/buildings.ts: "Microsoft", "GHSL", "Esri"): they name the method
  that found a building's height (its `s`, docs/buildings3d.md §2.3), not a credit; the
  buildings' credit itself comes from the catalog (`setBuildingsCredit`).
- **Terrain:** the raw tiles' store path (`sources/aws-terrarium/`, named for its source) and the
  credit in `rules::CREDITS` the base's declaration takes over once accepted.
- **Trees and land cover:** the canopy and leaf-type squares' resolutions (D, read from data).
- **Buildings:** `bld/prep.rs`'s `ghsl` meta (D); `dem/bldmeasure.py`'s own Overture and GHSL
  folders and estimates (P: a measuring tool run by hand, not a build step). `rules::CREDITS`'
  Overture and GHSL entries, which their declarations' credits replace once accepted (§4.8). `bld/fill.rs`'s `ESTIMATES` stays
  the fill's rule (§6.9).
- **Kept by design:** `dem/budget.py`'s and `budget`'s metered hosts (the polite-use budget, by
  host: §7.2); `markconv.rs`'s id schemes (`whc:`, `reg:dfhd:`, `legacy:`: ids must not
  change, docs/phase5.md, Ids); `pipeline::legacy` (a name for live code).
