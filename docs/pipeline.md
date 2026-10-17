# The pipeline: one shape for every step

How the build is described, so that a few concepts read the whole of it: every step is one row of
one table, every job is a step over targets, every text about a step is templated from its row,
and the build page, the menu bar and `scenic status` show the same checklist of stages and steps.
What each step does in detail is `docs/plan.md` §6; how jobs are placed on Macs, `docs/pool.md`;
tasks for any worker, `docs/workers.md`.

## 1. The concepts

| Concept | What it is | Where |
|---|---|---|
| **step** | one kind of work, described by one row of the steps table | `crate::agent::steps::TABLE` |
| **target** | what one run of a step is for: a tile, an area, the world (`scope`) | the plan (`agent::build::Work`) |
| **key** | a target's version and inputs by content; a target is done when its record's key is the plan's | `state/build/jobs.json` (plan.md §6, Job keys) |
| **job** | a step over a batch of its targets, run on one Mac | `agent::jobs::JobSpec` |
| **task** | a pure piece a running job offers any worker, described as a step is | `steps::TASKS`, workers.md §3 |
| **record** | what a job's hand-off puts in the records, within its step's write-set | the manifest and jobs (pool.md §7.3) |
| **stage** | a group of steps in the build's order: the checklist's lines, the page's cards and colours | `steps::STAGES` |

## 2. A step's row

Every row has the same fields; nothing about a step is kept anywhere else.

| Field | Meaning | Values |
|---|---|---|
| `id` (`name`) | the stored name: in the records, the journal, the leases, the timings, the floors, the `work/` paths; never shown as such, never changed (§3) | `unit`, `terrain-lo`, … |
| `title` | the name people see | "Roads", "Terrain, zoomed out" |
| `does` | one sentence: what it makes from what | |
| `kind` | where its output goes | `fetch` (`sources/`, from outside), `derive` (`work/`, `global/`: what other steps read), `pack` (what the map serves), `publish` (a catalog), `keep` (checks, lists, removals, backups) |
| `scope` | what one target is, and the noun its targets are counted in | `world` (one target: a run), `area` (a z3 tile), `tile` (a z6 tile), `input` (a drop box); a task's `task` |
| `stage` | its stage | `pass`, `sites`, `terrain`, `slope`, `trees`, `depth`, `roads`, `tiles`, `finish`, `trains`, `landmarks`, `buildings`, `publish`, `upkeep` |
| `place` | which Mac runs it: derived from the pool's rules (`agent::every::offered`), not kept | `any` (any member whose measured room fits), `lead` |
| resources | its first guess of memory until measured, whether members take it and at what rank (`shared`), the second slot's rank (`beside`), a whole-build run alone (`alone`), whether it keeps the pass's answers (`answered`), targets per job of the lead's own (`batch`), its write-set (`writes`) | pool.md §7.2, §7.7 |

The naming of titles: the thing a step makes (**Terrain**, **Roads**, **Map tiles**); its zoomed-out
assembly by area "…, zoomed out"; its world level "…, world level"; a fetch "…'s sources" or what it
fetches (**Rail timetables**); a task its step's piece (**Terrain's subtrees**).

Every text about a step is templated from its row (`steps::title`, `said`, `counted`):

| Text | Template | Example |
|---|---|---|
| a job's name | `{title}: {n} {noun}s`; a batch `{title}: {n} of {total} {noun}s`; a worldwide step `{title}` | "Terrain: 8 of 480 tiles", "Outlines" |
| a checklist count | `{done} of {total} {noun}s`, `{left} {noun}s left`, "done", "to come" (not sized yet) | "156 of 284 tiles" |
| a step waiting | `{title}: {why}` | "Lake fill: failed 3 times in a row: …" |
| an event | `{who} finished {job's name} in {time}` | "m4 finished Roads: 6 tiles in 10 min" |

The build page, the menu bar and `scenic status` have no words of their own for a step: the page
reads the table from the coordinator (`POST /work/steps`: every row and the stages), `scenic
steps` prints it.

## 3. The table

Ids are the stored names, kept as they are: a rename would call every record of the step stale
(its records, journal entries, timings, floors and `work/` paths all carry it), for nothing people
see. A test pins them (`steps::tests::the_ids_are_the_stored_names`). Place: any member (⇄) or
the lead.

