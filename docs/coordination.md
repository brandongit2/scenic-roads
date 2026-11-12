# Pool coordination by HTTP on the lead

The lead (the Mac mini, docs/pool.md §6.1) holds the pool's coordination state in one store on its
own disk and serves it over HTTP on its coordinator's port; the members talk to it alone, and
nothing in coordination goes through the NAS. The NAS keeps the bulk data and the records'
snapshots. docs/pool.md describes the pool end to end; this doc is the design of the
lead's store and protocol (§4, §5), the leases' rule (§6), the failure modes (§7) and the decisions
behind them (§10). docs/agent-parts.md's **pool** and **beat** parts are its client side. Built by
task #878 (§8 indexes its steps, which the code's comments cite).

**The rule for old-app paths** (the owner's, 10 Nov): an old app's path for a piece may be deleted
once every Mac runs an app that has the new path, as their beats show (each Mac's app, and the
words it says). No day's wait, no extra release. A rollback to an app from before that point loses
the old path: such an app finds the old path gone and works only as far as the new path it has.

## 1. Why

Coordination used to be small files on the SMB share (heartbeats, mail, the journal, terms,
statuses and views, holds, switches, the Wikimedia budget's tokens, progress files), and SMB is at
its worst exactly there: small creates run at about 9–55 a second for the whole share
(docs/nas-access.md), a mount stall holds a call in the kernel for minutes (pool.md §7.8), a held
file can't be removed for hours (pool.md §3), listings read stale for up to 33 s and one name for
40 minutes, and an answer that waited on the share often missed its 20 s. The share was used as a
message bus and a lock service, which it is not. Bulk data stays on it; coordination is one process
on one disk.

## 2. Principles

1. **The lead holds the pool's state** on its own disk, in one store (§4), written by one thread.
   No other Mac writes coordination state anywhere.
2. **Members talk to the lead by HTTP** on its coordinator's port (8090, over the tailnet), and
   only to the lead. Nothing in coordination goes member to member, and nothing through the NAS.
3. **The lead's HTTP handlers never touch the NAS.** They read and write the store and memory
   only; an answer never waits on the share (test `coord_handlers_never_touch_the_nas`).
4. **Every post is idempotent by its own content:** a lost answer is retried, never doubled, with
   no sequence numbers. A beat says whole state (its leases, heartbeat, status, hand-offs' keys,
   progress), and one sent before a request the lead took from that member since (a beat or an
   ask, ordered by `held::Sent`: its process's start and its number there) releases nothing; a
   hand-off is taken once by its key; a hold or setting by its `at` (the later wins, a repeat
   changes nothing); mail by its numbers; the answers' lease by its holder's token. A budget take
   repeated after a lost answer gives a second token, the first unspent: the budget errs below its
   rate.
5. **Durable before the answer:** a post that changes state (a grant, a hand-off, a hold, a
   setting) is in the store's log, flushed (`fsync`), before its 200.
6. **The member keeps its own until it's safe on the lead:** its hand-offs, asks and held leases
   live on its disk until the lead says they are merged or taken (§5.3). The members are the
   second copy of what matters.
7. **Bulk data stays on the NAS:** built files, sources, caches, records snapshots (one writer,
   big whole files: what SMB does well).

## 3. Where each piece lives

### 3.1 By HTTP, on the lead

| Piece | By HTTP | On the lead | On the NAS |
| --- | --- | --- | --- |
| A member's heartbeat, its agent's status, the leases it holds, its hand-offs' keys, its capacity and resume progress | `POST /pool/beat` every 10 s and on a change, from **beat**'s poster thread (agent-parts.md §4.2) | each member's last in memory (`crate::coord::beats`), forgotten after 20 min unheard (`beats::FORGET_S`); in the store its heartbeat and when heard (`beat/<member>`, saved each minute at most), its words (`member/<host>`), its progress (`progress/<member>`) | none |
| Who leads | the beat's answer (`leading`: its member, host and app) | — | the declaration, `state/build/lead-member.json`, read by a member only while no answer came within `PAGE_AWAY_S` (35 s), by the lead every step (pool.md §6.1) |
| The epoch | every answer's `x-pool-epoch`, `x-pool-member` | the store's (§4.3) | none |
| Jobs' leases | `/work/ask`, `/work/beat`, `/work/done`, `/work/fail`, `/work/cancel`, `/work/in/`, `/work/out/` (coordinator.md); `POST /pool/release` (the owner's) | `leases`, `lease/<id>` | none |
| Hand-offs | `POST /pool/handoff` for each the beat's answer says the lead lacks | `handoff/<key>` until a saved records snapshot names it | none |
| Mail to the lead (a page's `Cancel`, `Retry`) | `POST /pool/mail` | `inbox/<member>` | none; the lead sends none |
| Holds (another Mac's pause) | `POST /pool/hold`; the held member's in its beat's answer (`hold`) | `hold/<member>`, the later `at` winning | none |
| Pool settings | `POST /pool/settings`; their generation in each beat's answer (`settings_gen`) | `settings`, each with `by` and `at` | none |
| Resume progress released | `POST /pool/progress/release`, carried back in the beat's answer | `progress/<member>` | none |
| The Wikimedia budget, the answers' lease | `POST /pool/budget/take`, `/pool/budget/hold`, `/pool/lease/answers` from each agent's proxy | windows and tokens given in memory; `budget-hold`, `answers-lease` | none |
| The build page's view | `POST /pool/view` | in memory: the coordinator's view, its agent's status (its `status.json`, which other Macs' `scenic status`, `scenic pool status` and map servers ask it for: `coordwire::lead::status`), the forecast's input, the regions | none |
| The records' brief | `GET /pool/brief` (`ETag` its (epoch, seq)) | the last save's, gzipped, in memory | `state/build/brief.json`, written each save for the map server and the CLI |
| The records | — | `agent/pool/records.json`, the lead's disk copy | `state/build/records.json`, written whole each save (pool.md §6.2) |
| GC's queue and cursors | `/gc/chunk`, `/gc/gone`, `/gc/orphans`, `/gc/removing` (docs/maplog.md §7) | the lead's GC queue | — |
| The history | `/api/history` (the page's; the CLI's on another Mac) | the coordinator's `history.jsonl` (the last week's), which the page's view reads (pool.md §7.5) | each finished day packed into the archive by the daily `gc` job, `state/coord/history-archive/<day>.tar` |
| How to reach the lead | the beat's answer while it leads (`reach`: its addresses and page) | each Mac's agent keeps its lead's contact in its folder (`lead.json`, `coordwire::lead`) | read for it by no app since task #1119: with no `lead.json` (a new Mac) or one naming another member than the declaration, or a root that doesn't answer, the declared lead's host is reached by its tailnet address (as `tailscale status` lists that host) and its LAN name, with the pool's token (`state/coord/token`); a root that answers and declares no lead, none. The lead still writes `state/coordinator.json` each 5 min while it leads (`coordwire::OLD_CONTACT`, when it differs), for the apps before #1119 alone, whose agents, map servers and updaters find the lead by it (§9) |

### 3.2 On the NAS

On the NAS, as bulk, archive or rebuild data (nothing the lead alone reads: its contact, its status,
its store, the NAS budget's probes and the history are its own, §3.1, task #1119):

- **History archives** `state/coord/history-archive/<day>.tar`, one big file a day: the lead's
  history's finished days (its member's file in each), packed by the daily `gc` job from its
  coordinator's `history.jsonl` (`--history`, `agent::histarchive`). A lead whose disk is lost
  loses the history since the last run (today's, at most a day and a half's); a lead moved by hand
  takes it with its `coord/` (pool.md §6.1).
- **The token** `state/coord/token`: read at every agent's start (`agent::pool::seed`), and made
  there, create-new, only by an agent finding none (the first one's stays).
- **Read-only inputs** (`inputs/`, `inputs/keys.env`).

## 4. The lead's store

### 4.1 Shape

`crate::pool::store`, in `agent/coord/store/` on the lead's disk, its files `store::seglog`'s
(shared with the map's log, docs/maplog.md §4.2):

- `snapshot.json`: the whole state as of a log position, written whole (temporary file, `fsync`,
  rename, `fsync` of the folder).
- the log, segments `<first seq>.jsonl`: one line per change after that position, appended and
  `fsync`ed in batches (every change waiting when the writer wakes: one `fsync` for a burst of
  grants).
- The state: keyed JSON documents (`crate::coord::kept` names them) and the epoch: the jobs'
  leases (`leases`: the next id, the holds and cancels; `lease/<id>`), the coordinator's measures
  (`cost/`, `floor/`, `disk/`, `job_disks`, `fills`), the pause, the members' words and last beats,
  their mail, holds and progress, the settings, the hand-offs taken and not yet in a saved
  snapshot, the budget's hold and the answers' lease. Not the records: they are the lead's
  driver's, kept beside its saved state (`agent/pool/records.json`, ~15 MB written whole each
  save), and `Saved::records` says which it saved last.
- A coordinator finding no store fills its first from the files of an agent before the store
  (`leases.json`, `costs.json`, …): a store found is the truth.

One writer thread owns the files; a handler submits a change (applied in memory at once) and waits
for it to be durable, under `store::DURABLE_WAIT` (2 s), past which the post answers 503 (a lead
disk that slow is broken). Every JSON post of the coordinator waits so (`http::json`).

### 4.2 Compaction

Past `COMPACT_BYTES` (10 MB) of log or `COMPACT_AGE` (an hour), the writer writes a new snapshot at
its position and starts a new segment; the old segments are removed only after the new snapshot's
rename and folder `fsync`. A crash between leaves a snapshot and a log that overlaps it: replay
skips positions at or below the snapshot's. A failed append is followed by a compaction too (the
snapshot from memory), so a change in memory is never left out of the files.

### 4.3 The epoch

A counter in the store, bumped and `fsync`ed as the store opens (right after the coordinator
starts listening, before the agent asks anything of it): it is what the lead leads
(`pool::driver::Io::epoch`, `Coordinator::store_epoch`). Its records were saved in an epoch of
this store, so it is past theirs; a store that isn't (not the one that saved them) leads nothing.
So the records' (epoch, seq) and job ids `<epoch>-<n>` (the leases' runs, pool.md §7.5) order
across restarts, and across a hand move, which copies the store (pool.md §6.1). Every answer says `x-pool-epoch` (0 from a member's
broker) and `x-pool-member`. Records and lease JSON keep the key `term` for the epoch's value.

### 4.4 Recovery

- **A restart** (crash, update, reboot): load the snapshot, replay the log, bump the epoch,
  listen. A torn last line (a crash mid-append) is dropped: it was never answered. Every lease
  comes back with a fresh deadline, unconfirmed; each member's first beat lists the leases it
  holds, confirming them, and those of that member it doesn't list are released (§6.1).
- **The lead moved by hand** (pool.md §6.1): the new Mac's coordinator opens the copied store as
  a restart does, its own host's leases of before dropped, the old lead's slots' leases held by
  that Mac's word once it beats as a member.
- **The lead's disk lost** (its store and its records): not handled. A coordinator finding no store
  makes a new one, and its driver, its records gone, leads nothing, saying so (pool.md §4,
  invariant 5); the owner decides.

## 5. The protocol

### 5.1 Calls

All on the lead's coordinator, with the build's key, sent only to tailnet addresses (`mail_urls`'
rule):

| Call | From | Answer |
| --- | --- | --- |
| `POST /pool/beat` | each member's **beat** poster, every 10 s and on a change | the epoch, the leases it listed that aren't its (`released`), the words the lead speaks, the others' heartbeats (`members`), its hold, the settings' generation, the hand-offs merged (`merged`) and lacking (`lacks`), progress released, its running jobs' threads re-decided (`threads`), the lead's line for the menu bar (`lead`: docs/roles.md §3.4), who leads while it leads (`leading`), and what it has of the jobs' spans the beat carried (`spans`: docs/formats.md, Job spans) |
| `POST /pool/handoff` | the beat poster, for each hand-off the answer says the lead lacks | taken (durable) |
| `POST /pool/mail` | a member's agent: a page's `Cancel` or `Retry` | `took` |
| `POST /pool/hold`, `/pool/settings` | any Mac's agent, for the owner's CLI, menu and page | the new state (the settings: a post naming none reads them) |
| `POST /pool/release` | `scenic pool release <member>` | the leases released |
| `POST /pool/progress/release` | `scenic pool release <member> <job>` | what it released, or 400 when it keeps none of that member's |
| `POST /pool/budget/take`, `/pool/budget/hold`, `/pool/lease/answers` | an agent's proxy, for its jobs (the lead's own in process) | a token or a wait / the hold's end / the lease's word |
| `POST /pool/view` | the lead's page view (in process), and by HTTP | the view's documents, each only when its time differs from the ask's |
| `GET /pool/brief` | a member's agent: its pool's view, a job as it starts, `--expect-same` | the brief from memory, gzipped; 304 to `If-None-Match` naming its tag; 503 while it has none |
| `/work/*`, `/task/*`, `/gc/*` | jobs, tasks, map servers and GC | coordinator.md, docs/maplog.md §7 |

