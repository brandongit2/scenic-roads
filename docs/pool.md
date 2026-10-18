# The pool: any Mac can lead the build

Every Mac runs the same agent (`scenic agent`). One of them, the **lead**, plans, grants jobs,
merges results into the build's records and publishes; the others are **members**. Which Mac leads
is read at run time from the NAS (the terms, §6.1), can be handed over from any Mac's controls, and
is taken over when the lead is gone. Browser pages are workers too (§8, workers.md). The pool's
code is `crate::pool` (terms, records, journal, the driver) and `crate::agent::pool` (the agent's
side of it). It is always on.

Planned: the records' readers reading the term's snapshot directly; until then the lead keeps
writing today's three files (§6.2).

## 2. Principles

1. **One agent, every Mac.** A Mac is known by a member id made once; its host name is a label.
2. **Durable state on the NAS;** a Mac's disk holds caches and work in progress.
3. **Nothing on the NAS is written in place by two Macs.** A file has one writer (a member's own
   files), or is made once with create-new (terms, refusal notes), or belongs to one term (records,
   the coordinator's state). A stale Mac's late write lands where no one reads.
4. **Every job hands its results off** to the journal; only the lead's merge writes the records.
5. **Work goes where it fits,** by measured memory and disk (§7.7). Publishing and GC are the
   lead's duties, not jobs.
6. **Handing over is a protocol,** not a restart: nothing is lost, no job stops.
7. **Tasks belong to the member whose job offers them** (§8).
8. **Any number of Macs and pages.**

Where state lives:

| State | Truth | On a Mac | A new lead |
| --- | --- | --- | --- |
| Records, built files, catalogs | NAS: `state/build/`, content-named files | caches (`agent/cache/…`) | reads them |
| Downloads | NAS: `sources/` | cache copies | nothing |
| A pass's Wikidata/Wikipedia answers | NAS: `sources/items/<date>/` (`pipeline::answers`) | `agent/cache/items/`, `agent/cache/heritage-<date>/` | its first run takes them |
| Units' kept state | NAS: `cache/dem-units/`, `cache/scenic-units/` | none | nothing |
| A job's progress (the OSM pass's stages, scratch) | the Mac running it | scratch folders | the OSM pass's resumed there (§7.2, Resuming); the rest redone |
| The coordinator's state, token, history | NAS: `state/coord/` | `agent/coord/` | loads it (§6.2, §7.5) |
| The lead's own (map update under way, daily jobs' last runs) | the lead's disk | `agent/map-update.json`, `agent/state.json` | starts afresh |

## 3. What the NAS gives us

The share is SMB 3 on a Synology. The design assumes:

- **Create-new is atomic** (checked for one client; between two Macs unchecked). Terms and leases
  rely on it.
- **A rename can be delayed indefinitely** (a Mac sleeping between write and rename).
- **Reads can be stale** for minutes (macOS's SMB caches).
- **Renaming over a file another SMB session has open fails** (EBUSY) for as long as it's held:
  `crate::whole::rename_over` retries for 5 s. The server can hold a file for hours with no process
  on either Mac having it open;
  renaming the held file away, and renaming into its free name, still work, but removing it fails
  until it's let go. So heartbeats (the pool's, `Nas::write_aside`, and `state/status.json`) fall
  back to `crate::whole::rename_or_aside`: the old file renamed to
  `<name>.<host>.<pid>.<n>.aside.tmp`, the new one renamed into place, then asides removed: each
  fallback removes up to 4 (the oldest first), finds any Mac's or process's asides of that file by
  listing its folder at most once a minute, and remembers at most 16 not yet removable. Listings
  skip asides as temporary. When the second rename fails the aside stays (renaming it back could
  replace a newer file), so the heartbeat is missing until the next write. A reader may find no
  heartbeat between the two renames, never a torn one. The fallback logs
  `whole: rename over <path> busy: …`. Other files keep the plain retry.
- **No conditional writes or atomic appends** between clients.
- **Listings are slow** (3–33 s a folder under load): nothing lists in a loop; the current term is
  found by stat'ing the next, heartbeats are read by member id, the journal is listed off the loop.

## 4. Invariants

Checked by the simulator (§13):

1. One lead per term (terms are made with create-new).
2. The term never goes back; a lead checks `terms/<E+1>.json` before acting.
3. No accepted hand-off is lost: it's in the journal before it's accepted, the journal is a log, a
   new lead replays what its snapshot doesn't name.
