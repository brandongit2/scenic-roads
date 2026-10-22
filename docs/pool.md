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
| Z6 tiles' kept state | NAS: `cache/dem-units/`, `cache/scenic-units/` | none | nothing |
| A job's progress (the OSM pass's stages, scratch) | the Mac running it | scratch folders | the OSM pass's resumed there (§7.2, Resuming); the rest redone |
| The coordinator's state, token, history | NAS: `state/coord/` | `agent/coord/` | loads it (§6.2, §7.5) |
| The lead's own (its catalogs' record, daily jobs' last runs) | the lead's disk | `agent/publishing.json`, `agent/state.json` | starts afresh |

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
  ordinary member slots: its agent asks its own coordinator for work as any member asks (§7.2).

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
  manifest, keys, pending uploads, the sizes of the files the manifest names (from the entries'
  hand-offs, for the catalogs: `sizes.json` beside `manifest.json`), raw archives awaiting the
  index, the journal entries it reflects
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

**Who leads is the owner's call.** The app isn't smart about who leads: no capacity score, no
offer of its own. The owner hands the lead over by hand (§6.3) when about to leave. The lead moves
by itself only on these four triggers, each to a member that can lead (home, on power or charged
past `cond::BATTERY_MIN`, not paused, room past its disk target, an app the rule allows, reachable):

1. **Its battery below `cond::BATTERY_MIN`** (30 %): the lead, on battery under it by its own
   heartbeat's conditions, asks to hand over to the first member that can lead, by host
   (`agent::lead::low_battery`), through the stages above; asked again every `BATTERY_AGAIN_S`
   (5 min) while it still leads so. None can: it keeps leading. On battery at or above it, or away
   from home with the NAS, it keeps the lead.
2. **Uncontactable:** its heartbeat stops (asleep, offline, crashed, the app quit, its agent
   stopped): taken over as silent (§6.5). A lead quitting first hands the lead over itself
   (`Agent::hand_for_quit`), and one its members can't reach hands it to one they can (§6.8).
3. **It lost the NAS while online** (away, reachable over Tailscale, its share gone): it can't write
   its heartbeat, and its coordinator's answers stop saying its term (`Coordinator::set_member(None)`),
   so members don't count it late: taken over as silent (§6.5). The same holds while its share hangs
   (a pool step held past `STALL_S`).
4. **It stood down** (§6.6): taken over after `STOOD_DOWN_S` (§6.5).

A lead paused alone keeps the lead (§11), and a lead whose update waits on its own jobs keeps it
too, restarting at a safe point like any member (§7.6).
Lead-only work that doesn't fit the lead's memory limit waits; its status line names a member whose
limit fits it, for the owner to hand the lead over by hand if wanted (§7.2, Floors).

### 6.5 Taking over (the lead is gone)

- **By the owner:** when the lead's heartbeat is more than ten minutes old, or it stood down, "Take
  Over the Build…" in the menu, the worker page's "Take it" or `scenic lead take` makes term E+1.
  A lead whose coordinator answered this Mac within ten minutes is *late*, not gone, and needs
  `--force` (§6.7). `--force` skips the check and passes an unreadable term; `--downgrade` allows an
  older app. In-flight leases lapse and go back out; no built work is lost.
