# Pool coordination by HTTP on the lead

**Planned (task #878); steps 1, 2 and 3 are built, the rest planned.** This doc says how the pool's
coordination moves off the NAS onto the lead, which holds it on its own disk and serves it over
HTTP. For the steps not yet built (§8 says which are), docs/pool.md and the code are what is true. It assumes #881 (the Mac mini
as a fixed lead: no election, no takeover) has landed; where #881 leaves a piece of the
election machinery in place, the step that deletes it here says so. docs/agent-parts.md's **pool**
part (#871) is the client side of this design.

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
4. **Every post is idempotent:** it carries the member's id and a sequence number; the lead keeps
   each member's highest number taken and answers a repeat with the first answer. A lost answer
   is retried, never doubled.
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
| `state/pool/members/<id>.json` (pool heartbeat) and `state/status.json` / `state/helpers/<host>.json` (the agent's status) | each member, each lead | **one** `POST /pool/beat` every 10 s and on a change (agent-parts.md §4.2): conditions, slots, held leases, reach, tasks, broker addresses, status sections with ages | in memory, the last of each member; in the snapshot for the page after a restart |
| `state/pool/mail/<to>/<from>.json`, and the lead's `inbox.json` (#642) | each sender | member → lead: `POST /pool/mail` stays, now the only path; lead → member: carried in the answer to the member's next beat (`msgs`), so members need no endpoint for it | the outgoing queue per member, until a beat acknowledges it |
| `state/journal/<day>/<lease>.json`, `rejected/`, `gc.json` | each member; the lead's GC | `POST /pool/handoff` (the hand-off as today's entry, with its sizes) | the log; merged into records; the refusals in the snapshot |
| `state/build/terms/<E>.json`, `state/build/lead.json` | create-new by any Mac | none: the lead is #881's, fixed; members learn its address from #881's declaration | an **epoch** (§4.3), bumped each start |
| `state/coord/term/<E>/state.json` (gone with `coord::keep`, step 1) | — | none | the coordinator's state is the store's |
| `state/build/term/<E>/status.json`, `state/coord/term/<E>/view.json`, `forecast-input.json`, `regions.json`, `state/build/pause.json` | the lead | `GET /pool/view` (one document with each part's time; `If-None-Match` by generation) and `GET /pool/view/forecast-input` (big, fetched only when its generation changed) | in memory; regenerated from the store and the plan |
| `state/pool/holds/<member>.json` (another Mac's pause) | any Mac | `POST /pool/hold {member, on, at}` from any Mac's agent; the held member gets it in its next beat answer | a setting per member |
| switches and pool-wide settings: `state/pool/resume`, `placement`, `brokers`, `memory-guard`, `nas-budget`, `nas-target.json`, `nas-room.json`, `battery-floor.json`, `delta-catalog`, `near-slope`, `journal-gc` | the owner's CLI | `GET/POST /pool/settings`; each beat's answer carries the settings' generation, a member fetching them when it changes | settings, each with `by` and `at` |
| `state/pool/progress/<member>.json` (resume progress) | each member | in the beat (a job's progress is its slot's) | per lease |
| `state/pool/wiki/<class>/<window>-<slot>` (Wikimedia tokens), `state/pool/answers.lease` | create-new by every job's proxy | `POST /pool/budget/take {class, n}` → tokens or a wait; `POST /pool/lease/answers` | a token bucket per class in memory (not durable: a restart starts the window afresh, which only slows); the answers lease in the log |
| `state/pool/nas-probes.json` | the lead's prober | none | the lead's local store |
| `state/coord/history/<day>/<member>.jsonl` (members' brokers' events) | each member, appends | events ride in the beat (`events`, since the last acknowledged) | the lead's history file on its disk; a day's tar goes to the NAS by the daily `gc` as today (§3.2) |
| `state/coordinator.json` (the contact) | the lead | none: the fixed lead's address is #881's | — |

Two heartbeats become one: the pool's and the agent's status are the same post, so a member is
never "beating" in one and "out of touch" in the other.

### 3.2 Stays on the NAS

- **Built files, sources, caches, scenic units, raw archives:** unchanged (docs/storage.md).
- **Records snapshots** (manifest, keys, pending, sizes; and the brief), written whole by the lead
  alone after merges, read by jobs on every Mac and by the map server. They lose the per-term
  folder: `state/build/records.json` and `brief.json`, each naming the store's epoch and `seq`.
  Readers that are agents may instead `GET /pool/brief` (open decision 2).
- **The store's backup:** `state/lead/store.json`, the store's snapshot written whole every
  `BACKUP_EVERY` (10 min) and at a clean stop; one file of a few MB. Read only to rebuild a lost
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
  Built in memory, not in the store (step 3, `crate::coord::beats`): each member's last beat
  (heartbeat and status; a lead's restart hears every member again within a beat). Planned, with
  the steps that move them: the members' beats in the snapshot for the page after a restart (step
  4), each member's sequence high-water, the
  queued messages to each member, hand-offs taken and not yet in a saved records snapshot,
  refusals, holds and settings, the workers' failures, the answers lease, and the records' last
  saved `seq`.

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
it right after it starts listening, before the agent asks anything of it). Planned: lease ids
become `<epoch>-<n>` (still `<term>-<n>` today, so lease order across restarts is kept by the same
rule); every answer carries `x-pool-epoch`; a member seeing a new epoch re-sends what it keeps
(§5.3), nothing else changing for it.

### 4.4 Recovery

- **A restart** (crash, update, reboot): load the snapshot, replay the log, bump the epoch, listen.
  A torn last line (a crash mid-append) is dropped: it was never answered. Every lease comes back
  as it was, with a fresh deadline, marked *unconfirmed*; each
  member's first beat lists the leases it holds, confirming them, and those of that member it
  doesn't list are released at once (§6.2).
- **The lead's disk lost** (the mini's disk replaced, `agent/` deleted): the store is rebuilt from
  the NAS backup (`state/lead/store.json`, at most `BACKUP_EVERY` old), the epoch set past it by
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
| `POST /pool/beat` | each member, every 10 s and on a change | epoch, the settings' generation, holds for it, messages queued for it, the hand-offs merged (acks), leases released |
| `POST /pool/handoff` | a member's job end (via its agent) | taken (durable) or refused with why |
| `POST /pool/mail` | a member's `Tell`, `Cancel`, `Retry`, `HandTo` (gone with #881) | taken |
| `POST /pool/hold`, `GET/POST /pool/settings` | any Mac's agent, for the owner's CLI and menu | the new state |
| `POST /pool/budget/take`, `/pool/lease/answers` | a job's proxy, via its agent | tokens or a wait / the lease |
| `GET /pool/view`, `/pool/view/forecast-input`, `/pool/brief` | each Mac's pageview, CLI | the documents |
| `/work/*` | jobs and the build page | unchanged |

### 5.2 Timing

Each call has a 5-s deadline at the client (the lead answers from memory or a local `fsync`; 5 s
is already a lead in trouble). A failed call marks the lead unreached for that round
(`Client::new_round`, today's), and the next beat tries again. Nothing waits 20 s.

### 5.3 What a member keeps

In `agent/pool/` on its own disk, as today's outbox and `journal::Mine`:

- its **held leases** (the jobs running, already saved by the agent);
- its **hand-offs** not yet acknowledged as *merged into a saved records snapshot* (not merely
  taken: a lead disk lost between taking and saving loses nothing);
- its **asks** (mail, holds, settings) not yet answered 200.

On a new epoch, or every 10 minutes while any are unacknowledged, it re-sends them; the lead's
sequence numbers drop repeats.

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
`{epoch, released, speaks, nas_too, members}` (the leases it listed that aren't its on the lead,
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
  after `AWAY_S` (2 min without a beat): built in the coordinator's status (`/work/status`'s
  `away`: member, leases, last heard); planned on the page ("M1: 2 jobs held, not heard from since
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
  its local status and page (`/work/swarm` from its own last view, aged: "lead not answering since
  14:02"), its brokers' tasks for its running jobs (the lead isn't on a task's path, pool.md §8).
- **Budgets:** the Wikimedia budget falls back to the NAS-away rule of today, the rate divided by
  known members, held by each Mac's proxy; the answers lease isn't taken (no answers sync starts).
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
| a member can't reach the lead, both have the NAS | §7.1 for that member alone: it finishes its jobs and waits; the lead marks its leases `held_by_away` and grants their targets to no one. No NAS fallback: the mailboxes and journal are gone (§9). |
| the lead can't reach the NAS, members reach it | coordination goes on (the store is local); NAS-needing jobs of every Mac stall on the share as today; merges wait (records can't be saved: hand-offs stay taken, acks wait), catalogues and GC wait. The status says "the lead has lost the NAS". |
| a member can't reach the NAS, reaches the lead | as away (§7.2): local work only, its NAS jobs paused per today's rules. |
| two members can't reach each other, both reach the lead | nothing changes: members don't talk to each other for coordination (a member's page reads the others straight, pool.md §8: not coordination). |
| the lead hears a member but the member doesn't hear the answers (one-way) | the member's posts are taken but it sees failures: it re-sends (deduplicated) and starts nothing new; its leases stay live by its beats. Seen as "reached by the lead, not reaching it" (`Beat::reach`, today's). |

There is never a second lead to disagree with, so no split can make two grants of one target.

## 8. Migration: one piece at a time

Each step ships alone, keeps the build running (no stop, no shared flag day), and ends with its
tests. A member says what it speaks in its beat (`speaks: [...]`, as `mail_http` does today); the
lead keeps the old NAS path for a piece while any member in touch doesn't speak the new one, and
drops the old path one release after every member has for a day (the #642 gate). The tests use
the agent's two-agents-in-one-process harness (`pool_tests`) and #871's slow-NAS fake (every NAS
call blocks until released) plus a **network fake** on the client (drop, delay, lose the answer,
one-way) and a **lead kill** (the store's writer stopped mid-append, the coordinator restarted
from the same folder).

| # | Step | Deletes | Tests |
| --- | --- | --- | --- |
| 1 (done) | `pool::store` on the lead: snapshot, log, epoch, compaction, backup to the NAS; the coordinator's state moves into it. The handler rule: a scan test that no `coord` handler path reaches `nasread`, `Nas` or `store::whole` | `coord::keep`'s per-term NAS state file and its thread (`BATCH`, `GAP`, `Keep::off`) | `store_recovers_after_a_torn_append`, `store_compaction_crash_between_snapshot_and_log`, `store_restores_from_the_backup`, `coord_handlers_never_touch_the_nas` |
| 2 (done) | **Leases by the holder's word**: grants durable before the answer; beats list held leases; release on omission; `held_by_away`; `scenic pool release` | `OWED_S`; the TTL lapse re-grant for every member speaking `held-leases` (`coord::TTL`'s path stays for an older member until the gate drops it; `/work/beat` still renews and carries a job's progress, the pause and cancels) | `a_partitioned_member_s_target_is_never_regranted` (one member unheard for 3 h on a virtual clock; its job finishes; its end taken; no second grant), `an_away_member_s_leases_go_after_12_h`, `a_crash_before_the_answer_releases_the_grant`, `a_restarted_agent_s_lost_job_is_released_in_one_beat`, `mixed_versions_keep_the_old_rule`, `the_owner_releases_a_member_s_leases` |
| 3 (done) | **One beat by HTTP** (`POST /pool/beat`), the lead's answers carrying messages, holds, settings' generation and acks. Built: the beat carries the member's pool heartbeat (`agent::pool::Heartbeat`, with `beat_http`) and its agent's status (its plan's parts left out), posted by `agent::beat`'s own poster thread every `EVERY` (10 s), so a loop held on the NAS doesn't stop it; the lead keeps each member's last in memory (`crate::coord::beats`) and answers with the others' heartbeats, its own among them (`members`: the members' brokers and the lead's reached without the NAS), `speaks: ["beat"]` and `nas_too`. Every reader of a member's heartbeat (`Side::read_heartbeat`: reach, brokers, budgets, the merge's app check, mail) and of the members' statuses (the lead's `helpers`, the forecast's machines, `scenic pool status`, every Mac's page through the lead's `state/status.json` `helpers` and `beats`) reads the beats first. Members are known from their beats and forgotten after 20 min without one (`beats::FORGET_S`). **The gate:** a member writes `state/pool/members/<id>.json` and `state/helpers/<host>.json` only while its lead doesn't speak `beat` (an older lead's 404, or a step-2 lead's answer) or says `nas_too`; the lead says it, and writes its own NAS heartbeat, while a member in touch heartbeats on the NAS without `beat_http` (`beats::old_in_touch`). Planned: messages, holds and the settings' generation in the answer (step 5), acks (step 6); the members' beats in the store's snapshot (step 4); the lead's `state/status.json` stays until step 4 serves the view (the pages read it); the older apps' path (the members' listing and NAS reads, `old_in_touch`, writes under `nas_too`) deleted one release after every member speaks `beat` for a day (the #642 gate) | `state/pool/members/`, `state/helpers/` and the members' `state/status.json` writes once gated (above); the page's and `scenic pool status`'s reads of `state/helpers/`; `pool::beat`'s NAS read for a member heard by its beat; forgetting by listing alone (now: no beat for 20 min); the loop's own beat (`beat_lead`, `lead_beaten`, `held::BEAT_EVERY`) | `beats_go_on_while_the_nas_hangs` (the slow-NAS fake blocks every write of the status's NAS copy: the lead hears each of two members' 60 beats, and each hears the other), `mixed_versions_an_old_member_s_nas_heartbeat_still_read`, `the_nas_copies_stop_only_by_a_lead_that_takes_beats`, `the_lead_answers_each_member_with_the_others` |
| 4 | **The view by HTTP**: pageview fetches `/pool/view` from the lead | pageview's NAS reads (status, view, forecast-input, regions, pause, heartbeats, the 12-s reader), the lead's writes of `term/<E>/status.json`, `view.json`, `forecast-input.json`, `regions.json`, `pause.json` | `the_page_on_a_member_shows_the_lead_s_view_with_the_nas_hung`; a page asking with the lead down gets the last view with its age |
| 5 | **Mail, holds and settings by HTTP only**: lead → member messages in beat answers; `/pool/hold`, `/pool/settings`; the CLI posts to its own agent, which posts to the lead | the mailboxes (`state/pool/mail/`), `Mail::posted`'s fallback, `POST_AGAIN`, `inbox.json` (the store takes it), `state/pool/holds/`, every switch file under `state/pool/` and their readers on the Switches pool (switches whose rollout is finished are deleted outright, not moved: today `resume`, `brokers`, `placement`, `memory-guard` per pool.md §12, if each is still on everywhere) | `a_remote_pause_reaches_the_member_in_one_beat`, `settings_survive_a_lead_restart`, `an_ask_while_the_lead_is_down_is_sent_when_it_returns` |
| 6 | **Hand-offs by HTTP**: `POST /pool/handoff`; the lead merges from the store; acks only once a records snapshot naming them is saved | `state/journal/` (writes, listings, `MOVED_ON_S`, the hour's wait, `SWEEP_S`), `journal_gc` and its marker and fence (`gc.json`, `Beat::fenced`, `scenic pool journal-gc`, `ALERT_S`), `Tell`/`TELL_AGAIN` (the beat lists unacked keys), `caught_up`'s day listings | `a_hand_off_posted_twice_merges_once`, `a_lead_killed_after_taking_before_saving_loses_nothing` (the member re-posts on the new epoch), `hand_offs_merge_with_the_nas_slow` (records save waits, acks wait, nothing lost) |
| 7 | **Budgets and small leases**: Wikimedia tokens and the answers lease from the lead; resume progress in the beat | `state/pool/wiki/` and its create-new files, `state/pool/answers.lease`, `state/pool/progress/` | `the_wiki_budget_holds_across_three_macs` (rate within the class's), `the_budget_falls_back_when_the_lead_is_down` |
| 8 | **Terms out**: records at `state/build/records.json` and `brief.json` with the epoch; whatever #881 left of terms, records per term, re-assertion, the hand-over and take-over state machines and their simulator | `state/build/terms/`, `lead.json`, `state/build/term/<E>/`, `state/coord/term/`, `state/coordinator.json`, `pool::term`, `pool::handover` (if #881 kept it), `Records::newest`'s walk down the terms, "today's files for older apps" (`write_today`, `readers_ready`), the share model in `pool::sim` (replaced by step 9's) | readers find the records at the new path; an old reader finds the last per-term snapshot until the gate drops it |
| 9 | **The simulator over the network**: the lead and two to four members over the network fake (drops, delays, lost answers, one-way links, partitions of hours, lead kills mid-append, member restarts, the lead's disk restored from backup) | — | invariants at every step: no target under two live leases; every hand-off ever posted merged exactly once or refused with why; no lease id reused; acks only for saved records. 2,000 seeds |
| 10 | docs: pool.md §3, §6, §7.3, §7.5, §7.8, §9, §10, §12, §13 rewritten to this; agent-parts.md's **pool** part as the HTTP client; this doc folded in | — | — |

Order: 1 before all; 2 and 3 together or 2 first; 4, 5, 7 in any order after 3; 6 after 2; 8 after
#881 and 6; 9 alongside 6. Steps 3 and 4 are what the owner sees first (no Mac out of touch over a
stuck share, pages fast on every Mac).

## 9. What gets deleted

Once every step has landed and its gate has passed, the pool writes nothing small to the NAS. Gone:

- the files: `state/pool/members/`, `state/pool/mail/`, `state/pool/holds/`, `state/pool/progress/`,
  `state/pool/wiki/`, `state/pool/answers.lease`, `state/pool/nas-probes.json`, the switch and
  setting files under `state/pool/`, `state/journal/` (entries, `rejected/`, `gc.json`),
  `state/build/terms/`, `state/build/lead.json`, `state/build/term/`, `state/coord/term/`,
  `state/coordinator.json`, `state/status.json`, `state/helpers/`, `state/build/pause.json`,
  `state/coord/history/` (the day tars stay);
- the code: `pool::term`, `pool::handover`, `pool::journal`'s NAS side, `pool::journal_gc`,
  `pool::beat`'s NAS side, the mailboxes and `coord::inbox`'s file, `coord::keep`, the pageview's
  NAS reader, `write_today`/`readers_ready`, `MOVED_ON_S`, the lease TTL's re-grant, the members'
  listing and forgetting by listing, the share model of `pool::sim`;
- in `agent::nasread`: the pool's and switches' traffic (the Switches pool entirely; the Records
  pool keeps only the jobs' and the plan's reads, the Writes pool only the records' saves and the
  backup), which is what agent-parts.md §6 foresaw;
- the asides and stale-read rules stay for the records and bulk files (pool.md §3), but no longer
  for heartbeats, statuses or mail.

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
4. **`BACKUP_EVERY`:** 10 min proposed; it bounds only the owner's settings lost with the lead's
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
