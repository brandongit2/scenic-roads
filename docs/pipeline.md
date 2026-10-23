# The pipeline: one shape for every step

How the build is described, so that a few concepts read the whole of it: every step is one row of
one table, every job is a step over targets, every text about a step is templated from its row,
and the build page, the menu bar and `scenic status` show the same checklist of stages and steps.
What each step does in detail is `docs/steps.md`; how jobs are placed on Macs, `docs/pool.md`;
tasks for any worker, `docs/workers.md`.

## 1. The concepts

| Concept | What it is | Where |
|---|---|---|
| **step** | one kind of work, described by one row of the steps table | `crate::agent::steps::TABLE` |
| **target** | what one run of a step is for: a z6 tile, a z3 tile, an input, the world (`scope`) | the plan (`agent::build::Work`) |
| **z6 tile** | the build's working grid: most per-tile steps (terrain, slope, trees, `unit`, …) make a z6 tile's files at a time; `6/x/y`, named `6-x-y` in files | `Scope::Z6` |
| **z3 tile** | 8 × 8 z6 tiles: the zoomed-out levels (the `…-lo` steps) are assembled per z3 tile; `3/x/y`, `3-x-y` | `Scope::Z3` |
| **z8 tile** | 4 × 4 per z6 tile: the 3D buildings' tasks (`bldtile`), tree cover's blocks and terrain's subtrees are made per z8 tile | `bld::task`, docs/workers.md §3 |
| **area** | geography only: a polygon or map feature (a region's outline, protected and heritage areas, the area overlays, a rule's areas), never a grid tile; a number is a *size* (km²) | |
| **key** | a target's version and inputs by content; a target is done when its record's key is the plan's | `state/build/jobs.json` (docs/steps.md §12) |
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
| `scope` | what one target is, and the noun its targets are counted in | `world` (one target: a run), z3 tile, z6 tile, `input` (a drop box); a task's `task`. Counted in "z3 tiles", "z6 tiles", … (`Scope::noun`); sent as `scope` `area` and `tile` by `/work/steps` (the stored words, kept: `Scope`'s serde names) |
| `stage` | its stage | `pass`, `sites`, `terrain`, `slope`, `trees`, `depth`, `roads`, `tiles`, `finish`, `trains`, `landmarks`, `buildings`, `publish`, `upkeep` |
| `place` | which Mac runs it: derived from the pool's rules (`agent::every::offered`), not kept | `any` (any member whose measured room fits), `lead` |
| resources | its first guess of memory until measured, whether members take it and at what rank (`shared`), a whole-build run alone (`alone`), whether it keeps the pass's answers (`answered`), its write-set (`writes`). The rest is measured, not kept: each target's memory, disk, cores, time and NAS rate by the step's model (`needs::Book`), targets per job (`coord::JOB_S`), and whether a job runs with others on a Mac (packing, by those measures) | pool.md §7.2, §7.7 |

The naming of titles: the thing a step makes (**Terrain**, **Roads**, **Map tiles**); its zoomed-out
assembly by z3 tile "…, zoomed out"; its world level "…, world level"; a fetch "…'s sources" or what it
fetches (**Rail timetables**); a task its step's part (**Terrain's subtrees**).

Every text about a step is templated from its row (`steps::title`, `said`, `counted`):