4. Each records snapshot is one file written whole.
5. A stale lead's writes go to its own term's files, which no one reads once a later term saves.

## 5. Roles

- **A member** is any Mac running the agent with the NAS. Its id (`m-<16 hex>`) is kept in its home
  folder bound to the hardware UUID (a copied folder makes a new id). One process per member runs
  the pool, holding `pool-<id>.lock` in the app's folder (`crate::pool::MemberLock`, checked every
  step; a process that loses it stops, saving its state). A member writes its heartbeat (§10), runs
  job slots (§7), brokers its jobs' tasks (§8), and passes the owner's asks to the lead.
- **The lead** is the member the current term names. It also plans (`crate::agent::build`) and
  runs the coordinator, merges the journal into its term's records, publishes catalogs, runs GC,
  compacts the raw tiles' archives, keeps the pause and writes the build's status. Its slots are
  ordinary member slots.

## 6. Leadership

### 6.1 Terms

A term is `state/build/terms/<E>.json`, made once with create-new (`crate::pool::term`):

```json
{ "term": 12, "member": "m-3f9c…", "host": "…", "app": "20261012-0910-1a2b3c4",
  "since": 1791300000, "how": "handed over by …", "from": 11, "seq": 214 }
```

- **The current term is the highest that exists.** A Mac learns it at start (`term::current`, from
  the hint `state/build/lead.json` or one listing), then only stats `<E+1>.json` (`term::next`). It
  never goes back (`driver::Saved` keeps the highest seen).
- **Every change of lead makes term E+1:** handover, take-back, takeover, re-assertion. One create
  wins; no lock is needed.
- **A handover's term names a snapshot** (`seq`); the new lead takes up from that or later.
- **A term not readable whole** (its bytes not landed) has no known lead. Its maker finishes it
  (`term::finish`). One left unreadable is passed by a forced takeover (`term::forced`), by the
  owner or automatically after half an hour (§6.5).
- **The app rule:** a term's app must be at least as new as the previous term's (job keys include
  step versions; an older lead would rebuild everything). Published apps compare by publish
  minute; a development build follows only another and any app follows it (`term::app_at_least`).
  A take-back is checked against the term handed over (`term::back`). The owner can override
  (`scenic lead take --force --downgrade`). A lead seeing a newer app installed restarts and
  re-asserts; one restarted into an older app stands down (§6.6).
- Records keep keys of steps an app doesn't know (`build::Keys::other`), so a downgrade loses
  nothing it can avoid (docs/buildings3d.md §3.6).

### 6.2 Records per term

- **One file per term** (`crate::pool::records`): `state/build/term/<E>/records.json` holds the
  manifest, keys, pending uploads, raw archives awaiting the index, the journal entries it reflects
  and refused (with why), the lease that last set each target, the forget horizon, and `seq`. A
  handover's last snapshot also holds the coordinator's state. Written whole (`Nas::write_whole`)
  after each merge.
- **Term 1** started from today's layout (`manifest.json`, `jobs.json`, `pending.json`): its first
  snapshot is `term/1/first.json` (`records::first`, `term::bootstrap`).
- **Taking up term E+1** (`records::start`): from the newest snapshot found walking down the terms
  (a handover's only from its `seq` or later; a stale read fails and is retried). The new lead then
  replays every journal entry its records don't name, in (term, lease) order. It is **caught up**
  (`caught_up`) once a listing of every day, asked for under a day ago, is merged; catalogs and GC
  wait for that. It relists daily.
- **Acknowledgements are per term** (`journal::Mine`): each loop a member tells the current lead of
  its entries that lead hasn't acknowledged. A lead acknowledges an entry once a saved snapshot
  names it.
- **Readers** read today's three files, which the lead writes after each save
  (`agent::pool::write_today`). Planned: reading the current term's snapshot (`Records::newest`,
  falling back to an earlier term's).
- **The coordinator's state** is per term too: `state/coord/term/<E>/state.json`
  (`crate::coord::PoolState`: leases, costs, floors, disks, failures, pause), written by the lead
  each loop it changed. A take-up loads the handed-over state, else the newest earlier term's
  (`took_up`, §7.5).

### 6.3 Asking

The menu bar ("Hand the Build To ▸", "Make This Mac Lead"), and `scenic lead`
ask their own Mac's agent (`lead-request.json`, `crate::control::LeadRequest`); the worker page asks the agent serving it (`/work/lead`). `crate::agent::lead`
checks the ask as the driver would (`Driver::hand_to`, `Driver::takeover`) and either hands over (on
the lead) or mails it to the lead (`Msg::HandTo`, §9). Asks naming a Mac that isn't a live member,
is on too old an app, is unreachable (§6.8), or whose conditions say it can't lead (away, on
battery under 30 %, short of disk: `Conds::able`) are refused with why. An ask's state is kept in
`agent/pool/lead.json` and shown by every control.

