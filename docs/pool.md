# The pool: any Mac can lead the build

Status: **phase 1's core built** (`crate::pool`, its driver and phase 2's transitions with it: §12),
not wired into the agent, so nothing runs it yet; the rest is planned (design of 2026-10-05,
revised after an architect's review the same day). It replaces the fixed "build Mac" and its
"helpers" (plan.md §8, workers.md §8) with a pool of peer Macs, any number of them, one of which
leads the build at a time, and makes the browsers' pages workers of the same standing, by one model
of work. The lead can be handed to another Mac from any Mac's menu, the worker page, the map's
build panel or `scenic lead`, and taken by another Mac when it's gone. Nothing about a Mac's role is
fixed at install.

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

Principle 2 as the build stands (on a Mac, in `~/Library/Application Support/scenic/`: its agent's
folder, `agent/`, and the app's, `app/`; plan.md §3–4 and formats.md say more). Of the last
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
| The coordinator's: the workers' token and devices, leases, costs, the history, the hand-offs not yet merged, the pause (mirrored to `state/build/pause.json`) | the build Mac's disk alone (a helper's hand-off, the helper's until it's sent) | `agent/coord/` (`journal/`: the hand-offs taken); a helper's `agent/outbox/` | planned (phase 1): on the NAS, per term (§6.2, §7.3, §7.5, §8); today, none: pages ask again, costs are first guesses, unmerged hand-offs are built again |
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
  named by its id), and a second can't take it. It:
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
  takeover (§6.5), a re-assertion (§6.6). Exactly one Mac's create succeeds; the others see the file
  and stand down. No lock is needed (`lead.lock` and the records' lock of the first draft are gone).
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
  (`records::first`, with create-new) before term 1 itself is (`term::bootstrap`), and term 1's
  lead writes them after each of its saves, for old readers. The pool reads them only to make that
  first snapshot, and to take term 1 up from it when it can't be read (its maker stopped midway, or
  its bytes landed out of order, a hole mid-file): nothing has written them since (a lead's saves
  are whole, and replace it).
