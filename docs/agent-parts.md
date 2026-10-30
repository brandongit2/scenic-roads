# The agent in parts

**Planned (task #871; only the first slice is built).** This doc says how the build agent's single
serial loop (`crate::agent::Agent::run` and `step`, crates/pipeline/src/agent/mod.rs) will become
independent parts, each split by what it waits on. Until each step below lands, docs/pool.md §7.8
and §11, docs/building.md §2 and the code are what is true. The parts marked *today* describe main
at e3fa1d53.

## 1. The problem

*Today* one thread runs every duty in order (`step`'s `nasread::phase` names), then waits 2 or 20 s:

| Phase | Waits on | Seen |
|---|---|---|
| the NAS's mount, heal | the kernel's SMB mount and probe (no deadline) | 70 s – 17 min stalls |
| this Mac's settings | local files | fast |
| the pool's switch, live view | NAS switches (`nasread`, 20 s) | |
| the pause and hold (`sync_pause`, `sync_hold`) | NAS switches, local asks | a Pause ~5 min to show |
| the running jobs (`tend`, cancels, guards) | child processes, local disk | fast |
| the pool's step, the coordinator's contact, the outbox | the NAS (records, mail, terms) | |
| the gate, the plan, the grants (lead) | the NAS records, the CPU | plan 15–100+ s |
| the caches | local disk (already a thread) | |
| the checklist and counts | the manifest and outlines on the NAS, the CPU | 10–105 s a pass on the M4 (30 Oct) |
| the heartbeat | the NAS write (`nasread` Writes pool) | |

So a fast urgent duty (pause, resume, heartbeat, status, a cancel) waits behind every slow one. The
watchdog's kept beat (§7.8, `nasread::BEATING`, `KEEP_EVERY`) papers over the heartbeat's part of
this: it rewrites the last content with a new time. It is a symptom fix; this design removes the
cause, and with it the kept beat.

Already off the loop *today*: the forecast (`forecast`), the phase sums, region sync, the members'
upcoming hints, the memory sampler (`memguard`), the live measures (`live`), the pool driver's
ticks (`pool::Ticker`), the caches' clears (`caches_task`), the built counts (`built_making`), and
this Mac's own hold asks (`asks`, #846). Each is a thread with its own ad-hoc sharing (an `Arc`
of `Mutex`es, an atomic, a `JoinHandle` polled). The design gives them one shape.

## 2. The parts

Six parts, each a thread (or a small pool) owning its state outright. They share nothing mutable;
they talk only by the named channels and snapshots of §3.

| Part | Owns | Waits on | Period / trigger |
|---|---|---|---|
| **control** | this Mac's hold, the quit ask, cancels, retries, redos, the owner's settings (room target, in-use share, build memory) | the agent's HTTP endpoint and a watcher on the agent's folder | each request, at once |
| **beat** | the status document and its writes (local copy, NAS / lead) | a steady timer, and `Changed` events | every 10 s, and at once on a state change |
| **nas** | the mount, its health (`heal`), the three `nasread` pools | the share, under deadlines | requests from other parts; mount probe every 20 s |
| **jobs** | the slots, their child processes, leases held, the memory and disk guards | children's exits, control's orders, grants | each child event, and every 2 s |
| **plan** (lead only) | the plan, the offers, the grants, the gate, the checklist and counts | the records snapshot, the CPU | triggered by changes (§4.4); a 5-min fallback |
| **pool** (until #878) | the pool driver: terms, mail, member heartbeats | the NAS, through **nas** | its 1-s tick (`pool::Ticker`, today) |

The existing side threads (forecast, phase sums, region sync, hints, sampler, live, caches) become
workers of **plan** or **jobs** with the same in/out shape: a request in, a snapshot out.

`run` keeps only: start the parts, wait for `STOP`, a quit that is due, an update that is due or a
restart (`restart_now`), then stop them in order (§5.3).

## 3. Sharing: channels and snapshots

Two kinds of link, both named types in a new `agent::bus` module:

- **Channels** (`std::sync::mpsc`, one consumer): orders and events. Each carries a typed message:
  `ControlMsg` (hold set, quit, cancel, retry, setting changed) into **jobs** and **beat**;
  `JobEvent` (started, ended, paused, given back) into **plan** and **beat**; `Grant` from **plan**
  into **jobs**; `NasReq` into **nas**, answered on a reply channel with a deadline.
- **Snapshots** (`bus::Snap<T>`: an `Arc<T>` swapped whole under a lock held only for the swap, and
  a generation counter): state one part writes and others read. `Conditions` (power, NAS up, home,
  idle; written by **nas** and a power sampler), `JobsView` (the slots as **jobs** last saw them),
  `HoldView` (from **control**), `PlanView` (offers, regions, checklist; from **plan**), `PoolView`
  (from **pool**). A reader takes the `Arc` and works on it with no lock held; nothing reads a
  half-written state.

The rule: a part's own state is `&mut` inside its thread; anything another part needs it publishes
as a snapshot or sends as a message. No `Mutex` on shared state outside `bus`. A test
(`agent::bus::tests::no_shared_mutex`, a scan of `agent/` for `Mutex<` outside `bus` and `nasread`'s
own pools) holds the rule.

## 4. Each part

### 4.1 control

**Takes requests** from two sources, both taken up at once:

1. The agent's HTTP endpoint: the coordinator's / broker's / viewer's server already serves
   `/work/act`, `/work/cancel`, `/work/redo` and `/work/settings`; it gains this Mac's hold and quit
   and posts each to **control**'s channel instead of writing a file for the loop to read. The menu
   bar and `scenic pause --here` post to it over localhost.
2. A file watcher (FSEvents on the agent's folder, kqueue fallback) for the file asks
   (`crate::control::request`, `request_cancel`, `cancel-requests/`, the settings files), for older
   CLIs and the case where the endpoint isn't up. Each event reads the file once. A 2-s stat sweep
   stays as the watcher's backstop (FSEvents can coalesce or drop).

This replaces `asks`' 250-ms poll and the loop's `sync_hold`, `adopt_hold`, `hold_asked_anew`,
`quit_ask` polls, `tend_cancels`' folder reads and the settings block at the top of `step`.

**Owns:** the hold (`HoldState`, the later `at` winning, as today), the quit ask, settings. It
answers each ask (`control::answer_hold`) as soon as it is applied, and sends `ControlMsg` to
**jobs** (drain / resume / stop / cancel) and `Changed` to **beat**.

**Another member's hold** (`state/pool/holds/<member>.json`) is read through **nas**'s Switches pool
by **pool** today, and becomes an HTTP call to the lead with #878 (§6).

**Failure:** a request it can't parse is answered with the error at once (HTTP 400, or the answer
file). It never touches the NAS itself, so it can't hang on one. The page and menu see the answer
within a second; the hold's stage (asked, pausing, paused, resuming) comes from **beat**.

### 4.2 beat

**Owns** the status document (`Status`) and its writing. It composes it from the latest snapshots
(`Conditions`, `JobsView`, `HoldView`, `PlanView`, `PoolView`), each with its own age, and writes it:

- every 10 s on a steady timer, whatever the others are doing;
- at once (debounced 200 ms) on a `Changed` from **control** or a `JobEvent`.

The local copy is written directly (this Mac's disk). The shared copy goes through **nas**'s Writes
pool under its deadline (`state/status.json` today; to the lead by HTTP with #878).

**The BEATING rule, kept as an ordering by construction.** Today `nasread::BEATING` is a lock the
loop holds from its `say` to its writes, so the asks thread's or the watchdog's rewrite never lands
after a newer one. In parts, **beat** is the only writer of the status: one thread, writes in
order, each a whole document from snapshots at least as new as the last. A later state can't be
overwritten by an earlier one because there is no second writer. The hold's say (`Status::say`)
reads `HoldView`, which **control** publishes before it sends `Changed`, so a beat triggered by an
ask always carries it.

**Staleness instead of a kept beat.** Each section of the status carries its snapshot's age
(`plan_age_s`, `jobs_age_s`, `nas_age_s`). The heartbeat's time says the agent is alive; a section's
age says that part is slow. The page and menu show a part whose snapshot is older than its period
×3 as "planning slow (2 min)", "NAS not answering (since 14:02)", rather than the whole Mac going
out of touch. `OUT_OF_TOUCH_S` keeps meaning what it says: **beat** itself hasn't written.

**Failure:** a shared write that times out is logged once (as `nasread` does) and the next tick
tries again; the local copy and the endpoint's `/status` are always current. A Mac whose shared
writes fail shows on the lead's page as "status not reaching the NAS" from the beat's age there.

### 4.3 nas

**Owns** the mount (`root`, `find_mount`, the LAN remount, `heal_step`, `last_mount_try`) and the
`nasread` pools (Switches, Records, Writes). The mount and probe, which have no deadline today, run
on a worker of their own: the mount call is started and waited for 30 s; past it the worker is left
to the kernel (as `nasread` leaves a stuck thread) and `Conditions.nas` says "mounting (stuck
since …)". Only work that needs the NAS waits: `Needs { nas: true }` jobs don't start and are
paused per today's rules, **plan** waits for records, **pool** waits; control, beat and local jobs
go on.

**Moved under deadlines in this part** (pool.md §7.8's "still under no deadline" list): the mount
and probe, `build::Keys::load`/`load_with`, `out::read_record`, `inputs::read_report`, the helpers'
listing, the coordinator's contact publish/unpublish, GC's reads. Each becomes a `NasReq` with a
deadline sized by kind (a listing 60 s, a big read 5 min). Their callers are already on **plan** or
**pool**, so a timeout costs only that part a pass.

**Failure:** shown as `Conditions.nas` (`up`, `slow`, `stuck since`, `healing`) on the page's
machine card and the menu's NAS line.

### 4.4 plan (the lead's)

**Owns** the plan (`planned`), offers to the coordinator (`offer_plan`), the grants
(`lead_grants`), the gate (`tend_gate`), redos, the checklist and counts (`checklist`,
`built_making`), and its workers (forecast, phase sums, region sync, hints).

**Triggered by changes, not passes.** It runs when one of these arrives and coalesces bursts
(at most one plan running, one queued):

- a `JobEvent::Ended` (any member's, through the coordinator or **pool**);
- the records' generation changed (the journal's merge, `plan_seen`'s inputs);
- an input or region edit's hold ended (`edit_held`), a gate ask, a redo;
- a timed wait in the plan ran out (`plan_due`'s timers become one deadline the part sleeps to);
- a 5-min fallback.

Granting is fast and separate from planning: a free slot's ask (from **jobs** here, or a member's
by HTTP) is answered from the last `PlanView` at once, not after the next plan. Today's `may_pack`
and `try_pooled` collapse into that.

**Readiness (#879).** When docs/readiness.md lands, **plan** calls its one readiness function over
the records snapshot; the forecast and the page read the same function's answer from `PlanView`
instead of their own copies. This design adds no readiness logic of its own.

**Failure:** a plan that errs keeps the last `PlanView` (its age grows; the page says "plan
failed at 14:05: <error>", which `planned`'s error says today in the log only). A plan past 5 min is
logged with its phase (the watchdog's sample, kept for this part only). Grants go on from the last
plan meanwhile.

### 4.5 jobs

**Owns** the slots (`slots`, `Slot`, records, `save_hands`, `adopt_jobs`, `hand_over_jobs`), the
child processes, their leases (`keep_own`), cancels, the ramp, and the memory and disk guards.

It wakes on a child's exit (a reaper thread sends `JobEvent`), a `ControlMsg`, a `Grant`, a
`Conditions` change (power, NAS) and a 2-s timer for the guards. A drain is sent to a job's channel
the moment the hold arrives (as `asks` does today), and the job's stop and give-back follow as
today's `tend`.

**Failure:** a job's own failures are unchanged (backoff, retries). A give-back whose hand-off
needs the NAS is queued in **nas** (the outbox, `held_entries`) and retried; the slot is free for
local work meanwhile.

### 4.6 pool (until #878)

The pool driver keeps its thread (`pool::Ticker`) but reads `Conditions` and `HoldView` snapshots
instead of the agent's fields, and does its NAS calls through **nas**. Its gates (`pool_step`'s
`gates`: leads, settle, duties) become `PoolView`, which **plan** and **jobs** read.

## 5. Ordering guarantees

1. **One status writer** (§4.2): no beat lands over a newer one, here or on the NAS. Replaces
   `BEATING`, `beat_again`, the watchdog's kept beat.
2. **A hold is applied before it is said.** **control** sends the drain to **jobs** and publishes
   `HoldView` before `Changed`; the beat that says "pausing" is written after the jobs were told.
3. **Nothing starts under a hold.** **jobs** checks `HoldView` (its newest, read at the start) before
   it starts a granted job, and a hold arriving during a start drains that job at once.
4. **Grants follow the plan they came from.** A `Grant` carries its `PlanView` generation; **jobs**
   drops a grant older than a cancel of the same target.
5. **A part change stops beats first.** When this Mac's part changes (lead → member, or a restart),
   **beat** is told first and writes no more shared status under the old part (pool.md §7.6's rule,
   today enforced by clearing `BEATING`).
6. **Shutdown order:** control stops taking asks; jobs hand over or stop; plan, pool stop (60 s
   max, as today's `ticker.stop`); beat writes its last; nas's workers are left.

## 6. Planned changes this accounts for

- **#881 (the mini as fixed lead, no election).** **plan** runs only on the mini; on the M1 and M4
  it never starts. `pool_step`'s gates (`settle`, `leads`, re-assert) and the handover and takeover
  machinery in **pool** go with #881, not here; this design doesn't move them first. Step 6 below is
  sized on #881 having landed or not.
- **#878 (pool coordination by HTTP on the lead).** When it lands, **pool** shrinks to an HTTP
  client: a member's heartbeat, mail and holds are posts to the lead's endpoint, and **beat**'s
  shared write goes there too. **nas** then serves only the jobs' and the plan's data reads. The
  channels and snapshots don't change: `PoolView` is filled from HTTP answers instead of NAS reads.
- **docs/readiness.md (#879)**: §4.4; **plan** owns calling it, nothing else.

## 7. Migration

Each step is shippable alone, keeps today's behaviour for what it doesn't touch, and ends with its
test. The tests use the agent's existing fakes (`o.root` on a temp dir, `cond::Mac` fakes) plus a
**slow NAS fake**: a `nasread` hook that makes every NAS call (or one kind) block until released,
and a **slow plan fake** that makes `build::plan` sleep. Both live in `agent::tests` and exist only
in tests.

| # | Step | Deletes | Test that proves it |
|---|---|---|---|
| 0 | *Done (#846):* own hold asks on `asks`' thread | — | `agent::asks::tests` |
| 1 | `agent::bus` (`Snap`, the channel types), and the slow-NAS and slow-plan fakes | — | the fakes' own tests |
| 2 | **beat** on its own timer, sole status writer; the loop publishes `JobsView`/`PlanView` snapshots instead of writing | `nasread::BEATING`, `Beating`, `beat_again`, `asks::Beat`, the watchdog's kept beat (`KEEP_EVERY`, `KEEP_FOR`, `WORKING_S`), the heartbeat phase of `step` | `beat_steady_while_the_nas_hangs`: the NAS fake blocks every call; the local status' time advances every 10 s for 2 min. `beat_steady_while_planning`: the plan fake sleeps 3 min; same |
| 3 | **control** on the HTTP endpoint and a watcher; hold, quit, cancel, settings | `asks`' poll (`EVERY`), `sync_hold`, `adopt_hold`, `hold_asked_anew`, `quit_ask` polls in `run`'s wait, `tend_cancels`' folder read, the settings block in `step` | `pause_shows_within_a_second_on_a_hung_nas` and `…_while_planning`: a hold posted; its answer and a "pausing" status within 1 s, the job's channel says drain |
| 4 | **nas**: the mount and probe on a worker with a deadline; the §4.3 calls as `NasReq` | the "still under no deadline" list in pool.md §7.8; the mount block in `step` | `a_hung_mount_holds_only_nas_jobs`: the mount fake hangs; a local job starts, the beat and a pause go on, a NAS job waits with "NAS mounting (stuck)" |
| 5 | **jobs** as its own part; the reaper thread; grants by channel | the slot loop in `step`, `run`'s wait loop and its 2/20-s timing, `save_hands` per round | `a_job_end_starts_the_next_within_2_s`; the guards' tests moved |
| 6 | **plan** triggered by changes; grants from the last `PlanView`; the checklist and side threads as its workers | `step` itself, `plan_due`, `plan_seen`, `may_pack`, `try_pooled`, `built_making`'s polling, the loop watchdog (`nasread::loop_thread`, `ROUND_AT`, `loop-stall.json`; the per-part stall log replaces it) | `a_slow_plan_delays_no_grant`: the plan fake sleeps 2 min; a member's ask is granted from the last plan within 1 s |
| 7 | the no-shared-mutex scan test; docs: pool.md §7.8 and §11, building.md §2 rewritten to the parts | the rest of the ad-hoc `Arc<Mutex<…>>` fields on `Agent` | `agent::bus::tests::no_shared_mutex` |

Steps 2 and 3 matter most to the owner (the 5-min Pause, the heartbeat) and depend only on step 1.
Step 4 can go before or after 5. Step 6 goes after #881 if that lands first (then no gates to
carry); otherwise it carries `PoolView`'s gates as they are.

## 8. Open decisions

1. **The beat's period:** 10 s proposed (today a beat every 20 s at best, minutes at worst). The
   NAS write per Mac every 10 s is small; with #878 it's an HTTP post.
2. **The file asks' future:** keep the watcher for old CLIs indefinitely, or drop the files once
   every caller posts to the endpoint (one app version later).
3. **Watcher:** FSEvents (needs a run loop thread, a `fsevent` binding) vs kqueue on the folder
   (no new dependency, coarser). Proposed: kqueue plus the 2-s sweep.
4. **The stall sample:** keep `sample` on a part past 5 min (plan only), or drop it with the loop
   watchdog.
