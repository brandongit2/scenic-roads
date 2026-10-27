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
| Records, built files, catalogues | NAS: `state/build/`, content-named files | caches (`agent/cache/…`) | reads them |
| Downloads | NAS: `sources/` | cache copies | nothing |
| A pass's Wikidata/Wikipedia answers | NAS: `sources/items/<date>/` (`pipeline::answers`) | `agent/cache/items/`, `agent/cache/heritage-<date>/` | its first run takes them |
| Z6 tiles' kept state | NAS: `cache/scenic-units/` | none | nothing |
| A job's progress (the OSM pass's stages, a depth, terrain-hi or terrain-deep target's blocks, scratch) | the Mac running it | scratch folders | the OSM pass's resumed there (§7.2, Resuming), a depth, terrain-hi or terrain-deep target's when it runs there again; the rest redone |
| The coordinator's state, token, history | NAS: `state/coord/` | `agent/coord/` | loads it (§6.2, §7.5) |
| The lead's own (its catalogues' record, daily jobs' last runs) | the lead's disk | `agent/publishing.json`, `agent/state.json` | starts afresh |

## 3. What the NAS gives us

The share is SMB 3 on a Synology. The design assumes:

- **Create-new is atomic**, between two Macs too: 350 trials of both Macs creating one name at
  once (O_CREAT|O_EXCL, within ±30 ms) each had exactly one winner, its bytes the file's (task #645,
  `tools/check/nas-probe.py`). Terms and leases rely on it.
