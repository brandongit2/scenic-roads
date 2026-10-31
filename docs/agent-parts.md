# The agent in parts

**Built (task #871, steps 0–7 of §7), but the pool part (§4.6), which waits for #878.** This doc
says how the build agent's single serial loop (`crate::agent::Agent::run` and `step`,
crates/pipeline/src/agent/mod.rs, as it was) became independent parts, each split by what it waits
on; docs/pool.md §7.8 and §11 and docs/building.md §2 say the same as built. §1 and the parts
marked *today* describe main at e3fa1d53, before the split; §4's *Built* notes say what is.

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
(crates/pipeline/tests/no_shared_mutex.rs, a scan of `agent/` for `Mutex<` outside `bus` and
`nasread`'s own pools; *built*) holds the rule: the files not yet split keep the count they have,
listed, and the list only shrinks (a file with fewer fails it too).

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
out of touch. `OUT_OF_TOUCH_S` keeps meaning what it says: **beat** itself hasn't written. *Done for the loop as a
whole (task #899):* the status's `loop_age_s`, its threshold `loop_stuck_s` (`beat::LOOP_STUCK`,
the one constant the plan's stall log goes by too) and, past it, `loop_phase`; the page and the menu show "loop stuck 6 min, in <phase>". Since step 6 the age is the older of the round's view and a plan running (`plan::Handle::running`, read by the beat), the phase the round's or "the plan: <its phase>".

**Failure:** a shared write that times out is logged once (as `nasread` does) and the next tick
tries again; the local copy and the endpoint's `/status` are always current. A Mac whose shared
writes fail shows on the lead's page as "status not reaching the NAS" from the beat's age there.

### 4.3 nas

*Built (step 4):* `agent::nas` owns the mount: finding the share's folder, mounting it (every 5 min
at most), probing it (`answers`), the LAN remount when home again (`nas::Req::Lan`) and the
self-heal's remount (`nas::Req::Remount`, under the pool's lock taken when free, a minute at most),
on a thread of its own (looking every 20 s and at each ask). Each look (find, mount, probe) runs on
a worker waited for 30 s (`nas::DEADLINE`); past it the worker is left to the kernel, no second look
starts while it hangs, and the view (`Snap<NasView>`: the root, `Up`, `Away` or `Stuck { since }`)
says it. The loop's `root` only reads that view, so a hung mount holds neither the loop, control nor
the beat: the round says "the NAS is mounting (stuck since …): only work on the NAS waits" (a
`Waiting` the page and status show), and only work that needs the NAS waits (`Needs { nas: true }`
jobs don't start and are paused per today's rules, the plan and the pool step have no root). In
line before its thread runs (a single loop's, a dry run's, the tests'), each `root` one look under
the same deadline. The `nasread` pools (Switches, Records, Writes) stay nasread's, each call under
its deadline.

Moved under a deadline too: the planner's keys (`planning_keys`: `build::Keys::load_with` under
`nasread::with_big`, 5 min). Still under none, for **plan** (step 6) and #878 (pool.md §7.8):
`out::read_record` outside the keys, `inputs::read_report`, the helpers' listing, the coordinator's
contact publish/unpublish, GC's reads, the caches' clears. Their callers are on **plan**'s or
**pool**'s side, so a hang there costs that work, not the beat or control.

**Failure:** shown as the round's "the NAS is mounting (stuck since …)" while a look hangs.
Planned: the view on the page's machine card and the menu's NAS line (`up`, `slow`, `stuck since`,
`healing`).

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

*Built (step 6):* `agent::plan`: the `Planner` owns the plan, its offers, the gate (the lead's;
a member's asks go to the lead, `Agent::tell_gate`), the redos, the checklist, the regions' states
and steps, the forecast's and the build page's regions' workers and the catalogues' record, on a
thread of its own (in line for a single round, a dry run's, the tests'). **jobs**' round tells it
its side each round (`plan::Inputs`: a copy of the slots, the memory, the pause, the pool's view,
the switches; a job's end kicks a plan) and reads its last `PlanView` (numbered `n`). It plans when
`why_plan` says: kicked (a job's end, a gate ask), none yet, the digest (`plan_seen`, looked at
every 5 s) changed, a timed wait ended, or the 5-min fallback. The round grants from the last plan
at once (`Agent::lead_grants`; `may_pack`'s check is the grants' own, `takes_more`), each grant
carrying the plan's number (`coord::Grant::plan`, serde-defaulted for a mixed rollout); a cancel
notes the newest plan's number per target (`cancels_at`), and a grant from a plan no newer is
dropped (§5.4). A plan past `beat::LOOP_STUCK` is logged with its phase and sampled once
(`plan::Handle::stalled`), and the status says so. The forecast's files for the NAS come back on a
channel (`plan::Out`) for the lead's pool step. Not done: a plan's errors stay waiting lines (`plan`
returns jobs, not a result); the pool's own run is still locked by the pool's ops (`pool::try_lock`:
`lost_term`, the broker made on the run, the page's acts, the members' clock) until #878.

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

*Built (step 5), a first slice:* `agent::slots`, **jobs**' own thread, ticking every 100 ms on what
the loop publishes of its running jobs (`slots::JobRef`: channel, group, status file, step and
targets; published each round and the moment a job starts, shared with control):

- **the reaper:** a job's end seen as its group's head writes its status file (or its group
  empties) wakes the loop (`ControlMsg::Ended`), which records it and fills the slot at once: the
  loop's 2/20-s timing is gone (a round that ended a job goes again at once; else it waits 20 s,
  cut short by control or this thread);
- **cancels:** control tells this thread (not the loop) of a cancel ask; it takes the ask from
  `cancel-requests/`, passes it to the loop, then stops the jobs running its target at once
  (SIGTERM, SIGKILL 30 s on). The thread holds no job's child, so before each signal it confirms
  the group is still the job's (`jobs::Group::is_the_jobs`: its head the process the job started,
  by its start time, or every member started after the job): a group the loop has reaped since,
  its id maybe another's, is never signalled, and the loop's own stop (`cancel_slot`) sees to
  what is left. The loop records each as cancelled (given back, the lead told, the
  answer written) when it next tends: a cancel is passed on before its job is signalled, and the
  loop looks for one again before taking an end it sees, so a cancel's end is never taken as a
  failure;
