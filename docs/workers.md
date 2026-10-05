# Builds anywhere: one coordinator, its data plane, any number of workers

Status: **planned** (design of 2026-10-04, revised after review; the pilot in §10 is done). This
extends plan §8 (Building): today the build Mac's agent plans and runs every job, and the M1's
helper builds units through claim and hand-off files on the NAS. Here any device that opens a page
(a Mac, the iPad, a phone) can take work, the M1's helper speaks the same protocol, and the build
Mac's own I/O gets the same care.

## 1. Goals, and where the time goes

- **Any device, by opening a page.** No install. Up to about five workers.
- **One code path.** No device special cases: a worker has a memory ceiling it learns, and takes the
  work that fits; a phone simply ends up with smaller pieces.
- **Kind to the NAS.** The NAS (Btrfs RAID5 on spinning disks, 3.8 GB of RAM, SMB) is the slowest
  part: listings took 3–33 s per folder under load on 2026-10-04; small-file writes run at ~25 a
  second. Every byte should leave it once per need, in large reads, and arrive in large writes.
- **Resilient by construction.** Workers vanish (a tab closed, a phone asleep, Wi-Fi gone), the
  coordinator restarts, the NAS goes away, a browser miscompiles: none of it may corrupt the build.
- **Same bytes everywhere** (plan §8, Determinism): a task gives identical output on every worker,
  at any thread count and however its work is cut. Results can then be cached, re-run anywhere and
  checked by running them twice.

**Measured before designing:** in the build Mac's dense units of 2026-10-04, the Rust steps were 18%
of a unit's wall time, elevation sampling (`sample.py`) 39%, and 42% wasn't attributed (staging from
packs, the DEM cache slice, copies to and from the NAS). Distributing the Rust steps alone would make
units ~1.2× faster. So the data plane comes first, and pays off on the Macs before any browser joins.

## 2. The model

**Pure tasks over a content-addressed store; impure work at the coordinator.**

- **Blob:** an immutable file named by its content (the store's content names, plan §3). Never
  changed, so every copy anywhere is valid forever and no cache needs invalidating.
- **Task:** a deterministic computation: a step at a *source version*, its arguments, and its inputs
  (blobs or byte ranges of them, mounted at paths); it produces outputs at paths. Its key hashes
  the step's version and the inputs' content names: the native and WebAssembly builds of one source
  version are the same program (§3), so the same work has one key wherever it runs.
- **Impure work stays at the coordinator:** fetching outside data (DEM servers, WorldCover, AWS,
  Meta), sampling DEMs (whose per-vertex cache is mutable), and per-unit state carried between runs
  (the scenic carry-over). Each becomes an *I/O step* on the coordinator whose outputs are
  content-named blobs, inputs to the pure tasks after it; checking a task replays it with those same
  inputs.
- **Coordinator:** the build Mac's agent: the planner (as now), the data plane (§4), the scheduler
  of tasks onto workers (§5), and the one writer of the records (as now), which it keeps in memory.
- **Workers:** anything that runs tasks: the M1, a browser tab, eventually the build Mac's own
  executor. A worker never sees the NAS or a record, and holds nothing the build depends on.

## 3. Tasks

- **Jobs stay, tasks come from them.** The planner and its job keys (`jobs.json`) are unchanged: an
  app publish doesn't rebuild anything. A job fans its compute out as tasks, and commits when all
  of them are in.
- **Source versions:** each step carries its version (as `UNIT_V` and the rest do now); a task names
  the step version, and a worker runs the build of that version for its platform (native, or a
  content-named `.wasm` it fetches and caches). The determinism harness (§10) is what makes the two
  builds one program: every step's native and WebAssembly outputs compared byte for byte, at 1 and
  many threads, and however the work is cut.
- **Determinism rules:** one maths library (`det`, over `libm`) on every target; reductions that
  don't depend on the thread count or the cut; no hash-map order in outputs; the real zstd
  everywhere; NaNs canonicalised before they're written (WebAssembly leaves their bits open).
- **Cut to minutes.** A WebAssembly task is single-threaded, so a whole dense unit would hold a lease
  for tens of minutes, exposed to screen locks and tab discards. The heavy per-sample steps (canopy,
  view, buildings) run over ranges of samples, merged in order; canopy decodes its band of rows in
  chunks. Any cut gives the same bytes.

## 4. Data: the coordinator's plane

- **Tiers:** a task's memory; the worker's cache (OPFS in a browser, its own disk on a Mac); the build
  Mac's SSD; the NAS. Every tier holds immutable blobs, so a hit is always right.
- **One SSD budget.** The coordinator's cache, the mirror (published blobs are served from it, not a
  third copy) and room-making's caches share one budget (plan §8, Room on the disk). Outputs not yet
  on the NAS are pinned, never evicted; past a high-water mark of them, writes get a guaranteed
  share of the NAS and no new leases go out until they drain.
