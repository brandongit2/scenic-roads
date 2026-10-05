# The pool: any Mac can lead the build

Design, not built yet (2026-10-05; revised after an architect's review the same day). It replaces
the fixed "build Mac" and its "helpers" (plan.md §8, workers.md §8) with a pool of peer Macs, any
number of them, one of which leads the build at a time, and makes the browsers' pages workers of
the same standing, by one model of work. The lead can be handed to another Mac from any Mac's menu,
the worker page, the map's build panel or `scenic lead`, and taken by another Mac when it's gone.
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
   files), or is made once with create-new and never changed (terms, journal entries), or belongs
   to one term (the records, the coordinator's state). A late write from a Mac that slept through a
   change lands where no one reads; it's never needed to be refused.
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

## 3. What the NAS gives us

The design rests on what the share (SMB 3, Synology, macOS clients) does and doesn't promise.
Checked: ✓; to check on this NAS between two Macs before phase 1 relies on it: ◻.

- ✓ **Create-new is atomic** on the server for one client (crate::agent::claims relies on it).
  ◻ Between two Macs: two creates of one name at once, one wins.
- **A rename can be delayed indefinitely:** a Mac put to sleep between writing a temporary file and
  renaming it renames when it wakes, hours later, whatever happened meanwhile.
- **Reads can be stale:** macOS caches attributes and directory listings on SMB shares; a file
  renamed over on one Mac may read as before on the other for a while. ◻ How long.
- ✓ **Renaming over a file another Mac has open fails** (EBUSY on this share: crate::agent's
  heartbeat already retries).
- **No conditional writes and no atomic appends** between clients: "check, then write" is two
  steps, and an append at a cached end of file overwrites.
- ◻ **Exclusive rename** (store::naming's catalog numbering): server-side, or check-then-rename.
- ✓ **Listings are slow:** 3 to 33 s a folder under load. So nothing lists a folder in a loop: the
  current term is found by checking for the next (§6.1), heartbeats are read by member id, and the
  journal is listed once, at take-up.

## 4. Invariants

What any order of events, sleeps and stale reads must keep. Each protocol below is argued against
them, and the simulator (§13) checks them:

1. **One lead per term.** Terms are made with create-new (§6.1): two Macs can't both make term E.
2. **The term never goes back.** A Mac leads term E only while `terms/<E+1>.json` doesn't exist, and
   it checks that before acting on its view of the build (§6.6). Nobody writes a lower term.
3. **No accepted hand-off is lost.** A job's hand-off is in the journal before it's accepted (§7.3),
   the journal is a log nothing deletes while a term may need it, and a new lead replays every
   entry its snapshot doesn't name (§6.2).
4. **Each records snapshot is self-consistent.** The manifest, keys and pending uploads are one file
   per term, written whole: a reader never pairs keys from one version with a manifest of another.
5. **A stale lead's writes are ignored, not refused.** They go to its own term's files, which
   nobody reads once a later term exists.

## 5. Roles

- **A member** is any Mac running the agent with the NAS's project folder. Its id (`m-<16 hex>`)
  is made once, kept in its home folder, and named in everything it writes; its host name is a
  label (renaming a Mac, or macOS adding "-2" after a clash, changes nothing). It:
  - writes its heartbeat, `state/pool/members/<id>.json` (§10);
  - runs job slots (§7) that ask the lead for jobs, run them and hand their results to the journal;
  - brokers its own jobs' tasks (§8), and runs tasks itself in a slot when it has room;
  - answers the pool's API (§9): its own tasks, its status, the worker page;
  - passes its menu's, `scenic`'s and the map's asks (pause, lead) to the lead over HTTP;
  - takes the lead when it's handed it, or when the owner has it take over (§6).
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

A term is a file made once with create-new and never changed: `state/build/terms/<E>.json`.

```json
{ "term": 12, "member": "m-3f9c…", "host": "Brandons-MacBook-Pro", "app": "20261012-0910-1a2b3c4",
  "since": 1791300000, "how": "handed over by MacBook-Pro-de-Brandon (the menu bar)",
  "from": 11 }
```

- **The current term is the highest that exists.** A Mac learns it by listing `terms/` once (at
  start, at take-up), then each loop checks only whether `<E+1>.json` exists (a stat, never a
  listing). `state/build/lead.json` is a hint the lead writes after making its term (for old apps,
  and to save a listing), never the truth.
- **Making term E+1** is how every change of lead happens: a handover (§6.4), a take-back, a
  takeover (§6.5), a re-assertion (§6.6). Exactly one Mac's create succeeds; the others see the file
  and stand down. No lock is needed (`lead.lock` and the records' lock of the first draft are gone).
- **The app rule.** A term records its lead's app. A Mac makes a term only with an app at least as
  new as the term before's ("update first": job keys include the steps' versions, so a lead on an
  older app would take everything a newer one built as stale and build it again). A lead that sees
  a newer app installed restarts into it (as now) and re-asserts (§6.6).

### 6.2 Records per term

- **One file per term:** `state/build/term/<E>/records.json` holds the manifest, the keys, the
  pending uploads, the raw store's index, the journal entries it reflects (§7.3), and a sequence
  number. The lead writes it whole (crate::whole: its own temporary name, renamed over, EBUSY
  retried) after each merge. Today's three files are three renames a reader can see half done
  (`handoff.rs`); one file can't be.
- **Term 1 is today's layout** (`state/build/manifest.json`, `jobs.json`, `pending.json`), read and
  written as now, so nothing moves at migration (§12).
- **Taking up term E+1:** the new lead copies term E's snapshot (about 3 MB), then replays every
  journal entry it doesn't name, in (term, lease) order, and writes `term/<E+1>/records.json`. A
  stale read of term E only delays an entry to the replay; it can't lose one.
- **Readers** (jobs, the map server, catalogs) read the current term's snapshot. Content names never
  change, so a slightly old snapshot is fine for a job's inputs.
- **What a stale lead does** after a later term exists: its renames land in `term/<E>/`, which no
  one reads again; its journal reads and duties stop at its next check (§6.6). The lost-update path
  of the first draft (a late rename of a manifest over the new lead's, healed from a journal that
  had already dropped the entry) can't happen: there's no shared file to rename over.
- **The coordinator's state** is per term too: `state/coord/term/<E>/{leases,costs,failed,trust,
  pause}.json`, copied at take-up (§7.5).

### 6.3 Asking

Any member asks the lead over HTTP (§9): `POST /pool/lead {to, by, how}`. Its menu ("Hand the Build
To ▸", "Make This Mac Lead"), `scenic lead`, the map's build panel and the worker page all send one,
through their own member. A member that can't reach the lead can't hand anything over; it can only
take over (§6.5). An ask naming a Mac that isn't a live member (heartbeat beat within ten minutes,
NAS reachable, app new enough) is refused, saying why.

### 6.4 Handing over (the lead is there): a state machine

States are in the two Macs' heartbeats and the terms; times are each Mac's own wall clock.

| State | Who writes what | Next |
| --- | --- | --- |
| **Leading** (A, term E) | | an ask to hand to B: **Offered** |
| **Offered** | A's heartbeat: `handing_to: B, since` | B answers within 60 s: **Ready**; else A clears it: **Leading** |
| **Ready** | B loads term E's records and coordinator state read-only, checks its disk and app, then its heartbeat: `ready_for: E+1` | **Settling** |
| **Settling** | A stops granting jobs (asks are answered "the lead is moving; ask again in a moment"), keeps renewing the leases out and taking hand-offs into the journal; merges what's waiting; cancels its duties in flight (a catalog is killed: it writes once at the end; GC stops between folders); writes `state/coord/term/<E>/` | within 60 s: **Passed** |
| **Passed** | A makes `terms/<E+1>.json` naming B. A is now a member; its own member API sends lead asks on to B | B takes up within 2 min: **Leading** (B, E+1); else **Taken back** |
| **Taken back** | A makes `terms/<E+2>.json` naming itself ("B didn't take up") | **Leading** (A, E+2) |

- **Nothing running stops,** on any Mac: leases keep their ids (`<term>-<n>`, unique by
  construction) and deadlines (wall-clock times) in the state B loads, so every job, A's included,
  renews with B and hands back to the journal as before.
- **A job that can't move** (the OSM pass, whose stages are on its Mac's disk) doesn't stop a
  handover: it's a member's job like any other, wherever it runs. The one exception is a job running
  in the lead's own process (none, once phase 2 is done: §12).
- **A lead restarting** (a new app) is a re-assertion (§6.6): it makes the next term naming itself
  and keeps the leases of the jobs it relaunches.
- **"No lead"** is a state the views show (a term whose lead is out of touch, or a pass not taken up
  and not taken back), with a one-click "Take it" (§6.5).

### 6.5 Taking over (the lead is gone)

When the lead is out of touch (its heartbeat's own beat more than ten minutes old by the reader's
clock, or it says it can't reach the NAS), the owner may have another member take the lead: "Take
Over the Build…" on its menu, which says since when the lead's been gone and asks to confirm, the
worker page's "Take it", or `scenic lead take`. The member makes `terms/<E+1>.json` naming itself
and takes up as in §6.2. `--force` does it without the out-of-touch check (the owner knows the lead
is off).

What the old lead had in flight: its jobs' hand-offs are in the journal and get replayed; leases
its slots held lapse after their ten minutes and go back out (a lapsed lease's hand-off that comes
in later is still taken if its targets weren't leased again since); its coordinator's last changes
(costs, failures of the last minutes) may be lost; no built work is.

Automatic takeover is left out for now (the owner asked for buttons; a lead that's only asleep would
lose the role every night). In its place, **a proactive offer:** when the lead leaves home or goes on
battery while another member is home on power, every member's menu offers "Hand the build to <Mac>"
in one click (automatic if the owner turns it on).

### 6.6 Re-asserting, and stepping down

A lead's view can be old without its knowing: it slept (Rust's `Instant` doesn't count sleep), the
NAS didn't answer for a while, or a loop took minutes. So after any gap (asleep, NAS silent for over
a minute, a loop over a minute) and before every GC sweep, the lead **re-asserts**: it makes
`terms/<E+1>.json` naming itself (one copy of its records snapshot, about 3 MB). If that file already
exists, someone took over: it **steps down** at once.

Every loop it also checks `<E+1>.json`. Stepping down: it stops granting, planning, merging and its
duties, writes nothing more, and goes on as a member: its slots carry on (a lease the new lead knows
is renewed; one that lapsed is stopped, its done targets handed off). The history and its menu say so
("No longer leading: MacBook-Pro-de-Brandon took over at 14:12 while this Mac was asleep").

A write already past its last check when the Mac froze lands in its own term's files (§6.2), which
nobody reads: invariant 5.

### 6.7 Clocks and wakefulness

- **No NAS clock.** The first draft's "touch a file, read its mtime" measured the reader's own clock
  (a client sets the mtime it writes). Whether a member is out of touch compares its heartbeat's own
  `beat` time with the reader's clock, as helpers are read now; a member whose beat is over 60 s
  ahead is shown as "clock wrong". The Macs keep time by NTP to well under a second.
- **Nothing persisted as an `Instant`:** lease deadlines, failures' waits and workers' last-seen
  times are wall-clock times on the NAS, so they mean the same on the next lead.
- **The lead stays awake** while it leads with leases out on mains power (an idle-sleep assertion,
  as jobs hold now), so no member stalls behind a lead that dozed off with no job of its own.

## 7. The work: slots on every member

### 7.1 Slots and asks

- **Slots.** Each member runs as many job slots as its resources allow (two on a Mac with 32 GB or
  more, one on less; the second only beside jobs it can run with: crate::agent::SECOND), and a slot
  takes tasks (§8) when no job fits it.
- **An ask says what the slot can take now:** the memory it spares (its Mac's free memory less what
  its other slots' jobs are predicted to take), its disk's free space, its cores, whether its Mac is
  at home, on power, in use, its app and its member id. The lead answers with a job: a step's
  targets with their keys, in plan order, that fit.

### 7.2 Placement: the steps table

One table, crate::agent::build::STEPS, has a row per step with:

| Column | Meaning |
| --- | --- |
| needs | memory (predicted per target from its last run, crate::coord::Cost), a **floor** whatever the measurement (the OSM pass runs Planetiler with 24 GB of heap: 32 GB Macs only), disk (the OSM pass 80 GB, a terrain run 55, tree cover 30, others 15), home (whole-planet reads), power |
| write-set | the names it may add, change or remove, as patterns by target and pass date (§7.3) |
| moves | whether a job of it can run anywhere (most), or resumes on the Mac holding its progress |
| alone / beside | what may run with it on one Mac (today's SECOND, LIGHT, ALONE, RAW and WIKI sets) and across the pool (one Wikidata step at a time: one address at home) |
| batch | how much a job takes (about fifteen minutes) |

- **Order and locality.** A slot gets the first work in plan order that fits it, as a contiguous run
  of targets (a slot walks a region in spatial order, as one Mac does now, so what one unit fetches
  serves the next), with a preference for work next to its Mac's last.
- **A job that runs alone** reserves one Mac, the first eligible to come free; only that Mac drains
  for it. Others carry on.
- **Per-Mac rules kept:** network work goes to the second slot; only light work runs while the Mac
  is in use (plan.md §8, Two jobs at once).
- **Resuming.** A job interrupted with progress on its Mac's disk (the OSM pass's stages, a terrain
  area's raw tiles) is offered to that Mac only until the owner releases it, or for 24 hours.
  Which Mac holds which progress is on the NAS (`state/pool/progress/<step>.json`, written by that
  member), so a new lead knows.
- **The forecast** is rewritten for equal machines (each member's slots as lanes, the pages as one):
  a large piece of phase 4, not a detail.

### 7.3 Results: the journal

- **A job's hand-off goes straight to the NAS:** the member writes
  `state/journal/<day>/<term>-<lease>.json` itself (create-new, written whole), then tells the lead.
  That removes the outbox and its retry loop and the HTTP body limit; a takeover finds every result
  on the NAS; GC sees them all.
- **The lead validates at merge time:** every change in the step's write-set (STEPS), every content
  name matching its logical name, the lease's targets and keys. A refused entry moves to
  `state/journal/rejected/`, with why.
- **The journal is a log, not a queue:** nothing is removed on the merge path; a records snapshot
  names the entries it reflects; GC removes day folders older than a week once every snapshot since
  names their entries.
- **The lead lists the journal only at take-up;** after that it knows what it accepted (members tell
  it), and it lists a day folder only if a member says an entry's waiting that it hasn't seen.
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
- **A job resuming its own work** sees its earlier attempts: in hand-off mode `Out::open` lays this
  Mac's unmerged hand-offs for the same step over the snapshot (as `Keys::load_with` does for done
  records), so a pass restarted after sleep finds its earlier stages.

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
- **Going:** `state/build/writer`, `check_writer` and `SCENIC_BUILD_MAC` (phase 1); claims (leases
  alone, phase 4); every temporary name not from crate::whole.

### 7.6 The lead's own slots

They call the lead through one interface, re-resolved on every call: in process while this Mac
leads, over HTTP after a handover, so a job running across a handover just carries on. They use the
journal and validation like anyone's (none of today's shortcuts for the build Mac's own jobs), and
the in-process path never holds the coordinator's lock during NAS I/O.

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
  answers, its token with it. The token is the pool's (`state/coord/token`), the same everywhere.
- **Versions.** The page and a broker may run different apps now: the API has a version, and a page
  reloads when its broker's is newer. A task names the programs' build it needs; a broker serves
  them.
- **Trust.** A worker found giving wrong results (a broker's check: workers.md §5) is recorded per
  broker on the NAS (`state/coord/trust/<member>.json`), read by every broker.

## 9. The pool's API

Every member answers on the pool's port (8090): the worker page and its files; `/pool/status` (its
heartbeat); `/work/*` for its own tasks; and, on the lead, `/lead/*`: jobs (ask, beat, done), lead
asks, pause asks, "where is work". A member reaches the lead at the addresses in the lead's heartbeat
(Tailscale's, then the LAN's). A member that can't reach the lead keeps its jobs running (their
hand-offs go to the journal) and starts nothing new; its brokered tasks carry on.

## 10. Seeing the pool

- **Each member's heartbeat,** `state/pool/members/<id>.json` (whole, its own writer): its label,
  app, resources, conditions; its slots' jobs (progress, parts, memory) and why a slot is idle; its
  tasks waiting and out; its addresses; whether it leads (its term), or a handover's state
  (`handing_to`, `ready_for`). Written each loop it changes, at least every two minutes. Read by
  member id (the ids are in the lead's status), not by listing.
- **The build's status,** written by the lead per term (`state/build/term/<E>/status.json`): the
  checklist, the forecast, the regions, what waits and why, the jobs that ended lately (any
  member's), the pause, the last catalog, the lead and its term, a handover under way.
- **Everywhere it's shown** (the menu bar on every Mac, the map's build panel, the worker page,
  `scenic status`): the pool, a line or card per member, the lead marked; out-of-touch members with
  when they were last heard from; a handover's states as they happen; "No lead" with "Take it". Each
  Mac's map server reads the pool from the NAS, keeps the app its own agent runs, and holds its
  mirror's copying while any member's job runs.
- **The history** notes each term (handed over, taken back, taken over, re-asserted, stepped down),
  each member joining, leaving and coming back.

## 11. Controls

- **The menu bar, on every Mac:**
  - on the lead: "Hand the Build To ▸", the other members, each with its state (home on power, on
    battery, away, out of touch, app too old); those that can't lead now greyed with why. Handing to
    a Mac that's away warns that its duties run slowly over Tailscale;
  - on any other member: "Make This Mac Lead" (an ask to the lead); when the lead is out of touch,
    "Take Over the Build…", confirming;
  - the proactive offer (§6.5) when it applies; Pause/Resume, as now (any member).
- **The worker page:** on each member's card, "Make lead" (confirming); on the lead's, a handover's
  progress; "Take it" when there's no lead.
- **`scenic lead`** (who leads, the term, since when, a handover under way), `scenic lead give
  <member>`, `scenic lead take [--force]`.
- **The map's build panel:** who leads, and "Make this Mac lead" for the Mac it runs on.

## 12. Getting there from today

Each phase ships switched off behind `state/pool/enabled` (the agent behaves as today while it's
missing) and is switched on once every member's heartbeat shows the app that has it. The M1's launch
file passes `--helper` (install.sh), so the pool's app accepts it (and ignores it once enabled).

1. **Terms and records per term.** Member ids; `terms/` and term 1 (made with create-new, naming the
   Mac `state/build/writer` names); records per term with term 1 as today's layout; the journal as
   a log, written by members directly; the coordinator's state per term, leases saved on grant and
   finish, wall-clock times, lease ids `<term>-<n>`; history per writer; `writer`, `check_writer`
   and `SCENIC_BUILD_MAC` gone; crate::whole and EBUSY retries for every records, term and
   heartbeat write. **Seeding and draining** when it's switched on: the M4's token copied to
   `state/coord/token` (open pages keep working); the M4's local `coord/journal/` and the M1's outbox
   merged; `state/build/handoff/<host>/` still merged until empty.
2. **Handing over and taking over.** The state machine (§6.4) with take-back, takeover (§6.5),
   re-assertion and stepping down (§6.6), the app rule, staying awake, the lead's own jobs moved out
   of its process into its slots (so nothing pins the lead).
3. **Controls.** The menu bar, the worker page, the map's panel, `scenic lead` and `status`, the
   history's terms, the proactive offer.
4. **Every job hands off, placement and pages.** The STEPS table with floors and write-sets, every
   step offered to every member's slots, contiguous runs, the alone reservation, sticky resuming,
   the resume overlay and records-only planning; tasks brokered by every member, "where is work",
   pages direct with CORS; claims retired; the forecast for equal machines; the docs (plan.md §8,
   workers.md, formats.md) rewritten around the pool; `install.sh` without `--agent`/`--helper`.

Phases 1–2 are the ones that can lose work if they're wrong; each is tested as §13 says before it's
switched on.

## 13. Testing

- **A simulator** (deterministic, in the crate's tests): a fake filesystem that models stale reads
  per client, renames delayed across a "sleep", EBUSY on rename over an open file, and atomic
  create-new; two to four Macs driven by a seeded schedule of loops, sleeps, wake-ups and handover
  asks. It checks the four invariants of §4 after every step (one lead per term, the term never back,
  no accepted hand-off lost, keys agreeing with the manifest). The first draft's design fails it;
  this one must pass thousands of schedules.
- **Two agents on one Mac:** overrides for the member id, port and root, so two agents run against a
  scratch folder (today the coordinator starts only without a root, and both would bind 8090 and
  share a host name). Sleep is SIGSTOP and SIGCONT of an agent's process group, with fault points
  (`merge:before-rename=600s`, `handover:after-ready`, `takeup:after-copy`).
- **Two Macs on the real NAS,** a scratch folder: the ◻ checks of §3 first (create-new between two
  Macs, exclusive rename, how long a renamed-over file reads stale), then a handover and a takeover
  under load. (One Mac mounting the share twice shares one SMB client cache, so it can't show stale
  reads between clients.)
- **Each phase end to end** on the scratch folder with both Macs before `state/pool/enabled` is made
  on the real one.

## 14. Decisions

Taken (the owner may revise them):

- **Automatic takeover:** not now (§6.5); the proactive offer instead, one click, automatic only if
  the owner turns it on.
- **`tailscale serve` on every Mac** (HTTPS for pages talking to members directly): the owner's to
  turn on, one command per Mac. Until then, a page opened over plain HTTP on the tailnet reaches
  every member (WireGuard encrypts it anyway); one opened over HTTPS reaches only members with HTTPS
  (browsers block plain HTTP from it), and takes their tasks alone.
- **Work order:** every slot from the front of the plan, in contiguous runs, in place of today's
  helpers-from-the-far-end.
