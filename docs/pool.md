# The pool: any Mac can lead the build

Status: **phase 1 built, on since 8 Oct 2026 and always on** (`crate::pool`, its driver and
phase 2's transitions with it; the agent's part, crate::agent::pool: §12); **phase 3's controls
built** (crate::agent::lead: §11); **phase 4's thirteen batches built** (§12): the steps table
(crate::agent::steps), the memory guard (crate::agent::memguard), power, home and the in-use rule
gone, the lead's slots as any member's (behind its switch), disk measured and the Mac shared by
measure with its disk guard, the write-sets enforced, exclusivity where the resource is used (the
Wikimedia proxy with the pool's one budget, every job sandboxed; the answers' lease), resuming
(behind its switch), every step to every member, tasks brokered by every member (behind its
switch), placement (behind its switch), the forecast for equal machines, and what goes (no switch
for the pool, no claims, no named writer, no shadow run). Planned: the records' readers reading the
term's snapshot, and the lead no longer writing today's three files (§6.2, §12). The build is
led by a pool of peer Macs, any number of them, one of which leads the build at a time, and the
browsers' pages are workers of the same standing, by one model of work. The lead can be handed to another Mac from any Mac's menu, the
worker page, the map's build panel or `scenic lead`, and taken by another Mac when it's gone.
Nothing about a Mac's role is fixed at install.

## 1. What changes, and why

