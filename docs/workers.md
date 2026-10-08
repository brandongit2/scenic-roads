# Builds anywhere: one coordinator, its data plane, any number of workers

Status: **partly built** (design of 2026-10-04; what's built is marked so, the rest is planned).
This extends plan §8 (Building): the build Mac's agent plans, alone writes the build's records, and
runs a coordinator; every other worker (the M1's agent, any device that opens a page) asks it for
work that fits, does it, and hands it back.

## 1. Goals, and where the time goes

- **Any device, by opening a page.** No install. Up to about five workers.
- **One code path.** No device special cases: a worker says how much memory it can spare, and is
  given only work predicted to fit; a phone simply ends up with smaller work.
- **Kind to the NAS.** The NAS (Btrfs RAID5 on spinning disks, 3.8 GB of RAM, SMB) is the slowest
  part: listings took 3–33 s per folder under load on 2026-10-04; small-file writes run at ~25 a
  second. Every byte should leave it once per need, in large reads, and arrive in large writes.
- **Resilient by construction.** Workers vanish (a tab closed, a phone asleep, Wi-Fi gone), the
  coordinator restarts, the NAS goes away, a browser miscompiles: none of it may corrupt the build.
- **Same bytes everywhere** (plan §8, Determinism): a task gives identical output on every worker,
  at any thread count. Results can then be re-run anywhere and checked by running them twice.

**Measured:** in the build Mac's dense units of 2026-10-04, the Rust steps were 18% of a unit's wall
time and elevation sampling (`sample.py`) 39%; the rest was staging from packs, the DEM cache slice
and copies to and from the NAS. A dense unit's tail after its canopy step (view, buildings, flags)
is 15–20% of its time. So the data plane came first, and paid off on the Macs before any browser.

## 2. The model

**One planner, many workers; pure tasks, impure work at the hub.**

- **The coordinator** (`pipeline::coord`, built) runs in the build Mac's agent: the planner (as
  before), the one writer of the records (as before), the data plane (§4) and the broker of work
  (§5), on port 8090.
- **Two kinds of work,** by what a worker can reach:
  - **Jobs** of the plan (the shared steps': terrain, slope, tree cover's pieces, units, candidates
    and peaks, the 3D buildings' bldprep and bldtiles), for workers that mount the NAS (the M1's
    agent). A job saves into
    the store's content-named files as before; its record changes come back as one hand-off.
  - **Tasks** (`coord::task`), pure work a running job offers to any worker: programs run over a
    folder of files, giving files back. What a task reads of its unit's folder was staged on the
    build Mac, which serves it; what it reads of the NAS's data and the DEM servers' files it reads
    where it lies, through the coordinator (§3, Read where they lie), never writing there.
- **Impure work stays at the hub:** downloading into the NAS's stores (FABDEM tiles, Meta's canopy
  squares, AWS's terrain), staging from packs, the scenic carry-over between runs, keeping a
  unit's DEM samples, and every write to the records. (Sampling the DEMs is a task's: the unit's
  slice of the per-vertex cache goes with it and its samples come back.)
- **Workers** hold nothing the build depends on: a lost worker costs only its work in hand.

## 3. Tasks (built for a unit's tail, the 3D buildings' z8 areas and tree cover's rows of blocks)

- **Where the work splits:** a unit's tail (`unit::tail`) is a task from its elevations on:
  elevations, clean-up and grade, prep, canopy, view, buildings and flags. Each step says which of
  the unit's files it reads (`Run::reads`, earlier steps' outputs too: what's there when the task
  starts is sent), traced in WebAssembly by `tools/check/tail.mjs`, which runs the task as a
  browser does and compares every file it writes with the native run's (6/20/22, with its last
  run's results restored: 57 of the folder's 64 files, 996 MB; all 22 written files the same, and
  with only the listed files given too). Staging, heritage inputs, area flags (a fifth of a second)
  and land cover (only where the packs lack it, and before the last run's results are restored,
  which hash it) stay in the job.
- **Canopy squares:** the canopy step reads Meta's 10° squares where they lie, in the NAS's store
  (`sources/canopy/`), and a worker can't download one. While the store lacks one of a unit's
  squares (`unit::canopy_stored`, by its grid's tiles' corners, as the step picks them), the steps
  through the canopy run in the job (`unit::keep_here`), which downloads it there; the rest is the
  task.
