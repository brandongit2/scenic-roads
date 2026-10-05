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
  - **Jobs** of the plan (units), for workers that mount the NAS (the M1's agent). A job saves into
    the store's content-named files as before; its record changes come back as one hand-off.
  - **Tasks** (`coord::task`), pure work a running job offers to any worker: programs run over a
    folder of files, giving files back. A task never needs the NAS: what it reads was staged on the
    build Mac, which serves it.
- **Impure work stays at the hub:** fetching outside data (DEM servers, WorldCover, AWS, Meta's
  canopy), sampling DEMs (whose per-vertex cache is mutable), staging from packs, the scenic
  carry-over between runs, and every write to the records.
- **Workers** hold nothing the build depends on: a lost worker costs only its work in hand.

## 3. Tasks (built for a unit's tail)

- **Where the work splits:** a unit's tail (`unit::tail`) runs its clean-up and grade, prep and
  canopy steps on the build Mac, by the canopy files (gigabytes per 10° square: fetched there once,
  §4). View, buildings and flags read only the unit's own files, and each says which (`Run::reads`,
  traced in WebAssembly by `tools/check/reads.mjs`: 574 of the folder's 1,064 MB for 6/20/22), so
  any worker may run them (`unit::split`).
- **The unit job** (`pipeline::offload`) clones those files into the task's folder (copy-on-write,
  instant; the roadside buildings' links to the NAS stay links) and offers the task when a worker
  that takes tails is around and fewer than such are out (at most three: each holds a unit's folder
  on the build Mac's disk), then prepares the next unit. Tails back from workers are committed
  between units.
- **Nothing waits on a worker:** at the end of the job, a task no one took is taken back and run on
  the build Mac, and one a worker still holds is raced there (the build Mac's result counts; a
  worker's that comes in too is compared). With no worker around, a unit builds as before.
- **Source versions:** a task names the programs' build (the job's binary); a web worker fetches
  that build's WebAssembly programs from the coordinator (`/work/prog/<name>.wasm`, shipped in the
  app's `wasm/`). The native and WebAssembly builds of one source give the same bytes (§10).
- **Determinism rules:** one maths library (`det`, over `libm`) on every target; reductions that
  don't depend on the thread count; no hash-map order in outputs; the real zstd everywhere.
- **Planned:** more of a unit as tasks once the Python steps' ports are switched on (`elev`,
  `landcover`, `areaflags`: built, the same bytes as the scripts, the units still run the scripts;
  sampling needs the DEM ranges it reads listed ahead, §4); the heavy steps cut into sample ranges
  so a slow worker's lease is minutes; more kinds of task (map tiles, landmarks, slope, terrain,
  tree cover).

## 4. Data: the coordinator's plane

- **Built:** the packs staging reads are copied from the NAS once to the build Mac's SSD (`blobs/`,
  content-named, so a copy is always right) and shared by its units; the next unit's piece, packs
  and canopy squares are copied ahead while the current one builds; a task's files are served to
  its worker from the build Mac's SSD, so another worker costs the NAS nothing; one disk budget for
  the caches (plan §8, Room on the disk).
- **Planned:**
  - **Ranges:** the exact byte ranges a task reads (DEM tiles, canopy strips, pack entries), so only
    those are fetched and sent.
  - **No listings:** a content-named index per dataset (canopy squares, raw tiles, FABDEM, kept
    samples), read from one file instead of found by listing or probing.
  - **Writes, journaled then committed together:** outputs on the SSD, copied to the NAS whole, and
    applied to the records in groups; `pending.json` on the SSD; small files packed.
  - **Paced by latency:** background reads and writes paced by the latency the NAS shows.

## 5. Control: leases and trust (built)

- **Asking:** a worker sends its name, kind, the work it does (`unit`, `tail`), the memory it spares
  and its cores (`/work/ask`). One that mounts the NAS is given a unit first (the most work for
  what it fetches), then a task; a web page, tasks.
- **Leases** (`coord::lease`) are timed on the coordinator's monotonic clock (clocks between devices
  don't matter), ten minutes, renewed by a beat each minute while the work goes on. A paused job
  doesn't beat, so its work may go to another; the build Mac's own jobs hold leases too, and one
  that lapsed is taken again when no one took its work meanwhile. A lapsed lease's work is offered
  again.
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
  `SCENIC_COSTS`), else about ten times its piece, never under 3.7 GB. A tail's is its files twice
  (a web worker holds them, and a program reads them in) and 500 MB, raised to what a worker
  measured for that unit's last time (6/20/22: 1,608 MB predicted, 1,603 measured).
- **Ceilings, enforced:** the programs are linked to import their memory, and the page gives each a
  `WebAssembly.Memory` capped at its task's budget less the files it holds: an overrun is a failed
  task, reported with the peak it reached, not a tab the OS kills.
- **The page's ceiling, learned:** it starts from what the browser says it has (or 1 GB), never more
  than the largest memory the browser will create; after three tasks near it succeed it rises by a
  quarter; a task the tab died in lowers it below that task's (the page notes what it runs, and
  finds the note when it's reloaded). Nothing asks what device it is.
- **Planned:** cutting a task to the worker (smaller sample ranges for smaller ceilings).

## 7. The web worker (built, but HTTPS)

- **The page** is served by the coordinator at `/work/`; its address carries the token in its
  fragment (never sent to a server), and `scenic status` prints it. Its main thread asks for tasks
  that fit the memory the tab spares and beats for every lease; each slot (a Web Worker per core,
  less one) runs a task's programs over an in-memory filesystem (`web/work/runtime.js`, over
  browser_wasi_shim) and sends back what they wrote.
- **HTTPS:** the screen wake lock (and OPFS, below) need a secure context. Plain HTTP on the tailnet
  works (WireGuard encrypts it), but the device must be kept awake by hand. The coordinator can be
  reached over HTTPS through `tailscale serve` (a certificate for the build Mac's tailnet name):
  waiting for the owner's go-ahead, as it changes the Mac's network settings.
- **Planned:** inputs and outputs in OPFS, read through a `FileSystemSyncAccessHandle`, so a worker
  holds less in memory and a reload resumes the upload, not the work.

## 8. Native workers (built)

- **The M1** asks for units and tails over HTTP; a tail runs as `scenic run-task` (its files fetched
  from the coordinator, its steps run natively, the files they wrote sent back).
- **Planned:** the build Mac's own work as tasks too.

## 9. Security

- **Built:** the coordinator answers this Mac, its LAN and the tailnet only; every request but the
  page's carries the token (128 random bits, kept on the build Mac and in the contact on the NAS); a
  running job's requests come from this Mac only; a worker is served only the files of the task it
  holds, and uploads only into that task's folder; request bodies are capped.
- **Planned:** per-device credentials that can be revoked, exchanged for a pairing token.
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

## 11. Steps

1. **Measure and check:** a unit's wall time part by part; the determinism check. *Done.*
2. **The data plane under today's jobs:** the SSD copies, prefetching the next unit. *Done;* journaled
   group commits, dataset indexes, latency pacing *planned.*
3. **The M1 through HTTP leases.** *Done;* claims and NAS hand-off files kept one release for a
   helper on an older app, then retired.
4. **Units fan out:** a unit's last steps to any worker. *Done;* sample ranges, chunked canopy,
   ranged reads *planned.*
5. **Browsers:** the page, imported-memory ceilings, ramped verification. *Done;* HTTPS through
   Tailscale and OPFS *planned.*
6. **The Python steps in Rust:** *built* (same bytes), the units' switch to them *planned;* then more
   kinds of task, and the build Mac's own work as tasks.