A Mac whose role is fixed at install (only its agent plans, runs the coordinator, writes the
records and publishes; the coordinator's state on its disk; the worker page served from it alone;
the pages' tasks offered by its jobs only) stops the build when it's taken away from home or
offline, strands what's on its disk, and can't hand the role on without reinstalling both Macs.

The pool separates two things:

- **The work**, which every Mac does alike: each runs job slots that lease work, run it and hand
  the results back, and brokers the tasks its own jobs offer. The lead's slots are workers like any
  other's. The pages are workers too, of a kind that runs tasks.
- **The lead's duties**, which one Mac performs at a time: planning, handing out the jobs, merging
  the results into the build's records, publishing, and keeping the build's state (its pause,
  forecast). They're a role a Mac takes on and gives up, not what it is.

And it puts everything durable on the NAS, in a form two Macs can't corrupt between them whatever
their timing (§3, §4), so the role can move without copying anything from one Mac to another, and
a lead that vanishes loses no work.

## 2. Principles

1. **One agent, every Mac.** The same app and the same `scenic agent` on every Mac, installed the
   same way. Whether a Mac leads is read at run time from the NAS (the terms, §6.1), never from how
   it was installed. A Mac is known by a member id made once, its host name only a label.
2. **Durable state on the NAS; a Mac's disk holds caches and work in progress.**
3. **Nothing on the NAS is written in place by two Macs.** A file has one writer (its own member's
   files, its journal entries among them), or is made once with create-new and never changed
   (terms, refusals' notes), or belongs to one term (the records, the coordinator's state). A late
   write from a Mac that slept through a change lands where no one reads; it's never needed to be
   refused.
4. **Every job hands its results off.** No job writes the records itself, on any Mac, the lead's
   included: it uploads its files to the store and writes its record changes to the journal on the
   NAS; the lead's merger alone applies them. A job runs the same wherever it runs.
5. **Work goes where it fits.** What a step needs (memory, disk) and what it writes are measured or
   made by the code that does the work (§7.7: memory and disk built, the rest planned); any member with that may run it, the lead included. The few
   duties that must be the lead's (publishing a catalog, removing replaced files) are its duties,
   not jobs.
6. **Handing over is a protocol, not a restart:** asked for, agreed by both Macs, carried out in
   steps anyone can watch, taken back if the new lead doesn't take up, with nothing lost and no job
   stopped.
7. **Tasks belong to whoever offers them.** A job's tasks are brokered by its own member, so they
   live as long as the job, whoever leads; a page takes them from that member directly.
8. **Any number of Macs, any number of pages.** Nothing counts Macs or names "the other one".

Principle 2 as the build stands (on a Mac, in the app's folder, `~/Library/Application
Support/scenic/`: its agent's folder, `agent/`, and the app's versions, `app/`; plan.md §3–4 and
formats.md say more). Of the last
three rows, a job's progress is work in progress, as the principle allows; the lead's own state
is on the lead's disk alone; the coordinator's is on the NAS (§7.5). The rest a new lead has from the NAS, or makes again.

| State | Its truth | On a Mac | A new lead |
| --- | --- | --- | --- |
| The records (manifest, job keys, pending uploads), what's built, the catalogs | the NAS: `state/build/`, the content-named files | copies: the mirror, the pack cache (`agent/cache/base/`), `agent/cache/blobs/`, scenic-build's of the summits and the z8 terrain | reads them there |
| Downloads: the planet, Planetiler's data, the canopy squares, AWS's raw terrain tiles, FABDEM, the leaf types, Overture's boxes, the pageview months, the rail feeds; and what today's build left, imported: the DEM seed | the NAS: `sources/`, each fetched once | copies in its cache, filled from the NAS when missing (room-making empties the cheap ones) | nothing: they fill again |
| The Wikidata and Wikipedia answers of a pass (the items job's, the heritage chain's) | the NAS: `sources/items/<date>/answers.tar.zst`, `heritage.tar.zst` (`pipeline::answers`) | `agent/cache/items/`, `agent/cache/heritage-<date>/`, made one with the NAS's as a run starts and sent as it ends (a stopped run's at its next start; the lead's agent sends what the NAS lacks as it starts) | nothing: its first run takes them |
| What units keep for their later runs | the NAS: `cache/dem-units/`, `cache/scenic-units/` | none (its next unit job moves any there) | nothing |
| Made from the NAS's data and kept: the heritage chain's clip of the filtered planet (22 GB on the M4), the trains' stop pairs | made again from the NAS | its cache | its first runs make them again (the clip: an hour of osmium) |
| The Python steps' environment | the app's lock file (`dem/uv.lock`) | `app/<version>/dem/.venv`, which uv makes from it (from uv's cache, else PyPI) | its first Python step makes it |
| A job's progress: the OSM pass's stages, the buildings scan's parts (Overture's, ~40 GB), a rail-feeds run's zips not yet put, raw tiles not yet packed | the Mac running it, until the job puts its results on the NAS | its scratch folders and cache | the OSM pass's, offered to that Mac alone for a day, behind `state/pool/resume` (§7.2, Resuming); the rest done again |
| The coordinator's: the workers' token, leases, costs, the history, the hand-offs not yet merged, the pause (mirrored to `state/build/pause.json`) | the NAS: `state/coord/` (the token, the state per term, the history per writer), the journal | `agent/coord/` (`journal/`: hand-offs an older app's worker sent, drained into the journal); a member's `agent/outbox/` (its leased tasks) | loads the state a handover hands it, else the newest a term before has (§6.2, §7.5) |
| The lead's own: the map update under way, its retries, the daily jobs' last runs | the lead's disk alone | `agent/map-update.json`, `agent/state.json` | not in the phases yet: the next map update begins afresh, failed jobs may run again at once |

## 3. What the NAS gives us

The design rests on what the share (SMB 3, Synology, macOS clients) does and doesn't promise.
Checked: ✓; to check on this NAS between two Macs before phase 1 relies on it: ◻.

- ✓ **Create-new is atomic** on the server for one client (the terms and the budgets' leases rely on it).
  ◻ Between two Macs: two creates of one name at once, one wins.
- **A rename can be delayed indefinitely:** a Mac put to sleep between writing a temporary file and
  renaming it renames when it wakes, hours later, whatever happened meanwhile.
- **Reads can be stale:** macOS caches attributes and directory listings on SMB shares; a file
  renamed over on one Mac may read as before on the other for a while. ◻ How long. (A handover's
  new lead must read its old lead's last snapshot within the two minutes it has to take up: with
  reads kept two to five minutes the simulator sees most handovers taken back, none lost.)
- ✓ **Renaming over a file another Mac has open fails** (EBUSY on this share: a map-tile job
  failed so on 2026-10-05; crate::whole::rename_over retries, for the records, the keys and the
  heartbeats).
- **No conditional writes and no atomic appends** between clients: "check, then write" is two
  steps, and an append at a cached end of file overwrites.
- ◻ **Exclusive rename** (store::naming's catalog numbering): server-side, or check-then-rename.
- ✓ **Listings are slow:** 3 to 33 s a folder under load. So nothing lists a folder in a loop: the
  current term is found by checking for the next (§6.1), heartbeats are read by member id, and the
  journal is listed off the lead's loop (§7.3).

## 4. Invariants

What any order of events, sleeps and stale reads must keep. Each protocol below is argued against
them, and the simulator (§13) checks them:

1. **One lead per term.** Terms are made with create-new (§6.1): two Macs can't both make term E.
2. **The term never goes back.** A Mac leads term E only while `terms/<E+1>.json` doesn't exist, and
   it checks that before acting on its view of the build (§6.6). Nobody writes a lower term.
3. **No accepted hand-off is lost.** A job's hand-off is in the journal before it's accepted (§7.3),
   the journal is a log nothing deletes while a term may need it, a new lead replays every entry
   its snapshot doesn't name, and a member tells each term's lead of its entries until that lead
   acknowledges them: an entry a stale read kept from a take-up, or a stale lead acknowledged,
   reaches the new lead's records that way (§6.2), or, its member gone, by the lead's listings of
   the journal (§7.3).
4. **Each records snapshot is self-consistent.** The manifest, keys and pending uploads are one file
   per term, written whole: a reader never pairs keys from one version with a manifest of another.
5. **A stale lead's writes are ignored, not refused.** They go to its own term's files, which
   nobody reads once a later term has records of its own (§6.2).

## 5. Roles

- **A member** is any Mac running the agent with the NAS's project folder. Its id (`m-<16 hex>`)
  is made once, kept in its home folder with the Mac's hardware UUID (a copy of the folder on
  another Mac, Migration Assistant's, makes a new id there), and named in everything it writes; its
  host name is a label (renaming a Mac, or macOS adding "-2" after a clash, changes nothing). One
  process of a member runs the pool at a time: it holds the member's lock (crate::pool::MemberLock,
  `pool-<id>.lock` in the folder the agent gives it: the app's folder, above every copy of its home
  on the Mac, and not the temporary folder, which macOS empties of files three days old: the
  agent passes it, crate::agent::pool::Side), and a second can't take it. Its driver checks the lock every
  loop: a lock file removed is taken again while no other process holds it (the lock removes its
  file as it's let go, so a copy of its descriptor a child holds a moment between fork and exec
  blocks no next taker: crate::sys::PathLock); a check that can't
  tell (the folder unreadable a moment) holds that loop's duties; a process another holds it from
  stops for good, saving its state (the hand-offs handed to that loop in it) and leaving the pool.
  A Mac whose member file is lost makes a new id, and keeps of its saved state only its jobs'
  hand-offs not written yet. It:
  - writes its heartbeat, `state/pool/members/<id>.json` (§10);
  - runs job slots (§7) that ask the lead for jobs, run them and hand their results to the journal;
  - brokers its own jobs' tasks (§8), and runs tasks itself in a slot when it has room;
  - answers the pool's API (§9): its own tasks, its status, the worker page;
  - passes its menu's, `scenic`'s and the map's asks (pause, lead) to the lead over HTTP;
  - takes the lead when it's handed it, when the owner has it take over, or by itself when the
    lead stood down (§6).
- **The lead** is the member the current term names. On top of being a member, it:
  - plans (crate::agent::build) and grants jobs to every member's slots (the coordinator);
  - merges the journal into the records, the only writer of its term's records (manifest, keys,
    pending uploads, the raw store's index);
  - publishes catalogs (`catalog/`, `catalog-held/`), removes replaced files (GC), compacts the raw
    tiles' archives, keeps the backups of the owner's folders on the NAS;
  - keeps the build's pause and writes the build's status (§10).

Nothing else distinguishes it. Its slots are members' slots; a member may be stronger than the lead
and take the heaviest work.

## 6. Leadership

### 6.1 Terms

A term is a file made once with create-new and never changed: `state/build/terms/<E>.json`
(crate::pool::term).

```json
{ "term": 12, "member": "m-3f9c…", "host": "Brandons-MacBook-Pro", "app": "20261012-0910-1a2b3c4",
  "since": 1791300000, "how": "handed over by MacBook-Pro-de-Brandon (the menu bar)",
  "from": 11, "seq": 214 }
```

- **The current term is the highest that exists.** A Mac learns it once, at start (`term::current`:
  from the lead's hint, or a listing of `terms/` when the hint names no term there, then checked
  upward), then each step checks only whether `<E+1>.json` exists (`term::next`: a stat, never a
  listing); a lead handing over tells the new lead of its term too (§6.4). Its view never goes back:
  it keeps the highest term it knew across restarts (`driver::Saved`). `state/build/lead.json` is a
  hint the lead writes after taking up its term (for old apps, and to save a listing), never the
  truth.
- **Making term E+1** is how every change of lead happens: a handover (§6.4), a take-back, a
  takeover (§6.5), a re-assertion (§6.6). Exactly one Mac's create succeeds; the others see the
  file: a lead steps down, a member waits. No lock is needed.
- **A handover's term names a snapshot:** `seq`, the number of the records snapshot its old lead
  saved last, as it settled (§6.2, §6.4). The new lead takes up from that one or a later one of the
  same term, never from an older one a stale read gives. Other terms have no `seq`.
- **A term that can't be read whole yet** (its bytes land after its create: it reads empty or short
  meanwhile) has no lead anyone knows: no one leads it, and no term can follow it (`Term::after`
  refuses, its app unknown). Its maker finishes it: a create whose bytes didn't land (the share went
  away between) says so (`nas::Created::Unwritten`), and its maker writes them whole over its own
  file, at once or at a later loop (`term::finish`: no other Mac's create of the name can succeed);
  a try whose answer was lost knows its own term by what it reads (the same but for `since`). A
  maker that stopped for good, or lost its answer before its bytes landed (12 Oct: term 55, three
  days with no lead), leaves the term unreadable for good: a forced takeover follows it
  (`term::forced`), which checks the app rule against the newest term that can be read and says
  so in its `how` ("forced past: term E unreadable"). A member that can lead makes it by itself
  once the term has read unreadable half an hour (§6.5, Taking past a term left empty); else the
  owner's (`scenic lead take --force`), which the alert names (§10).
- **The app rule.** A term records its lead's app. A Mac makes a term only with an app at least as
  new as the term before's ("update first": job keys include the steps' versions, so a lead on an
  older app would take everything a newer one built as stale and build it again). Published apps
  (`20261012-0910-1a2b3c4`: the UTC minute of publishing, then the commit) compare by that minute;
  one that isn't published ("development") follows only another that isn't, and any app follows
  it, so a test lead never keeps the published app from leading (`term::app_at_least`). A
  take-back (§6.4) is checked against the term handed over, not the target's: the target never
  led, so nothing was built as its app would, and a target on a newer app that doesn't take up
  can't leave the build with no lead (`term::back`). The owner may make a lead on an older app or a
  development build (`scenic lead take --force --downgrade`: `term::forced`, its `how` saying "the
  owner's downgrade"). A lead that sees a newer app installed restarts into it (as now) and
  re-asserts; one restarted into an older app stands down (§6.6).
- **The 3D buildings and older apps** (docs/buildings3d.md §3.6): an app from before them doesn't
  know their steps. Leading, it drops their records from `jobs.json` as it saves it (their tiles
  then built again, about an hour of the M4's, once a newer app leads), and refuses a
  member's hand-off of them (not a step it shares). So both Macs run an app with them before the
  lead may move to either, and an owner's downgrade past them costs that rebuild. From that app on,
  the records keep what they don't know (`build::Keys::other`), so a later step's records outlive
  a downgrade.

### 6.2 Records per term

- **One file per term** (crate::pool::records): `state/build/term/<E>/records.json` holds the
  manifest, the keys, the pending uploads, the raw tiles' archives waiting to be named in the raw
  store's index, the journal entries it reflects and those it refused, with why (§7.3), the lease
  that last set each target (lease order across merges: an older lease's entry merged later, a
  member back from sleep telling of it, is named and changes nothing), the day before which the
  journal is forgotten (§7.3), and its number in the term, `seq` (one more each save, the number
  going up when a save fails too: two versions of a snapshot never share one). A handover's last
  snapshot holds the coordinator's state as well (§6.4). The lead writes it whole
  (`Nas::write_whole`: crate::whole's temporary name, renamed over, EBUSY retried) after each merge.
  Today's three files are three renames a reader can see half done (`handoff.rs`); one file
  can't be.
- **Term 1 starts from today's layout** (`state/build/manifest.json`, `jobs.json`, `pending.json`),
  so nothing moves at migration (§12): its first snapshot is made from those files
  (`records::first`, with create-new; its maker's create cut short is written whole, as a term's is)
  before term 1 itself is (`term::bootstrap`), in a file of its own that no lead writes,
  `state/build/term/1/first.json`, and term 1's lead writes them after each of its saves, for old
  readers. Term 1's take-up, and a reader, read its lead's snapshot first, then the first one: its
  maker's write landing late (its Mac asleep between its temporary file and its rename) lands where
  no one reads once term 1's lead has saved. The pool reads today's files only to make that first
  snapshot, and to take term 1 up from them when neither snapshot can be read whole (the first's
  maker stopped midway, or its bytes landed out of order, a hole mid-file): only once it has stayed
  so ten minutes awake (`STALE_S`), the take-up tried again meanwhile, as a handover's is. A stale
  read of a snapshot, or of today's three files (written one after another after each of term 1's
  saves), could pair a manifest of one version with keys of another; by then none is stale, and
  term 1's lead has saved none, so nothing has written them since.
- **Taking up term E+1** (`records::start`, crate::pool::driver): the new lead starts from the
  newest snapshot a read finds (about 3 MB), walking down the terms: E+1's own when an earlier try's
  landed, then E's, then E-1's, and so on (a term whose lead saved none has none). A lead
  re-asserting or taking back uses its own records for its old term, unread; an earlier try whose
  save failed, its records as tried. A handover's term starts only from the snapshot its `seq`
  names, or a later one (a stale read gives an older one: the take-up fails, to be tried again
  shortly). The new lead saves them as `term/<E+1>/records.json` and leads; its members tell it of
  their entries, and it lists the journal off its loop and replays every entry its records don't
  name, in (term, lease) order (§7.3). Its records reflect the journal once that listing is merged
  and saved, every entry it was told of or listed is read, and it owes no re-assertion
  (`caught_up`, with when it asked for the listing, `listed_at`): a catalog, and GC, wait for that
  (an entry not merged yet may hold uploads the records don't name). It lists every day again
  daily, an hour before the last listing is a day old, and is caught up only by such a listing
  asked for under a day ago: an entry written late into a day older than the sweeps reach, by a
  member gone before telling of it, waits a day at most.
- **Acknowledgements are per term** (`journal::Mine`). An entry term E's lead acknowledged can be
  missing from term E+1's records: the take-up listed the journal stale, or term E's lead, not yet
  knowing of term E+1, merged it after that take-up. So each loop a member tells the lead of the
  current term, as it knows it, of every entry of its own that this lead hasn't acknowledged, and
  after a change of lead tells the new one of them all; an entry keeps the highest term whose lead
  acknowledged it, so an older lead's late answer counts for nothing. A lead acknowledges an entry
  once a saved snapshot of its term names it, applied or refused (at once, if its records do), or
  when it's of a day its records forgot and not in the journal (merged before GC removed it).
- **Readers** (jobs, the map server, catalogs) read the current term's snapshot, or while it has
  none yet (its lead taking it up, or gone before saving one) the newest of a term before it
  (`Records::newest`). Content names never change, so a slightly old snapshot is fine for a job's
  inputs.
- **What a stale lead does** after a later term exists: its renames land in `term/<E>/`, which no
  one reads once a later term has records of its own; its journal reads and duties stop at its next
  check (§6.6). A stale lead's late rename can't land over the new lead's records: there's no
  shared file to rename over, each term having its own.
- **The coordinator's state** is per term too: one file, `state/coord/term/<E>/state.json`
  (crate::coord::PoolState: the leases, what each target cost, the memory floors and disks
  learned, the workers' failures, the pause), written whole by the lead of E at each loop of its
  agent's that changed it (crate::agent's `pool_lead_files`). A lead taking up a term loads one
  (crate::agent::pool's `took_up`, §7.5): a handover's new lead the state the old lead wrote
  settling, from the snapshot its term names (§6.4); any other the newest `state.json` of a term
  before its own; a lead re-asserting keeps its coordinator's as it is.

### 6.3 Asking

Any member asks the lead. Its menu ("Hand the Build To ▸", "Make This Mac Lead"), `scenic lead` and
the map's build panel ask their own Mac's agent (an ask file in its folder, `lead-request.json`,
crate::control::LeadRequest; the map through its server's `/api/build/lead`), the worker page the
agent of the Mac serving it (its coordinator's `/work/lead`); the pool's next step takes the ask up
(crate::agent::lead), within half a minute: the agent's loop's step, or while that loop is busy
(a long plan, a job being stopped) its pool's own thread's (§6.7). It checks it as the driver's step would (`Driver::hand_to`,
`Driver::takeover`): one refused never reaches the driver, and its status says why. On the lead the
driver hands over (§6.4); on another member it passes the ask on to the lead by mail
(`Msg::HandTo`, §9: the pool's HTTP API, `POST /pool/lead`, is planned), and the lead checks it
again. A member that can't reach the lead can't hand anything over; it can only take over (§6.5).
An ask naming a Mac that isn't a live member (heartbeat beat within ten minutes, app new enough:
`Driver::hand_to`; the NAS reachable, planned, the heartbeat saying nothing of it yet), or one whose
heartbeat's conditions say it can't lead now (away from home, on battery under 30 %, short of the
reserve on its disk: `Conds::able`, which its own driver answers an offer by; crate::agent::pool's
`Side::hand_to`), is refused, saying why. Where an ask stands (refused, passed on, under way, done, came to nothing, with why)
is in the asking Mac's status, kept across its agent's restarts (`agent/pool/lead.json`), and every
control shows it.

### 6.4 Handing over (the lead is there): a state machine

States are in the two Macs' heartbeats and the terms; times are each Mac's own wall clock, read
after what they're compared with. The transitions are crate::pool::handover's, pure functions of
what the lead's step saw, driven by crate::pool::driver.

| State | Who writes what | Next |
| --- | --- | --- |
| **Leading** (A, term E) | | an ask to hand to B: **Offered** |
| **Offered** | A's heartbeat: `handing_to: B, offer, since` (the offer known by when it was made) | A reads B's answer to this offer within two minutes of making it (`OFFER_S`: two steps of each Mac's pool, §6.7, with time to spare): **Ready**; else A clears it: **Leading** |
| **Ready** | B checks its disk and app, then its heartbeat: `ready_for: E+1` and the offer it answers (an answer to an earlier offer answers no later one) | **Settling** |
| **Settling** | A stops granting jobs (asks are answered "the lead is moving; ask again in a moment"), keeps renewing the leases out and taking hand-offs into the journal; cancels its duties in flight (a catalog is killed: it writes once at the end; GC stops between folders); writes the coordinator's state, and saves its records with it, merging what's waiting | settled within a minute (`SETTLE_S`, A's clock) of the step that read B's answer, B's heartbeat as A reads it then still answering this offer (the same answer read again counts: an answer names its offer, and none answers a later one): **Passed**; else **Leading** (a settle that ends later passes nothing) |
| **Passed** | A makes `terms/<E+1>.json` naming B, with `seq`: the number of the snapshot it saved settling (§6.1), and tells B of it. A is now a member; its own member API sends lead asks on to B | B takes up within 2 min (B tells A it leads, or A reads it in B's heartbeat, or E+1 has a snapshot): **Leading** (B, E+1); else **Taken back** |
| **Taken back** | A makes `terms/<E+2>.json` naming itself ("B didn't take up"), the app rule checked against term E (§6.1) | **Leading** (A, E+2) |

- **The agent's parts** (§12, built): settling stops grants (its coordinator answers asks "the
  lead is moving; ask again in a moment"), stops a catalog or a sweep in flight, and hands the
  coordinator's state to the step; the transitions themselves are the driver's. The pool's own
  thread (§6.7) settles too while the agent's loop is busy, but only with no duty running or about
  to start and no job starting (else that loop's next step stops the duty and settles); it leaves
  the coordinator's start and stop, and the state a term taken up loads, to that loop's next step,
  the coordinator granting nothing meanwhile. The owner's asks
  reach it (§6.3), so a handover starts only when someone asks, or by itself when the owner turned
  the proactive offer's switch on (§6.5).
- **Nothing running stops,** on any Mac: leases keep their ids (`<term>-<n>`, unique by
  construction) and deadlines (wall-clock times) in the state B loads, as A wrote it last (it's in
  the snapshot the term's `seq` names: crate::pool::records::Records::handed), so every job, A's
  included, renews with B and hands back to the journal as before.
- **A job that can't move** (the OSM pass, whose stages are on its Mac's disk) doesn't stop a
  handover: it's a member's job like any other, wherever it runs, the lead's own among them: with
  the part changing in its process (`state/pool/slots`, §12, phase 4's fourth batch), a lead handing
  over mid-job keeps its job running and works on as a member, the job's lease renewed with the new
  lead; without it, it restarts into a member once its job's slot is free.
- **A lead restarting** (a new app) is a re-assertion (§6.6): it makes the next term naming itself
  and keeps the leases of the jobs it relaunches.
- **"No lead"** is a state the views show (a term whose lead is out of touch, or a pass not taken up
  and not taken back), with a one-click "Take it" (§6.5).

### 6.5 Taking over (the lead is gone)

When the lead is out of touch (its heartbeat's own beat more than ten minutes old by the reader's
clock; or, planned, it says it can't reach the NAS), or its heartbeat says it stood down from its
term (§6.6), the owner may have another member take the lead (a lead out of touch whose
coordinator answered this Mac in the last ten minutes is late, not gone: §6.7, and needs
`--force`): "Take Over the Build…" on its menu,
which says since when the lead's been gone and asks to confirm, the worker page's "Take it", or
`scenic lead take`. The member makes `terms/<E+1>.json` naming itself and takes up as in §6.2.
`--force` does it without the out-of-touch check (the owner knows the lead is off), and past a term
that can't be read whole (§6.1); `--downgrade` on an app older than the term's (§6.1), the lead that
stood down included.

What the old lead had in flight: its jobs' hand-offs are in the journal and get replayed; leases
its slots held lapse after their ten minutes and go back out (a lapsed lease's hand-off that comes
in later is still taken if its targets weren't leased again since); its coordinator's last changes
(costs, failures of the last minutes) may be lost; no built work is.

Automatic takeover of a lead that's gone or asleep is left out for now (the owner asked for buttons;
a lead that's only asleep would lose the role every night, and it may come back). In its place,
**a proactive offer:** when the lead's heartbeat says it's away from home or on battery while
another member's says it's home on power and able to lead, and the lead can be handed to it, every
member's menu, the worker page and `scenic lead` offer "Hand the Build to <Mac>" in one click
(crate::agent::lead::offer; the first such Mac by name). Automatic when the owner turns it on
(`scenic lead auto on`: `state/pool/auto-handover` on the NAS, off while missing): the lead hands
over by itself once the same offer has stood five minutes, never while a handover or an ask is under
way, nor within half an hour of the last change of lead or of its last automatic ask, so the lead
can't flap between two Macs on the edge of their conditions (`auto`).

**A lead that stood down is taken over automatically** (§6.6): awake, it has declared it won't lead.
A member that can lead now (`Heard::able`: its disk, home and power) and has seen its heartbeat
stood down for two minutes takes over by itself, if the app rule lets it (crate::pool::driver:
`STOOD_DOWN_S`): the members that can, the newest app first and
then the lowest member id, try 30 s apart, and the next term's create-new decides between them. Its
`how` says so ("taken over by MacBook-Air: Mac-mini stood down"). When no member's app can lead the
term, it waits for the owner (an update, or `--downgrade`).

**Taking past a term left empty, by itself** (crate::pool::driver: `UNREAD_TAKE_S`). A member that
can lead now, every other member heard from (below), whose reads have found the current term E's file there, answered by the share, and too
short to be a term (`term::short`: empty or cut short) at every step for half an hour awake, makes
term E+1 past it as the owner's force would (`term::forced`: the app rule against the newest term
that can be read; never `--downgrade`), the members in touch that can taking turns as above,
`AUTO_RANK_S` apart, and the create-new deciding. Its `how` says so ("taken over by MacBook-Air:
term 55 unreadable for 30 min (forced past: term 55 unreadable)"). The half hour starts again
whenever a read of E (or the step's learning of the term) fails rather than answers short: a share
that doesn't answer says nothing of the term. A Mac that knows it made E and couldn't fill it
(`Saved::unfinished`) doesn't take it past: it finishes E, unless E+1 was made meanwhile (then it
drops E unfinished: `term::finish` refuses). Why this can't take the lead from a lead of E, nor make
two:

- **Every other member must be heard from, and none lead E; it fails closed**
  (`clear_to_take_past`): at every step it counts, and again just before, each other member it
  knows must have a heartbeat that reads whole, in touch, not leading E (a lead of E reads E whole,
  whatever this Mac's reads find). A read that fails, a heartbeat missing, not whole or out of
  touch, and the take waits, its half hour starting again. So does a member list with no other
  member (a listing that failed may leave only itself), and this Mac's own heartbeat on the NAS
  older than `OWN_BEAT_S` (six minutes: its writes not landing, its view an outage's). So a
  member that's gone (its heartbeat out of touch, or unreadable) leaves an empty term to the owner:
  `scenic lead take --force`, which the alert names after an hour (§10).
- **E is read again just before:** read whole then (or the read fails), and it waits.
- **E+1 has one maker:** its create-new succeeds for one Mac (invariant 1), as every term's.
- **A lead of E that no heartbeat showed steps down before acting:** every lead checks that its
  term's next doesn't exist before it acts (`term::next`). So were E's bytes to land late (its maker
  stalled mid-write, then finishing it), or a lead of E be out of touch, it would find E+1 and step
  down, as any lead past whose term another was made. This is what bounds the rest: how long the
  share keeps a read stale isn't checked (`STALE_S` is an assumption, plan §10), and the NAS's
  operations give no file times to check E's age by; the half hour of short reads is a margin, not
  a proof.
- **What it costs is what any takeover costs** (above): a lead of E out of touch loses what it had
  in flight; none of it built is lost.

What isn't done, not being provably safe: a Mac finishing (writing whole) an empty term its create's
answer was lost for. A record kept before the create can't prove it made the file: another Mac's
create of the same name may have won and be writing its bytes, and two contents written over one
term would be two leads of it. Nor is a lead that's only out of touch taken over by itself (above:
it may be asleep, and come back). Both stay the owner's, and the alert (§10) says so after an hour.

### 6.6 Re-asserting, and stepping down

A lead's view can be old without its knowing: it slept, its clock was set, or the NAS didn't answer
for a while. So the lead **re-asserts** before it acts again (crate::pool::driver): it makes
`terms/<E+1>.json` naming itself, a create-new no stale read can fool, and takes it up from its own
records (one copy of its snapshot, about 3 MB). It does when, since its last loop or within it, its
wall clock and its awake clock moved more than a minute apart (the awake clock, Rust's `Instant`,
doesn't count sleep: it slept, or its clock was set, either way); when its last step ran over five
minutes awake, or five minutes awake passed since it (`STALL_S`: the NAS stalled, or its process
was held up; the pool is stepped every 20 s or so whatever the agent does, §6.7, and on a share
under load every operation can take seconds, so a step's length short of that says nothing);
after a restart; and before every GC sweep. Time spent listing the journal (done off the steps)
isn't a gap. It keeps what it knew of the
journal (no other lead came between), its refusals not noted yet with it, so the loop that
re-asserts before a sweep is caught up (§6.2); after a sleep, the messages members sent it meanwhile
lost, it lists the journal again. A member whose saved state is lost, or another's (this Mac's
member file lost and a new id made; or a copy of another Mac's folder), re-asserts a term naming it
that it finds at its start, rather than take it up again: it may have led it, its leases granted. So
does one whose saved state is older than a term naming it (a backup restored): one it made that
`Saved::made`, the newest term it made, doesn't name, or one whose records are there already,
another process of this member having taken it up (a handover's too). If the term it makes already
exists, someone took over: it **steps down** at once.

A lead restarted into an older app or a development build can't re-assert (the app rule): it
**stands down**, and its heartbeat says so (`stood_down`: its term, which no one then leads), so
another member takes over, by itself after two minutes, or at the owner's ask unforced (§6.5);
once its app is new enough it re-asserts its term. A member named by a term made on a newer app
than it now runs does the same. A lead handing over that restarts into an older app can't take the
lead back when its target doesn't take up (the app rule): it drops the handover, and the term waits
for its target or a takeover; the owner's forced takeover drops a handover waiting too.

Every step it also checks `<E+1>.json`. Stepping down: it stops granting, planning, merging and its
duties, writes nothing more, and goes on as a member: its slots carry on (a lease the new lead knows
is renewed; one that lapsed is stopped, its done targets handed off). The history and its menu say
so: "No longer leading term 4: a later term exists; term 5 is MacBook-Pro-de-Brandon's (taken over
by MacBook-Pro-de-Brandon)", with when.

A write already past its last check when the Mac froze lands in its own term's files (§6.2), which
nobody reads: invariant 5.

### 6.7 Clocks and wakefulness

- **No NAS clock.** A file's mtime isn't one: a client sets the mtime it writes. Whether a member
  is out of touch compares its heartbeat's own
  `beat` time with the reader's clock, as the members' statuses are read; a member whose beat is over 60 s
  ahead is shown as "clock wrong". The Macs keep time by NTP to well under a second.
- **Nothing persisted as an `Instant`:** lease deadlines, failures' waits and workers' last-seen
  times are wall-clock times on the NAS, so they mean the same on the next lead.
- **The pool is stepped whatever the agent is doing** (crate::agent::pool's `Ticker`): the agent's
  loop steps it about every 20 s while its steps are short, and the pool's own thread steps it
  whenever that loop hasn't for 25 s (`QUIET_S`), every 20 s (`TICK_S`) until it does: a plan's
  minutes, a job being stopped, a share slow to answer hold up no heartbeat, offer, answer, settle,
  ask or view. One step at a time, under the run's lock; the agent's jobs, its coordinator's start
  and stop and the state a term taken up loads stay its loop's. So a heartbeat goes out of touch
  only when its process, or its writes to the NAS, stop.
- **Late, not gone:** a lead whose heartbeat is out of touch while its coordinator answered this
  Mac within the same ten minutes (crate::coord::client's `answered`: a member's requests to it,
  counted only when the answer says it's the current term's, from that term's lead: every answer
  of a lead's coordinator carries `x-pool-term` and `x-pool-member`) is shown as late ("its
  heartbeat is late, but its coordinator answered this Mac …"), not as "no lead", and an unforced
  takeover is refused, saying so (crate::agent::lead's `ANSWERED_S`). The hour's alert (§10)
  counts all the same, naming `--force`.
- **The agent's loop never waits on the pool's thread:** a tick holds the run across the share's
  operations, which may hang; the loop reads what it needs then without the lock (the member's id,
  the run as its last pool step left it), passes its own pool step over (granting and starting
  nothing on its behalf), keeps its jobs' hand-offs for the next, and at shutdown waits a minute at
  most for the tick. A tick that has held the run past five minutes (`STALL_S`) is taken for hung
  on the share: a lead's coordinator grants nothing ("its pool step hangs") and its contact comes
  off the NAS, and the status says the pool's step has hung, the NAS not answering it (also after
  three steps of the loop passed over); all until a pool step runs again.
- **The lead stays awake** while it leads with leases out, at any charge (an idle-sleep assertion,
  `caffeinate -i` held for the agent, as jobs hold one), so no member stalls behind a lead that
  dozed off with no job of its own (with the part changing in its process: §12, phase 4's fourth
  batch).

### 6.8 Reaching the lead

A lead whose coordinator the members can't reach (a Mac's application firewall dropping incoming
connections to `scenic`: found live in #191, the M1's) grants them nothing: their asks, beats and
hand-backs time out and their leases lapse. So the members say so, and the lead moves off it
(crate::pool::reachable; crate::agent::pool's `Run::tend_reach`; crate::agent::lead's `away`):

- **Tried, and counted.** Every member tries the current lead's coordinator (its broker's addresses
  in its heartbeat, else the contact on the NAS) and the other members' brokers in touch (§8) every
  `PROBE_S` (60 s), each a GET with a 3 s timeout on a thread of its own: any HTTP answer is
  reached. Its client's own requests to the lead count too: an answer from the current term's lead
  is reached; a request that failed to connect to any of its addresses (resolving, or the
  connection timed out or refused; not one that ran past its time once connected), since this Mac
  first saw the term by its own clock, is not.
- **One wait a loop.** A member's calls to the lead (its pause, beats, jobs' ends, hand-backs,
  asks, inputs) go through one client, in rounds of its agent's loop: once a request gets no answer
  (each of the lead's addresses failed to connect or closed the connection unanswered, or the
  answer broke off before it was whole), the loop's later requests to it fail at once, saying so,
  rather than each waiting out its timeout; the next loop tries again, so a lead back is heard
  within a loop (`Client::new_round`). Any HTTP answer, a refused token's included, is an answer,
  and a connected lead's request that ran past its time doesn't fail the round. A job's end the
  lead didn't take is owed, and told again each loop until its lease has lapsed (`OWED_S`), not
  dropped.
- **Reported in the heartbeat** (`Beat::reach`): whom it reached within `REACHED_S` (5 min), and
  whom it failed to reach across `REPORT_S` (2 min, three tries) with no success between, with the
  cause (connection timed out, refused, no such name) and the error. The first success clears it.
  Times are the reporter's clock, compared only with its own heartbeat's.
- **Not handed the lead.** A member that another member in touch reports unreachable (an older app's shadow
  runs' heartbeats left out; the heartbeats as this Mac last read them, once a round, not read again
  per check) can't be
  handed the lead (`Driver::hand_to`, so the controls, `scenic lead give`, the update's handover
  and the proactive offer all refuse it, saying "unreachable from m4: connection timed out (…)").
- **Leading, it hands the lead away**, by itself, when at least half the other members in touch
  (one at least) report it, one for `AWAY_S` (10 min) or more; nothing under way; its term
  `AWAY_S` old or more and its last such ask `CALM_S` (30 min) ago or more. To a member that can
  lead, that another member reached within `REACHED_S` (the lead itself counts: its own tries of
  the members' brokers are enough) and none reports unreachable, home on power
  first, then by host; with none such (no member's broker tried: brokers off), it keeps the lead and
  the views say why the members get no work. A target must be reached to be handed the lead, and
  an unreachable Mac can't be handed it back, so the lead can't flap between them.
- **Said:** each view (`View::unreached`: `scenic lead`, `scenic status`, the menu bar, the worker
  page, the map's build panel) says "m1's coordinator is unreachable from m4 for 12 min: connection
  timed out (m1 asleep, off the network, or its firewall blocking Scenic: allow Scenic in System
  Settings › Network › Firewall there): m4 gets no work while m1 leads" (refused: its agent stopped
  or restarting, or its firewall refusing Scenic), and what the owner does; a member's waiting line says
  the cause, naming the lead, rather than that no Mac is coordinating.
- **What the owner does:** on the Mac named, check it's awake, on the network and its agent
  running; then System Settings › Network › Firewall › Options…: allow
  incoming connections for `scenic` (or turn the firewall off). The installed `scenic` is signed ad
  hoc by the linker (identifier `scenic-<hash>`, no certificate) at a new path per version
  (`app/<version>/scenic`), and the firewall keys an ad-hoc signed program's allow by its code
  hash, so an allow lasts until the next update; neither a stable path nor a stable ad-hoc
  identifier changes that. One that survives updates needs a signing identity: a code-signing
  certificate in the publishing Mac's keychain that publish.sh signs `scenic` with: not built (the
  owner's to make; publish.sh signs only the menu bar's app, ad hoc).

## 7. The work: slots on every member

### 7.1 Slots and asks

- **Slots.** Each member runs as many job slots as its resources allow (two on a Mac with 32 GB or
  more, one on less; the second only beside jobs it can run with: the steps table's, §7.2), and a slot
  takes tasks (§8) when no job fits it.
- **An ask says what the slot can take now:** the memory it spares (its Mac's free memory less what
  its other slots' jobs are predicted to take), its disk's free space, its cores, whether its Mac is
  at home, on power, in use, its app and its member id. The lead answers with a job: a step's
  targets with their keys, in plan order, that fit; to a slot on another app than its own, older or
  newer, nothing, saying why, until the two run the same (a result made by other code than the key
  says would be recorded as current: plan.md §8, The same app). Its jobs under way go on; a lead on
  the older app updates, or the owner hands the lead to a Mac on the newer (§6.1), so no member waits
  for good.

### 7.2 Placement: the steps table

One table, the steps table (crate::agent::steps, built: phase 4's first batch, §12), has a row per
step the agent runs as a job. The step sets the agent and the coordinator test (the shared steps,
the second job's ranks, those keeping the pass's answers: `SHARED`, `SECOND`, `ANSWERED`) are made
from it when the app is built, and a step's first guess of memory and its batch are read from its
row; what depends on a target rather than its step (the memory a unit's piece or a 3D buildings
tile is offered with) stays with the agent.
Its columns:

| Column | Meaning |
| --- | --- |
| memory | a first guess per step until a target's own run says (predicted per target from its last run, crate::coord::Cost; units and candidates by their piece's size, terrain by its area's, the 3D buildings by their rows). No column for disk (a job starts with the reserve, 30 GB, and its measured disk free: §7.7; a member asks only for what its free disk fits, crate::agent's `helper_need`: 15 GB, a task 1–5 GB), power or home (no job waits on them: §7.7) |
| shared | whether other members take its jobs, and its rank of preference among those that are (what later steps wait on first) |
| alone / beside | whether it's a run of the whole build, the lead's alone (not offered to members: crate::agent::every); a Mac's second job's rank of it in the forecast (`SECOND`) |
| answered | it keeps the pass's Wikidata and Wikipedia answers (the lead's alone; none starts while the agent sends them) |
| batch | how many targets a job of the lead's own takes (a few minutes to a quarter of an hour of work; a member's by measured times: §7.7) |
| write-set | the logical names its jobs may add, change or remove in the manifest, as patterns of its targets' tiles and the pass's date, removals apart (the OSM pass removes older passes' entries; a prune only removes); a shared step's files for each target it did are kept apart too (`saves`: what its hand-off may change) |

The table only gathered constants the agent had; nothing new is built on its figures. Its disk
and groups columns went with phase 4's thirteenth batch, replaced by measures and locks where the
resources are used (§7.7, §12); what it holds is a first guess, the code's own sets and the
write-sets. Planned columns it once listed (a floor of memory, moves, groups across the pool)
aren't added to it.

Tree cover's rows (plan.md §6, Trees): `trees`, a piece per z6 tile (it may add, change or remove
that tile's hi packs of the three tree layers and its mid, `work/trees-mid/6-x-y`; a lease of the
scheme before pieces, a z3 tile's whole run, its lo pack and its z6 tiles' hi packs, for one
release), any member, four a job; `trees-lo`, an assembly per z3 tile (its lo packs of the three),
seconds, any member. Terrain's and slope's likewise (plan.md §6, Global-source layers):
`terrain` and `slope`, a piece per z6 tile (it may add, change or remove its layer's hi pack and its
mid, `work/terrain-mid/6-x-y` or `work/slope-mid/6-x-y`, a piece of a z6 tile the coverage has left
removing them; a lease of the scheme before, an area's whole run, its lo pack and its z6 tiles' hi
packs, for one release), any member, eight a job; `terrain-lo` and `slope-lo`, an assembly per z3
tile (it may add, change or remove its lo pack), any member. Their targets may lie outside the coverage (what it has left).

The 3D buildings' rows (docs/buildings3d.md §3.6): `bldprep`, per z6 tile (it may add, change or
remove `work/bld/6-x-y`), memory learned (0.3 GB and 160 B a row read until then), up to ~3 GB
of the NAS's parquet read a tile, any member, eight a job; `bldtiles`, per z6 tile
(`layers/buildings/hi/6-x-y`), memory learned, any member, sixteen a job. `bld-fetch` (the sources, onto the NAS, outside the manifest: its write-set is empty) is
network work, the lead's.

The input check's row (docs/inputs.md §4.3): `inputs`, a target per unit (`inputs/<unit>`), its
write-set the unit's accepted index, last listing and held report (`sources/inputs/<unit>/@index`,
`…/@listed`, `…/@held`; their files, the copies they list, are outside the manifest), kept to the
lead's slots (other members taking it is planned: the coordinator's offers are made with the pass's
region work), planned before every other job, and the second job's first choice in the forecast (`beside`
rank 0); it starts beside another job when both jobs' measures fit (§7.7).

What's planned of placement (phase 4, §12):

- **Every step offered to every member** (crate::agent::every and crate::coord's `pick_every`,
  built: phase 4's ninth batch, always on since its thirteenth, §12): the plan's every step but
  the lead's own (`every::LEAD_ONLY`: the catalogs, a prune, the 3D buildings' sources' fetch; the
  steps keeping the pass's answers; and the worldwide steps, a target the world's: the road index,
  the rail stops, the ferries, the world's roots, the landmarks' and overlays' draws, the rail
  chain, the terrain's water), so the targets by area: the map tiles' `pack` and `lo`, the
  assemblies (`terrain-lo`, `slope-lo`, `trees-lo`, `bldfield-lo`) and the shared steps, to a member
  whose measured room fits each target's measures (memory against what it spares, disk against its
  free space less the reserve and the owner's target, its ask's `disk_mb`); a cold start only while
  no job of its step is under way (a lease held, the lead's own jobs' too), to the member with the
  most room, the lead's own counted (its guard's limit less what its jobs hold, its free disk less
  the same reserve and target; none while its slots are full or a job runs alone there), alone.
  The memory compared isn't alike: the lead's is its guard's limit, a member's what it spares (three
  eighths of its Mac's, the owner's choice), so a lead with room takes cold starts before a member
  of the same Mac's size would. `pack` and `lo` are a map update's steps: granted only with the map update's
  units (`as_of`, told to the coordinator with the plan's offers, cleared as the map update ends), which
  their job reads as the lead's own does; with no map update under way, none is granted. The lead's
  plan leaves out what other workers' leases hold.
- **Order and locality** (built with it): a member gets work as a contiguous run of the plan's
  targets (a target held, done, kept from it or past its room ends the run), from the plan's front,
  or just after its last target of the step when that one's open; as many targets as fit a quarter
  of an hour by their measured times (`coord::JOB_S`: their cost lines, a target's own or its
  step's 90th percentile; none measured, one a job).
- **A map update shared** (crate::agent::publish, built behind `state/pool/update-share`, task #203;
  plan.md §8, Faster publishing): the lead's own job of a
  map update's map tiles (`pack`, `lo`) takes an even share of them (`publish::lead_share`: the targets
  over the other agents that asked for that step in the last half minute, `Coordinator::members_asking`,
  plus one; at most the step's batch), from the front of the plan, so the rest stay offered to the
  members with the map update's units rather than going into the lead's batch before a member asks.
  Off: the step's batch, as before. (Of the five switches, `delta-catalog` goes on only once every
  Mac's server runs an app that reads delta catalogs: plan.md §8.) **Scratch state (§13): both Macs, run in part** (15 Oct 2026,
  `root`, `map-update`, `every` and the five switches of plan.md §8's faster publishing, the M4 leading,
  the M1 a member): the switches read (`agent: faster publishing: …`), the map update begun at once (its
  region the last), its slope and tree cover pieces, then `pack` (a cold start, one tile, then the
  other three in one job) and `lo` granted to the M1 with the map update's units. The lead's own share
  wasn't seen: its fake pass-sets job, run alone and never satisfied, held it throughout, so the
  roots and the catalog (the lead's) never ran there; the share, the delta catalog and the low
  zooms' map updates are shown by the tests.
- **How crucial a job is** (crate::agent::placement, built: phase 4's eleventh batch, behind
  `state/pool/placement`, §12) decides who takes it first, not plan order alone: a member's ask
  is tried on the steps by their slack in the forecast (the long poles first), within a step from
  the target measured to take longest, and the members' asks and the pages take the work on a
  sliding scale of their measured speed, as a weighted draw (a function of the target, those that
  could take it and the minute's window, so a decision repeats), not by a rule singling out the
  fastest or slowest; no rule names a Mac or a kind of job. A task offered ahead (a terrain job's
  next piece's subtrees, a tree cover row the run comes to late) goes to a worker slower than this
  Mac only while its head start covers that worker's run. The lead's own jobs take the plan in
  its order (planned: the lead's slots placed the same way).
- **A Mac shared by measure** (built: phase 4's fifth batch, always on since its thirteenth, §12):
  a job starts beside another only if both jobs' measured memory and disk fit what the Mac has
  left; a cold start runs alone (§7.7); a job waiting on a worker holding its unit's tail counts
  as the memory it holds now, its cores idle (§7.7, A job waiting on a worker). No list or group of steps keeps two jobs apart: the raw
  tiles' accessor, the budgets' proxy and the answers' lease guard the resources where they're
  used (§7.7).
- **Resuming** (crate::agent::resume, built behind `state/pool/resume`: phase 4's eighth batch,
  §12). A job interrupted with progress on its Mac's disk is offered to that Mac only until the
  owner releases it (`scenic pool release <member> "<job>"`), or for 24 hours from when its
  progress began there (its first start: a job that fails or stops again and again there holds
  its Mac no longer). Which Mac holds which progress is on the NAS, so a new lead knows:
  `state/pool/progress/<member>.json`, one file a member written by it alone (principle 3: one
  file a step would be written by every Mac that ran it), only as a job starts or ends; the
  release a create-new file beside it. The lead reads its members' files by id as it plans: a
  member's live progress means the lead doesn't run the job, and its coordinator grants it to that
  member's agent alone, which asks for it only while its own file holds it; while a lease of the
  lead's coordinator holds the job, the lead doesn't run it whatever the files say (released, the
  day out, or read stale). The lead's own live progress comes before a member's (it runs the job
  itself). A member in the pool but asleep, away or with its agent stopped holds the job up to the
  day (the status says so; a release is the way to place it now). A member's progress not finished
  and noted within two leases' time to live (20 minutes) may be its job running under a lease the
  next lead's restored state lacks (granted less than a loop before the lead was lost): the lead
  doesn't run the job itself meanwhile, whatever the day or a release says; a running job's
  progress is noted again every five minutes (its `at`, not its `since`). A member notes a job's
  start only once its own file has been read, so a failed read never begins its day anew. One finished there is held,
  unoffered, until the records hold it, and if its entry is refused, until the day is out. Built
  for the OSM pass (its stages' markers); a terrain area's raw tiles and the other progress in §2's
  table are done again elsewhere (planned).
- **The forecast for equal machines** (crate::agent::forecast::equal_machines, built: phase 4's
  twelfth batch, §12; reports only): in the pool, the lead's forecast carries a second one
  (`equal`) run through on each member's slots as lanes (the lead's first slot first, which alone
  makes the map updates; every other slot takes any work that fits its memory in the lead's plan
  order), the pages as one lane (the shared steps alone, their measured speeds summed), each at its
  speed measured against the lead (placement::measured_speeds; one not measured, the lead's), busy
  with its lease's targets left. Nothing is placed or decided by it: the slack placement reads, and
  everything else, is the forecast's as before. Its simplifications: the pages' lane has the
  largest memory a page said (6 GB when none said) at their speeds summed, a page's
  leases its lane's; a worker saying no memory has 6 GB; the lead's second job's lane has
  a quarter of the Mac's memory, as in the forecast; and an equal lane may build a region's slope
  and tree cover before its map update is due (every step to every member), where the lead's
  forecast waits for it.

**A job waiting on a worker** (built on the lead's own two slots only: crate::offload's
`WAITING_ENV`, the agent's `waiting_on`; not on a member, which runs no second job and plans only
while idle).
A unit job whose last unit's tail a worker holds (a page, or another Mac), the job waiting on it
because that worker's measured pace beats this Mac's (`offload::Patience`; a slower one is raced
at once), keeps a small file saying so while it waits (`SCENIC_WAITING`, named by the agent per
slot as `SCENIC_WRITTEN` is: its process, the worker, the task, since when), written as the wait
begins and removed as it ends, whichever way. The agent takes the file as stale when its process
is gone or isn't of the slot's job's group. While it's there, placing a second job beside it (the
beside rule above) counts the waiting job's memory as it holds now (the sampler's), not its
target's measure, and its cores as idle: the job beside starts with all the cores, not half. It's
the existing second slot (`start_second`), no new kind of job; the waiting job stays its slot's
until its tail is back and it commits the unit (or races it, past its patience). The job started
beside it **yields** (tied to that job's id: once it ends, nothing yields to it): when the first
goes on and the two hold more than the limit together, or the Mac is short of memory, the job
beside is stopped at once, the larger or not, no safe point waited for (a drain may take 15
minutes, the Mac swapping meanwhile); it's given back as an interrupted job is, not kept from this
Mac, and what it held is learned as no floor (it was placed by what the first held while it
waited, not by anything it did). The status says the job waits on the worker (`waiting`,
beside its progress), and the forecast lists it per machine (`waiting`).

**A job far over its memory** (crate::agent::memguard, built: phase 4's second batch, §12). macOS
swaps rather than kills, so a job holding more than its Mac has (terrain's whole-area run on the
Alaska coast held 32.9 GB) leaves the Mac swapping for as long as it runs, every other job and the
owner's own work with it. Every agent, the lead's and each member's, acts on what its jobs hold, by
measure alone: no prediction, and nothing from the steps table, decides it.

- **Sampled on a thread of its own,** every 5 s, whatever the agent's loop waits on (a share that
  stalls holds the loop, not the sampler): each running job's processes' physical footprints summed
  (crate::sys::footprint_of_group), kept per target as the most the job held while that target was
  under way. Which target is under way the job says itself: a step that notes its targets' costs
  (`SCENIC_COSTS`) notes a `started` line as each begins, beside the line it notes as each ends; a
  job of a step that notes none is on the first of its targets it hasn't noted done
  (`SCENIC_DONE`), and the OSM pass (no targets of its own) on its pass. A job's costs file starts
  afresh with it, so no line an earlier job left names a target that isn't under way.
- **A limit per Mac:** its memory less an eighth for macOS and the owner's work, 4 GB at least (42
  GB of the M4's 48, 12 of the M1's 16); a Mac whose memory can't be read guards nothing, the status
  saying why. While its jobs hold more together, by what each holds now: when the largest fits the
  limit alone, the job beside it stops at its next safe point and nothing starts beside the largest
  until it ends; when the largest passes the limit by itself, it stops at once only while the Mac is
  short of memory (the kernel's memory pressure at warning or worse, or a GB more swap than the least
  in use in the last five minutes: the sampler freezes it then, at once, and the agent's loop stops
  it; a Mac whose owner's own apps keep its pressure at warning, as the M4's with ~36 GB of swap,
  stops any job alone past its limit at once, by design), else at its next safe point, and only trouble stops it sooner (a long job of one target, an
  area's whole terrain run, may reach no safe point for hours). A job beside asked to stop at its
  next safe point that reaches none in the time a pause gives (15 minutes), frozen meanwhile by a
  pause or not, stops at once. A sample is kept only for the job it read: a job started in the slot
  meanwhile takes none of it. Each sample is written to the agent's folder (`memory.json`: what each
  slot's job holds, frozen or not, the limit), so `scenic status` shows the latest whatever the
  agent's loop waits on. A job stopped is given back,
  not held against its targets as a failure, its targets noted done kept; it's kept from that Mac
  for an hour (a member's lease ends as failed, so its lead keeps it from that Mac as long, doubling,
  whether or not a floor reaches the lead). The history and the status (`memory`: the switch, the
  limit, what the jobs hold, why it guards nothing if it doesn't, what it last did) say why.
- **Learning, whatever the switch:** what a job held while a target was under way is that target's
  **floor**, the least it takes (crate::coord's floors, `floors.json` beside the costs, and the
  pool's coordinator state across terms), with whether its job held that target alone and the way
  its step ran (`cost_version`). A floor only rises, but one learned alone takes the place of a
  batch's (a batch's caches, filled over its earlier targets, may be counted against its last); a
  run that measures the target itself takes its place, the measure being the same kind of figure (a
  job's processes together, sampled: a unit's too). A target's predicted memory is never below its
  floor, so the coordinator offers it only to a slot sparing that much, and a second job starts
  beside another only if both fit. A member sends its jobs' floors with the lease's end (done, given
  back or failed: `floors`); its asks say its Mac's limit (`limit_mb`).
- **Held where it can't fit:** a target whose floor, learned alone the way its step runs now, passes
  this Mac's limit isn't started here: a shared step's is left to the members whose Macs have room
  ("left to a Mac with room"); one past every Mac's limit the lead knows from the members' asks
  waits, the status saying so ("more than any Mac in the pool has"). A step only the lead runs
  (not shared: the heritage chain, the items job…) is judged against the lead's limit alone: past
  it, the target waits, the status saying "past the lead's limit of X GB; this step runs only on
  the lead" and naming the member (an agent heard from in the last ten minutes) with the largest
  limit when that fits it, with the remedy, `scenic lead give <member>`. The lead isn't handed over
  by itself: moving the lead moves every lead-only job and the pass's caches, a floor learned wrong
  would move it for nothing, and which Mac leads is the owner's call. A
  target whose floor past the lead's limit was a batch's is tried again in a job of its own on the
  lead first; a member's offers keep a floor past what it spares from it, and batch the rest as
  any. A floor
  of an older way of its step holds nothing (it's an estimate until the target runs again).
  `scenic pool floors` lists the floors the lead keeps, `--clear [<step> [<target>]]` clears them
  (a step fixed, or a floor learned wrong).
- **A target never run** is placed as before the guard (a member by the size it's offered with, the
  lead's first slot whatever it holds); if it overruns, the guard stops it and its floor places it
  next time. Nothing in the guard trusts a first guess.
- **The switch:** `state/pool/memory-guard` on the NAS, `off` in it to stop no job, `on` to stop
  them; missing, on (the owner's choice: `memguard::DEFAULT`). Off, nothing is stopped or held, a job
  the sampler froze goes on, and the floors are still learned.

### 7.3 Results: the journal

- **A job's hand-off goes straight to the NAS** (crate::pool::journal): the member keeps it whole in
  its own folder until it's written (`journal::Mine`), writes `state/journal/<day>/<term>-<n>.json`
  itself, whole, by a temporary name (its own file: its lease's; a reader sees all of it or none),
  then tells the lead; one there already that says the same counts as written (an app of another
  version may order its fields otherwise). `<term>-<n>` is the job's lease (the term it was granted
  in, and its number there), `<day>` the UTC day of the hand-off, by its member's clock
  (`YYYY-MM-DD`), fixed by its first try (one written after midnight keeps its key); the entry's
  key, `<day>/<term>-<n>`, is what records name it by. That removes the outbox and its retry loop
  and the HTTP body limit; a takeover finds every result on the NAS; GC sees them all.
- **The lead validates at merge time:** every change in the step's write-set (the steps table's,
  §7.2), every content name matching its logical name, the lease's targets and keys (the merge is
  given the check: `records::Check`). The write-sets are enforced at both ends by one function per
  step (`agent::steps::writes`, its row's): a job's `Out`, opened for its step as `scenic-build`
  starts it (`Out::open`; the merge and tools open `Out::unbound`), puts and removes only names
  that function gives (a put outside fails before it uploads; a set or removal outside fails the
  job's next save, and nothing of that save lands, though the targets it saved before stand), a
  step's sweep of its stale names removing only names it gives (a legacy name under its prefix
  stays), and the merge refuses an entry with a change outside it, or of a step the table doesn't
  know (§7.7, §12 phase 4's sixth batch). So an entry the merge refuses for its write-set is one no
  job of the lead's app could hand off: an older app's, a damaged one, or a newer app's, whose step
  may write a name the lead's app doesn't know of; that one, from a member whose heartbeat says it
  runs a newer app than the lead's (or whose heartbeat can't be read now), is held instead
  (`agent::pool::check_from`, as the coordinator ends such a member's lease rather than hold its
  failure against it): neither merged nor refused, merged again every step until the lead runs
  that app, said in the log as it's first held and in the status (`pool.gates.held`; `scenic pool
  status`), and keeping no catalog back. Every entry in the journal from the pool's first two days
  is within the functions. A refused entry is named in the records with why; once they're
  saved its why is noted beside the journal, `state/journal/rejected/<key>.why`
  (`journal::note_refusal`), for the owner. The entry stays in its day: a refusal may be a lead's
  that's no longer current (its check depends on its state), and a lead whose records don't name the
  entry checks it itself, told of it or listing it.
- **Lease order holds across merges:** an entry of an older lease for targets a newer lease's entry
  set already (a member back from sleep telling of it late) is named and changes nothing.
- **The journal is a log, not a queue:** nothing is removed on the merge path; a records snapshot
  names the entries it reflects; GC (not built: plan §10) removes day folders older than a week once
  every snapshot since names their entries, and the records then forget those days' keys
  (`forget_before`), keeping the day they forget before: an entry of such a day told again, and not
  in the journal, is acknowledged (it was merged before GC removed it). Members forget what a lead
  acknowledged before that day, which the lead's answers carry.
- **The lead lists the journal off its loop** (3 to 33 s a folder): every day not forgotten after it
  takes up its term and then daily, and the last two days every ten minutes, for entries no member
  will tell it of (their member gone, having told only a lead no longer current); one listing at a
  time, none asked for while its last is out, however long it takes (the agent hands every one back,
  making a failed one again). One out two hours with none back meanwhile (the agent lost it, or it's
  that slow) is said, and the next asked for all the same: whichever comes back counts, the newest
  ask's time kept. Otherwise it reads only the entries members tell it of, by key (§6.2:
  acknowledgements are per term). An entry listed or told of that can't be read whole yet (a stale
  read) is kept and read again every loop; one its reads find not whole for an hour awake (cut short
  on the share, or removed) is refused, as a damaged one is, its work done again; so is one whose
  reads fail for an hour while the share answers the loop's other reads (a file the share errors
  on). A read that fails while none answer (the share away) says nothing, and starts the hour again.
  One waiting ten minutes is said, once, naming it and why. A loop reads entries to merge for a
  minute at most (on a share under load a read takes seconds, and a lead taking up from an old
  snapshot may have thousands to read), first those no read has found not whole or failed on, the
  oldest lease first, then the others in turn from where the last loop's reads of them stopped, and
  leaves the rest to the next; a member writes its jobs' entries for a minute at most too. A lead
  with entries left to read settles no handover (§6.4): the handover is given up, to ask for again
  once they're read.
- **Steps with open-ended writes** declare them too: the OSM pass (with `retire_older`'s removals in
  older passes), prune (which forgets keys), the chains, `verify`, patch-ferries. Hand-run steps
  (`scenic-build` from a shell) ask the lead for an ad-hoc lease and hand off the same way.
- **The raw tiles' index:** members only add archives (as every job does); compacting an area's
  archives is a lead duty, or a "replace X and Y with Z" hand-off applied only while X and Y are
  still named. A member making room doesn't pack.
- **summaries.json** is a cache keyed by content: any writer, no fencing needed.

### 7.4 Planning reads only the records

- **Planning, and a pass's being complete, read only the merged records and `inputs/`:** a pass is
  complete when `sources/osm/<date>/pass` is in the records, not when a `pass.*.json` file exists;
  the catalog's `--ready` comes from the merged records, and a catalog runs only when nothing waits
  to be merged.
  Built behind `state/pool/resume` (crate::osmpass::latest_pass_by: the agent's planning, and its
  jobs' by `SCENIC_RESUME`); the catalog's gate is phase 1's (caught up). Off, a `pass.*.json`
  file on the NAS still says it, as before.
- **A job resuming its own work** (built for a resumable step's job, behind the same switch) sees
  its earlier attempts: its agent writes this member's hand-offs for the step that the current
  term's lead hasn't acknowledged (unwritten, or read from the journal by key, a refused one left
  out) into its folder,
  and `Out::open` lays them over the snapshot (as `Keys::load_with` does for done records), so a
  pass restarted after sleep finds its earlier stages before they're merged.

### 7.5 The coordinator across terms

- **Saved on every grant and finish** (as now), not every ten seconds: a grant in a lead's last
  seconds is known to the next. Renewals aren't saved (a loaded lease gets a fresh deadline).
- **Workers are a member id and an agent instance:** a new lead drops only instances that no longer
  exist (today's start drops every lease of its own host, which at take-up would refuse its own
  slots' results).
- **The history** is one file per writer, `state/coord/history/<day>/<member>.jsonl`, merged for
  reading by (time, writer, number): no two Macs append to one file.
- **The contact** (`state/coordinator.json`, for old apps): a lead stopping removes it only if it
  still names its own addresses.
- **Its port, free for the next:** a stop returns with its listener and every connection it took
  closed, and no child holds either. On macOS a socket is made, then marked close-on-exec, and a
  child started between keeps it as long as it lives (the next listen refused, EADDRINUSE); a
  child started by fork and exec holds every socket until its exec. So every child is started
  (`store::spawn`) while no listener or connection is being made or closed, and the other way
  round: one lock, read by the starts, written by the sockets. The map's server's listeners are
  made the same way. clippy.toml's disallowed-methods, checked by tools/app/publish.sh (a hit fails
  the publish), keeps every start, listen and accept in the workspace to it; tests and examples
  that start no coordinator (names', scenic-metrics' test servers) are excepted by an allow.
  A child is started by its program's path (`store::spawn::command`: a name looked up in PATH
  first, the ban covering `Command::new`), and room-making's deletes (`store::cachefile`) wait for
  every start under way: a start that looks a program up (posix_spawnp: a failed exec for each PATH
  folder before the program's) or forks returns while a child still holds copies of this process's
  files and their locks, which made a file no one used look in use.
  Outgoing connections (ureq's) aren't made under the lock: a child holding one keeps a
  connection open, never a port from being listened on.
- **Gone** (phase 4's thirteenth batch): the named writer (`state/build/writer`), `check_writer`'s
  pass and `SCENIC_BUILD_MAC`: once `state/build/terms/` exists `check_writer` refuses every direct
  save, so a step run by hand writes nothing, `scenic-build verify`'s none either (unverified
  uploads wait in the records; a folder with no term yet, a test's or a scratch build's, saves as
  today's files); the claims (leases alone). **Going:** every temporary name not from crate::whole.

### 7.6 The lead's own slots

They call the lead through one interface, re-resolved on every call (crate::agent's `beat` and
`end_lease`, built: phase 4's fourth batch): in process while this Mac leads, over HTTP while it's a
member, whoever granted the lease, so a job running across a handover just carries on. A lease's
worker is its Mac's name whichever coordinator granted it, so the next lead's coordinator, holding
the leases handed over with the term, takes the old lead's renewals and hand-offs as any member's.
A job of the new lead's own process whose lease its coordinator doesn't know (a takeover, its state
older) is held again under a new id, its hand-off keeping the lease it began under; a member's job
whose lease the lead doesn't know is stopped, its done targets handed off, as a lapsed lease's. A
job's tasks (a unit's tail, the 3D buildings' areas, terrain's subtrees, tree cover's rows) are
offered through its own Mac's coordinator: one that stopped (the Mac no longer leading), crashed,
or no longer knows the job's key (the lead taken back, with a new one) answers nothing, and the
job runs each task out itself (crate::offload: as a task no one took), so it carries on. A job's
end reaches the new lead whatever ended it, its client made again when the old lead had none; an
own job ending as its lead wakes from a freeze to a later term, before its step knows, isn't
finished in its own coordinator (a stat of the next term's file first): its end is owed to the lead
and told through its client once the part has changed, for as long as a lease lasts; a job of no
targets of its own (a duty, as pass-sets') ends as done, and the coordinator checks a pooled end only
for being its lease's (its entry is the merge's to check). They use the journal and validation
like anyone's (no shortcut for the lead's own jobs), and the in-process path never holds the coordinator's lock during
NAS I/O. With the part changing in its process (`state/pool/slots`), a member that takes up a term
starts its coordinator (its port free once the last lead in that process has stopped one: a stop returns with its listener closed),
keeping the leases of the jobs it runs (a take-up drops its own host's leases from before only for
jobs no longer running), and a lead that loses its term stops its coordinator and takes its contact
off the NAS, then stops its duties in flight (a catalog, a sweep), their leases ended through the
new lead; neither restarts. A coordinator that can't start leaves the part to a restart, as without
the switch, and isn't tried again in that process. A lead whose update waits on one of its jobs (a newer app installed: it starts
nothing new, its jobs are asked to stop at their next safe points, as on any Mac, and it restarts
once its first slot is free: plan.md §8, A newly installed app) hands the lead, once per app, to a member already
on that app that can lead, as the owner's ask would (the same-app rule otherwise leaves that member
idle meanwhile: plan.md §8).

### 7.7 What a step needs, by structure (memory and disk built, the rest planned)

The owner's rule for the rest of phase 4: nothing may depend on a figure kept by hand that can go
stale. A step's needs are **measured** on every run and learned per target, or **enforced where the
resource is used** (a lock or accessor the code takes as it opens it, as `store::cachefile` does for
the caches: plan.md §8, Room on the disk), or **derived from the code that does the work**, with a
lint that nothing goes around it. The bar for each mechanism: a declaration missing or wrong costs
speed at worst, never a wrong output, impoliteness to a third party, lost work or a refused good
result. The steps table (§7.2) holds nothing the scheduler trusts: its disk and groups columns went
with batch 13 (§12); what's left is first guesses, the code's own sets and the write-sets.

Each of its columns, and what replaces it:

| Column | Replaced by | Notes |
| --- | --- | --- |
| power (`cpu`) | nothing: removed (the owner's choice; batch 3, built) | jobs run at any charge; `BATTERY_MIN` is left only for whether a Mac may lead |
| home | nothing: removed (the owner's choice; batch 3, built) | the OSM pass, its sets, the reach and the world's buildings run over Tailscale away from home, slower; mounting the share (the LAN name at home, the bare name through Tailscale away) stays |
| memory | measured per target (the jobs' `peak_mb`; the memory guard's floors, §7.2): built | a target never run is a **cold start**: placed only where it can't hurt (below); the guard is the backstop |
| disk | **built** (phase 4's fifth batch; the guard and the placement by measure always on since its thirteenth): measured per target, as memory is, without walking a job's folders (millions of raw tiles; Planetiler's sparse files): the bytes written through the accessors that write a job's files (`store::cachefile`'s, the scratch writers'), counted per job and target; the biggest writers, outside programs the accessors never see (Planetiler's temporary files, osmium's and extract's outputs, the Python steps'), by a cheap look every minute at the allocated sizes of the few largest files at the top of each slot's scratch folder (no walk of its tree); kept as the target's disk floor (the disk's free space falling while one job runs alone is logged beside, a weaker signal, never a floor: other programs on the Mac move it too); a **disk guard** at the point of use: when the Mac's free space falls below the reserve, the job growing fastest by those counts is frozen (not failed: its work kept), room is made from the caches no job holds (`store::cachefile`'s exclusive locks), and it goes on; if none can be made, it's given back, its disk floor learned; a job given back keeps its scratch folder (its work) while there's room, and room-making, short of what the job starting or the disk guard asked for once the caches are freed, deletes the given-back jobs' scratch folders (a job given back by either guard), the oldest given back first, never one a job running or frozen here uses (`room::free_given_back`, the agent's `given-back.json`), so nothing stalls on it | as built: store::cachefile counts the bytes a process adds to the caches (each file it names), a step's cost line notes its target's (`disk_mb`), and a running job's total is kept in a small file every 64 MB (`SCENIC_WRITTEN`) for the agent's sampler, which looks at the top of its scratch folder every minute and keeps the most per target (the disk's fall while it runs alone logged beside, a weaker signal, not a floor) (crate::agent::memguard); the floors go to the lead with the lease's end, rising only (`disks.json`, the pool's state); the disk guard freezes in the sampler, at once, and the agent's loop makes room through room::toward (store::cachefile's exclusive locks) on a thread, then lets the job go on or gives it back, kept from that Mac an hour as the memory guard's; beside another job, a job's disk room is the reserve and its measured disk; alone in the first slot, room is made for the reserve, its measured disk and the owner's target as the caches allow, but it waits only for the reserve and the target (a disk floor only rises: a stale or high one never holds the build), the disk guard its backstop, the table's figures of before (35, 55, 80 GB) gone; the reserve stays a policy of the Mac, not of a step; a write that bypasses the accessors is caught by their lint, as the caches' reads are. Not counted by the accessors: the Python steps' cache writes (dem/cachefile.py), which the scratch folder's look covers where they're large; the planned design's fall of the free space is kept apart, logged, not a floor (one outlier, or another program, would inflate it) |
| alone, beside and its ranks, light (in use) | removed (the owner's choice; built: phase 4's fifth batch, always on since its thirteenth): a job starts beside another only if both jobs' **measured** memory and disk fit what the Mac has left; a cold start runs alone | what they protected besides memory and disk, each covered at the point of use: the OSM pass clearing the pack cache (`--clear`) goes through `store::cachefile`'s `remove_tree`, which passes over files another job holds (built); GC beside other jobs: it removes only what no catalog or the manifest names and is older than two weeks (abandoned temporary files after two days), but a hand-off not merged yet (a member away more than two weeks, its entry unwritten or untold) names files neither reads, so GC reads the journal's entries too (every entry not yet in the lead's records keeps the files it names, its changes and uploads; built: agent::gc::unmerged; an entry that doesn't read whole, reads damaged or isn't there as listed, or a journal that can't be read, fails GC, nothing removed, tried again as a failed job is, until the lead has refused the entry, which it does after an hour unreadable; with no term led (its term not readable whole) no one refuses, so an entry unreadable whose day is over three days past is passed over, said as GC's warning in the status: the hold is bounded either way) before batch 5 lets it run beside other jobs; scratch folders are per slot already (`scratch-2/`); two jobs of one step on one Mac share no state but the caches (accessor) and the records (hand-offs per lease); Planetiler's thread pool beside another job only slows both (cores, not correctness). The "only network work while the Mac is in use" rule went first, without a replacement (the owner's choice; batch 3, built), the light steps' list and what read it with it; `idle_s` stays for a member sparing more memory while its owner is away |
| groups: raw tiles | `store::cachefile`, already: a raw tile is read whole under the accessor's lock, packing deletes only tiles no job holds (`try_remove`) and only once their archive is named on the NAS, and a tile gone is filled again from the archives: two jobs reading raw tiles at once are safe, at worst fetching a tile twice | checked by a chaos test (two terrain pieces of one area at once through one cache, its waiting tiles packed and every file no job holds deleted between: each piece's bytes as alone; `terrain_pack`'s `two_raw_tile_jobs_at_once_with_packing_between_make_the_same_bytes`); it found a job whose area's archives were opened before a tile it held loose was packed fetching that tile from AWS again (offline, failing): a job now opens its area's archives again, the index read afresh, when the index has changed since the archives it looked in were opened (by the index file's inode, the server's file ID on the share, its time and size: it's only written by a temporary name renamed over, so a new version has a new file ID as the share gives them, and a change the stamp missed would cost only a tile fetched again, not a wrong one; compared, and the area opened again, under the area's lock, so no thread opens it from an older index between), or when they couldn't be opened, before asking AWS (`RawTiles::get`, `refresh_area`); the group gone (batch 13) |
| groups: Wikidata | **built** (phase 4's seventh batch, always on since its thirteenth): **one budget for the whole pool**, enforced at egress (a Mac can't reliably know the address its requests leave from: a Tailscale exit node, IPv4 or IPv6; so stricter than one per address, and than the owner's "shared whenever unsure"): every job's connections out go through its agent's proxy (`HTTPS_PROXY` and the others set for every job, this Mac's own direct by `NO_PROXY`), and each job runs under a `sandbox-exec` profile that denies it every outbound connection but to the Mac itself, so a step that forgets the proxy fails at once, never reaching Wikimedia unmetered; the proxy meters by host on `CONNECT` (a tunnel is the client's TLS, never read) and on a plain HTTP request, taking a token from the pool's budget on the NAS before it opens a connection to a metered host; tokens leased in batches; the query services budgeted by query time as well as count; a `Retry-After` or a 429 seen by any process holds every process and Mac; with the NAS out of reach, each Mac takes the rate divided by the members it knows. Other hosts (the NAS's are the kernel's, SMB, not the job's sockets; AWS's tiles, the registers, PyPI) pass through unmetered | as built (crate::budget, its proxy crate::budget::proxy, its lint `tests/budgets.rs` gating publish): not one bucket file but a file per lease, made with create-new (§2, principle 3: nothing written in place by two Macs): time cut into ten-second windows, a window's tokens into slots of a batch, a lease a slot's file (`state/pool/wiki/<class>/<window>-<slot>`), so no window gives more than its tokens whatever the Macs' timing, and a batch unspent at its window's end is lost (the budget only errs below its rate); the leases' files of windows five minutes past removed by their names, no listing, but after a gap of more than 64 windows since the process last cleaned (its first lease, or one after a long idle): then by one listing of the class's folder, as the gap's windows can't be known by name (another Mac may have leased in any of them) and walking every name in a gap of days would cost more than the listing; a window is a Mac's clock's (Macs a second apart may spend a window over its length and a second). The rates are the owner's politeness, not a step's figure: Wikimedia's sites (`wiki`: Wikipedia, Wikidata's API, Commons, the dumps) four requests a second (the owner's choice, 14 Oct 2026), four a lease; the query services (`query`: WDQS and QLever, which heritagewd.py asks) one query in two seconds, and a tunnel open longer than two seconds owes a token for each two seconds more, paid before the process's next query. A 429 is told to the proxy (`POST /hold?s=<seconds>`, dem/budget.py's `seen`, called by heritagewd.py and whsshapes.py; capped at 15 minutes), which holds its own Mac at once and answers, then takes every slot of the windows the wait spans on a thread of its own, one hold at a time (a batch already leased elsewhere may still be spent); the NAS's part is the wait's first five minutes at most (a create a slot, fifteen slots a window across the classes: 450 creates, some 8 to 23 s on the share; past them, another Mac still asking meets its own 429). The proxy ends a client whose request head hasn't come in 30 s, a connection idle both ways for 10 minutes (a query's tunnel charged as it ends), and answers a connection past 256 at once with a 503; the budget's state is never locked across the share's I/O (one thread leases at a time, on a lock of its own), so the status and connections not metered never wait on the share. The NAS away (a lease that can't be made): this Mac's share in memory, the members its pool knows. A job starts only once the proxy runs. The status's `budgets` says what it opened and holds. Not built: a concurrency budget (dumps.wikimedia.org's two connections an address), pageviews.py's 429s (a month's download fails, tried again by the next run) | a missing budget can only make requests wait, never exceed the polite rate |
| groups: heavy NAS reads | **nothing** (the group gone): two large reads at once measured only to slow each other (14 Oct 2026, the M4 reading 2 GB files of a scratch folder of the NAS uncached, `F_NOCACHE`: alone 81, 86 and 80 MB/s; two at once 78, 88 and 88 MB/s together, each half) | measured with plain reads of the share, not two `bldprep` (whose reads have work between them, so overlap the better): no accessor built |
| answered | **built**: a lease at the point of use, across the Macs: each sync and keep of a pass's answers with the NAS's (`crate::answers::Kept`) takes the answers' lease on the NAS (`state/pool/answers.lease`, create-new, renewed every 150 s by its holder, lapsing 10 minutes after; a lapsed one broken by moving it aside by a name of the taker's own, put back if it isn't the one the taker read: renewed meanwhile, its `until` moved on, or another's); a sync or keep checks the lease is still its holder's (the file names it and hasn't lapsed: a renewal that failed while a breaker had it aside decides nothing) before each write to the NAS, and stops, writing nothing, when it isn't; on a Mac, with writer preference: the steps that keep answers hold this Mac's copies shared for their run (`answers::Local::reader`, a flock in the agent's cache), the agent sending them as it starts (`seed`) holds them alone (`Local::writer`: it waits for the steps under way, and a step that starts while it waits waits for it) | no list of the steps that keep answers waits on the agent's seed; a lapsed lease costs a wait, never a torn answers file (the answers are written whole by temporary name). What remains possible: a holder whose check passes and whose write then lands after its lease was broken (its Mac asleep between the two, or a write that takes past the lease's 10 minutes), and a put-back that fails because a third taker made the lease meanwhile (the holder put back loses it, and finds so at its next check or renewal, but a write already past its check lands): two writers to one archive for that moment, which costs a merge (answers fetched again by the next start), never a torn archive. As built, the cross-Mac lease is exclusive and short (a sync's or keep's minutes), and the long shared hold is the Mac's own lock: what the reader's hold protects (a step adding to this Mac's copies while the agent packs them) is on one Mac, and a merge across Macs is what the NAS lease serializes |
| shared and its rank | every step offered to every member (§7.2); the order by how crucial a target is (the forecast's slack, measured times) | |
| batch | measured: as many of a step's targets as fit a quarter of an hour by their measured times (a cold start: one target a job): **built** for a member's grants (batch 9); the lead's own jobs keep the table's batches | a wrong time makes a job longer or shorter, never wrong |
| write-set | derived from the code that writes, and enforced as it lands (the owner's choice: no soak): each step's output names are given by one function (`agent::steps::writes`, the steps table's row), which `Out` writes by and the lead's merge checks every entry by, so the writer and the check can't disagree; a job's `Out` refuses a name outside its step's (a put before it uploads; a set or removal fails the job's next save, and nothing of that save lands, though the targets it saved before stand), and the lint, best-effort (a scan of the source for what makes or writes a hand-off or an entry, or names the manifest's file or the journal's folder: nothing does but `Out` and the listed paths of the merge, the readers and the raw tiles' archives), gates publish; an entry from a member on a newer app than the lead's that its write-sets refuse is held, not refused (§7.3). The step's removals of stale names under its layer, a prune's of the pruned step's names, and the OSM pass's of older passes' are the same function's. Writes outside the manifest: the raw tiles' archives (named by their area, checked as today: crate::rawpack::named_for), the answers (`crate::answers`' own names, under its lease) and the 3D buildings' sources (bld-fetch's, under `sources/`, which nothing reads by the manifest) are outside the records, so outside the merge; each is written whole under a name its own module makes | **built** (phase 4's sixth batch): the function is a predicate over a name, not a type only it can construct (the design's first form): the same guarantee as the job runs, without rewriting every step's writes; entries of an app before it are refused as any outside is (their work done again) |
| moves (planned) | derived: a job's progress on its Mac's disk is written through one accessor (the OSM pass's stages, a terrain area's raw tiles), which names the Mac holding it on the NAS (`state/pool/progress/<step>.json`) | |
| floor of memory (planned) | not built: what a target held (its measured floor, §7.2) covers it, the Planetiler heap among it | |

**A cold start** (a target no run has measured, of a step none of whose targets has): placed where it
can't hurt, by what's known of the Macs, never by a guess: alone, on the member with the most memory
and disk free now, the guard on; once a target of a step has run, the step's other targets are
offered by the largest measured of the step's targets until their own run says (a prior learned, not
kept by hand). Built for a Mac's own slots (crate::coord::Coordinator::measured: a run's measure or
floor of the way its step runs now; a job of no targets of its own by its id); the member with the
most room is batch 9's (built: crate::coord's `most_room`). As built, the prior is the step's targets' 90th percentile, not their most
(one outlier holds no other target to its figure), and a target measured at 0 keeps its 0. A
batch is reckoned by its targets' disks summed (cache writes add up over a batch) or by the most a
job of its step took over its whole run (`job-disks.json`), the larger; what a running job has still
to take, by its targets not noted done yet, or that most less what it has taken. The disk's fall
while a job runs alone is logged beside its counts, a weaker signal, never a floor. A stale or high
floor never stalls the build: alone, the first slot starts a job once room-making has done what it
can, the disk guard its backstop. With two jobs, the second filling the disk past half the reserve
while the first is frozen for it is frozen too, and of the frozen the one taking the most is given
back first. Only the lead's process has a coordinator to ask, so a member with the switch on takes
every job as a cold start (it runs one at a time: safe).

**Where a job's inputs lie:** the pass's later steps (its sets, the reach, the route ends) read its
outputs from the NAS (`pass-sets` copies the filtered planet into its own scratch first); a Mac holding
a copy is faster, never required: locality is a speed, not a need, beyond the resumable progress that
**moves** names.

**The ways a step runs** (`crate::coord::cost_version`, which a measure or a floor of an older way
counts for nothing against, but as an estimate) are a figure kept by hand: a change of a step's memory
not marked so leaves its old measures standing until its targets run again (speed only: the guard and
its floors catch a target that now takes more). Planned: derived from the step's key version
(crate::agent::build's `*_V`, which job keys carry and a change of output bumps), so a measure is of
the code that made the result it was measured with.

**What can't be made structural:** the Mac's own reserves (memory's eighth, the disk's 30 GB) are
policies of the Mac and the owner, not needs of a step; the first run of a new step is a cold start
by necessity.

## 8. Tasks, and the pages as workers

**One model of work.** A job is a step's run over targets, on a Mac with the NAS. A task is a pure
piece of a job: programs (the build's, natively or as WebAssembly) run over a folder of files the
broker serves, reading what lies elsewhere through the broker (`/net`: the NAS's data and the data
servers' files, workers.md §3), giving files back. Every worker that can run a task's programs may
take it: a page, any member's slot. Jobs are for Macs; tasks are for everyone.

- **Kinds.** A task names its kind (today `tail`: a unit's elevations to flags), its programs, its
  inputs and their sizes, its places, the memory it's predicted to take, and its runtime needs
  (WebAssembly or native, `/net`). New kinds come as steps are cut into tasks (map tiles, landmarks,
  terrain and slope subtrees, tree blocks: workers.md §3, Planned), with no change to the workers.
- **Brokers.** Each member brokers the tasks its own jobs offer, with today's code (crate::coord::
  task) in its own agent: the task's files stay on its disk, its uploads come back to it, and a task
  lives as long as its job, whoever leads. Its heartbeat says how many tasks wait, of which kinds,
  how large.
- **Finding work.** A page (or a member's idle slot) asks the lead "where is work that fits me?"; the
  lead answers from the members' heartbeats with the brokers to ask, most waiting first. The page
  then leases from that broker directly: `/work/ask`, `/work/in`, `/work/net`, `/work/out`,
  `/work/done` on the broker's own address. The lead isn't on a task's path, so a handover drops
  nothing.
- **Pages talk to members directly** (built, behind `state/pool/brokers`: §12 phase 4's tenth
  batch), over HTTPS (`tailscale serve` on each Mac, the owner's to turn
  on) with CORS: every member answers the pool's own pages (`Access-Control-Allow-Origin` for the
  members' addresses; preflight for `Authorization`, `X-Worker`, `Range` and `PUT`). No gateway
  forwards anything: no streaming proxy for uploads, no file crossing the network twice, no
  forwarding loops while Macs disagree about who leads. (Worth checking first: Tailscale Services, one
  name for several hosts, would give the iPad's home-screen app one address and no cross-site calls.)
- **What a page is told, and what it can't count on** (built: §12 phase 4's tenth batch).
  `/work/where` answers addresses and counts alone (no member's id or host: a page names a member
  by its address). A member's LAN name (`.local`) is no page's origin when it has a tailnet
  address. A page's hand-back as it closes (`pagehide`) to another member's broker needs a
  preflight (JSON, `X-Worker`), which a browser may not send for a request kept alive past the
  page: the JSON-only routes stay so (a simple request is what a page elsewhere can send), and such
  a task comes back by the next page's look at its notes (they keep the broker's address) or by
  its lease lapsing. Chrome's Local Network Access (its successor to Private Network Access) may
  prompt for, or block, a page's calls to another member's LAN or tailnet address whatever the
  preflight's `Access-Control-Allow-Private-Network` says: on such a browser the page's own
  member, over HTTPS where `tailscale serve` is on, is the one sure way.
- **One page, any member.** The page loads from any member; it knows every member's address from the
  pool (the lead's answer, its heartbeats); when its member goes away it reloads from the next that
  answers (a page carries no key). The agents' token is the pool's (`state/coord/token`, the same
  everywhere).
- **Versions.** The page and a broker may run different apps now: the API has a version, and a page
  reloads when its broker's is newer. A task names the programs' build it needs; a broker serves
  them.
- **Trust.** A worker found giving wrong results (a broker's check: workers.md §5) is recorded per
  broker on the NAS (`state/coord/trust/<member>.json`), read by every broker.

## 9. The pool's API

Planned. In phase 1 the members' messages go by mailbox on the NAS (crate::agent::pool:
`state/pool/mail/<to>/<from>.json`, its sender's alone, its last messages there numbered; read by
member id, never by listing, so a lead reads the mail of the members its listing of the heartbeats
found, every two minutes), best effort as the driver's messages are, an owner's ask passed on to
the lead among them (`HandTo`, §6.3); jobs go out through the lead's coordinator, every
member's (`/work/*`), and the worker page's lead asks to it (`/work/lead`).

Every member answers on the pool's port (8090): the worker page and its files; `/pool/status` (its
heartbeat); `/work/*` for its own tasks; and, on the lead, `/lead/*`: jobs (ask, beat, done), lead
asks, pause asks, "where is work". A member reaches the lead at the addresses in the lead's heartbeat
(Tailscale's, then the LAN's). A member that can't reach the lead keeps its jobs running (their
hand-offs go to the journal) and starts nothing new; its brokered tasks carry on.

## 10. Seeing the pool

- **Each member's heartbeat,** `state/pool/members/<id>.json` (whole, its own writer): its label,
  app, resources, conditions; its slots' jobs (progress, parts, memory) and why a slot is idle; its
  tasks waiting and out; its addresses; whether it leads (its term), a handover's state
  (`handing_to`, its offer; `ready_for`, the offer it answers), or that it stood down from its term
  (`stood_down`: §6.6). Written each step of its pool it changes (§6.7), at least every two
  minutes, its beat stamped as it's written. Read by member id (the ids are in the lead's status), not by listing.
- **The build's status,** written by the lead per term (`state/build/term/<E>/status.json`): the
  checklist, the forecast, the regions, what waits and why, the lead's decision on the next map
  update and why (`map_update`: docs/plan.md §8, Faster publishing), the jobs that ended lately (any
  member's), the pause, the last catalog, the lead and its term, a handover under way.
- **Everywhere it's shown** (the menu bar on every Mac, the map's build panel, the worker page,
  `scenic lead` and `scenic status`): the pool, a line or card per member, the lead marked;
  out-of-touch members with when they were last heard from; a handover's states as they happen; "No
  lead" with "Take it". They read it from the agent's status (`pool.lead`: crate::agent::lead::View,
  made again every half minute, and at once after an ask or a change of lead), or from the pool's
  own view when that's fresher (`agent/pool/view.json`, written at every step of the pool's, §6.7:
  the status waits for the agent's loop; `lead::own_status`, `own_pool`, the menu bar's
  `ownStatus`): the term's lead,
  each member it knows read by id from its heartbeat (never listed), with its state (home on power,
  on battery, away, out of touch, clock wrong, stood down, app too old) and whether the lead can be
  handed to it and why not (`Driver::hand_to`), what a takeover from this Mac needs
  (`Driver::takeover`), "no lead" and why (the term unreadable, its lead's heartbeat missing, out of
  touch or stood down), the offer, the last ask and the last change of lead. The members' states
  come from their heartbeats' conditions (`conds`: home, mains power, battery, able to lead), which
  apps before phase 3 leave out ("conditions unknown"). The menu bar and the map show this Mac's
  agent's view (the map server's `/api/build` `pool`), the worker page the serving Mac's (in
  `/work/swarm`'s agent).
- **The alert: no lead for an hour** (`lead::alert`, `ALERT_S`): counted from the lead's last beat,
  or from when this Mac first saw the term with no lead (kept across its processes, `lead.json`'s
  `no_lead`), the view's `alert` says how long, why, and what to do: the command that takes it over
  from this Mac (`scenic lead take`, with `--force` or `--downgrade` as `Driver::takeover` needs),
  or, when this Mac can't, `scenic lead take --force` on another; for a term this Mac reads short
  (not one it can't read), that no Mac took it past by itself (§6.5). The menu bar makes it its
  state (the problem icon, its line, and a notification once: when it comes on, or at the menu
  bar's first look when it's already on), and `scenic lead`, `scenic status`, the map's build panel and the worker
  page (its alerts and the pool's section) show it in place of the "no lead" line.
- **A member's own status in the menu bar:** a member's agent writes `helper.json`, never
  `status.json` (the lead's); the map server shows the lead's heartbeat from the NAS, and a
  member's own `helper.json` in its place once that heartbeat is ten minutes old (no lead), so
  the member's menu bar stays fresh. The lead's list of its members' statuses leaves out its own host (its
  `helper.json` from before it led, fresh for ten minutes, isn't a second card).
- Each Mac's map server keeps the app its own agent runs, and keeps its
  downloads' copies to 20 MB/s while the lead's heartbeat says a job runs.
- **The history** notes each term (handed over, taken back, taken over, re-asserted), each step
  down, and a handover's offer, end, giving up and drop, as `term` events (crate::agent::lead::notes):
  appended as they come to the member's own file on the NAS (`state/coord/history/<day>/
  <member>.jsonl`), and into its coordinator's history (the worker page's activity), or, a member
  without one, kept for the coordinator of its next process (`agent/pool/notes.jsonl`: it restarts
  into the lead after a takeover). Members joining,
  leaving and coming back: planned.

## 11. Controls

Built (crate::agent::lead, `tools/status/main.swift`, `web/work/pool.js`, `web/src/ui/buildstatus.ts`,
`scenic lead`):

- **The menu bar's menu** (a right-click on its icon, or its panel's "⋯"), on every Mac:
  - on the lead: "Hand the Build To ▸", the other members, each with its state (home on power, on
    battery, away, out of touch, app too old); those that can't lead now greyed with why. Handing to
    a Mac that can't lead now (away from home, on battery under 30 %, short of disk) is greyed with
    why, as its heartbeat's conditions say;
  - on any other member: "Make This Mac Lead" (an ask to the lead), greyed with why when the lead
    can't be handed to it; when there's no lead in touch, "Take Over the Build…", confirming: it says
    why there's no lead, and what the takeover forces or downgrades when `Driver::takeover` says it
    needs that (the menu, on the Mac itself, may);
  - the proactive offer (§6.5) when it applies; an ask under way, and how the last one ended; the
    pool's members and the last change of lead among its lines; a notification when the lead
    changes or an ask ends; Pause/Resume, as now (any member).
- **The worker page** (`pool.js`, served by the lead's coordinator): on each member's card, "Make
  lead" (confirming); on the lead's, a handover's progress; "Take it" when there's no lead, unforced.
  Until any member serves the page (phase 4), only the lead's coordinator does: "Take it" shows only
  when the Mac serving it knows the term has no lead it's in touch with (its process stepped down,
  say), and asks that Mac to take over. With the lead truly gone the page isn't served at all, so
  the menu bar and `scenic lead take` are the ways to take over. A click on the menu bar's icon shows
  this page in a popover, with its buttons.
- **`scenic lead`** (who leads, the term, since when, a handover under way, each member and whether
  it can lead, the offer, what a takeover needs, the last ask), `scenic lead give <member>` (its host
  name or member id), `scenic lead take [--force] [--downgrade]` (refused at once with the driver's
  why when it needs a flag not given), each followed until it's done or refused; `scenic lead auto
  on|off`, the offer's switch. `scenic status` shows the pool too.
- **The map's build panel:** who leads, each Mac, and "Make this Mac lead" for the Mac it runs on;
  "Take over…" when there's no lead, unforced.
- **Forcing stays on the Mac:** a takeover with the owner's force or downgrade comes only from the
  Mac's own menu or `scenic lead take`; the map's `/api/build/lead` and the coordinator's
  `/work/lead` refuse one (each behind the same gate as pausing: this Mac, its LAN and the tailnet,
  from no page elsewhere).
- **Each asks the driver** (crate::pool::driver) what its step would decide, rather than check it
  again: whom the lead can be handed to, and why not (`Driver::hand_to`); what a takeover from this
  Mac needs, the owner's force or downgrade, and why (`Driver::takeover`).

## 12. Getting there from today

The pool is always on (phase 4's thirteenth batch): nothing switches it on or off. A batch that
changes what the agent does ships behind a switch of its own under `state/pool/` (item 4), made
once every member's heartbeat shows the app that has it.

1. **Terms and records per term.**
   - **The core: built** (`crate::pool`, checked by the simulator: §13). Member ids (`member_id`,
     bound to the Mac); `terms/` and term 1 (`term`: made with create-new by the first Mac
     in the pool, `term::bootstrap`, or by the owner's forced takeover, its first snapshot first; a create whose bytes didn't land finished
     by its maker; the app rule, the take-back checked against the term handed over; the owner's
     forced takeover, and downgrade); records per term (`records`: term 1's first snapshot from
     today's layout, in a file of its own no lead writes, its maker's create cut short written
     whole, and a take-up with no snapshot of term 1's it can read whole waiting ten minutes before
     today's files, which term 1's saves keep writing; the merge, in lease order across merges;
     taking up, numbered on across tries; the forget horizon; readers' fallback to a term before);
     the journal as a log, written whole by members directly (`journal`: entries by lease id
     `<term>-<n>`, kept whole until written, one there already that says the same counting as
     written; refusals noted beside it; what a member tells each lead); the NAS's operations (`nas`:
     create-new, saying when its bytes didn't land, and whole writes by crate::whole); the
     heartbeat's fields (`beat`); phase 2's transitions (`handover`: the state machine with
     take-back, an answer to its own offer); the member's lock (`MemberLock`: one process per
     member, in the folder it's given, checked every step); and **the driver** (`driver`): what a
     member is and does, one step per loop of the agent's, through an I/O trait (the NAS's
     operations and two clocks), the code the agent will run and the simulator runs. A step learns
     the current term; takes up a term naming it; leads (merges what members tell it and what a
     listing finds, saves, acknowledges, notes refusals); re-asserts after a gap, a stall, a
     restart, or a saved state that doesn't know a term it made; stands down when the app rule
     refuses it; hands over and takes back; takes over when the owner asks, and by itself a lead
     that stood down; writes its jobs' entries and tells the lead of them. It asks the agent for
     what's slow (a listing of the journal, off its loop, one at a time) and what's the agent's
     (settling: the coordinator's state), and says what the agent may do now (grant jobs and plan;
     settle; publish and sweep once its records are caught up, by a listing asked for under a day
     ago, a sweep only when fresh), and when it must stop (another process holds its lock); it
     answers the controls (whom the lead can be handed to, what a takeover needs). Its contract is
     the module's doc. The agent runs it (below).
   - **The integration: built, on since 8 Oct 2026, with no switch since phase 4's thirteenth
     batch** (crate::agent::pool; the agent, crate::agent). A process not in the pool (the NAS
     not reached as it starts, or its member's lock held by another process) starts nothing and
     restarts into the pool once it can.
     - **A process's part** is the terms', decided as it starts (its member's first step): the lead
       plans and grants through its coordinator; any other member asks the lead's coordinator for
       work (`scenic agent --helper` accepted and ignored). A member whose part changes (it took a term up, or stepped
       down) changes it in its process with `state/pool/slots` on (§7.6), else restarts into its
       new part once its first job's slot is free. A process whose member's lock another
       holds starts nothing, tries the lock each loop, and restarts into the pool once it's free;
       one the driver stops (its lock taken since) saves that step's state, starts nothing, and
       exits between jobs. A saved state naming terms when the NAS has none (its terms since
       moved aside) counts for nothing but its unwritten hand-offs.
     - **The loop:** the driver steps once a loop, after the loop's ended jobs are gathered and
       before the plan; its state (`Saved`) is written whole to the agent's folder whenever the
       step changed it, before anything the step said is done (a step whose state couldn't be
       written does nothing of it, its duties held); a job's folder stays until a saved state holds
       its hand-off.
     - **The listings** it asks for are made on a thread, one at a time, each handed back, a failed
       one made again a minute on; the members' heartbeats are listed the same way, every fifth
       of the time a heartbeat takes to go out of touch (two minutes), on the lead (whose mail it
       reads) and a member alike. A member whose heartbeat file the listings miss for twice that
       time (twenty minutes) is forgotten: no mail read for it, out of the views and of the
       count the budgets divide by, and not known again from a message, a term or another Mac's
       heartbeat naming it until a listing finds its file (the forgotten kept across restarts, the mail
       queued to it dropped). A failed listing, or one without this
       Mac's own heartbeat (the folder not there, the NAS away), forgets nobody; a Mac asleep or
       restarting keeps its file, so it stays known.
     - **Messages** by mailbox on the NAS (§9: the pool's API is planned).
     - **The heartbeat**, `state/pool/members/<id>.json`: the driver's fields and the members it
       knows, written when it changes and every two minutes; the agent's status says the member,
       its part and what the driver lets it do (`pool`).
     - **Jobs hand off:** every job, the lead's too, runs under a lease of the lead's coordinator,
       `<term>-<id>` (its folder `agent/pool/jobs/<term>-<id>/`, `SCENIC_HANDOFF`), and its saves,
       the targets it finished (all of them, or what it noted done when it stopped or failed) and
       for a shared step only those targets' files become its entry, handed to the driver, which
       writes it to the journal and tells the lead; a member's tells the lead's coordinator too
       (`/work/done` with `journaled`: the lease ends, its targets kept out of offers until merged,
       nothing journaled there). Jobs an earlier process left are handed off at its first loop,
       once any still running is stopped. No agent
       records keys, merges hand-offs or re-keys the records (phase 1 has no entry for a re-keying:
       a new key scheme builds its targets again).
     - **The merge's checks** (`agent::pool::check`): a done record of the entry's step naming
       targets, content names of their logical names, raw archives of their areas, and for a
       shared step the coordinator's check of a member's hand-off (its done targets' files, its own
       uploads); not the lease's targets (a later lead may not know the lease). An entry with a
       change outside its step's write-set is refused with them (§7.3).
     - **The lead's coordinator:** leases granted in its term (`Lease::term`, `granted_at`), its
       state per term (`state/coord/term/<E>/state.json`: the jobs' leases, costs, failures by the
       wall clock, the pause) written each loop it changed, on the NAS; "the lead is moving" while
       the driver says no duties; a take-up loads the state handed over, else the newest a term
       before has; its own host's leases from before dropped; its history's new events appended to
       `state/coord/history/<day>/<member>.jsonl`. Saved on the loop after a grant or a finish, not
       in the grant (the coordinator's lock is never held over the NAS): a lead gone between loses
       that grant's record, and its job's entry merges all the same.
     - **The records' readers** read today's three files as before: term 1's saves write them, and
       the lead writes them from its records after each save of a later term
       (`agent::pool::write_today`); reading the snapshot itself (§6.2's readers) is planned. The
       lead names the raw tiles' archives its records hold in the raw store's index; they stay in
       its records (phase 1 has no way to take them off).
     - **Gates:** nothing new starts on the lead while the driver says no duties; a catalog only
       when it says the records are caught up and today's files hold them; GC only on a step that
       re-asserted, caught up (the agent asks the re-assertion, and the next loop's sweep runs). A
       lead that no longer leads stops a catalog or GC in flight, and takes its coordinator's
       contact (`state/coordinator.json`) off the NAS.
     - **Settling** (§6.4) stops a catalog or GC in flight, stops granting and hands the
       coordinator's state to the step. The owner's asks start a handover (phase 3).
     - **Seeding and draining:** the workers' token copied to `state/coord/` by the first lead
       (create-new) and from there by every lead's coordinator before it starts; the hand-offs
       from before the pool, the coordinator's local `coord/journal/` (the lead's) and a member's
       `outbox/` (once a process; a folder with nothing to hand off removed), drained into the
       journal as its member's entries (term 0: from before the pool; an outbox's under its old
       lease, the others numbered from `agent::pool::DRAINED`, apart).
     - **Gone** (phase 4's thirteenth batch): the writer named, `SCENIC_BUILD_MAC`, `check_writer`'s
       pass (once a term is made it refuses every direct save), the NAS's hand-off folders.
   - **Switched on** 8 Oct 2026 on the real NAS: term 1 the M4's, made from today's files, the M1
     a member. With no term yet, the first Mac in the pool makes term 1 and its first snapshot
     from today's files (create-new: of Macs making them at once, one makes each), takes it up
     and leads (its status's `pool`: `role` lead, `term` 1). `scenic pool status` says how it
     stands. The first catalog waits for the records to be caught up (a listing of every day
     merged), the first GC for a re-assertion.
   - **No switching off, no shadow run** (phase 4's thirteenth batch): `scenic pool on|off`,
     `state/pool-off/`, the `pool-shadow` command and `state/pool-shadow/` are gone; `state/pool/enabled`
     and `state/pool/shadow` are no longer read, left on the NAS while an older app that reads them
     may run (never `scenic pool off` from an older app).
2. **Handing over and taking over.** The driver does them (§6.4 to §6.6: phase 1's core, built);
   the owner's asks reach the agent (phase 3); the agent's part, built with phase 4's fourth batch
   (below), behind its switch: staying awake while leading, and a part changing in its process,
   its jobs going on through the lead wherever it is (without the switch, a lead handing over
   mid-job keeps running its job, which hands off to the journal under its lease, and restarts
   into a member once its slot is free).
3. **Controls: built** (crate::agent::lead; §6.3, §10, §11). The owner's asks reaching the agent
   and the driver (an ask file in the agent's folder; the build page's through its coordinator; a
   member's passed on to the lead by mail), checked as the driver would; where each stands; the
   view in the agent's status; the menu bar, the worker page, the map's panel, `scenic lead` and
   `status`; the history's terms; the proactive offer, and its switch (off).
4. **Every job hands off, placement and pages: under way, in batches.** Each batch ships on its
   own, with its tests, and, where it changes what the agent does, a switch: a file under
   `state/pool/` (off while missing, read as `auto-handover` is, unless the owner chose it on, as
   for the memory guard), made once every member's heartbeat shows the app that has it, so a batch
   can soak on the real build behind it. A batch that only reports, or only learns, needs none. They're ordered so the parts that can lose work
   (refusing entries, stopping jobs, a job across a handover, resuming on one Mac) come early, and
   run longest, reported or switched, before what builds on them.
   1. **The steps table: built** (crate::agent::steps; §7.2, §7.3). A row
      per step: memory's first guess, disk, power, home, shared and its rank, alone, beside and its
      rank, light, the groups never two on one Mac, the answers kept, the batch, the write-set. The
      step sets the agent and the coordinator test are made from it as the app is built (a set of
      the wrong size, two steps of one rank or a rank missing failing the build), in the orders the
      agent and the forecast walk them, and its tests check them, the rows' memory, disk, needs
      and batches, a row for every step the agent runs, and a shared step's saves within its
      write-set. Every one of the 574 entries in the journal of
      8–9 Oct 2026 is within (`real_journal_entries_are_within_their_write_sets`, run on a copy of
      the journal), though they hold none of the pass's worldwide steps (the OSM pass, its sets, the
      reach, the world's buildings, the summits, the heritage sites; the labels and the water once
      each). Needs no switch.
   2. **A job far over its memory: built** (crate::agent::memguard; §7.2). Each job's memory
      sampled every 5 s on a thread of its own, per target under way (a job says which: `started`
      lines in its costs file, which starts afresh with it); the limit per Mac (none known, nothing
      guarded); by measure alone, the job beside the largest drained when that suffices, the
      largest past the limit alone stopped at once only while the Mac is short of memory (frozen
      by the sampler, stopped by the loop; the swap's growth over the last five minutes), else
      drained and stopped only by trouble; a drain of the job beside reaching no safe point in 15
      minutes stopped; a sample kept only for the job it read; a job stopped given back, not failed, kept from that Mac an hour; floors
      learned whatever the switch, with whether a job held the target alone and the way its step
      ran, a measure (the same kind of figure, a unit's too: `cost_version` 1) taking a floor's
      place; a target past this Mac's limit left to a Mac with room, one past every Mac's held with
      why, one whose floor past the lead's limit was a batch's tried again alone on the lead first
      (floors written as bare MB read as a batch's); `scenic pool floors [--clear
      [<step> [<target>]]]`. The switch `state/pool/memory-guard` (`off` or `on`), on while missing
      (the owner's choice). Tests: decisions by measure; the limit, unknown memory; trouble by
      pressure or swap; the target under way and its fallbacks; floors from what a run held but
      what it measured; the sampler freezing only in trouble; the switch; the coordinator's floors
      (a worker too small passed over, a larger given it, rising only, alone over a batch's, an
      older way's an estimate that holds nothing, the OSM pass's earlier passes' put away, a
      member's done and give-back carrying them, a batch's floor under what a worker spares batched
      as any, a measure
      taking its place, kept across a restart, cleared by `/work/floors`, a take-up keeping the
      newer of a floor and a measure); the agent (a stale costs file gone at a job's start, the job
      beside drained, nothing started beside the larger in either slot, a drain's 15 minutes, a
      lone job's drain not stopped by them, a new job in a slot taking none of the last's samples, the
      larger drained, then stopped in trouble, not as a failure, kept from this Mac an hour, a
      batch's floor tried alone, a floor learned alone held here, left to a Mac with room, held
      everywhere when none has, unknown memory and the switch off guarding nothing).
   3. **Power, home and the in-use rule removed: built** (the owner's choice, §7.7): the `cpu`
      need and the battery's pause of CPU work, `Needs.home` and the wait for home (the OSM pass,
      its sets, the reach and the world's buildings run over Tailscale when away; the share's
      mounting by its LAN name at home and its bare name away kept), the second job's "only network
      work while the Mac is in use" rule with the list it read (the light steps) and what else read
      it (the first slot leaving network work to the second, a network second job's four threads
      and 10 GB, room made beside a network job, the forecast's half hour in use); the table's
      columns for them. A Mac's charge and home stay in its conditions for the status and for
      whether it may lead (`BATTERY_MIN`, the proactive offer: §6.5), not for any job. In place of
      the first slot leaving network work to the second, a general rule with no list: a slot whose
      next job only can't share the Mac with the other's (a group of the table; later, a lock at the
      resource) takes the next job in plan order that can start, the one passed over starting as
      soon as the clash ends (in the second slot too if it's one of the second job's steps, else
      in the first's), and waited for once passed over half an hour, the second then starting
      nothing new but it; started, or gone from the plan, it holds nothing up. The first slot's
      threads no longer halve while the owner is at the Mac. The forecast doesn't model the
      passing over (plan.md §10). No switch (each is a
      rule gone, the owner's). Tests: a job's conditions the NAS alone, at any charge,
      away or home; a record of an older app's job read as its NAS alone; a second job beside the
      first whoever is at the Mac, in the forecast too; the heritage chain beside, the first slot
      taking the map tiles rather than wait for the Wikidata facts, which start once it ends; one
      passed over half an hour waited for, the second starting nothing but it; one started in the
      second slot and ended past the half hour holding nothing up.
   4. **The lead's slots as any member's: built** (§7.6, phase 2's part of the agent): its jobs'
      leases renewed and ended through the lead wherever it is, in process while it leads and over
      HTTP while it's a member, so a change of part restarts nothing (its coordinator starts or
      stops: a coordinator stopped frees its port as its stop returns), and the lead stays awake while
      it leads with leases out (§6.7). A lead whose own update is pending (a newer app installed,
      its slot busy, a member on the newer app idle by the same-app rule: plan.md §8) hands the lead
      to such a member by itself, rather than leave the members idle while its job stops at its
      next safe point (up to the 15 min a pause gives; before task #210 the job ran to its end,
      hours during an OSM pass, and went on running after the handover, the Mac a member shown
      "app too old"). Every Mac's jobs drain so for a newer app, whatever its part; the lead grants
      a member on another app nothing (409), and that member asks for nothing. Behind `state/pool/slots` (`on`, or present; off while missing, until
      the both-Macs scratch run). Tests: two agents in one process (a member's job granted in term
      1 running while the lead is handed to its Mac, neither restarting, the new lead's coordinator
      up and the old one's stopped, the job's entry reaching the new lead's records and its lease
      ended there, then the old lead, a member, given a job by the new lead over HTTP and its entry
      merged); a lead on an older app whose job holds its update handing the lead to the member on
      the newer; a coordinator stopped freeing its port for the next. The simulator is unchanged
      (the driver is: the agent's part isn't the simulator's). **Scratch state (§13): both Macs**
      (with `state/pool/slots` made in the scratch folder), and as processes with SIGSTOP.
   5. **Disk measured, and the Mac shared by measure: built** (§7.7): first GC reading the journal (every
      entry not yet in the lead's records keeps the files it names), so it can run beside other
      jobs; each job's disk use counted by the accessors that write its files and learned per
      target as its disk floor; the disk guard (the job growing fastest frozen when the Mac runs
      short, room made from what no job holds, given back if none can be made); the OSM pass's
      clear of the pack cache through `store::cachefile`; a chaos test of two raw-tile jobs at
      once; then the run-alone, second-job and raw-tile lists removed: a job starts beside
      another only if both jobs' measured memory and disk fit what the Mac has left, a cold start
      alone. Learning needs no switch; the lists' removal, behind `state/pool/by-measure` at first, is
      always on since batch 13. Tests:
      a disk floor learned; a job frozen short of room and going on once it's made; a second job
      started or held by the measures alone; the chaos test's same bytes; GC keeping an unmerged
      entry's files. As built: GC reads the newest records and every journal entry they don't
      reflect (agent::gc::unmerged); the caches' accessor counts what a process adds, a step's cost
      line notes it per target, the sampler counts each job's disk (those bytes, the top of its
      scratch folder each minute; the disk's fall while it runs alone logged, not counted) and the floors reach the
      lead with the lease's end (`disks`); the disk guard freezes the job
      growing fastest in the sampler, the loop makes room (room::toward) and lets it go on or gives
      it back, kept from that Mac an hour; the second job is the plan's first in
      plan order that fits, the first slot waits for one that doesn't, beside another job, a
      job's disk room is the reserve and its measured disk (alone in the first slot, room is made
      for its measured disk too as the caches allow, but it waits only for the reserve and the
      target, the disk guard its backstop), and a cold start runs alone, nothing beside it; a job of no
      targets of its own (GC, a catalog, the OSM pass) is measured by its id. **Scratch state (§13):
      both Macs**: planned (§13).
   6. **Write-sets enforced where they're written: built** (§7.3, §7.7): one outputs function per
      step (`agent::steps::writes`, its row's), which `Out` writes by and the merge checks by, so
      they can't disagree; a job's `Out` refusing any other name (`Out::open` takes the step, as
      `scenic-build` starts one; `Out::unbound` is the merge's and tools': a put outside fails
      before it uploads, a set or removal outside fails the job's next save, and nothing of that
      save lands, though the targets it saved before stand; a step the table doesn't know held to
      nothing while it writes the manifest by hand, and to no change when it hands off); the steps'
      sweeps of their stale names (the labels, the water and the small water, the stations, the
      ferries, the overlays, the landmarks, the heritage chain and sites) removing only names their
      step's function allows (`Out::may_remove`), so a legacy or malformed name can't fail a step
      for good; the merge refusing every entry with a change outside it (`agent::pool::check`),
      but holding one from a member on a newer app (`check_from`, `records::HOLD`: neither merged
      nor refused, merged again every step until the lead runs that app, `pool.gates.held` in the
      status); the lint (`tests/write_sets.rs`, best-effort: a scan of the source for what makes or
      writes a hand-off or an entry, or names the manifest's file or the journal's folder; nothing
      does but `Out` and the listed paths of the merge, the readers and the raw tiles' archives)
      gating publish. As built, the function is a predicate over a name, not a type only it can
      construct (§7.7's design): a name outside fails the job as it's written, without a rewrite of
      every step's ~100 writes; a later batch may make it a type. No switch. Tests: a job's `Out`
      refusing a name outside its step's, nothing of that save landing; a stale sweep leaving a
      name its step's function doesn't give; the merge refusing what `Out` would, the entry within
      merged; an entry outside from a newer app held, then merged once the check takes it,
      acknowledged then; every entry the journal held (the first batch's real journal run); the
      input check's and the rail feeds' steps run through a bound `Out`; the lint and its markers; the
      simulator's runs with entries outside refused by every lead (by the same check,
      `steps::outside_by`, over the simulator's own outputs function); all 682 entries of the
      journal of 8–10 Oct 2026 within their write-sets (`real_journal_entries_are_within_their_write_sets`,
      on a copy). **Scratch state (§13): both Macs, run** (14 Oct 2026, the M4 leading, the M1 a
      member over Tailscale; the fake jobs handing off within their steps' outputs, `outside` one
      outside them): the M1's terrain and tree cover entries, its outbox and an older helper's
      hand-off merged into the lead's records; the lead's own entry with a name outside its step's
      refused, its why noted beside the journal, the lead merging the next jobs' entries; the real
      `scenic-build` on the M1, a pooled job writing outside its step, failing before anything
      reached the NAS.
   7. **Exclusivity where the resource is used: built** (§7.7), always on: each agent runs a proxy (crate::budget::proxy,
      127.0.0.1, a port of its own) and every job's connections out go through it (`HTTPS_PROXY`
      and the others in its environment, this Mac's own direct), the job run under `sandbox-exec`
      with a profile denying every other way out (crate::budget::SANDBOX); a connection to a
      metered host takes a token of the pool's one budget on the NAS first (crate::budget: leases
      made with create-new, a file a slot of a ten-second window, `state/pool/wiki/<class>/`;
      Wikimedia's sites four requests a second, the query services one in two seconds and two
      seconds of query time a token), a 429 told to it (dem/budget.py) holding every process and
      Mac, the NAS away giving each Mac the rate divided by the members heard lately (their
      heartbeats, as last read whole, not out of touch); no job starts
      without the proxy; the lint (`tests/budgets.rs`: every file of the jobs' code naming a metered
      host listed with how it asks, the Python that asks telling a 429, nothing turning the proxy
      off unlisted, dem/budget.py's hosts crate::budget's) gating publish. The heavy NAS reads
      measured two at once against one (only slowing each other: §7.7), so no accessor: the group
      dropped. The answers' lease (`state/pool/answers.lease`) around each sync and keep of a pass's
      answers, and this Mac's copies held by the steps that keep them, the agent's seed alone with
      writer preference (crate::answers::Lease, Local). The Wikidata and heavy NAS reads' groups,
      and the agent's hold on the steps keeping answers while it seeds them, gone. Tests: two
      processes, and two agents' proxies, sharing one budget on one folder, many clients at once,
      never past a window's tokens (a clock that moves only when waited on, `StepClock`: no real
      time); a 429 told to one proxy holding the other; a query's time charged; the NAS away
      dividing the rate; old leases removed; a sandboxed job denied around the proxy and metered
      through it; the agent's jobs given the proxy and sandboxed, the Wikidata steps side by side,
      no job without the proxy (`agent::tests::jobs_go_out_through_the_proxy_and_no_group_keeps_steps_apart`); the answers'
      lease one holder's, a lapsed one broken, a holder whose lease was broken unable to renew or
      remove the new one's; a sender waiting for the readers under way, a reader that starts
      meanwhile waiting for it; the lint. **Scratch state (§13): both Macs, run** (14 Oct 2026, the
      M4 leading, the M1 a member, `budgets`, `wiki`, `upstream`: the proxies opening every
      connection to a server on their own Mac, so nothing reached Wikimedia): both agents' proxies
      up, every fake job's requests through them, its try at the NAS's SMB port around the proxy
      denied by the sandbox; the windows both Macs leased in at once never past a window's slots
      between them; a 429 told to the M1's proxy (60 s) holding the M4's jobs (their next
      answer at the first window after the hold ended, a batch already leased spent before); two
      processes, one a Mac, taking the answers' lease five times each, holding it three seconds:
      no two holds overlapping, the second Mac waiting while the first held it.
   8. **Resuming, and planning from the records alone: built** (§7.2 Resuming, §7.4;
      crate::agent::resume), behind `state/pool/resume` (off while missing: the agent as it was). A resumable job's (the OSM pass's) progress noted as its job
      starts and ends in its member's file, `state/pool/progress/<member>.json` (one a member,
      principle 3); the lead not running a pass a member's live file holds, nor one a lease of its
      coordinator holds, its coordinator granting it to that member's worker alone
      (`offer_resume`, taken back when the switch is off), which resumes it in its own scratch
      folder; finished there, held unoffered until merged; a day from its first start there, or
      released (`scenic pool release`), the lead's to place; its job given this member's
      unacknowledged, unrefused hand-offs of its step (`SCENIC_UNMERGED`), which
      `Out::open` lays over the snapshot; a pass complete when the records name
      `sources/osm/<date>/pass` (`osmpass::latest_pass_by`; jobs told by `SCENIC_RESUME`). Tests:
      the decision (held for its Mac alone, a day, a release of that progress only, the newest of
      two, finished held until merged, a pass failing again and again held a day from its first
      start), a member asking only for what its file holds live, the files and releases, a refused
      entry left out of what's laid; the coordinator granting it to that worker alone, once, before the plan's
      work; `Out` laying the hand-offs; a pass stopped after its filter resumed with its filtered
      planet unmerged; a pass complete by its records, the same as by the files when they agree;
      and two agents: the lead not planning it, the member resuming it with the switch's
      environment, finished held, then a day on and a release giving it back to the lead, the
      a release and the day out while a lease holds it not planned, the switch off
      taking the offer back and planning as before. **Scratch state (§13): both Macs, run** (15 Oct 2026, the M4
      leading, the M1 a member, `resume`, `planet`, `progress`): the M1's file holding the pass
      (its filter marked done in its scratch folder, its agent restarted, as after a stop
      mid-stage), the lead waiting with "its progress is on MacBook-Pro-de-Brandon's disk … offered
      to it alone", its coordinator granting it to the M1, which ran it in its own scratch folder
      with its unmerged file; the lease ended at the lead, the entry merged, the pass complete by
      the records (the status's checklist 6 of 6), the M1's file then dropping it; the lead never
      started the pass. (A member reads its own file once a process: only it writes it.)
   9. **Every step to every member: built** (§7.2, §7.7; crate::agent::every, crate::coord's
      `pick_every`), behind `state/pool/every-step` at first, always on since batch 13. The lead's coordinator offers the plan's every step but its own
      (`every::LEAD_ONLY`, the steps keeping the pass's answers, the worldwide steps) to every member
      whose measured room fits the target's measures, the map tiles' `pack` and `lo` among them
      (each Mac's saves reach the records only through the journal; a map update's step granted only
      with the map update's units, `as_of`); a cold start only with no job of its step under way, to the
      member with the most room, the lead counted, alone; a
      job's targets as many as fit a quarter of an hour by their measured times (`pack` and `lo`
      now note their targets' costs); contiguous runs from the front of the plan, or just after the
      member's last; no claims. The pass's worldwide jobs stay the lead's (they
      aren't the plan's work; plan.md §10). Tests: a member offered `pack`, with the map update's units,
      and its entry merged, two agents in one process; a member
      short of a target's measured memory or disk offered none of it; a cold start alone on the
      largest asking, none while a job of its step is under way or the lead has more; a map update's step
      only with the map update's units, none with no map update; others' leases left out of the lead's plan
      (the coordinator's `held_by_others`); runs contiguous; batches by time, and one a job with
      none measured. **Scratch state (§13): both Macs, run in part** (15 Oct 2026, the M4
      leading, the M1 a member, `root`, `region`, `every`): the switch read on both, no claim file
      made; the lead running its pass's sets (a step alone, so no room free now), the region's
      terrain and tree cover pieces, never run, went to the M1 as cold starts, one target a job, and
      both entries were merged. The scratch region's plan reached no map update, so `pack` and `lo`
      between the Macs, runs of several targets and the lead taking a cold start itself were shown
      only by the tests (the scratch state's fake jobs build no units: planned, a scratch state
      whose records hold a map update's work).
   10. **Tasks brokered by every member, pages talking to members directly: built** (§8, §9;
      crate::agent::brokers, crate::coord::brokers), behind `state/pool/brokers` (off while missing:
      the agent as it was). A member's agent runs a coordinator of its own
      for its jobs' tasks alone (its broker, on the pool's port; `become_broker`: no plan offered,
      no job lease held), its jobs given its address (`SCENIC_COORD`), their tasks closed as they
      end; its heartbeat's `broker` says its addresses (and its HTTPS one, when `tailscale serve`
      proxies it) and its tasks waiting by kind (how many, the least memory), given at once when work
      comes (none waiting to some, ten seconds after its last change at least), any other change
      at most every half minute. Each agent reads the members' heartbeats every
      half minute and tells its coordinator or broker: every member (not the lead alone: a page's
      own member answers it) answers `/work/where` (the brokers whose work fits, most waiting first;
      the lead's address; every member's) and allows the members' own pages (CORS: the preflight
      for `Authorization`, `X-Worker`, `Range`, `PUT`, and a private network's ask; every answer
      allowed to that origin; any other page refused as before). With a part changing in its
      process (`state/pool/slots`), a handover drops no task: the old lead's coordinator stays on as
      its broker (the jobs' leases left to the new lead), and the member's broker becomes its
      coordinator, its tasks' leases kept as it takes up the term's. The page (web/work) asks its
      own member, then the brokers `/work/where` names, straight; runs a task against the broker it
      took it from; reads the build at a glance and the lead's controls at the lead's address,
      falling back to its own member's; passes over a member that doesn't answer in 8 s for two
      minutes; and after three failed calls to its own member moves to the next member that answers
      (once no task it took elsewhere runs). Tests: a page from another member's address taking a
      broker's task with CORS (and refused, switched off); the brokers' answer, the origins, the
      heartbeat's timing; a broker taking the lead keeping its tasks' leases (not, switched off);
      and two agents in one process: the member's job given its broker, the lead answering where
      with it, a page taking the member's task from its broker and the lead's from the lead, the lead
      handed over, both tasks beating and handed back where they were taken, then off.
      **Scratch state (§13): both Macs, run** (15 Oct 2026, `root … slots`, `region`, `brokers`,
      `task`, `jobs … 240`; a page played by curl): each job's task offered at its own Mac's
      coordinator or broker, the heartbeats saying so, `/work/where` naming them; the M4's page
      taking the lead's task, the lead handed to the M1 mid-task (the M4's coordinator kept on as its
      broker, the M1's broker its coordinator, two job leases handed over), the task then beaten
      and handed back at the M4's broker; the M1's task named by the M4's `/work/where` but its
      broker never answering over the tailnet (the M1's application firewall drops the connection:
      passed over after 8 s), taken on the M1 at 127.0.0.1 by a page of the M4's origin (CORS allowed)
      and handed back; switched off, the M4's broker stopped. A browser page across Macs over
      HTTPS: not run (no member has `tailscale serve` for the pool's port).
   11. **Placement by how crucial a job is, and work offered ahead: built** (§7.2, workers.md §3;
      crate::agent::placement), behind `state/pool/placement` (off while missing: the agent and its
      coordinator as they were; its order and draws of the plan's targets through `pick_every`). The lead tells its
      coordinator each loop each step's slack in its forecast (the build's end less the step's: the
      long poles none) and each worker its coordinator knows measured against the lead by the
      build's history, under the name it asks with (`placement::measured_speeds`: the shared steps
      both ran; the forecast's own machines are the members' status files, `state/helpers/`). A member's ask
      is tried on the offers by their steps' slack, least first (plan order between equals), and
      within a step from the target measured to take longest (its cost lines, `secs_of`; plan order
      between equals, those never measured last), its run contiguous from there as before; each
      such start is drawn (`placement::draw`: a stable hash of the step, the target and the minute's
      window since the coordinator was told, `DRAW_S`) among the members asking lately that could
      take it by their own asks (not held, done or failed by them lately, within the room their
      asks say), weighted by measured speed (one not measured, the lead's), the lead never among
      them, and a member passes by a
      target drawn to another; a cold start stays the most room's, undrawn. A page's or worker's
      task is drawn likewise among those that could take it, weighted by one over their measured
      pace at its kind (a member's broker draws its own jobs' tasks the same way, by the paces
      it measured; it has no history or forecast, which only order the plan's targets). Every job is told to offer ahead (`SCENIC_OFFER_AHEAD`): a terrain job
      offers its next piece's subtrees as it begins a piece, and each task of a terrain piece or a
      tree cover row says its head start (`ahead`: the tiles or blocks this Mac makes before it
      needs the task, over the task's own, so many tasks' worth of this Mac's time); the
      coordinator gives a worker measured slower than this Mac at a kind such a task only while its
      pace, with the margin, times this Mac's time at the kind (`Tasks::here_s`, weighed in from the
      jobs' own runs as they close their tasks) fits in the head start left; the job's waiting rule
      already counts the time a worker has held it. No rule names a Mac or a kind of job. Slope has
      no tasks yet (workers.md §3, planned), so nothing of it is offered ahead. The lead's own jobs
      take the plan as before (only the members' asks and the tasks are placed). Tests: the draws'
      shares by speed over many keys and repeating, in any order; each target drawn to exactly one
      of two members asking, three in four to the one three times as fast, the same in its window
      and drawn again in the next, the other no longer asking leaving any to the one asking; the
      long poles' step first, from its longest target; a page slower than this Mac given a task
      offered ahead only while the head start covers it, never one not offered ahead, and none
      switched off; and two agents in one process: the lead's coordinator told, the member drawn
      its target and its job told to offer ahead, then off. **Scratch state (§13): both Macs, run in part**
      (15 Oct 2026, the M4 leading, the M1 a member, `root`, `region`, `every`, `placement`,
      `jobs … 30`): the switch read on both, the region's terrain and tree cover pieces granted to
      the M1 (the only member asking, so drawn to it), every job on both Macs given
      `SCENIC_OFFER_AHEAD=1`; switched off, both agents said so. Draws between two members asking,
      the long poles' order across steps with measured times and a slower page given a task ahead
      were shown only by the tests (the scratch state's fake jobs offer no subtrees and the region
      reached no map update).
   12. **The forecast for equal machines: built** (§7.2; crate::agent::forecast::equal_machines):
      each member's slots as lanes, the pages as one, from the measures and the placement, as the
      lead's forecast's `equal`. Reports only: no switch. Tests: every step shared by an equal
      lane (the work before the regions and the units too), the map updates the first lane's
      alone, the pages' lane only the shared steps, the same input the same forecast and more
      lanes done no later; a lane given only what fits its memory, its running targets its own.
   13. **What goes: built** (the owner's choice: the pool always on). Gone: the switch
      (`state/pool/enabled` no longer read), `scenic pool on|off` and the shadow run
      (crate::agent::shadow, the `pool-shadow` command, `state/pool/shadow`); a process not in the pool
      (the NAS not reached as it starts, or its member's lock held by another process) starts
      nothing and restarts into the pool once it can. The claims (crate::agent::claims,
      `state/build/claims/`): every step to every member always on (`state/pool/every-step` no
      longer read). The named writer (`state/build/writer`), `check_writer`'s pass and
      `SCENIC_BUILD_MAC`: once `state/build/terms/` exists `check_writer` refuses every direct save
      (a folder with no term yet saves as today's files) and every job hands off
      (`SCENIC_HANDOFF`); term 1 made by the first Mac in the pool or by the owner's forced
      takeover (`term::bootstrap`, create-new, its first snapshot likewise). A Mac's own merging of
      hand-offs, re-keying (the agent's `rekey_records`) and records-writing outside the lead's
      merge; the NAS's hand-off folders (`state/build/handoff/`, crate::handoff's merge and
      folders), the drain of hand-offs from before the pool keeping the lead's coordinator's
      journal and a member's outbox, which otherwise holds its leased tasks alone. The Mac shared
      by measure and the budgets always on (`state/pool/by-measure`, `state/pool/budgets` no longer
      read; GC's "no term led" a term not readable whole), so the steps table's `disk` and groups
      columns (and its raw-tile, Wikidata, NAS-reading and run-alone sets) gone with the
      passed-over machinery (a slot's only clash: the other slot running the same unshared step). `install.sh` without
      `--agent` and `--helper`, and `scenic agent --helper` (accepted and ignored: every Mac runs
      the agent); `tools/app/publish.sh` from either Mac (a lock on the NAS, `app/publish.lock`;
      refused unless the tree is clean, the task board's database aside, and HEAD is on origin's
      main). The docs (workers.md, formats.md) rewritten around the pool.
      - **What stays:** the steps table's other columns (memory's first guess, shared and its
        rank, the second slot's ranks, the lead's own steps and those keeping answers, the batch
        of the lead's own jobs, the write-set: §7.2); the lead writes today's three files
        (`manifest.json`, `jobs.json`, `pending.json`) after each save for their readers, older
        apps among them. **Planned:** the records' readers reading the term's snapshot first
        (§6.2), and the lead no longer writing today's files once no reader of them is left.
      - **The transition:** an older app reads the switch files, so `state/pool/enabled` and the
        others stay on the NAS until no older app runs; never `scenic pool off` from an older app.
      - **Tests:** `agent::pool_tests::the_pool_is_always_on_with_no_switch_read`,
        `a_member_whose_lock_is_held_waits_for_it_then_restarts_into_the_pool`;
        `out::tests::once_the_pool_has_begun_no_step_saves_the_records`; rawpack's
        `once_the_pool_has_begun_only_a_jobs_hand_off_packs`; `pool::term`'s
        `term_one_is_the_first_macs_to_make`; `agent::tests::which_jobs_run_beside_each_other`,
        `jobs_go_out_through_the_proxy_and_no_group_keeps_steps_apart`,
        `plans_with_the_hand_offs_waiting_in_the_journal`,
        `the_lead_sends_the_passs_answers_to_the_nas_once_as_it_starts`; handoff's
        `a_folders_hand_offs_wait_in_order_and_a_damaged_one_is_set_aside`. **Scratch state
        (§13): both Macs** (the pool always on): planned.

   The memory guard's follow-up (batch 2: grants refused to a member on another app than the
   lead's, plan.md §8) ran the in-process harness (two agents in one process,
   `a_member_on_another_app_than_the_leads_gets_nothing_until_they_run_the_same`).

   Packing the raw terrain tiles a job fetched onto the NAS (`pack_raw_with`) stays a step of
   each terrain piece's job, its own tiles only, with no job of its own: it takes about 1 % of a
   piece's time (measured: plan.md §12).

Phases 1–2 are the ones that can lose work if they're wrong, and phase 4's batches 2, 4, 5, 6, 8
and 10; each is tested as §13 says before it's switched on: the in-process harness, then both Macs
on a scratch state where marked above, then the live switch.

## 13. Testing

- **The simulator** (built: crate::pool's `sim`, in the crate's tests; a seed decides everything, so
  a run repeats exactly). Two to four Macs, a thread each, take turns step by step (a NAS operation,
  a message; a look at a clock takes no time) and run the pool's driver, the code the agent will
  run, the agent's part around it played as the design has it (`sim::Mac`: its jobs' hand-offs; its
  coordinator's state, granted while the driver lets it and written when it settles; the listings
  the driver asks for, made off its step, one that failed made again; its heartbeat, stamped as it's
  written; a job reading the records; a re-assertion asked now and then, as before a sweep; the Mac
  able to lead nine loops in ten). The share's model: create-new atomic, its maker holding the file
  open until its bytes land a step later (the file empty meanwhile, for as long as its Mac sleeps in
  between), or never (the share gone between: its maker's to finish); a whole write's rename a step
  after its temporary file, landing when its Mac wakes, over whatever was written meanwhile; in some
  runs an entry another Mac renamed into place not there to a Mac's reads for up to ten minutes
  (directory caching), now and then an entry landing cut short for good, and now and then one whose
  every read errors for good; in some runs the share answering none of a Mac's reads, stats and
  listings for one to ten minutes, its writes landing; a rename over a file another Mac has open (a
  read holds it up to 8 s) failing busy, the write after four tries; a create or a whole write that
  did its work answering an error (its answer lost); each Mac's reads, stats and listings kept up to
  30 s (60 to 300 s in some runs), its own writes seen at once; a listing taking 3 to 33 s a folder
  in some runs, its Mac waiting on it, and in some runs now and then one its agent loses, never
  handed back; messages reaching only a Mac that's awake, the others dropped; clocks up to a second
  off (up to 20 minutes in some runs), and each Mac's awake clock stopping while it sleeps; in some
  runs every create, read, stat and whole write taking 1 to 4 s (the share under load), and now and
  then one the share stalls on for 5 to 10 minutes, the Mac waiting on it, awake. The schedule:
  sleeps after any step, mid-loop (5 s to 40 minutes, more often right after a temporary file or a
  create), the owner's asks (hand the lead over; take it over, forced at times with the lead alive,
  and by the owner's downgrade), newer apps (a restart: the Mac's memory gone but what its driver
  saved, and at times that too), development builds and rollbacks, a member leaving for good (now
  and then the lead itself), a handover now and then split (its target asleep or gone before taking
  up, its old lead restarted into a development build or asked to take over, forced), a week of the
  journal's earlier days that term 1's records lack, and jobs' entries (units built, some prunes),
  some of them refused, some by the leads of odd terms only (a check that depends on the lead's
  state). Each run draws these knobs from its seed.
- **What it checks,** at every step that could break one of §4's invariants: terms made in order,
  never written over (but by their maker finishing them), a created file unchanged between its
  create and its bytes, no term or records file ever removed, term 1's first snapshot written by its
  maker alone, self-consistent; terms by the app rule (a take-back's checked against the term handed
  over; not the owner's downgrade, nor a term forced past one that can't be read, whose maker
  checked it against the newest term it could read); a term led by the Mac it names, and by no
  other, no Mac leading a term twice nor an older term after it; no Mac's term going back, across
  its restarts too (but one that lost its saved state); a term's records written by its lead alone,
  self-consistent (as members read them too), never going back in `seq` or in the entries they name;
  an entry acknowledged only once a saved snapshot names it, and never removed; a heartbeat's beat
  its Mac's clock as it's written; a handover's new lead taking up with the coordinator's state its
  old lead settled with; a lead saying it's caught up only by a listing of every day asked for under
  a day before, after it last woke and, but for a re-assertion that kept what it knew, after its
  term began, its snapshot naming every entry written before it; an entry refused as never read
  whole only if it never was, as its lead's reads could see; a handover passed is over, taken back
  or dropped within fifteen minutes awake, and dropped by a forced takeover its own query says
  suffices; an automatic takeover only of a lead whose heartbeat said it stood down from the term,
  two minutes before at least. Then, the faults over and every Mac awake for 25 minutes, the owner
  taking over a term the views would show with no lead that no member takes by itself (its lead gone
  for good, or stood down where no member's app can lead the term, or the term unreadable): the last
  term's lead leads it, alone, caught up, no term was made in those 25 minutes' last ten or after,
  and its records name every entry ever written, a Mac's gone for good included. While they lack
  entries, fewer each time, or its lead isn't caught up yet (an entry never whole waits its hour, a
  listing its agent lost two), the run goes on ten minutes at a time, up to four hours: on a share
  taking seconds an operation a lead merges about a dozen entries a loop, and hours of faults with
  no lead leave hundreds; a listing lost as the faults end is asked for again two hours on, and an
  entry never whole that only it finds is refused an hour after. The tests run 2,000 seeds, each
  kind of change of lead and of fault, and what each knob brings, among them at least three times (a
  lead caught up on a step it re-asserted for a sweep, and the owner's takeover, too); entries cut
  short in four-hour runs, refused after an hour (200 schedules); runs of the faults' 40 minutes and
  a day after, a lead listing every day again daily (50); slow listings with a week of the journal,
  development builds and rollbacks with a Mac leaving, and a Mac leaving, alone (300 to 1,000 seeds
  each); the schedule that found term 1's first snapshot paired with today's files of two versions
  (seed 3090226), and the one that found whole entries waiting an hour behind others not whole
  (1003691); left out by default, a long run of 100,000 schedules of four hours' faults, each knob
  alone over 1,000 seeds, and 400 seeds run twice, the same; and the first draft's scheme (one
  shared records file, the journal emptied as it's merged) on the same model, which finds its lost
  update. The driver's own tests decide what the simulator can't: whom a member lets try first, that
  a lead asleep or gone isn't taken over by itself, nor one whose stand-down is an earlier term's, a
  step's minute of reads, the hour over a share that doesn't answer, a listing a day old, a listing
  slower than any timeout, a saved state restored from a backup, the member's lock.
- **What it doesn't model:** torn or holed reads (a file reads whole, empty or as an older version,
  and an entry cut short for good: the modules' tests read holes); I/O errors other than a busy
  rename, a create cut short, lost answers and failed reads; `remove` failing busy; the member's
  lock and `Out::stop` (its drivers have no lock: the driver's tests); a wall clock set while awake
  (a Mac's skew is fixed); a listing that takes hours, its journal grown (the driver's tests); a
  member's file lost, a new id made, and a restart from an older saved state of its own (the
  driver's tests); refusal notes (no check reads them); GC (removing the journal's old days,
  forgetting them: the forget horizon is the modules' tests'); the coordinator (leases are numbered
  in order per term, granted by no one, and the lead's check depends on its term alone).
- **The integration's tests** (phase 1; crate::agent::pool's, and the agent's `pool_tests`): the
  pool always on with no switch read; a member's step, its state saved before anything is done, its heartbeat, its listing made
  off its loop and a failed one made again; a second process of a member running no driver, and
  one whose lock was taken stopping with its hand-offs kept; two members' mail, entries,
  acknowledgements and a handover settled with the coordinator's state handed over; a job's entry,
  the jobs a process left, draining, the token and devices, the merge's checks, today's files and
  the history per writer; and the agent: leading with its jobs handing off under its term's
  leases and its sweep after a re-assertion, a member's job reaching the lead's records through
  the journal and its mail, the lead's gates on catalogs and sweeps, a member whose lock another
  process holds waiting for it, then restarting into the pool.
- **The controls' tests** (phase 3; crate::agent::lead's, and the agent's `pool_tests`): two
  members in one process, their clocks moved on by the test (`Side::ahead`): the lead handed over
  each way (a member's "Make This Mac Lead" passed on by mail, and the lead's own "Hand the Build
  To"); refused asks with their reasons (a Mac that isn't a member, the lead itself, one out of
  touch; a takeover unforced while the lead is in touch), none reaching the driver; an offer whose
  target never answers, given up after a minute; a pass not taken up, taken back after two; a
  takeover of a lead out of touch; the proactive offer on battery, taken by itself only switched on,
  after it stood five minutes, and never back within half an hour (the flap guard, also alone); an
  ask's state across a restart; heartbeats of the apps before and after the controls read by each
  other; the history's terms; and two agents, the owner's ask arriving while the member's job runs:
  handed over, the job's entry reaching the new lead's records, both restarting into their parts.
  The coordinator's `/work/lead` and the map server's `/api/build/lead` take a page's ask and
  refuse a forced one. **Two agents on one Mac as processes** (8 Oct 2026, a scratch folder, its
  build paused): a member's "Make This Mac Lead" by `scenic lead give`, handed over in about two
  minutes (the agents' loops and the lead's reading of its members' mail); handed back from the new
  lead; the build page's "Make lead"; and a takeover of a lead stopped (SIGSTOP) out of touch, by
  `scenic lead take`.
- **Two agents in one process** (the agent's tests): a lead and a member, each with its folder and
  member, the lead's coordinator on a port of its own. **Two agents on one Mac** as processes, with
  SIGSTOP and SIGCONT for sleep and fault points (`merge:before-rename=600s`,
  `handover:after-ready`, `takeup:after-copy`): planned.
- **Two Macs on the real NAS** (`tools/check/pool-two-macs.sh`): the real agents on both Macs, over
  a scratch folder of the NAS, each in a folder of its own, their jobs played by a script that
  hands off. Run on 2026-10-08: term 1 the M4's, both Macs' hand-offs from before the pool
  drained, the lead's jobs and a sweep, the member's entry merged and acknowledged by mail, the lead
  restarted re-asserting, then standing down on an older app and the member taking over by itself
  and leading. Run on 2026-10-14 for phase 4's seventh batch (§12, item 7: the shared Wikimedia
  budget, a 429's hold across the Macs, the answers' lease), and on 2026-10-15 for its eighth (§12,
  item 8: an OSM pass held by the member's progress, resumed there alone, complete by the records). Planned: phase 4's fifth batch (two jobs beside each other by measure, the disk guard freezing one and making room, GC beside a job; the script's `measure` step); the ◻ checks of §3 (create-new between two Macs contended, exclusive
  rename, how long a renamed-over file reads stale), a handover (phase 3's controls ask it; run on
  one Mac with two scratch agents: §13, the controls) and load. (One Mac mounting the share twice shares one SMB client cache, so it can't show stale
  reads between clients.)
- **Each batch with a switch end to end** on the scratch folder with both Macs before its switch
  is made on the real one: phase 1's as above.

## 14. Decisions

Taken (the owner may revise them):

- **Automatic takeover:** not of a lead that's gone or asleep (§6.5: it may come back); the
  proactive offer instead, one click, automatic only if the owner turns it on. **A lead that's awake
  and stood down** (its re-assertion refused by the app rule, its heartbeat saying so) has declared
  it won't lead: after two minutes, any member the app rule lets takes over by itself (§6.5).
- **`tailscale serve` on every Mac** (HTTPS for pages talking to members directly): the owner's to
  turn on, one command per Mac (`tailscale serve --bg 8090`, proxying the pool's port at the root;
  the agent finds it by `tailscale serve status --json` every five minutes and puts it in its
  heartbeat), and to let the agent take incoming connections (macOS's application firewall, on
  the M1 on 15 Oct 2026, drops them: its broker can't be reached from another Mac, and pages pass
  it over; task #198 makes that visible). Until then, a page opened over plain HTTP on the tailnet reaches
  every member (WireGuard encrypts it anyway); one opened over HTTPS reaches only members with HTTPS
  (browsers block plain HTTP from it), and takes their tasks alone.
- **Work order:** every slot from the front of the plan, in contiguous runs (none from the plan's
  far end).