| Text | Template | Example |
|---|---|---|
| a job's name | `{title}: {n} {noun}s`; a worldwide step `{title}` | "Terrain: 8 tiles", "Outlines" |
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
| | Terrain | `terrain` | pack | a z6 tile | ⇄ |
| | Terrain, zoomed out | `terrain-lo` | pack | a z3 tile | ⇄ |
| | Terrain's subtrees (task) | `terrainsub` | | a task of Terrain | ⇄ |
| Slope | Slope | `slope` | pack | a z6 tile | ⇄ |
| | Slope, zoomed out | `slope-lo` | pack | a z3 tile | ⇄ |
| Tree cover | Tree cover | `trees` | pack | a z6 tile | ⇄ |
| | Tree cover, zoomed out | `trees-lo` | pack | a z3 tile | ⇄ |
| | Tree cover's blocks (task) | `treeblock` | | a task of Tree cover | ⇄ |
| Water depth | Depth, zoomed out | `depth-lo` | pack | a z3 tile | ⇄ |
| | Depth | `depth` | pack | a z6 tile | ⇄ |
| Roads | Roads | `unit` | pack | a z6 tile | ⇄ |
| | Roads' last steps (task) | `tail` | | a task of Roads | ⇄ |
| Map tiles | Map tiles | `pack` | pack | a z6 tile | ⇄ |
| | Map tiles, zoomed out | `lo` | pack | a z3 tile | ⇄ |
| Map finishing | Prune | `prune` | keep | the world | lead |
| | Road index | `roadunits` | derive | the world | lead |
| | Rail stops | `stations` | pack | the world | lead |
| | Ferries | `ferries` | pack | the world | lead |
| | Terrain, world level | `terrain-root` | pack | the world | lead |
| | Slope, world level | `slope-root` | pack | the world | lead |
| Trains | Rail timetables | `rail-feeds` | fetch | the world | lead |
| | Trains a day | `rail` | derive | the world | lead |
| Landmarks | Landmark candidates | `pois` | derive | a z6 tile | ⇄ |
| | Peaks | `peaks` | derive | a z6 tile | ⇄ |
| | Wikidata facts | `items` | fetch | the world | lead |
| | Heritage details | `heritage` | derive | the world | lead |
| | Landmarks | `marks` | pack | the world | lead |
| | Area overlays | `overlays` | pack | the world | lead |
| 3D buildings | Buildings' sources | `bld-fetch` | fetch | the world | lead |
| | Buildings read | `bldprep` | derive | a z6 tile | ⇄ |
| | 3D buildings | `bldtiles` | pack | a z6 tile | ⇄ |
| | 3D buildings, zoomed out | `bldfield-lo` | pack | a z3 tile | ⇄ |
| | 3D buildings' z8 tiles (task) | `bldtile` | | a task of 3D buildings | ⇄ |
| Publishing | Catalog | `catalog` | publish | the world | lead |
| | Catalog, held | `catalog-held` | publish | the world | lead |
| Upkeep | Input check | `inputs` | keep | an input | lead |
| | Names to do | `names-todo` | keep | the world | lead |
| | Timetables to do | `timetables-todo` | keep | the world | lead |
| | Clean-up | `gc` | keep | the world | lead |
| | Backup | `backup` | keep | the world | lead |

Publishing has no lane or step of its own: it goes per z6 tile (docs/building.md §2, Publishing), and the
forecast marks each region where it's all on the map (`map_at`).

## 4. The checklist