- **Taking up term E+1** (`records::start`, crate::pool::driver): the new lead starts from the
  newest snapshot a read finds (about 3 MB), walking down the terms: E+1's own when an earlier try's
  landed, then E's, then E-1's, and so on (a term whose lead saved none has none). A lead
  re-asserting or taking back uses its own records for its old term, unread; an earlier try whose
  save failed, its records as tried. A handover's term starts only from the snapshot its `seq`
  names, or a later one (a stale read gives an older one: the take-up fails, to be tried again
  shortly). The new lead saves them as `term/<E+1>/records.json` and leads; its members tell it of
  their entries, and it lists the journal off its loop and replays every entry its records don't
  name, in (term, lease) order (§7.3). Its records reflect the journal once that listing is merged
  and saved, and every entry it was told of or listed is read (`caught_up`): a catalog, and GC,
  wait for that (an entry not merged yet may hold uploads the records don't name).
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
  check (§6.6). The lost-update path of the first draft (a late rename of a manifest over the new
  lead's, healed from a journal that had already dropped the entry) can't happen: there's no shared
  file to rename over.
- **The coordinator's state** is per term too: `state/coord/term/<E>/{leases,costs,failed,trust,
  pause}.json`, copied at take-up (§7.5). A handover's new lead loads it as the old lead wrote it
  last, from the snapshot its term names (§6.4).

### 6.3 Asking

Any member asks the lead over HTTP (§9): `POST /pool/lead {to, by, how}`. Its menu ("Hand the Build
To ▸", "Make This Mac Lead"), `scenic lead`, the map's build panel and the worker page all send one,
through their own member. A member that can't reach the lead can't hand anything over; it can only
take over (§6.5). An ask naming a Mac that isn't a live member (heartbeat beat within ten minutes,
NAS reachable, app new enough) is refused, saying why.

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
clock, or it says it can't reach the NAS), or its heartbeat says it stood down from its term
(§6.6), the owner may have another member take the lead: "Take Over the Build…" on its menu, which
says since when the lead's been gone and asks to confirm, the worker page's "Take it", or `scenic
lead take`. The member makes `terms/<E+1>.json` naming itself and takes up as in §6.2. `--force`
does it without the out-of-touch check (the owner knows the lead is off), and past a term that
can't be read whole (§6.1); `--downgrade` on an app older than the term's (§6.1), the lead that
stood down included.

What the old lead had in flight: its jobs' hand-offs are in the journal and get replayed; leases
its slots held lapse after their ten minutes and go back out (a lapsed lease's hand-off that comes
in later is still taken if its targets weren't leased again since); its coordinator's last changes
(costs, failures of the last minutes) may be lost; no built work is.

Automatic takeover of a lead that's gone or asleep is left out for now (the owner asked for buttons;
a lead that's only asleep would lose the role every night, and it may come back). In its place,
**a proactive offer:** when the lead leaves home or goes on battery while another member is home on
power, every member's menu offers "Hand the build to <Mac>" in one click (automatic if the owner
turns it on).

**A lead that stood down is taken over automatically** (§6.6): awake, it has declared it won't lead.
A member that has seen its heartbeat stood down for two minutes takes over by itself, if the app
rule lets it (crate::pool::driver: `STOOD_DOWN_S`): the members that can, the newest app first and
then the lowest member id, try 30 s apart, and the next term's create-new decides between them. Its
`how` says so ("taken over by MacBook-Air: Mac-mini stood down"). When no member's app can lead the
term, it waits for the owner (an update, or `--downgrade`).

### 6.6 Re-asserting, and stepping down

A lead's view can be old without its knowing: it slept, its clock was set, or the NAS didn't answer
for a while. So the lead **re-asserts** before it acts again (crate::pool::driver): it makes
`terms/<E+1>.json` naming itself, a create-new no stale read can fool, and takes it up from its own
records (one copy of its snapshot, about 3 MB). It does when, since its last loop or within it,
its wall clock moved more than a minute beyond its awake clock (Rust's `Instant`, which doesn't
count sleep: it slept, or its clock was set); when its last loop ran over five minutes awake (the
NAS stalled: on a share under load every operation can take seconds, so a loop's length short of
that says nothing); after a restart; and before every GC sweep. Time spent waiting between loops, or
listing the journal (done off the loop), isn't a gap. A member whose saved state is lost (or is
another's: a copy of the agent's folder) re-asserts a term naming it that it finds at its start,
rather than take it up again: it may have led it, its leases granted. If the term it makes already
exists, someone took over: it **steps down** at once.

A lead restarted into an older app or a development build can't re-assert (the app rule): it
**stands down**, and its heartbeat says so (`stood_down`: its term, which no one then leads), so
another member takes over, by itself after two minutes, or at the owner's ask unforced (§6.5); it
takes its term up again once its app is new enough. A member named by a term made on a newer app
than it now runs does the same. A lead handing over that restarts into an older app can't take the
lead back when its target doesn't take up (the app rule): it drops the handover, and the term waits
for its target or a takeover; the owner's forced takeover drops a handover waiting too.

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
| needs | memory (predicted per target from its last run, crate::coord::Cost), a **floor** whatever the measurement (the OSM pass runs Planetiler with 24 GB of heap: 32 GB Macs only), disk (the OSM pass 80 GB, a terrain run 55, others 15), home (whole-planet reads), power |
| write-set | the names it may add, change or remove, as patterns by target and pass date (§7.3) |
| moves | whether a job of it can run anywhere (most), or resumes on the Mac holding its progress |
| alone / beside | what may run with it on one Mac (today's SECOND, LIGHT, ALONE, RAW and WIKI sets) and across the pool (one Wikidata step at a time: one address at home) |
| batch | how much a job takes (about fifteen minutes) |

Tree cover's rows (plan.md §6, Trees): `trees`, a piece per z6 tile (it may add, change or remove
that tile's hi packs of the three tree layers and its mid, `work/trees-mid/6-x-y`; a lease of the
scheme before pieces, a z3 tile's whole run, its lo pack and its z6 tiles' hi packs, for one
release), any member, four a job; `trees-lo`, an assembly per z3 tile (its lo packs of the three),
seconds, kept to one Mac as the build Mac keeps it now.

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

- **A job's hand-off goes straight to the NAS** (crate::pool::journal): the member keeps it whole in
  its own folder until it's written (`journal::Mine`), writes `state/journal/<day>/<term>-<n>.json`
  itself, whole, by a temporary name (its own file: its lease's; a reader sees all of it or none),
  then tells the lead. `<term>-<n>` is the job's lease (the term it was granted in, and its number
  there), `<day>` the UTC day of the hand-off, by its member's clock (`YYYY-MM-DD`), fixed by its
  first try (one written after midnight keeps its key); the entry's key, `<day>/<term>-<n>`, is what
  records name it by. That removes the outbox and its retry loop and the HTTP body limit; a takeover
  finds every result on the NAS; GC sees them all.
- **The lead validates at merge time:** every change in the step's write-set (STEPS), every content
  name matching its logical name, the lease's targets and keys (the merge is given the check:
  `records::Check`). A refused entry is named in the records with why; once they're saved its why
  is noted beside the journal, `state/journal/rejected/<key>.why` (`journal::note_refusal`), for the
  owner. The entry stays in its day: a refusal may be a lead's that's no longer current (its check
  depends on its state), and a lead whose records don't name the entry checks it itself, told of
  it or listing it.
- **Lease order holds across merges:** an entry of an older lease for targets a newer lease's entry
  set already (a member back from sleep telling of it late) is named and changes nothing.
- **The journal is a log, not a queue:** nothing is removed on the merge path; a records snapshot
  names the entries it reflects; GC removes day folders older than a week once every snapshot since
  names their entries, and the records then forget those days' keys (`forget_before`), keeping the
  day they forget before: an entry of such a day told again, and not in the journal, is
  acknowledged (it was merged before GC removed it). Members forget what a lead acknowledged
  before that day, which the lead's answers carry.
- **The lead lists the journal off its loop** (3 to 33 s a folder): every day not forgotten after it
  takes up its term, and the last two days every ten minutes, for entries no member will tell it of
  (their member gone, having told only a lead no longer current). Otherwise it reads only the
  entries members tell it of, by key (§6.2: acknowledgements are per term). An entry listed or told
  of that can't be read whole yet (a stale read) is kept and read again every loop. A loop reads
  entries to merge for a minute at most, the oldest leases first, and leaves the rest to the next
  (on a share under load a read takes seconds, and a lead taking up from an old snapshot may have
  thousands to read); a member writes its jobs' entries so too.
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
  answers, its device's key with it. The devices accepted are the pool's (`state/coord/devices.json`,
  the same everywhere: every member answers a page's key from it), as is the agents' token
  (`state/coord/token`).
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
  tasks waiting and out; its addresses; whether it leads (its term), a handover's state
  (`handing_to`, its offer; `ready_for`, the offer it answers), or that it stood down from its term
  (`stood_down`: §6.6). Written each loop it changes, at least every two minutes, its beat stamped
  as it's written. Read by member id (the ids are in the lead's status), not by listing.
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
  <member>`, `scenic lead take [--force] [--downgrade]`.
- **The map's build panel:** who leads, and "Make this Mac lead" for the Mac it runs on.
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
     `state/build/writer` names, its records first; a create whose bytes didn't land finished by its
     maker; the app rule, the take-back checked against the term handed over; the owner's forced
     takeover, and downgrade); records per term (`records`: term 1's first snapshot from today's
     layout, which its saves keep writing; the merge, in lease order across merges; taking up,
     numbered on across tries; the forget horizon; readers' fallback to a term before); the journal
     as a log, written whole by members directly (`journal`: entries by lease id `<term>-<n>`, kept
     whole until written; refusals noted beside it; what a member tells each lead); the NAS's
     operations (`nas`: create-new, saying when its bytes didn't land, and whole writes by
     crate::whole); the heartbeat's fields (`beat`); phase 2's transitions (`handover`: the state
     machine with take-back, an answer to its own offer); the member's lock (`MemberLock`: one
     process per member); and **the driver** (`driver`): what a member is and does, one step per
     loop of the agent's, through an I/O trait (the NAS's operations and two clocks), the code the
     agent will run and the simulator runs. A step learns the current term; takes up a term naming
     it; leads (merges what members tell it and what a listing finds, saves, acknowledges, notes
     refusals); re-asserts after a gap; stands down when the app rule refuses it; hands over and
     takes back; takes over when the owner asks, and by itself a lead that stood down; writes its
     jobs' entries and tells the lead of them. It asks the agent for what's slow (a listing of the
     journal, off its loop) and what's the agent's (settling: the coordinator's state), and says
     what the agent may do now (grant jobs and plan; settle; publish and sweep once its records are
     caught up, a sweep only when fresh); it answers the controls (whom the lead can be handed to,
     what a takeover needs). Its contract is the module's doc. Nothing runs it yet.
   - **The integration: planned.** The agent's loop calling the driver each loop, holding the
     member's lock: its messages over the pool's API (§9), the listings it asks for on a thread of
     their own, settling (cancelling the duties in flight, writing the coordinator's state) and
     loading the state handed over, its saved state in the agent's folder after every step (a job's
     hand-off kept until one holding it is on disk), its heartbeat's fields in the agent's
     heartbeat, the members it knows, the merge's checks; jobs handing off to the journal through
     it; the coordinator's state per term, leases saved on grant and finish, wall-clock times, lease
     ids `<term>-<n>`; history per writer; `writer`, `check_writer` and `SCENIC_BUILD_MAC` gone.
     **Seeding and draining** when it's switched on: the M4's workers' token and its devices
     (`devices.json`: the accepted devices' hashes) copied to `state/coord/` (open pages keep
     working); the M4's local `coord/journal/` and the M1's outbox merged;
     `state/build/handoff/<host>/` still merged until empty.
2. **Handing over and taking over.** The driver does them (§6.4 to §6.6: phase 1's core); the
   agent's part: the owner's asks reaching it (phase 3's controls), staying awake, the lead's own
   jobs moved out of its process into its slots (so nothing pins the lead).
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

- **The simulator** (built: crate::pool's `sim`, in the crate's tests; a seed decides everything, so
  a run repeats exactly). Two to four Macs, a thread each, take turns step by step (a NAS operation,
  a message, a look at a clock) and run the pool's driver, the code the agent will run, the agent's
  part around it played as the design has it (`sim::Mac`: its jobs' hand-offs; its coordinator's
  state, granted while the driver lets it and written when it settles; the listings the driver
  asks for, made off its step; its heartbeat, stamped as it's written; a job reading the records).
  The share's model: create-new atomic, its bytes landing a step later (the file empty meanwhile,
  for as long as its Mac sleeps in between), or never (the share gone between: its maker's to
  finish); a whole write's rename a step after its temporary file, landing when its Mac wakes, over
  whatever was written meanwhile; a rename over a file another Mac has open (a read holds it up to
  8 s) failing busy, the write after four tries; a create or a whole write that did its work
  answering an error (its answer lost); each Mac's reads, stats and listings kept up to 30 s (60 to
  300 s in some runs), its own writes seen at once; a listing taking 3 to 33 s a folder in some
  runs, its Mac waiting on it; messages reaching only a Mac that's awake, the others dropped; clocks
  up to a second off (up to 20 minutes in some runs), and each Mac's awake clock stopping while it
  sleeps; in some runs every create, read, stat and whole write taking 1 to 4 s (the share under
  load), the Mac waiting on it, awake. The schedule: sleeps after any step, mid-loop (5 s to 40
  minutes, more often right after a temporary file or a create), the owner's asks (hand the lead
  over; take it over, forced at times with the lead alive, and by the owner's downgrade), newer apps
  (a restart: the Mac's memory gone but what its driver saved, and at times that too), development
  builds and rollbacks, a member leaving for good, a week of the journal's earlier days that term
  1's records lack, and jobs' entries, some of them refused, some by the leads of odd terms only (a
  check that depends on the lead's state). Each run draws these knobs from its seed.
- **What it checks,** at every step that could break one of §4's invariants: terms made in order,
  never written over (but by their maker finishing them), by the app rule (but a take-back's and
  the owner's downgrade); a term led by the Mac it names, and by no other; no Mac's term going back,
  across its restarts too; a term's records written by its lead alone, self-consistent (as members
  read them too), never going back in `seq` or in the entries they name; an entry acknowledged only
  once a saved snapshot names it, and never removed; a heartbeat's beat its Mac's clock as it's
  written; a handover's new lead taking up with the coordinator's state its old lead settled with; a
  lead saying it's caught up only when its snapshot names every entry written before its take-up's
  listing. Then, the faults over and every Mac awake for 25 minutes, the owner taking over a term
  the views would show with no lead that no member takes by itself (its lead gone for good, or
  stood down where no member's app can lead the term, or the term unreadable): the last term's lead
  leads it, alone, no term was made in those 25 minutes' last ten or after, and its records name
  every entry ever written, a Mac's gone for good included. While they lack entries, fewer each
  time, the run goes on ten minutes at a time, up to two hours: on a share taking seconds an
  operation a lead merges about a dozen entries a loop, and hours of faults with no lead leave
  hundreds. The tests run 2,000 seeds, each kind of
  change of lead and of fault, and what each knob brings, among them at least three times; slow
  listings with a week of the journal, development builds and rollbacks, and a Mac leaving, alone
  (300 to 1,000 seeds each); a long run, left out by default, of 100,000 schedules of four hours'
  faults; and the first draft's scheme (one shared records file, the journal emptied as it's
  merged) on the same model, which finds its lost update.
- **What it doesn't model:** torn or holed reads (a file reads whole, empty or as an older version:
  the modules' tests read holes); I/O errors other than a busy rename, a create cut short and lost
  answers; `remove` failing busy; GC (removing the journal's old days, forgetting them: the forget
  horizon is the modules' tests'); the coordinator (leases are numbered in order per term, granted
  by no one, and the lead's check depends on its term alone).
- **Two agents on one Mac** (planned): overrides for the member id, port and root, so two agents run
  against a scratch folder (today the coordinator starts only without a root, and both would bind
  8090 and share a host name). Sleep is SIGSTOP and SIGCONT of an agent's process group, with fault
  points (`merge:before-rename=600s`, `handover:after-ready`, `takeup:after-copy`).
- **Two Macs on the real NAS** (planned), a scratch folder: the ◻ checks of §3 first (create-new
  between two Macs, exclusive rename, how long a renamed-over file reads stale), then a handover and
  a takeover under load. (One Mac mounting the share twice shares one SMB client cache, so it can't
  show stale reads between clients.)
- **Each phase end to end** (planned) on the scratch folder with both Macs before
  `state/pool/enabled` is made on the real one.

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