- **A lead gone silent is taken over automatically** (`Driver::take_over_silent`, task #435) when
  all of these hold at a member's step: the lead's heartbeat is older than `SILENT_S` by the
  member's clock (the longer of `beat::OUT_OF_TOUCH_S` and a lease's life, `coord::TTL`: 10 min);
  the member has read that same beat, unchanged, for `SILENT_SEEN_S` of its own awake time
  (`LEAD_BEAT_S` + `STALL_S` + a step + one heartbeat's interval: 10 min 20 s), counted from its
  first read of that beat, so a lead whose clock lags isn't taken for silent while its beat still
  changes every `beat::BEAT_EVERY_S` (2 min); the lead's coordinator hasn't answered the member
  within `ANSWERED_S`; the member's own heartbeat, read past its cache, landed within `OWN_BEAT_S`
  (a NAS slow for every Mac isn't the lead gone); it can lead (`Heard::able`: home, on power or
  charged, not paused (#224), free space past its room target (#421)); and the app rule allows
  it. A lead heartbeat that can't be read stops the watch. Candidates try in turn as for a
  stand-down (newest app, then lowest id, `AUTO_RANK_S` apart); the create-new of E+1 decides.
  The new lead takes up the newest records and the coordinator's state with its leases, as an
  owner's takeover does. A lead merely slow keeps the lead as long as its heartbeat lands.
- **When the old lead stops granting.** A lead's coordinator grants only while all three hold:
  its last step found no later term; that step read its own heartbeat back (`Nas::read_fresh`,
  `F_NOCACHE`) and found it at most `LEAD_BEAT_S` (3 min) old by its clock, or not there yet (a
  read that fails counts as stale); and that step ran within `STALL_S` (5 min:
  `Coordinator::set_moving_as_of` stamps the step's time, not the tell's, so a hang after the step
  (a history write on the share) or before the next one, or a Mac woken before its step, stops
  grants).
  So it grants at most `LEAD_BEAT_S` + `STALL_S` + a step (8 min 20 s) after its last heartbeat
  landed, and a member takes over no sooner than `SILENT_SEEN_S` after it last saw that beat
  change, 2 min later. Back, the old lead's next step finds `terms/<E+1>.json` and steps down
  (§6.6).
- **What that rests on, and what a double grant costs.** The timers assume reads aren't staler
  than the 2-minute margin, and that a heartbeat the lead reads back is on the server. Writes are
  `fsync`ed and renamed before they count, and the read-back skips the page cache, but macOS's SMB
  client may still answer from an SMB lease it holds on the file: that isn't guaranteed. Grants
  aren't fenced by term at the grant itself, so in such a corner both leads may grant the same
  target for a few minutes. That costs duplicated work, not corrupted records: built files are
  content-named and never rewritten (docs/storage.md, Immutable, content-named files), each hand-off is its
  own journal entry keyed by its lease, and a merge applies an entry only if no later lease set its
  targets (`Records::apply`; the old term's lease ids sort before the new term's), else counts it
  overtaken; the files it alone named go to GC. A stale lead's step already under way when the new
  term was made may write today's three files from its older records on one or more steps, until the new
  lead's next save. So: rarely, some work is built twice; nothing is corrupted.
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
  `STALL_S` is taken as hung on the share: the lead grants nothing, its coordinator's answers stop
  saying its term (`Coordinator::set_member(None)`, until a step runs again) and the status says so.
- **Late, not gone:** every coordinator answer carries `x-pool-term` and `x-pool-member`, except
  while its Mac has lost the NAS or its pool step is taken as hung; a lead whose heartbeat is stale but whose coordinator answered
  with them within `ANSWERED_S` is shown as late, and isn't taken over by itself.
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

Every job starts by a grant, on the lead as on a member. A member runs as many job slots as its
cores allow (`agent::job_slots`: its cores less the two kept for its owner, at least one), its lead
granting fewer when its room holds fewer; an idle slot takes tasks (§8).

**The lead's ramp** (`Agent::tend_ramp`): the lead's slots start at 2 (`RAMP_START`) and grow by
one only after a clean stretch of 6 hours (`RAMP_CLEAN_S`) in which it ran as many jobs at once as
its slots (the stretch's peak: idle time doesn't count), with no memory guard stop and no stall
(the pool's step taken for hung); either starts the stretch again. While the NAS's limits aren't
known (no probe landed yet: `Coordinator::nas_known`), its slots are held at 2 whatever it
reached, and the stretch goes on without growing (a hold isn't trouble: a restart's moment before
the probe lands doesn't start the six hours again). The probes failing, two or more in a row
(`Coordinator::nas_failing`, the limits the earlier probes' meanwhile), holds them too and is
trouble: the stretch starts again; one failed alone (probes are half an hour apart: a hiccup every
few hours) is neither.
The pool's leased jobs' NAS weights past the whole (`Coordinator::nas_over`) aren't trouble: the
budget grants that by its own rules (§7.7) and holds the lead's own jobs at grant whatever the
slots. They grow up to its cores' `job_slots`. The count reached, the stretch's start and its peak are kept in the
lead's state (`agent/state.json`, `ramp`), saved when trouble begins (the stretch starts again
then, said once in the log), when it ends (the stretch starting then) and at each step up; while
trouble lasts, or the slots are held, nothing is rewritten. `scenic status` and the build page show
the ramp (`agent::RampView`): the slots now of the most its cores allow, those reached, then
`held: …` (the NAS's limits not known, or the probes failing), `not clean: …` (a stall, or the
probes failing two or more in a row: growth stops, the slots stay), or since when it's clean and
when it grows if its slots keep busy. There's no switch.

 While a slot is free and
nothing holds more back (the build's pause, a job stopping at a safe point, the memory guard's
hold, a frozen job: `Agent::may_pack`), the agent asks for as many jobs as it has slots free
(`Ask::want`): a member its lead's coordinator over HTTP (`/work/ask`), the lead its own in process
(`Coordinator::ask_own`, `Agent::lead_grants`). An ask says the memory it spares, its free disk,
cores, conditions, app and member id. The coordinator answers with jobs that fit, packed (§7.2); a
member on another app than the lead's gets nothing (409) until they match (docs/building.md §2, The same
app).

An ask also carries its whole Mac's capacity (`coord::Capacity`, the ask's optional `cap`): the
memory guard's limit and what the running jobs hold, the build's memory (`build_mb`, §7.1, The
build's memory) and what's free of it less each running job's predicted need (the larger of what it holds and its predicted peak: its targets' measured peak on the
lead, the step's first guess on a member), its
cores and those the last minute's load leaves free, the disk free less the owner's room target
and its jobs' reservations (`reserved_mb`, below), and the disk's free space and the target
themselves (`free_mb`, `target_mb`: the build page, `scenic status` and a refusal say the disk as
"38 GB free · 18 GB for jobs (20 GB room target)", `coord::disk_words`; nothing kept, no
parentheses), in use and the cores its
in-use share keeps (`kept_cores`, below), mains or battery,
the NAS's reach, the guard's switch and its running leases. The lead's own asks carry it too, and
it sets it each loop. The coordinator keeps the last per member (`Coordinator::capacities`, leaving out one older than
two minutes, `CAP_FRESH_S`: a busy member asks nothing, so its figures go stale):
`scenic status` and the build page show it per Mac (§10, Each Mac's jobs). Placement reads
only its `cores`, and `kept_cores` while `in_use` (the threads granted, below). It rides the ask, not the heartbeat: the coordinator places at ask time, and the
ask's measures are taken then, where a heartbeat on the NAS is up to two minutes old and the
coordinator doesn't read heartbeats. Every field has a serde default: an older coordinator ignores
`cap`, and a newer one takes an ask without it (an older agent's) as before.

**Mixed versions** (task #247). An ask without a capacity is an older app's, which runs one job at
a time: the coordinator grants it one job, whatever it wants. A member whose last ask said no
capacity (`Coordinator::old_member`; the lead's own agent never counts) turns packing off for the
whole pool: while one is around (heard within two minutes), every ask is granted one job, its
refusal saying `one job at a time: <member> runs an older app …`. Once every member around reports
the new app (`Coordinator::all_new`), packing is on: there's no switch. The old wire is written
only while it isn't: the heartbeat's jobs are `jobs` (every slot's, each with its `slot`), and the
old fields an older reader takes (`job`, the first slot's; `beside`, the second's; `more`, the
rest) are written beside them only while the lead's coordinator says a member runs an older app
(the heartbeat's `all_new` false; a member takes it from its lead's heartbeat, read once a minute,
an older lead's saying none); likewise an ask's `limit_mb`, which its capacity's limit replaces
(`coord::ask_limit` reads either). Readers (`Status::jobs`, the build page, the menu bar app) take
`jobs`, else the old fields; a job not saying its slot (an older agent's) has its place in the
list. An agent on another app than the lead's still gets nothing (409) until they match (docs/building.md
§2, The same app): so a mixed pool is one whose coordinator doesn't check apps (a development
build) or a member waiting for its update.

**Disk reservations** (task #242). Each job granted carries the disk it's measured to take
(`Grant::disk_mb`: its targets' disks summed, or the 90th percentile of the disk its step's last
20 jobs took over their runs (`job-disks-v5.json`), the larger; 0 for a cold start; a halo
step's, `needs::HALO_STEPS`: `pack` and `lo`, the most of its targets' rather than their sum. A
`pack` job takes one z6 tile's map tiles (`build::by_unit`'s groups, each its own offer); `lo` stays
one work of a pass's z3 tiles (the low zooms' gate opens for a pass as a whole). A `pack` or `lo`
job's disk is its own: the parts of its halo's base packs it reads from the NAS, into its scratch,
and its output (task #396; docs/steps.md §8); their runs and targets' disks from when their
jobs copied whole base packs into a cache of the Mac's are dropped once, as `job-disks-v5.json`
is first written (docs/formats.md). `depth-lo`'s (task #398, `coord::REMEASURED`): its job
peaks and targets' measures past three times its targets' median and a GB (a 35 GB peak against
65 MB targets, which kept it on the lead's Mac) are dropped each time the coordinator loads them,
so it runs on any Mac by its measured use. **Shared caches** (task #392, `needs::CACHES`): a `unit`
reads the DEM cache on a Mac; `depth`, `depth-lo`, `terrain-water`, `lakefill` and `water` read the whole copies (task
#551, `localread`: whole local copies of the basemap and the depth sources' packs in the cache's
`blobs/`, made where the disk has room for one past the room target and 10 GB, else the NAS read in
place; their reads are the local disk's, `costin`'s `local`, not in a run's NAS weight). The run that fills it measures it whole, the warm runs after it
their output alone. So a target of such a step counts its own output (its disk floor at most its
step's 90th percentile, `needs::Book::disk`), and the cache is reserved apart. What's reserved
and when:
- *The fill* (`coord::Shared::fills`, `cache-fills.json`): a job that did all its targets (not
  a paused or stopped one) and whose disk passed its targets' output by 512 MB or more, and by
  at least half the fill kept, filled the cache; that excess (its run less its own output, so a
  z6 tile's output isn't counted in the DEM cache's) is the fill. Warm runs, and a part copied
  (a part refilled after an eviction), leave it.
- *What a Mac holds* (`Capacity::caches`, `have_mb`): read off its disk as its agent asks
  (`caches_here`: the bytes of its cache folder's `dem-cache.*` files; of `blobs/layers/basemap*`
  and `blobs/sources/fetched*` for the whole copies), so
  whatever emptied them (room-making, the OSM pass's clear, a hand) counts.
- *The reservation*: the first job granted on a Mac reading a cache (a cold start too) carries
  in its `Grant::disk_mb`, beside its output, the fill less what that Mac holds of it: nothing
  when it holds the fill, the rest when part, the
  whole fill when it holds none or its ask says nothing of its caches (an older agent). The jobs
  beside it on that Mac carry their output alone. Files of an earlier pass count as held until
  the next job reading them prunes them. Its agent counts the grant in
  `reserved_mb` (as `disk_words` and the build page's disk bar show it).
The cache files a Mac has are on its disk, outside the free space an ask says; the
room-target freeing and the disk guard (`room::toward`) can delete those no job holds, but
nothing counts them free before (room-making as a job starts frees the cheap caches alone)),
and its agent reserves it while it holds the lease: the whole of it until
the job starts, then that less what the job has written so far (the disk guard's count). The
lead's own running jobs reserve what their measures say they have still to take (`disk_left`).
A reservation ends with its lease, at hand-off or give-back. The free disk an ask and its
capacity say (`disk_mb`; the lead's own asks' likewise) is the free space less the
owner's room target and every live reservation (`reserved_mb`), so a job not yet written isn't
counted free twice. There is no fixed reserve: the room target is the only floor (a Mac with
49 GB free and a 20 GB target offers 29 GB, less its reservations). An older agent's `reserve_mb`
(a fixed 30 GB it took off too) parses and is passed over: its `disk_mb` has it taken off already,
so it only offers less; the coordinator takes off only the jobs a member holds that
its capacity doesn't name yet (granted since it asked, or an older agent's ask, all of them). The
lead running no job asks for one, its disk not bounded: a stale or high disk floor never holds the
build, its room made as it starts and the disk guard behind it (§7.7). A
job starting beside others waits on the target, its own measured room and the other slots'
reservations (a job alone in the first slot on the target alone); room-making alone makes room for the leases granted and not yet started too, and
the owner's room-target freeing (`room::toward`) frees toward the target plus the reservations,
the jobs' hints merged (`Agent::hints`). The disk guard stays the backstop (§7.7). `scenic
status` shows each member's in its capacity line, as `coord::disk_words` says it (`disk 49 GB free ·
20 GB for jobs (20 GB room target, 9 GB reserved by its jobs)`). Every job is told the target
(`SCENIC_ROOM_TARGET_MB`, room::TARGET_ENV; 0 for the backup and GC, which keep none): the OSM pass
and pass-sets reckon their room past it, and run by hand, without it, keep none.

**Threads granted** (task #241). Each job granted carries the worker threads it runs with
(`Grant::threads`), which the member's agent sets as `RAYON_NUM_THREADS` (the programs taking
workers read it: `scenic-build`'s); costs record them (`threads`), with the CPU time the target
took (`cpu_ms`). A job's measured cores (`needs::Book::cores`) are the threads its runs were given,
or fewer where their CPU time over their wall time says fewer were busy, plus one (a serial step:
1–2), or twice them where three quarters or more were busy (so a grant can climb back; capped at
the Mac's cores). The cores a Mac's jobs may use together (`coord::ask_cores`) are its Mac's
less, while its owner uses it, its in-use share's. Each job is granted its
measured cores within the packed share (`coord::packed_share`: a quarter of those cores, at least
two where they allow: the M1's 8, 2) and the cores its other jobs leave, else the share; a step
reading the NAS as it goes (`coord::NAS_STEPS`: `unit`, elev, pack, lo; or one whose costs show
10 MB/s or more of NAS bytes) the share, never fewer, its threads overlapping the waits. A first
job alone gets no more than the share, so its run is never measured to take every core and leave
no room beside it. The agent, the lead's as a member's,
starts a granted job with those threads, within its Mac's cores less those the in-use share keeps
while the owner uses it and those its other running jobs were given (one at least), and logs a
line per job: `agent: starting <job> (<n> threads: granted <g> of <c> cores, <k> kept for the
owner, <b> given to jobs beside it)`. The coordinator logs the threads with each grant.

**The in-use share** (`crate::agent::share`): the cores a Mac's agent keeps free of its jobs for
its owner (by default 2) while the owner uses the Mac (used within the last five minutes).
`scenic in-use [--cores <n>] | off` sets it in the agent's folder (`in-use-share.json`); `off`
clears it, the default again; with nothing, it says how it stands (and says so where the file can't
be read, the default then in force). The agent reads it each loop, as the disk room target.

**The build's memory** (`crate::agent::buildmem`, task #390): one fixed amount of memory a Mac gives
its build's jobs together, set per Mac, whether its owner uses it or not. `scenic memory <GB>` or
the menu bar's Memory ▸ sets it in the agent's folder (`build-memory.json`); `scenic memory off` or
the menu's The Memory Guard's Limit clears it; `scenic memory` says how it stands. The agent reads
it each loop. It is the memory of the Mac's asks (`Ask::mem_mb`, a member's and the lead's own:
`Agent::build_room`), its capacity's `build_mb`, and so the forecast's (which asks as that Mac's
last ask did), always within the memory guard's limit (`buildmem::room_mb`: the smaller of
the two); not set, the limit alone. The build page's Mac card, `scenic status` and the menu bar
show it.

### 7.2 Placement: the steps table

`crate::agent::steps` has a row per step the agent runs as a job (docs/pipeline.md: its id, title,
what it does, kind, scope and stage, and these resources); the step sets (`SHARED`, `ANSWERED`) are
generated from it at build time.

| Column | Meaning |
| --- | --- |
| memory | a first guess until a target's run is measured (`crate::coord::Cost`) |
| shared | whether members take it, and its rank |
| alone | a whole-build run kept to the lead (`crate::agent::every`) |
| answered | keeps the pass's answers (the lead's alone) |
| write-set | the manifest names its jobs may change (`agent::steps::writes`, §7.3) |

**One scheduler** (`crate::coord::sched`, task #535): every rule below that decides which jobs an
ask gets lives there and nowhere else: `Sched::pack` takes the scheduler's state (the leases out,
the offers, work done and failed, each worker's last ask, the measures, the NAS budget's limits and
waits), the ask and a clock (`sched::Clock`), and returns the jobs granted (`sched::Pick`), changing
nothing but that state: no file, network, wall clock or log. The live coordinator (`Shared::pack`)
calls it on each real ask, then saves the leases, logs and records the grants. The forecast
(`agent::forecast`'s `run`) calls the same `pack` in a loop over simulated time on the lead's state
as data (`sched::State`, `Coordinator::sched_state`, written with the forecast's input): each lane
asks on its Mac's behalf as its last ask did, jobs end at their modelled times and give their leases
back, and a job of a step never measured leaves its measures as its hand-off would. So the schedule
packs as the coordinator packs: by memory, disk, cores, the NAS budget, cold starts, the lead-only
steps and the mixed-version hold. Guards: `tests/grant_rules.rs` fails on a grant rule named
outside the scheduler; `fed_a_recorded_state_the_forecast_grants_what_the_coordinator_grants`
holds the forecast's first grants to the coordinator's; at run time the lead logs where its real
grants since the last forecast departed from that forecast's first hour (`forecast::departures`,
"the grants departed from the forecast"). What stays outside the scheduler moves the finish: the
times and NAS weights are measured estimates, and the repack, the owner's pauses, battery
handovers and a NAS slower than its probe are not foreseen. The backtest
(`backtest_against_the_history`, run by hand on the NAS's `state/`) forecasts from each term's
recorded state the jobs the history shows started in the next 6 h: over 58 starts of 18–21 Oct,
busy time with 1 job at once M4 78 % (history 70 %), M1 71 % (57 %); 4+ jobs M4 13 % (4 %), M1
13 % (14 %). The finish time falls short of the task's done bar: the last job's end is off by a
median 55 % (mean −3 %), for two reasons: the backtest releases all of a window's work at its start,
where the plan released it over the window, and the NAS's limits are guessed (100 MB/s, 40
creates/s), the probe's not being on the NAS. The forecast also assumes no load from outside the
pool (each ask's `cores_free` taken as its cores): while the owner uses a Mac it's optimistic. The
lead logs a departure only when a Mac's jobs of a step differ from the forecast's by 2 or more and
by more than half, each Mac and step at most once an hour.

Rows of note: `trees`/`terrain`/`slope` per z6 tile with `*-lo` assemblies per z3 tile (docs/steps.md);
`bldprep`/`bldtiles` per z6 tile, `bld-fetch` the lead's (docs/buildings3d.md §3.6); `inputs`, a
target per z6 tile, the lead's, planned before every other job (docs/inputs.md §4.3).

- **Every step to every member** (`crate::agent::every`, `crate::coord`'s `pick_every`): all but the
  lead's own steps (`every::LEAD_ONLY`: catalogs, prunes, `bld-fetch`, the answer-keeping steps, the
  worldwide steps) go to any member whose measured room fits, the lead among them. A cold start
  (§7.7) goes, one target, to the first asker its provisional need fits (or one holding no job),
  while no job of its step runs anywhere.
- **The lead's own steps by grant too** (`Agent::offer_plan`, `coord`'s `pick_own`): the lead
  offers its whole plan in its order (the publishing chain among it), each offered step's targets
  where its work first comes and each of its own planned jobs whole (`Offer::job`). The authority
  rule (`every::grants`): a step kept to the lead, and every planned job of the lead's, is granted
  only to the current lead's member, asking in process; a member's ask over HTTP never gets one.
  One job of such a step runs at a time; its measures (memory, disk), the NAS budget (always, §7.7)
  and a cancel's hold hold it as they hold a member's job. Its cold start (none of it measured)
  goes by its provisional need (§7.7) like any other: beside the lead's jobs when it can; when it
  can't, the lead is granted nothing more until its jobs end, then that job before any other
  (`own_cold_waits`), so offered work never starves it. A
  planned job that can't start for a reason of the agent's own (a failure's wait, the proxy not
  up, the memory guard's hour: `wait_reason`) isn't asked for. One that fails (or the memory guard
  stops) ends its lease failed (`finish_failed`): its targets are kept from the lead an hour,
  doubling each time, as a member's failed work is.
- **Packing** (`coord`'s `pack`): a Mac may hold several leases. An ask says how many jobs it would
  start (`want`); while every member reports the new app (§7.1, Mixed versions; else one each) the
  coordinator grants up to that many in one answer (`Grant::more`, each its own
  lease), packed in plan order into the ask's room: each after the first the Mac holds must fit,
  by its measures (memory, disk, and cores: `needs::Book::cores`, within the packed share, §7.1;
  cores not said count as the share) or a cold start's provisional need (§7.7), within
  what's left: the ask's memory and disk (the build's memory on its Mac, §7.1, `Agent::build_room`;
  and its free disk) and its Mac's cores (less the in-use share's
  while its owner uses it, and less the cores busy outside the pool: those its last minute's load
  average keeps busy (`Capacity::cores_free`, in each ask's capacity) past the threads its
  jobs were granted (`Lease::threads`), an excess within the packed share counting none (the
  load lags a minute and is rounded up: a job just ended), `coord::outside_cores`; task #440: a Mac swamped by other work, builds and tests,
  gets no job beside those it holds, and its refusal says the cores busy outside the pool), less the measured needs of every job it holds and of those granted
  with it. A lease counts until it ends or lapses: a restarted Mac's old leases hold its room
  until their TTL runs out. The room left is never more than what the Mac's jobs hold now leaves
  (the ask's memory less `Capacity::held_mb`, its memory guard's last sample): a cold start holding
  past its provisional need counts at what it holds. With placement on, a member holding a job whose cores, as its last ask
  showed, are filled by load from outside the pool with its jobs' (`Shared::swamped`; its own
  jobs alone filling them don't count) isn't among those a target is drawn to: the others take it. Each job is as many targets as fit a quarter hour (`JOB_S`) by their measured
  times (an unmeasured target's: its step's mean, the latest earlier way's when larger while the way it
  runs now has fewer than `needs::PRIOR_RUNS` runs, those being the quick ones first to end), a step kept to the lead in one job. The agent starts every job granted, each in a slot of
  its own; the slots past the first two are made as jobs are granted and dropped once empty. Each
  slot has its own files in the agent's folder (`job-<k>.json`, `control-<k>`, `costs-<k>.jsonl`,
  `done-<k>.txt`, …, the first slot's without a number) and scratch folder (`scratch-<k>/`), and
  its own name as a worker (`<host> #<k>`). Each job runs with the threads granted it (§7.1,
  Threads granted).
- **Order and locality:** a Mac gets a contiguous run of plan targets, from the front or just
  after its last target, as many as fit `coord::JOB_S` (a quarter hour) of measured time. A
  member's asks try the shared steps first, by their rank, then the others in plan order; the
  lead's own, the plan's order.
- **Placement by how crucial** (`crate::agent::placement`, behind `state/pool/placement`): asks are
  tried on steps by their forecast slack (least first), within a step from the longest target; each
  target is drawn among the members that could take it, weighted by measured speed
  (`placement::draw`, stable per minute window), the lead's own agent among them. A task offered
  ahead goes to a slower worker only while its head start covers that worker's run. The lead's own
  planned jobs keep plan order.
- **Resuming** (`crate::agent::resume`, behind `state/pool/resume`): a job interrupted with progress
  on a Mac's disk (built for the OSM pass) is offered to that Mac alone for 24 hours from its first
  start, or until `scenic pool release <member> "<job>"`. Progress is in
  `state/pool/progress/<member>.json`, written only by that member. A finished one is held unoffered
  until merged.
- **The forecast runs off the loop** (task #544): the lead makes its input on the loop and runs it
  on a thread of its own (`Agent::start_forecast`); the status keeps the last one done, so reaping,
  starting jobs and writing the status never wait on it. Its three runs go side by side (three
  threads, each at the utility QoS: `forecast::low_priority`), cut at `forecast::BUDGET` (15 s): one
  cut is `partial`, its `why` says so, no finish shown, and the build page marks its lanes and next
  jobs partial. On 21 Oct's live input (4,155 targets, 20 lanes) it took 3.7 s (205 s before #544).

**A job waiting on a worker** (`crate::offload`'s `WAITING_ENV`): a z6 tile job waiting on a faster
worker for its tail writes `SCENIC_WAITING`; while it exists the lead's asks count the waiting
job's current memory and none of its cores (the room past its measures given back to the ask,
`lead_grants`). A job started beside a waiting one **yields** to it (stopped at once, given back,
not counted as a failure, no floor learned) when it resumes and the jobs no longer fit together.

**The memory guard** (`crate::agent::memguard`; `state/pool/memory-guard`, on unless it says `off`):

- A thread samples each job's processes' footprint every 5 s, per target under way (from
  `started` lines in `SCENIC_COSTS`, else `SCENIC_DONE`), and writes `memory.json` for the status.
- Every child started through `store::spawn` (the agent's jobs, their programs) is started without
  macOS malloc's cache of freed large blocks (`MallocLargeCache=0`, an undocumented libmalloc
  setting, `store::spawn::NO_LARGE_CACHE`; their children inherit it; the agent and server, started
  by launchd, keep it): with it, freed blocks stay in a
  process's footprint, and a dense z6 tile's scenic-build held 6–7 GB of them while its programs ran.
- **Limit:** the Mac's memory less an eighth, 4 GB at least. Over it, the largest fitting alone,
  another job stops at its next safe point (`memguard::decide_by`): one that yields first, else the
  newest (by its start's minute), then the least progressed, never the largest; one at a time, and none more while the
  jobs not draining fit the limit. Nothing new starts beside the largest meanwhile (a member asks
  for nothing). While the Mac is short of memory (pressure warning, or swap growing a GB), no
  safe point is waited for: the job that would drain is frozen at once (SIGSTOP), then the next
  while the rest still pass the limit. A frozen one goes on once the Mac is no longer short or the
  largest job ended, and is stopped (given back) after a minute frozen with the Mac still short,
  or the 15 minutes a drain gets. The largest alone past the limit stops at once; else that one
  too stops at a safe point. This holds on every Mac's slots. A drain that finds no safe
  point in 15 minutes is stopped. A stopped job is given back and kept from that Mac for an hour.
- **A member's jobs far over what it spared them** (`Agent::over_grant`, task #28): each job a
  member is granted keeps the memory its ask spared (its `mem_mb`: the build's memory, §7.1). Once its jobs together hold more than half as much again (4 GB more at least,
  `far_over`), capped at the Mac's limit, the largest stops at once, the Mac short of memory or
  not; never a job beside it. So a member's jobs past the limit meet this first: the largest stops
  at once, not at a safe point. Past that mark by itself, or the member's only job, it's given back,
  not failed (no failure at the lead), and what it held goes with the give-back as an alone floor of
  its target under way (below), so the lead offers that target only to a Mac that spares that much.
  Otherwise (only the jobs together past it, or its target under way unknown) it ends as failed: the
  lead keeps it from that Mac for an hour, doubling, and its floors go as any job's.
- **An update's drain** (a newer app installed): every job stops at its next safe point; the agent
  restarts once every slot is free, each job given its own
  wait (its target under way's measured time, else its step's fit, else the 90th percentile of
  its step's runs, `needs::Book::secs_p90`, ×1.5, at most 3 h, at least 15 minutes); a job still
  running then is left running, its lease kept, and the new agent takes it over (docs/building.md §2,
  Restarts), as after any restart but a quit. The new agent may start while the old one still
  holds the member lock: it tries the lock for up to two minutes, joining as it starts; past that
  it starts out of the pool, its jobs' leases still beaten with the lead, and restarts into the
  pool at once when the lock comes free, its jobs left running for the next agent (docs/building.md §2,
  Joining after an update).
- **Floors**, learned whatever the switch: the most a target held is its floor
  (`floors.json`, and the pool's state), rising only, tagged with whether it ran alone and the
  step's `cost_version`. A target's predicted memory is never below its floor. A target whose
  floor exceeds this Mac's limit goes to a Mac with room; past every Mac's, it waits; for a lead-only
  step, the status names a member with room, for the owner to hand the lead to by hand
  (`scenic lead give`) if wanted. `scenic pool floors [--clear
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
- **Sizes ride along:** `Out` records each file it writes with its size, and the hand-off carries
  them (`Handoff::sizes`); the merge keeps those of the files the entry names, so a delta catalog
  never looks at them on the NAS. Entries from before carry none.
- **Lease order holds:** an older lease's entry for targets a newer one set changes nothing.
- **The journal is a log:** nothing is removed on merge. Planned (task #455): GC of day folders over
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
free. A lead whose update waits on its jobs keeps the lead and restarts into the new app at a safe
point like any member (docs/building.md §2, A newly installed app). The lead's own asks always carry its capacity, so the lead never
holds packing off for the pool (§7.1, Mixed versions); its heartbeat says whether the pool is all on
the new app (`all_new`) for its members' heartbeats.

### 7.7 What a step needs, by structure

Rule: nothing depends on a hand-kept figure. A step's needs are **measured** per target, **enforced
where the resource is used**, or **derived from the code**. A wrong declaration may cost speed,
never correctness, politeness or work.

| Need | How |
| --- | --- |
| memory | measured per target (`peak_mb`, the guard's floors, §7.2) |
| input size | measured per target beside its cost (`sizes` in `costs.jsonl`, `pipeline::costin`: bytes read, threads, area, GeoTIFFs opened, the bytes and files its `Out` made on the NAS (`wrote`, `creates`), and the counts a step has: ways and vertices, buildings, peaks); read by the step's model (below) |
| disk | measured per target: bytes written through `store::cachefile` and the scratch writers (`SCENIC_WRITTEN`), plus a look each minute at the largest files in a slot's scratch folder; kept as the target's disk floor (`disks.json`). **Disk guard:** when free space falls below the owner's room target (less a thirtieth of it, so a job just let in at the target isn't frozen at once: `memguard::disk_short_of`), the fastest-growing job is frozen (another too if the disk falls past half the target), room is made from caches no job holds (`room::toward`), and it goes on once the target is free again or is given back. No target is set until the owner picks one (Off is none); with none, the guard fires only at an actually full disk, with no other floor. Given-back jobs' scratch folders are deleted oldest first when room is short (`room::free_given_back`) |
| jobs on one Mac | a job is granted beside others only if their measured memory, disk and cores fit (packing, §7.2), and only while every member reports the new app (§7.1, Mixed versions); a cold start by its provisional need (§7.7) |
| raw tiles | `store::cachefile`'s locks; packing deletes only tiles no job holds; a job reopens its shard's archives when the index changed (`RawTiles::get`, `refresh_shard`; a shard is the z6 tile, z3 tile or `root` whose archives hold a tile: docs/macs.md) |
| Wikimedia budget | `crate::budget`: one budget for the pool, enforced at egress. Every job's connections go through its agent's proxy (`crate::budget::proxy`, `HTTPS_PROXY`), the job sandboxed (`sandbox-exec`) to deny any other way out. A metered connection takes a token first: leases are create-new files per slot of a ten-second window (`state/pool/wiki/<class>/<window>-<slot>`). `wiki` four requests a second; `query` (WDQS, QLever) one query per two seconds of query time. A 429 told to the proxy (`POST /hold`, dem/budget.py) holds every Mac. The NAS away, each Mac takes the rate divided by known members. Lint: `tests/budgets.rs` |
| NAS and network budget | `crate::budget::nas`: the lead's own jobs always; the members' too unless `state/pool/nas-budget` says `off` (on while missing). A job's **NAS weight** is its share of the NAS in thousandths: its targets' measured rate (`needs::Book::nas`: `bytes` read, less those of this Mac's own disk (`local`, task #551), plus `wrote`, and `creates`, each over its wall time, a halo step's (`pack`, `lo`) `wrote` alone, its job's read rate (`nas_read` over `nas_secs`: what the job read of the halo's base packs from the NAS and the time that took, on each of its targets; a measure without `nas_secs`, task #396's first, counts no reads) in place of its `bytes` (the halo's base packs' files); its own run's, else its step's 90th percentile of each, its runs of every way it ran; none measured, 0, but a step known to read the NAS (`NAS_STEPS`) weighs the whole, task #499 review), and its metadata work (task #451: `listed`, `stats`, `deletes` from `costin::meta`, the folder entries it listed and the files it statted and deleted (renames counted as deletes), each over its wall time; counted by GC's walks, which note a cost line `gc gc` in their job's costs file, and the inputs step's drop-box listing), each over the NAS's limit, added: one combined weight, at most the whole (1000); a job its heaviest target's (a job of no targets of its own, its lease's, its step's). **The limits** are measured: the lead's coordinator probes the NAS every 30 minutes, whatever the switch says (`probe`, 4 streams at once (`STREAMS`, task #534: on 21 Oct one stream measured 5 creates/s and 25 MB/s where four made 41 creates/s and eight 57 MB/s, so a single stream set limits a fifth to a half of the NAS's): in each, 12 files of 4 KB made, then in each one of 16 MB written, synced and read back past the cache; the 48 small files deleted one by one, and a kept folder of its own (`<host>-meta` beside the probe's folders, 48 files, never removed by the stale-folder clean-up) listed and its files statted, measured only once it has gone unwritten 5 minutes so the answers come from the NAS, not this Mac's SMB cache of what it just wrote (missing or young: made, and 0 this probe; a 0 weighs nothing and the day's best is kept), in a folder of its own under `state/pool/nas-probe/<host>-<pid>`, all removed after; as it starts, the coordinator removes its host's folders a stopped process left, never another host's) and takes the best of the last day's probes (a probe under the build's load measures what's left). **The gate on the NAS's own load** (`nas::Loop`, task #534): every 10 s the lead reads each disk's busy share from the Synology over SNMPv3 (`crate::budget::snmp`, SYNOLOGY-STORAGEIO-MIB `storageIOLA`, the one SNMP reader, for the build page's figures too; its keys `NAS_SNMP_USER` and `NAS_SNMP_AUTH` from the project's `inputs/keys.env`, handed to `snmpget` in a private `snmp.conf` (0600, in a 0700 folder under the temp folder, `SNMPCONFPATH`; made once per lead, removed when its coordinator stops; `~/.snmp` untouched), never on a command line, never logged; a poll killed after 8 s) and smooths the busiest disk's (a new reading weighs 0.3: about half a minute's memory). While that load is over the target the budget is held (`Limits::held`): a job that weighs anything is granted, the lead's or a member's, only beside leased jobs that weigh nothing (`Limits::open`, on top of every rule above; weights and room are the probes' as ever, so a held NAS never admits a job an unheld one refuses), whatever loads the NAS (an outside repack counts); jobs running go on. Held, the lead's waiting lines say so: a line for the NAS ("held by NAS load (91 % > target 85 %)") and each target held back for the budget names it too (`Coordinator::nas_held`, `agent::nas_wait_lines`). With no keys or no answer, the gate is open (the probes' limits alone). **The target** is the owner's, pool-wide: `state/pool/nas-target.json` in the project folder, `{"pct": <30–95>, "by": "<who>", "at": <unix s>}`, set by `scenic nas-target [<percent> | off]` on any Mac (off: the file removed, the default; with none, how it stands); it lives on the NAS, not an agent's folder, because the lead alone enforces it: a member's change reaches the lead at its next read (10 s), and a new lead reads the same file. Missing or unreadable, 85 % (`DEFAULT_TARGET_PCT`): never 100, after #451's stall at a NAS held busy, and above the 65–80 % an outside repack alone kept the disks at on 21 Oct, so the pool still works beside one. `scenic pool nas-probe [--dir <folder>]` runs one. **Enforced at grant** (`coord`'s `pick_every` and `pick_own`, so `pack` too, the lead's own jobs among them): a member's target is granted only while the weights of every job leased in the pool (the lead's own among them) leave room for its own within the whole (1000), or while no other member's job weighs anything (the lead's own alone never hold it back), or, weighing the whole itself, once it has waited 10 minutes for the budget alone and no leased job weighs the whole (`Limits::fits_job`, `OWN_AGE`: two members' jobs never starve it); the lead's own only within the whole, or while nothing leased weighs anything (`Limits::fits_own`); a planned job of the lead's own also while no leased job weighs the whole, once it weighs the whole itself, or has waited 10 minutes for the budget alone and the lead's own leased jobs leave it room within the whole (`Limits::fits_own_job`, `OWN_AGE`, the wait timed from when it was first held back and forgotten once granted: members' small jobs never hold it back for good, and the lead's own aged jobs don't pile up past the budget; the simulator checks each start of the lead's own against the same rule, with the waits as the coordinator kept them); **an aged target holds its room** (`Shared::nas_reserved`, task #444): of the targets held back for the budget alone for 10 minutes or more (and passed over in the last two minutes), the one held back first has its weight kept free: no other job weighing anything is granted, a member's or the lead's, unless the leased jobs, it and that one fit within the whole, so a target weighing less than the whole but more than the room left (depth-lo's) goes as the leased jobs end instead of waiting behind a stream of smaller ones; it never lets the budget past the whole, the reserved target itself aside (a member's whole-weight job going by `fits_job`'s aged path, or an aged job of the lead's own by `fits_own_job`); a target is noted waiting only when its asker would take it but for the budget (its measured memory and disk fit, or a cold start it's the one to make), so one no asker can take never holds room; a cold job of its own waits for the lead's jobs to end (§7.2) only when it would fit with the members' jobs alone. **The lead's own jobs are always held to it**, whatever the switch says (the limits probed: `nas_own`); the switch, unless `off`, holds the members' too (`nas`). Switched `off`, the members' jobs are not held and the lead's own are held by their own weights alone (`Shared::nas_room_heaviest`: a member's unheld jobs no longer keep the lead idle). The rest wait for a job to end; a target held back for over 30 minutes is a waiting line in the lead's status (`Coordinator::nas_waits`), once each (`agent::nas_wait_lines`). **The clean-up waits for a quiet NAS** (`QUIET_STEPS`: `gc`; task #451, after GC beside two depth-lo jobs held the NAS at 34 % IO wait and all three stalled): it starts only beside leased jobs weighing nothing, or, its weight measured, within the whole with them (`Limits::fits_quiet`), and the room held for an aged target; unmeasured, also only while no leased job is of a step known to read the NAS (`NAS_STEPS`) or unmeasured for it (`nas_unknown_running`). Its wait isn't noted (it holds no room) until it has been held back 24 hours since it was last granted (`QUIET_AGE`, `Shared::quiet_since`, kept in memory: a new coordinator counts afresh): then it goes as an aged job of the lead's own (`fits_own_job`, its wait noted, its room held). Running, it yields at its next safe point (each folder of its sweeps; `control::draining`, its cost line noted first, ending as paused) when a target held back for the budget `OWN_AGE` doesn't fit beside the leased jobs but would without it (`Coordinator::nas_yield`, asked by the agent each loop). **Downloads:** a job of a fetching step (steps table kind `Fetch`) counts one, capped pool-wide by the switch's `downloads <n>` (no cap without it): the lead's own fetching steps (`bld-fetch`, `rail-feeds`; none is offered to members). Not held to it: tasks |
| a pass's answers | `crate::answers`: a lease on the NAS (`state/pool/answers.lease`, renewed every 150 s, lapsing after 10 min) around each sync; on a Mac a reader/writer lock (`answers::Local`) |
| write-set | `agent::steps::writes`, enforced by `Out` and the merge (§7.3). Raw archives, answers and `bld-fetch`'s sources are outside the manifest, each written whole under its module's names |
| targets per job | as many as fit a quarter hour of measured time (`coord::JOB_S`), the lead's and a member's alike; a step kept to the lead in one job |

**One model per step** (`pipeline::needs::Book`: `need(step, target, threads, guess)`, and `mem`
alone for the picks): only what was measured the way the step runs now counts
(`coord::cost_version`); a run or floor of an older way of it (a z6 tile's before v3, its elevations sampled whole; the heritage job's before v1, its clip of the filtered planet, whose 21.9 GB floor no run since 20 Oct came near: 6.4 GB) is neither a
target's memory or time measure, nor a fit's run, nor part of a prior (disk floors, `disks.json`, carry no version and always count). A target's memory, disk and time are its own
run's (memory never below its floor; a floor alone counts as its measure); else base + k × size
fitted over the step's measured runs (least squares raised to cover every run, × 1.1, capped at the
most a run it was made from measured; threads a term when the runs differ in them), on whichever size
fits best (a `sizes` measure the target has from an earlier run, or its tile's area); else, for z6 tiles
and landmark candidates, their extract-size first guess (`coord::unit_peak`), so one dense z6 tile doesn't
set every other's; else the step's prior: memory and disk, the 90th percentile of its targets'
measures (memory: their floors among them), a heavy target held by its own floor; time, the mean
of its targets' runs, the expected time a job's targets are counted to the quarter hour by and the
forecast sums (an update's drain alone waits by their 90th percentile, `Book::secs_p90`, so a long
tail's long jobs aren't cut short); else the caller's first guess (the steps table's, or the size
the plan offered): a cold start. A fit is judged on runs held out of it
(5 folds) and not used when it predicts under on more than 5% of them, is off by more than 50% on
the median one, or has fewer than 10 runs. The models are kept until a cost, floor or disk floor
changes. The coordinator's picks (`pick`, `pick_every`, `measured`, `secs_of`, `peak`), its floors
and the forecast all read it. `scenic pool needs --fit` prints each step's fits and their held-out
errors (memory, wall time, disk) beside the prior's, and lists the steps that don't fit.

**A cold start** (no target of its step measured; task #499) is packed by its *provisional need*:
the most its step declares (the steps table's `mem_mb`) or it was offered with, times 3/2
(`coord::sched::provisional_mb`), its disk its step's measures' (else none); its cores the packed share
when its step is known to honour the threads it's granted (`coord::THREADS_HONOURED`: `slope`, on
the rayon pool the agent sizes; `bld-fetch`, its downloads bounded by `--jobs`), else the whole
Mac. It goes beside other jobs when that fits the room left, and other jobs go beside it by the
same figures; one bigger than a Mac's whole room goes only to a Mac holding no job. At most one
cold start runs on a Mac (two unknown needs never meet), and a step never measured in any way (no
cost, floor or disk of any way it ran: `never_measured`) runs alone, nothing beside it, as every
cold start did before task #499. A cold job of the lead's own that can't go beside its jobs
holds the lead's asks until they end, then goes before any other work (`own_cold_waits`). One job of the step
runs in the pool at a time (`pick_every`, the lead's `cold_under_way`), so the step is measured
before it fans out. Its Mac's room left is never more than what its jobs hold now leaves (§7.2,
`Capacity::held_mb`), so one holding past its provisional need is counted at what it holds as soon
as the guard samples it; one that grows past the Mac's limit is the memory guard's (§7.2), which
learns its floor. After one run, the step's other targets are offered by its model (above). A floor never stalls the build: the first slot starts once
room-making has done what it can, the disk guard as backstop.

Planned: `cost_version` derived from the step's key version (`crate::agent::build`'s `*_V`) rather
than kept by hand; progress of steps other than the OSM pass resumable on its Mac.

### 7.8 The agent never waits on one NAS call

An open on the SMB share can stay stuck in the kernel for many minutes while the same file reads at
once from a shell. The agent's NAS calls listed below go through `crate::agent::nasread`: each runs
on a thread of a small pool and is waited for a deadline at most (20 s; a listing 60 s; a create in the terms' folder 5 minutes, so a term made but not written comes back to the driver to finish rather than lost). Past it,
the caller gets a timeout and takes it as any call that fails now, logged once per path until that
path answers again. The stuck thread is left to its call and stays busy; while every thread of a
pool is busy, a call to it fails at once, so stuck threads never pile up. Three pools, so one kind
stuck can't starve another:

- **Switches** (two threads): `state/pool/resume`, `placement`, `brokers`, this member's hold
  (`state/pool/holds/<member>.json`), the memory guard's and
  publishing's switches. A switch that can't be read keeps its last value.
- **Records** (four threads): the members' progress files; the loop's reads of `state/status.json`,
  the manifest (where planning reads it through `nasread`) and the regions' extract (unreadable for
  a reason other than missing: no extract job planned); and the pool's reads, stats and listings
  (`Deadlined`, which the agent opens its member over: the driver's ticks on terms, mail and
  members, and the terms' listing as the member opens, tried three times, 10 s and 30 s apart,
  then gone on without).
- **Writes** (four threads): the heartbeat (`state/status.json`, or a helper's file) and the pool's
  writes, creates and removes (the members' heartbeats, mail, terms), and the members' holds. A write to a path is refused
  at once (a timeout) while an earlier write to it is still in flight, its thread stuck: two writes
  never share one temporary name. The path is free again once that write ends.

A healthy share's calls are unchanged.

A watchdog thread, started before the agent opens anything on the NAS, looks at when the loop last
finished a round (or the agent started, given five minutes more: a slow start isn't a stall): past
five minutes it logs the stall and keeps
`loop-stall.json` in the agent's folder (this Mac's disk), which `scenic status` shows as "this
Mac's agent loop is stalled"; it's removed when the loop goes on.

Still under no deadline (planned): the NAS's mount and probe (`root`); the build's keys
(`build::Keys::load`, `load_with`); the manifest read through `out::read_record`; the regions'
held report (`inputs::read_report`); the helpers' listing (`state/helpers`); the coordinator's
contact published and removed; GC's reads of the journal and records;
the caches' trims and clears; and every NAS call of the coordinator's and the jobs' own.

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
  `Authorization`, `X-Worker`, `Range`, `PUT`): their addresses, and the public names a proxy on
  one serves its page under (its broker's `pages`, from `~/.config/scenic/names`: the M6's
  https://map.brandontsang.net/build). HTTPS needs `tailscale serve` on each Mac (§14).
  Chrome's Local Network Access may still block calls to another member; the page's own member is
  the sure path.
- **One page, any member:** the page loads from any member and moves to the next that answers when
  its own goes away. The agents' token is the pool's (`state/coord/token`).
- **Trust:** a worker found wrong is not given more work (workers.md §5).

## 9. The pool's API

Members' messages go by mailbox on the NAS (`state/pool/mail/<to>/<from>.json`, one writer each,
read by member id). Jobs go through the lead's coordinator (`/work/*`), tasks through each member's
broker (§8), and the worker page's lead asks through `/work/lead`. Every member answers on port
8090. A member that can't reach the lead keeps its jobs running and starts nothing new. A member's
agent passes its owner's job cancels to the lead through `/work/cancel` (§11).

Planned: an HTTP pool API (`POST /pool/lead`, `/pool/status`) in place of the mailbox.

## 10. Seeing the pool

- **Heartbeat**, `state/pool/members/<id>.json` (its own writer): label, app, resources, conditions
  (`conds`), slots' jobs, tasks, addresses (`broker`), reach (§6.8), term led, `handing_to`,
  `ready_for`, `stood_down`. Written when it changes, at least every two minutes. Listed every two
  minutes; a member missing from listings for twenty minutes is forgotten.
- **Build status**, written by the lead per term (`state/build/term/<E>/status.json`): the
  checklist (a line per stage with an item per step: docs/pipeline.md §4), forecast, regions,
  waits, what publishing does, publishing as it stands (`published`: the last catalog and its
  kind, the map tiles being drawn by Mac, each Mac's adopted catalog), recent jobs, pause, last
  catalog, the lead.
  A member writes `helper.json` instead.
- **Coordinator view**, written by the lead per term (`state/coord/term/<E>/view.json`,
  `Coordinator::view`, in its pool step: `pool_lead_files`): `{at, view}`, the view what the
  worker page's `/work/swarm` takes from the coordinator's memory: the pause, each lease (worker,
  step, targets' count, progress, `frac`, when granted, its last beat and when it lapses), each
  worker (page or member: label, kind, memory, cores, done/failed/bad, visible, `can`, `fit`, when
  last seen, its paces), the tasks by state and `task_mb`. Times are unix seconds, so it is the
  same until something changes. Written as a whole (create and rename) when it changed, at most
  every 15 s (`pool::VIEW_EVERY`), and at once in a new term. Each time is taken from one
  anchor (an `Instant` and the wall clock read together, taken again only when they drift apart
  by more than 2 s), so an idle pool doesn't rewrite it. Read by each Mac's page data (below).
- **Forecast input**, written by the lead per term (`state/build/term/<E>/forecast-input.json`,
  `forecast::Record`): the forecast's whole input as data (the work left, each region target's
  cost in a table by step and target, the lanes, what runs now, each Mac's room, the cadence) and
  what the lead adds to its answer (each step's time a target, the slots waiting on a tail), so `Record::forecast` gives the lead's forecast on
  any Mac. Written as a whole in the lead's next pool step each time it makes the forecast (at
  most once a minute); zstd-compressed when its JSON is over 2 MiB (`forecast::ZSTD_OVER`; a
  reader tells by zstd's magic number). About 70 KB with today's work left (88 regions, ~500
  targets, 24 lanes); about 2 MB for a whole rebuild's. Every time and speed in it is made
  finite before it's written (`Record::clamp`). Read by each Mac's page data (below).
- **The page's data on every Mac** (`agent::pageview`, task #409): in the pool, each Mac's port
  8090 answers `/work/swarm` and `/work/history` from its own read of the NAS, the same JSON on
  every Mac (`/work/steps` is the steps table, the same everywhere); out of the pool the
  coordinator answers from its memory, as before. A thread of the agent reads only while a page
  has asked in the last 60 s, every 12 s at most, by known name (no listing): the lead's
  `state/status.json`, the term's `view.json`, its `forecast-input.json` (again only when its
  time changes), `state/build/pause.json`, each member's heartbeat (`state/pool/members/<id>.json`,
  for its host) and own status (`state/helpers/<host>.json`), and what each member's history
  file of the two newest days gained since the offset it last read to (whole lines; from the
  start again when the file is now shorter than that). The agent's loop hands it only what it
  holds already (the term and the members' ids from the pool's driver, this Mac's app): no NAS
  read on the loop, and this Mac's lanes worked out only while a page asks. Every read is done
  before the pageview's state is locked, and the results swapped in after, so a slow NAS never
  holds up an answer. The history is merged by time, writer and number, and numbered by this Mac
  as it arrives; the answers carry the numbering's `epoch` (new with each process's first read),
  and the page starts its cursor again when the epoch changes or `seq` falls below its last event
  ("since you last looked" goes by the events' time, so it outlives a restart). Until its first
  read a Mac answers 503 (never its memory's views: another numbering), and the page says it's
  reading the build's state (quietly, not as an error) and asks again every 2 s, saying it as an
  error only after a minute of 503s (`sums.js` `notRead`; a 503 from `/work/history` alone waits for the next 10 s poll; no retry while the page is hidden). The menu bar app asks `/work/swarm`
  every 50 s while it runs, so this Mac keeps reading and its panel has the build when it opens. The forecast is run here from the input, its time brought
  to now and this Mac's own lanes as its slots are now, worked out as the lead works out its own
  (`forecast::SlotJob`: its own estimate, or its targets left at its speed, a minute at least; its
  running job too); a target missing from the cost table is a bug, logged once. It runs on a
  thread of its own, again when the input changes or a minute has passed (`pageview::Runs`): the
  answer carries the last one done. Times are given
  the page as ages (`coord::ages`). The answer carries the app serving it (`app`). With a lead on
  an app from before these records (no `view.json`, no `forecast-input.json`), the answer has no
  leases, workers nor tasks, and the forecast is the one in the lead's status. A Mac with neither
  a coordinator nor a broker serves the page from a viewer (`Coordinator::viewer`): the page and
  its views, and this Mac's own page's asks (§11), every other call refused (its key's too); no
  state folder of the agent's (its token alone, in a scratch folder removed as it stops, a dead
  process's as the next starts), no history, no background loop. The usual Origin rules hold. A
  history file written again in its place (its inode or first line changed) is read from its
  start, the events taken in already passed over. The menu bar's panel loads this Mac's own page
  alone (docs/building.md §2, Status).
- **Each Mac's jobs** (the build page's machines, `scenic status`, task #246): a block per Mac,
  the lead's first, marked coordinator with its ramp (§7.1); its capacity as the coordinator last
  heard it (memory held of the guard's limit and free for more, cores busy, disk reserved and
  free, on the page as bars), its jobs' share of the NAS budget (`Coordinator::nas_shares`, the
  status's `nas_share`: their leased jobs' NAS weights in thousandths of the whole); a block per
  running job numbered by its real slot (`JobView::slot`: slots may be sparse, so a Mac running
  only its fourth slot shows slot 4), with that slot's lane's next work; and why nothing more
  fits: the packing's refusal as its last ask was packed (`Coordinator::refusals`, the status's
  `packing`: a cold job of the lead's own waiting for room, cores short for the next job open, or nothing
  offered fitting the room left), shown while the refusal itself is younger than two minutes
  (`CAP_FRESH_S`), older ones dropped. The forecast's lanes are per slot
  (`<host>`, `<host> #2`, …), as many as the slots each Mac may run now (`forecast::slot_lanes`):
  the lead's its ramp's, each member's `agent::job_slots` of the cores of its capacity (else a
  lane per job it runs: an older agent's jobs, not saying their slots, by their place), never
  fewer than its last running slot's number, and the lead's slots its ramp grows to later (one
  each clean stretch, opening then). Each lane's next job is the coordinator's scheduler's grant to
  its Mac (§7.2, One scheduler), the Mac's room shared by its lanes as the scheduler packs it (a Mac
  whose ask isn't recorded: `forecast::MacRoom`); each
  lane starts with its slot's running job and its time left (a member's without progress said:
  its step's measured time). The page names a lane by its Mac's chip and slot ("M4 Pro 2";
  `cond::Resources::chip`). Each lane runs at its slot's measured speed (else the lead's other
  slots' guess, four fifths of its pace, or a member's): slots on one Mac share its cores, so the
  forecast is optimistic until each slot's speed is measured under that sharing.
- **Views** (menu bar, worker page, `scenic lead`, `scenic status`) read the
  agent's `pool.lead` (`crate::agent::lead::View`) or the fresher `agent/pool/view.json`
  (`lead::own_status`): each member's state and whether the lead can be handed to it
  (`Driver::hand_to`), what a takeover needs (`Driver::takeover`), "no lead" and why, the last ask
  and change of lead.
- **The alert** (`lead::alert`, `ALERT_S`: an hour with no lead) says no Mac took it over by itself
  (none could lead, or the app rule refused) and names the command to take over; the menu bar
  notifies once.
- **History:** term changes, step-downs and handover events are `term` events in the member's
  history file (`crate::agent::lead::notes`).

## 11. Controls

`crate::agent::lead`, `tools/status/main.swift`, `web/work/pool.js`:

- **Menu bar:** on the lead, "Hand the Build To ▸" with each member's state (greyed with why); on a
  member, "Make This Mac Lead", or "Take Over the Build…" with no lead in touch (says what it forces
  or downgrades); Pause/Resume.
- **Worker page:** "Make lead" per member, a handover's progress, "Take it" (unforced). On a Mac not
  leading these, and Pause, are this Mac's own page's alone (its viewer or broker answers none from
  elsewhere): its agent passes them on (the pause as its menu's, `crate::control::request`; the
  lead asks and the inputs' acceptances from their queues, as on the lead).
- **Cancel and try again from the page** (task #409): this Mac's own page (the menu bar's panel, a
  browser on this Mac: `/work/act`, refused from elsewhere and with a key) cancels a running job
  and tries a failed target again. A cancel goes the way `scenic job cancel` does: on the lead to
  its agent; on a member, an ask in its agent's folder (`crate::control::request_cancel`), passed
  on to the lead whatever app it runs. A retry: this Mac's own back-off for the target forgotten at
  once; on the lead, its coordinator's failures of the target too (no worker held off it,
  `Coordinator::clear_failed`, in the state it writes this step, so a restart keeps it); on a
  member, mailed to the lead (`Msg::Retry`), only to a lead on its own app: with none, or one on
  another app, `/work/act` answers 409 and the page says why. Each mailed message is read alone, so
  one an app doesn't know is passed over and read past. **Still open:** a lead rolled back to an app
  from before these messages fails to read the whole mailbox of a member that mailed one, until
  that message leaves the member's last few sent (`KEPT`); and a member rolled back reads its own
  `mail.json` (its sent messages) as empty, so it may take messages again.
- **`scenic lead`**: shows the pool; `scenic lead give <member>`, `scenic lead take [--force]
  [--downgrade]`. `scenic status` shows the pool too. No control offers a handover by itself: the
  owner's give is the only way the lead moves short of §6.4's four triggers.
- **Forcing stays on the Mac:** `/work/lead` refuses forced takeovers.
- **Cancelling a job** (`crate::control::Cancel`): `scenic job cancel <target> [--step <step>]
  [--rerun-after-update]`, or the menu bar's "Cancel <job>" on the Mac running it (Option: "…,
  Rerun on a Newer App"), writes an ask to that Mac's agent (a file in `cancel-requests/`; one older than two minutes is dropped). The agent
  stops a slot of its own running the target, and tells the lead's coordinator (in process, or
  `/work/cancel` from a member), which marks every live lease of the target; a member's job hears
  it in its beat's answer (within a minute) and stops (a member on an app from before the cancel ignores it, and its job runs on). A cancelled job ends as given back, not
  failed: no retry delay, no backoff or guard hold against its Mac, its finished targets handed
  off, and its scratch entries named for the target (exactly, or with an extension) removed, so it starts again from the start.
  With `--rerun-after-update` the coordinator holds the target (`coord::lease::Hold`, saved with
  the leases and carried in the term's state) from every offer and the lead's own starts until it
  runs an app other than, and not older than, the one it was cancelled on; members get work only
  on the lead's app, so the Mac that takes it runs the newer. A cancelled job's slot is free, so
  an update waiting on it (docs/building.md §2) restarts at once. The cancel is a `cancel` event in the
  history; `scenic status` shows it among the recent jobs ("Cancelled:") and each held target
  under Waiting.
- **Pausing one Mac** (`crate::control::Hold`, task #224): `scenic pause --here` / `scenic resume
  --here`, `scenic pause <member>` / `scenic resume <member>` (host name or member id) from another
  Mac, or the menu bar's "Pause This Mac" / "Resume This Mac". The ask goes to this Mac's agent (a
  file in its folder, `hold-request.json`; `scenic pause` prints the agent's answer). A Mac's hold
  is kept in its agent's folder (`hold.json`, so a restart keeps it) and on the NAS
  (`state/pool/holds/<member>.json`), where another Mac's ask writes it; each member reads its own
  every 30 s on the switches' readers, the later setting winning: a Mac's own ask is always later
  than its setting so far, and another Mac's later than the file it replaces, so the Macs' clocks
  don't matter, except that a remote ask may lose to a Mac's own ask not yet written there (within
  a loop). A member whose app is older than the asking Mac's is refused ("too old to pause alone"). A paused Mac's running jobs stop
  at their next safe point and are given back (as the build's pause stops them, frozen past the
  grace), it asks for no work and starts none, it can't take the lead anew (`Conds::held`, `able`
  false), and a lead paused keeps the lead, granting the others' work, and runs none of its own.
  It keeps beating. `scenic status`, the build page's machine card and the menu bar show it
  paused (`Status::held`; the pool's view says "paused"); the forecast gives it no lanes. The
  rest of the pool goes on. `--now` stays the whole build's.
- Every control asks the driver (`Driver::hand_to`, `Driver::takeover`) rather than re-checking.

## 12. Getting there from today

Built and always on: terms, records per term, the journal, the driver, handover and takeover, the
controls, the steps table, the memory and disk guards, write-sets, the budgets and answers' lease,
every step to every member, every job by grant (the lead's too) and packing, the equal-machines
forecast. Gone: the pool's on/off switch and shadow
run, claims, the named writer (`SCENIC_BUILD_MAC`), the NAS's hand-off folders, per-Mac record
writes.

Still behind switches under `state/pool/` (a file; off while missing unless noted):
`slots` (§7.6), `resume` (§7.2, §7.4), `brokers` (§8), `placement` (§7.2),
`memory-guard` (on while missing). A new switch is made only once every
member's heartbeat shows the app that has it. The old files `state/pool/enabled` and
`state/pool/shadow` stay on the NAS while an older app may read them.

Planned: readers reading the term's snapshot and the lead no longer writing today's three files;
journal GC; the HTTP pool API; resuming beyond the OSM pass. Packing a terrain z6 tile's fetched raw
tiles (`pack_raw_with`) stays inside that z6 tile's job.

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
- **The placement simulator** (`sim::place`, in the crate's tests, a few seconds a run): N
  Macs, each with its memory, cores and disk, work a plan through the lead's real coordinator on a
  simulated clock, every job by grant. The plan is offered in its order, the steps kept to the lead
  (`bld-fetch`, `catalog`) as its own planned jobs. Members ask over HTTP with their capacity; the
  lead asks its coordinator in process (`ask_own`) as its agent does (`lead_grants`: its slots by
  its cores, alone for one job with its disk not bounded, else for its slots free with its disk
  room); each takes what `pack` grants. Each job takes its step's true memory and cores, and its
  disk written evenly over its run, the rest its reservation (an ask says its free space now less
  its room target and its reservations); its measures go back into the coordinator (crate::needs reads
  them). Checked at every start, first where a job may go (no Mac past its slots; lead-only steps
  on the lead, granted as its own jobs; one cold start of a step in the pool at a time, and one on a
  Mac), then what it takes (past a Mac's memory limit only a cold start, stopped by the guard;
  every job at its true NAS weight; never past its disk, every job on it counted whole, so no Mac's disk is over-committed,
  or past its cores with the threads granted); and the plan finishes. A run with an M1 8 GB past
  its room target and 5 GB jobs finishes reserving, and over-commits without (asking with its free
  space now alone).
  Runs: the M4 and the M1 (the lead packing two or more jobs and running its own steps, the member
  one) and four Macs; members' slots a setting: the M1 with two reaches two, a 48 GB member with 16
  cores holds three or four small jobs at once (its cores the bound); or the agent's own
  (`job_slots` by cores, `want`), the M1 and a 48 GB member each holding several at once. With the
  NAS budget on (§7.7; each step's NAS bytes and creates a setting, measured into the coordinator
  with its costs), NAS-heavy jobs past the NAS are queued: every start is checked within the
  budget, the pool's most at once stays under it where it passed twice the NAS off, and the plan
  finishes. Every job runs with the threads granted it (its costs recording them, and the CPU time
  its step's threads keep busy): every start checks them, with those of the jobs beside it, within
  its Mac's cores (less a member's in-use share's, with the owners at their Macs), a serial step
  granted 1–2 and packed beside its kind, and the plan finishes. A halo step's job (`pack`; `lo`, a pass's z3 tiles its halo) takes
  one halo's targets (offered a halo an offer, checked at every start), its disk the halo's output
  once; a job of a step reading a shared cache copies what its Mac lacks of it (a part of the
  fill at most, a setting), kept on its disk after, and no job is granted a cache's reservation
  beside another reading it there (§7.1, Disk reservations): a member copying the DEM cache a
  part at a time is reserved what it truly lacks at each first job, and again in full after its
  cache is emptied, the pass the same; with z6 tiles in the plan, a halo's map tiles are offered once its z6 tile is
  built, before the z6 tiles left: on the M4 and the M1, the NAS budget on and both steps measured
  before, each Mac runs map tiles beside z6 tiles, and the plan finishes. The M4 as measured on 18
  Oct (48 GB, 14 cores, 85 GB free, its caches on its disk so none of them reserved; z6 tiles of 6.6
  GB writing 6 GB, `pack` and `lo` 0.3 and 0.5 GB) runs four z6 tiles at once within its memory and
  disk, map tiles and low zooms beside each other. With its DEM cache gone and its fill measured,
  the first z6 tile is granted the fill, none beside it is, no start over-commits the disk, and z6 tiles
  still run three or more at once. Not
  modelled: other step dependencies, failures; a job's time doesn't depend on its threads.
- **The agent's tests** (`crate::agent::pool`, `crate::agent::lead`, the agent's `pool_tests`): two
  agents in one process, a lead and a member, through handover, takeover, controls, hand-offs and
  every switch on and off.
- **Two Macs on the real NAS** (`tools/check/pool-two-macs.sh`): the real agents on a scratch
  folder of the NAS, jobs played by a script; each switched batch is run so before its switch is
  made on the real build. Planned: the unchecked items of §3 (contended create-new, stale-read
  duration) and load.

## 14. Decisions

- **Automatic takeover** of a lead that stood down, or that went silent for `SILENT_S` (#435, 19
  Oct: a lead restarted to its FileVault login screen left the pool without a lead for 85 min);
  the lead otherwise moves only by the owner's hand, its battery low, or the NAS lost (§6.4, #497).
- **`tailscale serve` on every Mac** (`tailscale serve --bg 8090`, port 443) gives pages HTTPS;
  the agent finds it via `tailscale serve status --json` (on any port: the address keeps it) and
  puts it in its heartbeat. Without it, plain HTTP on the tailnet reaches every member, and an
  HTTPS page reaches only members with HTTPS. The owner also allows `scenic` through each Mac's
  firewall (§6.8).
- **The M6's on 8444** (`tailscale serve --bg --https=8444 8090`, #550): its port 443 is Caddy's,
  for https://map.brandontsang.net (docs/macs.md, Devices), and `tailscale serve` on 443 would take
  every connection to the tailnet address there, whatever name it asks (it can't share the port by
  name). Caddy serves the build page under `/build` too (`/build/work/`), and the page asks its own
  member under that path; its broker address stays the 8444 one (a broker's address is a root:
  the pages call `/work/…` on it).
- **Work order:** every slot from the front of the plan, in contiguous runs.