The agent's status carries the build to the end as a line per stage (`agent::build::Line`), each
with an item per step counted (`agent::build::Item`: its step, title and noun, and its targets
`done` of `total`, or `left` (a worldwide step's run: 1 or 0; a chain step's: the targets of its
works to come), or neither while it can't be sized yet, "to come"). A line names every step of its
stage (`steps`); its own `done` and `total` count the items of one noun (its `unit` field: the first sized
item's), never z6 and z3 tiles as one, and its `left` every item's targets left (none while an item
isn't sized); its counts in words are a noun at a time ("398 of 398 z6 tiles · 18 of 18 z3 tiles", "2
runs left": `Line::counts`, said the same by the page and the menu bar); then its jobs to come in
order (`next`), why it waits or which Mac is on it (`note`), and which of its steps any member may
take (`shared`). `agent::build::fold` makes the lines from the items; the planet's and its
worldwide steps' items are the agent's own (`Agent::checklist`), the regions' `build::checklist`'s.
A step with a job running, on any Mac, is never done (`Line::under_way`).
The landmarks' heritage details, points and overlays are sized as the planner decides them
(`landmarks_work`, `marks_ready`), not after the chain's candidates, peaks and items: a run left
while what they read isn't all built or their key isn't recorded.

The same lines are shown three ways, each the same shape: the build page's stage cards (§5), the
menu bar's menu (`tools/status`: "To the end", a line per stage with its counts, under it a line
per step with its count, then its jobs to come; its panel is the page itself), and `scenic status`'s
"To the end" (the same lines, a step's place marked ⇄).

## 5. The build page

`web/work/` (workers.md §7). One rule: an element is a card or a row of the same cells as its
kind's others, with templated words.

The shell: a header kept at the top (the title; the verdict; Pause, Freeze or Resume; the
alerts; the tabs) and a footer fixed at the bottom (the charts' range, the Live dot). A tab per
part, chosen by the address's `#hash` (`#overview`, `#jobs`, `#machines`, `#regions`,
`#activity`, `#details`, `#device`; none or another: Overview), so a link or a reload opens the
same tab; an alert links to the tab that says more.

- **Header's verdict:** one sentence (state, done ≈, next on the map), the alerts as chips, the
  pause controls: Pause (each job stops at its next safe point), Freeze (each job frozen where it
  is, after a confirm), Resume.
- **Footer:** the range the Machines tab's live charts show (1 min to 72 h, this browser's own,
  kept); the Live dot, green and blinking while the
  build's state answered in the last 30 s, else Stale with the time of the last answer.
- **Overview tab:** the gate's banners (held inputs), the overview, then the world map.
- **Overview:** how far: the whole build's work done, every stage's (`sums.js` wholeProgress:
  each step's targets done at the forecast's time for a target of it, `each_s`, measured or
  estimated, of that plus the forecast's work left; 100% only when every stage is done), how long
  (the same work left), the stages as a coloured strip; two numbers that aren't a step's
  (the map's last update with its catalogue's number and kind (with the low
  zooms or map tiles only), the next ≈ and the map tiles to draw; machines working).
- **World map** (`worldmap.js`, a canvas in Web Mercator, the world 0–1000 each way): the z3 grid
  (z6's too once its cells are 24 px), zoomed out at most to the world filling the box's width, the countries (`countries.json`), the regions' outlines
  (`/work/regions`) coloured by state (on the map or done, building, to come; counted in the
  legend), the tiles finished in the Jobs tab's past window (the history's ends, `sums.js`
  finishedTiles; each fades in as it comes, the window's history played back on the first look,
  paler as it ages; a low zoom's tile paler still), and each running job's target tiles (the
  history's start or lease event, else the tiles its id or words name: `sums.js` targetsOf) in
  its stage's colour, filled as far as they're done (`sums.js` runningTiles: from the job's
  `live`, its targets done taken as the first of its list and the one under way at its time so
  far against the step's `each_s`, under 95%; where `live` is missing, the job's progress or the
  lease's beat `frac` spread over its tiles). Hover cards (the Jobs tab's cards, pinned on a
  click or tap) for a tile (its step, state, phase, job and Mac, or when it finished, and a
  minimap of it and half a tile around) and a region (its folders, state, tiles, what's left,
  download size). Moved by a drag, zoomed by a pinch (ctrl+wheel in Chrome, gesture events in
  Safari, two fingers on a touch screen), a double click, its +/− and whole-map buttons or the
  keyboard; a two-finger scroll moves it only once zoomed in. Jobs with no tile (a worldwide step's) are named under it. Drawn only when something it shows changes; a theme change redraws it and the minimaps shown.
- **Jobs tab, the schedule:** lanes grouped by Mac (its short name over a lane per slot, the lead
  first, a Mac gone after), each segment in its stage's colour (a `--st-<stage>` token in
  index.html, a hue each in either theme): the jobs that ran (the history's job ends: a lead's
  `end` on its slot's lane, a worker's leased `done` or `fail`, which names no slot, on its Mac's
  first lane free by then; back as far as the Past jobs choice: none, 1 h, 6 h, 24 h, 3 days or
  7 days, this browser's own; read back from `/work/history` 5 000 events at a time, about 3.4 MB
  of JSON in four asks for a week's ~18 000 events, measured 23 Oct; runs of one step closer than
  a pixel packed into one segment: `sums.js` pastRuns, packRuns, laneGroups, pastFrom, tested by
  tools/check/worksums.test.mjs). A Mac's lanes are one group whatever names its slots had: an
  older agent's "(second job)" is slot 2, and a name that starts a Mac's current one up to a "-"
  is that Mac (`sums.js` macSlot: macOS renamed "Mac-mini" to "Mac-mini-de-Brandon"; the history
  names no Mac otherwise). Failed ones outlined red, those running now (to the now line)
  and the forecast's to come, faint; a now line; each region's mark where it's all on the map; a
  dense time axis (`chartmath.js` timeTicks, midnight a day's name); the pages' lane when a page
  helps.
- **Jobs tab, the stages:** a list: a stage's line (mark, title, counts, bar), then a row per step
  (mark, id, title, ⇄ when any Mac may take it, count) with a cell per job: finished (the history
  held), running, to come (the forecast's).
- **Cards:** everything in the Jobs tab has a card (what, where, when, how long, how it went; a
  step's forecast and jobs; a stage's note and next jobs), shown on hover with a mouse or trackpad
  and pinned on a click or tap, refreshed with each render; Escape, × or a click elsewhere closes
  it. No phase bars yet: they wait for the lead's measured phase shares (task #511).
- **Machines:** a card per Mac: name, role, state, heard from; its capacity; a block per running
  job by its slot (name, parts, progress, running since, memory; frozen or stuck) with what's
  inside it (the status's `jobs[].live`, docs/pool.md §10, Live: its phase and since when, its
  target under way against the step's `each_s`, its targets done, its NAS and internet bytes);
  then **Live** (`live.js`): the Mac's own `/work/live`, asked every 1.5 s while the tab is shown
  (the footer's range, then the points since), drawn by `chart.js` (its sums in `chartmath.js`,
  tested by web/test/chartmath.test.mjs): Transfers (the jobs' NAS reads and writes, the internet
  in and out, each said as its last minute's mean) and Cores (the build's and the rest's, stacked, against the Mac's cores and its
  P cores) with axes; compact rows for CPU and load, memory in use coloured by its pressure with
  the build's and the swap, the disk's free space and writes, the battery (read by the agent's
  loop, said while under 15 min old), the NAS's answer time;
  gaps where the Mac was silent shaded, and a footer with the range, the resolution and how long
  it was silent. What the lead offered a member is one line (its steps and the pieces it can
  take), the steps on a click. Pages one card.
- **The pool:** `pool.js`, under the machines (the Machines tab).
- **Settings:** `settings.js`, under the pool (the Machines tab): a row per setting (its name, its
  value, who set it and when, a field with Set and Off) for the in-use share, build memory and disk
  room target, a block per Mac named by its host (this page's own first; each other Mac's asked at
  its broker's address, as live.js asks `/work/live`), and the pool's NAS reserve once, through the
  first Mac that reaches the NAS (`/work/settings`, workers.md §7); read every 30 s while the
  Machines tab is shown, and after each Set or Off. Every control on the page (Pause, Freeze,
  Resume, the lead's, Accept, Cancel, Try again, the settings) works from any device.
- **The NAS:** `nas.js`, under the pool (the Machines tab), the answers `live.js` already asks
  (it keeps them and tells `nas.js` of each), drawn by `chart.js`. A card for the NAS itself, from
  the lead's status (`agent.nas`, docs/pool.md §10): the gate (open or shut), the disks' smoothed
  busy share against the owner's target (a bar with the target marked), the limits its probes give
  (MB/s, new files a second), the pool's traffic now; and, from the lead's `/work/live`, over time
  the pool's NAS reads and writes (`pool_nas_*`, MB/s), its files opened, made and listed a second,
  and the busiest disk's busy share against the target (`nas_disk_busy_pct`). A card a Mac: its
  share of the NAS budget (the status's `nas_share`, a bar, shown warn over the whole), and from its
  own `/work/live` its jobs' NAS reads and writes (`nas_read_mbs`, `nas_write_mbs`, each said as the
  last minute's mean), the answer time of the agent's listing (`nas_ms`, least to most), the calls
  in flight (`nas_calls`) and those that missed their deadline in the last minute (`nas_missed`). A
  measure missing or a Mac that doesn't answer is said so, never drawn as zero. The page's other
  NAS words (the capacity table's NAS column, a Mac's capacity bar, the alerts when one can't reach
  it) are as they were.
- **Regions:** a row per region: its folders and name, state, when it's done and all on the map,
  its download's size, its tiles built (on the map as they come), what's left by step; found by
  name, id or folder (`/work/regions`, docs/pool.md §10, Regions).
- **Activity:** the hours, and a sentence per event.
- **Details:** tables, unfolded, among them the steps table itself, each step with where its runs'
  time goes: its three largest phases by their mean share of a run's wall time, each with its share
  of the bytes read and written (local disk, NAS and internet: "I/O"), over its last 10 runs that ended well (the lead's status's `phase_shares`,
  docs/pool.md §10; measured, none typed by hand; blank before a step's first run).
- **This device:** helping with this tab (index.html; not in `?view`).

Each Mac's measures over time (NAS latency, load, cores busy, memory, the build against the other apps, disk space and writes, power, jobs, its NAS calls in flight and missed, the internet by host and its last transfers; on the lead the NAS's disks' busy share and the pool's NAS traffic) come from its own
`/work/live` (docs/pool.md §10, Live); a series missing is shown as no measure yet. The Machines
tab draws the ones above; the rest (the NAS's disks, the pool's traffic, the internet by host) aren't drawn yet.

Colours are the stage's (`STAGE_COLOURS` in `dash.js`), never a step's own.