- **another member's hold ask:** taken by this thread at once (`slots::take_member_hold`: the
  pool's lock tried each tick, never waited on; the member's hold written on the NAS under
  nasread's deadline, through **nas**'s root), this Mac's own through control (`asks::Setter`);
  each ask on a worker of its own, one at a time, so a hung NAS holds neither the reaper nor a
  cancel.

Nothing starts under a hold: `try_start_said` refuses one, and the start itself reads control's
newest hold again just before it starts the job, and once more just after publishing it: control
sets a hold before it reads the jobs to drain, so a hold taken up in that gap is either control's
to drain or seen by the start, and the new job is told "drain" before its first safe point. A
hold's drain stays control's, at once (§4.1).
In line (a single loop's, a dry run's) the loop takes cancels and members' asks itself. Still the
loop's, for steps 6–7: the slots themselves (their starts, `tend`, the memory and disk guards, the
ramp, `save_hands`, grants by channel).

### 4.6 pool (until #878)

The pool driver keeps its thread (`pool::Ticker`) but reads `Conditions` and `HoldView` snapshots
instead of the agent's fields, and does its NAS calls through **nas**. Its gates (`pool_step`'s
`gates`: leads, duties) become `PoolView`, which **plan** and **jobs** read.

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

- **#881 (the mini as fixed lead, no election: landed).** **plan** runs only on the mini; on the M1
  and M4 it never starts. The handover and takeover machinery and the settle gate are gone from
  **pool** (docs/pool.md §6); `pool_step`'s gates left are `leads`, `duties` and the re-assertion.
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
| 1 | *Done:* `agent::bus` (`Snap`, the channel types `ControlMsg`, `BeatMsg`), the slow-NAS fake (`nasread::fake::hang`: every call under a folder held) and the slow-plan fake (`Agent::slow_plan`), and the no-shared-mutex scan | — | `agent::bus::tests`, `nasread::tests::the_slow_nas_fake_hangs_its_folder_alone`, `agent::tests::the_slow_plan_fake_makes_a_plan_take_its_time`, tests/no_shared_mutex.rs |
| 2 | *Done:* **beat** on its own timer (`agent::beat`), sole status writer; the loop publishes what it composed (one `LoopView`, split into `JobsView`/`PlanView` by steps 5–6) instead of writing; also cut: `nasread::loop_thread`, `loop_work`, `PHASES`, `sys::this_thread`, `sys::thread_cpu_ms`, `Agent::last_beat` (the NAS copy written each beat) | `nasread::BEATING`, `Beating`, `beat_again`, `asks::Beat`, the watchdog's kept beat (`KEEP_EVERY`, `KEEP_FOR`, `WORKING_S`), the heartbeat phase of `step` | `beat_steady_while_the_nas_hangs`: the NAS fake blocks every call; the local status' time advances every 10 s for 2 min. `beat_steady_while_planning`: the plan fake sleeps 3 min; same |
| 3 | *Done:* **control** (`agent::asks::Handle`: its thread and a kqueue watcher on the agent's folder and `cancel-requests/`, opened anew when either is made again, a 2-s sweep behind it; inline for a single loop) on the files the endpoint writes too (`/work/hold`, `/work/quit` added beside `/work/settings`); this Mac's hold, the quit and the settings taken up and published at once, the loop's wait woken (`ControlMsg`). A cancel and another member's hold ask wake the loop, which still takes them (they need the jobs, the pool: steps 5 and #878); the hold's NAS mirror stays the loop's (`hold_on_nas`) | `asks`' poll (`EVERY`), `Asks`, `sync_hold`, `adopt_hold`, `hold_asked_anew`, `hold_ask_time`, `hold_seen`, `asks_on`, `Agent::hold` the field (control's snapshot now), `quit_ask`'s file reads and the wait's polls, the settings block in `step`, `Snap::update` | `pause_shows_within_a_second_on_a_hung_nas` and `…_while_planning`: a hold posted; its answer and a "pausing" status within 1 s, the job's channel says drain |
| 4 | *Done:* **nas** (`agent::nas`): the mount, probe, LAN remount and the heal's remount on its thread, each look under a 30-s deadline, published as `NasView`; the planner's keys under `nasread::with_big` | the mount block in `step`, `root`'s own finding and mounting, `Agent::last_mount_try`, `Agent::probe`, `heal_step`'s unmount, `answers`' `CHECKING` (a `Mutex`: mod.rs 4 → 3 in the scan's list); the mount, probe and keys off pool.md §7.8's "still under no deadline" list | `a_hung_mount_holds_only_nas_work`: the mount fake hangs; 20 rounds each in moments, "the NAS is mounting (stuck since …)" said, a pause answered at once, the root back once it answers; `agent::nas::tests` |
| 5 | *Done, a first slice:* **jobs**' thread (`agent::slots`): the reaper, cancels and other members' hold asks taken at once; nothing starts under a hold (checked again at the start). Left for 6–7: the slots, the guards and grants by channel | `run`'s 2/20-s timing (the 2-s look after a job's end, the 2-s look while quitting), `tend_cancels`' folder read (`take_cancels` is the thread's, or in line), `hold_on_nas`' member asks (now `slots::take_member_hold`) and its "pool step busy: ask again" refusal (asked again each tick instead); the loop woken for cancels and member asks (`ControlMsg::Ask` goes to the thread) | `a_job_is_reaped_while_the_nas_hangs`, `a_cancel_is_acted_on_within_a_second_while_planning`, `nothing_starts_under_a_hold` |
| 6 | *Done:* **plan** (`agent::plan`) on its thread, triggered by changes (`why_plan`); grants from the last `PlanView`, numbered (`coord::Grant::plan`); the checklist, counts, gate, redos and side threads its workers; the round (`Agent::round`) **jobs**' | `step` (now `round`, which never plans), `plan_due`, `LastPlan`, `IDLE_REPLAN`, `Planned`, `start_soon`, `may_pack` (its check the grants' own), `try_pooled` (the round reads `PoolView`; a sweep's re-assert asked of the next pool step), `built_making`'s polling in the wait, the loop watchdog (`nasread::watch`, `Watchdog`, `ROUND_AT`, `WATCH_DONE`, `STALL_GRACE`, `Stall`, `stall`, `loop-stall.json`, `scenic status`'s stalled line; the plan's stall log and sample replace it), `Agent::cache_size`'s `Mutex` (a channel) | `a_slow_plan_delays_no_grant_pause_or_heartbeat`: a 3-min plan on its thread; grants from the last plan within moments, no lease twice, a pause within 1 s, the beat every 10 s. `a_hung_nas_holds_only_nas_work_while_planning`, `a_grant_from_a_plan_before_a_cancel_is_dropped`, `a_grants_plan_number_reads_across_app_versions`, `plan::tests::the_plan_is_made_on_changes` |
| 7 | *Done:* docs: pool.md §7.8, building.md §2 and this doc as built | — | — |

Steps 2 and 3 matter most to the owner (the 5-min Pause, the heartbeat) and depend only on step 1.
Step 4 can go before or after 5. Step 6 carries `PoolView`'s gates as #881 left them (no settle).
Left after step 7: the pool part (§4.6, with #878), and the ad-hoc `Mutex`es in tests/no_shared_mutex.rs's
list (the views' and live measures' caches, the memory sampler's, the round's two statics), each
for the part that owns it.

## 8. Open decisions

1. **The beat's period:** 10 s proposed (today a beat every 20 s at best, minutes at worst). The
   NAS write per Mac every 10 s is small; with #878 it's an HTTP post.
2. **The file asks' future:** keep the watcher for old CLIs indefinitely, or drop the files once
   every caller posts to the endpoint (one app version later).
3. **Watcher:** FSEvents (needs a run loop thread, a `fsevent` binding) vs kqueue on the folder
   (no new dependency, coarser). Proposed: kqueue plus the 2-s sweep.
4. **The stall sample:** keep `sample` on a part past 5 min (plan only), or drop it with the loop
   watchdog.

## Decisions (PM, 30 Oct night)

1. The heartbeat is written every 10 s, and at once on a change.
2. File asks stay for one app version after **control** lands, then go.
3. Folder watcher: kqueue plus a 2-s stat sweep (no new dependency).
4. `sample` stays for a plan stalled past 5 min; it goes for the other parts with the loop watchdog.
5. Step 6 no longer waits for #881 (landed).