### 6.4 Handing over (the lead is there): a state machine

Pure transitions in `crate::pool::handover`, driven by `crate::pool::driver`; state lives in the
two heartbeats and the terms.

| State | What happens | Next |
| --- | --- | --- |
| **Leading** (A, term E) | | ask to hand to B: **Offered** |
| **Offered** | A's heartbeat: `handing_to: B` and the offer | B answers within `OFFER_S` (2 min): **Ready**; else **Leading** |
| **Ready** | B checks disk and app, heartbeat `ready_for: E+1` naming the offer | **Settling** |
| **Settling** | A stops granting ("the lead is moving"), keeps renewing leases, stops a catalog or GC in flight, saves records with the coordinator's state | within `SETTLE_S` (1 min), B still ready: **Passed**; else **Leading** |
| **Passed** | A makes `terms/<E+1>.json` naming B with `seq`; A is now a member | B takes up within 2 min: **Leading** (B, E+1); else **Taken back** |
| **Taken back** | A makes `terms/<E+2>.json` naming itself | **Leading** (A, E+2) |

Leases keep their ids (`<term>-<n>`) and wall-clock deadlines across the handover, so every job,
the old lead's included, renews with B (§7.6). A lead with journal entries left to read gives up a
handover (to be asked again).

### 6.5 Taking over (the lead is gone)

- **By the owner:** when the lead's heartbeat is more than ten minutes old, or it stood down, "Take
  Over the Build…" in the menu, the worker page's "Take it" or `scenic lead take` makes term E+1.
  A lead whose coordinator answered this Mac within ten minutes is *late*, not gone, and needs
  `--force` (§6.7). `--force` skips the check and passes an unreadable term; `--downgrade` allows an
  older app. In-flight leases lapse and go back out; no built work is lost.
- **No automatic takeover of a lead that's merely gone** (it may be asleep). Instead a **proactive
  offer** (`crate::agent::lead::offer`): when the lead is away or on battery and another member is
  home on power, every control offers "Hand the Build to <Mac>". With `scenic lead auto on`
  (`state/pool/auto-handover`) the lead hands over by itself once the offer has stood five minutes,
  not within half an hour of the last change.
- **A lead that stood down** (§6.6) is taken over automatically after `STOOD_DOWN_S` (2 min) by a
  member that can lead and whose app the rule allows; candidates try in turn (newest app, then
  lowest id), the create-new deciding.
- **A term left unreadable** is taken past automatically (`UNREAD_TAKE_S`, 30 min of short reads
  while awake, `term::forced`), but only when every other known member's heartbeat reads whole, in
  touch and not leading it (`clear_to_take_past`, fails closed), and this Mac's own heartbeat is
  landing. Otherwise it's the owner's, and the alert (§10) says so.

### 6.6 Re-asserting, and stepping down

A lead **re-asserts** (makes E+1 naming itself and takes it up from its own records) after a gap
between wall and awake clocks of over a minute (sleep, clock set), a step or a gap over `STALL_S`
(5 min), a restart, a saved state that doesn't know a term naming it, and before every GC sweep.
If the create fails, someone else took over: it **steps down** at once, stopping grants, planning,
merging and duties, and carries on as a member.

A lead on an older app or a development build can't re-assert: it **stands down** (`stood_down` in
its heartbeat), so another member takes over (§6.5). Every step it also checks `<E+1>.json`.

### 6.7 Clocks and wakefulness

- No NAS clock: out of touch compares a heartbeat's `beat` with the reader's clock; a beat over
  60 s ahead shows "clock wrong". Everything persisted is wall-clock time.
- **The pool is stepped whatever the agent is doing** (`crate::agent::pool`'s `Ticker`): the agent's
  loop steps it about every 20 s, and the pool's own thread steps it when that loop has been quiet
  for `QUIET_S` (25 s). Jobs and the coordinator's start/stop stay with the loop. A tick held past
  `STALL_S` is taken as hung on the share: the lead grants nothing and the status says so.
- **Late, not gone:** every coordinator answer carries `x-pool-term` and `x-pool-member`; a lead
  whose heartbeat is stale but whose coordinator answered within `ANSWERED_S` is shown as late.
- **The lead stays awake** (`caffeinate -i`) while it leads with leases out.

### 6.8 Reaching the lead

A lead the members can't reach (e.g. macOS's application firewall dropping incoming connections)
grants them nothing. `crate::pool::reachable`, `Run::tend_reach`, `crate::agent::lead`'s `away`:

- Every member probes the lead's coordinator and the other brokers every `PROBE_S` (60 s); its
  client's own requests count too. Results go in the heartbeat (`Beat::reach`): reached within
  `REACHED_S` (5 min), or unreachable for `REPORT_S` (2 min) with the cause.
- One failed request makes the rest of that loop's requests to the lead fail at once
  (`Client::new_round`); a job's end not taken is retried until its lease lapses (`OWED_S`).
- A member another reports unreachable can't be handed the lead.
- A lead reported unreachable by at least half the other members for `AWAY_S` (10 min) hands the
  lead to a reachable member that can lead, at most once per `CALM_S` (30 min).
- Every view says who can't reach whom and what to do (`View::unreached`): wake the Mac, or allow
  `scenic` in System Settings › Network › Firewall. `scenic` is signed ad hoc at a new path per
  version, so that allow lasts until the next update; a lasting one needs a code-signing identity
  (not set up).

## 7. The work: slots on every member

### 7.1 Slots and asks

Each member runs one job slot, two on a Mac with 32 GB or more (the second only when both jobs'
measures fit, §7.7); an idle slot takes tasks (§8). A slot's ask (`/work/ask`) says the memory it
spares, its free disk, cores, conditions, app and member id. The lead answers with a step's targets
that fit, in plan order; a member on another app than the lead's gets nothing (409) until they
match (plan.md §8, The same app).

An ask also carries its whole Mac's capacity (`coord::Capacity`, the ask's optional `cap`): the
memory guard's limit and what the running jobs hold, the memory free under the limit less each
running job's predicted need (the larger of what it holds and its predicted peak: its targets' measured peak on the
lead, the step's first guess on a member), its
cores and those the last minute's load leaves free, the disk free less the reserve and the owner's
room target (`reserved_mb`, for reservations, is 0: none are made yet), in use, mains or battery,
the NAS's reach, the guard's switch and its running leases. The lead sets its own each loop
(its jobs ask nothing). The coordinator keeps the last per member (`Coordinator::capacities`, leaving out one older than
two minutes, `CAP_FRESH_S`: a busy member asks nothing, so its figures go stale):
`scenic status` prints a line per member, and the worker page's details a table. Placement doesn't
read it yet. It rides the ask, not the heartbeat: the coordinator places at ask time, and the
ask's measures are taken then, where a heartbeat on the NAS is up to two minutes old and the
coordinator doesn't read heartbeats. Every field has a serde default: an older coordinator ignores
`cap`, and a newer one takes an ask without it (an older agent's) as before.

### 7.2 Placement: the steps table

`crate::agent::steps` has a row per step the agent runs as a job (docs/pipeline.md: its id, title,
what it does, kind, scope and stage, and these resources); the step sets (`SHARED`, `SECOND`,
`ANSWERED`) are generated from it at build time.

| Column | Meaning |
| --- | --- |
| memory | a first guess until a target's run is measured (`crate::coord::Cost`) |
| shared | whether members take it, and its rank |
| alone / beside | a whole-build run kept to the lead (`crate::agent::every`); the second job's rank in the forecast |
| answered | keeps the pass's answers (the lead's alone) |
| batch | targets per job of the lead's own (a member's batch is by measured time) |
| write-set | the manifest names its jobs may change (`agent::steps::writes`, §7.3) |

Rows of note: `trees`/`terrain`/`slope` per z6 tile with `*-lo` assemblies per z3 tile (plan.md §6);
`bldprep`/`bldtiles` per z6 tile, `bld-fetch` the lead's (docs/buildings3d.md §3.6); `inputs`, a
target per unit, the lead's, planned before every other job (docs/inputs.md §4.3).

- **Every step to every member** (`crate::agent::every`, `crate::coord`'s `pick_every`): all but the
  lead's own steps (`every::LEAD_ONLY`: catalogs, prunes, `bld-fetch`, the answer-keeping steps, the
  worldwide steps) go to any member whose measured room fits. `pack` and `lo` only with a map
  update's units (`as_of`). A cold start (§7.7) goes to the member with the most room
  (`most_room`), alone, while no job of its step runs.