- **Read once from the NAS.** Concurrent asks for a blob wait on one read (single flight), kept on
  the SSD for the next task. The next unit's inputs are prefetched while the current one computes.
- **Ranges.** Resolve knows the exact byte ranges a task reads (canopy strips, pack entries), so only
  those are fetched; call sites read ranges, not whole files (a whole-file read in WebAssembly is
  memory, and wasm32 tops out at 4 GB: offsets stay `u64`).
- **No listings.** The coordinator is the only writer and keeps the records in memory: no re-reading
  the manifest, no listing NAS folders each loop. Downloads (canopy squares, raw tiles, FABDEM) and
  kept samples get a content-named index per dataset, so what's there is read from one file, not
  found by listing or probing.
- **Writes: journal, then group commit.** An output lands on the SSD (verified against its hash), is
  copied to the NAS whole and flushed, and is recorded in a journal on the SSD; once a minute the
  journal is applied to the records in the order blob → manifest → keys (the order `handoff::merge`
  keeps), a task's outputs and removals together. `pending.json` (uploads awaiting their read-back
  check) moves to the SSD. Small blobs are packed into larger files; intermediates never leave the
  SSD.
- **Paced by latency.** Every coordinator NAS read and write goes through `store::iopool`, and
  background work (prefetch, write-behind) is paced by the latency the NAS shows, not by guessing
  when it's idle.

## 5. Control: leases and trust

- **Hello:** a worker introduces itself with a device credential (§9), its kind, cores and memory
  ceiling (§6). After a coordinator restart, the tasks workers say they hold are re-adopted.
- **Leases** are timed on the coordinator's monotonic clock (clocks between devices don't matter),
  renewed by heartbeats that report progress (a worker thread sends them, so a busy task can't
  starve them), and only while progress is made. A lapsed lease returns the task to the queue; a
  late result is still taken if it checks out. A NAS outage extends every lease rather than
  expiring them; the Mac stays awake (`caffeinate`) while leases are out.
- **Assignment:** a task goes to a worker whose free ceiling fits its predicted peak (§6), keeping a
  unit's tasks on one worker where its inputs already are. When the queue runs dry, a task a slow
  worker holds is also given to an idle one; the first result wins (the bytes are the same).
- **Verification, ramped:** every task of a new worker, program build, shim or memory ceiling is run
  again elsewhere and compared; then about one in ten. Which worker made each blob is recorded with
  its trust (checks passed), quarantine and failure counts, all persisted. A mismatch is settled by
  a native run; the loser's unverified outputs, and everything built from them, are made again.
- **Failures:** a task failing on two workers fails for real (the job's retry rule, plan §8); a
  worker failing what others finish stops getting that kind of task.
- **Regression accepted:** the M1 builds only while the build Mac is up (today's claim files let it
  go on alone): one protocol instead of two.

## 6. Fitting the work to the worker

- **Ceilings, enforced:** a WebAssembly task gets a `WebAssembly.Memory` (the programs are linked to
  import it) whose maximum is its budget, so an overrun is a trap the page catches, not a tab the OS
  kills. The page learns only its own overall ceiling: it starts low, grows on measured peaks, and
  keeps a quarter under any point where it was killed; a kill while hidden doesn't count (iOS
  suspends hidden pages anyway).
