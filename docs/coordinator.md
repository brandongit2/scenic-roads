# The coordinator and its workers: one coordinator, its data plane, any number of workers

This extends docs/building.md §2 (Building) and docs/pool.md. The lead's agent plans, alone merges the build's
records, and runs the coordinator; every worker (each Mac's agent, the lead's own slots among them)
asks it for work that fits, does it, and hands it back (its calls under `/work/`). The build page,
at the coordinator's root with its data and the owner's asks under `/api/` (§7), shows the build to
any device and takes the owner's asks; it runs no work. What is not built is marked planned.

## 1. Goals

- **Fitted work:** a worker says how much memory it spares and is given only work predicted to
  fit.
- **Kind to the NAS:** it is the slowest part (slow listings, ~25 small-file writes a second), so
  each byte leaves it once per need, in large reads, and arrives in large writes.
- **Resilient:** workers vanish, the coordinator restarts, the NAS goes away; none of it may
  corrupt the build.
- **Same bytes everywhere** (docs/building.md §2, Determinism): a task gives identical output on every worker
  and thread count, so any worker's result can stand for the job's own run.

## 2. The model

- **The coordinator** (`agentcore::coord`) runs in the lead's agent on port 8090: it grants jobs,
  holds leases, serves the data plane (§4) and brokers tasks (§5). Each member also brokers its own
  jobs' tasks (docs/pool.md §8).