- **Order and locality:** a member gets a contiguous run of plan targets, from the front or just
  after its last target, as many as fit `coord::JOB_S` (a quarter hour) of measured time.
- **A map update shared** (`crate::agent::publish`, behind `state/pool/update-share`): the lead's own
  `pack`/`lo` job takes an even share (`publish::lead_share`), leaving the rest to asking members.
- **Placement by how crucial** (`crate::agent::placement`, behind `state/pool/placement`): asks are
  tried on steps by their forecast slack (least first), within a step from the longest target; each
  target is drawn among the members that could take it, weighted by measured speed
  (`placement::draw`, stable per minute window). A task offered ahead goes to a slower worker only
  while its head start covers that worker's run. The lead's own jobs keep plan order.
- **Resuming** (`crate::agent::resume`, behind `state/pool/resume`): a job interrupted with progress
  on a Mac's disk (built for the OSM pass) is offered to that Mac alone for 24 hours from its first
  start, or until `scenic pool release <member> "<job>"`. Progress is in
  `state/pool/progress/<member>.json`, written only by that member. A finished one is held unoffered
  until merged.
- **The forecast for equal machines** (`crate::agent::forecast::equal_machines`): a second forecast,
  `equal`, with each member's slots and the pages as lanes at their measured speeds. Reports only.

**A job waiting on a worker** (the lead's slots; `crate::offload`'s `WAITING_ENV`): a unit job
waiting on a faster worker for its tail writes `SCENIC_WAITING`; while it exists a second job may
start beside it counting the waiting job's current memory and none of its cores. That second job
**yields** (stopped at once, given back, not counted as a failure) when the first resumes and both
no longer fit.

**The memory guard** (`crate::agent::memguard`; `state/pool/memory-guard`, on unless it says `off`):

- A thread samples each job's processes' footprint every 5 s, per target under way (from
  `started` lines in `SCENIC_COSTS`, else `SCENIC_DONE`), and writes `memory.json` for the status.
- Every child started through `store::spawn` (the agent's jobs, their programs) is started without
  macOS malloc's cache of freed large blocks (`MallocLargeCache=0`, an undocumented libmalloc
  setting, `store::spawn::NO_LARGE_CACHE`; their children inherit it; the agent and server, started
  by launchd, keep it): with it, freed blocks stay in a
  process's footprint, and a dense unit's scenic-build held 6–7 GB of them while its programs ran.
- **Limit:** the Mac's memory less an eighth, 4 GB at least. Over it, the job beside the largest
  stops at its next safe point; the largest alone past the limit stops at once only while the Mac
  is short of memory (pressure warning, or swap growing a GB), else at a safe point. A drain that
  finds no safe point in 15 minutes is stopped. A stopped job is given back and kept from that Mac
  for an hour.
- **Floors**, learned whatever the switch: the most a target held is its floor
  (`floors.json`, and the pool's state), rising only, tagged with whether it ran alone and the
  step's `cost_version`. A target's predicted memory is never below its floor. A target whose
  floor exceeds this Mac's limit goes to a Mac with room; past every Mac's, it waits; for a lead-only
  step, the status names a member with room and `scenic lead give`. `scenic pool floors [--clear
  [<step> [<target>]]]` lists or clears them.

### 7.3 Results: the journal

- **A job's hand-off goes straight to the NAS** (`crate::pool::journal`): the member keeps it in
  its folder until written to `state/journal/<day>/<term>-<n>.json` (the lease's id, the UTC day of
  first try), then tells the lead.
- **The lead validates at merge** (`agent::pool::check`, `records::Check`): every change within the
  step's write-set, content names matching logical names, the lease's targets. Write-sets are one
  function per step (`agent::steps::writes`) that both the job's `Out` (`Out::open` for its step)
  and the merge use; `Out` refuses a name outside before it lands. An entry outside its write-set
  from a member on a newer app is **held** (`check_from`, `records::HOLD`) until the lead runs that
  app; otherwise it's refused, the why noted at `state/journal/rejected/<key>.why`
  (`journal::note_refusal`). The lint `tests/write_sets.rs` gates publish.
- **Lease order holds:** an older lease's entry for targets a newer one set changes nothing.
- **The journal is a log:** nothing is removed on merge. Planned (plan §10): GC of day folders over
  a week old once every snapshot names their entries; `forget_before` already handles it.
- **The lead lists the journal off its loop:** every day after take-up and daily, the last two days
  every ten minutes. Otherwise it reads the entries members tell it of. An entry unreadable for an
  hour while the share answers is refused. Merging reads for at most a minute a loop.
- Raw tile archives are only added by jobs; compacting is the lead's duty. `summaries.json` is a
  content-keyed cache any writer may update.

### 7.4 Planning reads only the records

Behind `state/pool/resume`, a pass is complete when its records name `sources/osm/<date>/pass`
(`crate::osmpass::latest_pass_by`); off, a `pass.*.json` file still says it. A catalog runs only
when the lead is caught up. A resuming job is given its member's unacknowledged hand-offs of its
step (`SCENIC_UNMERGED`), which `Out::open` lays over the snapshot.

### 7.5 The coordinator across terms

- Saved on the loop after every grant or finish; renewals aren't saved (a loaded lease gets a fresh
  deadline).
