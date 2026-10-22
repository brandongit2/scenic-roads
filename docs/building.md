# Building

How the work is divided into z6 tiles and jobs, and how the pool's Macs run them.

## 1. How the work is divided

**The grid.** Most work is per z6 tile (about 600 km across at the equator). The z6 tiles are fixed
by the OSM pass's cut; a split into z7 or z8 tiles waits until a z6 tile outgrows the M4 (task
#480). Packs for serving are per z6 tile (hi), per z3 tile (lo) and one root (z0–2). (The step that
makes a z6 tile's roads is named `unit`, as it is stored; docs/pipeline.md §1 has the terms.)

**Where it runs.** All work runs on the pool's Macs (§2, The pool) except the planet's download,
which the NAS does itself. The jobs (§2 has their order and keys):
1. **The OSM pass**, when the NAS holds a newer planet: the filtered planet, the worldwide sets
   (rail, ferries, designated areas, places, outlines, labels, summits, hiking routes,
   heritage-named objects, water), the worldwide basemap, z6 tile extracts (filtered OSM per z6 tile, 10 km
   buffer, ways whole) and worldwide road values (docs/steps.md).
2. **Worldwide jobs, once per pass:** missing sets, the outlines, hiking routes' ends, each z6 tile's
   reach (docs/regions.md), summits, place labels, the water, the lake fill; once ever, the worldwide z8 terrain.
3. **Heritage sites and designated areas:** one job over the coverage plus 20 km.
4. **Global-source layers near the coverage:** terrain, slope and tree cover, a run per z6 tile
   and an assembly per z3 tile.
5. **Per z6 tile**, for every z6 tile meeting the coverage, a region at a time: base(U), the ways U owns
   (a way belongs to the z6 tile of its first node) that touch the coverage, with per-vertex
   elevations, grade and scenic channels.
6. **Then chains** that don't wait for each other: **roads** (road → z6 tiles index, pack(T) per z6
   tile from the base data of the z6 tiles whose ways come into it, lo packs, rail stops and ferries, roots,
   then a catalog), **rail service** (docs/steps.md), **landmarks** (`docs/phase5.md`) and **3D buildings**
   (`docs/buildings3d.md`).

Because road values come from the whole planet, adding a region changes nothing outside its own
z6 tiles and the packs of the tiles their ways come into.

## 2. Building

**Regions:** the agent works out what a region change affects from job keys. After an edit to
`inputs/regions/` the regions' work waits a quarter of an hour for more edits, an hour at most
from the first (`edit_held`).

**Scheduler.**
- **The plan:** every step's targets come with their keys (`state/build/jobs.json`); a target is
  stale when its key changed.