- **Read where they lie** (`/net`): a task's places name what it reads outside its folder:
  `{sources}` the NAS's `sources/` (FABDEM's store, the canopy squares: `{chm}`), `{moi}` its MOI
  DTM, `{net}` the DEM servers' files, laid out as `crate::fetch`'s mirror folders are
  (`<host>/<path>`). A browser reads them through the coordinator (`/work/net/<lease>/nas/<path>`
  under `sources/` and `inputs/moi-dtm/` only, `…/web/<host>/<path>` for the five DEM and land
  cover servers only, over HTTPS: `?probe` says what's there, `?list` a folder's entries, a range
  its bytes; nothing written), a 1 MB block at a time as a program reads them (web/work/runtime.js:
  synchronous requests, a WASI call can't wait; 64 MB of blocks kept); a server's "none" is its
  `.none` file. The coordinator fetches the servers' files over HTTPS only, following no redirect
  (none of the five redirects), keeps a couple of dozen open, and serves 8 MB at most a request; a
  NAS file or folder it can't read now, or read whole, is a 503, never "not there" or fewer files (a
  task fails rather than take FABDEM for Taiwan's missing MOI DTM, as an unlistable folder would
  otherwise have it). A native worker reads
  the NAS at its own mount and the servers as the build Mac does, and, like a browser, only reads
  the NAS's stores (`Tools::stores_read_only`, `SCENIC_STORES_READ_ONLY`): a FABDEM tile the store
  hasn't whole is read in place inside Bristol's zip (`dem::fabdem::stored`, the same values the
  store's copy has), a canopy square is read as it is (one not whole fails the task; nothing there
  is downloaded, touched or removed: the build Mac keeps the store).
- **What comes back:** the files the steps changed; one written back as it was sent isn't sent (a
  hash of each input; the unit's folder has it). Checks compare them whole, what says how long a
  run took aside (`dem-stats.json`'s `seconds`).
- **Memory:** a tail's task is predicted to take its files three times over, 300 MB at least (a
  worker holds them, a program reads them in, and room as much again for what its steps write
  meanwhile and a step's own: 6/20/22's view took 1 GB), until a run of it says: then what that run
  took, a tenth more.