- Workers are a member id and an agent instance; a new lead drops only instances that no longer
  exist.
- **The history** is one file per writer, `state/coord/history/<day>/<member>.jsonl`.
- **The contact** `state/coordinator.json`: a stopping lead removes it only if it still names its
  own addresses.
- **Its port is free for the next:** children are started (`store::spawn`) only while no listener
  or connection is being made or closed, under one lock, so no child inherits the socket. clippy.toml's
  disallowed-methods, checked by publish.sh, keeps every start, listen and accept going through it.
- With a term made, `check_writer` refuses every direct save of the records: everything hands off.

### 7.6 The lead's own slots

Jobs call the lead through one interface re-resolved each call (`crate::agent`'s `beat`,
`end_lease`): in process while this Mac leads, over HTTP otherwise, so a job carries on across a
handover. A job's tasks are offered through its own Mac's coordinator or broker; one that stopped
answers nothing and the job runs the task itself. With `state/pool/slots`, a member taking up a
term starts its coordinator in process, and a lead losing it stops its coordinator and duties,
keeping its jobs running; without it, the agent restarts into its new part once its first slot is
free. A lead whose update waits on its jobs hands the lead to a member already on the new app
(plan.md §8, A newly installed app).

### 7.7 What a step needs, by structure

Rule: nothing depends on a hand-kept figure. A step's needs are **measured** per target, **enforced
where the resource is used**, or **derived from the code**. A wrong declaration may cost speed,
never correctness, politeness or work.

| Need | How |
| --- | --- |
| memory | measured per target (`peak_mb`, the guard's floors, §7.2) |
| input size | measured per target beside its cost (`sizes` in `costs.jsonl`, `pipeline::costin`: bytes read, threads, area, GeoTIFFs opened, and the counts a step has: ways and vertices, buildings, peaks); read by the step's model (below) |
| disk | measured per target: bytes written through `store::cachefile` and the scratch writers (`SCENIC_WRITTEN`), plus a look each minute at the largest files in a slot's scratch folder; kept as the target's disk floor (`disks.json`). **Disk guard:** when free space falls below the reserve (30 GB), the fastest-growing job is frozen, room is made from caches no job holds (`room::toward`), and it goes on or is given back. Given-back jobs' scratch folders are deleted oldest first when room is short (`room::free_given_back`) |
| two jobs on one Mac | a job starts beside another only if both jobs' measured memory and disk fit; a cold start runs alone |
| raw tiles | `store::cachefile`'s locks; packing deletes only tiles no job holds; a job reopens its area's archives when the index changed (`RawTiles::get`, `refresh_area`) |
| Wikimedia budget | `crate::budget`: one budget for the pool, enforced at egress. Every job's connections go through its agent's proxy (`crate::budget::proxy`, `HTTPS_PROXY`), the job sandboxed (`sandbox-exec`) to deny any other way out. A metered connection takes a token first: leases are create-new files per slot of a ten-second window (`state/pool/wiki/<class>/<window>-<slot>`). `wiki` four requests a second; `query` (WDQS, QLever) one query per two seconds of query time. A 429 told to the proxy (`POST /hold`, dem/budget.py) holds every Mac. The NAS away, each Mac takes the rate divided by known members. Lint: `tests/budgets.rs` |
| a pass's answers | `crate::answers`: a lease on the NAS (`state/pool/answers.lease`, renewed every 150 s, lapsing after 10 min) around each sync; on a Mac a reader/writer lock (`answers::Local`) |
| write-set | `agent::steps::writes`, enforced by `Out` and the merge (§7.3). Raw archives, answers and `bld-fetch`'s sources are outside the manifest, each written whole under its module's names |
| batch | a member's: as many targets as fit a quarter hour of measured time; the lead's own: the table's |

**One model per step** (`pipeline::needs::Book`: `need(step, target, threads, guess)`, and `mem`
alone for the picks): only what was measured the way the step runs now counts
(`coord::cost_version`); a run or floor of an older way of it (a unit's before v2) is neither a
target's memory or time measure, nor a fit's run, nor part of a prior (disk floors, `disks.json`, carry no version and always count). A target's memory, disk and time are its own
run's (memory never below its floor; a floor alone counts as its measure); else base + k × size
fitted over the step's measured runs (least squares raised to cover every run, × 1.1, capped at the
most a run it was made from measured; threads a term when the runs differ in them), on whichever size
fits best (a `sizes` measure the target has from an earlier run, or its tile's area); else, for units
and landmark candidates, their piece-size first guess (`coord::unit_peak`), so one dense unit doesn't
set every other's; else the step's prior, the 90th percentile of its targets' measures (memory:
their floors among them), a heavy target held by its own floor; else the caller's first guess (the
steps table's, or the size the plan offered): a cold start. A fit is judged on runs held out of it
(5 folds) and not used when it predicts under on more than 5% of them, is off by more than 50% on
the median one, or has fewer than 10 runs. The models are kept until a cost, floor or disk floor
changes. The coordinator's picks (`pick`, `pick_every`, `measured`, `secs_of`, `peak`), its floors
and the forecast all read it. `scenic pool needs --fit` prints each step's fits and their held-out
errors (memory, wall time, disk) beside the prior's, and lists the steps that don't fit.

**A cold start** (no target of its step measured) runs alone on the Mac with the most room; after
one run, the step's other targets are offered by its model (above). A floor never stalls the build: the first slot starts once
room-making has done what it can, the disk guard as backstop.

Planned: `cost_version` derived from the step's key version (`crate::agent::build`'s `*_V`) rather
than kept by hand; progress of steps other than the OSM pass resumable on its Mac.