- **Predicted peaks:** a task's peak is predicted from the same target's last run (native units
  peaked at 3.5–5.5 GB whether they had 687 ways or 1.4 million: size alone predicts little).
- **Cuts:** a worker with a small ceiling gets small sample ranges; one with a large ceiling gets
  bigger ones (fewer round trips). Nothing is told what kind of device it is.

## 7. The web worker

- **HTTPS:** OPFS and the screen wake lock need a secure context, and `http://` on the LAN isn't one.
  The coordinator is reached through `tailscale serve` (a certificate for the build Mac's tailnet
  name; each device runs Tailscale), or a DNS-01 certificate for a domain of the owner's.
- **Inputs:** fetched by the exact ranges resolve lists into a per-task file in OPFS, then read by
  the WASI shim through a `FileSystemSyncAccessHandle` (synchronous inside a worker: no
  SharedArrayBuffer, no cross-origin isolation, and a network stall never blocks the compute).
  Outputs go to OPFS too, so a reload resumes the upload, not the work.
- **Storage that can vanish:** OPFS sized from `navigator.storage.estimate()`; eviction expected
  (Safari clears a site's storage after seven days unvisited).
- **Awake:** a wake lock while it has work. Hidden, iOS suspends the page and releases the lock: the
  page asks to stay in front, and its leases run out if it doesn't.

## 8. Native workers

- **The M1** takes the same leases over HTTP, replacing the claim and hand-off files (plan §8).
- **The build Mac's own executor** moves onto tasks last; until then its jobs run as now, over the
  new data plane.

## 9. Security

- The coordinator answers only on the tailnet or LAN, checks `Host`, and serves a worker only the
  inputs of the leases it holds: never `inputs/keys.env`, never `state/`.
- Joining: a link with a pairing token in its URL fragment (so it's never sent in a request or
  logged), exchanged for a per-device credential that can be revoked.
- Licensed data on the owner's own devices isn't redistribution (plan §3, sources' terms).

## 10. The pilot (2026-10-04)

On unit 6/20/22 (86,904 roads), the unit's Rust programs compiled to WebAssembly gave
byte-identical outputs in V8 (Chromium, Node) and JavaScriptCore (Safari's engine), at 1.45× and
1.24× native time on one thread, once the transcendental functions came from one library. Natively
on 1 and 14 threads and under WASI, every step matched. A page in Chromium with three Web Workers
took jobs from a small coordinator: 15 of 15 identical. Peak memory: canopy 3.5 GB (whole canopy
files read, full-width bands decoded), view 1 GB, the rest under 600 MB; a dense unit's inputs are
0.4–0.6 GB compressed. A GPU version of the view kernel ran 15× faster than 14 CPU threads (WebGPU
too), but the view step is ~0.4% of a unit's time and nothing else is GPU-bound: not pursued.

## 11. Steps to get there

1. **Measure and check:** every part of a unit's wall time logged; the determinism harness (each
   step natively and under WASI, at 1 and 14 threads, cut different ways, byte for byte). *Done in
   part: the crates build for WebAssembly; one maths library everywhere.*
2. **The data plane under today's jobs:** the SSD tier and one budget, prefetching the next unit,
   journaled group commits, records in memory, dataset indexes instead of listings, latency pacing.
3. **The M1 through HTTP leases;** claims and hand-off files retired.
4. **Units fan out:** canopy, view and buildings over sample ranges, chunked canopy, ranged reads.
   The records don't change.
5. **Browsers,** verified fully at first: the page, HTTPS through Tailscale, OPFS inputs and
   outputs, imported-memory ceilings.
6. **The Python steps in Rust** (elevations as the coordinator's I/O step; area flags and land
   cover as tasks), then more kinds of task (map tiles, landmarks, slope, terrain, tree cover), and
   the build Mac's own executor last.