- **Jobs** of the plan (the z6 tile runs of terrain, slope, tree cover, roads (`unit`), candidates, peaks,
  bldprep, bldtiles, map tiles) go to workers that mount the NAS (every Mac's agent). A job saves
  into the store's content-named files; its record changes come back as one hand-off, posted to the
  lead by its member (docs/pool.md §7.3).
- **Tasks** (`coord::task`) are pure work a running job offers to any worker: programs run over a
  folder of files, giving files back. A task's folder is served by the Mac whose job offers it;
  what it reads of the NAS's stores and the land cover's servers it reads where it lies (§3), never
  writing.
- **Impure work stays at the hub:** downloads into the NAS's stores, staging from packs, and every write to the records.
- **Workers** hold nothing the build depends on: a lost worker costs only its work in hand.

## 3. Tasks

Four kinds, all with the same spec shape (`unit` (stored: the task's z6 or z8 tile), `version`, `runs`, `inputs`, `places`), run by
`scenic run-task` on a Mac's agent:

- **A z6 tile's tail** (`tail`, `pipeline::offload`, `unit::tail`): from elevations on (clean-up and
  grade, prep, canopy, view, buildings, flags). Each step lists the files it reads (`Run::reads`).
  While the NAS's store lacks one of a z6 tile's canopy squares (`unit::canopy_stored`), the
  steps through canopy run in the job (`unit::keep_here`), which downloads it. A native worker
  reads the squares on its NAS mount, riding out a moment's outage of the share as a job's NAS
  reads do (`store::retry`, docs/pool.md §3), till the build pauses: then at once, the task given
  back as paused (`store::retry::any_until`). The `unit` job clones
  the files into the task's folder (copy-on-write) and offers at most three tails at once. A z6
  tile resumed (docs/pool.md §7.2) offers only the steps its last run didn't keep
  (`unit::split_kept`): a worker starts from the folder a run here would take up.
- **The 3D buildings' z8 tiles** (`bldtile`, `bld::bld::task`, docs/buildings3d.md §3.6): a
  `bldtiles` job offers some of a z6 tile's z8 tiles, cut from its work files; the run is `bldtile`
  over `{dir}`, writing `area.tiles` and `area.json` (the files' names from when a z8 tile was
  called an area: kept, as the task's files' names and the `"area"` in their meta are stored).
- **Tree cover's rows of blocks** (`treeblock`, `trees::trees::task`, docs/steps.md §2, Trees): a z6 tile's
  run offers rows (up to four z8 blocks sharing canopy rows), run as `trees --blocks … --coverage
  {dir}/coverage.json --chm {chm} --leaf {sources}/trees/leaf --squares … --out {dir}`. The task's
  one file is the coverage cut to the row; canopy strips are read where they lie.
- **Terrain's z8 subtrees** (`terrainsub`, `terrain::terrain_task`, docs/steps.md §2, Terrain): a z6 tile's
  16 subtrees go out each alone (a tile reads nothing past its subtree: its raw tile, its
  children, its water). Its one file `u/in.sect` holds all it reads;
  the run `terrainsub --in {dir}/in.sect --out {dir}` writes `hi.sect` and `mid.sect`.

Common rules:

- **Its programs** (a run's `prog`: `trees`, `bldtile`, `terrainsub`, a tail's `elev`, `tile`,
  `scenic-metrics`) are scenic-build's tools, run by the worker from its own app version's folder as
  `scenic-build tool <prog> …` (`agentcore::cli::program`; docs/building.md, Programs). A version from
  before #917 runs its binary of that name, so a task reads the same to either.

- **Read where they lie** (`/net` in a task's places): `{sources}` (the NAS's `sources/`; `{chm}`
  the canopy squares) and `{net}` (the land cover's servers' files, laid out as `crate::fetch`'s
  mirror, `<host>/<path>`). A worker reads its own mount, read-only (`Tools::stores_read_only`,
  `SCENIC_STORES_READ_ONLY`).
- **A worker's transfers** (`coordwire::client::Files`): a task's files (`/work/in/`,
  `/work/out/`) have no limit on their whole time, so a big file on a slow link goes on while it
  moves. Connecting has 5 s; after that a transfer fails once nothing has moved for 60 s,
  counted from the connection made, the start and end of each write (ureq's output buffer, each
  write itself limited to 60 s) and each read: the wait for the answer's first byte included. The task's log says each fetched file's size, time and rate; its timings give
  the fetch phase's bytes and rate. Its other requests have 15 s in all. A failed fetch isn't
  resumed: the task fails and its job runs it itself.
- **Inputs held till the lease ends** (`coord::task::stage`, `Tasks::offer`): the coordinator
  clones a task's inputs into its own folder when it's offered (outside its lock; copy-on-write on
  APFS, its folder and the jobs' both in the agent's home; a full copy elsewhere), refusing an
  offer whose input isn't there at the size offered, and serves them from `<id>-in` until the
  task is closed, whatever the job does with its own folders meanwhile; a coordinator starting
  empties its folder of a run before. A fetch for a task that has ended (its job ran it
  itself and closed it) is answered 410, that lease is gone; 404 is an input the task hasn't.
- **What comes back:** only files the steps changed (hashes of the inputs tell).
- **Memory:** predicted from the files (a tail: three times over, at least 300 MB; each kind has
  its own floor) until a worker's run measures it; then that, plus a tenth.
- **Pace and patience:** the coordinator measures each worker's pace per kind against the job's
  own Mac (`Tasks::note_pace`). A job needing a task nobody took waits up to 30 s
  (`offload::LEASE_WAIT`) only while a taker that beats this Mac by a quarter (`task::beats`), or
  is unmeasured and not tried in the last hour (`EXPLORE_EVERY`), is asking; else it runs it
  itself. A task a worker holds is waited on only while its pace says it will finish first
  (`offload::Patience`); past that it's raced, and the job's own run counts.
  While a job only waits it writes `SCENIC_WAITING` (`offload::WAITING_ENV`); its agent, the
  lead's or a member's, says so in its capacity and the lead grants the room it doesn't hold, and
  its cores, to a job beside it, which yields when it goes on (docs/pool.md §7.2). An unmeasured worker's raced task is kept for it to finish
  (`Tasks::measure`, its inputs already the coordinator's) to learn its pace. With placement on (the pool's setting `placement`), terrain and
  tree cover z6 tiles offer ahead (`terrain_task::offer_ahead`) with a head start a slower worker may use.
- **Versions:** a task names the job's build.
- **Determinism:** one maths library (`det`, over `libm`; `clippy.toml` refuses the platform's), one
  geometry (`roadcore::geo`), thread-count-independent reductions, no
  hash-map order in outputs, the real zstd everywhere.
- **Planned:** staging as a task; heavy steps cut into sample ranges; more kinds (map tiles,
  landmarks, slope subtrees); smaller 3D-building z8 tiles.

## 4. Data: the coordinator's plane

- **Built:** packs that staging reads are copied once to the job's Mac's SSD (`blobs/`,
  content-named) and shared by its z6 tiles; the next z6 tile's inputs are copied ahead; task files are
  served from that SSD; one disk budget covers the caches (docs/building.md §2, Room on the disk).
- **Planned:** exact byte ranges fetched;
  writes journaled on the SSD and committed in groups; reads and writes paced by the NAS's latency.

## 5. Control: leases and trust

- **Asking** (`/work/ask`): a worker sends its name, kind (`native`), the work it does, memory,
  cores and its app, with the workers' token. A NAS-mounting worker gets a job first, then a task.
  An agent on a different app than the lead's gets 409 until they match (docs/building.md §2, The
  same app). The workers' calls without the key (an ask, a beat, a hand-back or failure, a task's
  files) are refused (403): a page never works.
- **Pausing** (docs/building.md §2, Pausing): the coordinator holds the pause (in its store); asks are refused, beats carry
  it, and no lease lapses while it holds. Work given back for an interruption isn't held against it.
- **Leases** (`coord::lease`): ten minutes on the coordinator's monotonic clock, renewed by a beat
  each minute; a lapsed lease's work is offered again. A member whose agent speaks for its leases
  (`coord::held`, docs/coordination.md §6) holds its jobs' leases until its word instead: its beat
  to the lead every 10 s lists them, one left out is released, and they go after 12 h unheard or
  by `scenic pool release <member>`.
- **Handing back:** a job's saves, done record and costs go back as one hand-off, posted by its
  member to the lead and merged by the lead (docs/pool.md §7.3). A task's files go back to the
  broker that offered it; a hand-back for a gone lease is refused (410).
- **Restarts:** leases and costs are kept in the lead's store on its disk (backed up to the NAS,
  `state/lead/store.json`), the token in `state/coord/` (docs/pool.md §7.5); a
  worker finds the lead again (`coordwire::lead`: the lead its Mac's agent keeps, else the declared one's) when it can't reach the coordinator or its token fails.
