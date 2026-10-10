# The pool: any Mac can lead the build

Status: **phase 1 built and switched on** (since 8 Oct 2026: `crate::pool`, its driver and phase
2's transitions with it; the agent's part, crate::agent::pool: §12); **phase 3's controls built**
(crate::agent::lead: §11); a shadow run beside today's agents (crate::agent::shadow); **phase 4's
first two batches built** (the steps table, crate::agent::steps, its write-sets checked and reported
at merge; the memory guard, crate::agent::memguard: §7.2, §7.3, §12); the rest is planned. It replaces the fixed
"build Mac" and its "helpers" (plan.md §8, workers.md §8) with a pool of peer Macs, any number of
them, one of which leads the build at a time, and makes the browsers' pages workers of the same
standing, by one model of work. The lead can be handed to another Mac from any Mac's menu, the
worker page, the map's build panel or `scenic lead`, and taken by another Mac when it's gone.
Nothing about a Mac's role is fixed at install.

## 1. What changes, and why

Today the build Mac is chosen at install (`install.sh --agent` or `--helper`, written into the
agent's launch file) and everything else follows from that: only its agent plans, runs the
coordinator, writes the records and publishes; only its own jobs write the records directly; the
coordinator's state (its token, leases, costs, history, the helpers' hand-offs waiting to be
merged) is on its disk; the worker page is served from it, through a `tailscale serve` set up on it
alone; and the tasks the pages take are offered by its unit jobs only. Taking it away from home or
offline stops the build, strands what's on its disk, and moving the role means reinstalling both
Macs.

The pool separates two things that were one:

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
5. **Work goes where it fits.** Each step says what it needs (memory, a floor of it, disk, the home
   network, power) and what it writes; any member with that may run it, the lead included. The few
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
three rows, a job's progress is work in progress, as the principle allows; the coordinator's and
the lead's own state are on the build Mac's disk alone, the coordinator's until phase 1 (§12) puts
it on the NAS. The rest a new lead has from the NAS, or makes again.

| State | Its truth | On a Mac | A new lead |
| --- | --- | --- | --- |
| The records (manifest, job keys, pending uploads), what's built, the catalogs | the NAS: `state/build/`, the content-named files | copies: the mirror, the pack cache (`agent/cache/base/`), `agent/cache/blobs/`, scenic-build's of the summits and the z8 terrain | reads them there |
| Downloads: the planet, Planetiler's data, the canopy squares, AWS's raw terrain tiles, FABDEM, the leaf types, Overture's boxes, the pageview months, the rail feeds; and what today's build left, imported: the registers' snapshot, the DEM seed | the NAS: `sources/`, each fetched once | copies in its cache, filled from the NAS when missing (room-making empties the cheap ones) | nothing: they fill again |
| The Wikidata and Wikipedia answers of a pass (the items job's, the heritage chain's) | the NAS: `sources/items/<date>/answers.tar.zst`, `heritage-<id>.tar.zst` (`pipeline::answers`) | `agent/cache/items/`, `agent/cache/heritage-<date>-<id>/`, made one with the NAS's as a run starts and sent as it ends (a stopped run's at its next start; the build Mac's agent sends what the NAS lacks as it starts) | nothing: its first run takes them |
| What units keep for their later runs | the NAS: `cache/dem-units/`, `cache/scenic-units/` | none (its next unit job moves any there) | nothing |
| Made from the NAS's data and kept: the heritage chain's clip of the filtered planet (22 GB on the build Mac), the trains' stop pairs | made again from the NAS | its cache | its first runs make them again (the clip: an hour of osmium) |
| The Python steps' environment | the app's lock file (`dem/uv.lock`) | `app/<version>/dem/.venv`, which uv makes from it (from uv's cache, else PyPI) | its first Python step makes it |
| A job's progress: the OSM pass's stages, the buildings scan's parts (Overture's, ~40 GB), a rail-feeds run's zips not yet put, raw tiles not yet packed | the Mac running it, until the job puts its results on the NAS | its scratch folders and cache | planned: offered to that Mac alone (§7.2, Resuming); else done again |
| The coordinator's: the workers' token, leases, costs, the history, the hand-offs not yet merged, the pause (mirrored to `state/build/pause.json`) | the build Mac's disk alone (a helper's hand-off, the helper's until it's sent); with the pool on (§12), the NAS: `state/coord/` (the token, the state per term, the history per writer), the journal | `agent/coord/` (`journal/`: the hand-offs taken); a helper's `agent/outbox/` | with the pool on, loads the state a handover hands it, else the newest a term before has (§6.2, §7.5); off, none: pages ask again, costs are first guesses, unmerged hand-offs are built again |
| The lead's own: the round under way, its retries, the daily jobs' last runs | the build Mac's disk alone | `agent/round.json`, `agent/state.json` | not in the phases yet: the next round begins afresh, failed jobs may run again at once |

## 3. What the NAS gives us

The design rests on what the share (SMB 3, Synology, macOS clients) does and doesn't promise.
Checked: ✓; to check on this NAS between two Macs before phase 1 relies on it: ◻.

