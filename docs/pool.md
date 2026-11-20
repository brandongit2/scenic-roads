# The pool: the Mac mini leads the build

Every Mac runs the same agent (`scenic agent`). One of them, the **lead**, plans, grants jobs,
merges results into the build's records and updates the map; the others are **members**. The lead
is fixed: the Mac mini, the member `state/build/lead-member.json` declares (§6.1). No other Mac ever
leads: there is no election, handover or takeover. With the mini gone, the members finish the jobs
they hold, keep their hand-offs, and wait for it. The Macs' agents are the only workers (§8,
coordinator.md); the build page shows the build and takes the owner's asks. The pool's code is
`crate::pool` (the declaration, records, hand-offs, the driver) and `crate::agent::membership` (the
agent's side). It is always on.

The records' readers read one snapshot, `state/build/records.json`, which the lead alone writes
(§6.2). The lead leads the epoch of its coordinator's store, which numbers its records' snapshots
and its leases. Coordination goes by HTTP to the lead, never through the NAS: beats, hand-offs,
mail, holds, settings, budgets and the records' brief (docs/coordination.md: the lead's store, the
calls, the leases' rule, the failure modes; §9 here).

## 2. Principles

1. **One agent, every Mac.** A Mac is known by a member id made once; its host name is a label.
2. **Durable state on the NAS;** a Mac's disk holds caches and work in progress.
3. **Nothing on the NAS is written in place by two Macs.** A file has one writer (a member's own
   files, the lead's), or is made once with create-new (the declaration in a new pool, refusal
   notes). A late write by an earlier process of the lead at the one path is passed over by the
   next take-up (it starts from the lead's own disk's copy) and saved over within `RESAVE_S` (§4).
4. **Every job hands its results off** to the lead (§7.3); only the lead's merge writes the records.
5. **Work goes where it fits,** by measured memory, disk and cores (§7.7). Map updates and GC are
   the lead's duties.
6. **One lead, declared:** no Mac decides by itself to lead; a restart of the lead is a take-up of
   its own records in a new epoch, not a restart of the build: nothing is lost, no job stops.
7. **Tasks belong to the member whose job offers them** (§8).
8. **Any number of Macs and pages.**

Where state lives:

| State | Truth | On a Mac | The lead after a restart |
| --- | --- | --- | --- |
| Records, built files | NAS: `state/build/`, content-named files | caches; the lead's own copy of the records, `agent/pool/records.json` | takes up its own copy (§6.2) |
| Downloads | NAS: `sources/` | cache copies | nothing |
| A pass's Wikidata/Wikipedia answers | NAS: `sources/items/<date>/` (`agentcore::answers`) | `agent/cache/items/`, `agent/cache/heritage-<date>/` | its first run takes them |
| Z6 tiles' kept state | NAS: `cache/scenic-units/` | none | nothing |
| A job's progress (resume logs, scratch) | the Mac running it; a log carried at a safe point also on the NAS (`state/pool/carried/`) | scratch folders | resumed where its next run goes (§7.2, Resuming); the rest redone |
| The coordinator's state | the lead's disk: `agent/coord/store/` | | opens it (§6.2, §7.5) |
| The coordinator's token, its history's archive, its contact | NAS: `state/coord/token`, `state/coord/history-archive/`; each Mac's `lead.json` | | uses them |
| The lead's own (map updates' record, daily jobs' last runs, the map and its log) | the lead's disk | `agent/publishing.json`, `agent/state.json`, `agent/maplog/` | keeps it |

## 3. What the NAS gives us

The share is SMB 3 on a Synology. The design assumes:

- **Create-new is atomic**, between two Macs too: 350 trials of both Macs creating one name within
  ±30 ms each had exactly one winner, its bytes the file's (`tools/check/nas-probe.py`). The
  declaration in a new pool, and refusal notes, rely on it.
- **A rename can be delayed indefinitely** (a Mac sleeping between write and rename).
- **Reads can be stale** (macOS's SMB caches). Measured between two Macs on an idle share: a file
  renamed over read new within 1.1 s, a new file opened within 0.12 s; a stat or listing still
  showed a removed file for up to 33.5 s and missed a new one for up to 3.1 s; one name stayed
  stale for 40 minutes. The pool's protocol takes up no record from the NAS, so no stale read can
  lose what it acknowledged.
- **Renaming over a file another SMB session holds fails** (EBUSY), and the server can hold a file
  for hours with no process on either Mac having it open (a Mac killed with a file open). Renaming
  the held file away still works, so every replaced file falls back to
  `store::whole::rename_or_aside`: the old file renamed to `<name>.<host>.<pid>.<n>.aside.tmp`, the
  new one renamed into place, asides removed later (listings skip them). Files that must never read
  as missing (`manifest.json`, `pending.json`, `sizes.json`, `summaries.json`, the extents) are read
  with `whole::read_replaced`: missing with an aside beside it, the newest aside under a day old is
  read, so a manifest is never taken as empty.
- **Calls fail for a moment, or hang.** A build job's read that fails with a disconnect (EIO,
  ENXIO, ETIMEDOUT, EAGAIN, ESTALE; "not found" only on a file known to be there) is tried again
  after 1, 2, 4, 8, 16 s, then 30 s six times (211 s in all), each try logged with the Mac's time
  and UTC offset to match against the NAS's log (`store::retry`; docs/nas-access.md §2). A dropped
  SMB session leaves its handles dead, so a read meeting one opens the file again by its path (the
  files so read are content-named, so it is the same bytes). A call never answered fails instead
  of holding its job: every NAS call of a job is a `store::deadline` call (docs/nas-access.md §6),
  a read waited for `deadline::LONG` (120 s) and a second more per MB, then failed with `TimedOut`
  and left to the kernel. The agent's own calls work the same way with shorter deadlines (§7.8).
  Calls stuck past their retries are noted (`whole::stuck`); three within 10 minutes drain the
  Mac's new work and remount the share (`crate::agent::heal`, docs/macs.md).
- **No locks on the NAS:** no NAS file is flock'd or opened with O_EXLOCK; every lock is on a local
  file. A writer killed mid-write leaves only its temporary file.
- **No conditional writes or atomic appends** between clients.
- **Listings are slow** (3–33 s a folder under load): the pool's protocol lists nothing; the records
  are read at one path, and hand-offs and heartbeats go to the lead by HTTP.

## 4. Invariants

Checked by the simulator (§13):

1. Only the declared lead leads, and one process of it at a time (the member lock, §5). A lead
   whose read of the declaration names another member leads nothing.
2. Each take-up leads its store's epoch, past the records it starts from (a store whose epoch
   isn't past them isn't the one that saved them, and leads nothing), so (epoch, seq) orders every
   snapshot, and job ids (`<epoch>-<n>`, §7.5) order, across restarts and a hand move.
3. No accepted hand-off is lost: its member keeps it until a saved snapshot names it, and a lead
   never takes up from records older than those it saved (§6.2, §7.3).
4. Each records snapshot is one file written whole.
5. A late write of an earlier save never sends the lead back: it takes up from the records it
   kept on its own disk (§6.2), never the NAS's. At the one path, readers may read a late write
   until the lead's next save (`driver::RESAVE_S`, 10 minutes at most). Not covered: the lead's
   disk lost (its records and its store): it leads nothing, saying so, and the owner decides.

## 5. Roles

- **A member** is any Mac running the agent with the NAS. Its id (`m-<16 hex>`) is kept in its home
  folder bound to the hardware UUID (a copied folder makes a new id). One process per member runs
  the pool, holding `pool-<id>.lock` (`crate::pool::MemberLock`, checked every step; a process that
  loses it stops). A member beats (§10), runs job slots (§7) and brokers its jobs' tasks (§8); its
  port sends the build page to the lead's (§10, The build page). Its parts are its role's
  (docs/agent-parts.md §9).
- **The lead** is the declared member (§6.1): the Mac mini, always on, at home and on power. It
  also plans (`crate::agent::build`), runs the coordinator, merges hand-offs into the records,
  updates the map, runs GC, compacts the raw tiles' archives, keeps the pause and writes the
  build's status. Its slots are ordinary slots: its agent asks its own coordinator for work as any
  member asks (§7.2).

## 6. The lead

### 6.1 The declaration, and epochs

**The declaration** is `state/build/lead-member.json` (`buildstore::records::lead::Declared`):

```json
{ "member": "m-3f9c…", "host": "Mac-mini-de-Brandon" }
```

- **Read every step** by the lead (a failed read keeps the last), and by a member only while its
  lead hasn't said who leads within `PAGE_AWAY_S` (35 s) (`Answer::leading`, §9). A member it
  doesn't name never leads; a lead it no longer names leads nothing from its next step.
- **Made once** with create-new: in a new pool (no records at the one path nor its brief) by the
  first Mac, which declares itself; otherwise by the owner, by hand. With none there and records
  saved, no Mac leads and the members wait. Its existence means the pool has begun: no reader takes
  today's files for the records, and `out::check_writer` refuses every direct save of them.
- **Why a file and not a setting on each Mac:** one place every Mac reads, so two Macs can't each
  think they lead; written once with create-new, so no Mac's process replaces it. No member id is
  named in code. A coordinator grants nothing until its pool's step says it leads.

**Moving the lead** (the mini replaced) is done by hand; the app has no move. The lead's state is
in its agent's folder (`~/Library/Application Support/scenic/agent/`) on its own disk:

1. Stop the old lead's agent (`launchctl bootout gui/$(id -u)/<the agent's label>`, or quit it from
   the status app), and the new Mac's.
2. Copy from the old lead's agent folder to the new Mac's, replacing (not merging into) what is
   there: `coord/` but its `jobs-token`; `maplog/`; `pool/records.json`; `publishing.json`. Not the
   rest: `member`, `lead.json`, `coord/jobs-token` and the rest of `pool/` stay each Mac's; what
   else only a lead writes is made again by the new lead.
3. On the old lead, rename its `coord/store/`, `maplog/` and `pool/records.json` aside (remove them
   once the new lead leads), so a stale read of the old declaration can't make it lead (§3): with
   no records on its disk it takes nothing up (§6.2).
4. Edit the declaration to name the new Mac's member id (`scenic pool status` lists them).
5. Start the new Mac's agent, then the old one's. The new lead opens the copied store as its own
   (its epoch one past the old lead's), takes up the copied records in that epoch and holds the
   old lead's jobs' leases; the old lead is a member. Members find the new lead by the declaration
   (docs/coordination.md §3.1).

The pool's token (`coord/workers-token`) is the same on every Mac. A lead whose disk lost its
records or its store isn't moved this way and doesn't lead (§4, invariant 5): the owner decides.

**The epoch** is the lead's coordinator's store's (docs/coordination.md §4.3), bumped as the store
opens, so once a process. The records' snapshots are numbered (epoch, seq), their JSON key `term`;
a lease's job run is named `<epoch>-<n>` (§7.5). The old term folders (`state/build/term/<E>/`)
are written by no app; their `forecast-input.json` files are still read by the backtest (§7.9).
There is no app rule: the lead leads whatever app it runs. Records keep keys of steps an app
doesn't know (`pool::records::Keys::other`), so a downgrade loses nothing it can avoid.

### 6.2 The records

- **One snapshot, saved whole after each merge** (`crate::pool::records`, `Records::save`): the
  manifest, keys, pending uploads (`buildstore::pending`, docs/formats.md), the sizes of the files
  the manifest names, raw archives awaiting the raw store's index, the hand-offs it reflects and
  refused (with why), the lease that last set each target, its epoch and `seq`. Also saved
  unchanged `driver::RESAVE_S` (10 min) after the last save reached the NAS. A save goes:
  1. to the lead's own disk, `agent/pool/records.json`, written whole and flushed; a failure there
     writes nothing else;
  2. to the NAS's one path, `state/build/records.json`, and its brief `state/build/brief.json`.
  A save kept on the lead's disk but not on the NAS (past its 20-s deadline) leaves the lead
  leading: it acknowledges nothing and isn't caught up (no map update, no GC) until a save reaches
  the NAS.
- **Taking the records up** (`driver::take_up`, once a process): it needs its store's epoch, and
  starts from its own records (its disk's copy), none older than `Saved::records`; it never reads
  the one path for them. With none of its own: in a new pool it takes today's files
  (`manifest.json`, `jobs.json`, `pending.json`); else it waits, saying so. It numbers them
  (epoch, 0) and saves them.
- **Merging**: it merges what members posted and its own jobs' hand-offs, from memory, in (epoch,
  lease) order (`records::merge`). It is **caught up** once every hand-off it was given is merged
  and saved; map updates and GC wait for that.
- **Acknowledgements**: a lead acknowledges a hand-off once a saved snapshot names it (in its beat
  answer's `merged`); the member drops it.
- **Readers** (jobs' `Out`, the agent's plan and status, GC, the map server, the CLI, the checks)
  read the one path's snapshot alone (`Records::now`). Readers of the manifest or keys alone read
  the brief (`records::Brief`: ~6.5 MB against the snapshot's ~14 MB, mostly the unverified
  uploads). **Agents read the brief from the lead** (crate::agent::brief, docs/coordination.md
  §5.1): the lead's in process, a member's by `GET /pool/brief` (304 while unchanged); a job opens
  the brief its agent had as it started it (`SCENIC_BRIEF`), and isn't started while the lead
  doesn't answer. The NAS's brief is read by the map server, the CLI, and an agent whose lead
  serves none. Before the pool (no declaration) readers read today's files; with a declaration and
  the one path unreadable, an error, never today's files.
- **The coordinator's state** is in its store on the lead's disk (docs/coordination.md §4:
  `agent/coord/store/`): a restart opens it and keeps its leases. Nothing of it is on the NAS.

### 6.3 With the lead gone

When the mini is asleep, off the network, its agent stopped or its share hung, no Mac takes its
place:

- **A member finishes the jobs it holds.** Each hand-off is kept whole in the member's saved state
  (§7.3) and listed in its beats; a renewal or end the lead doesn't answer is retried until it
  does (docs/coordination.md §6).
- **Then it waits.** It asks the lead for work and gets none until the lead answers; it never
  leads, however long.
- **Every view says so** (§10): "no lead: <host> is out of touch", and after an hour the alert.
- **The lead, back,** merges what members post and grants again: woken, in the epoch it led;
  restarted, in a new epoch, from its own records. A job whose lease lapsed meanwhile is offered
  again.

### 6.4 Asks of the lead

A member's build page passes its asks (a job's cancel, a target's retry, §11) to the lead by mail
(§9.1), only once the lead's app is known and equals its own; else it says to ask on the lead's own
page. There is no ask about who leads.

### 6.5 Grants and the lead's view

A lead's coordinator grants only while its agent's last step said it may (the records taken up,
its member's lock held, still leading) and said so within `UNTOLD_S` (5 min; stamped with the
step's time, so a hang after the step stops grants). Built files are content-named and never
rewritten (docs/storage.md), each hand-off is its own entry keyed by its job id, and a merge
applies an entry only if no later lease set its targets (`Records::apply`), else counts it
overtaken; the files it alone named go to GC.

### 6.6 Stepping down

The lead leads one epoch a process. It steps down when its read of the declaration names another
member or none, and when another process holds its member's lock (§5). Declared again, it takes up
its records on its disk.

### 6.7 Clocks and wakefulness

- No NAS clock: out of touch compares a heartbeat's `beat` with the reader's clock; a beat over
  60 s ahead shows "clock wrong". Everything persisted is wall-clock time.
- **The pool is stepped whatever the agent is doing** (`crate::agent::membership`'s `Ticker`): the
  agent's loop steps it about every 20 s, and the pool's own thread steps it when that loop has
  been quiet for `QUIET_S` (25 s). A tick held past `UNTOLD_S` (5 min) is taken as hung on the
  share: the lead grants nothing, its coordinator's answers stop naming its epoch and member, and
  the status says so, with the fix if it lasts: disconnect that Mac's SMB session in DSM
  (docs/macs.md).
- Every coordinator answer carries `x-pool-epoch` and `x-pool-member`, except while its Mac has
  lost the NAS or its step is taken as hung.
- **The lead stays awake** (`caffeinate -i`) while it leads with leases out.

### 6.8 Reaching the lead

A lead the members can't reach (macOS's firewall dropping incoming connections, say) grants them
nothing. `crate::pool::reachable`:

- Every member probes the lead's coordinator and the other brokers every `PROBE_S` (60 s); its own
  requests count too. Results go in the heartbeat: reached within `REACHED_S` (5 min), or
  unreachable for `REPORT_S` (2 min) with the cause.
- One failed request makes the rest of that loop's requests to the lead fail at once; a job's end
  not taken is retried until the lead answers. A kept connection that hears nothing for 2 s
  connects anew, so a lead gone asleep is unreached within about 7 s (docs/coordination.md §5.2).
- Every view says who can't reach whom and what to do: wake the Mac, or allow `scenic` in System
  Settings › Network › Firewall. `scenic` is signed ad hoc at a new path per version, so that allow
  lasts until the next update. Nothing moves the lead off a Mac its members can't reach.

## 7. The work: slots on every member

### 7.1 Slots and asks

Every job starts by a grant, on the lead as on a member. A member runs as many job slots as its
cores allow (`agent::job_slots`: its cores less the two kept for its owner, at least one); an idle
slot takes tasks (§8).

**The lead's ramp** (`Agent::tend_ramp`): the lead's slots start at 2 (`RAMP_START`) and grow by one
after each clean stretch of 6 hours (`RAMP_CLEAN_S`) in which it ran as many jobs at once as its
slots, with no memory guard stop and no stall; either starts the stretch again, as do two or more
failed NAS probes in a row. While the NAS's limits aren't known (no probe yet) its slots are held
at 2. They grow up to its cores' `job_slots`. The ramp is kept in `agent/state.json`; `scenic
status` and the build page show it. There's no switch.

**Asks.** While a slot is free and nothing holds more back (the pause, a job draining, the memory
guard, a frozen job: `Agent::may_pack`), the agent asks for as many jobs as it has slots free: a
member over HTTP (`/work/ask`), the lead in process (`Coordinator::ask_own`). An ask carries its
Mac's **capacity** (`coord::Capacity`): the most its jobs could hold (`limit_mb`: memory less the
memory floor), what its jobs hold and may hold for one to start (`build_mb`) and why none may
(`mem_held_back`), its cores and those the last minute's CPU leaves free (`cores_free`, from CPU
ticks; the load average, which counts threads waiting on the NAS, only until sampled), its disk free
less the room target and reservations, the cores its in-use share keeps, power, the NAS's reach and
answers (§7.7), its running jobs' threads, and its sustained best (§7.2). It rides the ask, not the
heartbeat, because the coordinator places at ask time and needs measures taken then. The coordinator
keeps each member's last for two minutes (`CAP_FRESH_S`). A member on another app than the lead's
gets nothing (409) until they match (docs/building.md §2, The same app).

**Disk reservations.** Each grant carries the disk its job is measured to take (`Grant::disk_mb`:
its targets' disk floors summed, or the 90th percentile of its step's last 20 jobs, the larger; a
halo step's, `pack` and `lo`, the most of its targets' rather than their sum). A grant's agent
reserves that while it holds the lease (less what the job has written so far), and the free disk
an ask says is the free space less the owner's room target and every live reservation, so no job
is counted free twice. There is no fixed reserve: the room target is the only floor.

- **Shared caches** (`needs::CACHES`: `depth`, `depth-lo`, `terrain-water`, `lakefill`, `water`):
  these steps read whole local copies of large NAS files (`localread`), made where the disk has
  room past the target, else the NAS is read in place. The cache's fill is measured from the job
  that filled it (its disk past its targets' output); the first job granted on a Mac reading a
  cache reserves the fill less what that Mac holds (`Capacity::caches`), the jobs beside it their
  output alone.
- **Caches counted as free.** On a member with a room target, what its caches can free counts as
  free disk (`room::Freeable::cheap`), freed toward the job's reservation while it runs, the disk
  guard behind it. A finished step's whole copies count as free for a job that doesn't read them
  (`sched`'s `Copies`), so they never hold a later step off a Mac that has the room once they go.
  The lead's caches aren't counted. A job held back only by disk its caches can give
  (`freeable_mb`) has the caches freed toward it and room held for its memory and cores meanwhile
  (`Agent::held_room`); the refusal names the disk (`held_disk_mb`). The caches are counted every
  ten minutes (`CACHE_COUNT`) and after any job end, clear or freeing, on a thread of their own.
- Every job is told the target (`SCENIC_ROOM_TARGET_MB`; 0 for the backup and GC). `scenic status`
  says each member's disk as `coord::disk_words` does ("49 GB free · 20 GB for jobs (20 GB room
  target, 9 GB reserved by its jobs)"). The disk guard is the backstop (§7.7).

**Threads granted.** Each grant carries the worker threads its job runs with (`Grant::threads`, set
as `RAYON_NUM_THREADS`); cost records keep the threads, the CPU time (`cpu_ms`), the time the
process was stopped (`paused_ms`) and the wall time it waited on the NAS or network (`nas_net_ms`,
store::waits by the wall clock).
**A job starts on the threads its measure gives it, and its agent controls them from there**
(Threads controlled live, below). From a target's runs, its measured cores are two figures
(`needs::Book::cores`):

- its **grant**: the cores its runs kept busy (CPU time over wall time, waits and all), to the
  nearest whole, plus one thread to overlap its waits or to climb by. A job waiting on the NAS
  settles where a run measures the same and never climbs; one its threads held climbs a thread a
  run, capped at the Mac's cores. A target not run the way it runs now takes its
  **step's parallelism** (`needs::model::Par`, §7.7, What a step needs).
- its **busy** cores, that CPU time over the time it ran, to a tenth of a core.
  **A job counts for its busy cores** in packing (`Sched::need_cores`), not its threads, so jobs
  waiting on reads go side by side until memory, disk or the NAS budget is full.
  **A running job counts for what it keeps busy now** (`sched::busy_now`): once it has ended a
  target or run past its predicted time, the more of its CPU since its first beat and over its last
  minute.

No fixed share caps or floors a grant: a job's measure decides, within its Mac's cores less the
in-use share's while the owner uses it (`coord::ask_cores`).
**Past the cores its jobs' threads leave, a Mac climbs one job at a time** (`coord::SETTLE`): an ask
is granted at most one job that fits only by busy cores, and none until a minute has passed since
its last grant, or while the cores busy pass its jobs' by more than a core. Cores busy past the
jobs' by more than a core count as outside load (`outside_cores`, at most half the Mac's), which
stops further grants but no running job. The agent logs each start:
`agent: starting <job> (<n> threads: granted <g> of <c> cores, <k> kept for the owner)`.

**Threads controlled live** (`crate::agent::threads`). A running job's threads follow what it keeps
busy now, not what its step's past runs did: each Mac's agent decides them for its own jobs about
once a minute; the lead keeps placement and memory admission, and counts each job at what its agent
says it runs on. The agent writes the threads in a file of the job's slot (`SCENIC_THREADS_FILE`,
replaced whole). Steps that take it as each target begins (slope-hi, slope-deep, terrain-hi,
lake-depth: `control::on_grant`) run that target on a pool that wide; a change waits for the next
target. A live target (terrain-deep's: `control::on_grant_live`, task #1550) reads the file every
second as it runs (`control::Live`): its work runs on threads as many as the Mac's cores, each
taking a turn of the target's (`store::waits::Turns`, as many as the file says now) for the work
it does, so a grow is taken within `POLL` plus `store::waits::RECHECK` (2 s at most) and a cut as the turns held are given back, inside
the target; its costs line says `live` (and `live: false` once its live part ends, its landing and packing
on the threads it began on), and its agent watches it on its file's threads meanwhile. A paced job
(an input check, tree cover: `inputs::pace`) reads the file every second and limits its tasks under
way to it. Other steps keep their starting grant. The thread count changes no output.

Rayon's pools can't be resized and have no hook between a worker's jobs, so a pool can't hold its
threads to a count that changes: a step goes live by putting its parallel work under the target's
turns. terrain-deep's level pipeline (`terrain_pack`'s tile making, `terrain_base`'s sampling and
its passes on the outside pool) does; terrain-hi, slope-hi, slope-deep, lake-depth, bld-fetch and
pyramids-fill don't yet, and a one-target job of theirs keeps its start grant to its end.

- **What the agent measures** (`Agent::watch_threads`): each job's CPU time and the wall time it
  waited on the NAS and the network (an instant counts as waiting when any of its threads is in
  such a call), since its threads last changed. Not its local-disk waits (a job computing on every
  thread would read as waiting) nor its waits on other work. Time the job was stopped is left out.
- **The decision** (`threads::decide`, once a job has run a minute on its threads), by its busy
  cores over its threads:
  - **Every thread busy** (0.9 of them or more, `threads::BUSY`) and under a quarter of the minute
    waiting on the NAS or network (`threads::WAITING`): doubled, within the most a job may run on
    here, the cores the Mac has free, and its memory (each thread past those its grant covers adds
    `Grant::thread_mb`, §7.7, Memory admission; none while the Mac admits no work). A job waiting
    a quarter of its minute or more is never grown: more threads would wait with it.
  - **Threads left idle** (under half busy, `threads::IDLE`): cut to those that keep it busy at
    0.9, plus one to overlap its waits.
  - **The next thread pays while nothing waits** (task #1472): busy short of 0.9 a thread (a
    wave's tail, a dry queue), grown one thread at a time while nothing waits for the Mac's cores
    (its last ask, within the minute, was offered no work at all: `sched::Rule::offers_none`, the
    lead's own ask's refusal or a member's 202 saying `nothing_here`), it waits under a quarter of
    its minute, a core and the memory are free, and one thread more pays: its step's jobs on this
    Mac kept more cores busy on one thread more than on these by more than that difference's
    standard error (`threads::Yields`, the last 8 minute windows of each count of threads, those
    under a quarter waited). A step with no windows on one thread more is tried there once these
    threads are measured (two windows). A thread so grown that didn't pay is cut back the next
    minute, and all of them are given back once work waits for the Mac's cores again (those a
    job hasn't taken yet, its target under way still on fewer, at once: its file set back,
    `Agent::give_back_untaken`; a live target's and a paced job's, which take them within
    seconds, at their next minute). A Mac with every slot full asks for no work (`takes_more`), so
    after a minute it no longer knows that nothing waits, and it grows no job this way, even on a
    dry queue.
  - Past the most it may run on (the owner back): cut to that.
  The gap between 0.9 and 0.5 is the hysteresis: a grow that didn't pay is cut back the next
  minute (4 busy of 4, doubled to 8, still 4 busy: back to 6). These figures are the rule's form,
  not limits: the Mac's cores and memory bound a job's threads. The agent logs `agent: <job> on
  <n> threads (was <m>): <why>`. A grow reaches a job as its next target begins, or within
  seconds, a live target's (2 s at most) or a paced job's (1 s): a job of one target of the other steps runs it on its
  start grant.
- **What the lead counts**: each capacity lists the jobs' threads, and the lead counts each job at
  the more of its current and its file's threads (a grow at once, a cut once a target runs on it),
  its memory at its grant's figure plus what a grow took past the threads its grant covers
  (`Sched::grown_mb`).
- **Why not shorter batches.** Capping a job's length so its threads are decided as each starts
  would pay a job's start every batch: a job's records and regions read took a median 9.4 s of
  slope's 22 s runs and 12.4 s of terrain-deep's 15 s. A thread change at a target's start costs
  milliseconds. Batches stay at the batch length (§7.7, Batch length).

**The in-use share** (`crate::agent::share`): the cores a Mac keeps free of its jobs (by default 2)
while its owner uses it (active within five minutes). `scenic in-use [--cores <n>] | off` sets it.

**The memory floor** (`crate::agent::memfloor`, `crate::agent::memguard`): the memory a Mac keeps
available whatever runs on it, as the room target keeps free disk. `scenic memory <GB>`, the build
page's Settings row or the memory chart's line set it per Mac (`memory-floor.json`); `scenic memory
off` clears it. Not set, an eighth of the Mac's memory (6 GB of the M4's 48, 4 of the mini's 32, 2
of the M1's 16). The floor is taken as set: no cap, reserve or footprint stands behind it.

- **Available** (`sys::memory_available`) is what macOS hands out at no cost, plus what it frees by
  compressing others' idle memory with its pressure left normal; the pool's jobs' own resident
  memory is never counted as idle.
  - *At no cost:* free pages, purgeable pages, the file cache down to the kernel's file-cache line
    (`vm.vm_page_filecache_min`, scaled to where it will stand once compression is spent), and
    reusable pages (memory a process freed with `MADV_FREE_REUSABLE`) as far as the inactive
    anonymous pages hold them. The line is a soft preference: under it xnu's pageout compresses
    anonymous pages before taking file pages. Unreadable, the cache counts for nothing.
  - *By compressing:* each Mac's pressure went to warning most often once the compressor held
    48–50 % of the memory not wired, and was never at warning a whole bucket under 39.8 %. So the
    compressor may grow to two fifths of it (`sys::COMPRESSOR_NORMAL`); what it can still compress
    there (the inactive anonymous memory, less the jobs' share) frees ratio − 1 per page. Past two
    fifths only the no-cost part counts. Never raw free pages (macOS keeps those near zero), and
    not `kern.memorystatus_level` (it counts apps' memory in use).
  - *What leaves them:* macOS's malloc keeps a freed block's pages for blocks of that size: a block
    under about 2 MB is never unmapped, and its pages go to other sizes only in part, so a
    process's reusable pages are about the sum, over its block sizes, of the most each held at
    once, less what it holds now. `malloc_zone_pressure_relief` gives none back. A long-lived
    process keeps its reusable pages down only by keeping each size's peak down. Every child of
    `store::spawn`, the agent and the map server run with `MallocLargeCache=0`
    (`store::spawn::NO_LARGE_CACHE`), so freed large blocks leave the footprint (a dense z6 tile
    had held 6–7 GB of them), at ~50 ms a GB allocated.
  - *Measured* (a program taking memory in steps, 18 Nov): on the M4 what the measure counted was
    handed out with the pressure normal, give or take a GB near the bottom.
- **Reading** (`memguard::Reading`, every 5 s): what's available, what the jobs hold, the floor,
  the swap grown over five minutes (`SWAP_WINDOW`), the dip (the window's least of what's
  available), what other processes grew over the window, the kernel's pressure, a cool-down. Peaks
  are smoothed over a minute (`MEAN_OVER`); falls show at once. The jobs' **room** is what they
  hold and what's available, less the swap grown and the floor (`Reading::room_mb`); the Mac is
  below its floor when what's available, the swap grown off, is under it. Admission goes by the
  window's least (`Reading::low_usable_mb`), so a job is granted for its run, not a peak (the M1's
  available swung between 0.05 and 2.5 GB within 10 s).
- **Admission, with reservations:** a job starts only where available − (each running job's
  expected peak less what it holds now) − the new job's expected peak ≥ the floor. The ask says
  what its jobs may hold for one to start (`build_mb`) and what each holds (`held_each`); the lead
  packs each running job at its expected peak or what it holds, the larger (§7.2, Packing). A
  job's expected peak is the one figure the lead packed it at (`Grant::peak_mb`, §7.7, Memory
  admission); no agent has a figure of its own. A Mac held back is granted nothing, the refusal
  saying `memory: <why>`.
- **Give-back**: below the floor and when that relieves it, at safe points (§7.2, The memory
  guard).
- **Cool-down** (`Reading::cooling`): a job the guard gives back holds new work off its Mac until
  what's available is back past the floor and that job's peak, or `memguard::COOL_WAIT` (five
  minutes, the swap window) has passed, so a smaller job isn't started into the same shortage. The
  wait outlasts most outside swings: on the M4's ring, other processes' rises of 2 GB or more fell
  back within 60 s at the median and 190 s at the 90th percentile.
- **The brake** (`Reading::brake`), the safety net: the pressure at warning or critical, or swap
  grown past the floor's room: no job starts. It acts on running jobs (§7.2, The memory guard) only
  with evidence: the pressure critical, or the Mac below its floor by the numbers. Warning alone
  holds new work and freezes nothing: macOS sits at warning for long stretches while it compresses.
  The build page shows a "memory brake" chip while it's on.
- **Status**: the Mac's card, the menu bar and `scenic status` say it in a line (`memguard::words`):
  "Memory: 9.3 GB available (3.4 GB by compressing), floor 6.0 GB (default); the jobs hold 12.1 GB,
  3.0 GB more reserved for their growth; 1.2 GB for a new job", or why no new job.

### 7.2 Placement: the steps table

`crate::agent::steps` has a row per step the agent runs as a job (docs/pipeline.md: its id, title,
what it does, kind, scope and stage, and these columns). It holds no memory figure: a step's needs
are measured (§7.7).

| Column | Meaning |
| --- | --- |
| shared | whether members take it (no rank: the plan's reads order the work, below) |
| alone | a whole-build run kept to the lead (`crate::agent::every`) |
| answered | keeps the pass's answers (the lead's alone) |
| write-set | the manifest names its jobs may change (`agent::steps::writes`, §7.3) |

Rows of note: `trees`/`terrain-hi`/`slope-hi` per z6 tile with `*-lo` assemblies per z3 tile,
`terrain-deep`/`slope-deep` per z10 tile (docs/steps.md); `bldprep`/`bldtiles` per z6 tile,
`bld-fetch` the lead's (docs/buildings3d.md §3.6); `inputs`, the lead's, planned before every other
job (docs/inputs.md §4.3).

**One scheduler** (`crate::coord::sched`): every rule that decides which jobs an ask gets lives
there. `Sched::pack` takes the scheduler's state (leases, offers, work done and failed, each
worker's last ask, the measures, the NAS budget), the ask and a clock, and returns the jobs granted,
changing nothing but that state: no file, network, wall clock or log. The live coordinator calls
it on each real ask; the forecast (`agent::forecast`) calls the same `pack` over simulated time on
the lead's state as data (`sched::State`): each Mac asks as one of its lanes frees, jobs end at
their modelled times, and a target waits for what the plan says it reads. So the schedule packs as
the coordinator does: by memory, disk, cores, the NAS budget, cold starts and the lead-only steps.
Guards: `tests/grant_rules.rs` fails on a grant rule outside the scheduler, a test holds the
forecast's first grants to the coordinator's, and at run time the lead logs where its grants since
the last forecast departed from it ("the grants departed from the forecast"; a departure counts
when a Mac's jobs of a step differ by 2 or more and by more than half, once an hour each).

**Each Mac at its sustained best**: the forecast takes each Mac's room from now on to be the highest
value held continuously for 30 minutes over the last 48 hours (the owner's figures) of its memory
for jobs and its cores not used outside the build, from its live store's minute ring
(`live::Store::best`, carried as `Capacity::best`). The live coordinator never reads it. No use of
the owner's or passing load is foreseen: the build page says the finish is a best case. The NAS
budget's window is held as recorded while the jobs out at the record run. The forecast's
**live error** (`forecast::pace`): the oldest forecast of the last 1–6 h against the targets done
since, shown marked rough, not applied.

**A Mac granted fewer** than the forecast foresaw is said with what the lead answered its asks
(`Coordinator::answers`: the last hour's asks, wanted and granted, and the rule that held each back,
`sched::Rule`). Rules that decide by live state the forecast can't know say so (`Rule::live`: the
memory guard, cores busy outside the pool, the NAS pacing). For example:

```text
agent: the grants departed from the forecast: m4: 0 job(s) of unit granted, the forecast's 3;
its 3 ask(s) since wanted 5 job(s) and got 0, 2 offered unit; held back by memory ×2 (live: …),
cores ×1; the last: cores: 0.5 of 8 left, a job of unit takes 1.0
```

- **Every step to every member** (`crate::agent::every`): all but the lead's own steps
  (`every::LEAD_ONLY`: map updates, prunes, `bld-fetch`, the answer-keeping steps, the worldwide
  steps) go to any member whose measured room fits, the lead among them.
- **The lead's own steps by grant too** (`Agent::offer_plan`): the lead offers its whole plan in
  order, and a step kept to the lead is granted only to the current lead's member asking in
  process (`every::grants`); a member's ask never gets one. One job of such a step runs at a time,
  held by its measures and the NAS budget as a member's job is. A cold one that can't go beside the
  lead's jobs holds the lead's asks until they end, then goes first (`own_cold_waits`). A planned
  job of no targets of its own (GC, the backup, the planet's fetch, the OSM pass) that ended after
  the plan's inputs were taken isn't asked for again until the next plan says there's more, so a
  job of moments isn't granted every round. A failed one keeps its targets from the lead for the
  backoff (§7.5, Failures back off).
- **Packing** (`Sched::pack`): a Mac may hold several leases. An ask says how many jobs it would
  start (`want`); the coordinator grants up to that many in one answer, each its own lease, packed
  in plan order into the ask's room: each must fit by its measures (memory, disk, busy cores) or a
  cold start's provisional need (§7.7), within what's left of the ask's memory (§7.1, The memory
  floor), disk, and cores (less the in-use share's and the cores busy outside the pool, at most half
  the Mac's), less the needs of every job it holds and those granted with it. The room left is never
  more than what its jobs hold now leaves. A lease counts until it ends or lapses.
  **Room held for a big job**: the first offer refused for its room alone, one that would fit once
  the Mac's jobs end, has that room held, so small jobs filling each slot as it opens don't starve
  it. While it waits a later job still goes when it fits beside it, or, measured, when it ends (at
  its runs' high percentile) before the jobs waited for are expected to end (backfilling). A job's
  expected end (`Sched::run_end`) is its measured time less what it ran; else its progress's
  estimate (`Live::left_s`), discounted at a low percentile of how such estimates ended for its
  step; else, past its measured time, its slowest target's high percentile; else none. So a hold
  never idles a Mac for as long as its jobs last, and never waits for a long job while a shorter one
  frees its room. The refusal names the target held for, the jobs it waits for and which end it
  took. Each job is as many targets as fit the batch length on its Mac (§7.7, Batch length). Each
  slot past the first two is made as jobs are granted and dropped once empty; each has its own files
  (`job-<k>.json`, `costs-<k>.jsonl`, …), scratch folder and worker name (`<host> #<k>`).
