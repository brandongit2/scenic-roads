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
  - **Jobs** of the plan (the shared steps': terrain, slope, tree cover, units, candidates and
    peaks), for workers that mount the NAS (the M1's agent). A job saves into
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

## 3. Tasks (built for a unit's tail)

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
- **Nothing waits on a worker:** at the end of the job, a task no one took is taken back and run on
  the build Mac, and one a worker still holds is raced there (the build Mac's result counts; a
  worker's that comes in too is compared: each file it sent with this Mac's, and each this Mac's
  run changed that it didn't send with the copy it was sent). With no worker around, a unit
  builds as before.
- **Source versions:** a task names the programs' build (the job's binary); a web worker fetches
  that build's WebAssembly programs from the coordinator (`/work/prog/<name>.wasm`, shipped in the
  app's `wasm/`). The native and WebAssembly builds of one source give the same bytes (§10).
- **Determinism rules:** one maths library (`det`, over `libm`) on every target; reductions that
  don't depend on the thread count; no hash-map order in outputs; the real zstd everywhere.
- **Planned:** staging from packs as a task's (read where the packs lie); the heavy steps cut into
  sample ranges so a slow worker's lease is minutes; more kinds of task (map tiles, landmarks,
  slope, terrain, tree cover).

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

- **Asking:** a worker sends its name, kind, the work it does (the shared steps' jobs, `tail`), the
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

- **The page** is served by the coordinator at `/work/` (the menu bar's Copy the Build Page's
  Address, `scenic status`'s "Build page"): the build at a glance for anyone it answers (this Mac,
  its LAN, the tailnet), with no key; the coordinator answers its reads (`/work/swarm`,
  `/work/history`) without one. A device helps only when its owner asks there ("Help with this
  tab", kept by that browser: never by default, though a device that helped before the page asked,
  one that had learned its memory ceiling, keeps helping). The first time, the page asks the build
  Mac (`/work/join`, `pipeline::coord::devices`) with a secret it makes and alone keeps; the build
  Mac gives the ask a code of its own making, unlike any other ask's waiting, which only the page
  and the build Mac show. Its menu bar shows the ask with that code, and a notification with Accept
  and Decline for that ask alone (or `scenic devices accept <code>`): the owner accepts the ask whose
  code the device's page shows. Accepted, the secret is that device's key, for its own tasks (as
  itself: a page's tasks, under a worker name ending in its page's id) and to pause the build, until
  the owner forgets it there (Devices Helping, or `scenic devices forget <id>`). The page waits for
  the answer, through a reload too; Cancel withdraws the ask; one it couldn't send (the build Mac
  away or restarting) is sent again, less and less often. A browser's tabs share its ask and its
  key. Asks are few (eight waiting at most, three from one address, ten an hour from one address:
  past these, refused, never one waiting dropped), lapse after a day, and are the owner's alone to
  see: the dashboard's reads and the history don't show them (the history has the owner's answers).
  There's no key to copy: the build's own key, which pages from before devices asked carried from
  their address, is no key any more (replaced as the first app with devices started); such a page
  that helped asks again as it opens. "Stop helping" gives back what it has under way at once. (The old watching-only address,
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
  - **The road to done:** each machine's schedule from now to the end (the forecast's lanes, a step
    a colour, each round of publishing marked: pointed at, the regions it adds; the build Mac's
    second job's lane under its own; the pages' lane is the build Mac's area runs, both jobs',
    whose last steps it hands them as it builds them); the map updates
    (the last, the next with its regions, the rounds to come); the steps (done of all, the work
    left, done when, why one waits); the regions, in the order they reach the map with the rounds
    between (or by name, or by what's left), each with its state, its work left, when it's done and
    when it's on the map, found by name.
  - **The activity:** the last day by the hour (areas an hour, or busy minutes, each machine a
    colour; paused hours shaded), and what happened, newest first (all, problems, map updates,
    pauses and conditions), what came since this browser last showed it highlighted and summed.
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
  bar's, `scenic status`'s) is HTTPS while it proxies the coordinator; the owner's requests (the
  devices) and a job's can't come through it (they come from the build Mac itself, through no
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

- **The M1** asks for the shared steps' jobs (terrain, slope, tree cover, units, candidates, peaks:
  docs/plan.md §8, Two Macs) and tails over HTTP. A job runs the build Mac's own command for its step,
  its saves handed back through the coordinator; a tail runs as `scenic run-task` (its files fetched
  from the coordinator, its steps run natively, the files they wrote sent back).
- **The build Mac's second job** (docs/plan.md §8, Two jobs at once) is a worker of its own in the
  history and the forecast ("<host> (second job)"): beside its first job, the network-bound steps
  first, then the candidates and peaks, then units and slope; its units offer their tails as the
  first job's do.
- **Planned:** the build Mac's own work as tasks too.

## 9. Security

- **Built:**
  - The coordinator answers this Mac, its LAN and the tailnet only, and a request only when it names
    this Mac (its `Host`: an address, or a name only a tailnet or a local network resolves, never a
    public one a web page elsewhere could point here: DNS rebinding) and comes from no web page
    elsewhere (its `Origin`, when it has one: this Mac's own, or the address it asks).
    `crate::net::ours`, `from_the_page`, as the map's server. Its JSON requests are POSTs of
    `application/json` alone, which a page elsewhere can't send without asking first (nothing here
    answers that).
  - Keys, compared in constant time: the Macs' agents carry the workers' token (128 random bits,
    kept on the build Mac and in the contact on the NAS; the one pages carried before devices asked
    was replaced once, 2026-10-06). A device that helps through the page carries its own secret,
    good once the owner accepts it on the build Mac and until the owner forgets it: for its own
    tasks (its asks, beats, hand-backs, failures and its tasks' files, under its own name: a
    page's tasks alone) and to pause the build (made now by the build Mac's clock, by what it is),
    nothing else.
  - From the build Mac itself only, through no proxy (`tailscale serve` hands the tailnet's
    requests over from loopback, and says so: `crate::net::own`): the owner's (the devices' asks and
    answers, with the workers' token) and a running job's (offering tasks, with the agent's own
    token, never published).
  - A worker is served only the files of the task it holds, and uploads only into that task's
    folder; request bodies are capped; connections are bounded (at most 512 at once, headers within
    20 s, so idle ones close too, an upload cut after two minutes without a byte, a connection's
    life at most three hours, a device gone without a word noticed by TCP keepalive), and a slow one
    holds a task of the coordinator's runtime, not a thread; a hand-off may change only the files a
    unit job saves for its lease's units, each to a content name of that file; lease ids are never
    given twice, across restarts too; an agent's ask to pause can't say it was made more than a
    minute ahead of the build Mac's clock.
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
6. **The Python steps in Rust:** *done* (the same bytes; the units run them); then more kinds of
   task, and the build Mac's own work as tasks.