- **A new key scheme** (`agent::rekey`): when a step's keys change what they name, its records are
  re-keyed rather than built again: a record current under the old scheme gets its new key when
  every input the new key names is pinned by the old key's; else it's built again. Every reader
  re-keys records as it reads them (`agent::rekey::as_read`: the plan, the status, the checklist,
  the regions' state, `--expect-same`, `scenic-build p5-check`), deriving in memory
  (`rekey::derive`), a record a job saved always winning. The old schemes (`rekey::v1`, `v2`, and
  the terrain z6 tiles' keys naming their sources as code had them, `rekey::SOURCES_BEFORE`) are
  kept for late records. `scenic-build rekey-check`, `scenic-build p5-check terrain`, `scenic-build p5-check trees` and `scenic-build water-key-check`
  say what they'd do, reading only. Z6 tiles current only by derivation have no mid: it's made in idle
  time, expected the same (Order).
- **A job** is one step over a run of stale targets: as many as fit a quarter of an hour by their
  measured times (`coord::JOB_S`), a step kept to the lead in one job, so a failure or a new app
  costs one run.
- **Order:** the lead offers its plan in its order through its coordinator, and every Mac's agent,
  the lead's included, asks it for work (docs/pool.md §7.2). It plans again when what
  the plan depends on changes (`plan_seen`: the manifest, records, hand-offs, the gate, the pool,
  the pause, a newer app, switch files, catalogs, leases), at the earliest end of its timed waits
  (`replan_at`), and otherwise every five minutes (`plan_due`). Planning reads the terrain packs'
  indexes once each by content name, kept in the agent's `pack-idx/`. On the lead, the journal is
  merged each loop.
- **Jobs at once** (docs/pool.md §7.2, §7.7): every job starts by a grant of the lead's
  coordinator, the lead's own too, packed by their measures: a job beside others only when their
  measured memory, disk and cores fit the Mac's room. Never beside a cold start (a step none of
  whose targets has run), and one job at a time of a step kept to the lead. When the jobs hold more
  than the Mac's limit, the memory guard stops one beside the largest at its next safe point, or
  the larger at once (`agent::memguard`). Room is made only while no other job runs. Each slot past
  the first has its own scratch (`scratch-<k>/`), record and threads, and is a worker of its own in
  the history ("<host> #<k>"); the status has every slot's job in `jobs`, each with its slot.
  Several jobs per Mac only while every member reports the new app (its asks carry a capacity):
  a member on an older app is granted one job at a time and holds every Mac to one meanwhile, the
  old status fields (`job`, `beside`, `more`) written for it until it updates (docs/pool.md §7.1,
  Mixed versions).
- **A newly installed app:** nothing new starts and every running job is asked to stop at its next
  safe point (what it finished kept); once every slot is free the agent exits and the launcher
  starts the new one. A job reaching no safe point in time is left running then, and the new agent
  takes it over (Restarts, below), its time spent kept (`Agent::update_wait`):
  the time its target under way is expected to need, by its progress's pace, else its expected time
  (its own measured run, its step's fit or prior, `needs::Book::secs`, from the coordinator's costs;
  else its step's time on this Mac lately) less the time since the job last noted a target done (or
  began), ×1.5 (a target's time varies by about a third between runs), at most 3 hours, never under
  15 minutes; 15 minutes for a target with no estimate. A member, without the coordinator's costs,
  goes by its pace, else its step's time there, else 15 minutes. Once 15 minutes have passed, a job
  whose processes used less than a core-minute of CPU over the last 30 minutes (the memory guard's
  sampler) holds the restart no longer: by its CPU, since a long step may print no progress for hours. The log and the status's Waiting line say how long and
  why. Pauses, the memory guard and the disk guard keep their own rules. The lead
  grants nothing to an agent on another app, and an agent with a newer app waiting asks for
  nothing.
- **Pausing** (`pipeline::control`): one pause for the whole build, every Mac's jobs and the pages'
  tasks.
  - **Asked for** from the menu bar (Pause Building; Option: Pause Building Now), the map's build
    panel or `scenic pause [--now]`, lifted by Resume or `scenic resume`: an ask in the agent's folder
    (`pause-request.json`) passed to the lead's coordinator; an ask older than the build's last
    change is passed over.
  - **Held** by the lead's coordinator (`coord/pause.json`, mirrored to `state/build/pause.json`) and
    told to every worker; each agent keeps what it last heard.
  - **At a safe point** (the default): each job finishes its target, notes it done (`SCENIC_DONE`)
    and exits as paused (75; `SCENIC_CONTROL` said "drain"); a job not there within 15 minutes is
    frozen instead. **Now:** every job frozen at once (SIGSTOP).
  - **While paused** no lease lapses, and a job stopped by the pause, sleep or a restart is given
    back, not failed.
  - A job without the NAS is frozen at once and goes on when it's back.
  - **Any job's end** records the targets it noted done.
- **Room on the disk:** before a job starts, when the Mac has less free than the job needs (the
  owner's room target past its measured disk and the other jobs' reservations; the OSM pass 80 GB
  past the target; no fixed reserve; pass-sets, which makes the pass's missing sets, copies the
  filtered planet here only with its size and 20 GB free past the target), copies of what the NAS keeps
  lose files until there's a sixth more.
  - **Mid-job, safely by design** (`store::cachefile`, `dem/cachefile.py`): every cache read goes
    through one accessor that refills a missing file from where that cache fills from, so a wrong
    deletion costs a fetch, never a failure. A job holds each cache file it uses with a shared
    `flock`; a file is deleted only under an exclusive lock taken without waiting; a file is made
    under a temporary name and named only if the name is free. A guard test fails on code naming a
    cache path outside the accessor (`tests/cache_accessor.rs`), and chaos tests delete cache files
    under real steps and check the outputs are byte-identical. What can't be refilled (the DEM seed
    while the NAS hasn't it whole, the heritage clip without its planet, raw tiles and pageview
    months the NAS lacks) stays. What queued and running jobs read goes last (`room::Hints`).
  - **Disk measured** (docs/pool.md §7.7): what each job writes is counted by the accessor and by
    the agent's sampler over the job's scratch folder; the most a target took is its **disk floor**
    (the lead's `disks.json`). The **disk guard**: below the owner's room target, the fastest-growing
    job is frozen, room is made, and the job goes on or is given back; with no target set, only at
    a full disk.
  - The order of freeing: canopy squares and copies idle an hour, least recently used first; then
    raw tiles a folder at a time and the squares read since. A file goes once the NAS has it at the
    same size; what the NAS lacks is put there first (raw tiles packed as an archive), or kept.
  - **After the build** (`room::trim`): once the lead's forecast says nothing is left, each agent at
    home empties those copies once (the M4 keeps the canopy squares), and again only after a job has
    run there. Logged in its status (`caches.trimmed`) and the history.
  - **On the owner's ask** (`room::clear`): Clear the Build's Caches in the menu bar, or `scenic
    clean`, after a confirmation naming each cache and how it comes back, writes `clear-request.json`;
    once the build is done the agent empties every cache a later job refills from the NAS
    (`caches.cleared`), else says why not (`caches.declined`). The menu shows what it would free
    (`caches.clearable`, `caches.each`).
  - **The owner's room target** (`room::Target`, `room::toward`): `scenic room <GB>` / `scenic room
    off` or the menu's Disk Room, per Mac (`room-target.json`). A job starts only with the target free
    past its own room; one that can't waits, saying so in `waiting`. Whenever the disk is short of
    the target, the agent frees its caches toward it on a thread of its own, the cheapest to refill
    first (copies, base packs, the DEM seed, the heritage clip last), measuring as it goes; short with
    nothing more to free, it says so (`caches.room.short`). It never deletes what a job holds.
  - **With the mirror's reserve** (docs/macs.md, Mirror): the two floors are apart; the mirror never frees for
    the agent's target and the agent never touches the mirror (task #460).
  - A trim, clear or freeing runs on its own thread; jobs run meanwhile. Nothing goes through a link.
    Each ends early when the agent is asked to stop.
- **Z6 tiles run in map order** (by 10° square, then tile), so what one fetches serves the next.
- **Retries:** a failed job is retried after 10 minutes, doubling to 6 hours.
- **Restarts** (task #393): a job outlives its agent. Its record (`job[-k].json` in the agent's
  folder) holds its spec, threads and log from its start, and the agent saves its slot there (its
  lease, its grant's disk and memory, the budgets' proxy's port) whenever that changes
  (`Agent::save_hands`); a shell at the head of its process group writes its exit status beside
  the record (`job[-k].status`). An agent that ends for anything but a quit (a newer app's
  restart, its launcher restarted, a SIGTERM) leaves its jobs running, its leases not ended
  (`hand_over_jobs`); so does a crash after the first save. The next agent, as it starts and
  before it plans, takes each over (`adopt_jobs`, `jobs::adopt`): its group still the job's (its
  head's start time), or ended with its status written (then no group is kept: nothing signals
  an id another may have taken). The lead's own lease is renewed, or held
  again under a new id if its coordinator's restart ended it (never if another holds its targets:
  then the job is stopped, as at any lapse), its grant's disk reservation kept; a member's stays
  the lead's, and is held in the member's own coordinator at once if it takes the lead in its
  process. Its channel is set to
  "run" (a drain the agent before asked for is this one's to ask again), or "drain" while this
  Mac's build is paused (the pool's or its own), the memory guard watches it, the pool doesn't hand its folder off as left (`take_left`), the proxy listens again on the
  port the job was told while it's free (taken, the jobs taken over are stopped as interrupted,
  their targets given back), and the map server's updater keeps its app's folder
  (`job_apps` in the status). (scenic-build runs its steps' programs from that folder, `app/current`
  followed as it starts, so an app installed while it runs changes none of them.) Its end is read from the status file: a pause's 75 counts as paused;
  a group gone with no status (killed while no agent tended it) is interrupted, not failed. A
  record an older app wrote, or one with no slot saved, isn't taken over: its job is stopped at
  start when provably ours, as before. A helper's leased job (before the pool) is stopped at the
  agent's end. The app's quit stops its jobs as before (control::Quit). An adopted job's tasks
  offered after the restart run on its own Mac (the coordinator's job token is new: its offers are
  refused, and it runs them itself).
- **The heartbeat:** written locally each loop (~20 s) and to the NAS (`state/status.json`) when it
  changes or every two minutes. It holds the job, its progress (from the job's `progress:` lines)
  with the time left, and a checklist of the build to the end: a line per stage with an item per
  step (docs/pipeline.md §4), its targets done of all in its noun, its jobs left in order, and why
  work isn't this Mac's now. A job of several parts says them (`parts: <i>
  [names]`) and the status lists them, done, under way and to come. A long part says how far it is;
  osmium's `--progress` counts. A `unit` job's z6 tile counts by its stages, each weighted by its learned
  time (`unit-stages.json`). A part's time left comes from its own pace; a stuck part shows as such.
- **The heartbeat's resources:** each Mac's memory, cores and load, disk free and droppable caches,
  NAS latency and free space; its build caches (`caches`) and room target (`caches.room`).
- **The forecast** (`agent::forecast`), made with each plan and in the heartbeat: the work left run
  through in the order the agent runs it. The lead takes the pass's worldwide jobs, then a region at
  a time (terrain, then z6 tiles), else the chains' work; its other slots and each member's any work
  of the plan: a lane per slot each Mac may run now (docs/pool.md §10, Each Mac's jobs), each
  starting with its slot's running job, every job the one the coordinator's own scheduler would
  grant its Mac then (docs/pool.md §7.2, One scheduler).
  Z6 tiles reading stale terrain count `forecast::EXPECTED_STALE` (0.6) as work to come. Publishing
  goes per z6 tile: a region's map tiles (`RegionLeft::packs`: its z6 tiles' map tiles stale now or to be
  once its z6 tiles left are built) are drawn once its z6 tiles are, ahead of the regions' work, each at
  its own measured time; a region is all on the map as it's built now (`map_at`) at its last
  z6 tile's end, plus its map tiles, plus at most a minute for the catalog (`CATALOG_EVERY_S`). The
  whole-map steps go by their cadence (`publishes`): the low zooms, the landmarks and the area
  overlays each once its cadence has passed since it last went (`LastCatalog`), or once no z6 tile or
  terrain is left, whichever comes first (the low zooms may go sooner, half their packs stale).
  Each target takes its last run's time at the lead's pace, else its step's mean, else a first
  guess; each machine at its measured speed (half the lead's until measured). Run three times for
  a range. It says when each step, region and everything will be done, when each region is all on
  the map, when each cadenced publish may next go, and each machine's schedule, or why there's no
  finish.
- **The history** (`coord::history`): the last week's events (50,000 at most) on the lead's disk
  (`coord/history.jsonl`): jobs, leases, tasks, map tiles planned, catalogs, pauses, workers,
  agents, conditions, cache trims and clears. Summed by the hour for the worker page, and the
  forecast's measure of members' pace and of publishing's time.
- **Timings** (`pipeline::timings`; `dem/timings.py`; a page's task in `web/work/worker.js`): every
  job's run is broken into named phases, each with wall time, CPU time (`getrusage`), its class
  (`nas-read`, `nas-write`, `disk`, `net`, `compute`, `wait`, `mixed`), spans, bytes and files. One
  level of sub-phases; a child program's phases arrive as sub-phases (`SCENIC_PHASES_TO`). Rules:
  every bulk data move, compute stage and external program is a phase; `Out::open` and `Out::save`
  are phases for free; a loop is never a phase per iteration; nothing is timed per item in a hot
  loop; untimed time stays under about 5 %. A run keeps where its untimed time lies (`gaps`: the
  main thread's time between two top-level phases, named by them), and `scenic timings` shows it
  as "(unaccounted)" with its three largest gaps, flagging a kind past 5 % (`UNACCOUNTED_MAX`) with
  a closing line naming every such kind. Background and overlapped phases are marked. Each phase keeps the most memory the job held
  while it was open (`peak_mb`, sampled four times a second over the job's process group), shown in
  the log's table and as `scenic timings`' peak column; `scenic pool needs --fit` ends with each
  step's heaviest phase over its last 20 timed runs (`timings::peak_holders`). The record
  (docs/formats.md, Timings) goes to `SCENIC_TIMINGS`; the agent keeps `timings.jsonl`, the lead's
  coordinator `coord/timings.jsonl` for every worker. `scenic timings [kind] [--last N] [--host
  mac]` prints them (`--here` this Mac's own).
- **Publishing's times** (`pipeline::pubtimes`, task #250): each stage from a z6 tile's job to its
  tiles on every Mac's map leaves an event. In the history: the z6 tile's job ending; its hand-off
  merged (`merged`: each journal entry's key, member, step and targets); the map tiles
  first planned at their keys (`planned`: noted once per tile and key, what was noted known again
  from the history after a restart or a change of lead; with, per z6 tile, the tiles its change made
  stale: their owners whose base pack or road values differ from the served catalog's), leased and
  done; and catalog n written (`catalog`: its `n`, its job's time, delta or whole, the files listed
  and stat'ed, its phases once written, from the catalog's own `catalog-stats:` log line). On the
  NAS, each Mac's map server appends to `state/publish/<host>.jsonl` its adoption of each catalog
  and its mirror's sync of it (begun, and through everything wanted). The Mac writing a catalog
  tells its own server at once (`/api/catalog/refresh`); the others look every 10 s while a build
  runs (docs/macs.md). `scenic timings publish [--last N]` joins them: a z6 tile is in the first catalog begun
  once it was merged and its stale tiles packed; per catalog and per z6 tile, the hand-off, planned, leased, packed, catalog, servers
  and mirrors, the tiles each z6 tile restaled, which of ferries, stations, roadunits, marks and
  names-todo ran, and the medians (the catalogs' phases split delta and whole).

**The pool** (`docs/pool.md`; `crate::pool`, `crate::agent::pool`; other workers: `docs/workers.md`),
always on. Every Mac's agent is a member; one leads each term. A process not in the pool (no NAS at
start, or its member lock held) starts nothing new. `scenic pool [status]` shows it, and `scenic
status` labels the Mac by its part: "lead, coordinator", "member", or "not in the pool: waiting to
join (why)" (`Status::role`, `out_of_pool`). Every Mac serves its own map; the phone, the iPad and
devices away use the M6's (always on; HTTPS by `tailscale serve` there, docs/macs.md, Devices).
- **Joining after an update** (task #423): the agent before may still hold the member lock while it
  drains, so a new agent tries the lock each second for up to two minutes as it starts
  (`JOIN_WAIT`), and joins as it starts once the lock is let go. Still held past that, it starts
  out of the pool: the jobs it took over beat their leases with the lead over HTTP as before, and
  a member's job that ends well there gives its hand-off to the lead to journal (told done,
  `journaled: false`), its folder removed once the lead takes it (kept if the lead refuses it or
  isn't reached, for the next process to hand off as a job it left). A passing error opening the
  member is tried again within the same wait. Each loop it tries the lock again,
  and once it gets it, restarts at once (`Agent::restart_now`), not once its first slot is free: its
  jobs are left running for the next agent, which joins as it starts and takes them over
  (Restarts).
- **Who leads** is the terms' (`state/build/terms/`): the first Mac makes term 1 (or the owner takes
  over: `scenic lead`, pool.md §11) and leads it: it plans, runs a coordinator (`pipeline::coord`,
  port 8090) from which every other worker asks for work, and merges the journal into the records.
  A lead restarting or waking makes the next term naming itself; one on an app older than its term's
  stands down, and another member takes over after two minutes. With `state/pool/slots` on, a
  process changes part in place (docs/pool.md §7.6); otherwise it restarts into it between jobs.
- **What's offered** (crate::agent::every): every step to every member except the lead's own
  (`LEAD_ONLY`: catalogs, prune, `bld-fetch`, the OSM pass, GC, backup, input checks, …), the steps
  that keep the pass's answers, and the worldwide steps. The steps table (`agent::steps`, docs/pool.md
  §7.2) ranks the shared steps (`agent::steps::SHARED`): terrain's and slope's z6 tiles, tree cover's
  z6 tiles, roads (`unit`), the landmarks' candidates and peaks, bldprep and bldtiles. The status marks a step
  any member may take with ⇄ (`agent::steps::place`).
- **What fits a member:** each target is offered with its expected memory: a `unit` job's from its extract;
  another's what its last run took (sampled four times a second, `SCENIC_COSTS`), else a related
  measure, else a first guess per step (terrain z6 tile 3.3 GB, its assembly 1.2, tree cover z6 tile 1,
  slope z6 tile 1.5, peaks 2.5; the 3D buildings' by the rows they read, `agent::bld_peak`). Measures
  of older `coord::cost_version`s count for nothing. A member asks only for steps its disk has room
  for, never while a newer app waits, and takes the earliest step with a fitting target, from the
  front of the plan (the coordinator's `pick_every`; after its own last target of the step when
  that's open).
- **The same app** (docs/pool.md §6.1): a member on another app than the lead's gets nothing (409,
  why in words), since its work would be recorded under keys another app's code made. Its jobs under
  way go on. A page's tasks run the coordinator's own WebAssembly; their results are checked (the
  first three, then one in eight: docs/workers.md §5).
- **The contact:** `state/coordinator.json`, the coordinator's addresses and the agents' token
  (pages need none: docs/workers.md §7), removed when the agent stops.
- **Leases:** ten minutes on the coordinator's clock, renewed by a beat each minute; a lapsed lease's
  work is offered again, its "lapse" in the history saying why (no word from its worker for ten
  minutes: `coord::lapse_why`). A member's job that fails saying nothing goes to the lead with a
  reason all the same (`fail_why`). Paused, every lease is held. The lead's own jobs hold leases too, so a
  target is never built twice at once. A worker's failed z6 tile isn't offered to it for an hour,
  doubling.
- **The coordinator's state** (leases, costs, failures, the pause) is the term's, on the NAS
  (`state/coord/term/<E>/state.json`); the workers' token and accepted devices are the pool's
  (`state/coord/`); the history is each lead's own file per day (`state/coord/history/`).
- **Every job hands off:** under a lease `<term>-<id>` it saves into `agent/pool/jobs/<term>-<id>/`
  (`SCENIC_HANDOFF`, `pipeline::handoff`), then as an entry to the journal
  (`state/journal/<day>/<term>-<id>.json`) with its done record and costs. The lead merges entries
  into its term's records all or nothing, checking each against its step's write-set
  (`agent::steps::writes`) and, for a shared step, the files it saved. It writes the three files
  readers read (`manifest.json`, `jobs.json`, `pending.json`) from them. Until merged, the agent
  plans with its done records on top.
- **Raw terrain tiles a job fetches:** it packs them and puts the archives on the NAS itself, handing
  the lead only their names (`Handoff::raw`); the lead names them in the raw store's index, which
  it alone writes.
- **A job that fails** hands off the targets it finished; the rest are held against it.
- **A catalog** goes out only once the lead's records reflect the journal. **GC** runs only on a
  step that re-asserted the lead's term; it keeps every file the newest records or an unmerged
  journal entry names (`agent::gc::unmerged`), and renames a file to a tombstone first, putting it
  back if touched since.
- **Temporary names** are each Mac's and each process's own.

**Order:**
0. **The input check** (`inputs <unit>`, docs/inputs.md §4.3), before every other job: a unit whose
   drop box, acceptances or accepted index changed is checked (in full once a day), in the first
   slot or beside a job.
1. **The OSM pass**, when the NAS holds a newer planet.
2. **The pass's worldwide jobs:** `pass-sets`, `outlines` (the rest waits for them), hiking routes'
   ends, `reach`, `terrain-z8` (once), roadside buildings (once per release; z6 tiles wait), summits,
   labels, the lake fill, the water.
3. **The regions' build,** a region at a time (`agent::build::plan`):
   - heritage-sites first;
   - regions the map hasn't at all first, then those it has; of each, the one with the fewest z6 tiles
     left first. For each: the stale terrain z3 tiles it reads (z6 tiles, then each assembly once its
     z6 tiles are current with mids), then its z6 tiles whose terrain is built, then slope and tree
     cover; but first, the slope and tree cover of the regions done that the map hasn't as they are
     now. Other members take the earliest step offered them with work that fits, from the front of
     the plan (the coordinator's `pick_every`).
   - **Publishing**, before all of the regions' work (after heritage-sites), as soon as the records
     change, from the records as they are (nothing is fixed as of a time): a prune of what the
     coverage no longer builds (docs/regions.md, Shrinking), then the publishing chain (4, Roads), its map tiles
     whose owners changed a z6 tile's together (`build::by_unit`: the z6 tiles whose base pack or road
     values differ from the served catalog's, in build order; the tiles no z6 tile's change explains
     by their z3 tile), each group its own work and offer, so a job of them opens one halo of base
     packs (crate::needs::HALO_STEPS: its disk the halo's cache, not its tiles' summed, its NAS
     weight what it writes; docs/pool.md §7.1, §7.7), offered in the plan's order to every
     Mac asking, the lead's agent among them (docs/pool.md §7.2); once the map tiles and the road
     index are drawn (`build::CATALOG_GATE`) and none of their hand-offs waits to be merged (or ten
     minutes after it was first held back), a
     catalog (5), listed before the chain's rest (the low zooms, rail stops, ferries, roots), which
     goes out with a later one. Nothing is published before a z6 tile is
     built, while the regions have work left. The lead keeps `publishing.json` in its agent's folder
     (`build::LastCatalog`: when its last catalog went out, the last with the low zooms made, and
     when a catalog due was first held back since),
     and records what publishing does and why (`build::PublishLine`), shown in the build status
     (`publishing`), the worker page and `scenic status`, and logged when it changes. Beside it the
     status's `published` (`agent::Published`): the last catalog (number, when, with the low zooms
     or map tiles only), the map tiles being drawn by Mac (pack's targets leased), and the catalog
     each Mac's map server last adopted (`pubtimes`' servers' log). The worker page's Publishing
     card and `scenic status` say these in the same words, with the cadenced publishes next (the
     forecast's `publishes`); the Regions panel, `scenic status` and the menu bar say a region
     still building as "x of y tiles, on the map as they come", and when all of it is.
   - **The low zooms' cadence** (`build::low_gate`): the low zooms (roads' and rails' lo packs,
     terrain's and slope's roots, and with `near-slope` the slope and tree cover assemblies) are
     planned when nothing else is queued (no `unit`, terrain, slope or tree cover z6 tile, 3D buildings or
     depth work), at once; else `build::LOW_EVERY_S` (three hours) after a catalog last went out
     with them made (`LastCatalog::low_at`), when half the roads' lo packs are stale
     (`build::low_due`), or once everything is built. The gate decides only when; their keys stay
     by content. Zoom ranges don't overlap, so a region new since shows its roads from z9 until the
     low zooms are next made.
   - **Publishing's switches** (crate::agent::publish), under `state/pool/`, each off while missing
     or `off`:
     - `delta-catalog`: the catalog job (`scenic-build catalog --delta`) writes what changed over the
       last (docs/formats.md, Delta catalogs), each file's size from the records
       (`state/build/sizes.json`, which the files' jobs hand off) or the last catalog: it looks up no
       file's size on the NAS but a file written before sizes were recorded. The whole catalog, which looks
       at every file, is an idle-time job (`scenic-build catalog --whole`) the lead's agent runs
       when the deltas add up to a whole one (`store::catalog::whole_due`), or a delta's run nears
       `MAX_CHAIN`. Readers apply deltas (`store::catalog::latest_nas_from`). Switch on only once
       every Mac's server reads deltas. Held catalogs are always whole.
     - `near-slope`: a done region's slope and tree cover are the z6 tiles near it
       (`Publishing::near`), not every stale z6 tile around; it's recorded built with those.
4. **Four chains:**
   - **Roads** (the publishing chain, `build::publish_chain`): a prune of map tiles no z6 tile is near,
     pack (the map tiles), the road → z6 tiles index, lo, stations, ferries, terrain and slope roots,
     its first stale step at a time.
   - **Rail service**, from the start: `rail-feeds`, then `rail` (docs/steps.md §10).
   - **Landmarks**, from the start, each step once what it reads is built
     (`agent::build::landmarks_work`): candidates, peaks, items, the heritage chain, marks, then the
     overlays. Marks at most hourly while z6 tiles build (`MARKS_EVERY_S`), the overlays at most every
     6 h (`OVERLAYS_EVERY_S`) on their own gate, from the dots the last marks run wrote; both at
     once after the last z6 tile.
   - **Gated by time** (`build::GATED`: marks, overlays, names-todo): the gate decides only when
     a step is planned; its key stays by content. When each last ran is the lead's observation of
     its recorded key changing (`LastCatalog::observe`, kept in `publishing.json`; not known: due).
   - **3D buildings** (`docs/buildings3d.md` §3.3; `agent::build::bld_work`): `bld-fetch`, a prune,
     `bldprep` per z6 tile, then `bldtiles` once a tile and its 8 neighbours are prepared, in the
     regions' order.

   The trains', landmarks' and buildings' work is listed after the regions'. Last, in idle time: the
   mids of z6 tiles current without one, each made again with `--expect-same` (`scenic-build
   trees|terrain|slope --expect-same`: a pack differing from the manifest fails the job).
5. **A catalog** whenever the served files, the regions or which of them are done change
   (`build::catalog_key`), at most one each `build::CATALOG_EVERY_S` (a minute) since the last
   began. It waits while the map tiles or the road index are left to draw (`build::CATALOG_GATE`)
   or a hand-off of theirs waits to be merged. That wait is counted from when a catalog due was
   first held back, whether or not one was made before (`Publish::held_for`: the lead's agent keeps
   that time in `publishing.json`, `LastCatalog::held_at`, so a restart doesn't reset it; a new
   lead's agent, with a record of its own, starts the count afresh). It's reset by each catalog
   made, and when none is held back, none is due and none waits out its minute: past
   `build::CATALOG_CAP_S` (ten minutes) one goes out with what's drawn and merged, its minute
   allowing, the status saying which held it (the map tiles or the road index to draw, or their
   hand-offs waiting to be merged). It records as built (`--ready <id>=<outline
   digest>,…`) the regions done whose z6 tiles are merged, whose map tiles are drawn and merged and
   whose slope and tree cover are made, and the others as the last catalog had them; the status
   says why a done region isn't yet ("waiting to be merged", "map tiles to draw", "slope and tree
   cover to make"). A z6 tile's hand-off waiting holds no catalog: it goes out with a later one.
   While `inputs/hold-catalog` exists, catalogs go to `catalog-held/`.
6. **Daily:** backup and GC.

**Planned:** stale work after step-version bumps done oldest first in idle time; registers, Overture
and timetables refetched every ~6 months.

**Interruptions.** Any Mac may be asleep, away or unplugged at any time; nothing depends on it being
available. Until work is done, the map serves the last catalog.
- When the NAS goes, the agent freezes the job (`SIGSTOP`) and resumes it (`SIGCONT`) when it's back.
- Sleep often breaks SMB handles, so a job touching the NAS restarts after wake.
- **Kills** lose only the target under way: each target is noted done as it's saved. The OSM pass is
  a chain of stages with completion markers. The planet's copy and the mirror resume by byte range.
- **Atomic writes** (docs/storage.md); **caches are disposable**.

**Status.**
- `scenic status` (the lead, its jobs, what waits, the checklist by stage and step, the regions),
  `scenic steps` (the steps table: docs/pipeline.md), `scenic clean`, `scenic room` (Room on the
  disk, above), `scenic memory` (the build's memory: docs/pool.md §7.1).
- The app's status bar: the NAS or map server out of reach, new data or a new app.
- **The menu bar item** asks the local server (`/api/build`): this Mac's agent's status, else the
  NAS's copy. An agent not heard from for six minutes shows as out of touch.
  - **Its panel:** a click opens this Mac's own agent's build page in a popover
    (`http://127.0.0.1:8090/work/?view`: every Mac's agent serves it from its own read of the
    build's state, docs/pool.md §10), in a web view that keeps nothing, kept for the next opening
    and quiet while closed, 900 pt tall at most. With this Mac's agent not answering, one line says
    so ("The build's agent isn't running on this Mac"); with it on another app than the menu bar's
    (the swarm answer's `app`), the footer says so. From it, as from any page on this Mac, a job
    can be cancelled and a failed target tried again (docs/pool.md §11).
  - **Its menu** (right-click, or "⋯"): the state, downloads and caches; Pause/Resume Building; the
    lead items (docs/pool.md §11); Clear the Build's Caches; Disk Room ▸; Memory ▸; Open the Build Log; Open
    the Map; Copy the Map's Address; Open/Copy the Build Page.
  - Its timers run in the run loop's common modes, so it updates while a menu is open
    (`scenic-status --menu-proof`).
  - **The input check** (docs/inputs.md §4.7): a held input badges the icon, its menu has a line per
    held unit (Accept its warnings; Show on the Build Page), and it notifies. `scenic status` and
    `scenic inputs` print the units' states; the build page shows a banner
    per held unit.

**Determinism:** the same inputs give the same bytes, on any machine and in WebAssembly: sorted
outputs, no hash-map order, reductions independent of thread count, and every transcendental
function from one implementation (the `det` crate, over `libm`). Checked per step by hand and by
`tools/check/`; planned: a "build twice, compare hashes" test per step.

**Validation:** every upload is read back and checked against its hash, and a catalog fails on a
missing file. A held catalog is compared with the current map by hand. Planned: decode checks,
plausible ranges, and holding a publish when a z6 tile loses over 20 % of its ways without its coverage
shrinking.

**Tests:** a test holds on any Mac in any state, so none turns on the wall clock, the real machine
or its siblings' order:
- A decision is a function of its inputs, the time among them (`edit_held`, `runs_at`).
- Code that waits takes a clock (`store::clock`: `Offload::clock`, `Shared::clock`,
  `Side::set_clock`); tests use a virtual clock and count waits rather than time them.
- The agent reads its Mac through `cond::Mac`, fixed in tests (`Mac::TEST`), the disk's free space
  set (`room::TEST_FREE`, `free_set`).
- Exchanges with real threads or processes hold on gates, not sleeps.
- What must happen eventually is waited for, bounded only by a five-minute watchdog that says what
  never came.
- File times come from the time the code is given, or far off.
- A test that would see other tests' work runs alone in its own process. Unit tests are offline.
  Tests reading real data (the NAS, `data/build`) are `#[ignore]`d.

**Format bumps:** publish an app that reads both forms, then the data; drop the old reader once
everything is rebuilt.

**App publishing:** `tools/app/publish.sh`, run on either Mac with the NAS mounted:
1. takes a lock on the NAS (`app/publish.lock`, create-new; one over six hours old taken over);
2. refuses unless the tree is clean (the task board's db aside) and HEAD is on origin's main;
3. refuses while the lead's role's old name appears in the code, docs or UI (`tools/taskboard`
   exempt);
4. builds the server and pipeline;
5. runs the crates' tests, the type check and the web build, and checks every Python step loads
   from a copy of `dem/` in a fresh environment and passes pyflakes;
6. smoke-tests a server on a spare port against the NAS's catalog;
7. writes `app/<version>/` and `app/current.json` with every file's SHA-256;
8. copies `tools/nas/fetch-planet.sh` to the NAS's `nas/` when it differs.

`publish.sh --rollback` swaps `current.json` and `previous.json`. The agent never fetches from git.

**Programs:** `scenic` (the user's command and the agent); `scenic-build` (the build steps);
`server`; the `unit` job's programs `extract`, `elev`, `areaflags`, `landcover`, `tile`, `scenic-metrics`
(rail runs `extract` and `railfreq`, tree cover `trees`). The app also carries `dem/` (the Python
steps, run in `dem/.venv`, which uv makes from `uv.lock` at a Mac's first Python step: task #456),
Scenic.app, `web/` and `fonts/`. `tools/app/install.sh` gives the agent a PATH with uv, osmium, zstd
and Java 21.