- **Order and locality:** a Mac gets a contiguous run of plan targets. Every ask tries the offers in
  plan order. The plan lists each region's work, then the ready work of the steps reading it, each
  moved ahead of the steps it reads (`steps::readers_first`): finishing what's started. No step's
  rank is kept by hand: what reads a step isn't offered until it's built, and a hand-set "terrain
  before slope" only starved ready slope work. A step read several targets at a time
  (`steps::read_together`: terrain-deep's 3×3 for a slope-deep tile) is planned in Z-order
  (`steps::block_order`), so what reads it is ready a neighbourhood at a time. Measured on 31 Oct's
  state, that cut slope-deep's wait once ready from a median 10.2 h to 0.1 h.
- **Placement by how crucial** (`crate::agent::placement`, the pool's setting `placement`, §12): a
  member's asks try steps by their forecast slack, least first; within a step, the targets near
  its last one first (they read the same inputs), then the longest (a target whose run stopped
  short by what is left of it, `forecast::partials`, as the forecast takes it: `Sched::stopped`,
  task #1454). **Where it finishes soonest**:
  each target goes to whichever of the asker and the members asking lately that would take it now
  (`Sched::would_take_now`) finishes it soonest by its needs over that Mac's supply
  (`Sched::finish_s`), divided by `CACHE_PREFER` (4) where the Mac holds the step's shared cache
  whole. Each other member counts for as many targets as it has slots free; once those are spoken
  for the rest are the asker's (no Mac idles while another's slots are full). Placement only picks
  among Macs that would admit a target. Why a job went where it did is kept with its grant
  (`Pick::why`) and shown on its page. A member whose cores are swamped by outside load isn't
  passed targets.
- **Resuming** (`crate::agent::resume`): a job interrupted with progress on a Mac's disk (the OSM
  pass) is offered to that Mac alone for 24 hours, or until `scenic pool release <member> "<job>"`;
  its progress (`progress.json`) rides each beat and the lead keeps it in its store. A long target
  resumes at the block or piece after the last it finished (crate::resume_log), on that Mac or,
  given back, on whichever Mac takes it next:
  - **Safe points.** A drain (a pause, an update, the memory guard) stops a job at one shared kind
    of safe point: each block or piece a resume log keeps (`control::piece_saved`), and the
    terrain's sampling before each chunk. The job checks `SCENIC_CONTROL` (at most once a second)
    and exits as paused, its log kept. Steps keep a log of blocks (depth, terrain-hi, terrain-deep)
    or of independent pieces (`resume_log::Pieces`: bldtiles' z8 tiles, bldprep's row groups,
    slope-deep's z13 subtrees, trees' rows, water's z5 subtrees, the regions check's regions, and
    more); steps whose records are their pieces (pass-sets, heritage) need no log. A unit z6 tile's
    pieces are its preparing and each program of its tail, kept with its folders' digest, so it
    resumes only on its own Mac. A log is taken only for the same inputs (its step's versions, the
    target and what it reads), so its outputs are the same bytes.
  - **The bound**: every job records how often it reached a safe point and its longest gap
    (`safe_gap_s`). One declared bound, `control::SAFE_GAP_BOUND_S`, is a third of the drain's grace
    (`DRAIN_GRACE`, 15 minutes: 300 s), so a job asked to stop reaches a safe point before it's
    frozen, on the slowest Mac too; a run past it says so in its log and on the build page. Steps
    still past it by design: heritage's World Heritage clip, pass-sets' sets, unit's longest
    program, the regions check's stages after the waters, names-todo's labels.
  - **Carried to the next Mac:** a job stopping at a safe point short of its targets copies its
    logs to the NAS before it exits (`resume_log::carry_out`), under
    `state/pool/carried/<step>/<target>/<z>_<x>_<y>.resume`; the next run of the target anywhere
    takes the copy when it holds more than the local log (`take_carried`). A log is skipped when
    copying it up and down costs more than the work it holds (`worth_carrying`), and one Mac carries
    one at a time. Nothing is carried on a failure, cancel, signal or memory guard drain, nor for a
    unit tile (its folders stay on its Mac). The lead deletes copies of targets no longer planned
    hourly (`resume_log::sweep_carried`). Over 7–19 Nov's 353 give-backs, carrying cut the work lost
    from 3.4 to 2.5 minutes an hour (an upper bound).
- **The forecast runs off the loop**: the lead makes its input on the loop and runs it on a thread
  of its own (`Agent::start_forecast`); its three runs go side by side at utility QoS, each cut at
  `forecast::BUDGET` (15 s of CPU, not wall time: a loaded Mac starves utility threads) or 120 s of
  wall time. A cut run is `partial`, with no finish shown. A live-scale input (~19,000 items on 30
  lanes) computes in 5–12 s on the M4 under load (§7.9, Where the forecast's time goes).

**A job waiting on a worker** (`crate::offload`'s `WAITING_ENV`): a z6 tile job waiting on a faster
worker for its tail says so, and its agent names the lease in its capacity with what it holds now;
the lead packs it at that memory (never above its measure) and none of its cores, its disk still
held. A job started beside it **yields** (stopped, given back, not a failure) if the waiting one
resumes and its Mac is below its memory floor.

**A job leaving the cores idle**: a job that used under a core-minute of CPU over `IDLE_CPU_S` (10
minutes), waiting on the network, is named in its capacity (`Capacity::idle`), and the lead packs
it at its memory and disk but none of its cores. Once it uses a core-minute again it counts its
cores again and the Mac is granted nothing more until they fit.

**The memory guard** (`crate::agent::memguard`; always on):

- A thread samples each job's processes' footprint every 5 s, per target under way, and writes
  `memory.json` for the status.
- **Below the floor** (§7.1, The memory floor), a job is given back only when that relieves the Mac
  (`memguard::worth_stopping`): the jobs here together hold what it's short, and the shortfall isn't
  the other processes' growth. Else the Mac short by load beside the pool holds new work off
  through admission, not the running jobs. Relieving it, with the largest job fitting alone,
  another job stops at its next safe point (`memguard::decide_by`): one that yields first, else the
  newest, then the least progressed (the work lost least), never the largest; one at a time. The
  largest alone past the room stops at its next safe point too.
- **While the brake acts** (§7.1) no safe point is waited for: the job that would drain is frozen
  (SIGSTOP), then the next while the rest still pass the room. Freezing is the last resort: it stops
  a job's growth and lets macOS compress it while its work is kept. A frozen job goes on once the
  Mac is back. After a minute frozen with the brake still acting (`PRESSURE_HOLD`), or 15 minutes
  between the two, it's stopped only if that relieves the Mac (the frozen jobs, largest first, until
  what it's short is covered; at critical pressure without weighing the others' growth). One not
  worth stopping stays frozen; frozen 15 minutes with the brake acting it's given back as an
  interrupted job. Replayed on the M1's 15 stops of 18–19 Nov, the rule would have made one: 9 freed
  nothing near the shortfall and 4 were the other processes' growth.
- A freeze outlives its agent's restart (docs/building.md §2, Restarts). A drain that finds no safe
  point in 15 minutes is stopped. A stopped job is given back and kept from that Mac for an hour.
  Only the floor gives a job back: holding more than its expected peak isn't a reason alone.
- **An update's drain** (a newer app installed): every job stops at its next safe point and the
  agent restarts once every slot is free, each job given its own wait (its target's measured time,
  else its step's high percentile, ×1.5, between 15 minutes and 3 h). A job still running then is
  left running and the new agent takes it over (docs/building.md §2, Restarts), running the old
  release's programs to its end; the lead's status names such jobs (`Capacity::old_app`).
- **Floors**: the most a target held is its floor (`floors.json`), rising only, tagged with the
  step's `cost_version`; a target's predicted memory is never below it. A target whose floor
  exceeds every Mac's room waits, the status saying so. `scenic pool floors [--clear [<step>
  [<target>]]]` lists or clears them, through the running lead's coordinator.

### 7.3 Results: hand-offs

- **A job's hand-off goes to the lead by HTTP** (`crate::coord::handoffs`, docs/coordination.md
  §5.3): its member keeps it whole in its saved state (`journal::Mine`, keyed by its job id); each
  beat lists the keys it keeps, the lead's answer says which it lacks (posted next, `POST
  /pool/handoff`, durable in the lead's store before its 200) and which a saved snapshot names
  (dropped). Nothing goes to the NAS. A job's folder is removed only once its hand-off is durable.
- **The lead validates at merge** (`records::Check`): every change within the step's write-set,
  content names matching logical names, the lease's targets. Write-sets are one function per step
  (`agent::steps::writes`) used by both the job's `Out` (which refuses a name outside before it
  lands) and the merge. An entry outside its write-set from a member on a newer app is **held**
  until the lead runs that app; otherwise it's refused, with why. The lint `tests/write_sets.rs`
  gates publish.
- **Work the merge doesn't take is done again:** the coordinator holds a job's targets done from
  the hand-off's acceptance (never granted again while a plan made before the record was read
  offers them); the merge names the targets of entries it refused or passed over, and the
  coordinator lets go of exactly those, so the next ask grants them again. Each refusal is in the
  history and the log.
- **A hand-off refused every time backs off**: from the second refusal of a target in a row, it is
  held from the worker as a failure is (§7.5, Failures back off), and flagged (§10, Flags:
  *refused*). A first refusal holds nothing (it may be a one-off). A page's retry lifts it, and
  its "retry" history event keeps it lifted across a lead's restart.
- **A redo is a hand-off too:** `scenic redo` (§11) clears targets' keys through a job of the lead's
  whose entry carries only a done record, merged only by the lead whose member it is.
- **Sizes ride along:** `Out` records each file's size and the hand-off carries them, so the
  `catalog` step never stats them on the NAS.
- **Lease order holds:** an older lease's entry for targets a newer one set changes nothing.
- **Nothing is lost between the lead's save and its member's drop:** a hand-off is acknowledged
  only once a snapshot naming it is saved and the lead's state saying so is on its disk, and a
  take-up never starts from older records.
- **A removed step's names leave the records the same way**: the `gc` job removes from the manifest
  every name under `agent::gc::RETIRED`, its write-set these removals alone, so its hand-off merges
  as any entry. **So do the depth packs no target names** (`agent::gc::DepthKeep`); a depth job
  still running for such a tile merges after and puts its pack back.
- Raw tile archives are only added by jobs; compacting is the lead's duty. `summaries.json` is a
  content-keyed cache any writer may update.

### 7.4 Planning reads only the records

In the pool, a pass is complete when its records name `sources/osm/<date>/pass`
(`crate::osmpass::latest_pass_by`). A map update runs only when the lead is caught up. A resuming
job is given its member's unacknowledged hand-offs of its step (`SCENIC_UNMERGED`), which
`Out::open` lays over the snapshot.

### 7.5 The coordinator across restarts

- **Its state** is in the pool's store on the lead's disk (`crate::pool::store`,
  docs/coordination.md §4): each change in the store's log, fsynced before the request that made it
  is answered (a grant is never answered before it's durable). A restart opens the store, its leases
  back unconfirmed. A lapsed lease stays in the table (a member's word may hold it again) until its
  target is granted again.
- **Job ids** (jobrt::jobid, docs/formats.md, Job ids): each job run is named by the lease it was
  first granted under, `<epoch>-<n>`: what its spans are kept by (`GET /api/job/<id>`) and what its
  page on the build page is keyed by. Never given twice: each start of the store leads an epoch past
  every earlier one, and within an epoch lease numbers only go up, from past the time in
  milliseconds and the highest number its history names, so a lead with lost state and a clock set
  back gives none again. A job taken over by its agent's successor, or held again under a new
  lease, keeps its id; a resumed or retried job is a new run. The id goes into every record of the
  run and its environment (`SCENIC_JOB`).
- Workers are a member id and an agent instance; a take-up drops only instances that no longer
  exist.
- **Failures back off** (`Sched::failed`, jobrt::backoff): a target that failed on a worker (a
  member's job failed, not stopped; the lead's own failed or stopped by the memory guard), or whose
  hand-off from it the merge refused a second time in a row on (§7.3), isn't offered to it for
  `backoff-start-hours` (1) after the first, twice as long after each more, at most
  `backoff-ceiling-hours` (32); a failure is forgotten that long after its last, and the worker's
  hand-off of the target lets its hold go. Both are pool settings (`scenic pool set
  backoff-start-hours|backoff-ceiling-hours <hours>|off`, the build page's Settings, "Backoff"; the
  start 0.1 to 24 h, the ceiling 1 to 168 h, the history's week), taken as set. A lead's start
  counts the holds and the refusals in a row again from its history (`seed_backoff`): its "fail",
  "done" (failed: its "lease"'s targets less those done), own "end" (`held`), "refused",
  "merged" and "retry" (a page's retry, `clear_failed`: its target's holds and refusals
  forgotten) events, in order, each hold from its event's time.
- **What a job waited on**: the coordinator writes on each "start" and "lease" event `after`: the
  numbers of the "end"/"done" events of the jobs it waited on (per step its targets wait on in the
  forecast's dependencies, the one that finished last). The build page's schedule draws its arrows
  from it.
- **The history** is the coordinator's file on the lead's disk (`<agent>/coord/history.jsonl`, the
  last `history::DAYS`, 7, days), held in memory for the page (§10, The page's history); members'
  notes reach it in their beats. Each day before today is packed by the `gc` job into
  `state/coord/history-archive/<day>.tar` (merged into what the tar holds, written to a temporary
  name, read back and renamed), so a lead's disk lost loses only the days not yet packed. A Mac
  that led and starts as a member packs what its file holds, every day of it (the day it handed the
  lead over too), under its own member, on a thread of its own, looked at from its start every
  `FORMER_EVERY` (60 s; a look gone wrong, a tar unreadable, doubles the wait up to an hour,
  `FORMER_BACKOFF_MAX`, its warning said once while it stands). The file is removed only by a look
  `former_settle` (10 min) after its pack returned that finds every day's tar holding its lines
  and the file as packed, and never by the leftovers' sweep (docs/agent-parts.md §9.5):
  `agent::histarchive::spawn_former`, `Former::look`. Every merge into a day's tar reads the tar
  again just before its rename and merges again if it changed (`merge_into_tar`, 3 tries), so a
  writer frozen between its read and its rename (a GC job the memory guard stopped, a lead's
  laptop asleep) merges in what landed meanwhile. The settle covers the rest: another writer
  whose read just before its rename came before the pack's rename, its rename landing after (two
  back-to-back calls; 10 min is twice the agent's longest NAS deadline, the big read's 5 min).
  Lines dropped all the same are found missing by that look and packed again, as the lead's GC
  packs its own again at its next run. `scenic
  timings publish` and the forecast check read the tars and the lead's file.
- **The contact** (`coordwire::lead`): each Mac's agent keeps its lead's contact (`lead.json`:
  member, host, addresses, token, page), and every other process on the Mac (CLI, map server,
  updater) finds the lead by it with no NAS read. With none, or one naming another member than the
  declaration, the declared lead's host is reached by its tailnet addresses and LAN name. A member
  looks again when a request fails, or when the coordinator it has answered as a broker or as
  another member.
- **Its port is free for the next:** children are started (`store::spawn`) one at a time and only
  while no listener or connection is being made or closed, so no child inherits a socket or another
  child's pipe (Darwin has no `pipe2`). clippy.toml's disallowed-methods keeps every start, listen
  and accept going through it.

### 7.6 The lead's own slots

Jobs call the lead through one interface resolved again each call (`crate::agent`'s `beat`,
`end_lease`): in process while this Mac leads, over HTTP otherwise, so a job carries on across the
owner's move of the lead. A job's tasks are offered through its own Mac's coordinator or broker.
A process's parts are its role's, read as it starts (docs/agent-parts.md §9.1); a change of the declaration
naming another role restarts the agent, its jobs handed over to the next process. A lead whose
update waits on its jobs keeps the lead and restarts at a safe point like any member
(docs/building.md §2, A newly installed app).

**A lease names its slot** (`Lease::slot`, from the lead's loop or a member's beats), so the
forecast puts it on that slot's lane.

**The lead's running jobs' leases never lapse**: each loop the agent says which leases its slots
run jobs under (`Coordinator::keep_own`), and those are live past `coord::TTL` (10 min), beats or
not, so a loop stuck on a slow NAS never kills its own running jobs. A lease whose job ended
without giving it back leaves the set at the next loop and lapses as any. A live job of no targets
holds its whole step: no other lease of the step is held beside it.

### 7.7 What a step needs, by structure

Rule: nothing depends on a hand-kept figure. A step's needs are **measured** per target,
**enforced where the resource is used**, or **derived from the code**. A wrong declaration may cost
speed, never correctness, politeness or work.

| Need | How |
| --- | --- |
| memory | measured per target (peaks, the guard's floors, §7.2); predicted by the needs model (below) |
| input size | measured per target beside its cost (`sizes` in `costs.jsonl`, `buildstore::costin`: bytes read, threads, area, files opened, what `Out` made and read back, and step-specific counts); read by the step's model |
| disk | measured per target: bytes written through `store::cachefile` and the scratch writers, plus a look each minute at a slot's scratch; kept as the target's disk floor (`disks.json`). **Disk guard:** when free space falls below the owner's room target (less a thirtieth of it), the fastest-growing job is frozen (another too past half the target), room is made from caches no job holds (`room::toward`), and it goes on once the target is free again or is given back. With no target, the guard fires only at a full disk |
| jobs on one Mac | packing (§7.2): measured memory, disk and busy cores fit; a cold start by its provisional need |
| raw tiles | `store::cachefile`'s locks; packing deletes only tiles no job holds; a job reopens its shard's archives when the index changed |
| Wikimedia budget | `crate::budget`: one budget for the pool, enforced at egress (below) |
| NAS and network | `crate::budget::nas`: each job's NAS weight, held within a paced window (below) |
| a pass's answers ("answered") | `crate::answers`: a lease around each sync, the lead's (renewed every 150 s, lapsing after 10 min; `/pool/lease/answers`); on a Mac a reader/writer lock |
| write-set | `agent::steps::writes`, enforced by `Out` and the merge (§7.3) |
| targets per job | as many as fit the batch length of measured time on the Mac (Batch length, below); a step kept to the lead in one job |

**The Wikimedia budget.** Every job's connections go through its agent's proxy
(`crate::budget::proxy`, `HTTPS_PROXY`), the job sandboxed (`sandbox-exec`) to deny any other way
out. A metered connection takes a token first, from the lead (`POST /pool/budget/take`): `wiki` four
requests a second; `query` (WDQS, QLever) one query per two seconds of query time. A 429 told to the
proxy holds every Mac. The lead away, a Mac takes nothing. The budget, our User-Agent and the
back-off are how APIs and downloads are asked politely; robots.txt is for crawlers (docs/inputs.md
§6.9). Lint: `tests/budgets.rs`.

**The NAS budget** (`crate::budget::nas`). The NAS is soft with a cliff: past it, processes on two
Macs sat in uninterruptible waits. Each job has a **NAS weight**, its share of the NAS in
thousandths:

- **Weight**: its targets' measured rate (bytes asked of the NAS, whether the NAS or the page cache
  answered, plus what it wrote and read back, and its creates, listings, stats and deletes), each
  over the NAS's probed limit for it, summed, at most the whole (1000). A target's rate is its own
  run's, else its step's runs of about its size: their moves over their time together (a mean
  rate; a rate per run let slope-deep's one-second runs read 248 MB/s, three times the NAS). A job
  of several targets weighs what they move over their predicted times together. Unmeasured, 0, but
  a step known to load the NAS (`NAS_STEPS`: `unit`, `pack`, `lo`, `gc`, `planet-fetch`) weighs the
  whole. **A leased job that has run past its predicted time** weighs what its beats say it moves:
  the greater of its mean since its lease's first beat and its rate over the last minute.
- **The limits** are measured: the lead probes the NAS every 30 minutes (4 streams at once, as one
  stream measured a fifth to a half of the NAS: small files made, a 16 MB file written, synced and
  read back, files deleted, a folder listed and statted), and takes the best of the last day's
  probes, kept in its store so a restart keeps them.
- **The pacing** (`nas::Pace`): the leased jobs' weights together are held within a window, not the
  whole. Every reading (10 s) the lead reads each Mac's NAS answer time, the lower quartile over
  30 s of its listings of the project folder on its jobs' SMB mount (every 5 s) while a build runs,
  cached answers (under 0.5 ms) left out. Beside them each Mac runs a **probe apart**
  (`agent::apart`): a 4 KB read past the NAS's page cache (O_DIRECT, at a random offset of the
  largest file in `sources/canopy`) on an ssh of its own to the NAS (the key the app's other ssh
  calls take, `BatchMode`: never a password), kept open, every `nas-apart-seconds` (5), its answer
  the lower quartile over the span or over its cadence, whichever is longer. **The probe is in
  shadow** until live signals validate it (below): recorded, shown and flagged, it judges nothing
  unless `nas-apart-judges` is on; with it on, a Mac's probe answers in place of its listings while
  it answers (off or down, its listings). Each Mac's ratio is its excess over its own floor (its
  least answer over the last day on that link, never under 2 ms), measured against the least
  floor among those answering the same way (listings against listings' floors, probes against
  probes'); with two Macs or more answering, the second-highest counts, so one Mac's own link never
  paces the pool alone. A probe's floor windows are made by time span, a cadence of 15 s or 60 s
  filling a span's window as five-second listings do. And each disk's busy share, read from the Synology over SNMPv3
  (`crate::budget::snmp`, keys from `inputs/keys.env`, never on a command line or logged), the
  busiest smoothed.
- **Why a probe apart** (measured on the mini, 20 Nov, its five jobs running, a probe each second
  for 10 and 15 minutes): a listing on the jobs' SMB session waits behind their traffic, a median
  of 73-78 ms and 60-62 % of them over 30 ms in the seconds the session moved 5 MB/s or more (14
  MB/s on average), 1.7-2.0 ms in the seconds it moved under 0.5 MB/s. On an ssh of its own, the
  same listing took 1.3-1.8 ms either way, none over 30 ms, but it is answered from the NAS's
  cache: over 29 half minutes its rank correlation with the disks' busy share was -0.19, where
  the M4's own listings' was 0.72. The 4 KB direct read follows the disks (0.70; a median of
  31.7 ms, 5 ms at the least, 43 ms in the seconds the mini's session moved 5 MB/s or more, with
  the disks busier then). A second SMB session isn't to be had the app's way: on the mini, mounting the share
  again by the Keychain's other name for it (`mount volume`, the Bonjour name) mounts nothing new,
  macOS taking it for the share already mounted. The probe's ssh is its own, not `nasssh`'s
  ControlMaster: a multiplexed channel waits behind the other channels' streams (a `tar` or `rsync`
  over it), the stall the probe is there to stay out of.
- **What validates the probe** (none measured live yet; each from the records below): per Mac per
  half hour, the rank correlation of `nas_apart_ms` with the disks' busy share (expected 0.6 or
  more) and with the Mac's own NAS traffic at a steady disk load (expected about 0, the Wi-Fi M1
  and M4 especially); each Mac's probe floor (expected 4-7 ms); and the disks' busy share at which
  the probe's ratio passes 3 times its best. The records for it, kept for a later replay to model
  direct reads from real answers in place of `calm_ms`: each Mac's `nas_apart_ms` beside its
  `nas_read_mbs`/`nas_write_mbs` in its live store (archived to the NAS daily), and the lead's
  `nas_disk_busy_pct`, `pool_nas_answer_x` (judged) and `pool_nas_apart_x` (the counted Mac's
  probe ratio, judged or in shadow).
- **Judging the answers.** How slow the NAS answers says how slow it is, not why: over 19 and 20
  Nov the counted answers were past the knee in 17 % of the readings with the pool's jobs weighing
  under 25 thousandths. A job's weight is its heaviest target's rate, held for its whole run, so
  the answers are judged, as TCP Vegas judges its round trips, against **what the pool's jobs move
  on the NAS as measured** (each Mac's jobs' traffic over the answers' span, carried on its beats,
  weighed as a job is, `coord::pool_moving`): where the counted Mac's answers are its listings on
  the jobs' session, the others' alone, its own apart, as its own jobs' traffic stalls those
  whatever the NAS's load (above); where they are its probe's (`nas-apart-judges` on), all of it,
  its own too. Each ratio is kept, tagged with its source (listing or probe), with the uses it
  was read beside for `nas-track-minutes`; a growth cadence's (a minute) are judged against those
  of the same source a step lighter and a step heavier, never a listing's against a probe's.
  - While the counted Mac's own jobs move a step or more in the cadence and its answers are its
    listings on the jobs' session (always, with `nas-apart-judges` off), they can't tell
    the NAS from its link: **undecidable** (on 19 and 20 Nov, 57.5 % of the
    readings by the cadence, 34.5 % reading by reading; nearly all the mini's). They never halve the window and
    never let it grow past the knee. While they rise against those a step lighter in all the pool
    moves (its own too) it doesn't grow at all (what keeps the replay at the model's cliff no more
    often than the rule judging against the weight: without it, jobs of 100 on the mini reach it at
    7 readings and jobs of 250 at 8, against 5 and 5, never two in a row; the burst's H5 doesn't
    need it). Risen past 3 times their best, the window goes back to **the knee**: the heaviest
    weight in use read in the last `nas-track-minutes`, before the cadence, with answers under 3
    times their best; never under it, or halved where there's none.
  - Otherwise they are **rising** while their lower quartile is past the upper quartile of those a
    step lighter, and **not the pool's** (never while the counted Mac's own jobs move a step or
    more, its answers apart or not) while all the pool's jobs move under a step, or their
    median is within the upper quartile of those a step lighter, or those a step heavier were no
    slower. Between the two (the lower quartile past, the median not; a cadence is some three
    spans), neither: past the knee, held.
  - Not the pool's, slow answers neither cut the window nor hold it at the knee, but only with the
    disks read; nothing measured, they hold it, never cut it.
- **The window**: **congested** (the disks over the target, smoothed, or a reading's busiest disk at
  it or over: the smoothing lags a burst by half a minute; the counted Mac missing its deadlines;
  or the answers rising, past 3 times their best, `nas-slow`), it drops at once to half the weight
  in use (`nas-decrease`), never under a step, and nothing paced starts until enough jobs end; no
  further cut until the load has fallen to the window and a span has passed. **With room** (the
  answers read, not rising and under the knee, the disks with room for the next job), it grows
  while it's what limits: doubling a minute up to the threshold set at the last cut, then a step a
  minute, never more than one job past the load its readings tested, only on fresh readings from
  every Mac holding leased jobs, and only once the jobs last started have run a span (their
  answers in the reading) and, where the weight in use rose a step or more to the knee or past it
  (or no knee is known), a growth cadence more (their answers judged): fast where the answers were
  good at that weight lately, cautious beyond it. A knee read before an outside load rose stays
  trusted for `nas-track-minutes`, so the fast way can admit past the cliff then: up to 60 s more
  at it than the rule judging against the weight in the one-Mac step of 350, the disks read at
  half or not at all (a case built for task #1506's review, not among the tests). Otherwise it holds, and says why
  (`nas::Hold`). It is never more than the whole: the weights are estimates, so the answers may say
  the NAS is full before them, but past the whole they'd admit more than the probe measured. It is
  kept in the lead's store, so a restart within 30 minutes takes it up.
- **Ways past the window**: a job weighing under a step (`nas-small`) isn't paced, though never past
  the whole with the jobs out. A job alone goes whatever its weight, but not for 10 minutes after a
  congested reading (`nas-lone-minutes`), each hold in a row twice as long, to 80 minutes; a reading
  with room ends the hold.
- **The bounds are the pool's settings** (`nas::KNOBS`; `scenic pool set <name> [<value> | off]`,
  the build page's Settings): `nas-step` (50 thousandths, 10 to 500), `nas-small` (50, 0 to 500),
  `nas-decrease` (0.5, 0.1 to 0.9), `nas-floor-hours` (24, 1 to 24), `nas-slow` (3, 1.5 to 10),
  `nas-grow-seconds` (60, 10 to 600), `nas-read-seconds` (10, 2 to 60), `nas-span-seconds` (30, 10
  to 300), `nas-cached-ms` (0.5, 0 to 10), `nas-restore-minutes` (30, 0 to 60), `nas-lone-minutes`
  (10, 1 to 120), `nas-lone-most-minutes` (80, 10 to 240), `nas-young-spans` (20, 5 to 100:
  the spans of its link's history a Mac's floor needs; a Mac whose floor is younger may cut the
  window, never grow it), `nas-track-minutes` (10, 2 to 60: how long the answers are kept to
  judge against), `nas-apart-seconds` (5, 0 to 60: how often each Mac's probe apart answers; 0, no
  probe; `off`, back to 5) and `nas-apart-judges` (0, 0 to 1: 1, a Mac's probe answers for the
  pacing while it answers; 0, in shadow).
- **The probe apart down** (its ssh won't start; its key's agent doesn't answer within 60 s, as
  with 1Password locked or waiting for an approval; the file to read isn't there or a read comes
  back short of 4 KB; its connection ended; an answer past its 20 s deadline): its figures are
  missing and the lead flags it (`nas-apart`, §10); with `nas-apart-judges` on, that Mac answers
  with its listings on its jobs' session. Its ssh is tried again a minute on, each end in a row
  waiting twice as long, to 16 minutes; an answer in time ends the run. One try a wait at most,
  the wait kept while no build runs (the connection closed, the probe kept for the agent's run),
  so a locked key's agent never sees a stream of asks, however often builds start and stop. A probe past its deadline counts as the
  deadline, as a listing does.
- **The target** is the owner's: the disks' busy share the pool holds to, `nas-target` (30–95 %),
  set by `scenic nas-target [<percent> | off]` on any Mac; not set, 85 % (`DEFAULT_TARGET_PCT`),
  above the 65–80 % an outside repack alone kept the disks at, so the pool still works beside one.
  `scenic pool nas-probe [--dir <folder>]` runs a probe.
- **Enforced at grant** (`pick_every`, `pick_own`): a target is granted while every leased job's
  weight leaves it room within the window. **Aged targets** (held back for the budget alone for
  `OWN_AGE`, 10 minutes) get a way in: one weighing the whole goes once no leased job weighs the
  whole, and **an aged target holds its room** (`Shared::nas_reserved`): no other job weighing
  anything is granted unless it fits beside it, so a large target isn't starved by a stream of
  smaller ones. A step's grant restarts the waits of its other aged targets, so one step can't keep
  the reservation for good. **The lead's own jobs are always held to it**; the switch `nas-budget`
  (on unless `off`) holds the members' too.
- **The clean-up waits for a quiet NAS** (`QUIET_STEPS`: `gc`): it starts only beside leased jobs
  weighing nothing, or within the window, until it has waited 24 hours (`QUIET_AGE`); then it goes
  as an aged job. Unmeasured the way it runs now, it isn't held for a quiet NAS: it weighs the whole
  and goes beside no job weighing the whole, so its first run measures it. Running, it yields at
  its next safe point when an aged target would fit without it.
- **Inside a job** no allowance is handed out: each process holds its own reads in flight on the
  NAS to a limit it finds as TCP finds its window (`store::aimd::NAS`: doubled while latency is
  near its best, halved past three times it or on a timeout, 1 to 16), so processes and Macs settle
  into shares and a quiet one holds nothing.
- **Downloads:** a job of a fetching step (kind `Fetch`: the lead's `bld-fetch`, `rail-feeds`,
  `planet-fetch`) counts one, capped pool-wide by the switch's `downloads <n>`.
- **Seen**: the lead's waiting lines and `scenic status` say the pacing's last reading, the
  source judged and the counted Mac's probe beside it ("NAS pacing: window 55 % (threshold 5 %),
  50 % in use, …, answers on m6's jobs' SMB session 1.4× their best …; probe apart (in shadow,
  every 5 s, 1 connections, 0 past their deadline): 2.1× its best of 5.2 ms …: held: the window
  not what limits"; judged, "answers on m6's own ssh to the NAS (every 5 s) …; probe apart
  (judged, …)"; down, "probe apart (…): down: why"); the build page's NAS tab draws it (§10,
  Live): the lead's listing and its probe's reads under response time; under answers vs best, the
  judged ratio and the counted Mac's probe ratio, the counted Mac, its answer, best and connection,
  and its probe's ratio, best, role, connections and answers past their deadline. An ask granted fewer than it wanted while targets it
  would take wait for the budget says so first in its refusal.
- **Tested** on a simulated NAS (jobrt's `a_burst_of_nas_heavy_jobs_settles_below_the_knee` and its
  cases: outside loads, restarts, a slow link, lone jobs): held to a fixed whole, a burst sat at the
  cliff 1,475 s of 1,500; paced, never past the knee. **Replayed** on the lead's readings of 19 Nov
  12:22Z to 20 Nov 08:29Z (`the_readings_of_19_and_20_nov_let_work_go_and_cut_at_the_pools_load`,
  `nas_readings.csv`; the windows recorded were #1417's rule's to 19 Nov 23:06Z, then
  20261119-2306's), closed on the jobs it lets go, beside the rule judging against the weight (main
  at 6e92eb8bf, in brackets: `tools/dev/nas-replay-against.py 6e92eb8bf` runs this replay on it) on
  the same readings and model. The replay's cliff is the modelled disks at 100 %: it models
  neither the slowing the answers would show nor a Mac's own link's stalls, and it admits jobs
  more strictly than production (the window's sum alone, no aged or lone way past it).

  | | started | median window | at the floor | disks ≥ 85 % (most in a row) | at the cliff (most in a row) |
  |---|---|---|---|---|---|
  | the readings as they were | – | 177 (146) | 222 (215) | 26, 4 (26, 4) | 0 (0) |
  | jobs of 100, on a Mac not answering | 806 (685) | 361 (315) | 106 (63) | 57, 9 (41, 4) | 6, 1 (6, 1) |
  | jobs of 250, on a Mac not answering | 320 (342) | 403 (416) | 136 (81) | 49, 4 (76, 7) | 6, 1 (6, 1) |
  | jobs of 100, on the mini | 844 (685) | 381 (315) | 37 (63) | 63, 9 (41, 4) | 5, 1 (6, 1) |
  | jobs of 250, on the mini | 342 (342) | 423 (416) | 89 (81) | 65, 4 (76, 7) | 5, 1 (6, 1) |

  It never grows past the knee where the jobs it let go slowed the answers past it on their own.
  With jobs of 100 it starts 18 to 23 % more jobs than the rule judging against the weight; with
  jobs of 250 as many on the mini, and 6 % fewer on a Mac not answering. It sits at its floor more
  than that rule in three of the four closed-loop rows (106, 136 and 89 readings against 63, 81
  and 81), less with jobs of 100 on the mini (37 against 63). At the model's cliff about as often
  (5 or 6 readings against 6, never two in a row); at the disks' target at most about 1.5 times
  as often (63 readings against 41).
  As the readings were it is at its floor a little more (222 readings against 215); by the last
  cut's cause, 111 of them after cuts on deadlines missed (140), 57 on the answers (45), 31 on a
  disk reading at the target (the rule has no such cut; 7 of its floor readings followed a cut
  that fell on one) and 20 on the smoothed share (20). In 155 of the 222 the answers were
  undecidable too: a coincidence in time, not the cause. On 20 Nov 08:15-08:29 it held the window
  at 32 %, cut once (back to the knee), where the weight's rule went from 5 % to 22 % with 2 cuts.
  With the disks read at half the NAS's true load, or not read
  (`bursts_with_the_disks_read_low_or_not_at_all`), the burst's cases at the cliff (s; the
  weight's rule's in brackets, the same either way):

  | | paced | heavy | H1 | R2 | Q1 | one Mac, paced | one Mac, R2 |
  |---|---|---|---|---|---|---|---|
  | the disks at half | 100 (135) | 105 (260) | 145 (295) | 270 (300) | 225 (280) | 15 (135) | 300 (270) |
  | the disks not read | 100 (135) | 105 (260) | 140 (295) | 270 (300) | 225 (280) | 15 (135) | 300 (270) |

  The rows above are the pacing as it runs, the probe in shadow (`nas-apart-judges` off): on
  these readings and the bursts, line for line what main before the probe gives
  (`nas-replay-against.py main`, 20 Nov). With every Mac's probe judging (task #1552, `replay`'s
  `apart`): the readings hold no probe's answers, so where a Mac's own jobs moved a step or more its
  answer is modelled as the median of its own recorded answers at that tenth of the disks' busy
  share while its jobs moved under a step (`calm_ms`: the NAS's load without its own session's
  stalls). That models listings without their stalls, not direct reads, whose excess and floor
  differ (above), so these rows show what judging the counted Mac's own load does, not where a
  probe's knee lies: that waits on the signals above. Beside it in brackets, main before the probe
  on the same answers, taking them for listings on the jobs' session:

  | | started | median window | at the floor | disks ≥ 85 % (most in a row) | at the cliff (most in a row) | undecidable |
  |---|---|---|---|---|---|---|
  | the readings as they were | – | 224 (214) | 155 (168) | 26, 4 (26, 4) | 0 (0) | 0 (3,197) |
  | jobs of 100, on the mini | 806 (903) | 360 (406) | 47 (14) | 63, 4 (81, 13) | 5, 1 (6, 1) | 0 (5,328) |
  | jobs of 250, on the mini | 313 (345) | 389 (429) | 67 (36) | 79, 7 (92, 23) | 6, 1 (11, 2) | 0 (5,110) |

  Nothing is undecidable, against 3,105 readings of 5,400 with the answers as recorded (5,288 and
  5,091 with the jobs on the mini). Judging the mini's own load, it cuts on it: at its floor more
  often than main on the same answers in the closed loops (47 and 67 readings against 14 and 36),
  and with jobs of 100 the recorded answers' 37 readings; at the cliff less often (5 and 6
  readings against 6 and 11, never two in a row, where main reaches it twice in a row with jobs of
  250), and at the disks' target less often for longer (4 and 7 readings in a row at most against
  13 and 23). As the readings were, at its floor less often than with the answers as recorded
  (155 readings against 222).

  **Assumed:** a job adds to the disks' busy share what a thousandth of measured use added over
  those readings (0.046 %).

**One model per step** (`agentcore::needs::Book`; the model `needs::model`): only what was measured
the way the step runs now counts (`coord::cost_version`, the steps table's `cost_v`); runs and
floors of an older way are no measure (disk floors carry no version and always count). A target's
memory, disk and time are its own run's (memory never below its floor). Else a tile target's memory
(not unit's or pois', whose guess comes first: `GUESS_FIRST`) **by its place** (`needs::Near`): the
most its nearest measured targets of the step took (rings outward until they hold 5 runs,
`NEAR_RUNS`, or 8 rings), × 1.1, where that is judged good on the step's own measures (at most 5 %
predicted under, a median error at most 50 %): what a target holds is its place's, whichever Mac
runs it. Else, for z6 tiles and landmark candidates while their step has under `MIN_RUNS` (10) runs,
their extract-size guess; else **its step's needs** (below); else a cold start.

**What a step needs** (`needs::model::Model`): what a target needs apart from what its Mac supplied,
by the size of its input, so a prediction is its needs over a Mac's supply and crowding is an input,
not noise. A **run** is a target's cost line the way its step runs now whose job record passed its
plausibility checks and ended well. Each run is read by each of its input sizes (`sizes`, the
tile's area, one a target, and its source's time below):

- **work per unit**: its CPU in the lead's CPU seconds (by instructions where counted, else by the
  two Macs' speeds), over its size; a target's is the `needs-work-pct` percentile (40) of its step's
  runs of about its size: crowding only adds time, and one freak can't set it.
- **the rest of its time per unit**: its wall time at the lead's pace less its work over the
  threads it could use (its waits, serial stretches), at the same percentile.
- **parallelism** (`model::Par`): a run that kept every thread busy while not waiting is a floor;
  one that didn't says what the step can use (their median); waits left out, so a step crowded off
  the NAS doesn't read as serial.
- **memory**: the `needs-mem-pct` percentile (98) of the peaks of its step's runs at or above its
  size (up to twice it; fewer than `MIN_RUNS`, the nearest such; past every run, a smaller run's
  peak grown by the step's slope over size, `model::slope`, at most in proportion; no slope, and no
  growth, with fewer than `MIN_RUNS` runs, their sizes all equal or a falling fit: task #1535),
  each at the threads asked: past a run's threads, the step's memory per thread for each more
  (fitted by least squares over size and threads, at least `needs-thread-share`, 50 %, of the
  peaks over threads; more threads never read as less memory). Of every run the way the step runs
  now: a small new run can't lower a large target's. Disk alike, at `needs-high-pct` (95). Both
  are read by the target's best size and by each other size it is past every run of whose reads,
  taken with the best size's, read the step's own largest runs by it nearer than the best size's
  alone, the most of them (`model::Model::units_of`; `model::Past::beaten`: the `MIN_RUNS`
  largest by a size each held out and read from the runs below them, the log errors summed;
  worked out per size the first time a target past its runs asks, so a model's build costs
  nothing more; task #1542: bldprep 6/47/27, in range by its bytes but past every run by its
  buildings, had been read by its bytes alone, 517 MB against 10,770).
- **NAS** (`model::Moves`): bytes read (the NAS or the page cache), written and read back, and its
  creates, lists, stats and deletes; a target's rate its runs of about its size together.

"Of about its size" means half to twice its units (fewer than `MIN_RUNS`, the nearest). A target is
predicted by the first of its step's sizes, best first (the least spread of work per unit), that
both it and some run have. **Its source's time** is one such size (`needs::SOURCE`): the time the
same target of the step it reads took (slope-deep's terrain-deep): a slope-deep target whose
terrain-deep took under 2 s took a median 0 s, one whose took 64–127 s a median 57 s. Its time on
`threads` on a Mac at `r` times the lead's time is work × units × r / min(threads, parallelism) +
rest × units. An update's drain waits by the `needs-high-pct` percentile instead. A step's times
learn from its newest `needs-runs` runs (500). While the way a step runs now has under
`needs::PRIOR_RUNS` (20) runs, its earlier way's work and rest stand in where larger, so they can
only lengthen a prediction.

**Prediction against what came** (`model::Off`): each run of the window is predicted from the runs
before it of about its size; off by more than `needs-off` (3) times on its work, or on its time past
what its supply explains (the cores it lacked, the NAS's slowness: `jobrt::timings::explain`), it's
flagged. `needs-relearn` (5) runs in a row off on one side, and the step learns its times from the
first of them, as a cost version's bump does for code. `scenic pool needs` prints each step's runs,
sizes, rates, memory, parallelism and flagged runs.

**The knobs** (`jobrt::needs::KNOBS`): `scenic pool set
needs-work-pct|needs-high-pct|needs-mem-pct|needs-thread-share|needs-runs|needs-off|needs-relearn
<number>|off` or the build page's Settings ("Needs model"); not set, their defaults. The model's
form is fixed: "about its size" as a doubling either side, `MIN_RUNS` 10, `PRIOR_RUNS` 20,
parallelism as a median. A step whose time is the work its target finds (`needs::PER_WORK`:
`pyramids-fill`, by the tiles it fetched) counts only its runs that did work.

**The held-out test** (`needs`' `backtest_needs_on_the_leads_records`, by hand): on the lead's
records of 19 Nov, a book of the runs before each day from 12 Nov predicting each target granted
that day (7,396 targets), against the cost book the model replaced. Time, the median |log(predicted
/ took)|: 1.05 before, 0.72 now; memory predicted under what was taken: 6.2 % of targets before, 3.1
% now (by over a GB 1.1 % and 0.4 %); NAS bytes' median |log error| 2.36 and 1.62. Worse now:
slope-deep's time (most of its targets' sources ran the same day, unknown to either book) and
trees' time. The defaults were picked by the same test.

**Memory admission** (`coord::sched`): a job is admitted and packed at its memory by the needs model
on the threads it starts on, and counted, once grown, at that plus each thread past them. Memory is
the cautious limit; the memory guard and brake (§7.1, The memory floor) stand behind it.

- **On its threads** (`Sched::measures`): each target's memory (`needs::Book::mem`) is read on the
  threads a job of it starts on (its grant by the cost book), whatever cores the Mac has, so a
  target's memory is one figure everywhere: what `pack` packs, what room is held for it, what a
  Mac's last ask is weighed by, what flags it too big. A target with its own run: that peak plus
  its memory per thread past the threads it ran on; else its floor or its place's figure, grown to
  those threads by its step's model; else its step's model.
- **A job's targets together** (`Sched::job_need`): a job is admitted at the most of its targets'
  memory, all on the threads the most of them starts on, and targets are added only while that
  fits the room.
- **Grown**: its grant says what each thread past those its memory covers adds (`Grant::thread_mb`,
  `Grant::thread_covers`); its agent grows it only within the Mac's room by that (§7.1, Threads
  controlled live), and the lead counts each thread grown at it, so a grow never takes room a grant
  beside it was admitted in.
- **Held**: the lead counts a job it holds at its grant's figure; a job resumed on the Mac holding
  its progress is granted, admitted and counted on the whole Mac's threads.
- **When the memory guard acts** on a job holding past the memory it was admitted at, its estimate
  is flagged in the act's words ("its estimate flagged: it held past the 4.0 GB it was admitted
  at"); its give-back's floor raises its targets' memory from then.

**The admission replay** (`needs`' `replay_admission_on_the_leads_records` and
`replay_admission_packed_into_rooms`, by hand): 2,985 jobs of five days to 19 Nov, admitted as the
model admits them and packed into each Mac's room (its memory less its floor), against the most
their targets took. Reserved: each job's figure over its time; taken past it: its peak past its
figure over its time; waited: hours waited for room past their grants.

| `needs-mem-pct` / `needs-thread-share` | Reserved (GB h) | Taken past it (GB h) | Jobs past it | By over a GB | Waited (h) |
| --- | ---: | ---: | ---: | ---: | ---: |
| the scheduler before | 467.4 | 3.97 | 53 (1.8 %) | 7 (0.23 %) | 265.0 |
| 95 / 50 | 400.1 | 4.00 | 35 (1.2 %) | 7 (0.23 %) | 195.8 |
| 98 / 40 | 436.6 | 1.56 | 27 (0.90 %) | 3 (0.10 %) | 240.6 |
| **98 / 50 (the defaults)** | 460.2 | 1.54 | 22 (0.74 %) | 2 (0.07 %) | 257.6 |
| 99 / 50 | 504.9 | 1.16 | 19 (0.64 %) | 2 (0.07 %) | 293.5 |

At the defaults the jobs waited 3 % less for room than before and took 39 % as much past their
figures; no step had more jobs past it, and in none did the jobs together pass a Mac's room.

**Each Mac's speed** (`crate::paces`): each Mac's time against the lead's, from the lead's speed
over its own on its performance cores (`jobrt::timings::Speeds`, §10, Each Mac's speed); a Mac not
measured is taken at the lead's. The model reads every run at the lead's pace and predicts for a
worker at its Mac's (`Book::secs_for`, `nas_for`), in the cost book, the scheduler, the forecast and
the plan's lanes alike.

**Batch length** (`Sched::batch_len`): a job of a shared step takes its targets in plan order (its
first whatever its time) while their predicted time on the asking Mac stays within the batch length:
the pool's setting `batch-minutes` (`scenic pool set batch-minutes <minutes>|off` or the build
page's Settings; a minute to a day; not set, 15, `coord::JOB_S`, the owner's choice). At
slope-deep's 5 to 12 s of start a job, a quarter hour spends about 1 % of a job on its start; a
longer batch saves little and holds more targets behind one lease when its Mac pauses or goes away.
A job's time is its Mac's jobs' start at the step (`Paces::with_starts`: the median of a job's time
past its targets' runs) plus each target's time on that Mac at the threads it would be granted: its
own run (on its Mac as it ran; another Mac's by the two Macs' speeds, twice its time where a speed
isn't measured), else its step's model. A target's own run is read by one rule everywhere it's read
(`needs::own`). The grant's log line and the history's lease event say what it was sized to and how
("sized to 14.8 min at its Mac's speed"), and placement's why, and, with the NAS budget on, the NAS
weight it was granted under (`nas_w`, `nas_measured`: `sched::Pick::nas`); the job's page shows
them. The
forecast offers an ask at most the first 64 ready targets of a step (`forecast::OFFER_MAX`) where
the live coordinator grants up to 256 (`JOB_MAX`): a longer run of tiny targets is one job live and
several in the forecast. 64 keeps the forecast's CPU in its budget (256 took 463 s of CPU on the
live-scale test against 135). `scenic forecast-check --replay`'s last table holds each job's time
against what it was sized to, by step and Mac.

**A cold start** (no target of its step measured the way it runs now) is packed by its
*provisional need*: the most its step's runs of its latest earlier way measured (or its offered
guess: unit's and pois' extract size), times 3/2 (`coord::sched::provisional_mb`), its disk its
step's measures'. With neither, it runs alone (`Sched::no_figure`). Its cores: one thread when its
step honours its threads (`coord::THREADS_HONOURED`: `slope-hi`, `slope-deep`, `terrain-deep`,
`bld-fetch`, `pyramids-fill`; its agent grows it from there), else the whole Mac. It goes beside
other jobs when it fits; at most one cold start runs on a Mac, one job of the step in the pool at a
time (so the step is measured before it fans out), and a step never measured in any way runs alone.
A cold job holding past its provisional need counts at what it holds as soon as the guard samples
it. **A records-only step** (`records_only`: `redo`) is packed at one thread, no cores and no disk,
its memory measured. A floor never stalls the build: the first slot starts once room-making has
done what it can, the disk guard as backstop.

**A version not raised.** A cost version is raised by hand with the change that moves a step's
needs; it can't be derived from the step's key version (which moves with its outputs) nor a digest
of its code (nearly every release would make every step a cold start). So the lead watches: a
target's rerun of the same way on as many threads notes its run before's peak (`Cost::was_mb`). One
off by more than `needs-off` (3) times, where it matters in packing, is counted; a step with
`needs-relearn` (5) or more such on one side, and most of its reruns, is flagged `cost-version`
(§10, Flags): raise the step's `cost_v`.

### 7.8 The agent never waits on one NAS call

An open on the SMB share can stay stuck in the kernel for many minutes while the same file reads at
once from a shell. Every NAS call of the agent is a store::deadline call (docs/nas-access.md §6): on
its loop's root and in `crate::agent::nasread`, waited for 20 s at most (`deadline::SHORT`; the
planner's keys 5 min); on threads it starts and never waits on, 120 s. Past it, the caller gets a
timeout and takes it as a failure now, logged once per path. The call is left to the kernel; past
the process's cap of such calls held ("NAS calls held", the build page), a call fails at once, so
stuck threads never pile up. A write to a path is refused at once while an earlier write to it is
held in the kernel: a late write would land over the newer. A healthy share's calls are unchanged.

The agent is in parts (docs/agent-parts.md), each on a thread of its own, so no slow duty holds
another: **control** takes the owner's asks, **beat** writes the heartbeat, **nas** owns the mount,
**jobs**' thread reaps and cancels, the lead's **plan** plans (crate::agent::plan), and the round
(`Agent::round`: slots, guards, the pool's step, grants and starts) never plans: it grants from the
last plan. A round or a plan past `beat::LOOP_STUCK` (5 min) is a stuck loop in the status; a plan
past it is sampled (`/usr/bin/sample` for 5 s into `plan-stall-sample-<UTC date>-<time>Z.txt`,
the newest five kept), and the status says "planning has taken N min so far" while grants go on
from the last plan.
The round and the plan log each phase that took 10 s or more.

The heartbeat (the agent's `status.json`, a member's `helper.json`) is written by the beat every
10 s and at once on a change, from the view the round last published, so a long round or a hung
share leaves it beating; its `loop_age_s` says how old the view is. It is the only writer, each
write a whole file put in place. No copy goes to the NAS: the lead's is asked of its coordinator. A
heartbeat goes out of touch after ten minutes without a beat (`OUT_OF_TOUCH_S`). A member's beat
goes to the lead (`POST /pool/beat`, docs/coordination.md §5.1).

The NAS's mount and probe are the **nas** part's (crate::agent::nas): each look on a worker waited
for 30 s; past it the round says "the NAS is mounting (stuck since …)", only NAS work waits, and the
loop, beat and control go on. The plan's calls are each within their deadline, its last plan the
one granted from meanwhile.

### 7.9 The forecasts' archive

Every forecast the lead makes is kept, so its times can be held to what happened
(`agent::fcarchive`; the record in docs/formats.md, The forecasts' archive):

- **What:** a line per forecast: when it was made and its finish, every target it placed (lane,
  step, target, from and until), and what shaped it: each lane, each Mac's room and conditions,
  each step's time a target, the NAS budget, `forecast::VERSION` (raised whenever a change moves
  the run's times; called its code below) and the app.
- **Where:** `state/coord/forecasts/<day>.jsonl.zst`, one file a UTC day, appended in batches, each
  one zstd frame, by a thread of its own (never the pool's step or the agent's loop: a full queue
  drops the entry, said). A reader skips a frame that doesn't read, so a crossed append costs its
  frames, never the day.
- **Not twice:** a forecast the same as the last one written (its times in 2-minute buckets,
  `fcarchive::GRAIN`) isn't written.
- **Kept for good:** the `gc` job packs each day the history's window no longer holds (7 days) into
  its month's tar, `state/coord/forecasts-archive/<YYYY-MM>.tar`. A day is ~4 MB, up to ~19 MB:
  1.5–7 GB a year.
- **Joined with the history** by step and target: a forecast's target is held to the first job of
  that step and target that ended after the forecast was made; a lane's name is the history's
  worker.

**`scenic forecast-check [--from T] [--to T] [--replay [--split T]] [--json] [--root DIR]`**
(`agent::fccheck`, reads only) holds the forecasts made in the span (default the last 7 days) to the
history's jobs that ended in it. Each (forecast, job) is a sample: its end's error, and, where it
started after the forecast, its start's and its run's (the end's error is the two summed); error is
forecast less actual (+: the forecast was late). It reports, for the end, start and run, n, bias,
and the median and 90th percentile of the absolute error, overall and by horizon (under 15 min, to
1 h, 4 h, 12 h, more), step, Mac and code; how often it had the right Mac and slot; the share of
job pairs started in the forecast's order; the targets forecast done that weren't; and the work a
forecast placed in its first 6 h against what the pool did then.

**The forecast inputs' archive** (`agent::fcinputs`; docs/formats.md, The forecast inputs' archive):
the forecast's whole input (`forecast::Record`, §10), at most one every 15 minutes
(`fcinputs::EVERY_S`, the shortest horizon reported; a replay costs ~9 s an input), in
`state/coord/forecast-inputs/<day>.jsonl.zst`, appended once an hour (one frame's long-distance
matching stores an input mostly as what changed: ~23 MB a day). Kept 7 days (`fcinputs::KEEP_S`).
Inputs before it are the old terms' `state/build/term/<E>/forecast-input.json` (to 31 Oct).

**The backtest** (`--replay`, `fccheck::backtest`): each input in the span is rerun twice, as the
code before made it (old) and as the code is (new), each archived and checked as above.
`forecast::BEFORE_HAS_PATH` is false: the app keeps no path for the code before (codes 9 and 10
replaced their predecessors' ways), so old is new and the report says so; the code before is then
measured by an earlier app's replay. The inputs from `--split` on (default a day before the last)
are the test set, scored apart from those before (train): what the model was chosen on is never what
it's scored on. Run it on a copy of `state/coord` and `state/build/term` (`--root`), or on the NAS
(reads only). A term's file last written over an hour before `--from` holds no input in the span and
isn't read (`fccheck::inputs`; the hour for the clocks). Where its time went is said on stderr, the report on stdout as before: the
history read, the inputs read and the backtest, and inside it the decodes, the inputs made ready,
the forecasts, their check and the jobs' lengths (`fccheck::Spent`; `spent` in `--json`).

**The run as it is** (code 10): the forecast packs with the live scheduler (§7.2, One scheduler),
each Mac at its sustained best, a target waiting through the plan's waits, the NAS window as
recorded while the jobs out run. Each job is timed by its needs over its Mac's supply
(`forecast::needs_secs`), its start at the step on its first target; a job under way by its
targets' needs less how long it has run (`forecast::lease_busy`); each step's pace against its needs
(`forecast::needs_pace`: its jobs of the last 12 h over what their grants were sized to, said only
for a step quicker than its needs, never under a quarter); placement where it finishes soonest; and
the room held for a waiting job timed as the live scheduler times it, by each lease's time left by
its progress and the discounts the lead took (`Leased::left_s`, `State::lefts`), both in the record;
a record from before code 10 has neither, so its leases end by their measured times and slower runs
alone. Code 10 changed only that timing and wasn't scored on its own.

The score as code 9 came in, on test (17 to 19 Nov, 135 inputs, ~32,200 samples), against the app
before it (code 8):

| test (minutes: bias, median / p90 of the absolute error) | code 8 | code 9 |
| --- | --- | --- |
| end, all | +97, 187 / 632 | +83, 189 / 610 |
| end < 15 min | −151, 92 / 406 | −153, 93 / 408 |
| end 4-12 h | +138, 231 / 450 | +168, 261 / 526 |
| end 12 h + | +683, 647 / 1,197 | +617, 640 / 968 |
| run, all | +36, 1 / 98 | +35, 1 / 103 |
| the right Mac | 45 % | 45 % |

Code 9 is kept for one needs model across grants, placement and the forecast, not for accuracy,
which is mixed. What limits the forecast, measured: the near term's error is which targets start
next, not how fast a Mac runs them (the run's own error stays under 10 min at p90); about half of
the lanes' time in a forecast's first hour goes to work its input didn't hold yet (the plan's work
to come), and the far horizons stay late: work the pool did in a day was placed later.

**Where the forecast's time goes**: a run asks the scheduler some 50,000 times, 88 % refused (an
idle lane asks again at every item's end). So the cost book keeps each step's model, each target's
source's time and NAS rate; the scheduler keeps each leased job's weight, need and time and, within
one `pack`, the NAS sums; the run keeps each Mac's offers until the ready work changes; maps use
FxHash. Every answer is the same bytes as computed afresh. On 7 Nov's input (19,365 items, 30 lanes)
a run without a budget takes 23 s of wall (607 s looking each up again at every ask); over 64 inputs
no run is cut at `BUDGET` (the slowest a median 6.5 s, at most 12.3 s). The listing of the work,
once for the three runs and outside the budget, takes a median 13 s.

## 8. Tasks

A job is a step's run over targets on a Mac with the NAS. A **task** is a pure piece a running job
offers: programs over a folder of files, giving files back (kinds and protocol: coordinator.md §3,
§5). Any member's idle slot takes it (`scenic run-task`). Pages take none (coordinator.md §5).

- **Brokers** (`crate::agent::brokering`, `crate::coord::brokers`): each member's agent runs a
  broker for its jobs' tasks (`coord::Broker`, docs/agent-parts.md §9.3, on the pool's port; jobs given
  its address as `SCENIC_COORD`; its answers say epoch 0). Its heartbeat's `broker` gives its
  addresses. A task lives as long as its job, whoever leads.
- **Cross-member calls:** members allow each other's page origins (CORS, with preflight): every
  address a member's page is served at (`coord::brokers::origins_of`), from its heartbeat's
  `broker`: its tailnet address, LAN name and addresses, HTTPS page (`tailscale serve`), MagicDNS
  names, and the public names a proxy on it serves its page under (its broker's `pages`, from
  `~/.config/scenic/names`: the M6's https://map.brandontsang.net/build). A loopback origin is a
  Mac's own page only from that Mac. So a member's page reads the others' live charts and settings
  (docs/pipeline.md §5). HTTPS needs `tailscale serve` on each Mac (§14). Chrome's Local Network
  Access may still block calls to another member. The agents' token is the pool's
  (`state/coord/token`).

## 9. The pool's API

Every Mac's agent answers on port 8090: the lead's coordinator, a member's broker. A member talks to
the lead alone, by HTTP (the calls, their timing and what a member keeps until the lead has it:
docs/coordination.md §5):

- **Its beat** (`POST /pool/beat`, every 10 s and on a change): its heartbeat and status (§10), its
  leases (docs/coordination.md §6), its hand-offs' keys (§7.3), its capacity and resume progress,
  and its jobs' spans and logs the lead lacks. The answer carries the lead's epoch, who leads
  (`leading`), the leases ended, the others' heartbeats, this Mac's hold, the settings' generation,
  the hand-offs merged and those the lead lacks.
- **Jobs** go through the lead's coordinator (`/work/*`, coordinator.md), tasks through each
  member's broker (§8), the records' brief by `GET /pool/brief` (§6.2).
- **Its owner's asks:** job cancels through `/work/cancel` (§11); another Mac's pause and a pool
  setting through `/pool/hold` and `/pool/settings` (crate::coord::asks), queued on its disk and
  sent with its beat; a page's retry by mail (§9.1).

A member that can't reach the lead keeps its jobs running and starts nothing new
(docs/coordination.md §7).

### 9.1 Members' mail

**What goes by mail** (`crate::pool::driver::Msg`): to the lead, the build page's `Cancel` and
`Retry` a member passes on; from the lead, none (what it tells a member is in its beat's answer).
A message an app doesn't know is read past. Each sender numbers its messages and keeps its last 64
to each member (`mail.json`); a recipient's read marks are in its own folder, so each is taken once.

**To the lead**: `POST /pool/mail` on its coordinator, `{"from": <member>, "to": <member>, "msgs":
[[<n>, <Msg>], …]}` (at most 64; `crate::coord::inbox`).
- **Auth**: the build's key, sent only to the lead's tailnet addresses (never `.local` or a LAN
  address, which any device on the network could claim). The lead takes a sender only once its
  heartbeat is heard, so no post makes up a member.
- **Taken only by the member it's to**: 409 unless `to` is the coordinator's own member.
- **Durable before the answer**: the messages go into the lead's store (`inbox/<member>`) and are
  on its disk before the 200 `{"took": <n>}`.
- **Read once**: the lead's agent takes each sender's messages in order, each number once.
- **The member** posts each message with every earlier one not yet taken; anything but a 200
  leaves them queued for the next.

## 10. Seeing the pool

- **Heartbeat**: label, app, resources, conditions (`conds`: home, power, battery, paused), slots'
  jobs, tasks, addresses (`broker`), reach (§6.8), the epoch it leads in. Sent in each member's
  beat to the lead every 10 s (§9), which answers with the others'; a member not heard for twenty
  minutes is forgotten. Nothing on the NAS (`crate::coord::beats`).
- **Build status**, written by the lead (its agent's `status.json`, asked of its coordinator by
  other Macs): the checklist (docs/pipeline.md §4), forecast, regions, waits, the map updates'
  state (`publishing`, `published`), recent jobs, pause, the lead. A member writes `helper.json`
  instead and sends its status in its beat.
- **Flags** (`agent::flags`): what needs a look, worked out by the lead each round, in its status
  (`flags`: kind, Mac, a line) and its beat answers, so `scenic status` ("Flag:" lines), the build
  page and the menu bar show the same. Nothing is flagged while the build or that Mac is paused.
  - *idle*: a Mac whose last ask was refused (a slot free) while work it could take waits. The flag
    says how many targets of which steps, and the refusal: what in its capacity held the job back.
    Refused for room, it names the first target past it, what it needs and what the Mac is short
    of ("lo 3/6/3 needs 12.7 GB of memory and 35.2 GB of disk: disk short by 19.2 GB (…)"), then
    how many other targets are held by jobs, done, backing off, or held otherwise. With nothing
    free but work held behind the plan's waits, it names those waits.
  - *stuck*: a leased job whose work hasn't moved (no change in its progress line, target, phase,
    NAS bytes, files or internet bytes between its beats) for more than the overdue factor times
    its step's usual stillness: the 90th percentile of its jobs' longest still stretch
    (`History::still_p90s`); a step with none measured is held to its target's predicted time. A
    job is held to what it's doing, not to how long its targets took before: a slow copy that moves
    isn't stuck.
  - *slow*: a leased job whose progress line moved, over its window, at less than its step's usual
    rate divided by the overdue factor (`flags::slow`). The usual rate is the 10th percentile, in
    the line's unit, of its step's jobs that ended well with no more threads; its window the factor
    times its step's usual longest wait for progress. Needs ten such jobs (`history::ENOUGH`); a
    job flagged stuck isn't also slow.
  - *stale*: the lead's map update due for more than the overdue factor times the most the plan
    holds one (11 min), with no `catalog` job leased.
  - *repeat*: the lead's newest run of GC or of the backup ended having done nothing, less than that
    duty's period after the run before (`flags::REPEATS`): a duty planned again with nothing to do.
  - *refused*: targets whose hand-offs the merge refused twice in a row or more (§7.3), one flag
    per step, until a hand-off of it is merged or the plan offers it no more.
  - *too-big*: offered targets whose measure fits no Mac that runs the step even at its best (its
    memory less its floor, its disk for jobs with what reservations and caches can free:
    `Coordinator::too_big`). No packing grants these and no room is held, so they would wait
    unsaid. It stands until a Mac with room asks, or the measure or plan changes.
  - *cost-version*: a step whose reruns moved their memory past `needs-off` times without a cost
    version raised (§7.7, A version not raised), one flag per step and side.
  - *nas-apart*: a Mac whose probe apart on its own ssh is down, with why (its key's agent not
    answering among them; §7.7, The probe apart down): its figures missing, and with
    `nas-apart-judges` on its answers its listings on its jobs' SMB session; none while
    `nas-apart-seconds` is 0.
  - The factor (stuck's, slow's and stale's) is the pool's setting `overdue-factor` (`scenic pool
    set overdue-factor <times>|off` or the build page's Settings), 1.1 to 100; not set, 3
    (`OVERDUE_DEFAULT`, the owner's call).
  - `scenic status --flags [--since <unix s>]` prints what needs a look alone: the flags, the
    pool's warnings (no lead, a Mac out of touch, the lead's loop stuck) and the jobs failed since
    (the last 6 hours by default): tools/dev/health.sh's.
- **A job's progress** in its status (`JobView::progress`): a job of several targets says its
  batch's (its targets ended plus the share of the one under way) and its time left at the batch's
  pace; a job of one target, that target's own line.
- **Coordinator view** (`Coordinator::view`): the view the build page's `/api/swarm` takes from the
  coordinator's memory: the pause, each lease, each worker, the tasks by state. Times are unix
  seconds from one anchor, so an idle pool doesn't rewrite it. Served by `POST /pool/view`.
- **Forecast input** (`forecast::Record`): the forecast's whole input as data (the work left, each
  target's cost by step, the lanes, what runs now, each Mac's room, the scheduler's state), so
  `Record::forecast` gives the lead's forecast on any Mac. Made with each forecast (at most once a
  minute), zstd-compressed past 2 MiB (~11 MB of JSON, ~0.9 MB compressed); every time and speed
  made finite before it's written. Handed to the inputs' archive (§7.9); the last ones before it are
  the old terms' `state/build/term/<E>/forecast-input.json`.
- **The build page** is the lead's alone (`coord::memberpage`, task #882). A member's port (its
  broker, or a viewer's) answers the page, its files and its data and asks (`/api/swarm`,
  `/api/history`, `/api/pause`, `/api/act`, `/api/settings`, …) from a page of its own origin or
  none with a 307 to the lead's same path and query, at the lead's address of the network the
  request came by (`memberpage::location`: its HTTPS page through `tailscale serve`, its tailnet
  address, its LAN name `<host>.local`, else its first). Answered there still: `/api/live`,
  `/api/settings` and `/api/asked` asked by the lead's page (CORS, Cross-member calls, §8), the jobs' calls, and
  `sw.js`; the build's own asks from any other origin are refused. It redirects only to a lead whose
  beat answer says `page` (`beats::PAGE`), heard within 35 s (`beats::PAGE_AWAY_S`); else the port
  answers `/` with a small page of this Mac's own status and "The lead (<name>) isn't reachable
  since <time>", refreshed every 10 s, and the data calls 503. The agent builds the page's views
  only beside its coordinator; a member's port is told where the page is each loop
  (`Agent::member_page`).
- **The page's data on the lead** (`agent::pageview`; a member's port sends the page there, The
  build page, above): the lead's port answers `/api/swarm` (and `/api/steps`, the steps table), the
  forecast, `/api/tocome` and `/api/history` from its coordinator's memory. While a page asked in
  the last 60 s, a thread asks the lead's view every 2 s; with the lead down, the last view is kept
  and the answer says its age (`view_age_s`). No NAS read on any page's path. Until the first view a
  Mac answers 503, which the page shows as reading, not an error, for a minute. The swarm's forecast
  is the lead's agent's own, handed to the coordinator with its tiles to come and waits under one
  lock (`Coordinator::hand_forecast`), so the page and the status show the same one, about a minute
  old. A member with neither coordinator nor broker answers from a viewer (`Coordinator::viewer`):
  its live store, settings and hold, every other call refused.
- **The page's history** is the coordinator's, in memory (`coord::history`): the week's events
  (`history::KEEP_S`), each without its `data`. Measured on a copy of the lead's file: ~8,300 events
  a day, ~55 MB in memory for the week; the file read once as the coordinator starts (0.24 s for
  39 MB), rewritten with the week's lines once it holds twice as many. `/api/history` takes events
  after a number by binary search. One numbering, the coordinator's, going on across its
  restarts, so a page keeps its cursor over a lead's restart. Older history is in the NAS's archive
  (§7.5), which the page doesn't read.
- **Phase shares**: for each step, its top-level phases' mean share of a run's wall time and of the
  bytes its phases read or wrote, over its last 10 good runs (`timings::SHARES_RUNS`), in the lead's
  status as `phase_shares`. Only the lead holds the build's `coord/timings.jsonl`; its coordinator
  reads the lines added each minute on a thread of its own (`coord::phase_sharer`).
- **Each Mac's speed** (`timings::Speeds`): the instructions its cores retire a second of their CPU,
  performance and efficiency cores apart, from the build's records alone (each worker's newest 20
  clean runs a step). Each round, each core kind's speeds are fitted as `ln speed = Mac + step` by
  least squares over every (Mac, step) measured, so any step two Macs both ran ties them and a Mac
  running heavier code isn't the slower for it. In the lead's status as `speeds`; the Machines tab
  shows each Mac's.
- **Time by phase**: each step's runs summed by when they ended (`timings::TIMES_EDGES`: 60 s to 7
  days), so the build page's step rows break a step's time down over the range its footer picks; in
  the lead's status as `phase_times` (`timings::PhaseTimes`), at most 8 phases a step.
  **Measured waits, not classes**: every NAS call, network request, local-disk read and wait on
  other work holds a store::waits guard, and each phase span takes the totals, by the job's wall
  clock (an instant is the NAS's when any thread is in a NAS call, else the network's, else the
  disk's, else other work's, else computing), so a phase's parts add up to its wall time. The page
  splits each phase's bar so. **What ran alongside**: the seconds two kinds ran at once on parallel
  threads (`Waits::both_s`), drawn as a strip under the first kind's segment, never in the bar's
  sum. A guard costs ~620 ns. Phases are named as doings; a renamed phase's kept data is mapped to
  its new name by step (`jobrt::phase_names`), and each line of the page's phase descriptions is of
  a phase some step emits (crates/jobrt/tests/phase_names.rs).
- **Each Mac's jobs** (the build page's machines, `scenic status`): a block per Mac, the lead's
  first (with its ramp, §7.1): its capacity as last heard (memory against its floor, cores, disk); a
  block per running job by its real slot; and (`scenic status` alone) why nothing more runs: the
  packing's refusal of its last ask (`Coordinator::refusals_with`: a cold job waiting for room,
  cores short, a job refused for its room, the targets offered all held, done ("1 done (its record
  not read yet)") or backing off, steps offered but not to this Mac, or nothing offered), worded
  with the plan's waits; only with none of these, "the plan is complete: no job is left to offer".
  Shown while the refusal is younger than two minutes. The forecast's lanes are per slot (`<host>`,
  `<host> #2`, …), as many as the slots each Mac may run now; each lane's next job is the
  scheduler's grant to its Mac (§7.2, One scheduler), and each lane's runs are cut into the jobs as
  granted, so the page draws every job to come as a block of its own.
- **Regions** (`agent::regionsview`): each enabled region's `crumbs`, its `bytes` (the map's files a
  download of it copies) and its outline for the world maps (`fill` and `edge`, in Web Mercator,
  simplified to about 7 km), made by the lead on a thread of its own once per regions' version and
  map. `POST /api/regions` (no key) answers from it; a page that has the outlines of the answer's
  version gets it without them (0.1 MB against 3.1 MB). Until there is one it answers 200 with
  `{making: why}`. With `{"steps": true}` it answers each region's targets built of all by step
  (`build::region_steps`), counted while a page asked within two minutes.
  **To come**: the lead's port answers `POST /api/tocome` (no key) with every Mac's tiles in its
  last forecast (`forecast::ToCome`: `{at, macs: {mac: {step: [[z/x/y, s from at]…]}}}`), the
  schedule's runs, jobs and dependencies (`deps`: [a, b] when an item of job b waits on one of job
  a), and `after` (the history's event numbers of the finished jobs each job waited on, kept on the
  lead's disk in `coord/history-waits.json` so a restart still names them). Gzipped to a client that
  accepts it. Handed with the forecast under one lock, so its `at` is the swarm forecast's; the
  coordinator keeps the last and the one before. Asked once a forecast.
  `POST /api/built` (no key) answers the map's finished tiles: every tile target the build's keys
  record, by step, `empty` (targets whose files the manifest doesn't name), `dropped`, and
  `published` and `catalogue` once the agent's view of the map has read the map's log.
  `POST /api/catalogs` (no key; `agent::cataloguesview`) answers the map's age controls: each batch
  of the map's log (docs/formats.md, The map and its log) with the tiles it changed and dropped, and
  with `steps` each tile grouped by its step's newest publish. The agent follows the log on a
  thread of its own and keeps its view in `catalogues-view.json.zst`; `scenic catalogs [--after N]`
  prints it. The page's own files carry the countries' outlines and large lakes for the same map
  (`countries.json`, `lakes.json`, made by `tools/worldmap/countries.py` and `lakes.py`).
- **Live** (`agent::live`): each Mac's own measures over time, in rings on its own disk (`live.bin`,
  never the NAS): a point a second for an hour, every 10 s for a day, a minute for three days,
  every 5 minutes for seven, each point the least, mean and most of its samples; each whole UTC day
  archived once to `state/live-archive/<host>/<day>.bin.zst` (≤ ~0.8 MB), kept for good. Sampled on
  the agent's own threads, never its loop: every second the load, memory (available as the floor
  reads it, compressed, swap, pressure), cores busy (performance and efficiency apart), the build's
  CPU and memory against the rest of the Mac's, disk writes, thermal state and the sampler's own
  CPU; every 10 s disk, power, the owner's idleness and the build's limits; every 5 s while a build
  runs here, the NAS's answer time (`nas_ms`: a timed listing of the project folder, as a stat
  would come from the SMB cache), and every `nas-apart-seconds` the probe apart's
  (`nas_apart_ms`: a 4 KB read past the NAS's cache on an ssh of its own, §7.7).
  - **The jobs' NAS traffic**: each job process counts what it does where it happens
    (`store::nasio`, whose deltas are also a target's cost measures) in a file of its own mapped
    shared on the Mac's disk (no write call a count), with a ring of the last 32 NAS files it
    opened or made (the file stream); the agent sums them each second (`nas_*` series). The lead
    sums the members' every 10 s (`pool_nas_*`), with the disks' busy share from the pacing's SNMP
    reading, the agents' NAS calls in flight and missed, and the pacing's window, use and counted
    ratio. The lead's status says the rest (`nas`, `coord::NasView`): probes, limits, the pacing's
    last reading and verdict, its bounds, each step's weight.
  - **The internet**: the agent's proxy counts per host the bytes, requests, connections, retries,
    429 holds, its budget and robots.txt as a crawling job told it, with a ring of the last 200
    transfers; each job's own bytes too (its proxy address tagged with its slot).
  - **Each running job, live**: the status's `jobs[].live` and its lease's beat: its phase, target,
    targets done, NAS and internet bytes, `ran_s` (from its process's start, carried through a
    take-over, so a job's run is timed from its start, not its lease), and on the beat alone
    `left_s`, its time left by progress (§7.2, Room held for a big job).
  - Every Mac's port answers `POST /api/live` with no key (`{range}` or `{since}`, `series` to pick;
    at most 600 points a series; `net` and `files` unless `net: false`). A series missing has no
    measure yet: readers say so, never zero. Another Mac's page reads it directly (CORS as for
    tasks, §8): the Machines tab (docs/pipeline.md §5, Machines), its NAS section, and the
    Activity tab, which asks every Mac every 2 s for `files` and `net`.
- **Views** (menu bar, build page, `scenic lead`, `scenic status`) read the agent's
  `crate::agent::lead::View`: the declared lead, whether this Mac leads, each member's state, "no
  lead" and why, the last change of lead.
- **The alert** (`lead::alert`, `ALERT_S`: an hour with no lead) says the members finish their jobs
  and wait, and to see to the lead's Mac.
- **History:** take-ups and step-downs are `term` events in the member's history ("took the records
  up in epoch E, from …", "stepped down (epoch E): …"). Members joining and leaving are the lead's
  `member` events ("joined the pool", "left the pool (out of touch)"); a lead's first read only
  sets what it compares with.

## 11. Controls

`crate::agent::lead`, `tools/status/main.swift`, `web/buildpage/pool.js`:

- **No control of who leads**: the menu bar, the build page and `scenic lead` show the pool, and
  none moves the lead. The owner moves it only by editing the declaration (§6.1).
- **Build page:** Pause and the asks below work from a page on any device. On a Mac not leading
  they go to its agent, which passes them on.
- **Cancel and try again from the page** (`/api/act`): cancels a running job, or tries a failed
  target again. A retry forgets this Mac's back-off for the target and, on the lead, its
  coordinator's failures (`Coordinator::clear_failed`); on a member it's mailed to the lead
  (`Msg::Retry`), only to a lead on its own app, else 409 and the page says why.
- **`scenic lead`** shows the pool; `scenic status` too.
- **Cancelling a job** (`crate::control::Cancel`): `scenic job cancel <target> [--step <step>]
  [--rerun-after-update]`, or the menu bar's "Cancel <job>", writes an ask to the running Mac's
  agent (dropped after two minutes). The agent stops its slot at once (SIGTERM, SIGKILL 30 s on)
  and tells the lead's coordinator, which marks every live lease of the target and offers it to no
  one from a plan begun before the cancel. A cancelled job ends as given back, not failed (no
  backoff), its finished targets handed off and its target's scratch removed. With
  `--rerun-after-update` the coordinator holds the target until it runs an app newer than the one
  it was cancelled on. A cancelled job's slot is free, so an update waiting on it restarts at once.
- **Building finished targets again** (`crate::agent::redo`): `scenic redo <step> <target>…
  [--run]`, from any Mac, for a target saved wrong under the right key (a done target is never
  planned again). Without `--run` it prints, per target, the key it would clear and the files that
  stay served until the target is rebuilt. With `--run` it posts the targets to the lead
  (`/api/redo`), whose agent keeps the ask (`redo.json`) and plans a job of the `redo` step, the
  lead's own: no file changes, it hands off a done record with no key, which the merge takes only
  from the merging lead itself (`agent::membership::by_its_lead`) and which forgets those targets'
  keys. The plan offers them again; the old files leave the manifest as new ones replace them. The
  command follows the records until the redo's merge lands (15 minutes at most).
- **Pausing one Mac** (`crate::control::Hold`): `scenic pause --here` / `scenic resume --here`,
  `scenic pause <member>` / `scenic resume <member>` from another Mac, `POST /api/hold` from any
  device, or the menu bar's "Pause this Mac". The hold is kept in the agent's folder (`hold.json`)
  and by the lead (`/pool/hold`), the later setting winning. A paused Mac's jobs stop at their next
  safe point and are given back, it asks for and starts no work, and keeps beating; a paused lead
  keeps leading and granting the others' work. This Mac's own ask is taken up at once by the
  agent's control thread (a kqueue watcher on its folder), never by the loop, so it holds within a
  second on a hung NAS or during a plan. The status says the hold's stage (asked, pausing, paused,
  resuming); the forecast gives the Mac no lanes. A member whose app is older than the asking Mac's
  is refused ("too old to pause alone"). `--now` stays the whole build's.

## 12. What is built

Built and always on: the declared lead and its epochs, coordination by HTTP on the lead
(docs/coordination.md: its store, the beat, hand-offs, mail, holds, settings, budgets, the records'
brief), the records on the lead's disk and at one path on the NAS, the driver, the views, the steps
table, the memory and disk guards, write-sets, every step to every member, every job by grant (the
lead's too) and packing, the forecast, resuming (§7.2, §7.4) and brokers (§8). There is no
election, handover or takeover, and no pool switch.

The pool's settings are the lead's (`/pool/settings`, crate::coord::asks; `scenic pool set`):
`placement` (§7.2; off while not set), `nas-budget`, `nas-target`, the NAS pacing's bounds
(`nas-step` … `nas-apart-judges`, §7.7), the needs model's (`needs-work-pct` … `needs-relearn`,
§7.7), the failures' backoff (`backoff-start-hours`, `backoff-ceiling-hours`, §7.5), `batch-minutes` (§7.7), `overdue-factor` (§10, Flags), and the map updates' `hold-catalog`
and `near-slope` (docs/building.md §2; off while not set). A new setting is made only once every
member's heartbeat shows the app that has it. A role is read as the agent starts (docs/agent-parts.md §9.1);
`state/pool/slots` is read by no current app but stays `on` on the NAS for older apps, and
`state/pool/enabled` and `state/pool/budgets` are read by no app.

Planned: resuming the terrain's and the slope's z6 tiles. Packing a terrain z6 tile's fetched raw
tiles (`pack_raw_with`) stays inside that z6 tile's job.

## 13. Testing

- **The simulator** (`crate::pool`'s `sim`, in the crate's tests; seeded, repeatable): two to four
  Macs, any of which may be the declared lead. **Real:** the driver; the lead's coordinator in
  process (its routes, scheduler, lease table, hand-offs, its store with its durability wait); the
  member's beat on coordwire's client over a network fake; the records' brief; GC on the lead.
  **Modelled:** the agent's loop around them (`sim::mac`), the jobs, the map's publishing, app
  installs, clocks, and the share (late or lost create-new bytes, late and busy renames, stale
  reads, outages, slow calls). **Faults** (each a knob, mixed by the seed): dropped, delayed and
  lost requests and answers; Macs cut off, one-way links; the lead killed mid-append; crashes;
  skewed clocks, sleeps, upgrades and rollbacks; lost saved states; members leaving for good; reads
  failing at 5 % or 20 %. **Checked at every step:** no target under two live leases, two
  coordinators never granting at once, no job id granted twice, every hand-off merged once or
  refused with why, acknowledgements only of what a landed snapshot names, §4's invariants, a job's
  brief never older than its lease's, GC removing no name the records or a map server's window
  hold; at the end, the declared lead leading alone, caught up, the one path naming every hand-off
  owed (but the double fault's, docs/coordination.md §5.3, and a Mac gone for good's). Runs:
  160 mixed seeds and more per fault in the suite; by hand (`#[ignore]`), thousands per kind.
- **Not modelled by the simulator:** the HTTP transport itself, the agent's real loop, planning and
  jobs, torn reads and other I/O errors of the share, the member lock, a wall clock set while
  awake, the map server's own reads, more than four Macs, the owner's hand move of the lead.
- **The placement simulator** (`sim::place`): N Macs, each with its memory, cores and disk, work a
  plan through the lead's real coordinator on a simulated clock, every job by grant, members
  asking over HTTP with their capacity and the lead in process. Each job takes its step's true
  memory, cores and disk; a Mac may be slower to the NAS. Checked at every start: no Mac past its
  slots; lead-only steps on the lead; one cold start of a step at a time; no job past what a Mac's
  jobs may hold but a cold start (stopped by the guard); every job at its true NAS weight within the
  budget; no disk over-committed; threads within the Mac's cores; and the plan finishes. Cases: the
  M4 and the M1, four Macs, halo steps, shared caches copied a part at a time (§7.1, Disk
  reservations), z6 tiles and map tiles side by side. Not modelled: other step dependencies,
  failures; a job's time doesn't depend on its threads.
- **The agent's tests** (`crate::agent::membership`, `crate::agent::lead`): two agents in one
  process: a member never claims the lead, and with the lead gone keeps its hand-offs and waits;
  the lead's restart keeps its leases; the owner's hand move of the lead
  (`a_lead_moved_by_hand_leads_from_its_copied_folders`). Each piece of the protocol has its own
  tests (docs/coordination.md §8).
- **Two Macs on the real NAS** (`tools/check/pool-two-macs.sh`): the real agents on a scratch
  folder of the NAS, jobs played by a script. Planned: load.
- **The share itself** (`tools/check/nas-probe.py <peer> <scratch folder>`): contended create-new
  and stale reads, stats and listings (§3's figures).

## 14. Decisions

- **The Mac mini is the fixed lead** (the owner, 30 Oct): always on, at home, on power. Members
  never lead; with the mini gone they finish their jobs and wait. Named by a declaration on the NAS
  made once (§6.1), not a setting on each Mac (two could disagree) nor anything a Mac's process
  makes (that would move it).
- **`tailscale serve` on every Mac** (`tailscale serve --bg 8090`, port 443) gives pages HTTPS; the
  agent finds it via `tailscale serve status --json` and puts it in its heartbeat. Without it,
  plain HTTP on the tailnet reaches every member, and an HTTPS page reaches only members with
  HTTPS. The owner also allows `scenic` through each Mac's firewall (§6.8).
- **The M6's on 8444** (`tailscale serve --bg --https=8444 8090`): its port 443 is Caddy's, for
  https://map.brandontsang.net (docs/macs.md, Devices), and `tailscale serve` on 443 would take
  every connection to the tailnet address. Caddy serves the build page under `/build` too, and the
  page asks its own member under that path; its broker address stays the 8444 one.
- **Work order:** every slot from the front of the plan, in contiguous runs.