- **Failures:** a worker's failed z6 tile isn't offered to it for an hour, doubling; a task out of
  memory goes only to workers sparing more; after two other failures the job runs it.
- **Planned:** NAS outages extending leases; idle workers racing slow ones when the queue runs dry.

## 6. Fitting the work to the worker

- **Predicted peaks:** a z6 tile's `unit` job is its last measured peak (`SCENIC_COSTS`), else about ten times
  its extract, at least 3.7 GB and its floor (docs/pool.md §7.2). A task's: §3, Memory.

## 7. The build page

- **The page** at the coordinator's root, `/` (`/build/` through Caddy; also the menu bar's panel, on this Mac's own agent's port, and
  `scenic status`'s "Build page"; the menu bar's icon is a filled pause sign while this Mac is
  paused alone, its menu saying so with Resume This Mac, the same ask file as `scenic resume
  --here` (`hold-request.json`); the Jobs tab's schedule chips a paused, out-of-touch or
  on-battery Mac's lanes) shows the build to anyone the Mac answers, with no key: in the
  pool the lead serves it (docs/pool.md §10); a member's port sends it there with a 307, or shows a
  small page of its own while the lead isn't reached (docs/roles.md §3.3). It
  runs none of the build's work. Its data and the owner's asks are under `/api/` (`coord::API`). A page on any device the Mac answers may pause the build (`/api/pause`), accept a
  held unit's warnings (`/api/inputs`), cancel a job and try a failed target again (`/api/act`),
  and change the owner's settings (`/api/settings`, below), each at the lead (a Mac's own settings
  at its own port). None needs a key or a page of the Mac's own first (the owner, 21 Oct). `/?view`
  only watches: the same tabbed page, in the popover's fixed 780 pt width (no
  horizontal scroll on any tab; checked 2026-10-23), its height the page's up to 900 pt, scrolling
  inside beyond that.