- **A rename can be delayed indefinitely** (a Mac sleeping between write and rename).
- **Reads can be stale** (macOS's SMB caches). Measured between two Macs on an idle share (task
  #645, 570 trials): a file renamed over was read new within 1.1 s (median 0.02 s), a new file
  opened within 0.12 s; a stat or a folder listing still showed a removed file for up to 33.5 s
  (median 2.4 s; about one in five near 30 s) and a listing missed a new one for up to 3.1 s.
  `driver::STALE_S` takes 10 minutes, for a share under load; one name stale for 40 minutes
  (task #630) is past any such bound.
- **Renaming over a file another SMB session has open fails** (EBUSY) for as long as it's held:
  `store::whole::rename_over` retries for 5 s. The server can hold a file for hours with no process
  on either Mac having it open;
  renaming the held file away, and renaming into its free name, still work, but removing it fails
  until it's let go (a Mac killed with a file open: kill -9, Migration Assistant). So every
  replaced file (`store::whole::write`/`copy`, `Nas::write_whole`: heartbeats, `state/status.json`,
  the manifest, pending, sizes, records) falls back to `store::whole::rename_or_aside`: the old file renamed to
  `<name>.<host>.<pid>.<n>.aside.tmp`, the new one renamed into place, then asides removed: each
  fallback removes up to 4 (the oldest first), finds any Mac's or process's asides of that file by
  listing its folder at most once a minute, and remembers at most 16 not yet removable. Listings
  skip asides as temporary. When the second rename fails the aside stays (renaming it back could
  replace a newer file), so the file is missing until the next write. Every read of the NAS
  (`store::whole::read`/`retried`: `Nas::read`/`read_fresh`) tries a busy read or open again for
  5 s, and never lists. The files that must never read as missing, all replaced by
  `whole::write` and never removed on purpose (`state/build/manifest.json`, `pending.json`,
  `sizes.json`, `summaries.json`, the extents, `state/status.json`), are read with
  `whole::read_replaced` (`out::read_record`): missing with an aside beside it, it is tried again
  for 2 s, then the newest aside (by modification time) is read, the last version written whole,
  so a manifest is never taken as empty. An aside put aside (its status change time) more than a
  day ago is never read, so a stale aside can't bring back a file. Sweeps: each fallback removes
  asides as above; every `whole::write` also removes its folder's day-old asides and the day-old temporary files
  writes left there (`<name>.<host>.<pid>.tmp`: a process ended, or a call given up on, before its
  rename), a listing at most hourly a folder a process, so a rarely written file's go too (the
  members folder's, its members'); and a kept scenic-units
  folder that can't be removed (a held file) is renamed aside (`scache::drop_dir`), the unit's
  earlier folder asides removed at its next save. The fallbacks log
  `whole: rename over <path> busy: …`, `whole: <path> missing, read from its aside …` and
  `scache: <dir> not removed …`. Calls still busy past the retries (a read busy for 5 s, a rename aside or into
  place busy) are noted with their file, the agent's own and its Rust jobs' (`whole::stuck`,
  `whole::STUCK_ENV`); three within 10 minutes drain this Mac's new work and then remount the
  share (`crate::agent::heal`, docs/macs.md). A build job's read that the share fails for a
  moment (`store::retry`, task #656; since #663 every read, stat and listing through
  store::nasfs, docs/nas-access.md §2, but the agent's loop's, the pool's and the coordinator's,
  which fail at once, and the server's, 5 s: EIO, ENXIO "Device not configured", ETIMEDOUT and the other
  disconnects, EAGAIN, ESTALE; a "not found" only on a file known to be there: one open, one the
  store's listing has, a source the release lists) is tried again after 1, 2, 4, 8, 16 s, then
  30 s six times (211 s in all), each try logged; a process's retries run one at a time. Before
  the last try the share's mount is looked at, and a share gone or not answering is waited for a
  minute (the agent remounts it; a job never does). Past that the job fails with the original
  error. Covered: plain-file reads (`store::range::PlainFile`, reopened by path each try: the
  GLO-30 store's TIFF blocks, depth grids, building work files), GLO-30 store tiles' lookup and
  check (`dem::generic::stored`: a tile the store's listing lacks fails at once, one cut short
  too), terrain and slope packs' opens and spans (`terrain_pack::nas_call`), source grids' opens
  (`depth::grids::open_retrying`), slope mids, and bldprep's parquet footers and row groups.
  A writer SIGKILLed mid-write leaves only its
  temporary file (test `whole_kill`, on local disk, not SMB). No NAS file is flock'd or opened with O_EXLOCK/O_SHLOCK: every
  lock (`sys::PathLock`: the build lock, the member and agent locks; store::cachefile's holds) is
  on a local file; terms and leases use create-new (O_EXCL), an atomic create whose handle is
  closed once written.
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
  runs the coordinator, merges the journal into its term's records, publishes catalogues, runs GC,
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
  manifest, keys, pending uploads (an entry's `checked` takes them out: GC's job, which checks
  them on the NAS, docs/storage.md), the sizes of the files the manifest names (from the entries'
  hand-offs, for the catalogues: `sizes.json` beside `manifest.json`), raw archives awaiting the
  index (each taken off once the lead has named it in the raw store's index and saved that,
  `Heard::named_raw`, with the next snapshot; one named but not yet off when the lead restarts is
  named again, passed over unread; an archive that index lists as merged away is never named
  again, task #607), the journal entries it reflects
  and refused (with why), the lease that last set each target, the forget horizon, and `seq`. A
  handover's last snapshot also holds the coordinator's state. Written whole (`Nas::write_whole`)
  after each merge.
- **Term 1** started from today's layout (`manifest.json`, `jobs.json`, `pending.json`): its first
  snapshot is `term/1/first.json` (`records::first`, `term::bootstrap`).
- **Taking up term E+1** (`records::start`): from the newest snapshot found walking down the terms
  (a handover's only from its `seq` or later; a stale read fails and is retried; never from records
  missing an entry the journal's GC removes or removed, §7.3). The new lead then
  replays every journal entry its records don't name, in (term, lease) order. It is **caught up**
  (`caught_up`) once a listing of every day, asked for under a day ago, is merged; catalogues and GC
  wait for that. It relists daily.
- **Acknowledgements are per term** (`journal::Mine`): each loop a member tells the current lead of
  its entries that lead hasn't acknowledged. A lead acknowledges an entry once a saved snapshot
  names it.
- **Readers** read today's three files, which the lead writes after each save
  (`agent::pool::write_today`). Planned: reading the current term's snapshot (`Records::newest`,
  falling back to an earlier term's).
- **The coordinator's state** is per term too: `state/coord/term/<E>/state.json`
  (`crate::coord::PoolState`: leases, costs, floors, disks, failures, pause), written by the lead's
  coordinator's own thread as it changes (§7.5). A take-up loads the handed-over state, else the
  newest earlier term's (`took_up`, §7.5).

### 6.3 Asking

The menu bar ("Hand the Build To ▸", "Make This Mac Lead"), and `scenic lead`
ask their own Mac's agent (`lead-request.json`, `crate::control::LeadRequest`); the worker page asks the agent serving it (`/work/lead`). `crate::agent::lead`
checks the ask as the driver would (`Driver::hand_to`, `Driver::takeover`) and either hands over (on
the lead) or mails it to the lead (`Msg::HandTo`, §9). Asks naming a Mac that isn't a live member,
is on too old an app, is unreachable (§6.8), or whose conditions say it can't lead (away, on
battery under its own battery floor, short of disk: `Conds::able`) are refused with why. An ask's state is kept in
`agent/pool/lead.json` and shown by every control.

### 6.4 Handing over (the lead is there): a state machine

Pure transitions in `crate::pool::handover`, driven by `crate::pool::driver`; state lives in the
two heartbeats and the terms.

| State | What happens | Next |
| --- | --- | --- |
| **Leading** (A, term E) | | ask to hand to B: **Offered** |
| **Offered** | A's heartbeat: `handing_to: B` and the offer | B answers within `OFFER_S` (2 min): **Ready**; else **Leading** |
| **Ready** | B checks disk and app, heartbeat `ready_for: E+1` naming the offer | **Settling** |
| **Settling** | A stops granting ("the lead is moving"), keeps renewing leases, stops a catalogue or GC in flight, saves records with the coordinator's state | within `SETTLE_S` (1 min), B still ready: **Passed**; else **Leading** |
| **Passed** | A makes `terms/<E+1>.json` naming B with `seq`; A is now a member | B takes up within 2 min: **Leading** (B, E+1); else **Taken back** |
| **Taken back** | A makes `terms/<E+2>.json` naming itself | **Leading** (A, E+2) |

Leases keep their ids (`<term>-<n>`) and wall-clock deadlines across the handover, so every job,
the old lead's included, renews with B (§7.6). A lead with journal entries left to read gives up a
handover (to be asked again).

**Who leads is the owner's call.** The app isn't smart about who leads: no capacity score, no
offer of its own. The owner hands the lead over by hand (§6.3) when about to leave. The lead moves
by itself only on these four triggers, each to a member that can lead (home, on power or charged
past the battery floor, not paused, room past its disk target, an app the rule allows, reachable):

1. **Its battery below its battery floor** (`cond::FloorCache`: the owner's, each Mac its own,
   `scenic battery-floor`, docs/workers.md §7; 30 % while none is set; the floor its heartbeat says it
   went by, `Conds::battery_min`): the lead, on battery under it by its own
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
  saying its term (`Coordinator::set_member(None)`, until a step runs again) and the status says so,
  with the manual fix if it lasts: disconnect that Mac's SMB session in DSM (docs/macs.md). Nothing
  remounts a hung share by itself (the self-heal's remount waits for idle slots, `crate::agent::heal`).
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
20 jobs took over their runs (`job-disks-v6.json`), the larger; 0 for a cold start; a halo
step's, `needs::HALO_STEPS`: `pack` and `lo`, the most of its targets' rather than their sum. A
`pack` job takes one z6 tile's map tiles (`build::by_unit`'s groups, each its own offer); `lo` stays
one work of a pass's z3 tiles (the low zooms' gate opens for a pass as a whole). A `pack` or `lo`
job's disk is its own: the parts of its halo's base packs it reads from the NAS, into its scratch,
and its output (task #396; docs/steps.md §8); their runs and targets' disks from when their
jobs copied whole base packs into a cache of the Mac's are dropped once, as `job-disks-v5.json`
is first written, and their targets' disks again as `job-disks-v6.json` is (task #657: each take-up
of a term's state had brought back those of 51–68 GB, which no Mac's room fitted; a term's state
says its disks' generation, `disks_v`, and one of before it brings neither step's to a take-up;
docs/formats.md). `depth-lo`'s (task #398, `coord::REMEASURED`): its job
peaks and targets' measures past three times its targets' median and a GB (a 35 GB peak against
65 MB targets, which kept it on the lead's Mac) are dropped each time the coordinator loads them,
so it runs on any Mac by its measured use. **Shared caches** (task #392, `needs::CACHES`): `depth`, `depth-lo`, `terrain-water`, `lakefill` and `water` read the whole copies (task
#551, `localread`: whole local copies of the basemap and the depth sources' packs in the cache's
`blobs/`, made where the disk has room for one past the room target, else the NAS read in place; one
process copies a file at a time, the others waiting for its copy (a lock file in the cache's
`copying/`); their reads are the local disk's, `costin`'s `local`, not in a run's NAS weight; a
copy room-making frees is copied again by the next job reading it). Reserved only where it would be
copied (`needs::CACHES`, `coord`'s `missing`): on a Mac whose room past its target (its ask's
`disk_mb`) is short of what it lacks of the fill, nothing is reserved and its jobs there read the
NAS, so a laptop keeps getting them. The run that fills it measures it whole, the warm runs after it
their output alone. So a target of such a step counts its own output (its disk floor at most its
step's 90th percentile, `needs::Book::disk`), and the cache is reserved apart. What's reserved
and when:
- *The fill* (`coord::Shared::fills`, `cache-fills.json`): a job that did all its targets (not
  a paused or stopped one) and whose disk passed its targets' output by 512 MB or more, and by
  at least half the fill kept, filled the cache; that excess (its run less its own output) is
  the fill. Warm runs, and a part copied
  (a part refilled after an eviction), leave it.
- *What a Mac holds* (`Capacity::caches`, `have_mb`): read off its disk as its agent asks
  (`caches_here`: the bytes of its cache folder's `blobs/layers/basemap*` and
  `blobs/sources/fetched*`, the whole copies), so
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
took (`cpu_ms`) and the time its process was stopped meanwhile (`paused_ms`: frozen by the memory
or disk guard, or paused for its conditions; `costin::stopped_ms`, a thread that counts its own
wakes coming late, task #697). A job's measured cores (`needs::Book::cores`, its own run's, else the most of its
step's) are two: its **grant**, the threads its runs were given, or fewer where their CPU time over
their wall time says fewer were busy, plus one (a serial step: 1–2), or twice them where three
quarters or more were busy (so a grant can climb back; capped at the Mac's cores); and its
**busy** cores, that CPU time over the time it ran (its wall time less the time stopped) to the
nearest tenth of a core (a tenth at least, never past its grant; the packing sums a Mac's jobs'
tenths, so rounding no longer undercounts by up to half a core a job). A target not run the way
it runs now, nearly every one granted, counts for its step's most, which no run crowded off the NAS
lowers. The cores a Mac's jobs may use together (`coord::ask_cores`) are its Mac's
less, while its owner uses it, its in-use share's. Each job is granted its
measured grant within the packed share (`coord::packed_share`: a quarter of those cores, at least
two where they allow: the M1's 8, 2), else the share; a step
reading the NAS as it goes (`coord::NAS_STEPS`: `unit`, elev, pack, lo; or one whose costs show
10 MB/s or more of NAS bytes) the share, never fewer, its threads overlapping the waits. **A job
counts for its busy cores** (task #688, `Sched::need_cores`): the packing takes those, not its
threads, off its Mac's cores, so jobs waiting on their reads go beside each other until memory,
disk or the NAS budget is full rather than their threads: terrain-hi kept 1.16 cores busy of 2–3
threads on 26 Oct and counts for 1.2, where it counted for 3 and the M4 packed 3–4 of them ("cores:
2 of 14 left, a job of terrain-hi takes 3") with 22 GB of its 39 GB build memory free. The rule
reads the runs, not the step: a terrain job reading faster (from this Mac's disk, say) keeps more
busy and counts for more. A job's threads stay within the cores left plus those it leaves idle
(its grant less its busy cores). **Past the cores its jobs' threads leave, a Mac climbs one job
at a time** (task #688 review, `coord::SETTLE`): an ask is granted at most one job that fits only
by busy cores, and none until a minute has passed since its last grant (the span of the load
average its capacity reports) or while that load passes its jobs' busy cores by more than a core.
So the M4 takes four terrain-hi jobs by their threads at once, then one a minute up to what its
memory, disk and the NAS budget hold, and stops climbing as soon as jobs keep more busy than their
runs said (a run's busy cores read low where it waited on a busy NAS). Load past the busy cores beyond the packed share comes off
the cores left as outside load does (`outside_cores`, at most half the Mac's), which stops further
grants but stops no running job. A first
job alone gets no more than the share, so its run is never measured to take every core and leave
no room beside it. The agent, the lead's as a member's,
starts a granted job with those threads, within its Mac's cores less those the in-use share keeps
while the owner uses it (one at least; not less those its other jobs were given: its lead packed
them by their busy cores), and logs a
line per job: `agent: starting <job> (<n> threads: granted <g> of <c> cores, <k> kept for the
owner)`. The coordinator logs the threads with each grant.

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
handovers and a NAS slower than its probe are not foreseen.

**What holds work back, from the history** (task #563, `forecast::held_back`/`hold_back`): the
run releases work by the plan's dependencies alone, and alone it finished a median 65 % early, for
work reached the Macs slower than that (the lead's ramp and slot cap, the NAS budget, the plan
feeding work gradually: the lead idle half the time on 18 Oct). So before each run every Mac is
held to what the history shows of it over the last day: its lanes to the jobs it ran at once
while it ran any (the mean, rounded; a lane running a job kept), each at the share of those
lanes' time it kept busy while the pool ran planned work (0.25–1, so a Mac idle most of the day
isn't slowed more than fourfold). Every idle minute counts: a Mac idle for a day is the same Mac
with room, and most idle time while the pool worked was work held back, so the floor is low.
Backtest finish medians: 35 % at 0.25, 34 % at 0.1 (the live error said 52 % against 38 %), 42 %
at 0.4. Leaving out the minutes more than 2 h from a job of the Mac: finish unchanged, the
Mac-mini-led days' 6 h progress error 32 % against 8 % (median). Placement's slack (`placement::slack`, the steps' ends
against the finish) comes from this held-back forecast. The forecast says these
(`Forecast::held`) and the **live error** (`forecast::pace`, `Forecast::pace`): the oldest
forecast of the last 1–6 h (the lead keeps their runs' progress, `Forecast::curve`, a point each
10 min) against the planned targets the history shows done since, the build page's Details tab
(its Forecast), marked rough ("live error (rough, ±50 %): the forecast of 14:05 did the 40 targets done
since in 1 h, not 3 h"): over the backtest it was off by about half. It is said, not applied: as a stretch it made the finish worse (the backtest: 74 % from 65 %).

The backtest (`backtest_against_the_history`, run by hand on a copy of the NAS's `state/coord`
and the leads' `agent.log`; without them it prints that it skipped and checks nothing) forecasts
from each term's recorded state since 18 Oct, whichever Mac led (`lead_at`: the Mac whose own
jobs come next; a Mac renamed, one), the jobs the history shows started until the plan's work
first ran out (30 min with none of it running, within a day), each released once the run has
done what it waits on (the plan's dependencies, those the history shows started before it), the
lead's clocked jobs when they started, the lead's own held to the probes' limits the logs show.
Over the 58 starts that ran out (18–21 Oct, M4- and M1-led), the finish's error, median / 80th
percentile / mean: **"as live", held back, 35 / 64 / −26 %** (not quite live: the run still has
the history's job set and order, and a job waiting on one under way at the start gets that
one's real end; the live forecast's own error is likely higher); nothing held back 65 / 79 / −51 %;
with the history's release times instead (each job ready when the history shows what it waits
on done) 17 / 48 / −14 %; asserted: as live at most 40 % and at most two thirds of nothing held
back. The live error said then was off by 38 % (median). The Mac-mini-led days (from 21 Oct
23:00) have no finish yet (its work never ran out); their progress over the next 6 h (the time
the run took to do what the history did, against 6 h): median 8 % over 11 starts (all leads: 30 %
over 68). The forecast packs each Mac into its cores less those its last ask showed busy
outside the pool, as the coordinator counts them (`coord::outside_cores`, task #684), for the
whole run: load that comes or goes later isn't foreseen. The
lead logs a departure only when a Mac's jobs of a step differ from the forecast's by 2 or more and
by more than half, each Mac and step at most once an hour.

Rows of note: `trees`/`terrain-hi`/`slope-hi` per z6 tile with `*-lo` assemblies per z3 tile,
`terrain-deep`/`slope-deep` per z10 tile (docs/steps.md);
`bldprep`/`bldtiles` per z6 tile, `bld-fetch` the lead's (docs/buildings3d.md §3.6); `inputs`, a
target per z6 tile, the lead's, planned before every other job (docs/inputs.md §4.3).

- **Every step to every member** (`crate::agent::every`, `crate::coord`'s `pick_every`): all but the
  lead's own steps (`every::LEAD_ONLY`: catalogues, prunes, `bld-fetch`, the answer-keeping steps, the
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
  load lags a minute and is rounded up: a job just ended), and at most half the cores it has
  for jobs (`coord::ask_cores`: less the in-use share; `coord::outside_cores`; task #440: a Mac swamped by other work, builds and tests, gets no job
  beside those it holds past the other half, and its refusal says the cores busy outside the
  pool; task #684: the load average can't tell a niced test build (`tools/app/remote-tests.sh`
  runs at nice 10 on the Mac mini, the lead) from work that competes, and on 26 Oct one held the
  lead's load at 47 of 12 cores, its free cores none, for two hours; the owner's own use keeps
  the in-use share besides), less the measured needs of every job it holds and of those granted
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
  tried on steps by their forecast slack (least first); within a step, the targets in the asker's
  last one's z3 tile first (`placement::group`, from just after it in plan order: its jobs read
  the same neighbourhood's inputs, task #552), then from the longest. Each target is drawn among
  the members asking lately that would take it now (`Sched::would_take_now`: by their last ask,
  a slot free and their free memory fitting it; a busy Mac is never drawn a target it would leave
  untaken), the lead's own agent among them (`placement::draw`, stable per minute window); an
  asker passed by goes on down the offer to the next target drawn to it. A worker's weight
  (`Sched::draw_weight`) is its Mac's measured speed at the step against the lead's
  (`placement::step_speeds`: the history's median time a target, three runs or more each, its
  slots' runs its Mac's, held to 0.33–3 (`STEP_SPEED`); worked out again only as the history
  grows), else its speed over the shared steps, else the lead's; times `CACHE_PREFER` (4) where its
  ask's capacity says it holds the step's shared cache (`needs::CACHES`: the whole copies) whole. Trade-offs: weights only, so a Mac asking alone takes what it can and the
  weights act only where Macs ask together (no Mac idles for a draw); medians of different
  targets' times read a Mac given a step's large targets as slower, which the clamp bounds; a
  minute's window keeps a target drawn to one Mac until the next window. The forecast runs the
  same draws (the placement is in the scheduler's state it's given), its lanes asking as its
  model has them. A task offered
  ahead goes to a slower worker only while its head start covers that worker's run. The lead's own
  planned jobs keep plan order.
- **Resuming** (`crate::agent::resume`, behind `state/pool/resume`): a job interrupted with progress
  on a Mac's disk (built for the OSM pass) is offered to that Mac alone for 24 hours from its first
  start, or until `scenic pool release <member> "<job>"`. Progress is in
  `state/pool/progress/<member>.json`, written only by that member. A finished one is held unoffered
  until merged. A long target resumes at the block after the last it finished when it runs on
  that Mac again, not offered back (crate::resume_log): a depth target (`depth`, `depth-lo`: its
  composing's blocks, crate::depth::resume), a terrain-hi z6 tile (each level's blocks, a z8
  tile's tiles at z12, then the level's end) and a terrain-deep z10 tile (its base tiles, each
  with its z13–16 subtree; crate::terrain_resume). slope-hi and slope-deep keep none: their
  targets take minutes at most. A drain also stops the terrain's steps (terrain-hi, -deep, -lo, -root) mid-target: their
  sampling of the elevation pool checks `SCENIC_CONTROL` (`control::draining_lately`, read at most once a second) before each chunk (16 tiles) or tile read alone and ends
  with `control::Paused` (a level's sampling ran 7.7 h on 3/7/3, task #705); the step catches it
  once its uploads in flight have landed and exits as paused, the target unsaved and not noted
  done, its log kept (terrain-hi, terrain-deep; a level's sampling not yet in a block is redone). A log at the top of its step's scratch folder
  (`<z>_<x>_<y>.resume`, one append a block, each record with its length and BLAKE3, read a
  record at a time; a slot's moved to the slot running it), taken only for the same inputs (its
  step's versions and encodings, the target, the sources' and water's pins, and what else it
  reads: the depth lo packs, the terrain's tiles made here, the fine datasets' lines), so its
  outputs are the same bytes. Counted in the job's disk as its sampler counts its scratch's top.
  Deleted when its packs are written, on a cancel, with a given-back job's scratch by
  room-making, and, untouched for 24 hours, as the step's next run on that Mac starts.
- **The forecast runs off the loop** (task #544): the lead makes its input on the loop and runs it
  on a thread of its own (`Agent::start_forecast`); the status keeps the last one done, so reaping,
  starting jobs and writing the status never wait on it. Its three runs go side by side (three
  threads, each at the utility QoS: `forecast::low_priority`), each cut once it has computed
  `forecast::BUDGET` (15 s of its thread's CPU time, not wall time, #584: a loaded Mac starves
  utility threads, 4 s of CPU took 15 s on the mini at load 34; a live run computes 1-4 s), or
  after 120 s of wall time (a backstop, so a starved run can't hold the next forecast or the NAS
  self-heal); the trade-off: the three runs may take up to 3 × 15 s of CPU together: one
  cut is `partial`, its `why` says so, no finish shown, and the build page marks its lanes and next
  jobs partial. On 21 Oct's live input (4,155 targets, 20 lanes) it took 3.7 s (205 s before #544).

**A job waiting on a worker** (`crate::offload`'s `WAITING_ENV`): a z6 tile job waiting on a faster
worker for its tail writes `SCENIC_WAITING`; while it exists, on the lead and on a member alike,
its agent names the job's lease in its capacity with what it holds now (`Capacity::waiting`,
task #462), and the lead packs that lease at that memory (never above its measure) and none of
its cores (`Sched::pack`), within the Mac's room and the memory guard's limit as ever; its disk
stays held. Only the asking Mac's own leases count so. A job started beside a waiting one
**yields** to it (stopped at once, given back, not counted as a failure, no floor learned) when it
resumes and the jobs no longer fit together (its Mac's memory guard). A restarted agent no longer
holds the job's slot, so it names nothing: the lease counts by its measures until it lapses.

**A job leaving the cores idle** (task #665): a running job that has used under a core-minute
of CPU for `agent::IDLE_CPU_S` (10 minutes; its sampler's `busy_at`), waiting on the network,
not on a worker, is named in its Mac's capacity (`Capacity::idle`), and the lead packs its lease
at its measures (memory and disk as ever) but none of its cores, so CPU work goes beside it
within the Mac's slots (`job_slots`) and room. Only the asking Mac's own leases count so. A job
that uses its cores again is not stopped and nothing yields: it counts its cores again from its
Mac's next ask once it has used a core-minute, and the Mac is granted nothing more until they fit.

**The memory guard** (`crate::agent::memguard`; `state/pool/memory-guard`, on unless it says `off`):

- A thread samples each job's processes' footprint every 5 s, per target under way (from
  `started` lines in `SCENIC_COSTS`, else `SCENIC_DONE`), and writes `memory.json` for the status.
- Every child started through `store::spawn` (the agent's jobs, their programs) is started without
  macOS malloc's cache of freed large blocks (`MallocLargeCache=0`, an undocumented libmalloc
  setting, `store::spawn::NO_LARGE_CACHE`; their children inherit it; the agent and server, started
  by launchd, keep it): with it, freed blocks stay in a
  process's footprint, and a dense z6 tile's scenic-build held 6–7 GB of them while its programs ran.
- **Limit:** the Mac's memory less an eighth, 4 GB at least, less the agent's own footprint as measured each loop (`memguard::jobs_limit_mb`; the coordinator is told the same). Over it, the largest fitting alone,
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
  Restarts), as after any restart but a quit. A job taken over so runs the old release's programs to its end: its
  agent says so as it takes it over (its log line and the history's `adopt` event name the app),
  and names it in its capacity (`Capacity::old_app`: its job and app) for as long as it runs, so
  the lead's status says it as a notice, by Mac, job and app (task #688: terrain jobs of a release
  before #665's rolling fetch crawled over GSI's tiles for hours after the update). The new agent may start while the old one still
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
  [<step> [<target>]]]` lists or clears them: the clear is a request to the running lead's coordinator
  (`/work/floors`), which takes them out of its memory, its files and its next state at once (no
  restart), and the command says how many floors and disk floors it cleared. A floor learned before a
  step's fix stops counting once the step's `cost_version` is raised for the fix (`bld-fetch` 1, task #557).

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
- **A redo is a hand-off too:** `scenic redo` (§11) clears targets' keys through a job of the
  lead's whose entry carries only a done record, merged as any, from that term's lead alone.
- **Sizes ride along:** `Out` records each file it writes with its size, and the hand-off carries
  them (`Handoff::sizes`); the merge keeps those of the files the entry names, so a delta catalogue
  never looks at them on the NAS. Entries from before carry none.
- **Lease order holds:** an older lease's entry for targets a newer one set changes nothing.
- **The journal is a log:** nothing is removed on merge. Its days over a week old are removed by
  the lead's daily `gc` job (`crate::pool::journal_gc`, task #644), on by default; the owner's kill
  switch `state/pool/journal-gc` in the project folder saying `off` stops it (`journal_gc::SWITCH`;
  missing or anything else: on; unreadable: that run skipped; a marker already written stays and
  still fences take-ups: lifting one is `scenic pool journal-gc`, below). It runs off its loop, once its records
  are re-asserted and caught up, in two runs at least `WAIT_S` (12 h) apart:
  1. **Mark:** once the last GC's entries are forgotten, `state/journal/gc.json` is written whole,
     naming every entry of the days before `KEEP_DAYS` (7) ago, up to the first day holding one
     the current term's snapshot doesn't name (held, or not read whole yet); named by that
     snapshot as read back from the share (a save that failed, or whose answer was lost, counts
     for nothing), with the horizon they're before and the last GC's (`floor`). Nothing removed.
  2. **Remove:** only when every member's heartbeat says its app keeps the fence (`Beat::journal`)
     and the current term's snapshot, read back, still names or has forgotten every marked entry:
     the marked files, a day at a time, the term and the marker checked unchanged before each, at
     most `MAX_REMOVES` (3000) a run, the rest next run; a day emptied, its folder too. Then the
     marker says done, and the lead's next listing of every day forgets those entries
     (`Records::forget`), its horizon the marker's: told of one again, it answers it as merged.
  **Why it's safe.** A take-up starts from the newest snapshot its reads find, which a stale read
  can make an older one, and replays what the journal holds that its records don't name: an entry
  removed is replayed no more. A lead told of an entry of a forgotten day that a read finds missing
  answers it as merged only when the marker names it or it's before the marker's floor; else it
  waits (one written late and read stale would be lost). So `records::start` refuses (tried again shortly) records that
  don't name or forget every entry the marker names, or haven't its floor; the wait lets every
  Mac's reads see the marker before a file goes; `agent::gc::unmerged` refuses such records too,
  removing nothing. An entry written into a removed day later (its member asleep with it unwritten)
  isn't marked: it stays and is merged as any, and GC's file sweep lists every day for it. A lead
  on an app from before the fence (only by the owner's `--downgrade`, §6.1) takes up from records
  it reads unfenced; no entry is removed while any member's heartbeat is such an app's, but one
  installed after a removal and forced to lead could, on a stale read, start from records older
  than the removal. Today's three files are written from the records after each save: GC doesn't
  touch them. The refusals' notes (`rejected/`) are kept.
  **A marker that fences for good** (task #651): one that can't be parsed, or records that never
  come to cover it (older records restored, say), refuses every take-up and the pool has no lead.
  The lead's heartbeat says so (`Beat::fenced`: since when, why, from `journal_gc::Fenced`), and
  after `ALERT_S` (an hour: a stale read passes sooner) every Mac's view raises it as the lead
  alert (`agent::lead::alert`), on the build page above the Overview and on the menu bar; until
  then `scenic lead` says it on a line. `scenic pool journal-gc` checks the marker against the
  newest records (`journal_gc::plan`) and says what's safe; `--repair` writes what it proposes,
  only while the kill switch says `off` (no GC run then starts, and one under way stops before its
  next day), and only when a second read `journal_gc::REREAD_S` (60 s) later proposes the same; a
  dropped entry found removed after the write puts the marker read first back:
  - covered, or no marker: nothing to do (a refused take-up read stale);
  - records haven't the floor, or name not a marked entry already removed: **not safe**, nothing
    written: a take-up from them would lose those entries. Restore newer records
    (`state/build/term/`) from a backup; removing the marker by hand instead accepts that loss
    (the entries' targets look not done, and their work is redone or lost);
  - marked entries the records don't name, all still in the journal: dropped from the marker (a
    take-up replays them from the journal), the rest kept, so nothing it removed is unfenced;
  - can't be parsed: made again from the newest records as a mark would be, a superset of what the
    lost one can have named: every entry they name of the days before `KEEP_DAYS` ago (up to the
    first day holding one on the share they don't name), removed or not, its floor their horizon,
    not done. The lost marker named only entries its snapshot, read back from the share, named,
    so records at least as new name them all or have forgotten them; the command reads the newest
    there now. Its one risk is that read itself being stale (an SMB client's stale view, seen for
    up to 40 minutes, longer than the reread's minute): `--repair` it from a second Mac only if
    the first's proposal matches.
  Then the kill switch back to on (or removed).
- **The lead lists the journal off its loop:** every day after take-up and daily, the last two days
  every ten minutes. Otherwise it reads the entries members tell it of. An entry unreadable for an
  hour while the share answers is refused. Merging reads for at most a minute a loop.
- **An entry its writer has moved on from is refused in minutes** (`driver::MOVED_ON_S`, 10
  minutes; task #630). A member writes an entry whole (a temporary file renamed into place,
  `Nas::write_whole`) and tells the lead of it only once that write returned, so nothing of a told
  entry is in flight. Once the lead has also read whole an entry the same member first told it of
  at a later step of the lead's (the lead's own awake clock, never another Mac's), that member's
  writes reach the share and the lead's reads; a first entry still read short or not there 10
  minutes on is this Mac's stale view or the share's damage, and is refused (its job's targets done
  again: a refusal loses only work). Refused, it's in the records' `rejected`, which every later
  snapshot carries, so it is never merged if it reads whole later; the redone job's entry has a
  later lease. Entries first told in the same step, or found only by a listing, wait the hour; so
  does every entry after a lead change or a restart (the first tellings are kept only while the
  same lead carries on). This matters because the lead is caught up, and a catalogue goes out, only
  once every entry told or listed is read: on 24 Oct the Mac mini's own entry
  `2026-10-24/414-1792809170065`, whole on the share (318 bytes, renamed at 02:37Z, read whole
  from the M1), opened as "No such file" on the Mac mini itself for over 40 minutes (its SMB
  client's stale name), and the catalogue, the only work left, waited behind it. Listing the folder
  is no cure: the lead's own sweep listed that day's folder every 10 minutes (`SWEEP_S`) through
  those 40 minutes, in the same process; the agent's restart read it at once. Meanwhile the
  catalogue isn't asked of the lead's coordinator (it was granted, leased and logged about once a
  minute, only to wait).
- **A removed step's names leave the records the same way** (task #701): the lead's daily `gc`
  job removes from the manifest every name under `agent::gc::RETIRED` (a removed step's outputs'
  prefixes), its write-set these removals alone, so its one hand-off is merged as any entry, in
  one save of the records; none left, it hands off nothing. A lead on an app without them refuses
  the entry (outside `gc`'s write-set then), and the next run's hand-off tries again. The files
  then go by GC's own rule (docs/storage.md, GC).
- **So do the depth packs no target names** (task #679): in the same hand-off, the depth's packs
  (`layers/depth*/hi/6-…`, `…/lo/3-…`) whose tile is none of the depth's current targets as the
  lead's last plan found them, with those leased now (`agent::gc::DepthKeep`); `gc`'s write-set
  takes removals of depth packs. A depth job still running for such a tile merges after and puts
  its pack back (lease order sets no targets of gc's); the next run judges it again.
- Raw tile archives are only added by jobs; compacting is the lead's duty. `summaries.json` is a
  content-keyed cache any writer may update.

### 7.4 Planning reads only the records

Behind `state/pool/resume`, a pass is complete when its records name `sources/osm/<date>/pass`
(`crate::osmpass::latest_pass_by`); off, a `pass.*.json` file still says it. A catalogue runs only
when the lead is caught up. A resuming job is given its member's unacknowledged hand-offs of its
step (`SCENIC_UNMERGED`), which `Out::open` lays over the snapshot.

### 7.5 The coordinator across terms

- **Its state on the NAS** (`crate::coord::keep`, task #641) is written by a thread of the
  coordinator's own, never the agent's loop (whose step only names the term it leads): woken by
  every change the coordinator saves on its disk (a grant, a finish, a lapse, a given-back lease, a
  measure) and each minute for the rest (failures, the pause), written when its bytes changed. A
  change waits `BATCH` (2 s) for the rest of its burst, and a write starts at least `GAP` (10 s)
  after the one before, so a burst of grants is one ~3 MB file each 10 s at most (200 grants in 3 s:
  15 writes at the test's 0.2 s gap), through the agent's NAS deadlines (§7.8), a failed write tried
  again a gap on. A lead that loses its term stops writing it (its coordinator stopped, or kept on
  as a broker: `Keep::off`), and a writer that panics is started again at the next lead's step.
  Renewals aren't saved (a loaded lease gets a fresh deadline).
- **What it must hold, and when.** A handover needs none of it: the lead settling stops granting,
  then hands its state over whole (`Run::settled`), so every grant answered is in it. A lead gone
  without handing over (crashed, asleep, off the NAS, or restarted: its re-assertion loads the file
  over the leases on its disk) is taken up from the newest file, and a lease granted since that file is unknown
  there: its worker's next renewal is refused, its job stops and its targets go out again. That
  costs the work, never the records (the merge's lease order, §6.5), so a grant isn't held for the
  write; it's on the NAS within `BATCH` plus a write (a `GAP` in a burst), where it waited for the
  lead's next loop step before (20 s, minutes while the loop was busy).
- Workers are a member id and an agent instance; a new lead drops only instances that no longer
  exist.
- **What a job waited on** (task #791): the lead's coordinator writes on each "start" and "lease"
  event `after`, the numbers of the "end"/"done" events of the jobs it waited on: for each step
  its targets wait on in the forecast's dependencies (`forecast::Forecast::graph`, handed to the
  history after each forecast, `history::Waits`; what an earlier forecast's waited on and finished
  since is kept), the one of that step that finished last. A few numbers an event (one per step
  waited on, ~10 bytes each); events before it have none, and readers take none as nothing
  known. The build page's schedule draws its arrows from it.
- **The history** is one file per writer, `state/coord/history/<day>/<member>.jsonl`, for the last
  `history::DAYS` (7) days, the days the page view reads (`pageview` takes the same constant). A
  day before them is packed as one tar, `state/coord/history-archive/<day>.tar` (members
  `<day>/<member>.jsonl`), by the daily `gc` job, so the lead alone, outside its loop and the
  pool's lock (`agent::histarchive`, task #531). Per day: its files read whole and merged into
  what the day's tar holds already (a late write, an old day's event appended after the day was
  packed, is added to the tar, never written over it; a tar that can't be read is never
  replaced), the tar built in memory and written in one stream to a temporary name (one fsync),
  read back and checked, renamed into place; only then the folder's files and folder removed,
  and only if the folder still lists the same names and sizes (an append between that listing
  and the removal, milliseconds, is the one way a line can be lost). A stopped run's temporary
  file is removed once an hour old. A day that fails (a folder holding anything but files, over
  512 MB, a deadline, an unreadable tar) is left as it is and said as a warning in the job's
  note. Every call goes through `nasread`'s deadlines. `pubtimes::load_history` (`scenic
  status`, a lead's planned notes) reads a day's tar alongside its folder.
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
answers nothing and the job runs the task itself. A member taking up a term starts its
coordinator in process, and a lead losing it stops its coordinator and duties, keeping its jobs
running; only a coordinator that fails to start leaves the part to a restart once its first slot is
free. A lead drains the hand-offs its coordinator journaled on its disk (those a member out of the
pool told it, `journaled: false`) into the pool's journal (`Run::drain`); every member drains,
once a process, the jobs a process of its ran out of the pool and left in its outbox untold
(`Run::drain_outbox`). A lead whose update waits on its jobs keeps the lead and restarts into the new app at a safe
point like any member (docs/building.md §2, A newly installed app). The lead's own asks always carry its capacity, so the lead never
holds packing off for the pool (§7.1, Mixed versions); its heartbeat says whether the pool is all on
the new app (`all_new`) for its members' heartbeats.

**The lead's running jobs' leases never lapse** (task #666): its coordinator runs inside its agent,
and each loop the agent says which leases its slots run jobs under (`Coordinator::keep_own`,
`Leases::mine`, its own name's alone): those are live past `coord::TTL`, beats or not, so `expire`
passes over them and they can always be renewed and finished, and a loop stuck past the TTL (10 min
on a slow NAS) no longer kills its running jobs. A lease whose job ended without giving it back
leaves the set at the next loop and lapses as any; a stopped agent's are dropped as the next one
starts (`drop_where`, `drop_workers` taking a term up, sparing the jobs it adopts), the set not
saved; after a handover the old lead's leases are a member's, lapsing as any. A beat that still
finds its lease gone holds it again, a job of no record (buildings) by its step from its id and no
targets (`held_work`, as `adopt_jobs` and `lease_own` do). A live job of no targets holds its whole
step: no other lease of the step is held beside it (`Coordinator::hold`), and none beside a lease
of the step when the one asked has no targets; the planned jobs' grants already wait on any lease
of their step (`own_open`).

### 7.7 What a step needs, by structure

Rule: nothing depends on a hand-kept figure. A step's needs are **measured** per target, **enforced
where the resource is used**, or **derived from the code**. A wrong declaration may cost speed,
never correctness, politeness or work.

| Need | How |
| --- | --- |
| memory | measured per target (`peak_mb`, the guard's floors, §7.2) |
| input size | measured per target beside its cost (`sizes` in `costs.jsonl`, `pipeline::costin`: bytes read, threads, area, GeoTIFFs opened, the bytes and files its `Out` made on the NAS (`wrote`, `creates`), and the counts a step has: ways and vertices, buildings, peaks); read by the step's model (below) |
| disk | measured per target: bytes written through `store::cachefile` and the scratch writers (`SCENIC_WRITTEN`), plus a look each minute at the largest files in a slot's scratch folder; kept as the target's disk floor (`disks.json`). **Disk guard:** when free space falls below the owner's room target (less a thirtieth of it, so a job just let in at the target isn't frozen at once: `memguard::disk_short_of`), the fastest-growing job is frozen (another too if the disk falls past half the target), room is made from caches no job holds (`room::toward`), and it goes on once the target is free again or is given back. No target is set until the owner picks one (Off is none); with none, the guard fires only at an actually full disk, with no other floor. Given-back jobs' scratch folders are deleted oldest first when room is short (`room::free_given_back`) |
| jobs on one Mac | a job is granted beside others only if their measured memory, disk and cores fit (packing, §7.2; one only waiting on a worker by what it holds and no cores, §7.2, A job waiting on a worker; one leaving the cores idle by its measures and no cores, §7.2, A job leaving the cores idle), and only while every member reports the new app (§7.1, Mixed versions); a cold start by its provisional need (§7.7) |
| raw tiles | `store::cachefile`'s locks; packing deletes only tiles no job holds; a job reopens its shard's archives when the index changed (`RawTiles::get`, `refresh_shard`; a shard is the z6 tile, z3 tile or `root` whose archives hold a tile: docs/macs.md) |
| Wikimedia budget | `crate::budget`: one budget for the pool, enforced at egress. Every job's connections go through its agent's proxy (`crate::budget::proxy`, `HTTPS_PROXY`), the job sandboxed (`sandbox-exec`) to deny any other way out. A metered connection takes a token first: leases are create-new files per slot of a ten-second window (`state/pool/wiki/<class>/<window>-<slot>`). `wiki` four requests a second; `query` (WDQS, QLever) one query per two seconds of query time. A 429 told to the proxy (`POST /hold`, `budget::seen`) holds every Mac. The budget, our User-Agent and the back-off are how the APIs and data downloads it carries are asked politely: they read no robots.txt, which is for crawlers (docs/inputs.md §6.9). The NAS away, each Mac takes the rate divided by known members. Lint: `tests/budgets.rs` |
| NAS and network budget | `crate::budget::nas`: the lead's own jobs always; the members' too unless `state/pool/nas-budget` says `off` (on while missing). A job's **NAS weight** is its share of the NAS in thousandths: its targets' measured rate (`needs::Book::nas`: `bytes` read, less those of this Mac's own disk (`local`, task #551), plus `wrote`, and `creates`, each over its wall time, a halo step's (`pack`, `lo`) `wrote` alone, its job's read rate (`nas_read` over `nas_secs`: what the job read of the halo's base packs from the NAS and the time that took, on each of its targets; a measure without `nas_secs`, task #396's first, counts no reads) in place of its `bytes` (the halo's base packs' files); its own run's, else its step's 90th percentile of each, its runs of every way it ran; none measured, 0, but a step known to read the NAS (`NAS_STEPS`) weighs the whole, task #499 review), and its metadata work (task #451: `listed`, `stats`, `deletes` from `costin::meta`, the folder entries it listed and the files it statted and deleted (renames counted as deletes), each over its wall time; counted by GC's walks, which note a cost line `gc gc` in their job's costs file, and the inputs step's inputs-folder listing), each over the NAS's limit, added: one combined weight, at most the whole (1000); a job its heaviest target's (a job of no targets of its own, its lease's, its step's). **The limits** are measured: the lead's coordinator probes the NAS every 30 minutes, whatever the switch says (`probe`, 4 streams at once (`STREAMS`, task #534: on 21 Oct one stream measured 5 creates/s and 25 MB/s where four made 41 creates/s and eight 57 MB/s, so a single stream set limits a fifth to a half of the NAS's): in each, 12 files of 4 KB made, then in each one of 16 MB written, synced and read back past the cache; the 48 small files deleted one by one, and a kept folder of its own (`<host>-meta` beside the probe's folders, 48 files, never removed by the stale-folder clean-up) listed and its files statted, measured only once it has gone unwritten 5 minutes so the answers come from the NAS, not this Mac's SMB cache of what it just wrote (missing or young: made, and 0 this probe; a 0 weighs nothing and the day's best is kept), in a folder of its own under `state/pool/nas-probe/<host>-<pid>`, all removed after; as it starts, the coordinator removes its host's folders a stopped process left, never another host's) and takes the best of the last day's probes (a probe under the build's load measures what's left, so it's kept: the day's best passes over it, and dropping it would leave a lead started on a busy NAS no limits). The probes are kept on the NAS beside the pool's state (`state/pool/nas-probes.json`, `nas::PROBES_FILE`: one small file written whole every 30 minutes, by the lead's prober alone) and read once the lead knows the NAS, so a restart, or a new lead on another Mac, keeps the day's best (task #603: on 23 Oct a restart under load cut the limits from 86 MB/s to 66.5 and inflated every weight). **The gate on the NAS's own load** (`nas::Loop`, task #534): every 10 s the lead reads each disk's busy share from the Synology over SNMPv3 (`crate::budget::snmp`, SYNOLOGY-STORAGEIO-MIB `storageIOLA`, the one SNMP reader, for the build page's figures too; its keys `NAS_SNMP_USER` and `NAS_SNMP_AUTH` from the project's `inputs/keys.env` (a value with whitespace or `#` is refused, as net-snmp's conf can't carry it: no usable keys, the gate open; the coordinator logs it once, naming the key, never its value), handed to `snmpget` in a private `snmp.conf` (0600, in a 0700 folder under the temp folder, `SNMPCONFPATH`; made once per lead, removed when its coordinator stops, and a stopped process's folder left behind (`scenic-snmp-<pid>`, pid gone) removed as a new session starts; `~/.snmp` untouched), never on a command line, never logged; a poll killed after 8 s) and smooths the busiest disk's (a new reading weighs 0.3: about half a minute's memory). Once that load is over the target the budget is held, and it reopens only when the load is below the target less 3 points (`nas::BAND`, so a load hovering at the target doesn't open and shut it every reading) (`Limits::held`): a job that weighs anything is granted, the lead's or a member's, only beside leased jobs that weigh nothing (`Limits::open`, on top of every rule above; the lead's own jobs asking to go alone (`pick_own`) count the members' jobs alone as the leased ones, so while held a job of the lead's own may still go beside the lead's other own jobs, as intended: the whole-budget rules bind them by weight as ever; weights and room are the probes' as ever, so a held NAS never admits a job an unheld one refuses), whatever loads the NAS (an outside repack counts); jobs running go on. Held, the lead's waiting lines say so: a line for the NAS ("held by NAS load (91 % > target 85 %)") and each target held back for the budget names it too (`Coordinator::nas_held`, `agent::nas_wait_lines`). With no keys or no answer, the gate is open (the probes' limits alone). **The target** is the owner's, pool-wide: `state/pool/nas-target.json` in the project folder, `{"pct": <30–95>, "by": "<who>", "at": <unix s>}`, set by `scenic nas-target [<percent> | off]` on any Mac (off: the file removed, the default; with none, how it stands); it lives on the NAS, not an agent's folder, because the lead alone enforces it: a member's change reaches the lead at its next read (10 s), and a new lead reads the same file. Missing or unreadable, 85 % (`DEFAULT_TARGET_PCT`): never 100, after #451's stall at a NAS held busy, and above the 65–80 % an outside repack alone kept the disks at on 21 Oct, so the pool still works beside one. `scenic pool nas-probe [--dir <folder>]` runs one. (The NAS's space is apart: the owner's NAS reserve, `scenic nas-room`, docs/building.md.) **Enforced at grant** (`coord`'s `pick_every` and `pick_own`, so `pack` too, the lead's own jobs among them): a member's target is granted only while the weights of every job leased in the pool (the lead's own among them) leave room for its own within the whole (1000), or while no other member's job weighs anything (the lead's own alone never hold it back), or, once it has waited 10 minutes for the budget alone and no leased job weighs the whole, if it weighs the whole itself or more than the leased jobs together (`Limits::fits_job`, `OWN_AGE`: the members' jobs never starve it; it waits for heavier work to end, never for lighter; once it's in the leased jobs outweigh any other, so aged ones don't pile up); the lead's own within the whole, or while nothing leased weighs anything, or, once it has waited 10 minutes for the budget alone, while no leased job weighs the whole and the lead's own leased jobs leave it room within the whole (its every-step targets, `pick_every`: `Limits::fits_own_target`); a planned job of the lead's own (`own_nas_fits`) also, not waiting, once it weighs the whole itself while no leased job weighs the whole (`Limits::fits_own_job`; both `OWN_AGE`, the wait timed from when it was first held back and forgotten once granted: members' small jobs never hold it back for good, and the lead's own aged jobs don't pile up past the budget; the simulator checks each start of the lead's own against the same rule, with the waits as the coordinator kept them); **an aged target holds its room** (`Shared::nas_reserved`, task #444): of the targets held back for the budget alone for 10 minutes or more (and passed over in the last two minutes), the one held back first has its weight kept free: no other job weighing anything is granted, a member's or the lead's, unless the leased jobs, it and that one fit within the whole, so a target weighing less than the whole but more than the room left (depth-lo's) goes as the leased jobs end instead of waiting behind a stream of smaller ones; it never lets the budget past the whole, the reserved target itself aside (by `fits_job`'s, `fits_own_target`'s or `fits_own_job`'s aged path), and the reserved target always has a way in once no leased job weighing the whole runs (task #603: on 23 Oct a pack of 946 aged beside the M4's terrain of 103 could pass no rule, a member's aged path then taking only whole ones and the lead's every-step targets none, and the room held for it stopped every other job for 41-62 minutes); a target is noted waiting only when its asker would take it but for the budget (its measured memory and disk fit, or a cold start it's the one to make), so one no asker can take never holds room; a step's grant restarts the waits of its other aged targets weighing no more than the job granted, weighed by the NAS's own limits whoever asked (task #552: it isn't starved, it just took room; else a step granted target by target holds the reservation for good, each grant handing it to its next target, as 52 units aged an hour did on 22 Oct while every other step and the lead's slots waited); a cold job of its own waits for the lead's jobs to end (§7.2) only when it would fit with the members' jobs alone. **The lead's own jobs are always held to it**, whatever the switch says (the limits probed: `nas_own`); the switch, unless `off`, holds the members' too (`nas`). Switched `off`, the members' jobs are not held and the lead's own are held by their own weights alone (`Shared::nas_room_heaviest`: a member's unheld jobs no longer keep the lead idle). The rest wait for a job to end; a target held back for over 30 minutes is a waiting line in the lead's status (`Coordinator::nas_waits`), once each (`agent::nas_wait_lines`). **The clean-up waits for a quiet NAS** (`QUIET_STEPS`: `gc`; task #451, after GC beside two depth-lo jobs held the NAS at 34 % IO wait and all three stalled): it starts only beside leased jobs weighing nothing, or, its weight measured, within the whole with them (`Limits::fits_quiet`), and the room held for an aged target; unmeasured, also only while no leased job is of a step known to read the NAS (`NAS_STEPS`) or unmeasured for it (`nas_unknown_running`). Its wait isn't noted (it holds no room) until it has been held back 24 hours since it was last granted (`QUIET_AGE`, `Shared::quiet_since`, kept in memory: a new coordinator counts afresh): then it goes as an aged job of the lead's own (`fits_own_job`, its wait noted, its room held). Running, it yields at its next safe point (each folder of its sweeps; `control::draining`, its cost line noted first, ending as paused) when a target held back for the budget `OWN_AGE` doesn't fit beside the leased jobs but would without it (`Coordinator::nas_yield`, asked by the agent each loop). **Downloads:** a job of a fetching step (steps table kind `Fetch`) counts one, capped pool-wide by the switch's `downloads <n>` (no cap without it): the lead's own fetching steps (`bld-fetch`, `rail-feeds`; none is offered to members). Not held to it: tasks |
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
when its step is known to honour the threads it's granted (`coord::THREADS_HONOURED`: `slope-hi`,
`slope-deep` and `terrain-deep`, on the rayon pool the agent sizes, the last's reads waiting on the
network; `bld-fetch`, its downloads bounded by `--jobs`), else the whole
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
five minutes it logs the stall with the round's phase in progress (the loop names each phase as it
enters it, and logs one that took 10 s or more as it ends, but the wait between rounds), has
`/usr/bin/sample` sample the agent for 10 s into `loop-stall-sample.txt` (once per stall, the last
stall's overwritten; never two at once, one still running at 60 s killed), and keeps `loop-stall.json` (the round's start, the phase
and since when) in the agent's folder (this Mac's disk), which `scenic status` shows as "this Mac's
agent loop is stalled"; it's removed when the loop goes on, and the round's real length is logged.
These lines carry the time of day (UTC).

Still under no deadline (planned): the NAS's mount and probe (`root`); the build's keys
(`build::Keys::load`, `load_with`); the manifest read through `out::read_record`; the regions'
held report (`inputs::read_report`); the helpers' listing (`state/helpers`); the coordinator's
contact published and removed; GC's reads of the journal and records;
the caches' clears; and every NAS call of the coordinator's and the jobs' own.

### 7.9 The forecasts' archive

Every forecast the lead makes is kept, so its times can be held to what happened (task #621;
`agent::fcarchive`, the record in docs/formats.md, The forecasts' archive):

- **What:** a line per forecast: when it was made and its finish; every target its expected run
  placed (lane, step, target, from and until); and what shaped it: each lane (its Mac, speed,
  time left on its job under way), each Mac's slots, room, conditions (its last ask's memory and
  cores free, limits, in use, on power, battery, NAS) and what held it back over the day, each
  step's time a target, the NAS budget, `forecast::VERSION` (raised whenever a change moves the
  run's times) and the app.
- **Where:** `state/coord/forecasts/<day>.jsonl.zst`, one file a day (UTC), lines appended in
  batches, each batch one zstd frame. Handed over only by the lead's pool step: the forecast's
  own thread makes the entry, the lead's pool step hands it to the archive's thread (a bounded
  channel, never waiting: full while the thread waits on the NAS, the entry is dropped, said;
  nothing on the NAS under the pool's lock), and the thread appends a batch once 30 wait or the oldest has waited
  10 minutes (a few appends an hour; one write each, under `nasread`'s write deadline). A batch
  that fails is kept for the next (300 at most, the oldest dropped, said); one waiting when the
  agent stops is lost (10 minutes at most). A lead that has just handed over may still append its
  last batch beside the new lead's, and appends over SMB aren't atomic: a reader skips a frame that
  doesn't read and resumes at the next frame's magic number, so a crossed append costs its frames,
  never the day.
- **Not twice:** a forecast whose schedule, finish, lanes, Macs (but their passing conditions),
  each step's time and code version are those of the last one written, its times in the same
  2-minute buckets (`fcarchive::GRAIN`), isn't written (a time either side of a bucket's edge
  differs: a repeat is sometimes written, a changed forecast never skipped).
- **Kept for good:** the daily `gc` job packs each day the history's window no longer holds
  (§7.5's cutoff, 7 days) into its month's tar, `state/coord/forecasts-archive/<YYYY-MM>.tar`
  (members `<day>.jsonl.zst`), as the history's days are packed (merged into the tar, never over
  it; written to a temporary name, read back, renamed), then removes the day's file if its size is
  still the one packed. Nothing removes the tars. Measured on the lead's forecasts of 23 Oct (5,800-6,150
  targets placed each): a line ~120 KB, ~13 KB once compressed in a batch, so a day is ~4 MB at
  the cadence the lead's forecast input changed then (about one forecast in 5 minutes) and ~19 MB
  at most (one a minute): 1.5-7 GB a year, kept.
- **Joined with the history** by step and target: a forecast's target is held to the first job of
  that step and target (the history's `lease`→`done` by lease, or a lead's `end` less its `secs`)
  that ended after the forecast was made. A lane's `name` is the history's `worker` (the lead's
  slots past its first `<Mac> #k`), its `mac` the Mac. No key was missing on either side.

**`scenic forecast-check [--from T] [--to T] [--json] [--root DIR]`** (`agent::fccheck`, reads
only) holds the forecasts made in the span (default the last 7 days; T unix seconds,
`YYYY-MM-DD[THH:MM]` UTC, or back from now: `12h`, `3d`) to the history's jobs that ended in it. A
job's targets in one forecast are taken together (its forecast start the earliest of theirs, its
end the latest), so each (forecast, job) is a sample: its end's error where the job ended after the
forecast, its start's where it started after it, the error forecast less actual (+ the forecast was
late). It reports, for the end and the start, n, bias (the mean), and the median and 90th
percentile of the absolute error, overall and by horizon (the forecast's time until it: under
15 min, to 1 h, 4 h, 12 h, more), by step, by the Mac that ran it and by code version; how many
jobs some forecast placed, how often it had the right Mac, and the targets forecast to be done by
the history's end that weren't, by step. As text, a table in minutes; `--json`, the same in seconds.

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

Members' messages to the lead go by HTTP to its coordinator (`POST /pool/mail`), else by mailbox
on the NAS (`state/pool/mail/<to>/<from>.json`, one writer each, read by member id), as all other
messages between members do (§9.1). Jobs go through the lead's coordinator (`/work/*`), tasks
through each member's broker (§8), and the worker page's lead asks through `/work/lead`. Every
member answers on port 8090. A member that can't reach the lead keeps its jobs running and starts
nothing new. A member's agent passes its owner's job cancels to the lead through `/work/cancel`
(§11).

### 9.1 Members' mail by HTTP (task #642)

**What goes by mail** (`crate::pool::driver::Msg`, `crate::agent::pool`): to the lead, a member's
`Tell` (the keys of its journal entries the lead hasn't acknowledged: tens of bytes a key, sent when
they change and again every 10 minutes, `TELL_AGAIN`), `Leads` (once, taking a handed term up),
and the owner's `HandTo`, `Cancel` and `Retry` passed on (rare). From the lead: `Ack` (keys and
the horizon, once per merge) and `Passed` (once per handover). A pool of 3 members sends a few
messages a minute at most; a mailbox holds its sender's last 64 (`KEPT`), each a few hundred bytes.
Nothing is read back after a lead change: each mailbox is to one member, read by it alone, its read
marks in its own folder (`mail.json`); a new lead never reads its predecessor's. What depends on
durability: nothing the driver can't make up. Entries live in the journal, not in the message; a
`Tell` lost is told again (every 10 minutes, and to each new lead), an `Ack` lost is asked again by
the next `Tell`. Only the owner's asks passed on are one-shot: a mailbox kept them for the lead's
next process; the HTTP path keeps that (below).

**The endpoint**: `POST /pool/mail` on the lead's coordinator, JSON
`{"from": <member>, "to": <member>, "msgs": [[<n>, <Msg>], …]}` (at most 64; `crate::coord::inbox`).
- **Auth**: the build's key (the coordinator's contact token, as every agent request: no page may
  call it, a viewer refuses it). Sent only to the lead's tailnet addresses (`mail_urls`: 100.64/10
  or `.ts.net`; never `.local` or a LAN address, any device's on the network to claim), its
  heartbeat's broker's, else its contact's. None: by mailbox. The key is the whole build's, not
  one member's: `from` is what the post says, as a mailbox's path is (any member could write
  another's on the NAS); the lead takes a sender it doesn't know only once its heartbeat is on the
  NAS, and drops a post from one without, so no post makes up a member.
- **Taken only by the member it's to**: the coordinator answers 409 unless `to` is its own member
  (`Shared::pool_member`) and its agent gave it an inbox. A lead that changed mid-send still takes
  mail to itself, as its mailbox would; an address that now answers as another member refuses it.
- **Durable before the answer**: the messages are added to the inbox (`inbox.json` in the agent's
  pool folder, on the lead's own disk, written whole and flushed) before the 200 `{"took": <n>}`.
  A crash after the answer loses nothing: the next process reads the inbox. A write that fails is
  an error, never a 200.
- **Read once**: the lead's agent reads its inbox with its mailboxes, each sender's messages merged
  and taken in its numbering's order, each number once (a message may come both ways); its marks
  written, then what it read dropped from the inbox. A forgotten member's dropped.
- **The member**: each message to the current lead is posted with every earlier one the lead's
  coordinator hasn't taken (`Mail::posted`), so a mailbox message not read yet rides along too.
  Anything but a 200 taking them all (unreachable, 404, 409, 5xx, 15 s an address gone by): the mailbox is
  written as before, and HTTP left alone for 5 minutes (`POST_AGAIN`). A lead changed mid-send is
  either still the member the message is to (it takes it) or refuses it (mailbox); either way the
  driver re-tells the new lead.

**Mixed versions**: each heartbeat says whether its app takes and posts mail so
(`Heartbeat::mail_http`). A new member and an old lead: the lead's heartbeat doesn't say so, the
member never posts (and a 404 falls back too). An old member and a new lead: the lead reads its
mailboxes as ever. Messages from the lead to members stay by mailbox.

Planned (a later, gated step): **retiring the mailbox.** Once every member's heartbeat in touch says
`mail_http`, and has for a day, a lead stops reading mailboxes (as #640 and #644 gate on the
heartbeats); a member then keeps its messages queued while the lead is unreachable rather than
writing a mailbox. The lead's messages to members go to the members' brokers the same way. Also
planned: `/pool/status` in place of reading heartbeats for the view.

## 10. Seeing the pool

- **Heartbeat**, `state/pool/members/<id>.json` (its own writer): label, app, resources, conditions
  (`conds`), slots' jobs, tasks, addresses (`broker`), reach (§6.8), term led, `handing_to`,
  `ready_for`, `stood_down`. Written when it changes, at least every two minutes. Listed every two
  minutes; a member missing from listings for twenty minutes is forgotten.
- **Build status**, written by the lead per term (`state/build/term/<E>/status.json`): the
  checklist (a line per stage with an item per step: docs/pipeline.md §4), forecast, regions,
  waits, what publishing does, publishing as it stands (`published`: the last catalogue and its
  kind, the map tiles being drawn by Mac, each Mac's adopted catalogue), recent jobs, pause, last
  catalogue, the lead.
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
  time changes), `state/build/pause.json` (also alone each second between the full reads; its absence is no pause, never the view's; on the lead the answer's pause is its coordinator's own, which the NAS copy follows only at the agent's next loop), the term's `regions.json` (again only when its time changes), each member's heartbeat (`state/pool/members/<id>.json`,
  for its host) and own status (`state/helpers/<host>.json`), and what each member's history
  file of the last 7 days (today's and the six before: `pageview::DAYS`) gained since the offset
  it last read to (whole lines, 4 MiB of a file a read at most, `CHUNK_MAX`, the rest the next
  time; a whole 4 MiB without a line's end passed over, counted and logged; from the start again when the file is now shorter than that). A day before yesterday
  read to its end isn't read again. Of the events, the last week's are kept, without their
  `data` (the page doesn't show it), the newest 50 000 at most (`history::KEEP_MAX`, as the
  lead's own history): `/work/history` answers from those. Measured on the NAS's real history (23 Oct: two writers, 3.9 MB over the
  days kept, 9 719 events in the week): the first read 1.4–10 s (the NAS's own speed: a plain
  `cat` of the same files took 4.6 s), its peak memory about 20 MB; each read after it 0.13–0.15 s;
  `/work/history` with all 9 719 events about 1.9 MB of JSON, made in ~20 ms. The reads are off
  the answers' path, so a page waits only on its first (the 503s above). The agent's loop hands it only what it
  holds already (the term and the members' ids from the pool's driver, this Mac's app): no NAS
  read on the loop, and this Mac's lanes worked out only while a page asks. Every read is done
  before the pageview's state is locked, and the results swapped in after, so a slow NAS never
  holds up an answer. The history is merged by time, writer and number, and numbered by this Mac
  as it arrives; the answers carry the numbering's `epoch` (new with each process's first read),
  and the page starts its cursor again when the epoch changes (the Jobs tab's past jobs are asked further back by `since`, at most 5 000 a time) or `seq` falls below its last event
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
  its views, and the pages' asks (§11, and the settings: docs/workers.md §7), passed to its
  agent, every other call refused (its key's too); no
  state folder of the agent's (its token alone, in a scratch folder removed as it stops, a dead
  process's as the next starts), no history, no background loop. The usual Origin rules hold. A
  history file written again in its place (its inode or first line changed) is read from its
  start, the events taken in already passed over. The menu bar's panel loads this Mac's own page
  alone (docs/building.md §2, Status).
- **Phase shares** (task #511): for each step, its top-level phases' mean share of a run's wall
  time and of the bytes its phases counted as read or written (local disk, NAS and internet alike:
  `timings::count`; the page says "I/O"), over its last 10 runs that ended well
  (`timings::SHARES_RUNS`; the bytes' mean over the runs that counted any), largest by time first.
  The lead's status carries them as `phase_shares`, by step id (the steps table's; a leased task's
  runs, timed as "task <step>", are its step's: `timings::step_of`):
  `{"<step>": {"runs": n, "phases": [{"name", "class", "wall", "bytes"}]}}`, `wall` and `bytes`
  fractions 0–1 (`timings::PhaseShares`); a step with no good run in the log is absent. A phase
  missing from a run counts 0 there; the rest of a run's time is in no phase. Every Mac's page shows
  the same. Only the lead holds the build's `coord/timings.jsonl` (in its agent's folder, on its
  own disk); its coordinator reads it on a thread of its own every minute (`coord::phase_sharer`,
  ending within a second of the coordinator's stop), never on the agent's loop or under the pool's
  lock: the file's size taken first, then only the whole lines added since (`timings::LogTail`; a
  partly written last line waits for the next read; a log cut to its newer half, or replaced, i.e.
  another file at the path or a shorter one, is read whole again, at most 16 MB). A member's status
  carries none.
- **Each Mac's jobs** (the build page's machines, `scenic status`, task #246): a block per Mac,
  the lead's first, marked coordinator (`scenic status` with its ramp, §7.1); its capacity as the coordinator last
  heard it (memory held of the guard's limit and free for more, cores busy, disk reserved and
  free, on the page as bars); a block per
  running job numbered by its real slot (`JobView::slot`: slots may be sparse, so a Mac running
  only its fourth slot shows slot 4), (`scenic status`: with that slot's lane's next work); and (`scenic status` alone) why nothing more
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
- **Regions** (`agent::regionsview`, task #512): each enabled region's `crumbs` (its folders'
  names in the regions' tree, a folder without a `_group.toml` name by its own in words, then its
  own), its `bytes` (the catalogue's files a download of it copies, `areafiles::files_of` as the map
  server's downloads count them; not the basemap's pieces nor its terrain detail; null while the
  catalogue lacks it), and its outline for the world maps of the Overview and Regions tabs: its
  drawn shapes in Web Mercator, the world 0–1000 each way, simplified to about 7 km, `fill` the
  outer rings and `edge` the lines to stroke (a stretch two of its shapes share along a parallel
  or meridian left out). Made by the lead, on a thread of its own, once per regions' accepted
  version and served catalogue (the status's `catalog`; a failed read not tried again until one
  of them changes): the drawn files read then, the catalogue's sizes once per catalogue, from
  the catalogue the agent's loop read (`agent::served`: each new catalogue read once, by its
  new deltas applied to the last, for the loop, its regions on the map, publishing's times
  (`pubtimes::PlannedNotes::served`) and this view alike; a delta checks the catalogue it
  changes by its digest, so a number taken again is read afresh, never served stale; task
  #684: read three times over, each back through 135 deltas to a 9 MB whole one, it held the
  lead's agent near a whole core, stalled its loop 300 s after each catalogue and took this view
  136 s on 26 Oct). regions.json is about 0.96 MB. Written by
  the lead's pool step as `state/build/term/<E>/regions.json` (again in each new term), read by
  each Mac's page data when its time changes. Every Mac's port answers `POST /work/regions` with
  no key: in the pool from that read, out of it from this Mac's agent's; 503 until there is one.
  Steps (task #636): `POST /work/regions` with `{"steps": true}` answers each region's targets
  built of all by step, `{at, regions: {id: {step: [built, all]}}}` (`build::region_steps`: the
  plan's targets of each step that builds regions, counted whole: "unit" its z6 tiles,
  "terrain-hi", "terrain-deep" and "terrain-lo" those of the z3 tiles it reads, "slope-deep",
  "slope-hi" and "slope-lo" its z3 tiles', "trees"
  and "trees-lo" those of the z3 tiles it meets, "pack" the map tiles its z6 tiles' ways reach that the
  plan draws; a step with none left out; a target shared by regions counted in each). In targets, not seconds: the records know which targets are built,
  not what each took, and a step's targets take about alike. Counted by the lead (or a Mac out of
  the pool) on a thread of its agent's (about a second on the whole build's records, the loop's
  part ~40 ms: the coverage and reaches it holds, a copy of the terrain packs' indexes), only while
  a page asked within two minutes (`coord::REGION_STEPS_ASKING`; the page asks only while its
  Regions tab shows): again after a job ends or each minute; held in memory (a few numbers a
  region), nothing written to the NAS. A member's port answers 503: the page then
  asks the lead's addresses.
  To come (tasks #622, #685): the lead's port answers `POST /work/tocome` (`{}`; no key) with every
  Mac's tiles in its last forecast (`forecast::ToCome`, made of `Forecast::placed` on the
  forecast's thread before `placed` is dropped): `{at, macs: {mac: {step: [[z/x/y, s from at]…]}}}`,
  each tile once a Mac and step at its first start, targets naming no tile left out: the page's
  map's queue; and the schedule's runs (`forecast::Runs`, task #787): `runs`, each run's targets,
  the runs numbered as the forecast's `lanes` hold them (the lanes by name in byte order, each
  lane's runs in order), and `deps`, [a, b] when an item of run b waits on one of run a (its own
  deps, a release's, or a set's it waits on whole), each pair once. Measured on the 26 Oct 20:15
  record (31,132 items, 850 runs): `macs` 642 KB, `runs` 366 KB, `deps` 117 KB (12,046 pairs);
  ~275 KB gzipped whole, though sent uncompressed (the agent compresses no answer). Another Mac's port answers 503 (it made no forecast), or a former lead's its last,
  older one: the page takes its own Mac's answer only when its `at` is the status's forecast's,
  else asks the lead's addresses; a lead change drops the old answer at once and asks the new
  lead. Asked once a forecast; never in the 10-second poll. Measured on the 24 Oct forecast
  (3,618 targets placed, 3,615 tiles): about 62 KB (the Mac mini's 52 KB, 3,053 tiles, the M4's
  10 KB); held by the agent: ~150 KB.
  Every Mac's port also answers `POST /work/built` (no key; task #674) for the map's finished
  tiles: `{at, steps: {step: [z/x/y…]}, error}`, every tile target the build's keys
  (`state/build/jobs.json`) record, by step (a `-root` step's as 0/0/0; worldwide steps', records of steps the app doesn't know (a
  retired step's) and terrain-hi's, slope-hi's and tree cover's old z3 whole-run records left out:
  `agent::builtview`), read by the agent on a
  thread of its own while a page asks, again a minute after the last read at most (~600 KB over
  SMB), the last read kept when the file can't be read now. The page asks each minute while the
  map is drawn.
  Every Mac's port also answers `POST /work/catalogs` (`{"after": n}` optional; no key) for the
  map's age controls (task #629, `agent::cataloguesview`; the page asks the lead's): every
  catalogue on the NAS as `{n, t, whole, first?, z3, z6, z3_gone?, z6_gone?}`, the tiles as
  `y * 2^z + x`, with `latest`, `reading` and `error`. A catalogue publishes a z6 tile when a file it
  names for it (base pack, road values, hi data, landmark points, a layer's hi pack, the fine
  terrain under it) is new or another than the last catalogue's, a z3 tile by its layers' lo packs
  and area overlays; `first` marks the oldest kept (it names everything). Read only while asked:
  an ask starts the agent's reader thread when the folder wasn't listed in the last 20 s; it reads
  the catalogues after the last it read, oldest first (a delta when it changes the last read, else
  the whole file, streamed from zstd into only the tile sections), through nasread, and the answer
  is what's read so far. Kept: the entries and each tile file's name hash by its key (about 18 000),
  never a catalogue. Measured 24 Oct on the M4 (372 catalogues, 15–386, 14 whole): the first read
  35 s (debug build, mostly the NAS) at 34 MB peak; the whole answer 67 KB of JSON (17 KB
  gzipped), a new catalogue's a few hundred bytes. `scenic catalogs [--after N]` prints the same
  JSON from the NAS (read only). 503 until the agent has made it.
  The page's own files carry the world's countries' outlines for the same map (`countries.json`,
  Natural Earth 1:50m, the same projection).
- **Live** (`agent::live`, task #514): each Mac's own measures over time, in rings on its own
  disk (the agent's folder's `live.bin`, never the NAS; written every 5 minutes and as the agent
  stops, read back as it starts): a point a second for an hour, every 10 s for a day, a minute
  for three days, every 5 minutes for seven (task #624), each point the least, mean and most of
  its samples; each whole UTC day of the seven archived once to the NAS from the finest ring still
  holding it (yesterday by the minute, older days by 5 minutes), one file per Mac per day,
  `state/live-archive/<host>/<day>.bin.zst` (the store's format, that ring alone, zstd; at most ~0.8 MB a day, measured with 70 series of random values),
  written in one create and rename on the NAS sampler's thread (never the agent's loop or the
  pool's lock; the folder listed once as the agent starts, then one write a day), kept for good.
  Sampled on the agent's own
  threads, never its loop: every second the load (`load1`), memory available (`mem_free_pct`,
  `kern.memorystatus_level`), its pressure (`mem_pressure`: 1, 2, 4) and the swap (`swap_mb`),
  and the jobs running (`jobs`); the cores busy in cores' worth (`cores_busy`, and by group
  `cores_busy_e`, `cores_busy_p`: `host_processor_info`'s ticks over the second, the efficiency
  cores the first processors, as many as the last perf level's `logicalcpu` (named Efficiency;
  checked on the M1 and the Mac mini by ioreg's cluster types); "P" is every other core, Super
  and Performance together on a three-level chip such as the Mac mini's; no per-core series: each
  core's share over the last second is kept apart and answered as `cores_now`, efficiency cores
  first, task #609); the build against
  the rest of the Mac (task #515): its jobs' CPU in cores' worth (`build_cores`, their processes'
  `proc_pid_rusage` over the second) and the rest's (`others_cores`: the cores busy less the
  build's), their memory (`build_mem_gb`, the jobs' physical footprints; `others_mem_gb`: the
  memory in use by `kern.memorystatus_level` less the build's); the bytes written to the Mac's
  disks (`disk_write_mbs`, MB/s, its block storage drivers' statistics through IOKit, as `iostat`
  reads them) and by each job (`job_write_mbs.<slot>`, its processes' `ri_diskio_byteswritten`);
  a job's first second sets its baseline, and a process's last moment before it ends isn't counted.
  The sampler's own CPU time (`sampler_cpu_ms`, its CPU time a second, the overhead's own measure; each job adds a `proc_pid_rusage` a process). The
  memory compressed (`mem_compressed_gb`, `host_statistics64`'s compressor pages) and macOS's
  thermal state (`thermal`: 0 nominal, 1 fair, 2 serious, 3 critical, ProcessInfo.thermalState's
  levels, read from the public `com.apple.system.thermalpressurelevel` notification's state, a
  shared-memory read). Every 10 s the disk's free space (`disk_free_gb`) and size (`disk_size_gb`); the power (`ac`, `battery_pct`) and
  the owner's idleness (`idle_s`) as each loop reads them, and with them the build's limits (task
  #609): the cores it may use now (`build_cores_max`: all, less those kept for the owner while
  they use the Mac), the memory its jobs may hold together (`build_mem_max_gb`), the owner's room
  target (`disk_room_gb`) and the jobs' disk reservations (`disk_reserved_gb`). An answer's `mac`
  also says the battery floor the agent last went by (`battery_min`, the owner's setting) and the idleness counted away (`away_s`). The NAS's latency (`nas_ms`) every 5 s
  only while a build runs here (a job of this Mac's, or its coordinator's leases out): a listing
  of the project's folder on the records' readers, timed inside the reader (`nasread::probe`; a
  stat would come from macOS's SMB attribute cache; never two at once, one past 20 s dropped);
  the status's `nas_ms` is its last within half a minute. **The jobs' NAS traffic** (task #527), every second: each
  process of a job counts what it does where it happens (`store::nasio`, the one per-process
  counter set, whose deltas are also a target's cost-line measures, `costin`: the store's readers,
  a build's `Out`, the metadata walks `costin::meta` notes; beside the NAS's, the local disk's
  reads and the GeoTIFFs opened) into a file of its own, `<agent folder>/nascounts[-k]/<pid>-<nonce>`
  (a later process with an ended one's pid makes its own), ten u64s mapped shared (no write call a
  count; on this Mac's disk, never the NAS). A program a step runs counts in a folder of its own
  inside its runner's (`sub-<runner pid>-<n>`, summed one level down, so its traffic shows as it
  runs), moved out and added to its runner's as it ends (the rates keep the most seen, so that
  moment's dip is never counted twice). The agent folds the files of
  processes that ended into the folder's `base`, sums each job's folder and records the jobs' NAS
  traffic as rates: `nas_read_mbs`, `nas_write_mbs` (MB/s), `nas_files_s` (files opened to read),
  `nas_creates_s`, `nas_listed_s` (entries), `nas_stats_s`, `nas_deletes_s`. A job's first second
  sets its baseline; a job given no folder (an older agent's) leaves the second unmeasured. Not
  yet counted: the build's own copies to the NAS outside `Out` (scenic-build's direct copies), and
  the agent's own NAS work. **The file stream** (task #522): after its counts and its phase, a
  process's counts file keeps a ring of the last 32 NAS files it opened for reading (the store's
  pooled and plain readers, `localread`'s whole copies) or made (`Out`'s uploads and
  `store_bytes`): each its path's last 112 bytes, its bytes, how long its open or copy took (ms)
  and when, in a slot of its own written under a sequence number (odd while written; a few relaxed
  stores in the mapped page, no write call, nothing on the NAS: ~125 ns a file measured, against
  the NAS's tens of ms an open). Every second the agent reads each job's running processes' rings
  (`store::nasio::notes_in`: each file read twice, a slot taken only when both reads agree) and
  keeps the newest 200 files in memory (`agent::live::FILES`, never saved), each with its job's
  slot and its path below the NAS's root. A ring's slot overwritten within the second is missed:
  the stream is what moves now, not a ledger. At the pool's rates measured on 23 Oct over a day
  (the lead's `pool_nas_files_s` and `pool_nas_creates_s`: means 0.25 and 0.16 a second, peaks 18
  and 5 a second) that is under 30 files a second at the most, a few µs of CPU a second. **The NAS as a whole** (task #517): each member's ask carries its
  jobs' NAS traffic now (`Capacity::nas_traffic`, its newest second of the seven series), and the
  lead sums the fresh ones (its own among them) every 10 s into `pool_nas_read_mbs` and the other
  six (`pool_` then the name); on the lead too, from the load gate's own SNMP reading every 10 s
  (§7.7; no second poller), the disks' busy share, `nas_disk_busy_pct` (the busiest) and
  `nas_disk_busy_pct.<n>` (each, from 1). Each Mac's agent's NAS calls under their deadlines
  (`agent::nasread`, §7.8) every second: `nas_calls`, in flight now in its three pools (stuck
  ones included), and `nas_missed`, those that passed their deadline or were refused, every
  thread busy, over the second; each member's ask carries them too (`Capacity::nas_calls`,
  `Store::calls_now`: in flight now, missed a second over its last 30 s, its NAS call threads),
  summed by the lead every 10 s into `pool_nas_calls`, `pool_nas_missed` and `pool_nas_threads`
  (task #634; memory only, like every series). The lead's status says the rest (`nas`, `coord::NasView`): the
  day's probes as its prober keeps them (the ones in `state/pool/nas-probes.json`, read from there
  and saved there, no other copy), the limits they give, the gate's smoothed load, target and
  whether it holds, the disks' last reading (finite values alone), the pool's traffic summed, and
  the NAS's mount (`mount: {name, proto, free}`: the server's name and protocol from what the
  project folder's filesystem mounts, `//user@fishandchips.local/personal` giving `fishandchips`
  and `SMB`, and the bytes free; a statfs by the prober's thread once a minute, never the agent's
  loop). Beside it, `nas_step`: what one job of each step leased now weighs of the budget
  (`Coordinator::nas_step_weights`, thousandths of the whole).
  The sampler's cost shows in `sampler_cpu_ms`; an ask carries seven more numbers. **The internet** (task #516), counted by the agent's proxy (§7.7), which
  every job's connection out goes through (`budget::net`): per host, the bytes in (to the jobs)
  and out, added as each chunk is relayed (one atomic add a read, so a long download shows as it
  goes), the requests asked (a tunnel is one: what's inside is the client's TLS), the connections
  opened, the retries (crate::fetch's: each wait before asking again tells the proxy, `POST
  /retry?host=`) and the 429 holds told (`/hold?s=&host=`), and the budget its connections are
  metered by (`limit`: `budget::net::limit_of`, in words from its class's rate; none unmetered), and its
  robots.txt as a job fetching its pages told it (`robots`: `{state, t}`, `POST
  /robots?host=&state=`; null never told);
  and a ring of the last 200 transfers,
  each `{t, host, in, out, ms, outcome}` as it ended (`ok`, `broken`, `unreached`: answered 502,
  `refused`: 503). The counts are since the proxy started, in memory alone; their totals sampled
  each second as `net_in_mbs`, `net_out_mbs` (MB/s) and the requests asked as `net_req_s` (a
  second; task #620). The status's budget counts (`wiki`, `query`,
  `other`, `holds`) are summed from them. Each job's own bytes in and out are counted too (task
  #518): its agent gives it the proxy's address tagged with its slot (`HTTPS_PROXY` and the others
  as `http://slot<k>:x@127.0.0.1:<port>`, which ureq, curl and Python send as
  `Proxy-Authorization`; `SCENIC_BUDGETS`, where 429s are told, untagged), counted from none as
  each job begins; a client that sends no tag is counted to its host alone. **Each running job,
  live** (task #518): the status's `jobs[].live` and its lease's beat to the lead (`/work/beat`'s
  `live`, kept on the lease in memory and shown in the coordinator's leases), read as the status
  is: `{phase, class, phase_since, target, target_since, targets_done, targets, nas_read,
  nas_wrote, net_in, net_out}` (bytes since the job began). The phase is the top-level phase its
  main thread has open (pipeline::timings: its name and class, since when, unix s), said in the
  job's counts file after its counts (`store::nasio::set_phase`: a sequence number, odd while it's
  written, its time and 48 bytes of text, a few stores in the mapped page, ~40 ns a call measured; never
  summed), read from the newest set among the folder's running processes' files, each read twice and taken only when both reads give the same even sequence number (so a text torn by a write mid-copy is never taken). The target is
  the one its costs file says is under way (`started`, not yet ended) and since when; of its
  targets (the lease's), how many ended; the page weighs `now - target_since` against the step's
  `each_s` (the forecast's). Each field missing: not said (a step that times no phases, notes no
  costs, was given no tag or folder; a job taken over from an earlier agent counts no internet bytes, its tag begun by that agent's proxy). The counted relay copies in 64 KB
  chunks: 4 GiB over loopback in ~1.0 s on the M1, against ~2.6 s with `io::copy`'s 8 KB it
  replaced (task #516), so the counting costs nothing measurable. Every Mac's port
  answers `POST /work/live` with no key: `{range}` (seconds back from now, an hour by default,
  seven days at most) or `{since}` (unix seconds: the points from then on, the last again as it
  may have grown), `series` to pick names; the answer `{host, now, step, from, series: {name:
  [[t, lo, mean, hi], …]}, net, files}` from the finest ring covering the window, its buckets grouped
  to at most 600 points a series (`POINTS_MAX`; an hour is a point every 6 s, a `since` ask grouped
  the same, by its `range`); `net`, the internet's
  counts (`{hosts: {host: {in, out, requests, conns, open, retries, holds, limit}}, events}`; `conns`
  opened, `open` open now) once the
  proxy runs; `files`, the file stream, oldest first (`[{t, kind: read|wrote, path, bytes, ms,
  slot}]`; both left out when asked with `net: false`). A series missing has no
  measure yet (or none in the window): readers say so, never zero. 503 until the agent gives its
  port the store. Another Mac's page reads it directly: `/work/swarm`'s `macs` lists each member's
  host and addresses (`{host, urls, https}`, its heartbeat's broker; none with the brokers'
  switch off), and the answer is allowed to the members' origins (CORS as for tasks, §8; the
  M6's https://map.brandontsang.net among them), the page reading by the scheme it was served
  over. The build page's Machines tab draws it (docs/pipeline.md §5, Machines), asking with
  `net: false` (the proxy's counts left out); an answer also says the Mac's size, `mac: {cores,
  cores_e, mem_gb, nas_threads}` (read once: `nas_threads` its NAS call threads, the most calls in
  flight at once), the charts' scales. Its NAS section (`web/work/nas.js`) reads the same answers:
  the lead's `nas_ms`, `pool_nas_*` and `nas_disk_busy_pct` (missing from a member's answer); beside the
  status's `nas` (`NasView`). The
  Activity tab (`web/work/activity.js`) asks every Mac every 2 s while it shows, with no series:
  their `files` and `net` for the file stream and the internet by website, each host's `robots`
  among them: a job fetching a host's pages (`FETCHES = "page"`) tells its proxy the robots.txt
  state (`POST /robots?host=…&state=unchecked|allowed|disallowed[&t=<read, unix ms>]`, crate::budget::net::Robots: a host name, at most 4096 hosts,
  with when), null for a host never told: an API's or a data download's, read without robots.txt
  (docs/inputs.md §6.9).
- **Views** (menu bar, worker page, `scenic lead`, `scenic status`) read the
  agent's `pool.lead` (`crate::agent::lead::View`) or the fresher `agent/pool/view.json`
  (`lead::own_status`): each member's state and whether the lead can be handed to it
  (`Driver::hand_to`), what a takeover needs (`Driver::takeover`), "no lead" and why, the last ask
  and change of lead.
- **The alert** (`lead::alert`, `ALERT_S`: an hour with no lead) says no Mac took it over by itself
  (none could lead, or the app rule refused) and names the command to take over; the menu bar
  notifies once.
- **History:** term changes, step-downs and handover events are `term` events in the member's
  history file (`crate::agent::lead::notes`). Members joining and leaving are the lead's `member`
  events (`lead::membership`): at each read of the heartbeats, a member whose beat came into
  touch ("joined the pool") or went out of it (`OUT_OF_TOUCH_S`, "left the pool (out of touch)");
  a lead's first read only sets what it compares with, so a new lead or a restart notes none.

## 11. Controls

`crate::agent::lead`, `tools/status/main.swift`, `web/work/pool.js`:

- **Menu bar:** on the lead, "Hand the Build To ▸" with each member's state (greyed with why); on a
  member, "Make This Mac Lead", or "Take Over the Build…" with no lead in touch (says what it forces
  or downgrades); Pause/Resume.
- **Worker page:** "Make lead" per member, a handover's progress, "Take it" (unforced), from a page
  on any device (task #521: no page of the Mac's own first). On a Mac not leading, these and Pause
  go to its agent, which passes them on (the pause as its menu's, `crate::control::request`; the
  lead asks and the inputs' acceptances from their queues, as on the lead).
- **Cancel and try again from the page** (task #409): a page on any device (`/work/act`)
  cancels a running job
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
- **Building finished targets again** (`crate::agent::redo`, task #780): `scenic redo <step>
  <target>… [--run]`, from any Mac. A target done under its key is never planned again, so one
  saved wrong under the right key (25 Oct: terrain-hi z6 tiles saved without their water while the
  NAS restarted) stays until named here. Without `--run` it reads the newest records and prints,
  per target, the key it would clear and the files that stay served until the target is rebuilt
  (a target with no key: nothing to clear); it changes nothing. With `--run` it posts the targets
  that have a key to the lead's coordinator (`/work/redo`, the build's key: not a page's), whose
  agent keeps the ask in its folder (`redo.json`, over restarts) and plans a job of the `redo`
  step for it, the lead's own (`every::LEAD_ONLY`). That job changes no file (its write-set is
  empty) and hands off a done record of `<step> <target>` with no key; the merge checks it as any
  entry (§7.3), and takes it only from the lead of its lease's term (`agent::pool::by_its_lead`:
  another member's refused, one whose term can't be read held), and `Keys::record` forgets those targets' keys and nothing else, the lease order
  kept by `<step>, <target>` as a prune's is (a build of the target leased before the redo,
  merged after, is passed over). The job succeeded, the ask leaves `redo.json`. The plan then
  offers the targets again under the keys it has; what reads their outputs follows by content
  (slope-hi's key names the terrain hi pack's content, the assemblies' keys their z6 tiles'
  mids), and the old files leave the manifest as the new ones replace them, for GC (§7.3, 14
  days). The coordinator answers the ask with the last lease it granted before it
  (`after`); the redo's job is leased later. The command then follows the newest records until
  the redo's merge lands in them: each target's last merged lease (`Records::last`) is later than
  `after`, so a job leased before the ask that merges meanwhile isn't taken for the redo; a lead
  without `after` in its answer is followed by each target's lease before the ask. Not until the
  keys are gone: the plan rebuilds the targets as the merge lands, and a rebuilt one has its key
  again within minutes. 15 minutes at most; a longer wait fails with that said, and an unreadable
  read is said and read again. A
  lead on an app from before `scenic redo` answers the post with an error: the command says so
  and asks nothing. An entry of a redo merged by such a lead (a takeover replaying the journal)
  clears nothing (that app records `<step> <target>` under the lo keys, as an unknown step's): the
  targets keep their keys, and `scenic redo`'s next dry run says so.
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

A depth, terrain-hi or terrain-deep job resumes its target at the block after the last it
finished, from a log in its scratch folder on its Mac (§7.2, Resuming; crate::resume_log), with
`resume` on.

Still behind switches under `state/pool/` (a file; off while missing unless noted):
`resume` (§7.2, §7.4), `brokers` (§8), `placement` (§7.2),
`memory-guard` (on while missing). A new switch is made only once every
member's heartbeat shows the app that has it. A part changing in its process (§7.6) is no longer
switched: `state/pool/slots` isn't read, but stays `on` on the NAS while an older app may read it
(one reading it missing or `off` restarts to change part). The old files `state/pool/enabled` and
`state/pool/budgets` are no longer read either.

Planned: readers reading the term's snapshot and the lead no longer writing today's three files;
retiring the mailbox (§9.1); resuming the terrain's and the slope's z6 tiles (after #639). Packing a terrain z6 tile's fetched raw
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
  beside another reading it there (§7.1, Disk reservations): a member copying a shared cache a
  part at a time is reserved what it truly lacks at each first job, and again in full after its
  cache is emptied, the pass the same; with z6 tiles in the plan, a halo's map tiles are offered once its z6 tile is
  built, before the z6 tiles left: on the M4 and the M1, the NAS budget on and both steps measured
  before, each Mac runs map tiles beside z6 tiles, and the plan finishes. The M4 as measured on 18
  Oct (48 GB, 14 cores, 85 GB free, its caches on its disk so none of them reserved; z6 tiles of 6.6
  GB writing 6 GB, `pack` and `lo` 0.3 and 0.5 GB) runs four z6 tiles at once within its memory and
  disk, map tiles and low zooms beside each other. No start over-commits the disk, and z6 tiles
  still run three or more at once. Not
  modelled: other step dependencies, failures; a job's time doesn't depend on its threads.
- **The agent's tests** (`crate::agent::pool`, `crate::agent::lead`, the agent's `pool_tests`): two
  agents in one process, a lead and a member, through handover, takeover, controls, hand-offs and
  every switch on and off.
- **Two Macs on the real NAS** (`tools/check/pool-two-macs.sh`): the real agents on a scratch
  folder of the NAS, jobs played by a script; each switched batch is run so before its switch is
  made on the real build. Planned: load.
- **The share itself** (`tools/check/nas-probe.py <peer> <scratch folder>`, run on one Mac, which
  times everything on its own clock): contended create-new and stale reads, stats and listings
  (§3's figures).

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