## 8. Tasks, and the pages as workers

A job is a step's run over targets on a Mac with the NAS. A **task** is a pure piece a running job
offers: programs over a folder of files, giving files back (kinds and protocol: workers.md §3, §5).
Any worker that can run its programs takes it: a page or a member's idle slot.

- **Brokers** (`crate::agent::brokers`, `crate::coord::brokers`, behind `state/pool/brokers`): each
  member's agent runs a coordinator of its own for its jobs' tasks (`become_broker`, on the pool's
  port; jobs given its address as `SCENIC_COORD`). Its heartbeat's `broker` gives its addresses and
  tasks waiting by kind. A task lives as long as its job, whoever leads.
- **Finding work:** every member answers `/work/where` with the brokers whose work fits, most
  waiting first. A page leases from that broker directly (`/work/ask`, `/work/in`, `/work/net`,
  `/work/out`, `/work/done`); the lead isn't on a task's path.
- **Cross-member calls:** members allow each other's page origins (CORS, preflight for
  `Authorization`, `X-Worker`, `Range`, `PUT`). HTTPS needs `tailscale serve` on each Mac (§14).
  Chrome's Local Network Access may still block calls to another member; the page's own member is
  the sure path.
- **One page, any member:** the page loads from any member and moves to the next that answers when
  its own goes away. The agents' token is the pool's (`state/coord/token`).
- **Trust:** a worker found wrong is not given more work (workers.md §5).

## 9. The pool's API

Members' messages go by mailbox on the NAS (`state/pool/mail/<to>/<from>.json`, one writer each,
read by member id). Jobs go through the lead's coordinator (`/work/*`), tasks through each member's
broker (§8), and the worker page's lead asks through `/work/lead`. Every member answers on port
8090. A member that can't reach the lead keeps its jobs running and starts nothing new.

Planned: an HTTP pool API (`POST /pool/lead`, `/pool/status`) in place of the mailbox.

## 10. Seeing the pool

- **Heartbeat**, `state/pool/members/<id>.json` (its own writer): label, app, resources, conditions
  (`conds`), slots' jobs, tasks, addresses (`broker`), reach (§6.8), term led, `handing_to`,
  `ready_for`, `stood_down`. Written when it changes, at least every two minutes. Listed every two
  minutes; a member missing from listings for twenty minutes is forgotten.
- **Build status**, written by the lead per term (`state/build/term/<E>/status.json`): the
  checklist (a line per stage with an item per step: docs/pipeline.md §4), forecast, regions,
  waits, map-update decision, recent jobs, pause, last catalog, the lead.
  A member writes `helper.json` instead.