A coordinator without its store (not the lead's, or not open yet) answers the asks 503, asked
again.

### 5.2 Timing

Each call has 5 s to connect and 15 s in all at the client (`coordwire::client`; a file's
transfer, `/work/in/`, `/work/out/`, has no limit on its whole time, only 60 s with no byte
moving). Once a call gets no answer, the later calls of that round (an agent's loop:
`Client::new_round`) fail at once without trying, until its next round; the beat's poster tries
again at its next beat.

Connections are kept alive (task #958): each process has one HTTP client, every `Client` sharing
its pool, at most 4 idle to an address and 64 in all, each let go after 10 s idle; the coordinator
closes one idle for 20 s (`HEADERS_MAX`), so the client always lets go first. A kept connection
gone stale:

- **closed by the coordinator** (restarted, or closing it as a request came): ureq finds it closed
  before taking it, or the request fails before any answer, and goes again at once on a new
  connection (hyper closes an idle connection only between requests, so the coordinator never
  read it);
- **its Mac silent** (asleep, off the network): after 2 s with no answer the client connects anew
  (5 s); taking no connection, the request fails as a new connection's would; taking one, the lead
  is there and slow, and the request waits out its time.

A connection takes no new request after 3 h (`CONNECTION_MAX`).

### 5.3 What a member keeps

In `agent/pool/` on its own disk:

- its **held leases** (the jobs running, saved by the agent);
- its **hand-offs** until acknowledged as merged into a saved records snapshot (not merely taken:
  a lead disk lost between taking and saving loses nothing). Its driver's saved state keeps them
  whole (`journal::Mine`); every beat lists their keys (`handoffs`), from its saved state at its
  start, whatever its lead last answered (the lead's GC queue takes the list as every lease it may
  still hand off under); the answer says which a saved snapshot names (`merged`: dropped) and
  which the lead lacks (`lacks`: posted next). So a hand-off is posted the beat after its job ends,
  and again whenever a lead lacks it (a restart that forgot it, a lost store), with no state of
  what was posted;
- its **asks** (mail, holds, settings: `asks.json`, `setting-asks/`), each sent until answered 200;
  one the lead refuses as it is (400) leaves the queue and is said on the asking Mac
  (`asks-answered.json`);
- its **own hold and settings**, told again to a lead that lacks them or has them older (a moved
  lead, a lost store: `Beats::set_settings`, seeded from `pool-settings.json`).

A repeat changes nothing (principle 4). A hand-off is lost only to a double fault: its member's
saved state lost after its job ended and before a saved snapshot named it, while no store of the
declared lead's holds it (the simulator excuses that alone, pool.md §13).

## 6. Leases: a job is never run twice

### 6.1 The rule

A target is granted only by the lead, only while no lease on it is live, and a lease ends only by
its holder's word (the job ended, gave back, or the holder's beat doesn't list it) or by the
owner's. **The clock alone never re-grants a target** while its holder may be running it: the lead
can tell "holder gone" from "holder unreachable" only by hearing from it, so it waits to hear
(`crate::coord::held`).

- **The beat** (`POST /pool/beat {member, speaks, held, held_age_s, heartbeat, status, handoffs,
  cap, progress, sent, spans}`, every `agent::beat::EVERY`, 10 s, from its own poster thread, whatever the
  loop waits on): `held` is the list the loop made last, `held_age_s` its age, so the lead releases
  only leases granted before the list was made (a loop stuck after a grant never loses it).
- **The grant is durable before its answer.** A lead crashing after the `fsync` and before the
  answer leaves a lease its member never got: the member's first beat doesn't list it, so it is
  released.
- **Renewals are beats.** Any lease of that member a beat doesn't list is released at once (a
  crashed job, a restarted agent that lost it), but one granted within `held::OMIT_GRACE` (30 s: its
  answer may still be on its way) and not loaded since, and none by a beat sent before another
  request the lead took from it (`Sent`). The beat lists the member's slots' leases and those whose
  ends it still owes the lead (told again each loop until answered).
- **A member the lead stops hearing from** keeps its leases: held while away after `held::AWAY_S`
  (2 min without a beat, `Coordinator::away`), released after `held::AWAY_RELEASE_S` (12 h unheard,
  decision 1). *Planned:* saying it on the page, the status and the menu bar ("M1: 2 jobs held,
  not heard from since 14:02"; task #1158); no view shows it today.
- **Releasing them** is otherwise the owner's call: `scenic pool release <member>` (`POST
  /pool/release`) when that Mac is known gone for good. A hand-off from a released lease is
  refused if a newer lease set its targets (lease order), so nothing is merged twice.
- **A member returning** beats with what it holds; those leases go on, or, released meanwhile, the
  answer says so (`released`) and the member stops those jobs at once.
- **Gated by what the member speaks** (`held::SPEAKS`, `held-leases`, kept in the store as
  `member/<host>`, so a restart keeps it): an app that doesn't say it has its leases lapse after
  `coord::TTL` (10 min) unrenewed. Every Mac says it. The lead's own jobs' leases are its slots'
  (`Leases::mine`, pool.md §7.6).

### 6.2 What it costs

A member that dies for good holds its targets until the owner releases them or 12 h pass: the
price of never running twice while it may still be running. A repeat after the 12 h costs doubled
work only, since the merge refuses the old lease's result.

### 6.3 The lead's own jobs

Its slots ask its own coordinator in process (pool.md §7.6); their leases are in the same store,
live while its agent says its slots run them (`Coordinator::keep_own`). A lead's restart ends
them as its store opens (they ended with the agent before); the jobs the new agent takes over
(`adopt_jobs`) are held again, under a new id where the old one ended.

## 7. Failure modes

### 7.1 The lead is down

There is no takeover (#881):

- **Members** finish their running jobs; each ending's hand-off stays in their saved state. They
  start nothing new: no grants. Their beats fail and are retried every 10 s.
- **What still works on a member:** its own hold and quit (**control** is local), its local status,
  its brokers' tasks for its running jobs (the lead isn't on a task's path, pool.md §8).
- **Budgets:** no Wikimedia request goes out (each Mac's proxy asks the lead again each window):
  the lead is the budget's one source, since a share taken by a Mac that can't reach it would add
  to what it gives the Macs that can. A 429 holds that Mac alone. The answers' lease isn't taken
  (a job waits for it, saying so).
- **The lead back:** a new epoch; members' beats confirm their leases and re-post the hand-offs it
  lacks; the lead merges, saves, acknowledges; grants resume. Nothing is lost or run twice.
- **Seen as:** every Mac's menu and page: the lead not answering, since when; no member promotes
  itself.

### 7.2 A member away over Tailscale

Coordination doesn't need the NAS, so a member away from home but on the tailnet beats, asks and
is held exactly as at home. Whether it gets work is placement's call: it lacks the NAS (or the NAS
over Tailscale is too slow), so only `Needs { nas: false }` work fits it. A member away with no
tailnet is §7.3's first case.

### 7.3 A split network

| Split | What happens |
| --- | --- |
| a member can't reach the lead, both have the NAS | §7.1 for that member alone: it finishes its jobs and waits, keeping their hand-offs; the lead holds its leases as away and grants their targets to no one. There is no NAS fallback. |
| the lead can't reach the NAS, members reach it | coordination goes on (the store is local); NAS-needing jobs stall on the share; merges wait (records can't be saved: hand-offs stay taken, acknowledgements wait), catalogues and GC wait. |
| a member can't reach the NAS, reaches the lead | as away (§7.2): local work only. |
| two members can't reach each other, both reach the lead | nothing changes: members don't talk to each other for coordination. |
| the lead hears a member but the member doesn't hear the answers (one-way) | the member's posts are taken but it sees failures: it re-sends (idempotent) and starts nothing new; its leases stay live by its beats. Seen as reached by the lead, not reaching it (`Beat::reach`). |

There is never a second lead to disagree with: only the declared lead grants (pool.md §4,
invariant 1), and a move of the lead stops the old one's agent first (pool.md §6.1).

## 8. The steps of #878

Each step shipped alone, the build running throughout; each piece's old NAS path stayed while a
Mac didn't say the new piece's word in its beat, and was deleted by the rule for old-app paths
(tasks #1036, #1125, #1124). The code's comments cite these numbers.

| # | What it built | Tests (the main ones) |
| --- | --- | --- |
| 1 | the store (§4), the coordinator's state in it; the handlers' rule | `store_recovers_after_a_torn_append`, `store_compaction_crash_between_snapshot_and_log`, `coord_handlers_never_touch_the_nas` |
| 2 | leases by the holder's word (§6) | `a_partitioned_member_s_target_is_never_regranted`, `an_away_member_s_leases_go_after_12_h`, `a_crash_before_the_answer_releases_the_grant`, `the_owner_releases_a_member_s_leases` |
| 3 | one beat by HTTP: heartbeat and status, the others' heartbeats in the answer | `beats_reach_the_lead_and_each_member_hears_the_other`, `the_lead_answers_each_member_with_the_others` |
| 4 | the build page's view by HTTP (`/pool/view`) | `the_page_shows_the_lead_s_view_while_the_history_waits`, `a_page_asking_with_the_lead_down_gets_the_last_view_with_its_age` |
| 5 | mail, holds and settings by HTTP (`crate::coord::asks`, `coord::inbox`) | `a_remote_pause_reaches_the_member_in_one_beat`, `settings_survive_a_lead_restart`, `an_ask_while_the_lead_is_down_is_sent_when_it_returns`, `a_post_answered_survives_the_leads_crash` |
| 6 | hand-offs by HTTP (`crate::coord::handoffs`), acknowledged once a saved snapshot names them | `a_hand_off_posted_twice_merges_once`, `a_lead_killed_after_taking_before_saving_loses_nothing`, `hand_offs_merge_with_the_nas_slow`, `a_hand_off_is_in_the_store_until_merged_and_answered_so` |
| 7 | the Wikimedia budget and the answers' lease from the lead, resume progress in the beat (`crate::coord::budgets`) | `the_wiki_budget_holds_across_three_macs`, `the_answers_lease_is_one_holders`, `the_budget_waits_for_the_lead_while_it_is_down` |
| 8a | a member learns its lead from the beat's answer, not the NAS | `a_member_learns_its_lead_from_its_answer_with_no_nas_call`, `a_member_takes_its_lead_from_a_fresh_answer_alone` |
| 8b | the lead's records on its own disk, then one path on the NAS | `a_lead_whose_take_up_s_nas_save_fails_leads_on`, `a_late_save_over_the_one_path_is_saved_over` |
| 8c | no terms: the lead leads its store's epoch (§4.3); `publish.sh --rollback` refuses to go back past it (`tools/app/rollback-floor`, `878-8c`) | `a_reader_reads_the_one_path_alone`, `a_save_writes_the_one_path_and_its_brief_alone`, `a_restarted_lead_takes_up_from_its_own_disk_in_a_new_epoch`, `a_lead_moved_by_hand_leads_from_its_copied_folders` |
| 8d | the records' brief by HTTP (`GET /pool/brief`, crate::agent::brief): agents and their jobs read it from the lead | `the_brief_is_answered_by_its_tag`, `a_member_has_the_leads_brief_by_its_tag`, `a_members_job_has_the_leads_brief_or_waits_for_the_lead` |
| 9 | the simulator over the network (pool.md §13) | its invariants at every step, over 2,000 seeds a sweep |
| 10 | these docs | — |

## 9. The NAS's leftover files

No app writes these, and none reads them but as said below; they are the owner's to remove, as one job on the NAS over
SSH (`rm -r`; SMB removes file by file, and a held file's aside can't be removed for hours), never
the agent's loop. Measured 12 Nov, read only (the terms' other files, `state/coord/term/` and the
old coordination folders are gone):

- **keep**: `state/build/term/<E>/forecast-input.json`, E 232 to 673 (352 folders, ~283 MB), which
  `agent::fccheck::inputs` reads for the backtest where the forecast inputs' archive
  (`state/coord/forecast-inputs/`, from 4 Nov) lacks them, until the archive covers them or the
  check no longer reads term folders;
- **remove** (task #1119: what the lead alone wrote, now on its disk or asked of it), once every
  Mac runs an app since #1119 (#642's rule: each Mac's updater's `app/releases/live.json` written by
  such an app a day ago; an older app reads them): `state/status.json` and its
  `status.json.<host>.<pid>.<n>.tmp` asides (an older `scenic status` and map server show the lead's
  status by it), `state/pool/nas-probes.json`, `state/tmp/` (empty), `state/logs/` (logs of 2 and
  6 Oct), and `settings/` (the map's profiles, which an older map server reads and writes; and once
  a daily backup lists `mac/profiles.json`); `map/` once every Mac's beat says `nas-check-inputs`
  (docs/maplog.md §4.4);
- **remove** once the lead runs an app since task #1183 (which writes it no more; an older lead reads it at a take-up
  only to pass over its own, and a member never): `state/lead/store.json`, the lead's store's old
  backup, and the `state/lead/` folder;
- **still written, for older apps**: `state/coordinator.json`, the lead's contact, which an app
  before #1119 finds the lead by (its agent, its map server, and its updater, which so reaches the
  new app: an older lead's agent took the file off as it stopped for its own update). At #642's rule
  above, a change of its own deletes `coordwire::OLD_CONTACT`, `keep_old_contact` and its call in
  the agent's pool step, then the file.

`state/coord/history/<day>/` goes by itself: the `gc` job packs each day into the archive and
removes it as it passes the week.

## 10. Decisions

PM, 30 Oct, with what was built:

1. **An away member's leases are released after 12 h unheard** (`AWAY_RELEASE_S`), not only by the
   owner: a repeat costs doubled work only, since the merge refuses the old lease's result, and a
   lease no one releases would stall the build while the owner is away.
2. **The records' brief is served over HTTP to agents too** (step 8d); the map server and the CLI
   keep the NAS's copy.
3. **The store is a JSON log plus a JSON snapshot:** no new dependency, readable by hand, a few MB
   (against SQLite: a new dependency, fewer hand-written recovery paths).
4. **History: members' events in the beat, to one file on the lead.** Not built: no member's events reach
   the lead (its broker's stay on its Mac; task #1160). The lead's own events are its coordinator's
   file on its disk (`history.jsonl`), which the page's view reads every 12 s (`pageview::EVERY`,
   no NAS read since task #1119); each finished day is packed into the NAS's archive by GC.
5. **A switch was deleted only once it read on for every member;** one still off anywhere became a
   setting (`/pool/settings`).