- ✓ **Create-new is atomic** on the server for one client (crate::agent::claims relies on it).
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
  loop: a lock file removed is taken again while no other process holds it; a check that can't
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
  upward), then each loop checks only whether `<E+1>.json` exists (`term::next`: a stat, never a
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
  maker that stopped for good, or lost its answer before its bytes landed, leaves the term
  unreadable for good: only the owner's forced takeover follows it (§6.5, `term::force`), which
  checks the app rule against the newest term that can be read and says so in its `how` ("forced
  past: term E unreadable").
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
  then built again, about an hour of the build Mac's, once a newer app leads), and refuses a
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
- **The coordinator's state** is per term too: `state/coord/term/<E>/{leases,costs,failed,trust,
  pause}.json`, copied at take-up (§7.5). A handover's new lead loads it as the old lead wrote it
  last, from the snapshot its term names (§6.4).

### 6.3 Asking

Any member asks the lead. Its menu ("Hand the Build To ▸", "Make This Mac Lead"), `scenic lead` and
the map's build panel ask their own Mac's agent (an ask file in its folder, `lead-request.json`,
crate::control::LeadRequest; the map through its server's `/api/build/lead`), the worker page the
agent of the Mac serving it (its coordinator's `/work/lead`); the agent takes the ask up at its next
loop (crate::agent::lead). It checks it as the driver's step would (`Driver::hand_to`,
`Driver::takeover`): one refused never reaches the driver, and its status says why. On the lead the
driver hands over (§6.4); on another member it passes the ask on to the lead by mail
(`Msg::HandTo`, §9: the pool's HTTP API, `POST /pool/lead`, is planned), and the lead checks it
again. A member that can't reach the lead can't hand anything over; it can only take over (§6.5).
An ask naming a Mac that isn't a live member (heartbeat beat within ten minutes, app new enough:
`Driver::hand_to`; the NAS reachable, planned, the heartbeat saying nothing of it yet) is refused,
saying why. Where an ask stands (refused, passed on, under way, done, came to nothing, with why)
is in the asking Mac's status, kept across its agent's restarts (`agent/pool/lead.json`), and every
control shows it.

### 6.4 Handing over (the lead is there): a state machine

States are in the two Macs' heartbeats and the terms; times are each Mac's own wall clock, read
after what they're compared with. The transitions are crate::pool::handover's, pure functions of
what the lead's step saw, driven by crate::pool::driver.

| State | Who writes what | Next |
| --- | --- | --- |
| **Leading** (A, term E) | | an ask to hand to B: **Offered** |
| **Offered** | A's heartbeat: `handing_to: B, offer, since` (the offer known by when it was made) | B answers this offer within 60 s: **Ready**; else A clears it: **Leading** |
| **Ready** | B checks its disk and app, then its heartbeat: `ready_for: E+1` and the offer it answers (an answer to an earlier offer answers no later one) | **Settling** |
| **Settling** | A stops granting jobs (asks are answered "the lead is moving; ask again in a moment"), keeps renewing the leases out and taking hand-offs into the journal; cancels its duties in flight (a catalog is killed: it writes once at the end; GC stops between folders); writes the coordinator's state, and saves its records with it, merging what's waiting | settled within 60 s of B's answer, B's answer standing: **Passed**; else **Leading** (a settle that ends later passes nothing) |
| **Passed** | A makes `terms/<E+1>.json` naming B, with `seq`: the number of the snapshot it saved settling (§6.1), and tells B of it. A is now a member; its own member API sends lead asks on to B | B takes up within 2 min (B tells A it leads, or A reads it in B's heartbeat, or E+1 has a snapshot): **Leading** (B, E+1); else **Taken back** |
| **Taken back** | A makes `terms/<E+2>.json` naming itself ("B didn't take up"), the app rule checked against term E (§6.1) | **Leading** (A, E+2) |

- **The agent's parts** (§12, built): settling stops grants (its coordinator answers asks "the
  lead is moving; ask again in a moment"), stops a catalog or a sweep in flight, and hands the
  coordinator's state to the step; the transitions themselves are the driver's. The owner's asks
  reach it (§6.3), so a handover starts only when someone asks, or by itself when the owner turned
  the proactive offer's switch on (§6.5).
- **Nothing running stops,** on any Mac: leases keep their ids (`<term>-<n>`, unique by
  construction) and deadlines (wall-clock times) in the state B loads, as A wrote it last (it's in
  the snapshot the term's `seq` names: crate::pool::records::Records::handed), so every job, A's
  included, renews with B and hands back to the journal as before.
- **A job that can't move** (the OSM pass, whose stages are on its Mac's disk) doesn't stop a
  handover: it's a member's job like any other, wherever it runs. The one exception is a job running
  in the lead's own process (none, once phase 2 is done: §12).
- **A lead restarting** (a new app) is a re-assertion (§6.6): it makes the next term naming itself
  and keeps the leases of the jobs it relaunches.
- **"No lead"** is a state the views show (a term whose lead is out of touch, or a pass not taken up
  and not taken back), with a one-click "Take it" (§6.5).

### 6.5 Taking over (the lead is gone)

When the lead is out of touch (its heartbeat's own beat more than ten minutes old by the reader's
clock; or, planned, it says it can't reach the NAS), or its heartbeat says it stood down from its
term (§6.6), the owner may have another member take the lead: "Take Over the Build…" on its menu,
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

### 6.6 Re-asserting, and stepping down

A lead's view can be old without its knowing: it slept, its clock was set, or the NAS didn't answer
for a while. So the lead **re-asserts** before it acts again (crate::pool::driver): it makes
`terms/<E+1>.json` naming itself, a create-new no stale read can fool, and takes it up from its own
records (one copy of its snapshot, about 3 MB). It does when, since its last loop or within it, its
wall clock and its awake clock moved more than a minute apart (the awake clock, Rust's `Instant`,
doesn't count sleep: it slept, or its clock was set, either way); when its last loop ran over five
minutes awake (the NAS stalled: on a share under load every operation can take seconds, so a loop's
length short of that says nothing); after a restart; and before every GC sweep. Time spent waiting
between loops, or listing the journal (done off the loop), isn't a gap. It keeps what it knew of the
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

Every loop it also checks `<E+1>.json`. Stepping down: it stops granting, planning, merging and its
duties, writes nothing more, and goes on as a member: its slots carry on (a lease the new lead knows
is renewed; one that lapsed is stopped, its done targets handed off). The history and its menu say
so: "No longer leading term 4: a later term exists; term 5 is MacBook-Pro-de-Brandon's (taken over
by MacBook-Pro-de-Brandon)", with when.

A write already past its last check when the Mac froze lands in its own term's files (§6.2), which
nobody reads: invariant 5.

### 6.7 Clocks and wakefulness

- **No NAS clock.** A file's mtime isn't one: a client sets the mtime it writes. Whether a member
  is out of touch compares its heartbeat's own
  `beat` time with the reader's clock, as helpers are read now; a member whose beat is over 60 s
  ahead is shown as "clock wrong". The Macs keep time by NTP to well under a second.
- **Nothing persisted as an `Instant`:** lease deadlines, failures' waits and workers' last-seen
  times are wall-clock times on the NAS, so they mean the same on the next lead.
- **The lead stays awake** (planned, §12) while it leads with leases out on mains power (an
  idle-sleep assertion, as jobs hold now), so no member stalls behind a lead that dozed off with no
  job of its own.

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
the second job's, the light, alone, raw-tile, Wikidata and NAS-reading ones, those keeping the pass's
answers) are made from it when the app is built, and a step's first guess of memory, disk, needs and
batch are read from its row; what depends on a target rather than its step (a terrain area's whole
run's disk, the memory a unit's piece or a 3D buildings tile is offered with) stays with the agent.
Its columns:

| Column | Meaning |
| --- | --- |
| needs | memory: a first guess per step until a target's own run says (predicted per target from its last run, crate::coord::Cost; units and candidates by their piece's size, terrain by its area's, the 3D buildings by their rows); disk: what a job starts with free on its Mac (the reserve, 30 GB; terrain's pieces and assemblies and the water 35; the OSM pass 80, less the pack cache it clears; an area's whole terrain run 55; a member's jobs 15, crate::agent's `helper_need`); home (whole-planet reads: the OSM pass, its missing sets, the reach, the world's buildings); power (CPU work: all but a catalog, a prune, GC and the backups) |
| shared | whether other members take its jobs, and its rank of preference among those that are (what later steps wait on first) |
| alone / beside | whether it runs alone on its Mac; whether a Mac's second job may be one of it, and its rank there; whether it mostly waits on the network (beside the first job while the Mac is in use too); the groups of which two never run at once on one Mac (they read the raw terrain tiles here; they ask Wikidata, each paced as if alone; they read gigabytes of the NAS's sources a target) |
| answered | it keeps the pass's Wikidata and Wikipedia answers (none starts while the agent sends them) |
| batch | how many targets a job takes (a few minutes to a quarter of an hour of work) |
| write-set | the logical names its jobs may add, change or remove in the manifest, as patterns of its targets' tiles and the pass's date, removals apart (the OSM pass removes older passes' entries; a prune only removes); a shared step's files for each target it did are kept apart too (`saves`: what its hand-off may change) |

The table only gathered constants the agent had; nothing new is built on its figures. Each column
is to be replaced by a measure, a lock where the resource is used, or the code that does the work,
and the table to go (§7.7); planned columns it once listed (a floor of memory, moves, groups across
the pool) aren't added to it.

Tree cover's rows (plan.md §6, Trees): `trees`, a piece per z6 tile (it may add, change or remove
that tile's hi packs of the three tree layers and its mid, `work/trees-mid/6-x-y`; a lease of the
scheme before pieces, a z3 tile's whole run, its lo pack and its z6 tiles' hi packs, for one
release), any member, four a job; `trees-lo`, an assembly per z3 tile (its lo packs of the three),
seconds, kept to the lead. Terrain's and slope's likewise (plan.md §6, Global-source layers):
`terrain` and `slope`, a piece per z6 tile (it may add, change or remove its layer's hi pack and its
mid, `work/terrain-mid/6-x-y` or `work/slope-mid/6-x-y`, a piece of a z6 tile the coverage has left
removing them; a lease of the scheme before, an area's whole run, its lo pack and its z6 tiles' hi
packs, for one release), any member, eight a job; `terrain-lo` and `slope-lo`, an assembly per z3
tile (it may add, change or remove its lo pack), kept to the lead, terrain-lo apart from the other
steps reading raw tiles. Their targets may lie outside the coverage (what it has left).

The 3D buildings' rows (docs/buildings3d.md §3.6): `bldprep`, per z6 tile (it may add, change or
remove `work/bld/6-x-y`), memory learned (0.3 GB and 160 B a row read until then), the NAS (up to
~3 GB of parquet read a tile), power, any member, eight a job, never two on one Mac; `bldtiles`, per
z6 tile (`layers/buildings/hi/6-x-y`), memory learned, power, any member, sixteen a job. Neither
needs home. `bld-fetch` (the sources, onto the NAS, outside the manifest: its write-set is empty) is
network work, the lead's.

What's planned of placement (phase 4, §12):

- **Every step offered to every member's slots** whose Mac's measured room fits the target's
  measures (§7.7: today a member takes the shared steps alone, and the lead the rest, the map
  tiles' `pack` and `lo` among them).
- **Order and locality.** A slot gets work as a contiguous run of targets (a slot walks a region in
  spatial order, as one Mac does now, so what one unit fetches serves the next), with a preference
  for work next to its Mac's last.
- **How crucial a job is** decides who takes it first, not plan order alone: each target scores by
  its slack in the forecast (the long poles highest), and the members' slots and the pages take the
  work on a sliding scale of their measured speed, as a weighted draw (a function of the target, the
  worker and the time, so a decision repeats), not by a rule singling out the fastest or slowest;
  no rule names a Mac or a kind of job.
- **A Mac shared by measure:** a job starts beside another only if both jobs' measured memory and
  disk fit what the Mac has left; a cold start runs alone (§7.7). The run-alone and second-job
  lists, and the rule of only network work while the Mac is in use, go.
- **Resuming.** A job interrupted with progress on its Mac's disk (the OSM pass's stages, a terrain
  area's raw tiles) is offered to that Mac only until the owner releases it, or for 24 hours.
  Which Mac holds which progress is on the NAS (`state/pool/progress/<step>.json`, written by that
  member), so a new lead knows.
- **The forecast** is rewritten for equal machines (each member's slots as lanes, the pages as one):
  a large piece of phase 4, not a detail.

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
  it), else at its next safe point, and only trouble stops it sooner (a long job of one target, an
  area's whole terrain run, may reach no safe point for hours). A job beside asked to stop at its
  next safe point that reaches none in the time a pause gives (15 minutes), frozen meanwhile by a
  pause or not, stops at once. A sample is kept only for the job it read: a job started in the slot
  meanwhile takes none of it. A job stopped is given back,
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
  ("left to a Mac with room"); one past every Mac's limit the lead knows from the members' asks, or a
  step only the lead runs, waits, the status saying so ("more than any Mac in the pool has"). A
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
  given the check: `records::Check`). The write-sets are checked and reported, not yet enforced: an
  entry the other checks pass with a change outside its step's write-set is merged, logged and kept
  in its lead's status (`pool.outside`, the newest twenty, and `outside_n`; `scenic pool status`
  says them), until a later batch of phase 4 derives the write-sets from the code that writes and
  refuses such entries as it lands (§7.7, §12; every entry in the journal from the pool's first two
  days is within today's). A refused entry is named in the records with why; once they're
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
- **The raw tiles' index:** members only add archives (as helpers do now); compacting an area's
  archives is a lead duty, or a "replace X and Y with Z" hand-off applied only while X and Y are
  still named. A member making room doesn't pack.
- **summaries.json** is a cache keyed by content: any writer, no fencing needed.

### 7.4 Planning reads only the records

- **Planning, and a pass's being complete, read only the merged records and `inputs/`:** a pass is
  complete when `sources/osm/<date>/pass` is in the records, not when a `pass.*.json` file exists;
  the catalog's `--ready` comes from the merged records, and a catalog runs only when nothing waits
  to be merged.
- **A job resuming its own work** (planned, phase 4) sees its earlier attempts: in hand-off mode
  `Out::open` lays this Mac's unmerged hand-offs for the same step over the snapshot (as
  `Keys::load_with` does for done records), so a pass restarted after sleep finds its earlier
  stages.

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
- **Going:** `state/build/writer`, `check_writer` and `SCENIC_BUILD_MAC` (phase 1: while the pool
  is on, no agent names the writer, no job carries `SCENIC_BUILD_MAC`, and `check_writer` refuses
  every save, so a step run by hand writes nothing, `scenic-build verify`'s none either: unverified
  uploads wait in the records; kept for the pool off); claims (leases alone,
  phase 4); every temporary name not from crate::whole.

### 7.6 The lead's own slots

They call the lead through one interface, re-resolved on every call: in process while this Mac
leads, over HTTP after a handover, so a job running across a handover just carries on. They use the
journal and validation like anyone's (none of today's shortcuts for the build Mac's own jobs), and
the in-process path never holds the coordinator's lock during NAS I/O.

### 7.7 What a step needs, by structure (planned)

The owner's rule for the rest of phase 4: nothing may depend on a figure kept by hand that can go
stale. A step's needs are **measured** on every run and learned per target, or **enforced where the
resource is used** (a lock or accessor the code takes as it opens it, as `store::cachefile` does for
the caches: plan.md §8, Room on the disk), or **derived from the code that does the work**, with a
lint that nothing goes around it. The bar for each mechanism: a declaration missing or wrong costs
speed at worst, never a wrong output, impoliteness to a third party, lost work or a refused good
result. The steps table (§7.2) then holds nothing the scheduler trusts; what's left of it goes
(§12, batch 13).

Each of its columns, and what replaces it:

| Column | Replaced by | Notes |
| --- | --- | --- |
| power (`cpu`) | nothing: removed (the owner's choice) | jobs run at any charge; `BATTERY_MIN`'s pause of CPU work goes with it |
| home | nothing: removed (the owner's choice) | the OSM pass, its sets, the reach and the world's buildings run over Tailscale away from home, slower; mounting the share (the LAN name at home, the bare name through Tailscale away) stays |
| memory | measured per target (the jobs' `peak_mb`; the memory guard's floors, §7.2): built | a target never run is a **cold start**: placed only where it can't hurt (below); the guard is the backstop |
| disk | measured per target, as memory is, without walking a job's folders (millions of raw tiles; Planetiler's sparse files): the bytes written through the accessors that write a job's files (`store::cachefile`'s, the scratch writers'), counted per job and target; the biggest writers, outside programs the accessors never see (Planetiler's temporary files, osmium's and extract's outputs, the Python steps'), by a cheap look every minute at the allocated sizes of the few largest files at the top of each slot's scratch folder (no walk of its tree); and the disk's free space falling while one job runs alone; kept as the target's disk floor; a **disk guard** at the point of use: when the Mac's free space falls below the reserve, the job growing fastest by those counts is frozen (not failed: its work kept), room is made from the caches no job holds (`store::cachefile`'s exclusive locks), and it goes on; if none can be made, it's given back, its disk floor learned | today's figure (30, 35, 55, 80 GB) goes; the reserve stays a policy of the Mac, not of a step; a write that bypasses the accessors is caught by their lint, as the caches' reads are |
| alone, beside and its ranks, light (in use) | removed (the owner's choice): a job starts beside another only if both jobs' **measured** memory and disk fit what the Mac has left; a cold start runs alone | what they protected besides memory and disk, each covered at the point of use: the OSM pass clearing the pack cache (`--clear`, a `remove_dir_all` today) goes through `store::cachefile`'s clear, which passes over files another job holds; GC beside other jobs: it removes only what no catalog or the manifest names and is older than two weeks (abandoned temporary files after two days), but a hand-off not merged yet (a member away more than two weeks, its entry unwritten or untold) names files neither reads, so GC reads the journal's entries too (every entry not yet in the lead's records keeps the files it names) before batch 5 lets it run beside other jobs; scratch folders are per slot already (`scratch-2/`); two jobs of one step on one Mac share no state but the caches (accessor) and the records (hand-offs per lease); Planetiler's thread pool beside another job only slows both (cores, not correctness). The "only network work while the Mac is in use" rule goes with them, without a replacement (the owner's choice); `idle_s` stays for a member sparing more memory while its owner is away |
| groups: raw tiles | `store::cachefile`, already: a raw tile is read whole under the accessor's lock, packing deletes only tiles no job holds (`try_remove`) and only once their archive is named on the NAS, and a tile gone is filled again from the archives: two jobs reading raw tiles at once are safe, at worst fetching a tile twice | to check by a chaos test (two terrain pieces of one area at once, packing between: the same bytes) before the group goes |
| groups: Wikidata | **one budget for the whole pool, always**, its proxy enforced by the Mac: each job run under a `sandbox-exec` profile that denies it every outbound connection but to the Mac itself (so a step that forgets the proxy fails at once, in the tests before publish, never reaches Wikimedia unmetered), (a Mac can't reliably know the address its requests leave from: a Tailscale exit node, IPv4 or IPv6; so stricter than one per address, and than the owner's "shared whenever unsure"), enforced at egress: every Wikimedia request, the Python steps' (several by `curl` in a subprocess: heritagewd.py, pageviews.py) and any Rust one's, goes through a proxy on the Mac (`HTTPS_PROXY` set for every job; metered per host on `CONNECT`) that takes its requests from a token bucket on the NAS (`state/pool/wiki.json`), leasing tokens in batches (the share takes 20 to 55 creates a second, fewer over Tailscale), the query service (WDQS) budgeted by query time as well as count; a `Retry-After` or a 429 seen by any process holds every process and Mac (written beside the bucket); with the NAS out of reach, each Mac takes the rate divided by the members it knows. The profile allows the NAS and the build's other hosts through the proxy too, metered only for Wikimedia | a missing budget can only make requests wait, never exceed the polite rate |
| groups: heavy NAS reads | an accessor for large reads of the NAS's sources with a concurrency budget across a Mac's processes (flock slots in the app's folder), with its lint; or, if measuring two `bldprep` at once shows they only slow each other, nothing (the group dropped) | to measure first: two at once against one, their read rates and wall time |
| answered | a lease at the point of use, across the Macs: sending the pass's answers to the NAS (`crate::answers`) takes a lease on the NAS (`state/pool/answers.lease`, create-new, renewed, lapsing as a job's does) with writer preference (a reader that starts while a sender waits waits for it; readers run for hours, so a sender never waits on new readers), the steps that read them a shared one | no list of the steps that keep answers; a lapsed lease costs a wait, never a torn answers file (the answers are written whole by temporary name) |
| shared and its rank | every step offered to every member (§7.2); the order by how crucial a target is (the forecast's slack, measured times) | |
| batch | measured: as many of a step's targets as fit a quarter of an hour by their measured times (a cold start: one target a job) | a wrong time makes a job longer or shorter, never wrong |
| write-set | derived from the code that writes, and enforced as it lands (the owner's choice: no soak): each step's output names are **produced** by one function of its targets and pass (`outputs(step, target, pass)`), and enforced by type: `Out` puts and removes only an output name, a type only that function can construct, so a step's code can't form one itself, a new kind of output can only come by changing that function, and the lead's merge checks every entry against the same function. A good result can't be refused: the writer can't name what the function doesn't give (the compiler refuses it), the every-step tests run each step through `Out`, and the lint (nothing writes the manifest, the hand-offs or the journal but through `Out`) gates publish. The step's removals of stale names under its layer, a prune's of the pruned step's names, and the OSM pass's of older passes' are the same function's. Writes outside the manifest: the raw tiles' archives (named by their area, checked as today: crate::rawpack::named_for), the answers (`crate::answers`' own names, under its lease) and the 3D buildings' sources (bld-fetch's, under `sources/`, which nothing reads by the manifest) are outside the records, so outside the merge; each is written whole under a name its own module makes, and the same lint covers their writers | entries of an app before it are checked by the merge alone, reported, until no member runs one |
| moves (planned) | derived: a job's progress on its Mac's disk is written through one accessor (the OSM pass's stages, a terrain area's raw tiles), which names the Mac holding it on the NAS (`state/pool/progress/<step>.json`) | |
| floor of memory (planned) | not built: what a target held (its measured floor, §7.2) covers it, the Planetiler heap among it | |

**A cold start** (a target no run has measured, of a step none of whose targets has): placed where it
can't hurt, by what's known of the Macs, never by a guess: alone, on the member with the most memory
and disk free now, the guard on; once a target of a step has run, the step's other targets are
offered by the largest measured of the step's targets until their own run says (a prior learned, not
kept by hand).

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
- **Pages talk to members directly,** over HTTPS (`tailscale serve` on each Mac, the owner's to turn
  on) with CORS: every member answers the pool's own pages (`Access-Control-Allow-Origin` for the
  members' addresses; preflight for `Authorization`, `X-Worker`, `Range` and `PUT`). No gateway
  forwards anything: no streaming proxy for uploads, no file crossing the network twice, no
  forwarding loops while Macs disagree about who leads. (Worth checking first: Tailscale Services, one
  name for several hosts, would give the iPad's home-screen app one address and no cross-site calls.)
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
the lead among them (`HandTo`, §6.3); jobs go out through the lead's coordinator as today's
helpers' do (`/work/*`), and the worker page's lead asks to it (`/work/lead`).

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
  (`stood_down`: §6.6). Written each loop it changes, at least every two minutes, its beat stamped
  as it's written. Read by member id (the ids are in the lead's status), not by listing.
- **The build's status,** written by the lead per term (`state/build/term/<E>/status.json`): the
  checklist, the forecast, the regions, what waits and why, the jobs that ended lately (any
  member's), the pause, the last catalog, the lead and its term, a handover under way.
- **Everywhere it's shown** (the menu bar on every Mac, the map's build panel, the worker page,
  `scenic lead` and `scenic status`): the pool, a line or card per member, the lead marked;
  out-of-touch members with when they were last heard from; a handover's states as they happen; "No
  lead" with "Take it". They read it from the agent's status (`pool.lead`: crate::agent::lead::View,
  made again every half minute, and at once after an ask or a change of lead): the term's lead,
  each member it knows read by id from its heartbeat (never listed), with its state (home on power,
  on battery, away, out of touch, clock wrong, stood down, app too old) and whether the lead can be
  handed to it and why not (`Driver::hand_to`), what a takeover from this Mac needs
  (`Driver::takeover`), "no lead" and why (the term unreadable, its lead's heartbeat missing, out of
  touch or stood down), the offer, the last ask and the last change of lead. The members' states
  come from their heartbeats' conditions (`conds`: home, mains power, battery, able to lead), which
  apps before phase 3 leave out ("conditions unknown"). The menu bar and the map show this Mac's
  agent's view (the map server's `/api/build` `pool`), the worker page the serving Mac's (in
  `/work/swarm`'s agent). Each Mac's map server keeps the app its own agent runs, and keeps its
  downloads' copies to 20 MB/s while the build Mac runs a job.
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
    a Mac that's away warns that its duties run slowly over Tailscale;
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

Each phase ships switched off behind `state/pool/enabled` (the agent behaves as today while it's
missing) and is switched on once every member's heartbeat shows the app that has it. The M1's launch
file passes `--helper` (install.sh), so the pool's app accepts it (and ignores it once enabled).

1. **Terms and records per term.**
   - **The core: built** (`crate::pool`, checked by the simulator: §13). Member ids (`member_id`,
     bound to the Mac); `terms/` and term 1 (`term`: made with create-new, naming the Mac
     `state/build/writer` names, its first snapshot first; a create whose bytes didn't land finished
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
     the module's doc. The agent runs it while the pool is on (below).
   - **The integration: built, switched on since 8 Oct 2026** (crate::agent::pool; the agent, crate::agent). While
     `state/pool/enabled` is missing the agent is as it was (one stat more a loop, and one a save's
     `check_writer`); a change of the switch, read so two loops in a row, restarts it, between
     jobs, into the other. Until the switch is known (the NAS not read yet), today's coordination
     (the writer named, hand-offs merged, re-keying) waits.
     - **A process's part** is the terms', decided as it starts (its member's first step): the lead
       plans and grants through its coordinator as the build Mac did; any other member works as a
       helper did, `--helper` or not. A member whose part changes (it took a term up, or stepped
       down) restarts into its new part once its first job's slot is free: phase 1's lead runs its
       own jobs in its process (phase 2 moves them out). A process whose member's lock another
       holds starts nothing, tries the lock each loop, and restarts into the pool once it's free;
       one the driver stops (its lock taken since) saves that step's state, starts nothing, and
       exits between jobs. A saved state naming terms when the NAS has none (an earlier time the
       pool was on, its files since moved aside) counts for nothing but its unwritten hand-offs.
     - **The loop:** the driver steps once a loop, after the loop's ended jobs are gathered and
       before the plan; its state (`Saved`) is written whole to the agent's folder whenever the
       step changed it, before anything the step said is done (a step whose state couldn't be
       written does nothing of it, its duties held); a job's folder stays until a saved state holds
       its hand-off.
     - **The listings** it asks for are made on a thread, one at a time, each handed back, a failed
       one made again a minute on; the members' heartbeats are listed the same way, every two
       minutes on the lead (whose mail it reads), ten on a member.
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
       a new key scheme while the pool is on builds its targets again).
     - **The merge's checks** (`agent::pool::check`): a done record of the entry's step naming
       targets, content names of their logical names, raw archives of their areas, and for a
       shared step the coordinator's check of a helper's hand-off (its done targets' files, its own
       uploads); not the lease's targets (a later lead may not know the lease). The steps'
       write-sets are checked beside them, and what lies outside reported, not refused (§7.3).
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
     - **Seeding and draining** when it's switched on: the workers' token copied to `state/coord/` by
       the first lead (create-new) and from there by every lead's coordinator before it starts; the coordinator's local `coord/journal/` (the lead's) and the NAS's
       `state/build/handoff/<host>/` (every two minutes) and a helper's `outbox/` (a folder with
       nothing to hand off removed) drained into the journal as its member's entries (term 0: from before the pool;
       an outbox's under its old lease, the others numbered from `agent::pool::DRAINED`, apart).
     - **Going while it's on:** the writer named, `SCENIC_BUILD_MAC`, `check_writer`'s pass (it
       refuses every save). Kept for the pool off.
   - **Switching it on:** `scenic pool on` (agent::pool::switch_on), once every agent runs the app
     that has phase 1. It checks the switch isn't on, none of the pool's files from an earlier
     time are there (`state/build/terms/`, `state/build/term/`, `state/build/lead.json`,
     `state/journal/`, `state/pool/members/`, `state/pool/mail/`, `state/coord/`),
     `state/build/writer` names a Mac (term 1's maker), every agent heard from in ten minutes
     runs this app, and none runs a job (a job started before the switch saves as before, which
     the pool refuses: `scenic pause` and wait; `--force` passes these two over); then makes
     `state/pool/enabled`. Each agent
     restarts into the pool between jobs: the build Mac makes term 1 from today's files, takes it
     up and leads (its status's `pool`: `role` lead, `term` 1), the M1 works as a member (its helper
     status's `pool`). `scenic pool status` says how it stands. The first catalog waits for the
     records to be caught up (a listing of every day merged), the first GC for a re-assertion.
   - **Switching it off:** `scenic pool off` (`switch_off`): once the lead is caught up, every
     member's `pool.unacked` is 0 (each entry in the lead's records) and no job runs (`scenic
     pause` first; `--force` passes these over, its unmerged work done again), it removes the
     switch; each agent restarts as before between jobs (the build Mac names the writer again and
     writes today's files, which hold the last lead's records). Run again once no agent's status
     says it's in the pool, it moves the pool's files aside to `state/pool-off/<day>-<time>/`, so
     a later switch-on begins afresh from today's files (each member's saved state then counts for
     nothing: above). A member's hand-off said journaled to a coordinator whose pool is off is
     journaled there, and merged as before.
   - **A shadow run** (crate::agent::shadow; `state/pool/shadow` on the agent, or `scenic
     pool-shadow` beside it): the driver beside today's coordination, shadowing this Mac's agent
     (the build Mac's by its coordinator's history, a helper's by its outbox), each of its jobs
     that ended an entry of the shadow's member; reading the build's records where today's files
     keep them, writing only under `state/pool-shadow/` (`agent::pool::Overlay`); its log says the
     terms, take-ups, merges, gates against the catalogs and GC the agent ran, and its records
     against the build's.
2. **Handing over and taking over.** The driver does them (§6.4 to §6.6: phase 1's core, built);
   the owner's asks reach the agent (phase 3); the agent's part, planned (phase 4's fourth batch,
   below): staying awake, the lead's own jobs moved out of its process into its slots (so nothing
   pins the lead: in phase 1 a lead
   handing over mid-job keeps running its job, which hands off to the journal under its lease, and
   restarts into a member once its slot is free).
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
   1. **The steps table, its write-sets reported: built** (crate::agent::steps; §7.2, §7.3). A row
      per step: memory's first guess, disk, power, home, shared and its rank, alone, beside and its
      rank, light, the groups never two on one Mac, the answers kept, the batch, the write-set. The
      step sets the agent and the coordinator test are made from it as the app is built (a set of
      the wrong size, two steps of one rank or a rank missing failing the build), in the orders the
      agent and the forecast walk them, and its tests check them, the rows' memory, disk, needs
      and batches, a row for every step the agent runs, and a shared step's saves within its
      write-set. The lead checks every entry it merges against its step's write-set and reports
      what lies outside, refusing nothing for it; every one of the 574 entries in the journal of
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
   3. **Power, home and the in-use rule removed** (the owner's choice, §7.7): the `cpu` need and
      the battery's pause of CPU work, `Needs.home` and the wait for home (the OSM pass, its sets,
      the reach and the world's buildings run over Tailscale when away; the share's mounting kept),
      the second job's "only network work while the Mac is in use" rule; the table's columns for
      them. Deletions, with their tests and docs; no switch (each is a rule gone, the owner's).
   4. **The lead's slots as any member's** (§7.6, phase 2's part of the agent): its jobs ask, beat
      and end through one client, in process while it leads and over HTTP after a handover, so a
      change of part restarts nothing (only its coordinator starts or stops), and the lead stays
      awake while it leads with leases out (§6.7). A lead whose own update is pending (a newer app
      installed, its slot busy, a member on the newer app idle by the same-app rule: plan.md §8)
      hands the lead to such a member by itself, rather than leave the members idle while its job
      runs (hours, during an OSM pass). Behind `state/pool/slots`. Tests: the
      simulator's runs with a handover while the lead's own job runs (its entry reaching the new
      lead's records, its lease renewed with it); two agents in one process, then as processes
      with SIGSTOP and the fault points of §13. **Scratch state (§13): both Macs.**
   5. **Disk measured, and the Mac shared by measure** (§7.7): first GC reading the journal (every
      entry not yet in the lead's records keeps the files it names), so it can run beside other
      jobs; each job's disk use counted by the accessors that write its files and learned per
      target as its disk floor; the disk guard (the job growing fastest frozen when the Mac runs
      short, room made from what no job holds, given back if none can be made); the OSM pass's
      clear of the pack cache through `store::cachefile`; a chaos test of two raw-tile jobs at
      once; then the run-alone, second-job and raw-tile lists removed: a job starts beside
      another only if both jobs' measured memory and disk fit what the Mac has left, a cold start
      alone. Learning needs no switch; the lists' removal is behind `state/pool/by-measure`. Tests:
      a disk floor learned; a job frozen short of room and going on once it's made; a second job
      started or held by the measures alone; the chaos test's same bytes; GC keeping an unmerged
      entry's files.
   6. **Write-sets from the code that writes** (§7.3, §7.7): one outputs function per step, `Out`
      refusing any other name in the job, the merge checking every entry against the same
      function, the lint gating publish; entries outside refused as it lands (the owner's choice:
      the writer and the check share one function, so they can't disagree, and no soak is needed).
      No switch. Tests: every step's real saves within its outputs (the
      journal's entries, and each step run in the tests); a name outside failing the save in the
      job; the merge refusing what `Out` would; the lint; the simulator's runs with entries outside
      refused by every lead. **Scratch state (§13): both Macs** (the merge's refusals are the
      journal's).
   7. **Exclusivity where the resource is used** (§7.7): the Wikimedia proxy with the pool's one
      budget on the NAS (tokens leased in batches, the query service by query time, a 429's wait
      shared, the rate divided by the members with the NAS away), every job's requests through it,
      and its lint gating publish; the heavy NAS reads measured two at once against one, then an
      accessor with a budget, or nothing; the answers' lease with writer preference; the Wikidata
      and NAS-reads lists removed. Behind `state/pool/budgets`. Tests: two processes, and two
      agents, sharing one budget, never past its rate; a 429 holding them all; the NAS away; the
      lint.
   8. **Resuming, and planning from the records alone** (§7.2 Resuming, §7.4): a job's progress on
      its Mac's disk written through one accessor that names the Mac on the NAS
      (`state/pool/progress/<step>.json`), offered to that Mac only for 24 hours or until the owner
      releases it; `Out::open` laying the Mac's unmerged hand-offs for its step over the snapshot; a
      pass complete when its records say so, not by `pass.*.json`. Behind `state/pool/resume`.
      Tests: an OSM pass stopped mid-stage resumed by its Mac from its stages, offered to no other,
      released after a day; plans the same from the records as from today's files. **Scratch state
      (§13): both Macs.**
   9. **Every step to every member** (§7.2, §7.7): the lead's coordinator offers every step to
      every member whose Mac's measured room fits the target's measures (a cold start to the
      member with the most room, alone), the map tiles' `pack` and `lo` among them (each Mac's saves
      reach the records only through the journal); a job's targets as many as fit a quarter of an
      hour by their measured times; work in contiguous runs from the front of the plan, near the
      Mac's last; claims no longer read with the pool on. Behind `state/pool/every-step`. Tests: a
      member offered `pack` and its entry merged; a member short of a target's measures offered
      none of it; a cold start placed alone on the largest; runs contiguous; batches by time.
      **Scratch state (§13): both Macs.**
   10. **Tasks brokered by every member, pages talking to members directly** (§8, §9): each member's
      coordinator brokers its own jobs' tasks, its heartbeat saying how many wait; the lead answers
      "where is work"; every member serves the page and the pool's API with CORS; a page finds the
      members from the pool and reloads from the next when its own goes. Behind `state/pool/brokers`
      (and each member's `tailscale serve`, the owner's: §14). Tests: a member's job's task taken by
      a page from that member; a handover dropping no task; the preflight and the headers.
      **Scratch state (§13): both Macs, and a page** (tasks across a handover).
   11. **Placement by how crucial a job is, and work offered ahead** (§7.2, workers.md §3): each
      target's score from the forecast's slack, the long poles highest, by measured times; members'
      slots and pages chosen on a sliding scale of measured speed, as a weighted draw that repeats
      for the same inputs; no rule naming a Mac or a kind of job; a job's tasks offered as it
      begins a piece (`OFFER_AHEAD`), a worker waited for while its pace times this Mac's time is
      less than its head start, for terrain's subtrees, tree cover's rows and slope alike. Behind
      `state/pool/placement`. Tests: the draws' shares by speed over many targets; the long poles
      first; a worker slower than this Mac given a piece's tasks only with a head start that
      covers it. **Scratch state (§13): both Macs** (grants).
   12. **The forecast for equal machines** (§7.2): each member's slots as lanes, the pages as one,
      from the measures and the placement. Reports only: no switch.
   13. **What goes:** the steps table (its last columns replaced: §7.7), the claims
      (crate::agent::claims, `state/build/claims/`), once no app that reads them runs; `install.sh`
      without `--agent` and `--helper` (a launch file passing them still accepted and ignored); the
      agent without the pool, the owner's choice: `scenic pool off` and the switch
      `state/pool/enabled` (the pool always on), and everything kept for the pool off: the writer
      named (`state/build/writer`), `check_writer`'s pass, `SCENIC_BUILD_MAC`, the build Mac's own
      merging of hand-offs, re-keying and records-writing, the helpers' coordination (`--helper`'s
      outbox, the NAS's hand-off folders), and today's three files as the records' truth (the
      records' readers read the term's snapshot first, §6.2, and the lead stops writing today's
      files once no reader of them is left); the docs (plan.md §8, workers.md, formats.md)
      rewritten around the pool. **Scratch state (§13): both Macs** (the pool always on).

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
  switch; a member's step, its state saved before anything is done, its heartbeat, its listing made
  off its loop and a failed one made again; a second process of a member running no driver, and
  one whose lock was taken stopping with its hand-offs kept; two members' mail, entries,
  acknowledgements and a handover settled with the coordinator's state handed over; a job's entry,
  the jobs a process left, draining, the token and devices, the merge's checks, today's files and
  the history per writer; a shadow run writing nothing of the real folder; and the agent: off as it
  was, the switch changing restarting it, on the build Mac leading with its jobs handing off under
  its term's leases and its sweep after a re-assertion, a member's job reaching the lead's records
  through the journal and its mail, the lead's gates on catalogs and sweeps, the shadow beside it.
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
  hands off. Run on 2026-10-08: term 1 the build Mac's, both Macs' hand-offs from before the pool
  drained, the lead's jobs and a sweep, the member's entry merged and acknowledged by mail, the lead
  restarted re-asserting, then standing down on an older app and the member taking over by itself
  and leading. Planned: the ◻ checks of §3 (create-new between two Macs contended, exclusive
  rename, how long a renamed-over file reads stale), a handover (phase 3's controls ask it; run on
  one Mac with two scratch agents: §13, the controls) and load. (One Mac mounting the share twice shares one SMB client cache, so it can't show stale
  reads between clients.)
- **A shadow run on the real NAS** (§12), beside both Macs' agents, before the switch: what the
  pool decides against what the agents did.
- **Each phase end to end** on the scratch folder with both Macs before `state/pool/enabled` is
  made on the real one: phase 1's as above.

## 14. Decisions

Taken (the owner may revise them):

- **Automatic takeover:** not of a lead that's gone or asleep (§6.5: it may come back); the
  proactive offer instead, one click, automatic only if the owner turns it on. **A lead that's awake
  and stood down** (its re-assertion refused by the app rule, its heartbeat saying so) has declared
  it won't lead: after two minutes, any member the app rule lets takes over by itself (§6.5).
- **`tailscale serve` on every Mac** (HTTPS for pages talking to members directly): the owner's to
  turn on, one command per Mac. Until then, a page opened over plain HTTP on the tailnet reaches
  every member (WireGuard encrypts it anyway); one opened over HTTPS reaches only members with HTTPS
  (browsers block plain HTTP from it), and takes their tasks alone.
- **Work order:** every slot from the front of the plan, in contiguous runs, in place of today's
  helpers-from-the-far-end.
