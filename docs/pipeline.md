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
| **z6 tile** | the build's working grid: most per-tile steps (terrain-hi, slope-hi, trees, `unit`, …) make a z6 tile's files at a time; `6/x/y`, named `6-x-y` in files | `Scope::Z6` |
| **z10 tile** | 16 × 16 per z6 tile: a deep pack's (the terrain's and the slope's past z12: terrain-deep, slope-deep); `10/x/y`, `10-x-y` | `Scope::Z10` |
| **z3 tile** | 8 × 8 z6 tiles: the zoomed-out levels (the `…-lo` steps) are assembled per z3 tile; `3/x/y`, `3-x-y` | `Scope::Z3` |
| **z8 tile** | 4 × 4 per z6 tile: the 3D buildings' tasks (`bldtile`), tree cover's blocks and terrain's subtrees are made per z8 tile; the terrain's deep levels are planned over theirs | `bld::task`, docs/workers.md §3 |
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
| `id` (`name`) | the stored name: in the records, the journal, the leases, the timings, the floors, the `work/` paths; and the name every reader shows (task #659: no display names, the build page's, the menu bar's and `scenic status`' alike); renamed only with its old name read as the new (§3) | `unit`, `terrain-lo`, … |
| `does` | one sentence: what it makes from what | |
| `kind` | where its output goes | `fetch` (`sources/`, from outside), `derive` (`work/`, `global/`: what other steps read), `pack` (what the map serves), `publish` (a catalog), `keep` (checks, lists, removals, backups) |
| `scope` | what one target is, and the noun its targets are counted in | `world` (one target: a run), z3 tile, z6 tile, `input` (a drop box); a task's `task`. Counted in "z3 tiles", "z6 tiles", … (`Scope::noun`); sent as `scope` `area` and `tile` by `/work/steps` (the stored words, kept: `Scope`'s serde names) |
| `stage` | its stage | `pass`, `sites`, `terrain`, `slope`, `trees`, `depth`, `roads`, `tiles`, `finish`, `trains`, `landmarks`, `buildings`, `publish`, `upkeep` |
| `place` | which Mac runs it: derived from the pool's rules (`agent::every::offered`), not kept | `any` (any member whose measured room fits), `lead` |
| resources | its first guess of memory until measured, whether members take it and at what rank (`shared`), a whole-build run alone (`alone`), whether it keeps the pass's answers (`answered`), its write-set (`writes`). The rest is measured, not kept: each target's memory, disk, cores, time and NAS rate by the step's model (`needs::Book`), targets per job (`coord::JOB_S`), and whether a job runs with others on a Mac (packing, by those measures) | pool.md §7.2, §7.7 |