- **Views** (menu bar, worker page, `scenic lead`, `scenic status`) read the
  agent's `pool.lead` (`crate::agent::lead::View`) or the fresher `agent/pool/view.json`
  (`lead::own_status`): each member's state and whether the lead can be handed to it
  (`Driver::hand_to`), what a takeover needs (`Driver::takeover`), "no lead" and why, the offer, the
  last ask and change of lead.
- **The alert** (`lead::alert`, `ALERT_S`: an hour with no lead) names the command to take over; the
  menu bar notifies once.
- **History:** term changes, step-downs and handover events are `term` events in the member's
  history file (`crate::agent::lead::notes`).

## 11. Controls

`crate::agent::lead`, `tools/status/main.swift`, `web/work/pool.js`:

- **Menu bar:** on the lead, "Hand the Build To ▸" with each member's state (greyed with why); on a
  member, "Make This Mac Lead", or "Take Over the Build…" with no lead in touch (says what it forces
  or downgrades); the proactive offer; Pause/Resume.
- **Worker page:** "Make lead" per member, a handover's progress, "Take it" (unforced).
- **`scenic lead`**: shows the pool; `scenic lead give <member>`, `scenic lead take [--force]
  [--downgrade]`, `scenic lead auto on|off`. `scenic status` shows the pool too.
- **Forcing stays on the Mac:** `/work/lead` refuses forced takeovers.
- Every control asks the driver (`Driver::hand_to`, `Driver::takeover`) rather than re-checking.

## 12. Getting there from today

Built and always on: terms, records per term, the journal, the driver, handover and takeover, the
controls, the steps table, the memory and disk guards, write-sets, the budgets and answers' lease,
every step to every member, the equal-machines forecast. Gone: the pool's on/off switch and shadow
run, claims, the named writer (`SCENIC_BUILD_MAC`), the NAS's hand-off folders, per-Mac record
writes.

Still behind switches under `state/pool/` (a file; off while missing unless noted):
`slots` (§7.6), `resume` (§7.2, §7.4), `brokers` (§8), `placement` (§7.2), `update-share` (§7.2),
`auto-handover` (§6.5), `memory-guard` (on while missing). A new switch is made only once every
member's heartbeat shows the app that has it. The old files `state/pool/enabled` and
`state/pool/shadow` stay on the NAS while an older app may read them.

Planned: readers reading the term's snapshot and the lead no longer writing today's three files;
journal GC; the HTTP pool API; resuming beyond the OSM pass. Packing a terrain piece's fetched raw
tiles (`pack_raw_with`) stays inside the piece's job.

## 13. Testing

- **The simulator** (`crate::pool`'s `sim`, in the crate's tests; seeded, repeatable): two to four
  Macs run the real driver, step by step, against a model of the share: atomic create-new whose bytes
  may land late or never, delayed renames, stale reads and listings, entries cut short or erroring,
  outages, busy renames, lost answers, slow and stalling operations, skewed clocks, sleeps, restarts,
  app upgrades and rollbacks, members leaving, owner asks and forced takeovers. It checks §4's
  invariants at every step, and at the end that one lead leads, caught up, with every entry ever
  written. 2,000 seeds by default; longer runs are opt-in.
- **Not modelled:** torn reads, other I/O errors, the member lock, a wall clock set while awake, GC,
  the coordinator (the driver's and modules' own tests cover these).
- **The agent's tests** (`crate::agent::pool`, `crate::agent::lead`, the agent's `pool_tests`): two
  agents in one process, a lead and a member, through handover, takeover, controls, hand-offs and
  every switch on and off.
- **Two Macs on the real NAS** (`tools/check/pool-two-macs.sh`): the real agents on a scratch
  folder of the NAS, jobs played by a script; each switched batch is run so before its switch is
  made on the real build. Planned: the unchecked items of §3 (contended create-new, stale-read
  duration) and load.

## 14. Decisions

- **Automatic takeover** only of a lead that stood down; otherwise the proactive offer, automatic
  only when the owner turns it on.
- **`tailscale serve` on every Mac** (`tailscale serve --bg 8090`) gives pages HTTPS; the agent
  finds it via `tailscale serve status --json` and puts it in its heartbeat. Without it, plain HTTP
  on the tailnet reaches every member, and an HTTPS page reaches only members with HTTPS. The
  owner also allows `scenic` through each Mac's firewall (§6.8).
- **Work order:** every slot from the front of the plan, in contiguous runs.
