# Workers: one coordinator, its data plane, any number of workers

This extends docs/building.md §2 (Building) and docs/pool.md. The lead's agent plans, alone merges the build's
records, and runs the coordinator; every worker (each Mac's agent, the lead's own slots among them,
any device that opens the build page) asks it for work that fits, does it, and hands it back.
What is not built is marked planned.

## 1. Goals

- **Any device, by opening a page.** No install; a worker says how much memory it spares and is
  given only work predicted to fit. One code path, no device special cases.
- **Kind to the NAS:** it is the slowest part (slow listings, ~25 small-file writes a second), so
  each byte leaves it once per need, in large reads, and arrives in large writes.
- **Resilient:** workers vanish, the coordinator restarts, the NAS goes away; none of it may
  corrupt the build.
- **Same bytes everywhere** (docs/building.md §2, Determinism): a task gives identical output on every worker
  and thread count, so results can be checked by running them twice.

## 2. The model

- **The coordinator** (`pipeline::coord`) runs in the lead's agent on port 8090: it grants jobs,
  holds leases, serves the data plane (§4) and brokers tasks (§5). Each member also brokers its own
  jobs' tasks (docs/pool.md §8).
- **Jobs** of the plan (the z6 tile runs of terrain, slope, tree cover, roads (`unit`), candidates, peaks,
  bldprep, bldtiles, map tiles) go to workers that mount the NAS (every Mac's agent). A job saves
  into the store's content-named files; its record changes come back as one hand-off through the
  journal (docs/pool.md §7.3).
- **Tasks** (`coord::task`) are pure work a running job offers to any worker: programs run over a
  folder of files, giving files back. A task's folder is served by the Mac whose job offers it;
  what it reads of the NAS's stores and the DEM servers it reads where it lies (§3), never writing.
- **Impure work stays at the hub:** downloads into the NAS's stores, staging from packs, keeping a
  z6 tile's DEM samples, and every write to the records.
- **Workers** hold nothing the build depends on: a lost worker costs only its work in hand.

## 3. Tasks

Four kinds, all with the same spec shape (`unit` (stored: the task's z6 or z8 tile), `version`, `runs`, `inputs`, `places`), run by
the page and by `scenic run-task` unchanged:

- **A z6 tile's tail** (`tail`, `pipeline::offload`, `unit::tail`): from elevations on (clean-up and
  grade, prep, canopy, view, buildings, flags). Each step lists the files it reads (`Run::reads`);
  `tools/check/tail.mjs` runs the task as a browser does and compares every file with the native
  run's. While the NAS's store lacks one of a z6 tile's canopy squares (`unit::canopy_stored`), the
  steps through canopy run in the job (`unit::keep_here`), which downloads it. The `unit` job clones
  the files into the task's folder (copy-on-write) and offers at most three tails at once.
- **The 3D buildings' z8 tiles** (`bldtile`, `pipeline::bld::task`, docs/buildings3d.md §3.6): a
  `bldtiles` job offers some of a z6 tile's z8 tiles, cut from its work files; the run is `bldtile`
  over `{dir}`, writing `area.tiles` and `area.json` (the files' names from when a z8 tile was
  called an area: kept, as the task's files' names and the `"area"` in their meta are stored). Checked by `tools/check/bldtile-same.mjs`.
- **Tree cover's rows of blocks** (`treeblock`, `pipeline::trees::task`, docs/steps.md §2, Trees): a z6 tile's
  run offers rows (up to four z8 blocks sharing canopy rows), run as `trees --blocks … --coverage
  {dir}/coverage.json --chm {chm} --leaf {sources}/trees/leaf --squares … --out {dir}`. The task's
  one file is the coverage cut to the row; canopy strips are read where they lie. Checked by
  `tools/check/treeblock-same.mjs`.
- **Terrain's z8 subtrees** (`terrainsub`, `pipeline::terrain_task`, docs/steps.md §2, Terrain): a z6 tile's
  16 subtrees go out in groups closed under the lakes they share (a lake's level depends on all its
  shore); groups over two subtrees are not offered. Its one file `u/in.sect` holds all it reads;
  the run `terrainsub --in {dir}/in.sect --out {dir}` writes `hi.sect` and `mid.sect`.

Common rules:

- **Read where they lie** (`/net`): places `{sources}` (the NAS's `sources/`; `{chm}` the canopy
  squares) and `{net}` (DEM and land-cover servers' files, laid out as `crate::fetch`'s mirror,
  `<host>/<path>`). A browser reads them through the coordinator (`/work/net/<lease>/nas/<path>`,
  `…/web/<host>/<path>`; `?probe`, `?list`, byte ranges; read-only), 1 MB blocks at a time
  (`web/work/runtime.js`, synchronous). The coordinator fetches only over HTTPS, follows no
  redirect, serves at most 8 MB a request, and answers 503, never "not there", for what it can't
  read now. A native worker reads its own mount, read-only (`Tools::stores_read_only`,
  `SCENIC_STORES_READ_ONLY`).
- **A native worker's transfers** (`coord::client::Files`): a task's files (`/work/in/`,
  `/work/out/`) have no limit on their whole time, so a big file on a slow link goes on while it
  moves. Connecting has 5 s; after that a transfer fails once nothing has moved for 60 s,
  counted from the connection made, the start and end of each write (ureq's output buffer, each
  write itself limited to 60 s) and each read: the wait for the answer's first byte included. The task's log says each fetched file's size, time and rate; its timings give
  the fetch phase's bytes and rate. Its other requests have 15 s in all. A failed fetch isn't
  resumed: the task fails and its job runs it itself.
- **What comes back:** only files the steps changed (hashes of the inputs tell).
- **Memory:** predicted from the files (a tail: three times over, at least 300 MB; each kind has
  its own floor) until a worker's run measures it; then that, plus a tenth.
- **Pace and patience:** the coordinator measures each worker's pace per kind against the job's
  own Mac (`Tasks::note_pace`). A job needing a task nobody took waits up to 30 s
  (`offload::LEASE_WAIT`) only while a taker that beats this Mac by a quarter (`task::beats`), or
  is unmeasured and not tried in the last hour (`EXPLORE_EVERY`), is asking; else it runs it
  itself. A task a worker holds is waited on only while its pace says it will finish first
  (`offload::Patience`); past that it's raced, and a late worker result is compared byte for byte.
  While a job only waits it writes `SCENIC_WAITING` (`offload::WAITING_ENV`) so its agent counts
  its room free (docs/pool.md §7.7). An unmeasured worker's raced task is kept for it to finish
  (`Tasks::measure`) to learn its pace. With placement on (`state/pool/placement`), terrain and
  tree cover z6 tiles offer ahead (`terrain_task::offer_ahead`) with a head start a slower worker may use.
- **Versions:** a task names the job's build; a page fetches that build's WebAssembly programs
  (`/work/prog/<name>.wasm`, shipped in the app's `wasm/`).
- **Determinism:** one maths library (`det`, over `libm`), thread-count-independent reductions, no
  hash-map order in outputs, the real zstd everywhere.
- **Planned:** staging as a task; heavy steps cut into sample ranges; more kinds (map tiles,
  landmarks, slope subtrees); smaller 3D-building z8 tiles.

## 4. Data: the coordinator's plane

- **Built:** packs that staging reads are copied once to the job's Mac's SSD (`blobs/`,
  content-named) and shared by its z6 tiles; the next z6 tile's inputs are copied ahead; task files are
  served from that SSD; one disk budget covers the caches (docs/building.md §2, Room on the disk).
- **Planned:** exact byte ranges fetched; a content-named index per dataset instead of listings;
  writes journaled on the SSD and committed in groups; reads and writes paced by the NAS's latency.

## 5. Control: leases and trust

- **Asking** (`/work/ask`): a worker sends its name, kind, the work it does (pages: only the task
  kinds in `coord::PAGE_TASKS`), memory, cores and (an agent) its app. A NAS-mounting worker gets a
  job first, then a task. An agent on a different app than the lead's gets 409 until they match
  (docs/building.md §2, The same app).
- **Pausing** (docs/building.md §2, Pausing): the coordinator holds `pause.json`; asks are refused, beats carry
  it, and no lease lapses while it holds. Work given back for an interruption isn't held against it.
- **Leases** (`coord::lease`): ten minutes on the coordinator's monotonic clock, renewed by a beat
  each minute; a lapsed lease's work is offered again.
- **Handing back:** a job's saves, done record and costs go back as one hand-off, written by its
  member to the journal and merged by the lead (docs/pool.md §7.3). A task's files go back to the
  broker that offered it; a hand-back for a gone lease is refused (410).
- **Restarts:** token, leases and costs are kept per term (`state/coord/`, docs/pool.md §7.5); a
  worker rereads `state/coordinator.json` when it can't reach the coordinator or its token fails.
- **Failures:** a worker's failed z6 tile isn't offered to it for an hour, doubling; a task out of
  memory goes only to workers sparing more; after two other failures the job runs it.
- **Verification:** a worker's first three results, then one in eight, are checked against the
  job's own run; any difference stops its work.
- **Planned:** NAS outages extending leases; idle workers racing slow ones when the queue runs dry.

## 6. Fitting the work to the worker

- **Predicted peaks:** a z6 tile's `unit` job is its last measured peak (`SCENIC_COSTS`), else about ten times
  its extract, at least 3.7 GB and its floor (docs/pool.md §7.2). A task's: §3, Memory.
- **Ceilings, enforced:** programs import their memory, and the page caps each `WebAssembly.Memory`
  at its task's budget: an overrun is a failed task, not a killed tab.
- **The page's ceiling, learned:** starts from `navigator.deviceMemory`, else 3 GB on an iPad, else
  1 GB; settable on the page; rises a quarter after three tasks near it succeed; a tab that died on
  screen lowers it below that task. On phones and tablets a tab killed in the background lowers
  nothing.
- **Leaving and coming back:** a frozen page resumes within the lease, else drops the task ("taken
  back"). Reloading or closing gives tasks back at once (`pagehide`). No job ever waits on a page.

## 7. The web worker

- **The page** at `/work/` (also the menu bar's panel, on this Mac's own agent's port, and
  `scenic status`'s "Build page") shows the build to anyone the Mac answers, with no key: in the
  pool every Mac's agent serves it, the same, from its own read of the build's state (docs/pool.md
  §10; a Mac with neither a coordinator nor a broker from a viewer that answers nothing else). A
  device helps only when its owner turns on "Help with this tab"; a page may also pause the build
  and ask about the lead (`/work/lead`, docs/pool.md §11): on a Mac not leading, from that Mac's
  own page alone, its agent passing them on. This Mac's own page may also cancel a job and try a
  failed target again (`/work/act`). `/work/?view` only watches. Each slot (a Web Worker per core, less one) runs a
  task's programs over an in-memory filesystem (`web/work/runtime.js`, over browser_wasi_shim).
- **The build at a glance** (`web/work/dash.js`, `dash.css`, `pool.js`, `sums.js`; docs/pipeline.md §5): a
  sticky header (the verdict: going, done when, what needs a look; Pause, Freeze, Resume; the
  tabs), a tab per part by the address's `#hash`, and a fixed footer (the charts' range, the Live
  dot). Overview: the overview (its map figure from the last catalogue) and the road to done
  (each machine's forecast lane); Stages (a card per stage, a row per step); Machines (a card per
  machine, the pool: who leads, "Make lead", "Take it"); Regions; Activity (last day by the hour,
  events); Details (leases, waits, each Mac's capacity, workers, the steps table); This device. It reads
  `/work/steps` once (the steps table: every word about a step), `/work/swarm` every 10 s and
  `/work/history` incrementally; `/work/regions` (each region's folders, download size and
  outline: docs/pool.md §10, Regions) when the last catalogue changes, else every ten minutes;
  each Mac's measures over time (on the lead also the NAS's disks and the pool's NAS traffic), and its internet's counts by host and last transfers, from its own port's `/work/live` (docs/pool.md §10, Live);
  each running job's phase, target and NAS and internet bytes in the status (`jobs[].live`, and a
  member's on its lease, from its beat: the same section). The
  regions' outlines and the countries' (`countries.json`, one of the page's files) are for the
  overview's map (task #513), not drawn yet. A 503 (the Mac hasn't read the build's state yet) says so quietly and asks
  again every 2 s, what's shown said to be stale if it had an answer before, an error only after a
  minute of them; a new `epoch` (the
  Mac's agent restarted) starts the events again. "Since you last looked" goes by the events' time,
  so it outlives a restart.
- **HTTPS** through `tailscale serve` (docs/pool.md §14), or the M6's
  https://map.brandontsang.net/build (Caddy, docs/macs.md), gives the secure context the wake lock
  and PWA need; plain HTTP on the tailnet still works. The page's files and requests are relative
  to where it was served (the path before `/work/`), so it works under a proxy's path.
- **The page as an app** (`manifest.webmanifest`, `sw.js`, `icons/`): installable; its service
  worker caches the page's files and the programs' WebAssembly by version (`?v=`), takes a new
  page when the coordinator's hash changes, reloading once no task runs. It works only while open
  on screen.
- **Planned:** task files in OPFS so a worker holds less in memory.

## 8. Native workers

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
  A page carries none and may only take page tasks (under "page <name>", at most 4 GB) and pause
  the build. A running job's requests come from the lead itself, through no proxy (`crate::net::own`).
- A worker is served only its task's files and uploads only into its folder; bodies, connections,
  timeouts and names are bounded; lease ids are never reused.
- **Known limits:** by the owner's choice, any device on the LAN or tailnet can take page tasks
  (results verified, §5) or pause the build.
- Licensed data on the owner's own devices isn't redistribution (docs/storage.md).