Stages, steps, tasks, jobs, targets, parts and phases have no display names: each is said by the
name the code gives it (a stage's `id`, a step's `id`, a job's id: its step and first target, a
target's name, a part's or phase's words as the job emits them). Every text about a step is
templated from its row (its `id`, `steps::said`, `counted`):

| Text | Template | Example |
|---|---|---|
| a job's name (agent, menu bar) | `{id}: {n} {noun}s`; a worldwide step `{id}` | "terrain-hi: 8 z6 tiles", "outlines" |
| a job's name (the build page) | its id: `{id} {first target}`; a worldwide step `{id}` | "terrain-deep 10/259/338", "outlines" |
| a checklist count | `{done} of {total} {noun}s`, `{left} {noun}s left`, "done", "to come" (not sized yet) | "156 of 284 z6 tiles" |
| a step waiting | `{id}: {why}` | "lakefill: failed 3 times in a row: …" |
| an event | `{who} finished {job's name} in {time}` | "m4 finished unit 6/33/22 (unit: 6 z6 tiles) in 10 min" |

The build page, the menu bar and `scenic status` have no words of their own for a step: the page
reads the table from the coordinator (`POST /work/steps`: every row and the stages), `scenic
steps` prints it.

## 3. The table

Ids are the stored names, kept as they are: a rename would call every record of the step stale
(its records, journal entries, timings, floors and `work/` paths all carry it), for nothing people
see. A test pins them (`steps::tests::the_ids_are_the_stored_names`). The one rename, task #639's
(the terrain's and the slope's steps by the pack each makes: `terrain` → `terrain-hi`, `slope` →
`slope-hi`), keeps the old names readable: `steps::RENAMED`, `steps::current` (a row looked up by
either name, a hand-off's or journal entry's done targets recorded under the new one), and the
records' `terrain` and `slope` keys read as `terrain_hi` and `slope_hi` (`jobs.json`); the `work/`
paths (`terrain-mid`, `slope-mid`) and a mid's `step` field keep the old names. Place: any member
(⇄) or the lead.

| Stage | Step | kind | each target | place |
|---|---|---|---|---|
| `pass` | `osm-pass` | derive | the world | lead |
| | `pass-sets` | derive | the world | lead |
| | `outlines` | derive | the world | lead |
| | `trailends` | derive | the world | lead |
| | `reach` | derive | the world | lead |
| | `terrain-z8` | derive | the world | lead |
| | `buildings` | fetch | the world | lead |
| | `summits` | derive | the world | lead |
| | `labels` | pack | the world | lead |
| | `lakefill` | derive | the world | lead |
| | `water` | pack | the world | lead |
| | `spoken` | derive | the world | lead |
| | `regions-extract` | derive | the world | lead |
| `sites` | `heritage-sites` | derive | the world | lead |
| `terrain` | `terrain-water` | derive | the world | lead |
| | `terrain-hi` | pack | a z6 tile | ⇄ |
| | `terrain-deep` | pack | a z10 tile | ⇄ |
| | `terrain-lo` | pack | a z3 tile | ⇄ |
| | `terrainsub` (task) | | a task of `terrain-hi` | ⇄ |
| `slope` | `slope-deep` | pack | a z10 tile | ⇄ |
| | `slope-hi` | pack | a z6 tile | ⇄ |
| | `slope-lo` | pack | a z3 tile | ⇄ |
| `trees` | `trees` | pack | a z6 tile | ⇄ |
| | `trees-lo` | pack | a z3 tile | ⇄ |
| | `treeblock` (task) | | a task of `trees` | ⇄ |
| `depth` | `depth-lo` | pack | a z3 tile | ⇄ |
| | `depth` | pack | a z6 tile | ⇄ |
| | `depth-shade` | pack | a z3 tile | ⇄ |
| `roads` | `unit` | pack | a z6 tile | ⇄ |
| | `tail` (task) | | a task of `unit` | ⇄ |
| `tiles` | `pack` | pack | a z6 tile | ⇄ |
| | `lo` | pack | a z3 tile | ⇄ |
| `finish` | `prune` | keep | the world | lead |
| | `roadunits` | derive | the world | lead |
| | `stations` | pack | the world | lead |
| | `ferries` | pack | the world | lead |
| | `terrain-root` | pack | the world | lead |
| | `slope-root` | pack | the world | lead |
| `trains` | `rail-feeds` | fetch | the world | lead |
| | `rail` | derive | the world | lead |
| `landmarks` | `pois` | derive | a z6 tile | ⇄ |
| | `peaks` | derive | a z6 tile | ⇄ |
| | `items` | fetch | the world | lead |
| | `heritage` | derive | the world | lead |
| | `marks` | pack | the world | lead |
| | `overlays` | pack | the world | lead |
| `buildings` | `bld-fetch` | fetch | the world | lead |
| | `bldprep` | derive | a z6 tile | ⇄ |
| | `bldtiles` | pack | a z6 tile | ⇄ |
| | `bldfield-lo` | pack | a z3 tile | ⇄ |
| | `bldtile` (task) | | a task of `bldtiles` | ⇄ |
| `publish` | `catalog` | publish | the world | lead |
| | `catalog-held` | publish | the world | lead |
| `upkeep` | `inputs` | keep | an input | lead |
| | `names-todo` | keep | the world | lead |
| | `timetables-todo` | keep | the world | lead |
| | `gc` | keep | the world | lead |
| | `backup` | keep | the world | lead |

Publishing has no lane or step of its own: it goes per z6 tile (docs/building.md §2, Publishing), and the
forecast marks each region where it's all on the map (`map_at`).

## 4. The checklist

The agent's status carries the build to the end as a line per stage (`agent::build::Line`), each
with an item per step counted (`agent::build::Item`: its step and noun, and its targets
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

The shell: a header kept at the top, two rows (the title, the verdict, Pause, Freeze or Resume;
then the tabs) and a footer fixed at the bottom (the charts' range, the Live dot). A tab per
part, in the mockup's order, chosen by the address's `#hash` (`#overview`, `#jobs`, `#machines`,
`#nas`, `#regions`, `#dropboxes`, `#activity`, `#details`, `#device`; none or another: Overview), so a
link or a reload opens the same tab; an alert links to the tab that says more (a held change: Drop
boxes).

- **Header's verdict:** one sentence (paused, out of touch or idle; percent done; done ≈), the
  pause controls: Pause at a safe point (each job stops at its next safe point), Freeze now (each job frozen where it
  is, after a confirm), Resume.
- **Footer:** the range the Machines tab's live charts show (1, 5, 10 or 30 min, 1, 2, 6, 24 or 72 h, or 7 days, its time words then every two days; this browser's own,
  kept); the Live dot, green and blinking while the
  build's state answered in the last 30 s, else Stale with the time of the last answer.
- **Overview tab:** the alerts (full width, each linking to the tab that says more), the overview, then the world map.
- **Overview:** how far: the whole build's work done, every stage's (`sums.js` wholeProgress:
  each step's targets done at the forecast's time for a target of it, `each_s`, measured or
  estimated, of that plus the forecast's work left; 100% only when every stage is done), and the
  finish (done ≈), nothing else (the work left, its measured share and the live error are in
  Details' Forecast), the stages as a coloured strip (each segment with the Jobs tab's stage card on hover, pinned on click or tap); two numbers that aren't a step's
  (the last catalogue: its time, how long ago, its number and kind (with the low
  zooms or map tiles only); the next catalogue ≈ and the map tiles to draw; each Mac by name,
  a line each, with its jobs, idle or out of touch).
- **The map's tabs** (#629): a tab bar right above it, "Build progress" (the world map below) and
  "Catalogues" (`catalogs.js`), this browser's choice kept. Catalogues: the published z3 or z6
  tiles (a radio, z3 first) on a world map of its own, each in the colour of when a catalogue last
  published it (a ramp from deep blue, older, to light yellow, newer; its legend the ramp between
  the range's ends, and the counts) over the range a pair of inputs sets. Each takes a local date
  and time (`2026-10-23 14:05`, a date alone, "now") or a span (`48 hours`, `7 days`, `90 min`,
  `2 weeks`), the span in one of the two at a time, counted from the other: the left's always back
  (`-48 hours` whatever sign is typed), the right's always on (`+48 hours`); two times with the
  left the later swap as one is committed (Enter or leaving it); anything else is refused, said
  under them, the pair kept. Defaults: the left `-48 hours`, the right now at render (kept as
  "now", moving on with the clock, until another time is committed there). "Hide tiles outside the
  date range": unticked, those take the nearer end's colour; ticked, they aren't drawn. Under
  the map a timeline across the box's width: every catalogue a mark (a tall one written whole),
  labelled "number · time" (with the date on the first labelled of a day) where the label fits (none overlapping, the latest first, the dated ones kept first), the range
  in its middle 80% and the tenth beyond it each side dimmed, a time axis under it (times of day
  up to two days, then days; local time, said once). Hovering a mark (a tap holds it, again lets
  it go) names the catalogue (number, time, whole or delta, its tiles) and lights its tiles in
  magenta, apart from the ramp, whatever the hide box: the zoom shown filled, the other outlined.
  A tile's card: the catalogue that last published it, when, the degrees it covers, a minimap. The
  range's inputs, zoom and hide box are this browser's (`catalogues`), but "now". Its data:
  `/work/catalogs` (docs/pool.md §10), asked only while the tab is shown, the catalogues after
  the last the page has: every 30 s, every 2 s while the agent is reading. The sums
  (`parseWhen`, `commit`, `tileTimes`, `paint`, `labelled`, the axis) are tested by
  web/test/catalogs.test.mjs.
- **World map** (`worldmap.js`, a canvas in Web Mercator, the world 0–1000 each way): the z3 grid
  (z6's too once its cells are 24 px), opening on the whole world (about 82° N to 66° S) and zoomed out at most to that whole world fitting the box both ways (bands beside it in a wide box), a crosshair over it, the countries (`countries.json`), the regions' outlines
  (`/work/regions`) coloured by state (on the map or done, building, to come; counted in the
  legend), the finished tiles (`sums.js` finishedTiles, #674: every tile the build's keys record
  as built, `/work/built`, whenever it was, and those the history's ends held finished in the Jobs
  tab's past window, not merged into the keys yet; its time and Mac the last end held for it, a
  recorded tile with none "earlier"; a recorded tile is built at some version, its key maybe not
  the plan's now, so one to be built again stays drawn until its job runs; each fades in as it comes, the window's history played back
  on the first look, paler as it ages, or evenly with the fade filter off; a low zoom's tile paler
  still; published, finished by the last catalogue's time (one with no end held when the oldest
  event the page holds is by then, else completed), solid; completed but not in a catalogue yet half as
  strong under a dashed outline), and each running job's target tiles (the
  history's start or lease event, else the tiles its id or words name: `sums.js` targetsOf) in
  its stage's colour, filled as far as they're done (`sums.js` runningTiles: from the job's
  `live`, its targets done taken as the first of its list and the one under way at its time so
  far against the step's `each_s`, under 95%; where `live` is missing, the job's progress or the
  lease's beat `frac` spread over its tiles). Its cards (the Jobs tab's, drawn as
  docs/mockups/build-live.html's: a head with the stage's or state's swatch, a kicker and the
  title; stat tiles; bars; chips; a foot) show the whole stack under the point, one after
  another under a line: every tile there at every zoom (the deepest first), then every region.
  A tile is keyed by its step and tile ("<step> z/x/y"): steps' at one spot (slope and roads on
  6/57/16) are entries of their own, drawn across the whole square as diagonal stripes in each
  step's colour in turn, repeating, and each its own card in the stack; the minimaps the same.
  A tile's state (`sums.js` runningState, finishedState): unstarted, in progress, completed (its
  job's work on it done, or finished, not in a catalogue yet) or published (in the app).
  A tile's card: its stage and state, its share done and phase (stat tiles), a bar of it done in its
  stage's colour, step, job, Mac, the degrees it covers, a minimap of it and half a tile around;
  a finished one's when and whether it's in the app. A region's: its folders, its
  download and tiles built (stat tiles), a bar of its tiles built, a bar of the build's stages
  (built solid, the one under way striped, to come faint), state, times, build order, its work
  left by step (chips), its outline's recipe (the foot). Under a mouse the stack follows the
  pointer, every item in it outlined on the map (each tile's whole square, each region by its
  outline; the canvas's `data-outlined` counts them); a click or a tap pins it (the map takes its own clicks at their pointerup, the stack
  where it lifted, and never leaves them to the page: Mobile Safari pinned the tap before's,
  #615). Under it the legend (#627): the regions by state ("64 regions on the map", building, to
  come), the tiles by state (unstarted, in progress, completed not yet published, published),
  counted; the tile areas by the packs the steps make (#659, `sums.js` AREAS: `root` z0, `lo` z3,
  `hi` z6, `deep` z10; a tile's area is its zoom's, a `<layer>-root` step's worldwide target drawn
  as 0/0/0), counted, each with its outline's weight (the root's heaviest, a deep tile's a
  hairline: the weight says the area, the dashes stay the states'); and every step that draws
  tiles, by its id and counted, grouped by its stage (the stage's id first, a toggle of all its
  steps; the stage's colour, the one colour a step has across the page); each item a toggle
  showing or hiding its kind (off: struck and dimmed), hovered: the rest of the map faint;
  "Check all" and "Uncheck all" by the steps.
  Then the filters. Toggles and filters are this browser's own (localStorage): completed tiles
  faded by age; "Show only for" one pool member (the tiles it ran, the history's ends,
  and runs or will run in its running jobs and leases, and, task #622, the tiles the lead's last
  forecast places on it later: a "to come" layer, faint under a dotted outline in each stage's
  colour, a legend toggle of its own, kept by the steps and tile areas, its card saying "to come
  at ≈ HH:MM on <Mac>"; asked of `/work/tocome` only while a member is chosen, pool.md §10).
  Both maps and
  their stacks share them (`sums.js` mapTilesKept). On a touch screen, zoomed out, a finger
  moves the map sideways only: up and down is the page's scroll. Moved by a drag, zoomed by a pinch (ctrl+wheel in Chrome, gesture events in
  Safari, two fingers on a touch screen), a double click, its +/− and whole-map buttons or the
  keyboard; a two-finger scroll moves it only once zoomed in. Jobs with no tile (a worldwide step's) are named under it. Drawn only when something it shows changes; a theme change redraws it and the minimaps shown.
- **Jobs tab, the schedule:** lanes grouped by Mac (its short name over a lane per slot, the lead
  first, a Mac gone after; beside the name the Machines tab's chips for what keeps it from work:
  paused or pausing (paused alone, `scenic pause --here`: the forecast gives it no lanes, the
  lead's own too while it still leads; its jobs from before the pause still show, and with no lanes in view its row stays, chips
  only), out of touch, on battery), each segment in its stage's colour (a `--st-<stage>` token in
  index.html, a hue each in either theme): the jobs that ran (the history's job ends: a lead's
  `end` on its slot's lane, a worker's leased `done` or `fail`, which names no slot, on its Mac's
  first lane free by then; back as far as the Show past tasks choice: none, the past 1, 2, 6, 24,
  48 or 72 hours or 7 days, this browser's own; read back from `/work/history` 5 000 events at a time, about 3.4 MB
  of JSON in four asks for a week's ~18 000 events, measured 23 Oct; runs of one step closer than
  a pixel packed into one segment: `sums.js` pastRuns, packRuns, laneGroups, pastFrom, tested by
  tools/check/worksums.test.mjs). A Mac's lanes are one group whatever names its slots had: an
  older agent's "(second job)" is slot 2, and a name that starts a Mac's current one up to a "-"
  is that Mac (`sums.js` macSlot: macOS renamed "Mac-mini" to "Mac-mini-de-Brandon"; the history
  names no Mac otherwise). Failed ones outlined red, those running now (to the now line)
  and the forecast's to come, faint; a red now line capped by a dot labelled "now"; each region's mark where it's all on the map; a
  dense time axis (`chartmath.js` timeTicks, midnight a day's name); the pages' lane when a page
  helps.
- **Jobs tab, the stages:** a list: a stage's line (mark, id, counts, bar), then a row per step
  (mark, id, ⇄ when any Mac may take it, count), under it a row of its jobs, every one (none
  folded into a count): a box a job, a square a target (filled once done), finished (the history
  held: the targets its end names), running (its live targets done), to come (the forecast's lane `n`).
- **Cards:** everything in the Jobs tab has a card, drawn as the final mockup's (#659: a head
  with the stage's swatch, a kicker ("terrain-hi · step", "terrain-deep · job under way", "stage")
  and the code's name as its title; a world map of a job's tiles, 190 px until pinned, then the
  card's width up to 1000 px; stat tiles; a bar; key–value rows; chips; a foot with its names;
  560 px wide, twice the map's cards): a job's (where, when, how long, how it went, its part,
  what's inside it; its targets as chips), a step's (its targets done, what it makes, kind, where
  it runs, memory, its forecast and jobs; its time by phase as chips), a stage's (steps done, its
  counts, what it makes, work left, done ≈, now or next, its note; its steps as chips), shown on hover with a mouse or trackpad
  and pinned on a click or tap, refreshed with each render. A following card never catches the
  pointer (pointer-events none; placed beside the point, never over it) and goes as the pointer
  leaves what it's about (into anything else, off the map, out of the window). A pinned card is
  modal (#627): a clear veil under it, not dimmed, takes every click and scroll behind it (the
  page doesn't scroll), a click on it only closing the card; the card itself scrolls and stays
  live. Escape or × close it too. A running job's card says what's inside it (the status's `jobs[].live`, docs/pool.md §10,
  Live: its phase and since when, its target under way against the step's `each_s`, its targets done, its NAS and internet
  bytes). No phase bars yet: they wait for the lead's measured phase shares (task #511).
- **Machines:** a card per Mac, one under the other at every width (the pool's cards too): name, role, app, state, heard from; a block per running
  job by its slot, about that job alone (name, progress in words, running since, memory; frozen or stuck; Cancel);
  then **Live** (`live.js`): the Mac's own `/work/live`, asked every 1.5 s while the tab is shown
  (the footer's range, then the points since), drawn by `chart.js` (its sums in `chartmath.js`,
  tested by web/test/chartmath.test.mjs), as docs/mockups/build-live.html's vitals: each core's
  share busy now (bars, the efficiency and performance groups named: `cores_now`); four transfer
  charts (the jobs' NAS reads and writes, the internet in and out, each its last minute's mean
  said; each with a second line, thin and dashed, on its own scale in a 40 px gutter at the right,
  its 0 and top and its unit once between them, in its colour: `nas_files_s` and `nas_creates_s` as
  files/s, `net_req_s` on both internet charts as req/s; task #620); then a chart a measure. As in the mockup, each row is its name over its figures in a
  150 px column, its chart beside (the scale's 0, half and top in a 58 px gutter, a 40 px one at the right in every chart, the time under it
  as time ago, limits dashed in the warning's colour, the newest point a dot): CPU use (the build's and the rest's,
  stacked, against the Mac's cores and the cores the build may use now; P and E said), the load
  (against the cores; what waits for a core said), memory (the build's and the rest's, stacked, in
  GB, against the build's memory), its pressure (100 % less memory free, the line coloured by
  macOS's level, 50 % and 80 % dashed; the memory compressed said; what the memory guard last did,
  its status's `memory.last`, marked and said while in the range), the swap (GB), the disk's free space (against the room
  target; the disk's size and the jobs' reservations said), its writes, the battery (shaded while on mains, against
  the battery floor, the owner's pool-wide setting), the NAS's answer time with the calls in flight and missed, the
  heat (macOS's thermal state, nominal to critical), and
  whether the owner is using the Mac (shaded while so; how long since a touch). What the agent's
  loop reads (power, idleness, the limits) is said while under 15 min old. Each chart (chart.js):
  at most five times under it, its scale's numbers with the unit once beside them (words for the
  heat and the owner's use), each limit a boxed tag on its dashed line. A settable limit (the
  build's cores, the build's memory, the room target, the battery floor) is set on its chart, as
  the mockup draws: its line or tag dragged up or down (a mouse at once; a touch after a long press
  on the tag) previews the value snapped to whole cores, 1 GB, 5 GB or 5 %, within what
  agent/settings.rs takes (cores 1 to the Mac's, memory 1 GB to the Mac's, room 1 GB to under its
  disk, the floor 0 to 100 %), and sets it on release through POST /work/settings, as the Settings
  rows do (Escape cancels); its ✎ opens a small editor on the chart (a number, Set, Off, Cancel).
  Each Mac's lines set that Mac's; the battery floor's tag says it's pool-wide, set through the Mac
  the Settings' pool box uses. Set, a line holds the new value till the agent's loop reports it (15
  min at most). The Settings rows stay, for the keyboard. Gaps where the Mac was silent shaded faintly, and a footer with the range, the resolution and how long
  it was silent. What the lead offered a member is one line (its steps and the pieces it can
  take), the steps on a click. Pages one card.
- **The pool:** `pool.js`, under the machines (the Machines tab).
- **Settings:** `settings.js`, under the pool (the Machines tab): a row per setting (its name, its
  value, who set it and when, a field with Set and Off) for the in-use share, build memory and disk
  room target, a block per Mac named by its host (this page's own first; each other Mac's asked at
  its broker's address, as live.js asks `/work/live`), and the pool's NAS reserve and battery floor once,
  through the first Mac that reaches the NAS (`/work/settings`, workers.md §7); read every 30 s while the
  Machines tab is shown, and after each Set or Off. Every control on the page (Pause, Freeze,
  Resume, the lead's, Accept, Cancel, Try again, the settings) works from any device.
- **NAS tab:** `nas.js`, every measure a chart over time (the owner's 04:33 ask, task #634), as
  the owner's final mockup's NAS tab lists them, without a heading of its own (the tab names it):
  the Machines tab's rows (`live.js`'s `row` and `words`: its name, its figure at the base size
  with dim small lines under it, the chart beside; the same widths, `chart.js`, and the footer's
  range), one column, under a line saying the range, its resolution and when the NAS's capacity
  was last measured (the best of how many probes over the last day). From the lead's status
  (`nas`) and its `/work/live` answer, which `live.js` asks while the Machines or NAS tab is shown:
  **response time** (the lead's listing, `nas_ms`; the server's name, protocol and free space,
  `nas.mount`), **requests waiting** (`pool_nas_calls`, every Mac's calls in flight, against
  `pool_nas_threads` dashed), **timed out** (`pool_nas_missed`, a minute; the range's count),
  **NAS disks busy** (`nas_disk_busy_pct` against the owner's target dashed; "the gate is shut"
  when it holds), then the pool's use against the limits its probes give (`nas.limits`, dashed,
  with its share of each): **data moved** (`pool_nas_read_mbs` and `pool_nas_write_mbs` stacked),
  **files created**, **entries listed**, **files looked up** (`pool_nas_creates_s`,
  `pool_nas_listed_s`, `pool_nas_stats_s`), and **files opened** (`pool_nas_files_s`). The pool's
  series are the lead's alone, a point every 10 s. Each Mac's own NAS response, calls in flight
  and calls missed in the last minute are its Live block's NAS row. A measure missing, or a lead
  that doesn't answer, is said so, never drawn as zero.
- **Regions:** the world map (worldmap.js) over the list, in its own box headed Regions, split by
  a bar to drag (a handle on it; kept in the browser; stacked, the map first, below 720 px). The list is virtual (only the rows in view are
  made) and ordered as the regions reach the map, A–Z or by most work left; found by name, id or
  any folder. A row per region: its folders and name, state, when it's done and all on the map,
  its work in time and download size, and a segmented bar, a segment named in it per stage that
  builds regions, each filled by the share of that stage's targets built in the region (the lead's
  counts, `/work/regions` with `steps`, docs/pool.md §10, Regions; "N of M built (P%)" on hover;
  task #636), the one under way striped past its fill; a stage the lead doesn't count by region
  by its state in the forecast alone (built solid, to come faint; its tiles built and what's left
  by step on the bar's hover). Above, what's left in all regions by step, summed in the page
  (sums.js `stepTotals`). A row under the pointer lights its region on the map; its card (the
  Jobs tab's, also in the map's stacks, as above) says its folders, its download and tiles
  built, its stages, state, times, build order, work left and outline recipe (`/work/regions`, docs/pool.md §10, Regions).
- **Activity:** the hours, and a sentence per event. Below them, as the owner's mockup lays them
  out, from every Mac's `/work/live` (`activity.js`, asked every 2 s while the tab shows; docs/pool.md
  §10, Live): the **file stream**, what each Mac is moving now, newest first (the NAS files its
  jobs opened to read or made, with their size and the open's or copy's time, and the internet
  transfers that ended, downloads and breaks), each Mac a chip in its colour on the page, and
  **internet by website**, a row per host: requests and MB a second over the last 30 s, the
  connections open now (those opened since each Mac's agent started on hover), retries and 429
  holds since then, the budget it is metered by, and its robots.txt: allowed or disallowed (with
  when it was read), not checked yet (a page host whose robots.txt is still to be read), or n/a (an
  API or a data download: no robots.txt applies), as the jobs told their Macs' proxies (the newest
  told). Not shown: a NAS read's MB/s (a read file's bytes aren't counted per file, so its line says its
  size and the open's time), and the tokens left in a budget's window (pool-wide, on the NAS; a
  Mac holds only its batch). Below 640 px a line's time is left out (it shows on hover).
- **Details:** tables, unfolded, among them the steps table itself, each step with where its runs'
  time goes: its three largest phases by their mean share of a run's wall time, each with its share
  of the bytes read and written (local disk, NAS and internet: "I/O"), over its last 10 runs that ended well (the lead's status's `phase_shares`,
  docs/pool.md §10; measured, none typed by hand; blank before a step's first run).
- **Drop boxes tab:** the gate (docs/inputs.md §4.7), from the status's `inputs`: a banner per held
  unit (its held files, each finding, Accept per warning, Accept all; errors say what to fix, the
  only actions here: the inputs are otherwise read-only through the page), a quiet line per unit
  being checked, else a line naming the drop boxes, all taken in. The Overview's alerts still say a
  change is held.
- **This device:** helping with this tab (index.html; not in `?view`).

Each Mac's measures over time (NAS latency, load, cores busy, memory, the build against the other apps, disk space and writes, power, jobs, its NAS calls in flight and missed, the internet by host and its last transfers; on the lead the NAS's disks' busy share and the pool's NAS traffic) come from its own
`/work/live` (docs/pool.md §10, Live); a series missing is shown as no measure yet. The Machines,
NAS and Activity tabs draw them, each asking only while it is shown.

Colours are the stage's (`STAGE_COLOURS` in `dash.js`), never a step's own.