- **The settings** (task #521; `web/buildpage/settings.js`, the Machines tab; `crate::agent::settings`):
  each Mac's in-use share (cores kept free while it's used), memory floor (GB kept
  available on the Mac, docs/pool.md §7.1), disk room target (GB kept free) and dev sweep margin
  (GB past the room target at which tools/dev/sweep-targets.sh frees the agents' build caches,
  tools/dev/testing.md), and the pool's NAS reserve (GB kept free on the NAS; "not here" while the
  pool box's Mac doesn't reach the NAS), batch length (minutes of measured time a shared step's
  job is sized to, docs/pool.md §7.7, task #927) and overdue factor (the status flags a job still
  past that many times its usual still stretch, and one whose progress goes on that many times
  slower than its step's slowest tenth of jobs, docs/pool.md §10, Flags), and publishing's two
  switches, on or off (holding publishing for review, `hold-catalog`, and slope near regions,
  `near-slope`: docs/building.md §2), all but the reserve as that Mac's agent last had them from the
  lead, set through any Mac's agent. The pool box is set through the
  first Mac on the page that reaches the NAS, else the first that answered. Each Mac's `POST /api/settings`
  (its coordinator, broker or viewer; the lead's page reaches the others' through CORS) with `{}` reads them; with
  `in_use` (cores), `memory`, `room`, `sweep_margin` or `nas_room` (GB), `batch_minutes` or
  `overdue_factor` (times), each a number or `"off"`, or `hold_catalog` or `near_slope`, `"on"` or
  `"off"`, sets them through the same functions as `scenic in-use`, `scenic memory`, `scenic room`,
  `scenic nas-room` and `scenic pool set batch-minutes|overdue-factor|hold-catalog|near-slope` (the
  sweep's margin has no command; the pool's are asks of the agent, sent to the lead with its next
  beat)
  (the same checks and files; `by` "the build page (<device>)"), and answers with them as they
  stand (`mac`: its host). One ask carrying several is set in order, so a bad one after a good one
  leaves the good one set; the page sends one at a time. A bad value is refused (400) with the check's words. They change only when a person
  presses Set or Off, in a row or a chart's limit editor, or lets go of a chart's limit line dragged (docs/pipeline.md §5). The menu bar's settings are unchanged. (The battery floor went with
  the elected lead, task #881: it only ever decided who could lead.)
- **The build at a glance** (`web/buildpage/dash.js`, `dash.css`, `pool.js`, `settings.js`, `sums.js`, `live.js`, `nas.js`, `activity.js`, `chart.js`, `chartmath.js`, `worldmap.js`, their elements and words `dom.js` and `words.js`; docs/pipeline.md §5): a
  sticky header (the verdict: going, done when; Pause, Freeze, Resume), the world map in a left
  panel beside the tabs (`worldmap.js`: regions, the queue, running and finished tiles; a step
  soloed from its legend, its published tiles by age, `catalogues.js`; a card of the whole stack
  under a point, hovered or pinned by a click or tap; docs/pipeline.md §5), a tab per part by the
  address's `#hash` in the right panel, and a fixed footer (the charts' range, the Live dot).
  Overview (the alerts, the work left in the queue, the last and next catalogue, the Macs); Jobs (the schedule: each
  Mac's lanes with the jobs that ran, running and to come (a job to come's card, as one that ran: its targets and a map of them, from `/api/tocome`, which the page asks once a forecast; a block a job: docs/pool.md); the stages: done and left a step; a card on hover, pinned on a click or tap); Machines (a card per
  machine, the pool: who leads, "Make lead", "Take it"; the settings); Regions (a virtual list; a row under the pointer lights its region on the left panel's map and shows its card); NAS (`nas.js`: how fast it responds, use against its limits, its use over time); Inputs (the gate's held changes with Accept and Accept all, docs/inputs.md §4.7); Activity (last day by the hour,
  events, the file stream and the internet by website: `activity.js`); Details (leases, waits, each Mac's capacity, workers, the steps table with each step's measured
  phase shares: the lead's status's `phase_shares`, docs/pool.md §10). It reads
  `/api/steps` once (the steps table: every word about a step), `/api/swarm` every 10 s and
  `/api/history` incrementally (and further back, a chunk at a time, for the schedule's past jobs); `/api/regions` (each region's folders, download size and
  outline: docs/pool.md §10, Regions) when the last catalogue changes, else every ten minutes (an answer
  the same as the last, its catalogue and version, isn't shown again);
  each Mac's measures over time (its vitals as docs/mockups/build-live.html draws them, each core's share now among them; on the lead also the NAS's disks and the pool's NAS traffic), and its internet's counts by host and last transfers, from its own port's `/api/live` (docs/pool.md §10, Live; the Machines and NAS tabs ask each Mac's every 1.5 s while shown);
  each running job's phase, target and NAS and internet bytes in the status (`jobs[].live`, and a
  member's on its lease, from its beat: the same section). The
  world map draws the regions' outlines, the countries' and the large lakes' (`countries.json`, `lakes.json`, the page's
  files). A 503 (the Mac hasn't read the build's state yet) says so quietly and asks
  again every 2 s, what's shown said to be stale if it had an answer before, an error only after a
  minute of them; a new `epoch` (the
  Mac's agent restarted) starts the events again. "Since you last looked" goes by the events' time,
  so it outlives a restart.
- **A job's page** (task #1105; `web/buildpage/job.html`, `job.js`, its sums `jobview.js`, a run's
  lanes `runlanes.js`, the theme `theme.css` with the build page; docs/pipeline.md §5, A job's page)
  at `/job/<epoch>-<n>` (`/build/job/…` through Caddy), any id, no key: one job run, from the lead's
  `GET /api/job/<id>` (docs/formats.md, Job spans), its step's last records (`/api/timings`, a
  page's to read as `/api/history` is), and its Mac's measures (that Mac's `/api/live`). A member's
  port sends it to the lead (memberpage: like the page's files). The schedule's jobs link to it.
- **HTTPS** through `tailscale serve` (docs/pool.md §14), or the M6's
  https://map.brandontsang.net/build (Caddy, docs/macs.md), gives a secure context;
  plain HTTP on the tailnet still works. The page's files and requests are relative
  to the folder it was served from, so it works under a proxy's path.
- **Its old addresses** (task #1101 moved the page from `/work/` and its data and asks from
  `/work/<name>` to `/api/<name>`; no redirect from them): while an app before it may ask them, a
  coordinator answers them as the new (`coord::oldpage`, gated by `beats::ROOT`: on the lead while
  a member heard doesn't say it, on a member's port while its lead doesn't); a member's port sends
  the page to an older lead's `/work/`, an agent and the page ask an older Mac's `/work/<name>`
  when `/api/` is refused. It
  lifts by itself once every Mac runs the new app; the code goes then (the rule for old-app paths,
  docs/coordination.md; #1121).
- **The page's version and its home screen entry.** The page has no service worker and no offline
  copy: it is what the coordinator answers, and a Mac that doesn't answer shows the browser's own
  error. Its `manifest.webmanifest`, `icons/` and the `apple-touch-icon` link and
  `apple-mobile-web-app-*` meta tags only give a device's own Add to Home Screen (every browser has
  its own way: there is no Install button or hint) its name, icon and full screen.
  A page open for days reloads into a newly published one: the coordinator's hash of the page's
  files (`http::page_version`) is in the page it serves (index.html's and job.html's `page-version`
  meta tag, filled in as it is served) and in the `X-Page-Version` header of every answer (`stamp`); the page
  (`version.js`, from its `call`, so within 10 s while it is visible) reloads when an answer's
  differs, once per version (`sessionStorage`, against a reload that serves the same old page), not
  while a field has the focus or a pointer is down (a setting being typed, a limit line dragged: the
  next answer asks again), and not when an answer has none (the lead away, a proxy's 502) or the
  page was served without one.
  - **A one-off cleanup** (task #1117) of the service worker the page had, for a device that still
    has it: `sw.js` is now a worker that ends itself (no page registers it; a member's port answers
    it too, not with a redirect, which fails a worker's update). A device's next update check of the
    old worker (a page loaded; an old page open for days checks when it comes back to the front and
    half-hourly) takes it, and it deletes the old one's `shell-<version>` caches, unregisters and
    reloads the pages it held into the new page. The page itself also unregisters the worker of its
    own folder (through Caddy, `/build/`: the map's at `/` stays) and deletes those caches. Both
    can go (`sw.js`, its `PAGE` entry and memberpage's arm; index.html's two lines) once every device
    that had the page as an app (the owner's phone, iPad and Macs) has opened it since this app was
    published.

## 8. The workers: the Macs' agents

- **Every Mac's agent** (the lead's slots too: docs/pool.md §7.6) asks for jobs and tasks over HTTP.
  A job runs its step's one command; a task runs as `scenic run-task`.
- **A Mac's slots past the first** (docs/building.md §2, Jobs at once) each appear as a worker of their own
  ("<host> #<k>"); a Mac's agent asks once for as many jobs as it has slots free, and is granted
  them together (docs/pool.md §7.2, Packing). An agent on an older app (its ask carries no
  capacity) is granted one job at a time, and while one is around every Mac is (docs/pool.md §7.1,
  Mixed versions).
- **Planned:** the jobs' own work as tasks too.

## 9. Security

- The coordinator answers only this Mac, its LAN and the tailnet, checks `Host` (against DNS
  rebinding) and `Origin` (`crate::net::ours`, `from_the_page`), and takes JSON only as
  `application/json` POSTs.
- The Macs' agents carry the workers' token (128 random bits, constant-time compare; wrong is 401).
  A page carries none: it reads the build and takes the owner's asks (§7), never work (§5). A
  running job's requests come from the lead itself, through no proxy (`crate::net::own`).
- A worker is served only its task's files and uploads only into its folder; bodies, connections,
  timeouts and names are bounded; lease numbers, and the job ids they make (pool.md §7.5), are never reused.
- The app's releases (§10) are served and taken with the workers' key alone, by the lead alone; a
  release goes live only once it and every file in it match its manifest.
- **Known limits:** by the owner's choice, any device on the LAN or tailnet can pause the build or
  change the owner's settings.
- Licensed data on the owner's own devices isn't redistribution (docs/storage.md).

## 10. The app's releases

- **Kept by the lead** (`coord::releases`, task #1118): the live release (its archive and its
  manifest: every file's SHA-256 and the archive's) and the one before it, in its Mac's
  `<home>/app/releases/` (`jobrt::appupdate::Releases`: `live.json`, one file written whole, and
  the two archives), which every Mac's updater keeps as it follows the lead: a Mac made the lead
  serves what it followed. Its agent gives its coordinator them (`set_releases`); a member's port
  refuses them (409).
- **Served:** `GET /release/live` (`{"current", "previous"}`, each a manifest or null) and
  `GET /release/<version>.tgz` (either's archive), with the workers' key, to every Mac's updater
  (docs/macs.md, The updater) and the map server's.
- **Put live by a publish** (tools/app/publish.sh, through `scenic releases push`): the archive
  streamed in (`PUT /release/upload/<version>`, into `incoming/`, flushed), then
  `POST /release/install` with the manifest and the `base` (the live version publish.sh checked its
  commit against, which needs git): the archive's SHA-256 checked, the archive unpacked aside and
  every file's checked (400 and the upload dropped, else), then under the releases' lock taken only
  while `base` is still the live version (409 with what is live, else: publish.sh reads and checks
  again), the live one kept as the one before, the archives of neither removed, durable before the
  answer. An upload no install took is removed by a later change once an hour old.
- **Rolled back** (`POST /release/rollback`, `scenic releases rollback`, publish.sh --rollback after
  its floor check): from the two releases as read (409 else), the one before made live and the
  live one kept before it.
- **Planned:** nothing of it on the NAS once the rollout ends (docs/building.md, The NAS's `app/`
  until then).
