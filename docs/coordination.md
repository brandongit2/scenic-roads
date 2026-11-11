# Pool coordination by HTTP on the lead

**Planned (task #878); steps 1 to 7 and 8 (8a, 8b, 8c) are built, steps 3, 4, 5 and 7's NAS paths
for older apps deleted (task #1036), the rest planned.** This doc says how the pool's
coordination moves off the NAS onto the lead, which holds it on its own disk and serves it over
HTTP. For the steps not yet built (§8 says which are), docs/pool.md and the code are what is true. It assumes #881 (the Mac mini
as a fixed lead: no election, no takeover) has landed; where #881 leaves a piece of the
election machinery in place, the step that deletes it here says so. docs/agent-parts.md's **pool**
part (#871) is the client side of this design.

**The rule for old-app paths** (the owner's, 10 Nov; it replaces the #642 gate, which waited a
day after every Mac spoke the new piece and then a release more): an old app's path for a piece
may be deleted once every Mac runs an app that has the new path, as their beats show (each Mac's
app, and the words it says). No day's wait, no extra release. A rollback to an app from before
that point loses the old path: such an app finds the old path gone (the NAS's files no longer
written or read for it) and works only as far as the new path it has.

## 1. The problem

Every coordination message today is a small file on the SMB share: heartbeats, mail, journal
entries, terms, statuses and views, holds, switches, the Wikimedia budget's tokens, progress
files. SMB is at its worst exactly there:

- **Small creates are slow:** about 9–55 a second for the whole share (docs/nas-access.md), shared
  with the build's own writes. The Wikimedia budget alone makes a create-new file per slot per
  10-second window (`state/pool/wiki/<class>/<window>-<slot>`).
- **Mount stalls freeze Macs:** a call stuck in the kernel for minutes (pool.md §7.8), which is why
  `nasread`'s deadlines, the stall watchdog and the kept beat exist.
- **Handles go stale:** a held file can't be removed for hours (pool.md §3, the asides), and a
  name can read as missing for 40 minutes (task #630, the 24 Oct entry).
- **Reads are stale:** listings up to 33 s, one name 40 min; every timer in pool.md §6.5 is sized
  around it, and the journal's GC needs a fence (#644, #651) because a take-up may read old
  records.
- **Requests go unanswered within 20 s:** the coordinator's answers wait on the agent and the
  share (a pool step held on a NAS call stops its term's headers; the page data is read off the
  NAS every 12 s).

The root cause is one: the share is used as a message bus and a lock service, which it is not.
The fix is to stop using it as one. Bulk data stays on it; coordination goes to one process on one
disk.

## 2. Principles

1. **The lead holds the pool's state** on its own disk, in one store (§4), written by one thread.
   No other Mac writes coordination state anywhere.
2. **Members talk to the lead by HTTP** on its coordinator's port (8090, over the tailnet), and
   only to the lead. Nothing in coordination goes member to member, and nothing goes through the
   NAS.
3. **The lead's HTTP handlers never touch the NAS.** They read and write the store and memory only;
   an answer never waits on the share. A test holds the rule (§8, step 1).
4. **Every post is idempotent:** a lost answer is retried, never doubled. Built: each post is
   idempotent by its own content, with no sequence number: a beat says whole state (its leases,
   heartbeat, status, hand-offs' keys, progress); a hand-off is taken once by its key; a hold or
   setting by its `at` (the later wins, a repeat changes nothing); mail by its numbers; the
   answers' lease by its holder's token (a repeated take or renew is the same lease); a budget
   take repeated after a lost answer gives a second batch, the first unspent (the budget errs
   below its rate). Planned, should a post come that isn't: it carries the member's id and a
   sequence number, the lead keeping each member's highest number taken and answering a repeat
   with the first answer.
5. **Durable before the answer:** a post that changes state (a grant, a hand-off, a hold, a
   setting) is in the store's log, flushed (`fsync`), before its 200. Local SSD makes that
   milliseconds.
6. **The member keeps its own until it's safe on the lead:** hand-offs, asks and its held leases
   live on its disk until the lead says they are merged or taken (§5.3). The members are the
   second copy of what matters.
7. **Bulk data stays on the NAS:** built files, sources, caches, records snapshots (one writer, big
   whole files: what SMB does well).

## 3. What moves, and what stays

### 3.1 Moves to the lead

| Today (NAS) | Writer today | By HTTP | Lead's store |
| --- | --- | --- | --- |
| `state/pool/members/<id>.json` (pool heartbeat) and `state/status.json` / `state/helpers/<host>.json` (the agent's status) | each member, each lead (the members' files and the members' listing deleted, #1036; the lead's `state/status.json` stays, §3.2) | **one** `POST /pool/beat` every 10 s and on a change (agent-parts.md §4.2): conditions, slots, held leases, reach, tasks, broker addresses, status sections with ages | in memory, the last of each member; in the snapshot for the page after a restart |
| `state/pool/mail/<to>/<from>.json`, and the lead's `inbox.json` (task #642) | each sender (the mailboxes deleted, #1036) | member → lead: `POST /pool/mail`, the only path (built, step 5); lead → member: carried in the answer to the member's next beat (`msgs`), so members need no endpoint for it | the members' posts (`inbox/<member>`); the outgoing queue per member is its agent's `mail.json` on its disk (each sender's last 64), sent after what the member's beat says it took (`took`) |
| `state/journal/<day>/<lease>.json`, `rejected/` (`gc.json` and the journal's GC deleted, step 6) | each member, while its lead doesn't take hand-offs by HTTP (step 6's gate) | `POST /pool/handoff` (built, step 6: the hand-off whole, the entry it was in the journal, with its sizes) | `handoff/<key>` until a saved records snapshot names it; merged into records; the refusals in the snapshot |
| `state/build/terms/<E>.json`, `state/build/lead.json` | none since step 8c (the declared lead made them, #881) | a member learns the lead from the beat's answer (`leading`: its member, host and app; step 8a, 8c), the declaration (`lead-member.json`) read only while no answer came within 35 s; the lead's address still from `state/coordinator.json` | an **epoch** (§4.3), bumped each start, raised past a take-up's records (built, step 8c) |
| `state/coord/term/<E>/state.json` (gone with `coord::keep`, step 1) | — | none | the coordinator's state is the store's |
| `state/status.json` (as the pages read it), `state/coord/term/<E>/view.json`, `forecast-input.json`, `regions.json`, `state/build/pause.json` | the lead (none of the four written since #1036) | `POST /pool/view` (built, step 4: one document; the lead's status, the forecast's input and the regions in it only when their time differs from the ask's (#913: the status's file is ~850 KB on the live lead, written every ~7 s, asked every 2 s)) | in memory: the coordinator's state, its agent's status, the forecast's input and regions its agent hands it |
| `state/pool/holds/<member>.json` (another Mac's pause) | any Mac (deleted, #1036) | `POST /pool/hold {member, held, at}` from any Mac's agent (built, step 5); the held member gets it in its next beat answer (`hold`) | `hold/<member>`, the later `at` winning |
| switches and pool-wide settings: `state/pool/placement`, `nas-budget`, `nas-target.json` (built, step 5); `resume`, `brokers`, `memory-guard` deleted outright (on everywhere); planned: `nas-room.json` (read by every job: moves once the jobs have it from their agent), `near-slope` (on everywhere: deleted outright, decision 6), `journal-gc` (deleted with step 6: no journal GC runs); `delta-catalog` no longer read (#883 step 6: no catalogue is written, its file deleted with #883 step 9); `battery-floor.json` gone with #881 | the owner's CLI (`scenic pool set`, `scenic nas-target`); the build page's Settings for `batch-minutes` (no switch file: task #927); no switch file written or read since #1036 | `POST /pool/settings` (a setting, or none: the settings); each beat's answer carries the settings' generation (`settings_gen`), a member fetching them when it changes | `settings`, each with `by` and `at`, and its generation |
| `state/pool/progress/<member>.json` (resume progress) | each member (deleted, #1036) | in the beat, whole (built, step 7: `Beat::progress`; kept on the member's disk, `progress.json`); the owner's release `POST /pool/progress/release`, carried back in the beat's answer | `progress/<member>`: its progress and the releases of it |
| `state/pool/wiki/<class>/<window>-<slot>` (Wikimedia tokens), `state/pool/answers.lease` | create-new by every job's proxy (deleted, #1036) | `POST /pool/budget/take {class}` → a batch (its tokens, its window, how long it may be spent) or a wait; `POST /pool/budget/hold {s}` (a 429); `POST /pool/lease/answers {op, by, token}` (built, step 7) | each class's window and its tokens given, in memory (not durable: a restarted lead gives nothing in the window it first hears an ask in, so a restart only slows); a 429's hold (`budget-hold`) and the answers' lease (`answers-lease`) in the log |
| `state/pool/nas-probes.json` | the lead's prober | none | the lead's local store |
| `state/coord/history/<day>/<member>.jsonl` (members' brokers' events) | each member, appends | events ride in the beat (`events`, since the last acknowledged) | the lead's history file on its disk; a day's tar goes to the NAS by the daily `gc` as today (§3.2) |
| `state/coordinator.json` (the contact) | the lead | none: the fixed lead's address is #881's | — |

Two heartbeats become one: the pool's and the agent's status are the same post, so a member is
never "beating" in one and "out of touch" in the other.

### 3.2 Stays on the NAS

- **Built files, sources, caches, scenic units, raw archives:** unchanged (docs/storage.md).
- **Records snapshots** (manifest, keys, pending, sizes; and the brief), written whole by the lead
  alone after merges, read by jobs on every Mac and by the map server, at one path:
  `state/build/records.json` and `brief.json`, each naming its epoch (the store's, under the key
  `term`: step 8c) and `seq` (built, step 8b; no term's own snapshot since 8c). The lead's own
  copy, on its disk, is what its take-up reads. Planned: readers that are
  agents `GET /pool/brief` instead (open decision 2).
- **The store's backup:** `state/lead/store.json`, the store's snapshot written whole every
  `backup_every` (10 min) and at a clean stop; one file of a few MB. Read only to rebuild a lost
  lead disk (§4.4).
- **History archives:** `state/coord/history-archive/<day>.tar`, one big file a day.
- **The token** (`state/coord/token`): read once at start by every agent, written never by the
  pool. A secret on the share it already relies on; it moves to each Mac's keychain later, if ever
  (not this task).
- **Read-only inputs** (`inputs/`, `inputs/keys.env`): unchanged.

## 4. The lead's store

### 4.1 Shape

`crate::pool::store` on the lead, in `agent/coord/store/` on its disk (built, step 1), its files
`store::seglog`'s, shared with the map's log (docs/maplog.md §4.2):

- `snapshot.json`: the whole state as of a log position, written whole (temporary file, `fsync`,
  rename, `fsync` of the folder).
- the log, segments `<first seq>.jsonl`: one line per change after that position, each with its
  position, appended and `fsync`ed in batches (a batch is every change waiting when the writer
  wakes: one `fsync` for a burst of grants).
- The state: keyed JSON documents (`crate::coord::kept` names them) and the epoch. Built (step
  1): the jobs' leases (id, holder, step, targets, granted; the next id, the holds and cancels),
  the coordinator's measures (costs, floors, disks, jobs' disks, caches' fills) and the pause.
  Built (steps 3 and 4, `crate::coord::beats`): each member's last beat (heartbeat and status) in
  memory, and in the store (`beat/<member>`, `kept::Beat`, saved each minute at most: the
  heartbeat and when heard, not the status, which was ~85% of the store's log, #913), so a lead's
  restart has the members heard within 20 min for the page and the answers before their next
  beats (their statuses with those beats, within 10 s). Built (step 5): the members' mail
  (`inbox/<member>`), holds (`hold/<member>`) and the settings. Built (step 6): the hand-offs taken
  and not yet in a saved records snapshot (`handoff/<key>`, crate::coord::handoffs); the records'
  last saved epoch and `seq` are the lead's driver's (`Saved::records`, on its disk with its state),
  and so (step 8b) are the records themselves, kept beside it (`agent/pool/records.json`), not in
  the store: a snapshot of ~15 MB written whole each save.
  Built (step 7, crate::coord::budgets): the answers' lease (`answers-lease`) and each member's
  resume progress (`progress/<member>`). Planned, with the steps that move them: the workers'
  failures (and each member's sequence high-water, should principle 4 need it).

One writer thread owns the files (the agent-parts.md rule); handlers submit a change (applied to
the state in memory at once) and wait for its "durable at N", under a 2-s deadline past which the
post answers 503 (a lead disk that slow is broken). Built: every JSON post of the coordinator
waits so (`http::json`). Planned: saying it in the status.

### 4.2 Compaction

When the log passes 10 MB or an hour, the writer writes a new snapshot at its position and starts
a new segment; the old segments are removed only after the new snapshot's rename and folder
`fsync`. A crash between leaves a snapshot and a log that overlaps it: replay skips positions at or
below the snapshot's. A failed append is followed by a compaction too (the snapshot from memory),
so a change in memory is never left out of the files.

### 4.3 The epoch

A counter in the store, bumped and `fsync`ed as the store opens (built; the coordinator opens
it right after it starts listening, before the agent asks anything of it). Built (step 8c): it is
what the lead leads, in place of a term. A take-up raises it past the records it starts from
(`Store::raise_epoch`, a durable line; `pool::driver::Io::epoch`), and past the NAS backup's by
`RESTORE_EPOCHS` when another Mac wrote that (the owner moved the lead: its leases may be of
epochs up to a restart past its last backup), so the records' (epoch, seq) and lease ids
`<epoch>-<n>` order across restarts, moves and the move from terms (the first take-up after it
starts past the records' term: the live store's 121 becomes 841, past term 840). Every answer
says `x-pool-epoch` (0 from a member's broker) and `x-pool-member`. Planned: a member seeing a new
epoch re-sends what it keeps (§5.3), nothing else changing for it (today it does so when the
answer says the lead lacks it).

### 4.4 Recovery

- **A restart** (crash, update, reboot): load the snapshot, replay the log, bump the epoch, listen.
  A torn last line (a crash mid-append) is dropped: it was never answered. Every lease comes back
  as it was, with a fresh deadline, marked *unconfirmed*; each
  member's first beat lists the leases it holds, confirming them, and those of that member it
  doesn't list are released at once (§6.2).
- **The lead's disk lost** (the mini's disk replaced, `agent/` deleted): the store is rebuilt from
  the NAS backup (`state/lead/store.json`, at most `backup_every` old), the epoch set past it by
  1000 (no lease id repeats), then members' first beats fill in what the backup missed: their held
  leases (adopted as granted, with their ids; planned: today a beat's lease the lead doesn't know is
  answered released, and its job stops) and their unmerged hand-offs (re-posted, §5.3; planned). What
  is lost: grants answered since the backup to members that then lost them too (none in practice),
  the settings changed since (the owner's, said in the status: "settings restored as of 14:02").
  Built: `pool::store::Store::restore` (test `store_restores_from_the_backup`). Planned:
  `scenic lead restore` to run it; the agent never does it by itself (a missing store on a lead
  that had one is an error, not a fresh start: the agent refuses to lead without `--restore` or
  `--fresh`). Today a coordinator finding no store makes one from its old files (the migration),
  and a lead whose take-up finds the NAS backup written by another member loads it
  (`agent::pool::took_up`: the owner moved the lead).

## 5. The protocol

### 5.1 Calls

All on the lead's coordinator, the build's key as today, sent only to tailnet addresses
(`mail_urls`' rule):

| Call | From | Answer |
| --- | --- | --- |
| `POST /pool/beat` | each member, every 10 s and on a change | epoch, the settings' generation, holds for it, messages queued for it, the hand-offs merged (acks), leases released, the lead's line for the menu bar (`lead`: docs/roles.md §3.4), the lead while it leads (`leading`: its member, host and app; steps 8a, 8c) |
| `POST /pool/handoff` (built, step 6) | a member's agent's beat poster, for each hand-off the beat's answer says the lead lacks | taken (durable), or 409 while the lead's agent hasn't given its coordinator the hand-offs' registry |
| `POST /pool/mail` | a member's `Tell`, `Cancel`, `Retry`, `HandTo` (gone with #881) | taken |
| `POST /pool/hold`, `POST /pool/settings` | any Mac's agent, for the owner's CLI and menu | the new state (the settings: a POST naming none reads them) |
| `POST /pool/budget/take`, `/pool/budget/hold`, `/pool/lease/answers` (built, step 7) | an agent's proxy, for its jobs (the lead's own in process) | a batch or a wait / the hold's end / the lease's word (done or not, who holds it, until when) |
| `POST /pool/progress/release` (built, step 7) | the owner's CLI (`scenic pool release <member> <job>`) | what it released, or (400) that it keeps none of that member's |
| `POST /pool/view` (built), `/pool/brief` (planned) | each Mac's pageview (the CLI planned) | the documents |
| `/work/*` | jobs | unchanged (the build page's data and asks moved to `/api/*`, task #1101) |

### 5.2 Timing

Each call has 5 s to connect and 15 s in all at the client (`coordwire::client`; a file's
transfer, `/work/in/`, `/work/out/`, no limit on its whole time, only 60 s with no byte moving). A
call that gets no answer marks the lead unreached for that round (`Client::new_round`), and the next
beat tries again.

Connections are kept alive (task #958): each process has one HTTP client, every `Client` sharing
its pool of connections, at most 4 idle to an address and 64 in all, each let go after 10 s idle;
the coordinator closes one idle for 20 s (`HEADERS_MAX`, the time a request's headers have), so the
client always lets go first. A member beating every second holds one connection to the lead, where
it once opened one a request, each holding its port in TIME_WAIT for 30 s after (a test program's
many requests ran a busy Mac out of ports). A kept connection gone stale:
- **closed by the coordinator** (restarted, or closing it just as a request came): ureq finds it
  closed before taking it, or the request fails before any answer, and goes again at once on a new
  connection (the coordinator never read it: hyper closes an idle connection, or one shutting
  down, only between requests; one stopping mid-request has already stopped taking them, and its
  answer to the repeat is a 503 or none);
- **its Mac silent** (asleep, off the network, no reset sent): after 2 s with no answer the client
  connects to the address anew (5 s); taking no connection, the request fails as a new
  connection's would (unreached, its cause read from it); taking one, the lead is there and slow,
  and the request waits out its time (as does one on a LAN address after this Mac changed networks:
  its old connection's answer never comes, and it fails once, at 15 s).
A connection takes no new request after 3 h (`CONNECTION_MAX`); the one under way is answered, or
cut once it has had 3 h of its own, as when each request had its own connection. An older member or lead (one connection a request) works with a newer one
as before: either side may close after an answer.

### 5.3 What a member keeps

In `agent/pool/` on its own disk, as today's outbox and `journal::Mine`:

- its **held leases** (the jobs running, already saved by the agent);
- its **hand-offs** not yet acknowledged as *merged into a saved records snapshot* (not merely
  taken: a lead disk lost between taking and saving loses nothing). Built (step 6): its driver's
  saved state keeps them whole (`journal::Mine`); each beat lists them (`handoffs`), and the answer
  says which a saved snapshot names (`merged`: dropped) and which the lead lacks (`lacks`: posted
  next). So a hand-off is posted the beat after its job ends, and again whenever a lead lacks it (a
  new epoch after a lost store), with no state of what was posted;
- its **asks** (mail, holds, settings) not yet answered 200.

On a new epoch, or every 10 minutes while any are unacknowledged, it re-sends them; each is
idempotent by its own content (principle 4), so a repeat changes nothing. (Hand-offs, step 6: re-sent when the lead's answer says it lacks
them, which covers a new epoch; a repeat is taken once by its key, and one posted after it's merged
is passed over by the records, which name it.)

## 6. Leases: a job is never run twice

### 6.1 The rule

A target is granted only by the lead, only while no lease on it is live, and a lease ends only by
its holder's word (the job ended, gave back, or the holder says it doesn't hold it) or by the
owner's. **The clock alone never re-grants a target.** Today a lease lapses after `coord::TTL`
(10 min) unrenewed and goes back out, which is how a partitioned member's job gets run twice; here
the lead can always tell "holder gone" from "holder unreachable" only by hearing from it, so it
waits to hear.

Built (step 2: `crate::coord::held`; step 3: one beat), but for what is marked planned below. A
member's agent posts `POST /pool/beat {member, speaks, held, held_age_s, heartbeat, status}` every 10 s
(`agent::beat::EVERY`) while it's a member, from its beat's own poster thread, whatever its loop
waits on; `held` is the list its loop made last, `held_age_s` its age: the lead releases only
leases granted before the list was made (a loop stuck after a grant never loses it), and a member
whose lead hasn't said it takes `beat` sends no list older than half the grace. The answer is
`{epoch, released, speaks, members, …}` (the leases it listed that aren't its on the lead,
its tasks' leases counted its: its jobs stop as their own beats are then refused; the rest, step
3's, §8). The rule holds for a
member whose beat says it speaks `held-leases` (`held::SPEAKS`), kept in the store
(`member/<host>`) so a lead's restart keeps it; an older member's leases lapse after `coord::TTL`
unrenewed as before, and an older lead answers the beat 404, which the member passes over (§8's
gate). The lead's own jobs' leases are its own slots' as before (`Leases::mine`, §6.3).

- **The grant is durable before its answer** (every JSON post waits for the store, step 1).
  A lead crashing after the `fsync` and before the answer leaves a lease its member never got: the
  member's first beat doesn't list it, so it is released (§4.4).
- **Renewals are beats.** A reachable member's beat lists the leases it holds; any lease of that
  member the beat doesn't list is released at once (a crashed job, a restarted agent that lost it),
  but one granted within `held::OMIT_GRACE` (30 s: its answer may still be on its way) and not
  loaded since. The beat lists the member's slots' leases and those whose ends it still owes the
  lead (told again each loop until answered: `OWED_S`, which dropped them after 10 min, is gone).
  This replaces the lapse for every member the lead hears from, and is faster: seconds, not 10 min.
- **A member the lead stops hearing from** keeps its leases (they never lapse), held while away
  after `AWAY_S` (2 min without a beat): built in the coordinator (`held::away`: member, leases, last heard); planned on the page ("M1: 2 jobs held, not heard from since
  14:02"). Its targets aren't granted again until it's been unheard `held::AWAY_RELEASE_S` (12 h,
  the PM's decision 1), when they're released.
- **Releasing them** is the owner's call: `scenic pool release <member>` (`POST /pool/release`;
  the page's button planned)
  when that Mac is known gone for good. After a release, a hand-off from the old lease is refused
  (lease order: the newer lease set the targets), so even then nothing is merged twice.
- **A member returning** beats with what it holds; those leases go on (or, if released meanwhile,
  the answer says so and the member stops those jobs at once).

### 6.2 What it costs

A member that dies for good (disk dead, Mac gone) holds its targets until the owner releases them.
The status raises it after an hour (`AWAY_ALERT_S`), the menu bar too. That is the price of never
running twice; open decision 1 offers an automatic release instead.

### 6.3 The lead's own jobs

Its slots ask its own coordinator in process (pool.md §7.6); their leases are in the same store and
the same rule. A lead restart confirms them from its own saved slots.

## 7. Failure modes

### 7.1 The lead is down (#881: no takeover)

- **Members** finish their running jobs; each ending's hand-off stays in its outbox. They start
  nothing new: no grants. Their beats fail and are retried every 10 s, logged once.
- **What still works on a member:** its own hold and quit (agent-parts.md's **control** is local),
  its local status and page (`/api/swarm` from its own last view, aged: "lead not answering since
  14:02"), its brokers' tasks for its running jobs (the lead isn't on a task's path, pool.md §8).
- **Budgets** (built, step 7): no Wikimedia request goes out (each Mac's proxy waits, asking the
  lead again each window): the lead is the budget's one source, since a share taken by a Mac that
  can't reach it would add to what it gives the Macs that can (a Mac away over Tailscale, or whose
  asks time out). A 429 holds that Mac alone. The answers' lease isn't taken (no answers sync
  starts: a job waits for it, saying so).
- **The lead back:** a new epoch; members' first beats confirm their leases and re-send their
  hand-offs; the lead merges, saves records, acks; grants resume. Nothing is lost or run twice.
- **Seen as:** every Mac's page and menu: "the lead (Mac mini) isn't answering since 14:02; jobs
  running finish, nothing new starts". No member promotes itself.

### 7.2 A member away over Tailscale

Coordination no longer needs the NAS, so a member away from home but on the tailnet beats, asks
and is held exactly as at home. Whether it gets work is placement's call as today: it lacks the
NAS (or the NAS over Tailscale is too slow), so only `Needs { nas: false }` work fits it, and its
beat says `nas: away`. Its hand-offs' outputs are on the NAS (a NAS job's are; a job without the NAS
writes none there), so a hand-off posted while away is the same as at home. A member away with no
tailnet is §7.3's case.

### 7.3 A split network

| Split | What happens |
| --- | --- |
| a member can't reach the lead, both have the NAS | §7.1 for that member alone: it finishes its jobs and waits, keeping their hand-offs; the lead marks its leases `held_by_away` and grants their targets to no one. No NAS fallback: the mailboxes are gone (#1036), the journal once step 6's gate goes (§9). |
| the lead can't reach the NAS, members reach it | coordination goes on (the store is local); NAS-needing jobs of every Mac stall on the share as today; merges wait (records can't be saved: hand-offs stay taken, acks wait), catalogues and GC wait. The status says "the lead has lost the NAS". |
| a member can't reach the NAS, reaches the lead | as away (§7.2): local work only, its NAS jobs paused per today's rules. |
| two members can't reach each other, both reach the lead | nothing changes: members don't talk to each other for coordination (a member's page reads the others straight, pool.md §8: not coordination). |
| the lead hears a member but the member doesn't hear the answers (one-way) | the member's posts are taken but it sees failures: it re-sends (deduplicated) and starts nothing new; its leases stay live by its beats. Seen as "reached by the lead, not reaching it" (`Beat::reach`, today's). |

There is never a second lead to disagree with, so no split can make two grants of one target.

## 8. Migration: one piece at a time

Each step ships alone, keeps the build running (no stop, no shared flag day), and ends with its
tests. A member says what it speaks in its beat (`speaks: [...]`, as `mail_http` does today); the
lead keeps the old NAS path for a piece while any member in touch doesn't speak the new one, and
drops the old path by the rule for old-app paths (above). The tests use
the agent's two-agents-in-one-process harness (`pool_tests`) and #871's slow-NAS fake (every NAS
call blocks until released) plus a **network fake** on the client (drop, delay, lose the answer,
one-way) and a **lead kill** (the store's writer stopped mid-append, the coordinator restarted
from the same folder).

| # | Step | Deletes | Tests |
| --- | --- | --- | --- |
| 1 (done) | `pool::store` on the lead: snapshot, log, epoch, compaction, backup to the NAS; the coordinator's state moves into it. The handler rule: a scan test that no `coord` handler path reaches `nasread`, `Nas` or `store::whole` | `coord::keep`'s per-term NAS state file and its thread (`BATCH`, `GAP`, `Keep::off`) | `store_recovers_after_a_torn_append`, `store_compaction_crash_between_snapshot_and_log`, `store_restores_from_the_backup`, `coord_handlers_never_touch_the_nas` |
| 2 (done) | **Leases by the holder's word**: grants durable before the answer; beats list held leases; release on omission; `held_by_away`; `scenic pool release` | `OWED_S`; the TTL lapse re-grant for every member speaking `held-leases` (`coord::TTL`'s path stays for an older member until the gate drops it; `/work/beat` still renews and carries a job's progress, the pause and cancels) | `a_partitioned_member_s_target_is_never_regranted` (one member unheard for 3 h on a virtual clock; its job finishes; its end taken; no second grant), `an_away_member_s_leases_go_after_12_h`, `a_crash_before_the_answer_releases_the_grant`, `a_restarted_agent_s_lost_job_is_released_in_one_beat`, `mixed_versions_keep_the_old_rule`, `the_owner_releases_a_member_s_leases` |
| 3 (done) | **One beat by HTTP** (`POST /pool/beat`), the lead's answers carrying messages, holds, settings' generation and acks. Built: the beat carries the member's pool heartbeat (`agent::pool::Heartbeat`, with `beat_http`) and its agent's status (its plan's parts left out), posted by `agent::beat`'s own poster thread every `EVERY` (10 s), so a loop held on the NAS doesn't stop it; the lead keeps each member's last in memory (`crate::coord::beats`) and answers with the others' heartbeats, its own among them (`members`: the members' brokers and the lead's reached without the NAS), and `speaks: ["beat"]`. Every reader of a member's heartbeat (`Side::heartbeat`: reach, brokers, the merge's app check, mail) and of the members' statuses (the lead's `helpers`, the forecast's machines, `scenic pool status`, every Mac's page through the lead's `state/status.json` `helpers` and `beats`) reads the beats. Members are known from their beats and forgotten after 20 min without one (`beats::FORGET_S`). **The gate** (a member's `state/pool/members/<id>.json` and `state/helpers/<host>.json` while its lead didn't take the beat or said `nas_too`; the lead's own NAS heartbeat for an older member; the members' listing and NAS reads; `old_in_touch`) was deleted with task #1036, every Mac having said `beat` since 7 Nov: a member is known by its beat alone, forgotten 20 min after its last; its heartbeat still says `beat_http` and `mail_http`, for a lead on the app before | `state/pool/members/`, `state/helpers/` and the members' `state/status.json` writes once gated (above); the page's and `scenic pool status`'s reads of `state/helpers/`; `pool::beat`'s NAS read for a member heard by its beat; forgetting by listing alone (now: no beat for 20 min); the loop's own beat (`beat_lead`, `lead_beaten`, `held::BEAT_EVERY`) | `beats_go_on_while_the_nas_hangs` (the slow-NAS fake blocks every write of the status's NAS copy: the lead hears each of two members' 60 beats, and each hears the other), `a_forgotten_member_is_known_again_only_from_its_beat`, `a_member_takes_the_others_from_its_leads_answer`, `the_lead_answers_each_member_with_the_others` |
| 4 (done) | **The view by HTTP**: pageview asks `POST /pool/view` from the lead. Built: the lead's coordinator answers from memory (`coord::pool_view`): its view (`view_of`), the pause, the Macs' addresses (the brokers its agent read, else the beats), its app, and its agent's status (its local copy, read only when its file changed since the ask's `status_at`, a token of the file's time, inode and length: each write renames a new file into place), the forecast's input and the page's regions its agent hands it (`set_page_forecast`, `set_page_regions`), each only when the ask's time differs. The lead's pageview (the page is the lead's alone since #882 step 4, docs/roles.md §3.3) asks its coordinator in process (`pageview::Lead`) every 2 s (`pageview::LEAD_EVERY`) while a page asks, on a thread of its own (`tick_lead`), apart from the NAS's reads; `POST /pool/view` still answers by HTTP. A lead that can't be asked: the last view kept, the answer saying how old it is (`view_age_s`) and since when the lead isn't reached (`view_unreached`), the page saying so. The members' beats in the store (`beat/<member>`: heartbeat and time, read back as the store opens). **The gate** (the lead's writes of `view.json`, `forecast-input.json`, `regions.json` and `pause.json` while a member heard didn't say `view`, `Coordinator::page_on_nas`) was deleted with task #1036. Planned: the history by HTTP (the members' events in the beat, decision 5: today each Mac still reads the members' history files on the NAS every 12 s, `pageview::EVERY`, the one NAS read left on a page's path, on its own thread); the lead's `state/status.json` stays (`scenic status` and the map server read it: step 8; the menu bar and a member's `all_new` no longer do, docs/roles.md step 5) | pageview's NAS reads of the lead's status, view, forecast's input, regions, pause and the members' heartbeats once its lead serves `/pool/view` (gated, above); the lead's writes of `view.json`, `forecast-input.json`, `regions.json` and `pause.json` (#1036) | `the_page_shows_the_lead_s_view_with_the_nas_hung` (a real coordinator, in process; every NAS call hangs: the view, its pause changing, the forecast and regions answered three times over while the history's read waits), `a_page_asking_with_the_lead_down_gets_the_last_view_with_its_age`, `a_lead_s_restart_keeps_the_members_beats` |
| 5 (done) | **Mail, holds and settings by HTTP only**: lead → member messages in beat answers; `/pool/hold`, `/pool/settings`; the CLI posts to its own agent, which posts to the lead. Built (`crate::coord::asks`, `coord::inbox`): a member's beat says `asks` (`beats::ASKS`) and the number up to which it took the lead's messages (`took`); the lead's answer says it serves them, and carries the messages after that number (`msgs`, from its agent's `mail.json`), the member's hold as the lead keeps it (`hold`) and the settings' generation (`settings_gen`). A member's mail to the lead goes by `/pool/mail` alone, into the lead's store (`inbox/<member>`; an older agent's `inbox.json` taken in once), those not taken posted again while the lead answers its beats. The owner's asks (another Mac's pause, its own pause told the lead, a setting) are queued on the agent's disk (`asks.json`) and sent by its beat's poster once the lead answers saying `asks`, each kept until answered 200 (the lead's own agent hands them its coordinator in process); the lead keeps each hold (`hold/<member>`) and the settings (`settings`, with `by`, `at` and a generation) in its store, the later `at` winning (whole seconds, each Mac's clock: two hold asks in one second go to the later arrival, kept a second on; a repeat changes nothing). A lead that lacks what a member has (a lead move, a lost store) is told again: the member's own hold when the lead's word of it changes, and each setting the member had that the lead lacks or has older, as it was (`Beats::set_settings`, seeded from `pool-settings.json`). An ask the lead refuses as it is (400, or in process `asks::Refused`: most likely a newer Mac's ask an older lead doesn't know) leaves the queue, the asks after it going on, and its answer is kept on the asking Mac (`agent::answered`, `asks-answered.json`, task #922; so is a refused `/api/pause`, which is no longer asked again forever as if the lead were away): its status carries the refusals not yet seen (`refused`: what, from which Mac, the lead's reason, when), which the build page shows by Mac with a Dismiss (`/api/asked` on that Mac marks it seen) and the menu bar shows for its own Mac, until seen or a later ask about the same thing is taken; `scenic pause` and `resume` wait up to 20 s for the lead's answer to theirs and exit 1 with its reason. A coordinator without its store (not the lead's, or not open yet) answers 503, asked again (it had been a 400, dropped). The settings now: `placement` (the agent's reader), `nas-budget` and `nas-target` (the lead's NAS prober), `batch-minutes` (the lead's scheduler: docs/pool.md §7.7, Batch length) and `overdue-factor` (the lead's status flags, agent::flags), neither with a switch file, so an older lead refuses them; `scenic pool set <name> [<value>|off]` and `scenic nas-target` write an ask to this Mac's agent (`setting-asks/`), and read the settings it last had (`pool-settings.json`), no NAS read. **The gate** (the mailboxes written to and read for a Mac that didn't say `asks`, a hold's `state/pool/holds/<member>.json`, the switch files written for a setting and taken into the lead's settings, `Answer::asks_on_nas`, `held::on_nas`) was deleted with task #1036. Planned: `nas-room.json` (every job reads it: a setting once the jobs have it from their agent), `delta-catalog`'s and `near-slope`'s files deleted outright (on everywhere), `journal-gc` with step 6, `state/pool/map-serve` (the map server's: not the pool's agent) | `POST_AGAIN` and the mailbox fallback after a failed post to a lead serving the asks (`Side::post_off`); the switches `resume`, `brokers` and `memory-guard` (on everywhere) and their readers and off paths (`resume::on`, `brokers::on`, `memguard::on`, `SWITCH`es, `memguard::DEFAULT`, `Agent::resume`, `brokers`, `guard_on`, `guard_stops`, the planner's `resume`, `brokers`, `guard_on`, the capacity's `guard`); the lead's reads of `nas-budget` and `nas-target.json` (`budget::nas::read_switch`, `read_target`, `set_target`); `inbox.json` (the store's `inbox/<member>`); `scenic nas-target`'s NAS write. With the gate (#1036): the mailboxes (`state/pool/mail/`, `Mailbox`, `mail_path`), `state/pool/holds/` (`hold_path`, `write_hold`, `HOLD_READ_EVERY`), the switch files' writes and readers (`placement::on`, `write_setting`, `setting_of`, `asks::file`) | `a_remote_pause_reaches_the_member_in_one_beat`, `settings_survive_a_lead_restart`, `an_ask_while_the_lead_is_down_is_sent_when_it_returns`, `an_ask_the_lead_refuses_is_said_and_stops_none_after_it` (agent::beat), `the_build_page_reads_a_macs_refused_asks_and_marks_one_seen` (coord), `a_members_mail_goes_by_http_alone_to_a_lead_that_serves_it`, `a_post_refused_stays_queued_until_the_lead_takes_it`, `a_post_answered_survives_the_leads_crash` (agent::pool) |
| 6 (done) | **Hand-offs by HTTP**: `POST /pool/handoff`; the lead merges from the store; acks only once a records snapshot naming them is saved. Built (`crate::coord::handoffs`): a member's driver keeps each job's hand-off whole in its saved state (`journal::Mine`, as before its first write); its beat (`agent::beat`'s poster) says `handoff` (`beats::HANDOFF`) and lists their keys (`Beat::handoffs`), and the lead's answer says which a saved snapshot names (`merged`: the member drops them) and which it lacks (`lacks`: posted next, `{"entry": …}`, each taken into the lead's store, `handoff/<key>`, before its 200; a repeat taken once by its key). The lead's agent hands its driver what its coordinator took (`Heard::handed`), merged from memory with its own jobs' (never written to the journal now) by the same checks and in lease order as the journal's (`records::merge_from`); once the snapshot naming them is saved and its driver's state with it, they leave the store and are answered `merged` (`Out::merged`, `Handoffs::merged`: kept in memory until the member stops listing them; a restart forgets them, and the member's next beat has them posted again, then passed over). A lead's take-up never starts from records older than those it saved last (`Saved::records`, checked by `driver::take_up`: a stale read waits), since what a save acknowledged is on no member's disk any more. A lead moved by the owner: the old lead acknowledges only while a read of the declaration after its save names it, and the new one takes the one path's records up only `STALE_S` (10 min) after finding itself declared (step 8c; by terms before), so the old lead's last acknowledged save is in what it reads (`a_lead_moved_mid_step_acknowledges_nothing`, `a_moved_lead_s_last_acknowledgement_is_in_the_next_lead_s_records`). GC removes only what its queue on the lead frees, each name marked deleted there before it goes, so the merge refuses a hand-off bringing one being removed, or removed under its lease, and a hand-off it takes holds its names from GC until the records after it are seen (`agent::gcqueue::refusal`, docs/maplog.md §7.4). A member drops a hand-off, by HTTP or by the journal, once acknowledged (`Mine::acked`; it kept the acknowledged ones before, re-telling each new term). **The gate** (deleted by the rule for old-app paths, above; due, as every Mac runs an app that says `handoff`, task #1036 left it): a member whose lead's answer doesn't say it (an older lead, or a 404) writes its hand-offs to the journal and tells the lead by mail (`driver::Route::Journal`; before any answer since its start it keeps them, `Route::Wait`); the lead reads told entries from the journal as before, and lists the journal (its take-up's listing, daily, and the last two days every `SWEEP_S`), being caught up only by a listing under a day old, only while a member heard within 20 min doesn't say `handoff` (`Beats::journal_on`, `Heard::http_only`). Measured: none yet (not live) | Outright: `journal_gc` with its marker and fence (`state/journal/gc.json`, `records::start`'s and GC's fence, `Records::forget` and the records' horizon, `Merged::forgotten`, `Mine::forget_before`), the heartbeat's `journal` and `fenced` (`Beat::fenced`, the lead alert's fenced line), `scenic pool journal-gc` and the `state/pool/journal-gc` switch's reader; the lead's own jobs' journal writes; the members' acknowledged keys (`Mine`'s per-term acks). Kept: `ALERT_S`, the no-lead alert's hour. Gated, deleted when the gate drops: `state/journal/`'s writes and listings, `Tell`/`TELL_AGAIN`, `MOVED_ON_S`, the hour's wait (`UNREADABLE_S`), `SWEEP_S`, `caught_up`'s day listings, `Msg::Ack`'s empty `horizon`, GC's `unmerged` read of the journal | `a_hand_off_posted_twice_merges_once`, `a_lead_killed_after_taking_before_saving_loses_nothing` (its store kept: merged after the restart; its store lost: the member posts it again on the new epoch), `hand_offs_merge_with_the_nas_slow` (records save waits, acks wait, nothing lost), `mixed_versions_an_older_member_s_journal_hand_off_is_merged` (agent::pool); `a_lead_lists_the_journal_only_while_a_member_writes_it`, `the_lead_merges_its_own_jobs_hand_offs_from_memory`, `a_take_up_never_starts_from_records_older_than_it_saved`, `a_lead_moved_mid_step_acknowledges_nothing`, `a_moved_lead_s_last_acknowledgement_is_in_the_next_lead_s_records` (pool::driver); `a_hand_off_is_in_the_store_until_merged_and_answered_so` (coord::handoffs) |
| 7 (done) | **Budgets and small leases**: Wikimedia tokens and the answers lease from the lead; resume progress in the beat. Built (`crate::coord::budgets`): the lead counts each class's window's tokens in memory by its own clock (`POST /pool/budget/take {class}`: a batch of one token, its window, and how long it may be spent, counted on the asking Mac from its ask, so it never outlives the lead's window; or a wait), a 429 holding every class (`/pool/budget/hold`, kept in its store as `budget-hold`, so a restart keeps it): one token a take, so no Mac holds tokens it hasn't spent and every Mac stops at its next token (task #984: batches of four were spent on after another Mac's 429; a take is a round trip to the lead for each metered connection, at most four a second pool-wide, a kept connection's 5-33 ms from the M1 over Wi-Fi); a restarted lead gives nothing in the window it first hears an ask in. A member's agent gives its proxy the lead over HTTP (`budgets::OverHttp`; the lead's agent its coordinator in process, `InProcess`: `crate::budget::Budget::set_lead`); the lead unreached, the proxy takes nothing and asks again the next window (§7.1). A lead whose clock is set back into a window it gave in gives none till that window ends. The answers' lease: a job asks its agent's proxy (`/lease/answers?op=take\|renew\|release`, `answers::Leasing`), which asks the lead (`POST /pool/lease/answers`, `answers-lease` in its store before the answer; a renew only while it's that holder's and live, so a holder whose lease lapsed and was taken never writes on it again; a job on an app before task #1036 takes the NAS's `state/pool/answers.lease` first as well, then the lead's, so the lead's is the one both hold). Resume progress: kept on the member's disk (`progress.json`), carried whole in each beat (`Beat::progress`), kept by the lead when it changes (`progress/<member>`, with the owner's releases: `POST /pool/progress/release`, carried back in the beat's answer, `progress_released`); the lead's plan reads them from memory, a member's for its day whether heard lately or not. **The gate** (every Mac taking the budget and the lease on the NAS while a member heard didn't say `budgets`, `Answer::budgets_on_nas`; the members' progress files and their releases there; the lead's batches given only while every member said `budgets-by-lead`, `budgets::gives`, and `budget::SWITCH_MS`; each Mac's own share of the rate with the NAS away) was deleted with task #1036: a proxy takes from the lead alone, nothing while it has none yet (the agent starting) or none answers, and its jobs' answers' lease likewise (503, asked again). A member's beat still says `budgets` and `budgets-by-lead`, for a lead on the app before. Measured: none yet (not live) | Outright: `answers::budgets_on_for_job` and the agent's always-true `leased` (where the lease is taken: `answers::Leasing`); `held::asks_on_nas` (one rule for both gates, `held::on_nas`); `Budget::hold_nas` no longer public (`hold_pool`). With the gate (#1036): `state/pool/wiki/` (`Budget::lease`, `clean`, `hold_nas`, `KEEP`, `CLEAN_BY_NAME`, `HOLD_NAS_MAX_S`, `budget::DIR`), `state/pool/answers.lease` (`try_lease`, `renew_lease`, `release_lease`, `lease_is`, `answers::LEASE`), `state/pool/progress/` (`resume::read`, `write`, `release`, `released`, `path`, `release_path`, `DIR`, the agent's `progress_to_nas`, the CLI's NAS release), `Budget::members`, `Source`, `Rate::batch`, `NoLease`, `PoolView::heard` | `the_wiki_budget_holds_across_three_macs` (three Macs' proxies on one lead's windows: no window past its class's tokens), `the_answers_lease_is_one_holders`, `a_members_progress_and_its_release_are_kept`, `a_members_beat_carries_its_progress_and_the_answer_its_releases` (coord::budgets); `the_budget_waits_for_the_lead_while_it_is_down`, `nothing_is_taken_before_a_lead_is_given`, `a_leads_batch_ends_with_its_window` (jobrt's budget); `the_answers_lease_is_the_leads_through_the_proxy` (answers) |
| 8a (done) | **The term by the beat**: a member learns its lead and the current term from its lead's beat answer, not the NAS. Built: the lead's agent tells its coordinator the term its driver leads after each step (`Coordinator::set_led`, in `agent::lead::after`); every beat's answer carries it (`Answer::term`) while it is the term its leases are granted in and its member is said, as the answers' `x-pool-term` and `x-pool-member` say them and its last step's own reads found it leading (`Out::vouched`: the declaration read naming it, no later term found; a lead whose share fails those reads keeps its view and leads on, but says no term, so its members read the declaration themselves and find an owner's move). None while it leads none, its step is taken as hung, or a take-up isn't told yet. A member's registry keeps it with the answer's time (`Beats::lead_says`) and its pool side hands it to its driver's step while that answer is under `PAGE_AWAY_S` (35 s) old by its own clock, and not timed after it (a clock set back) (`Heard::lead_says`); the step takes the term (no older than its view) and its lead as the declared one, and reads neither the declaration nor the terms. A member opening with a saved state stats that state's term (`exists`), where it listed the terms (794 names on 7 Nov, 3-33 s a listing under load). **The gate:** an older lead's answers carry no term, so its members read the NAS as before; an older member reads past the field. Nothing to lift: what it gates is the NAS path, which stays while the lead is unreached (the owner may have moved the lead to that member: with no word for 35 s it reads the declaration and takes the lead: the driver test below), until 8c removed the terms (since then the declaration alone, read only while no answer came lately). **Measured** (the driver's NAS calls, counted in its test, below): a member's step 2 (the declaration read, the next term stat'd) and its first 7, before; 0 while its lead answers, after: per member, 6 calls a minute fewer at a step every 20 s (`TICK_S`). Live: not yet (not published) | Outright: today's files for older apps (`agent::pool::write_today`, `Run::keep_today`, `Run::today`, `Side::readers_ready`, `Beat::reads_records`: the gate passed live, today's `manifest.json` last written 31 Oct 21:43, a week before), `list_patiently` and `open_backoff` (the open's listing) | as renamed with 8c: `a_member_learns_its_lead_from_its_answer_with_no_nas_call` (pool::driver), `a_member_learns_its_lead_by_the_beat_with_the_declaration_hung` (agent::pool), `the_lead_s_answers_say_it_leads_only_as_its_own_step_read_it` (agent::lead), `a_beat_s_answer_says_the_lead_while_it_leads` (coord), `a_member_takes_its_lead_from_a_fresh_answer_alone` (coord::beats) |
| 8b (done) | **The lead's records on its own disk, then one path on the NAS** (task #1093). Why the disk first: the agent's NAS writes run under a 20-s deadline on four threads (`agent::nasread`), and a write past it is left running (the next to its path refused meanwhile, "an earlier write to it still runs"); one an earlier process left running as it stopped, or a lead's before the owner moved it, can land at one path over a later save, and a lead taking up from the NAS would take up from it, or refuse it as older than it saved (`records::Older`) and never lead; per-term paths are what fence that (docs/pool.md §4, invariant 5), so each term's own snapshot is still written (below). Built: each save (`Records::save`) goes first to the lead's disk (`driver::Io::keep`: `agent/pool/records.json`, written whole and flushed; nothing else is written if that fails, `Unsaved::Kept`), then to `state/build/records.json` and `brief.json` (`records::ONE`, `ONE_BRIEF`, naming their term and `seq`), then to the term's own `state/build/term/<E>/records.json`, always: only that term's lead writes it, so a late write of an earlier term's lead over the one path never hides it (without it, a late write of term E-1's landed over the one path and the owner moving the lead before term E's lead saved again, the next lead would take up from term E-1's, losing what term E's lead acknowledged: `a_late_save_of_an_earlier_term_never_hides_a_later_one`). A take-up (`records::start`) starts from the lead's own records, in memory or kept on its disk (`Driver::kept`, only when its saved state is its own), when they're of the term before or this one: no NAS read, the term before's file not read either; else (another Mac led the term before, or its disk's copy is lost or older than `Saved::records`) from the NAS's one path, or a term's own above it (a late write of an earlier term's at the one path: the term before's own). The disk's copy is written durably (`seglog::write_durable`: its folder flushed after the rename), so a power cut after a save reached the NAS never leaves an older copy that a take-up would start from unread. A take-up kept on its disk whose NAS save fails leads on, acknowledging nothing and not caught up until a save reaches the NAS (on 9 Nov at 20:40Z such a failure dropped term 812 and restarted the lead's agent, leaving three jobs for the next). The lead saves again unchanged `driver::RESAVE_S` (10 min) after its last save reached the NAS, so a late write at the one path stays that long at most: one of this term's (an earlier process's take-up of it, as at the term's own path before) is read meanwhile; one of an earlier term's is passed over. Readers (buildstore::records `now`, `brief_now`) read the one path and stat the term after its own; a term after it made (being taken up, a lead on an older app, a late landing), they walk the terms' own down to its term (`newest_over`), never past it. **The gate:** a member's beat says `records` (`beats::RECORDS`); the lead's saves write the term's own `state/build/term/<E>/brief.json` beside while a member heard within 20 min doesn't say it (`Beats::per_term`, `Heard::one_only`). It lifted by itself once every member heard ran this app; its code (`per_term`, `one_only`, `Beats::per_term`, `RECORDS`, `Records::save`'s write of the term's own brief) was deleted with step 8c, every Mac then running 20261111-0225 (which says `records`), by the rule for old-app paths (above). An older app's reader, with the gate lifted, finds no brief of the current term and reads its snapshot (`brief_now`, as for a term whose lead was on an app before briefs): the map server, the lead's, so reads the current records until it restarts into the lead's app (after a minute of the map idle), never an earlier term's; so does a lead rolled back to an app before step 8b, whose take-up walks the terms' own. **Measured** (the M4 over the LAN, 10 Nov, the share's term 813: records 13.9 MB, brief 6.5 MB; reads past the page cache, median of 5; tools: the tests below count the calls): a reader, before 5 NAS calls (the hint read, the term stat'd, the next stat'd, the term read, its snapshot read), 434 ms for the records and 296 ms for the brief; after 2 (the one path read, the next term stat'd), 343 ms and 153 ms. A take-up's reads, before 3 NAS calls (the term before's file, the new term's records, the term before's 13.9 MB), 404 ms; after none, its disk's copy read in ~1 ms. A save: before 2 NAS writes (the term's snapshot and brief), after the disk's copy (5 ms, before its folder's flush) and 4 NAS writes while the gate holds (the one path's two and the term's own two), 3 once it lifts (the term's own brief no more); one more save every 10 min while nothing changes. Live: not yet (not published) | Outright: `records::take_up` and `TakenUp` (public, used by tests alone: moved into records' tests); the CLI's `scenic redo` and GC's journal check finding the term and walking the records themselves (`Records::now`); a restarted lead's reads of its records and of the term before. Gated (above): `state/build/term/<E>/brief.json`, ~4-6 MB a term | `a_lead_whose_take_up_s_nas_save_fails_leads_on`, `a_late_save_over_the_one_path_is_saved_over`, `a_moved_lead_s_last_acknowledgement_is_in_the_next_lead_s_records`, `a_take_up_never_starts_from_records_older_than_it_saved` (its disk's copy lost) (pool::driver); the simulator's one-path checks. Its readers' and restart's tests are 8c's (`a_reader_reads_the_one_path_alone`, `a_restarted_lead_takes_up_from_its_own_disk_in_a_new_epoch`); its terms' and gate's went with 8c |
| 8c (done) | **Terms out** (task #1124): no term is made or read; the lead leads its store's epoch (§4.3). Built: the declared lead takes the records up once a process (`driver::take_up`): it waits for its store (none before its coordinator starts, after the agent's first step), starts from its own records (an earlier try's, its last led, or its disk's copy, none older than `Saved::records`), and reads the one path once: newer than its own (another lead saved them: the owner moved the lead here, or away and back) or none of its own (its disk's copy lost), it takes the one path's up, read again `STALE_S` after this process first found itself declared, so the last save the lead before it acknowledged is in them; no one path nor brief and none saved before (a new pool), today's files at once. Never from records older than the one path's brief (`records::brief_mark`, read before the one path): a save writes the brief only once its snapshot is whole, so a snapshot older than its brief is an earlier lead's write landed late over a newer save, the case the terms' own paths fenced (step 8b); the take-up waits, saying the Mac whose records the brief is of has them (`a_late_save_of_an_earlier_lead_never_hides_a_later_one`). Its epoch is raised past the records' (`Io::epoch`, `Store::raise_epoch`; past another Mac's store backup by `RESTORE_EPOCHS`: `agent::pool::epochs`), the records renumbered (epoch, 0) and saved. The acknowledgement's guard is a read of the declaration after the save (it acknowledges only while that names it), and the lead steps down when its step's read names another member; GC sweeps on a step whose read named it (`Out::vouched`, `Gates::vouched`), no re-assertion before; and it grants and plans only while such a read is under `driver::VOUCH_S` (5 min) old by both its clocks (`Out::duties`), so a lead whose reads fail stops before one the owner declared meanwhile takes up, `STALE_S` after finding itself declared. A raise of the epoch is durable before it answers, a retry after a timed-out raise too. Saves write the one path and its brief alone; readers read the one path alone (`buildstore::records::now`, `brief_now`), today's files only while no lead is declared (the declaration is the pool's "has begun" mark, and `out::check_writer`'s). Members learn the lead from `Answer::leading` (its member, host and app), else the declaration; the answers say `x-pool-epoch`; a page's ask goes to a lead whose app its answer said is this Mac's. Records and lease JSON keep the key `term` (the epoch's value), which older apps read. Kept of the old: a gap (the Mac slept) makes a lead whose members write the journal list it again (`GAP_S`; the journal is #1125's). **Mixed versions:** a lead on this app with members on 20261111-0225: they find no `term` in its answers and read the declaration and stat `terms/<E+1>.json` each step (none is made), their page asks not passed on (the lead's app unknown to them) until they update; their readers stat `terms/<epoch+1>.json`, never there, and read the one path. A member on this app with a lead on 20261111-0225: it reads past that lead's `term`, learns the lead from the declaration, and reads the one path (current: that lead writes it each save). An app before 8c leading again (a rollback, or the lead moved to a Mac not updated yet) numbers leases by terms again, below every epoch (a later lease's hand-offs passed over), and without its own records takes up a term's old snapshot, losing what was acknowledged since: so `publish.sh --rollback` refuses to go back past this release (`tools/app/rollback-floor`'s first line, `878-8c`, which the previous version's tree lacks: `lib.zsh`'s `rollback_ok`; a fix goes forward instead), and the owner doesn't move the lead while the update reaches the Macs (minutes: each beat says its app). **Measured** (NAS calls, counted by the tests): a reader, 2 calls before (the one path, the next term stat'd), 1 after; a member with no word from its lead, 2 a step before (the declaration, the next term), 1 after; the lead's step, a stat of the next term after a save before, a read of the declaration (~60 bytes) after; a take-up, none before (its disk's copy), two reads after, once a process: the one path's brief (~6.5 MB, ~153 ms) and the one path (~14 MB, ~343 ms on 10 Nov), and in the agent the store's backup (`state/lead/store.json`) read for its epoch (`agent::pool::epochs`) at each try and as it raises it. Live: not yet (not published) | `state/build/terms/`, `lead.json`, `state/build/term/` (but `forecast-input.json`: §9), `state/coord/term/` (none written or read); `pool::term`, the take-up's term walk (`newest_over`, `term_one`, `FIRST`), re-assertion (`STALL_S`, the gap's), `Answer::term`, step 8b's gate. Not done: `state/coordinator.json` (still the lead's contact: members find its coordinator by it; deleting it needs the address elsewhere, the declaration say); the share model in `pool::sim` (step 9's) | `a_reader_reads_the_one_path_alone` (buildstore::records), `a_save_writes_the_one_path_and_its_brief_alone` (pool::records), `the_epoch_raised_past_a_take_ups_records_stays_past_them` (pool::store), `a_restarted_lead_takes_up_from_its_own_disk_in_a_new_epoch`, `the_owners_edit_of_the_declaration_moves_the_lead`, `a_moved_lead_s_last_acknowledgement_is_in_the_next_lead_s_records`, `a_lead_moved_mid_step_acknowledges_nothing`, `a_take_up_never_starts_from_records_older_than_it_saved`, `a_take_up_waits_for_its_store_and_a_lost_state_for_the_one_path`, `a_late_save_of_an_earlier_lead_never_hides_a_later_one`, `a_lead_whose_declaration_reads_fail_stops_granting_before_a_next_lead_could`, `a_member_learns_its_lead_from_its_answer_with_no_nas_call` (pool::driver), `a_member_learns_its_lead_by_the_beat_with_the_declaration_hung` (agent::pool), `a_beat_s_answer_says_the_lead_while_it_leads` (coord), `a_member_takes_its_lead_from_a_fresh_answer_alone` (coord::beats); the simulator's invariants (only the declared lead leads, each take-up's epoch past every snapshot landed, an epoch's snapshots never going back, no acknowledged entry missing from a snapshot of its epoch or a later one: 2,000 seeds; by hand at review, 60,000 long runs of 2 h of faults and each knob alone 3,000 seeds: none broke a safety invariant, 6 runs ended with an entry not yet merged or refused, or a take-up in the last ten minutes, as 7 did on the commit before the review's: the end checks' bounds against `STALE_S` and a 600-s entry lag) |
| 9 | **The simulator over the network**: the lead and two to four members over the network fake (drops, delays, lost answers, one-way links, partitions of hours, lead kills mid-append, member restarts, the lead's disk restored from backup) | — | invariants at every step: no target under two live leases; every hand-off ever posted merged exactly once or refused with why; no lease id reused; acks only for saved records. 2,000 seeds |
| 10 | docs: pool.md §3, §6, §7.3, §7.5, §7.8, §9, §10, §12, §13 rewritten to this; agent-parts.md's **pool** part as the HTTP client; this doc folded in | — | — |

Order: 1 before all; 2 and 3 together or 2 first; 4, 5, 7 in any order after 3; 6 after 2; 8a after
3, 8b after 8a, 8c after #881, 6 and 8b (all done); 9 alongside 6. Steps 3 and 4 are what the owner sees first (no Mac out of touch over a
stuck share, pages fast on every Mac).

## 9. What gets deleted

Once every step has landed and its gate has passed, the pool writes nothing small to the NAS.
Gone already (task #1036, steps 3, 4, 5 and 7's gates): `state/pool/members/`, `state/helpers/`,
`state/pool/mail/`, `state/pool/holds/`, `state/pool/progress/`, `state/pool/wiki/`,
`state/pool/answers.lease`, the switch and setting files under `state/pool/` (`placement`,
`nas-budget`, `nas-target.json`), `state/coord/term/<E>/view.json`, the terms' `forecast-input.json`
and `regions.json`, `state/build/pause.json`, and the code that wrote and read them (the members'
listing and forgetting by listing among it). Gone with step 8c (task #1124): every write and read
of `state/build/terms/`, `state/build/lead.json`, `state/build/term/<E>/` (but the backtest's
reads of `forecast-input.json`, below) and `state/coord/term/<E>/`, and `pool::term` (the
declaration and the role are buildstore::records::lead's). Gone once the rest land:

- the files: `state/pool/members/`, `state/pool/mail/`, `state/pool/holds/`, `state/pool/progress/`,
  `state/pool/wiki/`, `state/pool/answers.lease`, `state/pool/nas-probes.json`, the switch and
  setting files under `state/pool/`, `state/journal/` (entries, `rejected/`, `gc.json`),
  `state/coordinator.json`, `state/status.json`, `state/helpers/`, `state/build/pause.json`,
  `state/coord/history/` (the day tars stay);
- the code: `pool::journal`'s NAS side, `pool::journal_gc`,
  `pool::beat`'s NAS side, the mailboxes and `coord::inbox`'s file, `coord::keep`, the pageview's
  NAS reader, `MOVED_ON_S`, the lease TTL's re-grant, the members'
  listing and forgetting by listing, the share model of `pool::sim`;
- in `agent::nasread`: the pool's and switches' traffic (the Switches pool entirely; the Records
  pool keeps only the jobs' and the plan's reads, the Writes pool only the records' saves and the
  backup), which is what agent-parts.md §6 foresaw;
- the asides and stale-read rules stay for the records and bulk files (pool.md §3), but no longer
  for heartbeats, statuses or mail.

The terms' files on the NAS (measured 11 Nov over SSH, read only), for the owner to remove once
every Mac runs an app with step 8c (by its beats; until then an app before it reads them):

- **keep**: `state/build/term/<E>/forecast-input.json`, E 232 to 673 (352 files, ~283 MB), which
  `agent::fccheck::inputs` reads for the backtest where the forecast inputs' archive
  (`state/coord/forecast-inputs/`, from 4 Nov) lacks them: their folders stay with that file
  alone, until the archive covers them or the check no longer reads term folders;
- **remove**: every other file under `state/build/term/` (`records.json` in 836 folders,
  `brief.json` in 320, `regions.json` in 290, `term/1/first.json`, the `*.aside.tmp` and `*.tmp`
  left there), the folders of terms with no `forecast-input.json`, `state/build/terms/` (840
  files, ~5 MB), `state/build/lead.json`, and `state/coord/term/` whole (647 `state.json`, an empty
  folder a term up to 673, two left temporary files; ~1.3 GB): ~12 GB in all (`state/build/term/`
  ~11 GB less the kept 283 MB). As one job on the NAS (`rm -r`, `find … ! -name forecast-input.json
  -delete` over SSH: SMB removes file by file, and a held file's aside can't be removed for hours),
  by the owner, never the agent's loop; the Synology `@eaDir` folders go with them.

## 10. Open decisions

1. **Releasing an away member's leases:** never automatically (proposed: the strict reading of
   "never run twice"; the owner releases), or after a long bound (say 12 h unheard), accepting
   that a member back after that may have run a job a second Mac then runs again (its result
   refused at merge, so the records stay right; only work is doubled).
2. **Records by HTTP too:** agents (jobs' `Out`, plan readers on members) `GET /pool/brief` from
   the lead instead of reading the NAS snapshot, which ends the stale-records reads; the map server
   and the CLI keep the NAS copy. Proposed: yes, as a step after 8, since the brief is ~4 MB and
   read every minute by each Mac.
3. **The store's format:** JSON lines plus a JSON snapshot (proposed: no new dependency, readable
   by hand, a few MB) against an embedded database (SQLite: a new dependency, fewer hand-written
   recovery paths).
4. **`backup_every`:** 10 min proposed; it bounds only the owner's settings lost with the lead's
   disk (everything else members re-send).
5. **The history:** members' events in the beat to the lead's one file (proposed) against members
   keeping their own files on the NAS as today (appends, which SMB handles less badly than creates).

## Decisions (PM, 30 Oct night)

1. An away member's leases are released after 12 h unheard. A repeat costs doubled work only, since the merge refuses the old lease's result, and a lease no one releases would stall the build while the owner is away.
2. The records brief is served over HTTP to agents too, as a later step.
3. Store format: JSON log plus snapshot (no new dependency).
4. The store is backed up to the NAS every 10 min.
5. History: members' events go in the beat, to one file on the lead.
6. Step 5 deletes a switch only once it reads on for every member; a switch still off anywhere moves to `/pool/settings` instead.