- **The unit job** (`pipeline::offload`) clones those files into the task's folder (copy-on-write,
  instant; the roadside buildings' links to the NAS stay links) and offers the task when a worker
  that takes tails is around and fewer than such are out (at most three: each holds a unit's folder
  on the build Mac's disk), then prepares the next unit. Tails back from workers are committed
  between units.
- **Nothing waits on a worker that's slower than the build Mac:** the coordinator measures each
  worker's pace at each kind of task, its time from lease to done over the job's own run of the same
  task (the job's run when it made one, else what it takes it to be), weighed in half and half as
  they come (`Tasks::note_pace`; kept while the coordinator runs). When the job needs a task (at the
  end of the job; with one unit, at once after its offer):
  - **No one took it:** it waits up to 30 s (`offload::LEASE_WAIT`) only while a worker that could
    take it asks for work (around, not found wrong, doing its kind, sparing its memory, not one it
    failed on, asked in the last 30 s, as a page with a slot idle does every 15–20 s: the task's
    `takers` in `/task/<id>`, each with its pace) and that worker's pace beats the build Mac's with
    a quarter's margin (`task::beats`), or isn't measured yet and wasn't waited for in the last hour
    (`EXPLORE_EVERY`, `/task/<id>/explore`). Then it's taken back and run on the build Mac.
  - **A worker holds it:** waited on only while that worker's pace says it'll be back, with the
    margin, before the build Mac's own run of it, begun when the waiting began, would end
    (`offload::Patience`; that run is the tail's stages as this Mac has timed them,
    `unit-stages.json`, else 120 s). A worker not measured is raced at once. Past that it's raced
    there (the build Mac's result counts; a worker's that comes in too is compared: each file it
    sent with this Mac's, and each this Mac's run changed that it didn't send with the copy it was
    sent).
  - **Measured by finishing:** a task the job ran itself while a worker not measured holds it is
    kept for that worker to finish (`Tasks::measure`: its files moved into the coordinator's folder,
    no longer the job's, so the job may end), its result not used, its time its pace.
  - So a slow worker costs a unit nothing, and an unmeasured one 30 s once an hour; a fast one
    frees the build Mac's cores for the job beside. While the build pauses, nothing is waited on.
    With no worker around, a unit builds as before.
- **Source versions:** a task names the programs' build (the job's binary); a web worker fetches
  that build's WebAssembly programs from the coordinator (`/work/prog/<name>.wasm`, shipped in the
  app's `wasm/`). The native and WebAssembly builds of one source give the same bytes (§10).
- **The 3D buildings' areas** (kind `bldtile`, docs/buildings3d.md §3.6, `pipeline::bld::task`): a
  `bldtiles` job offers some of a z6 tile's z8 areas the same way, while workers that take them are
  around (one per worker, three at most out at once, from the far end of the tile's list; topped up
  before each area it makes itself). A task's files are cut from the work files on the build Mac
  (only the blocks the area reads, as stored, and the coverage's shapes that can answer there:
  docs/formats.md); its one run is the program `bldtile` over `{dir}` and the area; it writes the
  area's tiles (`area.tiles`) and summary (`area.json`). Its spec has a tail's shape (`unit` the area
  `8/x/y`, `version`, `runs`, `inputs`, `places`), so the page and `scenic run-task` run it
  unchanged. Its memory: its files three times over, 256 MB and 280 B a building of the area, until
  a worker measures it (the coordinator's `bldtile 8/x/y`). Settled in the area's turn as a tail is,
  with the same patience (this Mac's own time for it: its buildings at the pace of the areas the
  job made here; a worker's pace at `bldtile` its own): taken back and made here if no one took it, raced if someone holds it, a
  worker's result taken, or checked against the job's own run byte for byte.
- **Tree cover's rows of blocks** (kind `treeblock`, docs/plan.md §6 Trees, `pipeline::trees::task`):
  a tree cover piece's run (`trees --z6`, the program the `trees` job runs; it inherits the job's
  coordinator) offers some of its rows the same way: a row is the piece's z8 blocks in one z8 row
  (up to four), which read the same canopy rows (a canopy square's strips are rows of its whole 10°
  width, ~18 kB each). Offered when the run begins, from its last rows, one per worker that takes
  them around, three at most, never every row; settled when the run comes to a row's first block
  (the blocks go in column order, so the run's other blocks are made meanwhile), with the same
  patience (this Mac's own time for the row: its blocks at the pace of the blocks made here so far,
  as many at once as there are threads; a worker's pace at `treeblock` its own), a worker's result
  taken or checked byte for byte against the run's own of the row, and taken into the piece as if
  made here. Its one run is `trees --blocks <8/x/y,…> --coverage {dir}/coverage.json --chm {chm}
  --leaf {sources}/trees/leaf --squares <top,left;…> --out {dir}`: the row's blocks made together a
  band at a time, so each canopy strip is read and decoded once for the row (each block fed the
  same bands in the same order as alone: the same bytes, `trees::blocks`); `--squares` names the
  canopy squares the job found there (one a worker doesn't find fails the task, never a block
  without its trees). Its only file is the piece's coverage cut to the row (docs/formats.md;
  41 kB–1.2 MB on the rows checked, against 6–15 MB for a z3 tile's); the squares are read where
  they lie, never whole (187–289 MB through `/net` for a row of three or four blocks: the strips of
  its latitudes in each canopy square it meets, and the leaf-type tiles over it). Its memory: 120 MB
  and 120 a block until a worker measures it (`treeblock 8/x/y`, the row's first block): measured
  as WebAssembly 141–160 MB for a block, 233 for three, 365–505 for four (the most where they meet
  four squares), and 3–14 MB a block of archives written. `tools/check/treeblock-same.mjs` checks a
  row (2026-10-08, on the M4, its native and WebAssembly builds from one compiler: a coast's two
  rows of one block, a dense forest's of four across a canopy square's edge in longitude, the
  tropics' of three, and four across the edges of four squares): natively on one thread and on all,
  each block alone, as WebAssembly under Node's WASI and under the page's own runtime, every block
  the same bytes, and the same tiles and values as the NAS's packs and mid. A helper takes them too
  (1 GB of disk: it reads the squares where they lie). They pay off only for a worker measured
  faster than the build Mac at them: a page on the M1 took 20 s for a row the M1's own run made in
  about as long (its pace 1.07), and the build Mac makes a row on its threads in about 3 s.
- **Determinism rules:** one maths library (`det`, over `libm`) on every target; reductions that
  don't depend on the thread count; no hash-map order in outputs; the real zstd everywhere.
- **Planned:** staging from packs as a task's (read where the packs lie); the heavy steps cut into
  sample ranges so a slow worker's lease is minutes; more kinds of task (map tiles, landmarks,
  slope, terrain);
  the 3D buildings' areas cut smaller (z9, z10) for workers that spare less than a dense area needs.

## 4. Data: the coordinator's plane

- **Built:** the packs staging reads are copied from the NAS once to the build Mac's SSD (`blobs/`,
  content-named, so a copy is always right) and shared by its units; the next unit's piece, packs
  and canopy squares are copied ahead while the current one builds; a task's files are served to
  its worker from the build Mac's SSD, so another worker costs the NAS nothing; one disk budget for
  the caches (plan §8, Room on the disk).
- **Planned:**
  - **Ranges:** the exact byte ranges a task reads (DEM tiles, canopy strips, pack entries), so only
    those are fetched and sent, as a job reads a few raw terrain tiles of an area from the NAS's
    archives (plan §3, Downloads).
  - **No listings:** a content-named index per dataset (canopy squares, FABDEM, kept samples), read
    from one file instead of found by listing or probing, as the raw terrain tiles' archives are
    (plan §3, Downloads).
  - **Writes, journaled then committed together:** outputs on the SSD, copied to the NAS whole, and
    applied to the records in groups; `pending.json` on the SSD; small files packed.
  - **Paced by latency:** background reads and writes paced by the latency the NAS shows.

## 5. Control: leases and trust (built)

- **Asking:** a worker sends its name, kind, the work it does (the shared steps' jobs; the tasks'
  kinds, `tail`, `bldtile` and `treeblock`: a page asks with those three, and may ask with no
  others, `coord::PAGE_TASKS`), the
  memory it spares, its cores and (an agent) its app (`/work/ask`). One that mounts the NAS is given
  a job first (the most work for what it fetches), then a task; a web page, tasks. An agent on an
  older app than the build Mac's gets nothing (409, why in words) until it runs that one or a newer.
- **Pausing** (`docs/plan.md` §8, Pausing): the coordinator holds the build's pause (`pause.json`)
  and says it in its answers: an agent's ask is refused (409) with the pause, a page's gets nothing,
  and a beat carries it, so a running job stops at its next safe point (or freezes, as the pause
  says); no lease lapses while it holds. A worker's agent passes its Mac's ask on (`/work/pause`). A
  job paused at a safe point hands off the targets it finished (part of its lease's); one given back
  unfinished for an interruption (the pause, sleep, a restart) isn't held against its targets.
- **Leases** (`coord::lease`) are timed on the coordinator's monotonic clock (clocks between devices
  don't matter), ten minutes, renewed by a beat each minute while the work goes on. A job paused
  for its conditions doesn't beat, so its work may go to another; while the build is paused, every
  lease is held, beats or not, and each has a whole ten minutes again when it goes on. The build
  Mac's own jobs hold leases too, and one that lapsed is taken again when no one took its work
  meanwhile. A lapsed lease's work is offered again.
- **Handing back:** a job's saves (merged in order), done record and what its units cost go back as
  one hand-off, kept in an outbox folder per lease on the worker until taken (across restarts),
  journaled whole on the build Mac and merged with the records all at once. A hand-off for a lease
  that's gone is refused (410) and dropped: its work was offered again, and a late save could put
  an older build in the manifest. One that names another unit's files is refused.
- **Restarts:** the token, the jobs' leases and the units' costs are kept on the build Mac's disk,
  so its agent restarting (a new app) is a pause to workers, nothing more. A worker reads the
  contact (`state/coordinator.json`) again whenever it can't reach the coordinator or its token is
  refused.
- **Failures:** finished units aren't offered again before the plan shows them; a worker's failed
  unit isn't offered to it for an hour, doubling. A task out of memory is offered only to workers
  that spare more; one failed otherwise, or held by a worker that went quiet, isn't offered to that
  worker again, and after two the job runs it.
- **Verification, ramped:** a worker's first three results, then one in eight, are checked against
  the build Mac's own run of the same steps; the steps are deterministic, so any difference is the
  worker's fault, and it gets no more work.
- **Regression accepted:** the M1 builds only while the build Mac's agent runs (the claim files let
  it go on alone): one protocol instead of two. The build Mac still claims its own jobs' targets,
  and merges the NAS's hand-off files, for a helper on an older app.
- **Planned:** a NAS outage extending leases rather than letting them lapse; a task a slow worker
  holds given to an idle one too when the queue runs dry.

## 6. Fitting the work to the worker (built)

- **Predicted peaks:** a unit's is what it took last time (each unit job notes its units' peaks,
  `SCENIC_COSTS`), else about ten times its piece, never under 3.7 GB. A tail's is its files three
  times over and 300 MB (a web worker holds them, a program reads them in, and what its steps
  write), or what a worker measured for that unit's last time, a tenth more, in its place.
- **Ceilings, enforced:** the programs are linked to import their memory, and the page gives each a
  `WebAssembly.Memory` capped at its task's budget less the files it holds: an overrun is a failed
  task, reported with the peak it reached, not a tab the OS kills.
- **The page's ceiling, learned:** it starts from what the browser says it has, else 3 GB on an iPad
  (the owner's choice, 2026-10-05, for their 8 GB iPad Pro: Safari doesn't say, and gives a tab 4 GB
  or more there; a page that had learned less from the old 1 GB start takes it up once, unless a
  death lowered it), else 1 GB,
  never more than the largest memory the browser will create; it can be set on the page (This
  device, Memory to spare: kept in that browser); after three tasks near it succeed it rises by a
  quarter; a task the tab died in on screen lowers it below that task's (each tab notes what it
  runs and whether it's on screen, under a key of its own, and the page finds the note when it's
  reloaded, or within a minute after: iOS reloads a tab it killed at once, its note still fresh)
  and is given back. On an iPhone, iPad or Android, a tab killed while it was in the background
  (they empty a hidden tab's memory when they want it, whatever the tab holds) gives its tasks back
  and lowers nothing; elsewhere a browser does that only to a tab holding too much, so there it
  lowers the ceiling as on screen. Each task's own memory is capped at its budget besides.
- **Leaving the page, and coming back:** behind another app or with the screen off, iOS freezes
  the page and its tasks where they are; back within the lease's ten minutes, they go on where they
  were (the next heartbeat renews the lease), else the lease has lapsed, the task's gone to another
  worker or the build Mac, and the page drops it ("taken back") and asks for more. Reloaded or
  closed, the page gives its tasks back as it goes (`pagehide`: offered again at once, to it too,
  not held against it) and marks its note gone, so the next page gives them back again should those
  not have got through; brought back from the browser's back-forward cache, its slots start
  afresh. The worker page shows a task given back as such, not as a failure. The build Mac never
  waits on a page: a tail it needs is run there if a worker still holds it.
- **Planned:** cutting a task to the worker (smaller sample ranges for smaller ceilings).

## 7. The web worker (built)

- **The page** is served by the coordinator at `/work/` (the menu bar's panel, a click on its icon;
  its menu's Copy the Build Page's Address; `scenic status`'s "Build page"): the build at a glance for anyone it answers (this Mac,
  its LAN, the tailnet), with no key; the coordinator answers its reads (`/work/swarm`,
  `/work/history`) without one. A device helps only when its owner asks there ("Help with this
  tab", kept by that browser: never by default, though a device that helped before, one that had
  learned its memory ceiling, keeps helping). Helping and pausing need no key either: any page the
  coordinator answers may take a page's tasks, pause the build, and ask about the pool's lead
  (`/work/lead`: hand it to a Mac, or have the Mac serving the page take it over, never forced:
  docs/pool.md §11), and works under "page" and the
  name it gives (what it is and its browser's id: never an agent's). "Stop helping" gives back what
  it has under way at once. `/work/?view` (the menu bar's panel) only watches: it never helps,
  whatever its browser kept or a click asks, and hides "This device". (The old watching-only address,
  `/work/watch/`, leads to `/work/`.) While it helps, its main thread asks for tasks that fit the
  memory the tab spares and beats for every lease; each slot (a Web Worker per core, less one) runs
  a task's programs over an in-memory filesystem (`web/work/runtime.js`, over browser_wasi_shim) and
  sends back what they wrote. (The Macs' own agents still reach the coordinator with the build's key
  from the NAS, `state/coordinator.json`.)
- **The build at a glance** (`web/work/dash.js`, `dash.css`): above its own work, if it helps, the
  page shows the whole build, in the device's light or dark appearance. Five parts, each answering
  many questions at once:
  - **The verdict**, pinned while the page scrolls (on a phone, not): going or not and on how many
    machines, when it'll all be done (and the range; or why it can't be told), when the map next
    gets new data and with what; what needs a look (a Mac out of touch, a job that hasn't moved on
    for a quarter of an hour, on battery, a disk short of room, the NAS away, another app,
    failures, a page in the background); the pause (Pause the build, Pause it now, Resume the
    build: the coordinator's clock orders a page's asks).
  - **The overview:** the share of the areas built, the work left and when it'll be done if the
    Macs keep going, how much of that time was measured; the steps as a strip; the numbers that
    matter (areas built, regions on the map, as drawn now and as they were, terrain and slope, tree
    cover, map tiles, the last map update (the catalog served) and the next, the
    machines).
  - **The machines,** a card each (the build Mac, each helper, the pages together; a helper that's
    stopped reporting, with when it was last heard from): its job, its parts, its progress with its
    time left, its threads and memory, whether it's stuck; its next jobs (the forecast's); why it
    waits; the build Mac's second job beside it (docs/plan.md §8, Two jobs at once), or why it has
    none, with its next jobs; for a helper, how the work offered fits it (taken by another, done, kept from it after it
    failed it, too large for its memory); its power, the NAS's answer (and room), its disk and
    caches, memory, load, whether it's in use, its app and pace (measured, or a guess); its last day
    by the hour. The helpers' statuses are read each loop.
  - **The pool** (with it on: `web/work/pool.js`, docs/pool.md §11): who leads and since when; a
    card per Mac in the pool, with its state (home on power, on battery, away, out of touch, app
    too old), "Make lead" (confirming; greyed with why when the lead can't be handed to it), on the
    lead's a handover's progress as it happens; "Take it" when there's no lead in touch; the
    proactive offer; the last ask and the last change of lead. From the serving Mac's agent's
    status (`/work/swarm`'s `agent.pool.lead`).
  - **The road to done:** each machine's schedule from now to the end (the forecast's lanes, a step
    a colour, each round of publishing marked: pointed at, the regions it adds; the build Mac's
    second job's lane under its own; the pages' lane is the build Mac's area runs, both jobs',
    whose last steps it hands them as it builds them, drawn while a page around is measured faster
    than the build Mac at tails (`/work/swarm`'s workers' `paces`, by kind) and spares what a tail
    takes typically (the last tails offered, `task_mb`, by kind), and its tree cover pieces, whose
    rows of blocks it hands them, while one is likewise faster at `treeblock`; with a worker faster
    at tails around, the forecast gives each of the build Mac's units the 30 s it gives one to take
    its tail; a piece gets nothing more: its run makes its other blocks while it waits); the map updates
    (the last, the next with its regions, the rounds to come); the steps (done of all, the work
    left, done when, why one waits); the regions, in the order they reach the map with the rounds
    between (or by name, or by what's left), each with its state, its work left, when it's done and
    when it's on the map, found by name.
  - **The activity:** the last day by the hour (areas an hour, or busy minutes, each machine a
    colour; paused hours shaded), and what happened, newest first (all, problems, map updates,
    pauses and conditions), what came since this browser last showed it highlighted and summed; the
    pool's terms (handed over, taken back, taken over, re-asserted, stepped down) among them.
  - A refresh leaves what the user's doing alone: a part where they're typing or have text
    selected stays until they're done, the feed keeps its place, the region search redraws its rows
    alone.
  - **The details,** folded: the leases (since their last beat, until they lapse), what's waiting
    and why, the build Mac's last jobs, recipes that don't read, its job's last lines, the workers.
  - It reads `/work/swarm` every 10 s (the build Mac's heartbeat with its checklist, forecast,
    resources and last catalog, its helpers', every worker with how the work offered fits it, every
    lease, the tasks by state, the history's last number and the last day by the hour) and
    `/work/history` (the events after the last it has), one read at a time. A page says, in its
    asks, whether it's in front, and in its beats how far its task is.
- **HTTPS:** the screen wake lock, the page as an app and OPFS (below) need a secure context. The
  coordinator is reached over HTTPS through `tailscale serve` (the owner turned it on, 2026-10-05:
  the tailnet has a certificate for the build Mac's name), and the page's address (the status
  bar's, `scenic status`'s) is HTTPS while it proxies the coordinator; a job's requests can't come
  through it (they come from the build Mac itself, through no
  proxy). Plain HTTP on the tailnet still works (WireGuard
  encrypts it), the device then kept awake by hand.
- **The page as an app** (a PWA: `manifest.webmanifest`, `sw.js`, `icons/`, served without the
  token like the page): installable (the browser's own Install, or on an iPhone or iPad Share, Add
  to Home Screen, which the page says how to, an iPad's too), opening full screen straight to work.
  - Its service worker keeps the page's files (the coordinator's list, in it) as it installs. It
    takes them from the build Mac when it answers well within 4 s (a newly published app's page at
    once), else from what it kept. A Mac that doesn't (asleep, away, or its agent restarting:
    `tailscale serve` answers 502) is taken for away for a minute.
  - It keeps the programs' WebAssembly by version (their addresses' `?v=`): opened again, the page
    fetches no program it has, and only each program's newest version is kept. Nothing else is
    touched (a task's files, every request to the coordinator), and a cache write that fails (the
    storage full) never fails the answer.
  - Its version is the coordinator's hash of the page's files: a newly published page is a new
    service worker, which takes over, and the page reloads into it once no task is running (paused,
    it stays paused). The page asks for one whenever it comes back to the front and every half hour.
  - An installed app on an iPhone keeps its own storage, apart from Safari's: when it's to help (or
    pause the build), the page asks the build Mac there too, and keeps its own key once accepted;
    forgotten on the build Mac, it stops helping and may ask again.
  - It still runs only while it's open on screen: neither iOS nor Android lets a web app work in
    the background.
- **Planned:** inputs and outputs in OPFS, read through a `FileSystemSyncAccessHandle`, so a worker
  holds less in memory and a reload resumes the upload, not the work.

## 8. Native workers (built)

- **The M1** asks for the shared steps' jobs (terrain, slope, tree cover's pieces, units,
  candidates, peaks, the 3D buildings' bldprep and bldtiles: docs/plan.md §8, Two Macs) and tasks
  (tails, the 3D buildings' areas) over HTTP. A job runs the build Mac's own command for its step,
  its saves handed back through the coordinator; a task runs as `scenic run-task` (its files fetched
  from the coordinator, its steps run natively, the files they wrote sent back).
- **The build Mac's second job** (docs/plan.md §8, Two jobs at once) is a worker of its own in the
  history and the forecast ("<host> (second job)"): beside its first job, the network-bound steps
  first, then the candidates and peaks, then units and slope, then the 3D buildings; its units
  offer their tails as the first job's do.
- **Planned:** the build Mac's own work as tasks too.

## 9. Security

- **Built:**
  - The coordinator answers this Mac, its LAN and the tailnet only (through the proxy on this Mac
    too: the address it took a request from, X-Forwarded-For's last, one of those, never the
    internet's through Tailscale Funnel), and a request only when it names this Mac (its `Host`: an
    address, or a name only a tailnet or a local network resolves, never a public one a web page
    elsewhere could point here: DNS rebinding) and comes from no web page elsewhere (its `Origin`,
    when it has one: this Mac's own, or the address it asks).
    `crate::net::ours`, `from_the_page`, as the map's server. Its JSON requests are POSTs of
    `application/json` alone, which a page elsewhere can't send without asking first (nothing here
    answers that).
  - Keys, compared in constant time: the Macs' agents carry the workers' token (128 random bits,
    kept on the build Mac and in the contact on the NAS); a wrong one is refused (401), so an agent
    with an old one reads the new one. A page carries none: it may take a page's tasks (its asks,
    beats, hand-backs, failures and its tasks' files, under "page <its name>", never an agent's; what
    its tasks took no more than a page's 4 GB) and pause the build (made now by the build Mac's
    clock, said to be the build page's), nothing else.
  - From the build Mac itself only, through no proxy (`tailscale serve` hands the tailnet's
    requests over from loopback, and says so: `crate::net::own`): a running job's (offering tasks,
    with the agent's own token, never published).
  - A worker is served only the files of the task it holds, and uploads only into that task's
    folder; request bodies are capped; connections are bounded (at most 512 at once, headers within
    20 s, so idle ones close too, an upload cut after two minutes without a byte, a connection's
    life at most three hours, a device gone without a word noticed by TCP keepalive), and a slow one
    holds a task of the coordinator's runtime, not a thread; a hand-off may change only the files a
    unit job saves for its lease's units, each to a content name of that file; lease ids are never
    given twice, across restarts too; an agent's ask to pause can't say it was made more than a
    minute ahead of the build Mac's clock; what a worker says it's doing is kept to 200
    characters, and a worker not heard from for a day, holding nothing, is dropped.
- **Known limits:** any device on the LAN or the tailnet can take a page's tasks, hand back wrong
  outputs for them (a worker's first results are checked against the build Mac's, then one in
  eight: §7), or pause the build: no keys, the owner's choice (2026-10-08). A page's uploads are
  bounded by the file (8 GB), not by the task.
- Licensed data on the owner's own devices isn't redistribution (plan §3, sources' terms).

## 10. Results

- **The pilot (2026-10-04):** on unit 6/20/22 (86,904 roads), the unit's Rust programs compiled to
  WebAssembly gave byte-identical outputs in V8 (Chromium, Node) and JavaScriptCore (Safari's
  engine), at 1.45× and 1.24× native time on one thread, once the transcendental functions came from
  one library. Peak memory: canopy 3.5 GB (whole canopy files read, full-width bands decoded), view
  1 GB, the rest under 600 MB. A GPU version of the view kernel ran 15× faster than 14 CPU threads
  (WebGPU too), but the view step is ~0.4% of a unit's time and nothing else is GPU-bound: not
  pursued.
- **End to end (2026-10-04):** the coordinator offered 6/20/22's view, buildings and flags to the
  page in the app's browser, which ran them in 16 s at 1,603 MB; the build Mac's own run of them gave
  the same bytes.
- **The canopy step in bands (2026-10-05):** it reads only the strips and columns each band of 1,024
  rows needs: 3.4 GB → 0.55 GB natively and 3.5 → 0.55 GB in WebAssembly on 6/20/22, the same bytes
  (bands down to 5 rows, across 10° boundaries).
- **The 3D buildings' areas (2026-10-08):** Paris's densest z8 area (8/129/88: 2.6 M buildings, a
  task of 119 MB) made by `bldtile` from its task's files gave the same bytes natively on one thread
  (11.5 s, 0.86 GB) and on all (6.4 s) and in Node's WebAssembly (28 s, 0.79 GB of memory), all on
  the loaded M1, and the same tiles as the pack built whole (`tools/check/bldtile-same.mjs`).
- **Tree cover's blocks (2026-10-05):** ten zoom-8 blocks (coasts, dense forest, the tropics, one
  across four canopy squares) gave the same bytes natively on 1 and 14 threads and in Node's
  WebAssembly, the latter reading only the squares' bytes the native run recorded (3 to 280 MB),
  in 130 to 180 MB of memory (`tools/check/trees-same.py`).

## 11. Steps

1. **Measure and check:** a unit's wall time part by part; the determinism check. *Done.*
2. **The data plane under today's jobs:** the SSD copies, prefetching the next unit. *Done;* journaled
   group commits, dataset indexes, latency pacing *planned.*
3. **The M1 through HTTP leases.** *Done;* claims and NAS hand-off files kept one release for a
   helper on an older app, then retired.
4. **Units fan out:** a unit's last steps to any worker; the canopy step in bands. *Done;* sample
   ranges, ranged reads *planned.*
5. **Browsers:** the page, imported-memory ceilings, ramped verification. *Done;* HTTPS through
   Tailscale and OPFS *planned.*
6. **The Python steps in Rust:** *done* (the same bytes; the units run them; tree cover's, the same
   pixels); then more kinds of task: the 3D buildings' z8 areas (`bldtile`) *done* (B3), tree
   cover's rows of blocks (`treeblock`) *done*; others,
   and the build Mac's own work as tasks, *planned.*