| Stage | Step (title) | id | kind | each target | place |
|---|---|---|---|---|---|
| OpenStreetMap pass | Planet | `osm-pass` | derive | the world | lead |
| | Pass sets | `pass-sets` | derive | the world | lead |
| | Outlines | `outlines` | derive | the world | lead |
| | Route ends | `trailends` | derive | the world | lead |
| | Road reach | `reach` | derive | the world | lead |
| | World terrain, z8 | `terrain-z8` | derive | the world | lead |
| | Roadside buildings | `buildings` | fetch | the world | lead |
| | Summits | `summits` | derive | the world | lead |
| | Place labels | `labels` | pack | the world | lead |
| | Lake fill | `lakefill` | derive | the world | lead |
| | Water | `water` | pack | the world | lead |
| | Spoken languages | `spoken` | derive | the world | lead |
| | Regions' extract | `regions-extract` | derive | the world | lead |
| Heritage sites | Heritage sites | `heritage-sites` | derive | the world | lead |
| Terrain | Terrain water | `terrain-water` | derive | the world | lead |
| | Terrain | `terrain` | pack | a tile | ⇄ |
| | Terrain, zoomed out | `terrain-lo` | pack | an area | ⇄ |
| | Terrain's subtrees (task) | `terrainsub` | | a task of Terrain | ⇄ |
| Slope | Slope | `slope` | pack | a tile | ⇄ |
| | Slope, zoomed out | `slope-lo` | pack | an area | ⇄ |
| Tree cover | Tree cover | `trees` | pack | a tile | ⇄ |
| | Tree cover, zoomed out | `trees-lo` | pack | an area | ⇄ |
| | Tree cover's blocks (task) | `treeblock` | | a task of Tree cover | ⇄ |
| Water depth | Depth, zoomed out | `depth-lo` | pack | an area | lead |
| | Depth | `depth` | pack | a tile | ⇄ |
| Roads | Roads | `unit` | pack | a tile | ⇄ |
| | Roads' last steps (task) | `tail` | | a task of Roads | ⇄ |
| Map tiles | Map tiles | `pack` | pack | a tile | ⇄ |
| | Map tiles, zoomed out | `lo` | pack | an area | ⇄ |
| Map finishing | Prune | `prune` | keep | the world | lead |
| | Road index | `roadunits` | derive | the world | lead |
| | Rail stops | `stations` | pack | the world | lead |
| | Ferries | `ferries` | pack | the world | lead |
| | Terrain, world level | `terrain-root` | pack | the world | lead |
| | Slope, world level | `slope-root` | pack | the world | lead |
| Trains | Rail timetables | `rail-feeds` | fetch | the world | lead |
| | Trains a day | `rail` | derive | the world | lead |
| Landmarks | Landmark candidates | `pois` | derive | a tile | ⇄ |
| | Peaks | `peaks` | derive | a tile | ⇄ |
| | Wikidata facts | `items` | fetch | the world | lead |
| | Heritage details | `heritage` | derive | the world | lead |
| | Landmarks | `marks` | pack | the world | lead |
| | Area overlays | `overlays` | pack | the world | lead |
| 3D buildings | Buildings' sources | `bld-fetch` | fetch | the world | lead |
| | Buildings read | `bldprep` | derive | a tile | ⇄ |
| | 3D buildings | `bldtiles` | pack | a tile | ⇄ |
| | 3D buildings, zoomed out | `bldfield-lo` | pack | an area | ⇄ |
| | 3D buildings' areas (task) | `bldtile` | | a task of 3D buildings | ⇄ |
| Publishing | Catalog | `catalog` | publish | the world | lead |
| | Catalog, held | `catalog-held` | publish | the world | lead |
| Upkeep | Input check | `inputs` | keep | an input | lead |
| | Names to do | `names-todo` | keep | the world | lead |
| | Timetables to do | `timetables-todo` | keep | the world | lead |
| | Clean-up | `gc` | keep | the world | lead |
| | Backup | `backup` | keep | the world | lead |

The forecast's lane for a map update as a whole is `map-update` ("Map update", the publishing
stage's colour): not a step.

## 4. The checklist

The agent's status carries the build to the end as a line per stage (`agent::build::Line`), each
with an item per step counted (`agent::build::Item`: its step, title and noun, and its targets
`done` of `total`, or `left` (a worldwide step's run: 1 or 0; a chain step's: the targets of its
works to come), or neither while it can't be sized yet, "to come"). A line names every step of its
stage (`steps`); its own `done` and `total` count the items of one noun (`unit`: the first sized
item's), never tiles and areas as one, and its `left` every item's targets left (none while an item
isn't sized); its counts in words are a noun at a time ("398 of 398 tiles · 18 of 18 areas", "2
runs left": `Line::counts`, said the same by the page and the menu bar); then its jobs to come in
order (`next`), why it waits or which Mac is on it (`note`), and which of its steps any member may
take (`shared`). `agent::build::fold` makes the lines from the items; the planet's and its
worldwide steps' items are the agent's own (`Agent::checklist`), the regions' `build::checklist`'s.

The same lines are shown three ways, each the same shape: the build page's stage cards (§5), the
menu bar's menu (`tools/status`: "To the end", a line per stage with its counts, under it a line
per step with its count, then its jobs to come; its panel is the page itself), and `scenic status`'s
"To the end" (the same lines, a step's place marked ⇄).

## 5. The build page

`web/work/` (workers.md §7). One rule: an element is a card or a row of the same cells as its
kind's others, with templated words.

- **Verdict:** one sentence (state, done ≈, next on the map), the alerts as chips, the pause
  controls.
- **Overview:** how far (the roads' tiles built, the bulk of the work), how long, the stages as a
  coloured strip; three numbers that aren't a step's (regions on the map, the map's last update,
  machines working).
- **Stages:** a card per stage: mark, title, count, bar; a row per step: mark, title, ⇄ when any
  Mac may take it, count, bar, its forecast (work, done ≈); the stage's jobs to come and note.
- **Machines:** a card per Mac: name, role, state, heard from; its job (name, parts, progress,
  running since, memory; frozen or stuck), beside it its second; what's next; facts as chips; its
  day as a spark line. Pages one card.
- **The pool:** `pool.js`.
- **Road to done:** each machine's lane (segments in the stages' colours, map updates hatched and
  marked), the map updates' decision.
- **Regions:** a row per region: name, state, when, tiles built, what's left by step.
- **Activity:** the hours, and a sentence per event.
- **Details:** tables, among them the steps table itself.

Colours are the stage's (`STAGE_COLOURS` in `dash.js`), never a step's own.
