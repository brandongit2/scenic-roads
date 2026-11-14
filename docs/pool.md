# The pool: the Mac mini leads the build

Every Mac runs the same agent (`scenic agent`). One of them, the **lead**, plans, grants jobs,
merges results into the build's records and updates the map; the others are **members**. The lead is
fixed: the Mac mini, the member `state/build/lead-member.json` declares (§6.1, task #881). No other
Mac ever leads: there is no election, handover or takeover. With the mini gone, the members finish
the jobs they hold, keep their hand-offs, and wait for it. The Macs' agents are the only
workers (§8, coordinator.md): the build page shows the build and takes the owner's asks. The pool's code is `crate::pool` (the declaration, records, hand-offs, the
driver) and `crate::agent::poolside` (the agent's side of it). It is always on.

The records' readers read the one path's snapshot, `state/build/records.json`, which the lead alone
writes (§6.2). The lead leads the epoch of its coordinator's store, which numbers its records'
snapshots and its leases. Coordination goes by HTTP to the lead, never through the NAS: each
member's beat, its hand-offs, mail, holds, settings, budgets and the records' brief
(docs/coordination.md: the lead's store, the calls, the leases' rule, the failure modes; §9 here).

## 2. Principles

1. **One agent, every Mac.** A Mac is known by a member id made once; its host name is a label.
2. **Durable state on the NAS;** a Mac's disk holds caches and work in progress.
3. **Nothing on the NAS is written in place by two Macs.** A file has one writer (a member's own
   files, the lead's), or is made once with create-new (the declaration in a new pool, refusal
   notes). A late write of a lead's process before, at the one path, is passed over by the next
   take-up (it starts from the lead's own disk's copy) and saved over within `RESAVE_S` (§4).
4. **Every job hands its results off** to the lead (§7.3); only the lead's merge writes the records.
5. **Work goes where it fits,** by measured memory and disk (§7.7). Map updates and GC are the
   lead's duties, not jobs.
6. **One lead, declared:** no Mac decides by itself to lead; a restart of the lead is a take-up
   of its own records in a new epoch, not a restart of the build: nothing is lost, no job stops.
7. **Tasks belong to the member whose job offers them** (§8).
8. **Any number of Macs and pages.**

Where state lives:

| State | Truth | On a Mac | The lead after a restart |
| --- | --- | --- | --- |
| Records, built files | NAS: `state/build/`, content-named files | caches (`agent/cache/…`); the lead's own copy of the records, `agent/pool/records.json` | takes up its own copy (§6.2) |
| Downloads | NAS: `sources/` | cache copies | nothing |
| A pass's Wikidata/Wikipedia answers | NAS: `sources/items/<date>/` (`agentcore::answers`) | `agent/cache/items/`, `agent/cache/heritage-<date>/` | its first run takes them |
| Z6 tiles' kept state | NAS: `cache/scenic-units/` | none | nothing |
| A job's progress (the OSM pass's stages, a depth, terrain-hi or terrain-deep target's blocks, scratch) | the Mac running it | scratch folders | the OSM pass's resumed there (§7.2, Resuming), a depth, terrain-hi or terrain-deep target's when it runs there again; the rest redone |
| The coordinator's state | the lead's disk: `agent/coord/store/` | | opens it (§6.2, §7.5) |
| The coordinator's token; its history's archive; its contact | NAS: `state/coord/token`, `state/coord/history-archive/`; each Mac's agent's `lead.json` (coordwire::lead) | | uses them |
| The lead's own (its map updates' record, daily jobs' last runs; the map and its log, docs/formats.md) | the lead's disk | `agent/publishing.json`, `agent/state.json`, `agent/maplog/` | keeps it |

## 3. What the NAS gives us

The share is SMB 3 on a Synology. The design assumes:

- **Create-new is atomic**, between two Macs too: 350 trials of both Macs creating one name at
  once (O_CREAT|O_EXCL, within ±30 ms) each had exactly one winner, its bytes the file's (task #645,
  `tools/check/nas-probe.py`). The declaration in a new pool, and refusal notes, rely on it.
- **A rename can be delayed indefinitely** (a Mac sleeping between write and rename).
- **Reads can be stale** (macOS's SMB caches). Measured between two Macs on an idle share (task
  #645, 570 trials): a file renamed over was read new within 1.1 s (median 0.02 s), a new file
  opened within 0.12 s; a stat or a folder listing still showed a removed file for up to 33.5 s
  (median 2.4 s; about one in five near 30 s) and a listing missed a new one for up to 3.1 s; one
  name stayed stale for 40 minutes (task #630). The pool's protocol takes up no record from the
  NAS, so no read's staleness can lose what it acknowledged.
- **Renaming over a file another SMB session has open fails** (EBUSY) for as long as it's held:
  `store::whole::rename_over` retries for 5 s. The server can hold a file for hours with no process
  on either Mac having it open;
  renaming the held file away, and renaming into its free name, still work, but removing it fails
  until it's let go (a Mac killed with a file open: kill -9, Migration Assistant). So every
  replaced file (`store::whole::write`/`copy`, `Nas::write_whole`: the manifest, pending, sizes, records) falls back to `store::whole::rename_or_aside`: the old file renamed to
  `<name>.<host>.<pid>.<n>.aside.tmp`, the new one renamed into place, then asides removed: each
  fallback removes up to 4 (the oldest first), finds any Mac's or process's asides of that file by
  listing its folder at most once a minute, and remembers at most 16 not yet removable. Listings
  skip asides as temporary. When the second rename fails the aside stays (renaming it back could
  replace a newer file), so the file is missing until the next write. Every read of the NAS
  (`store::whole::read`/`retried`: `Nas::read`/`read_fresh`) tries a busy read or open again for
  5 s, and never lists. The files that must never read as missing, all replaced by
  `whole::write` and never removed on purpose (`state/build/manifest.json`, `pending.json`,
  `sizes.json`, `summaries.json`, the extents), are read with
  `whole::read_replaced` (`out::read_record`): missing with an aside beside it, it is tried again
  for 2 s, then the newest aside (by modification time) is read, the last version written whole,
  so a manifest is never taken as empty. An aside put aside (its status change time) more than a
  day ago is never read, so a stale aside can't bring back a file. Sweeps: each fallback removes
  asides as above; every `whole::write` also removes its folder's day-old asides and the day-old temporary files
  writes left there (`<name>.<host>.<pid>.tmp`: a process ended, or a call given up on, before its
  rename), a listing at most hourly a folder a process, so a rarely written file's go too; and a
  kept scenic-units
  folder that can't be removed (a held file) is renamed aside (`scache::drop_dir`), the unit's
  earlier folder asides removed at its next save. The fallbacks log
  `whole: rename over <path> busy: …`, `whole: <path> missing, read from its aside …` and
  `scache: <dir> not removed …`. Calls still busy past the retries (a read busy for 5 s, a rename aside or into
  place busy) are noted with their file, the agent's own and its Rust jobs' (`whole::stuck`,
  `whole::STUCK_ENV`); three within 10 minutes drain this Mac's new work and then remount the
  share (`crate::agent::heal`, docs/macs.md), this Mac's stale sessions on the NAS dropped through
  DSM with it (`crate::agent::dsm`, which also drops them at the agent's start, on leaving home and
  once the share is on the LAN name again after Tailscale:
  docs/macs.md). A build job's read that the share fails for a
  moment (`store::retry`, task #656; since #663 every read, stat and listing through
  store::nasfs, docs/nas-access.md §2, but the agent's loop's, the pool's and the coordinator's,
  which fail at once, and the server's, 5 s: EIO, ENXIO "Device not configured", ETIMEDOUT and the other
  disconnects, EAGAIN, ESTALE; a "not found" only on a file known to be there: one open, one the
  store's listing has, a source the release lists) is tried again after 1, 2, 4, 8, 16 s, then
  30 s six times (211 s in all), each try logged with this Mac's time to the second and its UTC
  offset (`retry::log`, to match against the NAS's log); a process's retries run one at a time. Before
  the last try the share's mount is looked at, and a share gone or not answering is waited for a
  minute (the agent remounts it; a job never does). Past that the job fails with the original
  error. Covered: plain-file reads (`store::range::PlainFile`, reopened by path each try: the
  GLO-30 store's TIFF blocks, depth grids, building work files), GLO-30 store tiles' lookup and
  check (`dem::generic::stored`: a tile the store's listing lacks fails at once, one cut short
  too), terrain and slope packs' opens and spans (`terrain_pack::nas_call`), source grids' opens
  (`depth::grids::open_retrying`), slope mids, and bldprep's parquet footers and row groups.
  A file a job keeps open for its life is a `PlainFile` too: the packs `ManifestTiles` reads, the
  raw tiles' archives read by range (`RawTiles`, `rawpack::entries_of`), the stored pyramids'
  archives, the lake depth's hi packs. A dropped SMB session leaves its handles dead (ENXIO, then
  EIO on every read; `retry::handle_gone`, EBADF too), so a read meeting one opens the file again
  by its path and reads that, each try, and the new handle is kept (task #1090: a slope job's
  every read of one neighbour pack failed for 3 h 50 min on a handle cached dead). A file that
  can't be opened and read again still fails past the bound. A NAS call that is never answered
  (no error, the call held in the kernel) fails instead of holding its job for good: every NAS
  call of a job is a store::deadline call (docs/nas-access.md §6), made on a helper thread its
  calling thread keeps and waited for its deadline at most, counting only the time the job ran (a
  look at it the job was stopped in, the guards' freeze, counts as asked at most; any other as long
  as it took).
  A `PlainFile`'s read is waited for `deadline::LONG` (120 s) and a second more a MB it reads, its
  opens (its first, and again by its path) for `LONG`; past it the read fails with `TimedOut`
  naming the file, not tried again, the NAS's I/O limit halves (store::aimd), its descriptor is let
  go (the next read opens the file again) and the call is left to the kernel with the descriptor
  and the read's buffer (task #1139: on 11 Nov two unit jobs, on the M4 and the mini, sat in
  `pread` on a terrain deep pack at "staging layers from the packs" for 40 minutes and more at no
  CPU, flagged stuck, which ends nothing, while the NAS answered every other read at once).
  `NasPath`'s other calls (whole reads, `NasFile` reads and writes, stats, listings, copies,
  writes, renames, removes), crate::blobs' copies, the content-named uploads' calls and a base
  pack's ranges are each waited for `LONG`, a read, listing or copy counted from its last move. A
  job ending with calls left in the kernel says its end to its agent before its exit, which may
  never end (docs/nas-access.md §6). A raw tile's archive read tries once
  (the file opened again if its handle is gone), then takes the tile from the next source; its
  archive's entries are read as an open is, within the bound. Each of these files is content-named
  (a pack's or archive's name is its hash, never rewritten), so the file opened again by its path
  is the same bytes; a `PlainFile` of a file not named so is checked by its length alone (another
  length: `InvalidData`, not tried again). Being `PlainFile`s, their reads count in the job's NAS
  bytes (`store::nasio`, its NAS weight's `bytes`; a copy here, this Mac's disk's) and their
  descriptors are among the `plain_open_cap` held open (a quarter of the process's open-file limit;
  the one read longest ago closed past it, opened again at its next read).
  A writer SIGKILLed mid-write leaves only its
  temporary file (test `whole_kill`, on local disk, not SMB). No NAS file is flock'd or opened with O_EXLOCK/O_SHLOCK: every
  lock (`sys::PathLock`: the build lock, the member and agent locks; store::cachefile's holds) is
  on a local file; the declaration in a new pool uses create-new (O_EXCL), an atomic create whose
  handle is closed once written.
- **No conditional writes or atomic appends** between clients.
- **Listings are slow** (3–33 s a folder under load): the pool's protocol lists nothing; the records
  are read at one path, and hand-offs and heartbeats go to the lead by HTTP.

## 4. Invariants

Checked by the simulator (§13):

1. Only the declared lead leads (no member ever does), and one process of it at a time (the member
   lock, §5). A lead whose read of the declaration names another member leads nothing (an old lead
   started again after the owner moved the lead by hand, §6.1).
2. Each take-up leads its store's epoch, past the records it starts from (its store numbered
   them, bumped since as it opened: a store whose epoch isn't past them isn't the one that saved
   them, and leads nothing), so (epoch, seq) orders every snapshot, and job ids (`<epoch>-<n>`,
   §7.5) order, across restarts and a hand move.
3. No accepted hand-off is lost: its member keeps it until a saved snapshot names it, and a lead
   never takes up from records older than those it saved (§6.2, §7.3).
4. Each records snapshot is one file written whole.
5. A late write of an earlier save never sends the lead back: it takes up from the records it
   kept on its own disk (§6.2), never the NAS's. At the one path, readers read a late write until
   the lead's next save (`driver::RESAVE_S`, 10 minutes, at most). Not covered: the lead's disk
   lost (its records and its store): it leads nothing, saying so, and the owner decides.

## 5. Roles

- **A member** is any Mac running the agent with the NAS. Its id (`m-<16 hex>`) is kept in its home
  folder bound to the hardware UUID (a copied folder makes a new id). One process per member runs
  the pool, holding `pool-<id>.lock` in the app's folder (`crate::pool::MemberLock`, checked every
  step; a process that loses it stops, saving its state). A member writes its heartbeat (§10), runs
  job slots (§7), brokers its jobs' tasks (§8), and passes its build page's asks to the lead.
- **The lead** is the declared member (§6.1): the Mac mini, always on, at home and on power. It
  also plans (`crate::agent::build`) and runs the coordinator, merges the members' hand-offs into the
  records, updates the map, runs GC, compacts the raw tiles' archives, keeps the pause and
  writes the build's status. Its slots are ordinary member slots: its agent asks its own
  coordinator for work as any member asks (§7.2).

## 6. The lead

### 6.1 The declaration, and epochs

**The declaration** is `state/build/lead-member.json` (`buildstore::records::lead::Declared`,
re-exported as `crate::pool::lead`):

```json
{ "member": "m-3f9c…", "host": "Mac-mini-de-Brandon" }
```

(A `since` an older app wrote is read past.)

- **Read every step** by the lead (`lead::declared`; a read that fails keeps the last), and by a
  member only while no answer of its lead's within `PAGE_AWAY_S` (35 s) said who leads
  (`Answer::leading`: its member id, host and app; §9): with one, the member takes that member for
  the declared lead. A member it doesn't name never leads (`Driver::step`'s `declare`); a lead it
  no longer names leads nothing from its next step.
- **Made once** with create-new: in a new pool (no records at the one path nor its brief), by the
  first Mac in it, which declares itself (`lead::declare`); otherwise by the owner, by hand. With
  none there and records saved, no Mac leads (a lead steps down: "no lead is declared") and the
  members wait. Its being there is the pool's having begun: no reader takes today's files for the
  records then (`records::now`), and `out::check_writer` refuses every direct save of them.
- **Why a file and not a setting on each Mac:** one place every Mac reads, so two Macs can't each
  think they're the lead; written once with create-new, so no Mac's process replaces it. No member
  id is named in code. Its coordinator grants nothing from its start until its pool's step says it
  leads (`agent::UNSTEPPED`).

**Moving the lead** (the mini replaced) is done by hand, outside the app, which has no move of its
own: the lead's state is in its agent's folder (`~/Library/Application Support/scenic/agent/`) on
its own disk, and the new Mac takes it.

1. Stop the old lead's agent (`launchctl bootout gui/$(id -u)/<the agent's label>`, or quit it
   from the status app), and the new Mac's.
2. Copy from the old lead's agent's folder to the new Mac's, each in place of what the new Mac has
   there (its own `coord/store/` or `maplog/` removed first, not merged into: a store's log
   segments it kept would be replayed over the copy's):
   - `coord/` but its `jobs-token`: the coordinator's store (`coord/store/`), its history, its
     timings, and the files a store's first fill reads;
   - `maplog/`, whole: the map's log, its snapshot, GC's queue and its sweeps' places;
   - `pool/records.json`: the records it saved last, which the new lead takes up;
   - `publishing.json`: the map updates' record (when one was first held back, the cadence-gated
     steps' last runs).
   Not the rest: `member` (each Mac's own id), `lead.json`, `coord/jobs-token` (each Mac's own
   jobs' token, which its jobs running across the stop hold), the per-slot files and the rest of
   `pool/` (its saved state as a member, its mail) stay each Mac's; what else only a lead writes
   (`status.json`, the page view's waits, GC's depth keep, the inputs' listing, the backups' local
   copy) is made again by the new lead.
3. On the old lead, rename its `coord/store/`, `maplog/` and `pool/records.json` aside (for
   example with a `.moved` suffix; remove them once the new lead leads). Then the old lead, started
   again, can't lead from them even while its share shows it the declaration as it was (a stale
   read, §3): with no records on its disk it takes nothing up (§6.2).
4. Edit the declaration to name the new Mac's member id (`scenic pool status` lists them; the
   host is a label).
5. Start the new Mac's agent, then the old one's: the new lead's coordinator opens the copied
   store as its own (its epoch one past the old lead's, every lease in it, the leases of the new
   Mac's own slots of before dropped as at any restart, §7.5), and its driver takes up the copied
   records in that epoch. The old lead, declared no more, is a member: its jobs, taken over by its
   agent as at an update, beat the new lead, which holds their leases by its word. Members find
   the new lead by the declaration as their old one stops answering (docs/coordination.md §3.1).

The pool's token (`coord/workers-token`) is the same on every Mac; `coord/page` (where its page
is served) is written again as its coordinator starts. A lead whose disk lost its records or its
store isn't moved this way and doesn't lead (§4 invariant 5): the owner decides.

**The epoch** is the lead's coordinator's store's (docs/coordination.md §4.3), bumped as the store
opens, so once a process; a take-up leads it, its records numbered on in it. The records' snapshots are numbered (epoch, seq), their JSON key still
`term` (older apps read them); a lease's job run is named `<epoch>-<n>` (its job id, §7.5).

- No app writes the old term folders (`state/build/term/<E>/`, their `forecast-input.json` alone
  left: §7.9, `fccheck::inputs` reads them; docs/coordination.md §9).
- There is no app rule: the lead leads whatever app it runs (a development build, a rollback).
  Records keep keys of steps an app doesn't know (`pool::records::Keys::other`), so a downgrade loses
  nothing it can avoid (docs/buildings3d.md §3.6).

### 6.2 The records

- **One snapshot, saved whole after each merge** (`crate::pool::records`, `Records::save`): the
  manifest, keys, pending uploads (by content name alone, `buildstore::pending`, its shapes in
  docs/formats.md; an entry's `checked` takes them out: GC's job, which checks them on the NAS,
  docs/storage.md), the sizes of the files the manifest names (from the entries'
  hand-offs, for the map updates: `sizes.json` beside `manifest.json`), raw archives awaiting the
  index (each taken off once the lead has named it in the raw store's index and saved that,
  `Heard::named_raw`, with the next snapshot; one named but not yet off when the lead restarts is
  named again, passed over unread; an archive that index lists as merged away is never named
  again, task #607), the hand-offs it reflects
  and refused (with why), the lease that last set each target, its epoch (under the JSON key `term`) and `seq` (one more
  each save). Also saved, unchanged, `driver::RESAVE_S` (10 min) after the last save reached the NAS.
  A save goes:
  1. to the lead's own disk, `agent/pool/records.json` (`Io::keep`: written whole and flushed, its
     folder too, `seglog::write_durable`); a failure there writes nothing else, and a take-up
     isn't done (tried again next step);
  2. to the NAS's one path, `state/build/records.json`, and its brief `state/build/brief.json`
     (`records::ONE`, `ONE_BRIEF`; `Nas::write_whole`). Nothing else: no today's files.
  A save kept on the lead's disk and not written whole to the NAS (past its 20-s deadline, or
  refused while an earlier write to that path still runs, crate::agent::nasread) leaves the lead
  leading: it acknowledges nothing and isn't caught up (no map update, no GC) until a save reaches
  the NAS, tried each step.
- **Taking the records up** (`driver::take_up`, once a process, or again after the declaration
  names it once more): it needs its store (its epoch: the agent's first step runs before its
  coordinator starts, so it waits a step). It starts from its own records (an earlier try's, or its
  disk's copy, `agent/pool/records.json`), none older than `Saved::records`; it never reads the
  one path for them. With none of its own: in a new pool (no one path, no brief and nothing saved
  before) it takes today's files (`manifest.json`, `jobs.json`, `pending.json`); else it waits,
  saying so (a lead moved by hand without its records, or its disk lost). A store whose epoch isn't
  past its records' waits too (not the store that saved them). It numbers them (epoch, 0) in its
  store's epoch and saves them. The history says how (`Event::TookUp`): "its own records", "today's
  files: the pool's first".
- **Merging**: it merges what members posted it and its own jobs' hand-offs, from memory, in (epoch,
  lease) order (`records::merge`). It is **caught up** (`caught_up`) once every hand-off it was
  given is merged and saved; map updates and GC wait for that. It reads and lists nothing on the
  NAS for them.
- **Acknowledgements** (`journal::Mine`): a lead acknowledges a hand-off once a saved snapshot names
  it (in its beat's answer, `merged`); the member drops it.
- **Readers** (the jobs' `Out`, the agent's plan and status, GC, the Mac mini's map server, the CLI
  and the checks) read the one path's snapshot alone (`Records::now`, buildstore::records::now: one
  NAS call). Before the pool (no declaration) they read today's files `manifest.json`, `jobs.json`,
  `pending.json` and `sizes.json`, which a Mac's own jobs write without a pool; with a declaration
  and the one path not readable (a stale read), an error, never today's files. Readers of the
  manifest or keys alone (the map server's syncs every minute, the hosts, the plan and status, job
  starts, the CLI and checks) read the brief instead, the same way (`records::Brief`,
  `state/build/brief.json`: the manifest and keys, ~6.5 MB against the snapshot's ~14 MB, which is
  mostly the unverified uploads). An idle agent's plan digest stats the one path's snapshot. The
  map server reads them through its own pool (`pool::nas::ReadOnly`). **Agents read the brief
  from the lead** (crate::agent::brief, docs/coordination.md §5.1): the lead's in process (its
  coordinator holds the brief of its last save), a member's by `GET /pool/brief` (304 while
  unchanged); a job of a pool's lease opens the brief its agent had from the lead as it started it
  (`SCENIC_BRIEF`, a file in the agent's folder), never the NAS's snapshot, and isn't started while
  the lead doesn't answer (its lease given back). The NAS's brief is read by the map server and the
  CLI, and by an agent whose lead serves none (an older app's, or none saved yet in its process).
- **Today's files** are the records before the pool: a new pool's first take-up is made from them.
  No lead writes them.
- **The coordinator's state** is in its store on its own disk (docs/coordination.md §4:
  `agent/coord/store/`): a restart opens it, so it keeps its leases. Nothing of it is on the NAS.

### 6.3 With the lead gone

When the mini is asleep, off the network, its agent stopped or its share hung, no Mac takes its
place:

- **A member finishes the jobs it holds.** Each runs on; its hand-off is kept whole in the member's
  saved state as it ends (§7.3), and its beats list it. A renewal or end the lead doesn't answer is
  retried until it answers (its lease held meanwhile: docs/coordination.md §6); the work is on the
  member's disk either way, and is posted to the lead when it's back.
- **Then it waits.** It asks the lead for work, and gets none until the lead answers; it never
  leads, however long (`Driver::step`: only the declared lead takes the records up).
- **Every view says so** (§10): "no lead: <host> is out of touch", and after an hour the alert,
  saying to see to that Mac (awake, on the network, its agent running).
- **The lead, back,** merges what its members post it and grants again: woken, in the epoch it
  led; restarted, in a new epoch, from its own records (§6.2). Its leases granted before are in its store; a job whose
  lease lapsed meanwhile is offered again.

### 6.4 Asks of the lead

A member's build page passes its asks (a job's cancel, a target's retry, §11) to the lead by mail
(§9), only once the lead's app is known (from its beat answer, `Answer::leading`) and equals its
own (an older app's mail reader would fail on them); else it says to ask on the lead's own page.
There is no ask about who leads. `scenic lead` only shows the pool (§11).

### 6.5 Grants and the lead's view

A lead's coordinator grants only while its agent's last step said it may (`agent::poolside::moving`:
the records taken up, its member's lock known held, still leading) and told it so within
`UNTOLD_S` (5 min: `Coordinator::set_moving_as_of` stamps the step's time, not the tell's, so a
hang after the step (a history write on the share) or before the next one, or a Mac woken before
its step, stops grants until a step runs again). Built files are content-named and never rewritten
(docs/storage.md, Immutable, content-named files), each hand-off is its own entry keyed by
its job id, and a merge applies an entry only if no later lease set its targets (`Records::apply`;
an earlier epoch's job ids sort before a later epoch's), else counts it overtaken; the files it
alone named go to GC.

### 6.6 Stepping down

There is no re-assertion: the lead leads one epoch a process, and nothing else makes one. It steps
down when its step's read of the declaration names another member ("another member is the declared
lead") or none ("no lead is declared"), and when another process holds its member's lock (§5).
Declared again, it takes up its records on its disk.

### 6.7 Clocks and wakefulness

- No NAS clock: out of touch compares a heartbeat's `beat` with the reader's clock; a beat over
  60 s ahead shows "clock wrong". Everything persisted is wall-clock time.
- **The pool is stepped whatever the agent is doing** (`crate::agent::poolside`'s `Ticker`): the agent's
  loop steps it about every 20 s, and the pool's own thread steps it when that loop has been quiet
  for `QUIET_S` (25 s). Jobs and the coordinator's start/stop stay with the loop. A tick held past
  `UNTOLD_S` (5 min) is taken as hung on the share: the lead grants nothing, its coordinator's
  answers stop saying its epoch and member (`Coordinator::set_member(None)`, until a step runs
  again) and the status says so, with the manual fix if it lasts: disconnect that Mac's SMB session
  in DSM (docs/macs.md). Nothing remounts a hung share by itself (the self-heal's remount waits for
  idle slots, `crate::agent::heal`).
- Every coordinator answer carries `x-pool-epoch` and `x-pool-member` (`coordwire::POOL_EPOCH`,
  `POOL_MEMBER`), except while its Mac has lost the NAS or its pool step is taken as hung: a member
  counts only its current lead's answers toward its reach (§6.8).
- **The lead stays awake** (`caffeinate -i`) while it leads with leases out.

### 6.8 Reaching the lead

A lead the members can't reach (e.g. macOS's application firewall dropping incoming connections)
grants them nothing. `crate::pool::reachable`, `Run::tend_reach`:

- Every member probes the lead's coordinator and the other brokers every `PROBE_S` (60 s); its
  client's own requests count too. Results go in the heartbeat (`Beat::reach`): reached within
  `REACHED_S` (5 min), or unreachable for `REPORT_S` (2 min) with the cause.
- One failed request makes the rest of that loop's requests to the lead fail at once
  (`Client::new_round`); a job's end not taken is retried until the lead answers.
- The client keeps its connections alive (docs/coordination.md §5.2): a request on a kept connection
  that hears nothing for 2 s connects to the lead anew, so a lead gone asleep is unreached within
  about 7 s, as a new connection's 5 s would find it.
- Every view says who can't reach whom and what to do (`View::unreached`): wake the Mac, or allow
  `scenic` in System Settings › Network › Firewall. `scenic` is signed ad hoc at a new path per
  version, so that allow lasts until the next update; a lasting one needs a code-signing identity
  (not set up). Nothing moves the lead off a Mac its members can't reach.

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
most its jobs could hold (`limit_mb`: its memory less its memory floor), what the running jobs hold
(together and each, `held_each`), its memory against its floor (§7.1, The memory floor: what's
available and what compressing frees among it, the floor, the swap grown, what's reserved for its
jobs' growth, and why no job may start, `mem_held_back`), what its jobs may hold for one to start (`build_mb`) and what's
left of it less each running job's need (the larger of what it holds and the peak its grant was
packed at, `Grant::peak_mb`: the lead's measure of its targets, on the lead and a member alike), its
cores and those the last minute's CPU leaves free (`cores_free`: the cores busy by the CPU's
ticks, the live store's `cores_busy` over the minute, rounded up; not sampled yet, the load
average, which counts threads waiting on the NAS too: on 10 Nov the M4's read 13 while its CPU
was 8.3 cores busy, task #1091), the disk free less the owner's room target
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
is first written, and their targets' disks again as `job-disks-v6.json` is (task #657: those of
51–68 GB, which no Mac's room fitted; docs/formats.md). `depth-lo`'s (task #398, `coord::REMEASURED`): its job
peaks and targets' measures past three times its targets' median and a GB (a 35 GB peak against
65 MB targets, which kept it on the lead's Mac) are dropped each time the coordinator loads them,
so it runs on any Mac by its measured use. **Shared caches** (task #392, `needs::CACHES`): `depth`, `depth-lo`, `terrain-water`, `lakefill` and `water` read the whole copies (task
#551, `localread`: whole local copies of the basemap, the lakes' fill and the depth sources' packs in the cache's
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
  (`caches_here`: the bytes of its cache folder's `blobs/layers/basemap*`, `blobs/work/lakefill*` and
  `blobs/sources/fetched/<id>/*.tar`, the whole copies; the terrain's copies of the stored GeoTIFFs,
  `*.tif` beside them (task #689), are its jobs' own disk, not this cache), so
  whatever emptied them (room-making, the OSM pass's clear, a hand) counts.
- *The reservation*: the first job granted on a Mac reading a cache (a cold start too) carries
  in its `Grant::disk_mb`, beside its output, the fill less what that Mac holds of it: nothing
  when it holds the fill, the rest when part, the
  whole fill when it holds none or its ask says nothing of its caches (an older agent). The jobs
  beside it on that Mac carry their output alone. Files of an earlier pass count as held until
  the next job reading them prunes them. Its agent counts the grant in
  `reserved_mb` (as `disk_words` and the build page's disk bar show it).
The whole copies a Mac has are on its disk, outside the free space an ask says (held by this
rule, not counted free twice); the room-target freeing and the disk guard (`room::toward`) can
delete those no job holds. For a job of a step reading none of them, on a member with a room
target none of whose jobs (held or granted in the same answer) reads them, the lead counts them
free beside the ask's disk (`sched`'s `Copies`, task #1170): its room-making and the freeing toward
the target and its reservations delete them for that job, those no job there reads first, then
those the forecast's jobs there read, the furthest ahead first (`room::Hints`; the basemap's only
once the others have freed its bytes), so a finished step's copies never hold a later step off a
Mac that has the room once they go (12 Nov: lo 3/6/3, 35.2 GB, waited two hours while the M4 held
28 GB of the depth steps' copies with 16 GB for jobs, until a hand cleared them). What a grant
takes past the ask's disk is spent of them, and a job reading one granted after it in the same
answer reserves what was spent beside its fill. In the answers after, while the copies are still
there, so is what the Mac's reservations take past its free space (its ask's disk at 0: the room
target and the reservations past the free space, its cheap caches not said), and the disk the
lead takes off for the jobs it holds past the disk said: the copies are counted once. A job reading them counts none (they're its
cache), nor does the lead's own ask or a Mac with no room target, as for its other caches. A
grant's agent reserves its disk while it holds the lease: the whole of it until
the job starts, then that less what the job has written so far (the disk guard's count). The
lead's own running jobs reserve what their measures say they have still to take (`disk_left`).
A reservation ends with its lease, at hand-off or give-back. The free disk an ask and its
capacity say (`disk_mb`; the lead's own asks' likewise) is the free space, on a member with a room
target with what its caches can free (`agent`'s `freeable`: `room::helper_cheap_bytes`, the canopy
squares, the copies of the records' files and the raw terrain's archives, but the whole copies above
and the files a job holds now, which room-making leaves; counted at most every ten minutes, and
again after a job or a freeing), less the owner's room target and
every live reservation (`reserved_mb`), so a job not yet written isn't counted free twice. A
member's job granted for what its caches hold starts with the room past the target and the other
jobs' reservations (a shared step's), and its caches are freed toward its reservation while it
runs (the freeing toward the target and the reservations, `goal`), the disk guard behind it: freed
short of what was counted, its growth stops at the target (the guard freezes it there, makes what
room it can, and gives it back, failed, kept from that Mac an hour). With no target the guard holds
only a full disk while the freeing runs beside the job's writes, so a member's caches are counted
only with one (task #1000: counted without its caches, the M4, 37 GB free against its 30 GB
target and 77 GB of caches, was offered 7 GB for jobs and ran one, 11 of its 14 cores idle). The lead's caches aren't counted: it asks alone with its
disk unbounded, and beside its jobs a job of its own needs its measured room free as it starts or
is given back, so one granted for its caches would be granted and given back each loop. There is no
fixed reserve: the room target is the only floor (a Mac with 49 GB free, a 20 GB target and no
caches offers 29 GB, less its reservations). An older agent's `reserve_mb`
(a fixed 30 GB it took off too) parses and is passed over: its `disk_mb` has it taken off already,
so it only offers less; the coordinator takes off only the jobs a member holds that
its capacity doesn't name yet (granted since it asked, or an older agent's ask, all of them). The
lead running no job asks for one, its disk not bounded: a stale or high disk floor never holds the
build, its room made as it starts and the disk guard behind it (§7.7). A
job starting beside others waits on the target, its own measured room and the other slots'
reservations (a job alone in the first slot on the target alone); room-making alone makes room for the leases granted and not yet started too, and
the owner's room-target freeing (`room::toward`) frees toward the target plus the reservations,
the jobs' hints merged (`Agent::hints`: the queued and running jobs', and those the lead's forecast
places on this Mac, a member's asked of the lead's `/api/tocome` every 5 minutes; docs/building.md
§2, Room on the disk). The disk guard stays the backstop (§7.7). `scenic
status` shows each member's in its capacity line, as `coord::disk_words` says it (`disk 49 GB free ·
20 GB for jobs (20 GB room target, 9 GB reserved by its jobs)`). Every job is told the target
(`SCENIC_ROOM_TARGET_MB`, room::TARGET_ENV; 0 for the backup and GC, which keep none): the OSM pass
and pass-sets reckon their room past it, and run by hand, without it, keep none.

**Threads granted** (task #241). Each job granted carries the worker threads it runs with
(`Grant::threads`), which the member's agent sets as `RAYON_NUM_THREADS` (the programs taking
workers read it: `scenic-build`'s); costs record them (`threads`), with the CPU time the target
took (`cpu_ms`) and the time its process was stopped meanwhile (`paused_ms`: frozen by the memory
or disk guard, or paused for its conditions; `costin::stopped_ms`, a thread that wakes each second
and counts the time since its last wake, less a second, when a SIGCONT came meanwhile, `store::sys::continued`,
tasks #697 and #1155), and the wall time its process waited on the NAS, the network, the
disk or other work meanwhile (`waited_ms`, store::waits by the wall clock, task #870; where the
run is timed). **A job is granted the threads its step keeps busy on its Mac** (task #947,
`crate::uses`): a job holds its threads from its start to its end, so what it uses of them is its
CPU time over its wall time, whatever it waited on or however its work came in bursts. On 4 Nov
slope-deep kept 12–22 % of its threads busy on batches of nearly empty tiles and about 55 % on full
ones, terrain-deep 51–67 %; one M4 slope-deep job held 3 threads for five hours and kept 0.8 of a
core busy. The build's timings (the lead's `coord/timings.jsonl`: every job's `cpu_s`, `wall_s`
and `threads`, read as the coordinator starts and taken as each job's come in, `Sched::add_use`)
are grouped by step, Mac (a slot's worker is its Mac's, `paces::mac_of`) and threads, over the runs
ended well in the window the paces read (`paces::PACE_S`, 5 days, picked by their held-out test;
the log is cut to its newer half past 16 MB, about 3 days of the lead's on 4 Nov at 5 KB a line, so
a coordinator started after a cut reads what was kept, and keeps the window's runs from then on),
and only those of the way their step runs now: the lead stamps each record it keeps with its step's
cost version (`RunRec::cost_v`, `coord::cost_version`), and a run below the step's (or unstamped,
read as 0) isn't read, as the cost book's rule reads its costs (task #929 review: slope-deep's runs
before #952 kept their threads busy on z10–12 work it no longer does, terrain-deep's on a base it no
longer makes); each group's busy cores are its CPU time summed over its wall time summed, so a long job weighs as
long as it held its threads. The grant (`uses::Uses::cores`) is the most cores any group kept busy,
rounded up to whole threads, plus one where that group needed every thread it had (its busy cores
past one fewer, and half its threads at least) and no run there had more: a step its threads hold
climbs a thread a run, measured at each; one held by its serial parts settles at what it keeps
busy, and every core it would have held idle goes to other jobs (several slope-deep batches side
by side, a terrain job beside one). **A cut is a trial** (task #947 review): a step whose threads
each wait keeps fewer cores busy at fewer threads (slope-deep about 0.3 of a core a thread on the
M4, at 3 threads and at 14), so by its use alone it was cut run by run and never needed every
thread to climb back (14, 5, 2, 1 at 0.32 a thread; at 0.85 a thread, 14 down to 6 in a dozen
rounds). So where a run at the grant or fewer threads kept within a core of the busiest group the
cut stands; where it kept more than a core less, those threads paid, and the step holds the fewest
threads that kept within a core of the busiest. A run at more threads than the cut doesn't count
as its trial (a group's use is its batches' as much as its threads'). It counts for those busy
cores (a climb's thread besides; held, the held group's). A step's use on a Mac
stands whether the job is a cold start or not (the threads it takes are measured even where its
memory isn't). What it is per batch isn't known as a batch is granted (no record says which of its
targets will have output before they run), so a step's mix of batches is taken as its runs had
it; a running job's grant may be decided again at its safe points (task #948, by the same rule,
`Sched::lease_cores`). Where a step has no run on the Mac in the window, the cost book's rule
below decides. The rule is `Sched::pack_need` and `need_cores` (a job's step, targets and worker,
and its Mac's cores for jobs; `Sched::lease_cores` the two for one job): the live scheduler's
packing and the forecast's (it runs that packing on the scheduler as recorded, `sched::State`,
which carries the uses; its grants say their threads) go through it; `State::cores_left` counts a
held job's busy cores as the lead counted them (`Leased::busy_tenths`, below), else from the same
uses, else the cost book. The whole-run use hides a step's
phases: pack keeps 0.1–0.9 of a core busy over its run, so 1 thread, but draws its road and rail
tiles on 4–5.5 cores, which at 1 thread makes its long runs at least 1.3–1.6 times as long (2
threads: 1.2 at most, 3: 1.07); trees' program ran at 1.2 cores (at least 1.16 times at 1).
So a step is granted at least its **phases' floor** (task #948, `uses::Uses::floors`): the
fewest threads at which its runs, together, are predicted to take no more than 5 % longer
than they did (`uses::WALL_SLACK`, a choice). Its runs on every Mac count: the threads a phase keeps
are the step's (its readers, its batches), and a Mac whose runs were all cut low can't show them (the
M4's one terrain-lo run in the lead's window was at 1 thread). Each top-level phase of a run's main thread keeps
the job's CPU while it was open (every thread's: its `job_cpu_s`, a record's before task #1116 its
`cpu_s`) over its wall busy, and one overlapped by phases on other threads also as many threads as
those phases' wall summed over the overlapped phases' (terrain-lo's sampling keeps 3-5 threads
waiting on GLO-30 at once: 1556 s of reads in a 533 s run); at fewer threads than a phase keeps it
takes that many over them as long, and a long run weighs as long as it took, so a rare short burst
holds nothing. From terrain-lo's 6-thread run it predicts 2151 s at 1 thread; a 1-thread run took
2063. On the lead's timings (the mini's `coord/timings.jsonl`, to 4 Nov; trees ran none in the
window), the grant by use alone, then with the floor:

| step | M4 | mini | M1 |
|---|---|---|---|
| pack | 1 → 3 | 1 → 3 | 1 → 3 |
| slope-hi | 4 → 5 | 2 → 5 | 4 → 5 |
| slope-deep | 11 | 9 | 7 |
| terrain-lo | 1 → 2 | 2 | – |
| terrain-hi | 5 | 5 | 3 |
| terrain-deep | 3 | 4 | 5 |

(The floors by each Mac's runs alone would have left pack and terrain-lo on the M4 at 1.) A run whose targets ran on their job's grant as each began
(Threads re-granted, below) says what each thread count held (`timings::RunRec::held`): it's read
at each (`uses::Run::of`). Without a run on the Mac, a job's
measured cores (`needs::Book::cores`, its own run's, else the most of its
step's) are two (task #896: no hand-set ratio or factor): its **grant**, the cores it kept busy
while it computed (its CPU time over the time it ran and didn't wait; no waits said, over the time
it ran), within its threads, to the nearest whole, plus one thread: to overlap its waits, or, where
it computed on every thread it had, to climb by. So a job waiting on the NAS or the network, or held
by its serial parts, settles where a run measures the same (2.4 cores busy of 6, of 4 or of 3: 3;
a serial step: 1–2) and never climbs, and one its threads held climbs a thread a run, measured at
each, to what it can use (capped at the Mac's cores); and its
**busy** cores, that CPU time over the time it ran (its wall time less the time stopped) to the
nearest tenth of a core (a tenth at least, never past its grant; the packing sums a Mac's jobs'
tenths, so rounding no longer undercounts by up to half a core a job), plus, for a climb, the
thread it climbs by: none measured it idle yet, so a climbed job counts for every thread it
may keep busy and no Mac is overcommitted by a climb. A target not run the way it runs now, nearly
every one granted, counts for its step's most over its runs at the most threads it ran with (runs
at fewer, their climb measured since, say nothing past them), which no run crowded off the NAS
lowers. **A job whose waits dominate takes its threads from its step's curve** (task #904,
`needs::Curve`): a single run can't tell how much more threads help a step waiting on the NAS, so
each step's runs the way it runs now (the costs book, each target's last run) are grouped by their
threads, each run's wall time (less the time stopped) over its target's work (what the step's good
time fit predicts for its size, without its thread term; no good fit, the MB it read, where every run says some; else the wall alone); a thread
count of 5 runs or more (`CURVE_RUNS`) is a point: their median and its standard error (1.4826 ×
the median absolute deviation over √runs). The **knee** is the fewest threads that no more threads
beat by 2 standard errors of the difference (`CURVE_SE`): the margin scales with the runs' own spread,
not a set time. These three are statistical choices, not measures of a step: 5 runs is the fewest whose
median and MAD say anything about spread; 2 standard errors is the usual ~95 % bar against calling noise
a gain; one in ten bounds the share of jobs run off the knee while learning. With two points or more
known, a job whose waits dominate (the median of its step's runs' shares of time waited, `waited_ms`, is
half or more, taken over the step so one run doesn't switch a job between the curve and the climb, and
its own run didn't climb) is
granted the knee, whatever its busy cores' grant; under two, that grant. **One target in ten**
(`EXPLORE_EVERY`, by a stable hash of its name) is granted a neighbour of that (one thread more
first, else one fewer) while the neighbour has under 5 runs and that count itself has 5 or more, so the curve is learned around its
knee and the exploring stops once both are known. A curve's grant counts for the most cores a run
kept busy at the most threads known up to it, plus each thread past those as a whole busy core
(none measured them idle yet), so exploring never overcommits a Mac, and it stays within the Mac's
cores like any grant. Any other job (a climbing one, its runs computed on every thread, or one
computing more than it waited) keeps its busy cores' grant. `scenic pool needs` prints each step's curve, knee and grant. The cores a Mac's jobs may use together (`coord::ask_cores`) are its Mac's
less, while its owner uses it, its in-use share's. Each job is granted its
measured grant within its Mac's cores (task #888: no fixed share caps it, so a busy step's grant
climbs where cores sit idle; the M4's terrain jobs had been held to 3 of 14 threads; task #896: no
share floors it either, a step reading the NAS among them: its waits, measured, give it its
thread to overlap them); a job whose runs said no CPU time, the threads they ran with, counted
for them all; one whose runs said nothing of its threads, one thread. **A job
counts for its busy cores** (task #688, `Sched::need_cores`): the packing takes those, not its
threads, off its Mac's cores, so jobs waiting on their reads go beside each other until memory,
disk or the NAS budget is full rather than their threads: terrain-hi kept 1.16 cores busy of 2–3
threads on 26 Oct and counts for 1.2, where it counted for 3 and the M4 packed 3–4 of them ("cores:
2 of 14 left, a job of terrain-hi takes 3") with 22 GB of its 39 GB build memory free. The rule
reads the runs, not the step: a terrain job reading faster (from this Mac's disk, say) keeps more
busy and counts for more. **A running job counts for what it keeps busy now** (task #1091,
`sched::busy_now`): its beats carry its CPU time (`jobs::Live::cpu_ms`, the memory guard's
sampler, its processes' rises summed so a process ending never takes it back; a view saying less
than the one before, as an agent's take-over of the job counts its processes afresh, is counted on
from the one before, `Lease::cpu_carry`), and once it has
ended a target (one read and computed) or run past the time its targets are predicted to take its
Mac (`OWN_AGE` where none is known), it counts for the more of the cores it kept busy since its
lease's first beat and over its last minute or so of beats (`Lease::cores_mean`,
`Lease::cores_lately`, the NAS weight's two windows, §7.7: a job busy late counts for its busy
stretch, one quiet a moment between busy stretches for its mean), in place of its measure; before
that, or where its beats say no CPU time (an older agent's), its measure, so a job still reading
its first target isn't counted as using nothing. The packing, a re-grant's count of its Mac's
other jobs and placement's `swamped` count it so, and the forecast too (task #1094): the
scheduler's state as recorded carries no beats, so each lease in it says the cores its job keeps
busy as the lead counts it (`sched::Leased::busy_tenths`, tenths; left out until its run says
it), which the forecast's scheduler counts it for (`Lease::busy_said`, while no beat of its own
says) and `State::cores_left` takes off its Mac's cores; a record without it (an older app's)
counts the job for its measure, and an older app reading a newer record passes it over. So a Mac
whose jobs keep less busy than their measure is granted what the forecast foresaw (where the grants
depart from it, the lead logs "the grants departed from the forecast"). A step's measure is its busiest thread count's
over the window's runs: on 10 Nov it counted the M4's three terrain-deep jobs at 3.8 cores each
(266 runs at 5 threads on 8–9 Nov kept 3.75 busy; the 4-thread runs since, beside each other on a
busier NAS, 2.4–2.8), 11.4 of the 12 it had while in use, and a fourth was refused ("cores: 0.6 of
12 left, a job of terrain-deep takes 3.8") while they kept about 2.5 each busy over their runs.
A job's threads stay within the cores left plus those it leaves idle
(its grant less its busy cores). **Past the cores its jobs' threads leave, a Mac climbs one job
at a time** (task #688 review, `coord::SETTLE`): an ask is granted at most one job that fits only
by busy cores, and none until a minute has passed since its last grant (the span of the cores
busy its capacity reports; a job older than its lease, taken up at a lead's restart or from a
record as the forecast's are, since it began) or while those pass its jobs' busy cores by more
than a core.
So the M4 takes four terrain-hi jobs by their threads at once, then one a minute up to what its
memory, disk and the NAS budget hold, and stops climbing as soon as jobs keep more busy than their
runs said (a run's busy cores read low where it waited on a busy NAS). Cores busy past the jobs' by more than a core (the minute's rounding up) come off
the cores left as outside load does (`outside_cores`, at most half the Mac's), which stops further
grants but stops no running job. A cold
start that honours its threads gets one thread (task #896): its run measures it, and it climbs a
thread a run from there while its threads hold it. The agent, the lead's as a member's,
starts a granted job with those threads, within its Mac's cores less those the in-use share keeps
while the owner uses it (one at least; not less those its other jobs were given: its lead packed
them by their busy cores), and logs a
line per job: `agent: starting <job> (<n> threads: granted <g> of <c> cores, <k> kept for the
owner)`. The coordinator logs the threads with each grant.

**Threads re-granted** (task #948). A running job's grant is decided again while it runs, and it
takes the new one as its next target begins. Its agent keeps the threads it may use now in a file
of its slot's (`threads`, `threads-<k>`; `SCENIC_THREADS_FILE`, `jobrt::control::THREADS_ENV`), written as it
starts and at each re-grant, always replaced whole (`control::grant`), so a read never meets it
emptied. A step that takes it (slope-hi, slope-deep, terrain-hi, terrain-deep:
`control::on_grant`)
reads the file once as each target begins and runs that target on a rayon pool that wide, the
timings' main thread carried onto the pool's thread (`timings::Main`); a change written while a
target runs waits for the next, so no target changes threads under way, and the thread count
changes no output (`slope_pack`'s test: a z6 tile's packs the same bytes on 2 threads and on 8). The
elevation pool's sampling (`terrain_base`'s `prefetch_each`, in terrain-hi and
terrain-deep) runs its chunks as tasks of a second pool as wide as the target's grant, one a thread
(`control::on_outside_pool`), their passes (`dem::generic`) there too, while the target's thread
waits, working on nothing. Beside them a level's tiles
are made by as many threads again (task #1097, docs/steps.md §2, A level is a pipeline), each tile
on a turn the sampling's threads lend while they wait (store::waits::Turns): the job computes on
its grant's cores, no more (but for a thread back from a wait, until the tile on its turn is made). Not threads of their own
whose passes go on a pool: on the target's own one, whose threads were waiting for them, that work
never ran on a 1-thread grant, and every terrain-hi job stalled at 0 tiles (5 Nov, task #961); on
another beside them the job would run on twice its grant. A thread the target starts of its own is
in no pool, and rayon would send its parallel work to the process's global pool, as wide as the
job's starting grant: such work goes on that second pool too (`control::on_target_pool`). Test: `terrain::grant_tests` (terrain-hi and terrain-deep targets on
1 and 2 threads, within a bound, the same bytes) and slope-deep's re-granted batch. Its started line in its costs file says the threads (`memguard::Targets::threads`):
what the job runs on now. A paced job (an input check, tree cover: `inputs::pace`, one long target)
reads the same file every second. Other steps keep their starting grant (they say no threads, so
nothing is re-decided for them).
- **What the lead hears.** Each capacity (a member's in every beat, `held::Beat::cap`, every 10 s;
  the lead's own each loop) lists the jobs that say their threads (`Capacity::threads`): the
  threads each runs on now and the CPU time its process group took on them (the memory guard's
  sampler, `memguard::Sampler::cpu_ns`) over the time since it began on them (`Agent::tend_grant`;
  a frozen or stopping job measures nothing).
- **The decision** (`Sched::regrant`): on a minute of use on its threads now (`SETTLE`), at most
  once a minute, by the grant rule (task #947): its own run so far as a run of its step on its Mac,
  among its step's runs there (`uses::Uses::cores`: a cut a trial those at fewer threads decide,
  never under their phases' floor, above), and where that climbs, the more of it and what a job of
  its step is granted there now (`Sched::lease_cores`); fitted into its Mac's cores for jobs (less
  the in-use share's while its owner uses it) less the cores its other jobs count for and the load
  outside the pool, as a grant is (`fit_threads`), and the most it can run on (`JobThreads::most`:
  its Mac's cores less the in-use share's, a paced job's 8 at most). It **grows** past what its
  agent has told it only where its Mac's memory admits work (no brake, floor or give-back
  cool-down: `Capacity::mem_held_back`, §7.1's memory floor) and each extra thread fits the
  capacity's `mem_mb`, less what its jobs' earlier grows hold, at its step's measured peak over the
  threads that peak was measured at (`Sched::per_thread_mb`; the whole peak a thread where they
  aren't recorded, as a regions check once needed). Its agent counts a job at its measure alone, so
  the lead adds what a grow took past the threads its grant was admitted with (`Lease::start_threads`)
  and its measure's (`Sched::grown_mb`), there and in packing. It **shrinks** to fewer only where its Mac's jobs' threads pass its cores for jobs
  (the owner back) or its Mac's last ask, within two minutes, was refused for cores: other work
  could use them. So a slope-deep batch keeping 1 of 8 threads busy keeps them while nothing waits
  for its Mac's cores, and gives them up when something does; and a job granted 3 of the M4's 14
  while its owner used it (4 Nov) climbs to its step's grant a minute after the owner leaves.
- **What the lead counts** (`Lease::threads`, `Leases::regranted`): the most of the threads the job
  runs on, those its agent has told it (its file: `JobThreads::granted`) and those still to tell: a
  grow at once (its next target may take them), a shrink once a target runs on them. A lead that
  takes over knows no re-grant its predecessor decided, but counts what the job's file says, which
  its next target takes. Packing counts a held job at no more than that (`Need::held`); it had
  counted each at its measured grant, so a job granted 3 of 14 counted as 13 and kept its Mac
  looking full. A job's threads may pass its Mac's cores while its threads wait (its busy cores fit,
  as a grant's do); a long target takes a re-grant only when it ends, counted meanwhile at the more.
  Two things run past the count for a while: a paced job's tasks under way when its file shrinks
  keep their threads until each ends, and a job's work outside its targets (its start, its last
  uploads' landing) runs on its process's pool, as wide as its starting grant.
- **Telling it.** A member's beat answer carries the re-grants its agent hasn't told the job yet
  (`Answer::threads`, said again each beat until its capacity says the file has them); the lead's own
  agent asks its coordinator each loop (`Coordinator::regrant_own`). The agent writes each, within
  `JobThreads::most`, to the job's file and logs `agent: <job> re-granted <n> threads (was <m>)`; the
  lead logs each decision.
- **Mixed versions.** An older member's beat carries no capacity, and an older agent's capacity no
  job threads: nothing is re-decided, its jobs keep their grant. An older lead reads past the beat's
  capacity and answers no re-grants (`Beats::lead_threads` empty; a lead answering 404 clears
  them): the jobs keep their grant. An older job (adopted from an older app) says no threads.
- **Why not shorter batches.** Capping a job's length by its predicted time, so grants are decided
  more often, pays a job's start on every batch: on the coordinator's timings (2,683 runs to
  25 Oct) a job's records and regions read took a median 9.4 s of slope's 22 s runs, 12.4 s of
  terrain-deep's 15 s, 5.7 s of terrain's 237 s, 18.6 s of pois' 27 s, besides each hand-off's merge
  and ask. A re-grant at a target's start costs a pool's making (milliseconds), only when the grant
  changes, and catches a single long target's job too (a paced check). Batches stay at the batch
  length (§7.7, Batch length).

**The in-use share** (`crate::agent::share`): the cores a Mac's agent keeps free of its jobs for
its owner (by default 2) while the owner uses the Mac (used within the last five minutes).
`scenic in-use [--cores <n>] | off` sets it in the agent's folder (`in-use-share.json`); `off`
clears it, the default again; with nothing, it says how it stands (and says so where the file can't
be read, the default then in force). The agent reads it each loop, as the disk room target.

**The memory floor** (`crate::agent::memfloor`, `crate::agent::memguard`, task #932): the memory a
Mac keeps available, whatever runs on it (the build's jobs, its agent, the owner's apps), as the
disk room target keeps a minimum of free disk. Set per Mac, whether its owner uses it or not:
`scenic memory <GB>`, the build page's Settings row or the memory chart's line (dragged) set it in
the agent's folder (`memory-floor.json`); `scenic memory off` clears it; `scenic memory` says how it
stands and the memory as the guard last sampled it. The agent reads it each loop. Not set, the
Mac's default, an eighth of its memory (6 GB of the M4's 48, 4 of the Mac mini's 32, 2 of the M1's
16), shown as the setting's value: on 4 Nov each of the three, busy and compressing, kept a file
cache of 11–12 % of its memory (the M4 5.4 GB, the Mac mini 3.6, the M1 1.8) that macOS held rather
than dropped, and often more (below, *Past two fifths*), so a Mac's available memory falls near an
eighth once macOS compresses rather than drops cache. The floor is taken as set: no cap, reserve or footprint stands behind it (the agent
counts as any other process).

- **Available** (`sys::memory_available`, `sys::Pages::available`; task #945): what macOS hands
  out at no cost, and what it frees by compressing other processes' idle memory with its pressure
  left normal.
  - *At no cost:* the free pages, the file-backed pages (the file cache), the purgeable ones
    (`host_statistics64`), and the reusable ones (`vm.page_reusable_count`, task #1215) as far as
    the inactive anonymous pages hold them. A read-ahead's speculative pages are in both the
    kernel's free count and its file-backed count (file-backed + anonymous = active + inactive +
    speculative, to the page), so they come off the free, as `vm_stat` prints it. Never raw free
    pages (macOS keeps those near zero), and not `kern.memorystatus_level`: that counts every page
    neither wired nor compressed, apps' own pages in use among them, and read 60 % on the M4 at
    01:10 on 4 Nov with 21 GB swapped.
    - *Reusable pages* are anonymous memory a process freed and its allocator told the kernel it
      no longer needs (malloc's `MADV_FREE_REUSABLE`): out of the process's footprint, still in
      its resident size and in `vm_stat`'s anonymous pages (Activity Monitor's app memory), marked
      clean. xnu leaves each on the queue it was on (most are active: memory just used); the
      pageout scan frees one with nothing compressed when it reaches it on the inactive queue, in
      turn with the anonymous pages there, which it compresses. So they come out no sooner than
      the other idle memory: counted at no cost only up to the inactive anonymous pages (the
      inactive queue less every file-backed page not speculative and the purgeable), and off the
      idle memory compressing is reckoned on. The rest are counted nowhere.
    - *What leaves them* (task #1251; macOS 27.2 on the M4, with `MallocLargeCache=0` as the
      agent and its jobs run, a program allocating blocks of one size, writing to them and
      freeing them, wave after wave): macOS's malloc keeps a freed block's pages for blocks of
      that size. A block under about 2 MB is never unmapped (1.9 MB kept, 2.3 MB unmapped as it
      is freed), and its pages go to blocks of another size only in part, among the larger sizes
      (1.2 MB blocks took 100 KB blocks' whole; 1.1, 1.3, 1.5 and 1.9 MB in turn left 0.74 GB
      for 0.42 to 0.57 GB each): 0.5 GB of 600-byte blocks, then of
      100 KB, 48-byte and 4 KB blocks, each wave freed before the next, left 2.0 GB resident and
      reusable beside a footprint of 10 MB; waves of one size reused their own (0.46 GB however
      many).
      `malloc_zone_pressure_relief` gives none of them back (0 MB of 0.5 GB freed, 600-byte
      blocks or 1.2 MB ones). So a process's reusable pages are about the sum, over the sizes of
      its blocks, of the most each size held at once, less what it holds now: they come back
      only as the kernel's scan reaches them, and what a long-lived process can keep down is
      each size's most, never by freeing sooner.
    - *Measured* (12 Nov, a program taking incompressible memory in steps and writing to it all
      the while, as a job's working memory, beside a process holding memory it had just marked
      reusable): on the M1 (nothing free), of 1.3 GB taken 0.8 came out of the reusable pages
      and 0.2 from compressing; on the M4 (four runs) the free pages went first (10–23 GB), then
      the file cache down to 5.4–5.6 GB, then of the next 2–4 GB taken 1.4–3.6 GB out of the
      reusable pages, before macOS compressed the taking program's own memory in a burst
      (below). In the fourth run (4.5 GB marked reusable, 5.6 GB of them in all) 1.4 GB of them
      were handed out and 4.0 GB were left when it compressed 17 GB of the taker's memory at
      25.1 GB taken; counted available at the start: 28.7 GB, 34.4 GB with all the reusable
      pages counted.
    - *Not all of it is handed out.* Past that point macOS compressed the taking program's own
      memory in bursts (5 GB, then 17–20 GB, once with the pressure at warning for a sample and
      32 MB swapped; decompressed within 5 s; 23 GB left compressed and the pressure at warning
      in the run that wrote its memory once), while 10–17 GB was still counted available: the
      5.4 GB of file cache it kept (a ninth of the M4's memory, as on 4 Nov, below), the reusable
      pages its scan hadn't reached, and 3–6 GB counted by compressing that was the taking
      program's own memory (below: the kernel keeps its inactive queue as long as its active
      one, whatever is in use, so a job's own memory fills the queue the idle memory is read
      from). So near a floor under the cache macOS keeps (the Mac mini's 1 GB, the M4's 4 GB)
      the last GBs counted come from compressing memory in use, and the brake is what stands
      behind them. (Counting neither the kept cache nor a job's own memory as available: task
      #1244, planned.)
    - *The Mac mini's* (19:01 on 12 Nov, no job running): 0.1 GB free, 7.6 GB of file cache,
      0.3 GB purgeable, 1.8 GB of idle anonymous memory and 5.5 GB of reusable pages, most of
      them its agent's (resident 5.6–6.2 GB, 1.5 GB of it mapped files; footprint 1.6 GB, at most
      2.8 GB in its first half hour). Counted: 9.9 GB (the idle 1.8 GB as reusable, at no
      cost), where it was 8.9 GB (the idle 1.8 GB as worth 0.8 compressed) and 13.5 GB would
      have counted all 5.5.
    - *The agent's* are left by its first plans after a start (above, *What leaves them*) and
      then stay: 36 min after one (13 Nov, app 20261113-0149) it was 6.9 GB resident, 3.96 GB of
      it malloc's reclaimable pages (`footprint <pid>`: Malloc Small), and a plan moved neither
      by more than 0.05 GB (sampled every 2 s for 9 min). Most were one list's: the plan built
      every depth target's water tiles, 3.1 GB of 1–2 MB blocks at once, to ask the water's
      digests by the targets' names. It asks by name since #1251 (`build::depth_water_wants`).
      On a copy of the lead's state of that day (`plan::tests::frozen_plans_memory`: the
      planner alone, seven plans in a row, the M4, test profile) the reusable pages after the
      seventh plan are 1.04 GB where they were 3.71, the resident size 3.04 GB where 5.73 (1.3
      GB of each the coverage's mapped file), the footprint's most 0.59 GB where 1.95; what is
      left is the first plan's (1.04 GB after it, and no more after the six that follow). That
      copy runs the planner alone: no coordinator, forecast, gate watch or pool driver, whose
      own blocks the live agent's pages hold too. The lead before #1251's rollout, 15 min after
      a start (app 20261113-0334, `footprint <pid>`): 7.0 GB resident, 4.6 GB of it Malloc
      Small's reclaimable pages, footprint 1.08 GB (at most 2.44). (The lead's own after the
      rollout: not yet read.)
  - *By compressing:* idle app memory macOS compresses cheaply counted as used made a 32 GB Mac
    mini at normal pressure show 6.2 GB available. The kernel's pressure follows its compressor:
    on the three Macs' live series (the 10 s ring, 2–4 Nov, the compressor's share of the memory
    not wired from `kern.memorystatus_level` and the compressor's pages) each went to warning
    most often once the compressor held 48–50 % of the memory not wired (174 entries: 16–33 % of
    each Mac's under 48 %, 6 under 40 %, as low as 31 %) and back to normal most often under
    39–41 %, and was never in warning a whole bucket under 39.8 % (the kernel's warning line on the
    pages not compressed, its way back at 1.2 times that). So the compressor may grow to two fifths of it
    (`sys::COMPRESSOR_NORMAL`) with the pressure normal; each page more holds what it holds now
    (its pages stored over its pages, the ratio), freeing ratio − 1. What it compresses is the
    inactive anonymous memory: the inactive queue less every file-backed page not speculative
    and the purgeable (the least it can be: the kernel doesn't split the queue), less the reusable
    counted at no cost, which frees 1 − 1/ratio of itself. Counted: the less of the two; none while the compressor
    holds nothing (no ratio measured) or past two fifths. The inactive queue isn't only other
    processes' idle memory: the kernel keeps it as long as the active one whatever is in use
    (20.8 GB each on the M4 on 12 Nov with one program writing to 25 GB all the while), so a
    job's own memory is counted there too (above, *Not all of it is handed out*). Read at 14:39 on 4 Nov, the pressure normal on each:
    the M4 6.8 GB at no cost and 3.5 by compressing (8.0 GB idle at 1.79), the Mac mini 6.1 and
    3.4 (5.1 idle at 3.0), the M1 3.0 and 1.3 (2.1 idle at 2.4); 3 GB of incompressible memory
    touched on the M4 then (in 512 MB steps, its jobs running) left the pressure normal and the
    compressor's share at 29 %. Shown as said: "10.3 GB available (3.5 GB by compressing)".
  - *Past two fifths*, compression spent, the available is what's left at no cost: chiefly the
    file cache macOS keeps rather than drops while it compresses. On the M4's 10 s series of 4 Nov
    (06:50 on, the hours the available was recorded), with the compressor at 40–48 % of the memory
    not wired, 5.3 GB at the 10th percentile and 7.5 at the median; at 48 % and past, 5.2 and 6.4;
    the M1's few such buckets 2.7–3.3 GB. So the floor gives back there only where it sits above
    that cache: the M4's default 6 GB less than half the time, its owner's 4 GB rarely (3.4 GB the
    least), the M1's 2 GB not once. Past two fifths it is the brake (the pressure at warning
    holding new work, swap grown past the floor's room freezing and giving back) that stands
    between the jobs and swap, not the floor.
- **Reading** (`memguard::Reading`, every 5 s on the sampler's thread, with the jobs' footprints):
  what's available and what compressing frees among it, what the jobs hold, the floor, the swap
  grown over the last five minutes (`SWAP_WINDOW`: its use now less the least in the window), the
  kernel's pressure level, and a give-back's cool-down. The jobs' **room**, what they may hold
  before the Mac is below its floor, is what they hold and what's available, less the swap grown
  and the floor (`Reading::room_mb`); the Mac is below its floor when what's available, the swap
  grown off, is under it, or the brake acts.
- **Admission, with reservations** (as the disk room reserves running jobs' space): the floor is
  the only memory setting. A job starts only where available − (each running job's expected peak
  less what it holds now, none once it holds more) − the new job's expected peak ≥ the floor. The
  Mac's ask says what its jobs may hold for one to start (`Ask::mem_mb`, its capacity's
  `build_mb`: the room, `Reading::admit_mb`, none while no job may start), what each running job
  holds (`Capacity::held_each`), and why no job may start whatever it needs
  (`Capacity::mem_held_back`: the brake, available under the floor, or a give-back's cool-down);
  the lead packs each running job at its expected peak or what it holds, the larger, and a new
  one at its expected peak (§7.2, `Sched::pack`), the same arithmetic for its own Mac and every
  member's, from their asks. A Mac held back is granted nothing, a cold start to an empty Mac
  among it, the refusal saying `memory: <why>`. Terrain-deep starts near 2 GB and grows past 7:
  its growth is reserved before the next job starts. A job's expected peak is the one figure the
  lead packed it at, carried by its grant (`Grant::peak_mb`: its targets' measure, `pack_need`; a
  cold start's provisional need; a task's predicted memory), which its agent counts it at in its
  capacity (`Agent::need_mb`, `Capacity::mem_reserved_mb`) and its cool-down; no agent has a
  figure of its own (task #970). A grant of an older lead's says none: its job counts at what it
  holds.
- **Give-back**: below the floor, at safe points, the work lost least first (§7.2, The memory
  guard).
- **Cool-down** (`Reading::cooling`, the agent's `cool_down`): a job the memory guard gives back
  (drained to its safe point, stopped, or yielding) holds new work off its Mac until what's
  available (the swap grown off) is back past the floor and that job's peak (what it held or its
  expected peak, the larger), or `memguard::COOL_WAIT` has passed, whichever comes first; a later
  give-back starts it again. The wait is the swap window, five minutes, so the swap a give-back's
  shortage grew has left the reading new work is let in by; it outlasts most outside swings (on
  the M4's 10 s ring, the 7.8 h of 4 Nov since the available was recorded, other processes' rises
  of 2 GB or more fell back within 60 s at the median, 190 s at the 90th percentile). A job given
  back because the Mac fell below its floor isn't followed at once by a smaller one the same fall
  would give back too; said as "a job was given back: no new job until 13.0 GB is available (the
  6.0 GB floor and its 7.0 GB peak; 10.0 GB now), or for 5 min more".
- **The brake** (`Reading::brake`), the safety net behind the count of compressible memory: the
  kernel's pressure at warning or critical, or swap grown in the window past what was available
  above the floor: no job starts whatever the number says. It acts on the running jobs
  (`Reading::acts`: frozen and given back, §7.2, The memory guard) only with evidence: the
  pressure critical, or the Mac below its floor by the numbers (what's available, the swap grown
  off, under the floor: swap grown past the floor's room is that). The pressure at warning alone
  holds new work and freezes nothing: macOS sits at warning for long stretches while it
  compresses, and a job frozen for it and given back each minute would lose work the Mac had room
  for. The build page shows a "memory brake" chip on the Mac's card while it's on, with why.
- **Replayed** (task #945): on the M4's night of 4 Nov (00:07–01:18, the 10 s ring) the compressor
  went from 11 % of the memory not wired to 40 % in two and a half minutes as four jobs started at
  once, and the pressure to warning 25 s later; of the 268 buckets at warning or critical, 4 had
  the compressor under two fifths (the turns), so compression counts nothing while a Mac is in
  trouble, and admission, give-back and the brake go by the available at no cost there
  (`memguard`'s `the_fourth_of_november_replayed`). A quiet 12 minutes on each Mac (14:40 on 4
  Nov, 5 s samples, the pressure normal throughout): every sample had room for a slope-deep job
  (2.2 GB) on each, 3.6 on average on the M4, 3.2 on the Mac mini, 1.6 on the M1, the running
  jobs' growth left out.
- **Status**: the Mac's card on the build page, the menu bar (under the state line) and `scenic
  status` (this Mac's, from the guard's sample) say the memory in a line (`memguard::words`):
  "Memory: 9.3 GB available (3.4 GB by compressing), floor 6.0 GB (default); the jobs hold 12.1
  GB, 3.0 GB more reserved for their growth; 1.2 GB for a new job", or why no new job, the brake
  first when it's on; the capacity table and `scenic status`'s Macs say each Mac's
  (`coord::mem_words`); a job given back says why (the guard's last word, on the Mac's live
  block).
- **Migration**: an agent from task #932 on finds the old `build-memory.json` (a cap on the jobs
  together, `{"mb", "by", "at"}`), removes it and says so in its log and as the guard's last word
  ("the build memory setting (the build's jobs held to 24.0 GB together (set by …)) is replaced by
  the memory floor: this Mac keeps its default floor of 6.0 GB available"); `scenic memory` does the
  same if it runs first. A cap has no floor it stands for (what the owner's apps hold isn't
  available memory: the M4's 24 GB cap read as a floor of 48 − 24 would have run nothing with 6.6 GB
  available), so the Mac keeps its default floor; a floor set already stays.

### 7.2 Placement: the steps table

`crate::agent::steps` has a row per step the agent runs as a job (docs/pipeline.md: its id, title,
what it does, kind, scope and stage, and these resources); the step sets (`SHARED`, `ANSWERED`) are
generated from it at build time. It holds no memory figure: a step's memory is measured (§7.7, task
#979).

| Column | Meaning |
| --- | --- |
| shared | whether members take it (no rank: which ready job goes first is derived from the reads, §7.2) |
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
as data (`sched::State`, `Coordinator::sched_state`, written with the forecast's input): pulled as
live (task #951), a Mac asks only when one of its lanes frees, once, for a job each of its lanes
free then (`Ask::want`), offered the work ready in the plan's order; jobs end at their modelled
times and give their leases back, and a job of a step never measured leaves its measures as its
hand-off would. What a target waits for is the plan's: its declared reads (`steps::read_targets`)
and, where an input of it can't be worked out yet, the target that works it out
(`seam::Left::unknown_for`, `forecast::Input::unknown`: a slope tile reading the terrain's root
waits for the root while it's stale, the root a job of the lead's in the run). So the schedule
packs as the coordinator packs: by memory, disk, cores, the NAS budget, cold starts, the lead-only
steps and the mixed-version hold. Guards: `tests/grant_rules.rs` fails on a grant rule named
outside the scheduler; `fed_a_recorded_state_the_forecast_grants_what_the_coordinator_grants`
holds the forecast's first grants to the coordinator's; at run time the lead logs where its real
grants since the last forecast departed from that forecast's first hour (`forecast::departures`,
"the grants departed from the forecast"; a Mac granted fewer, with why: below, Each Mac at its
sustained best). What stays outside the scheduler moves the finish: the
times and NAS weights are measured estimates, and the repack, the owner's pauses, the lead
asleep and a NAS slower than its probe are not foreseen.

**Each Mac at its sustained best** (task #951): the run takes each Mac's room from now on to be
its quietest sustained state over the last 48 hours: the highest value held continuously for 30
minutes (the highest 30-minute rolling minimum, `forecast::sustained`; 30 minutes and 48 hours are
the owner's choices, 4 Nov) of its memory for jobs (its memory less what the rest of the Mac used
and its memory floor, each minute's most) and of its cores not used outside the build (its cores
less the rest's, each minute's mean, within those the build may use), from its live store's minute
ring (`live::Store::best`, worked out at most once a minute). Each Mac's ask carries it
(`Capacity::best`), the lead's own too, so the forecast's input holds it; the scheduler never reads
it, and the live coordinator packs by each ask's room now. In the run a Mac with one has that
memory and those cores, no brake, no owner's use; one without (not 30 minutes sampled yet, an
older app) has its room as it last asked. The NAS budget is taken as recorded: its limits the best
of the day's probes (§7.7), and the NAS's own gate (`Limits::held`, a reading of now) as recorded while
the jobs out then run, open after them. A job, under way or granted in the run, weighs its targets'
measured rates throughout (no beats to weigh it by, where the live scheduler weighs one past its predicted
time by them, §7.7). No use of the owner's or passing load is foreseen: the build page says
the finish is a best case ("each Mac at its quietest sustained state over the last 48 h"). Its
**live error** (`forecast::pace`, `Forecast::pace`): the oldest
forecast of the last 1–6 h (the lead keeps their runs' progress, `Forecast::curve`, a point each
10 min) against the planned targets the history shows done since, the build page's Details tab
(its Forecast), marked rough ("live error (rough, ±50 %): the forecast of 14:05 did the 40 targets done
since in 1 h, not 3 h"). It is said, not applied.

A Mac without a sustained best is packed into its cores less those its last ask showed busy
outside the pool, as the coordinator counts them (`coord::outside_cores`, task #684), for the
whole run. A running job counts at its lease's
threads as re-granted now (§7.1, Threads re-granted); its later re-grants aren't foreseen either.
The forecast models no target's time by its threads, a grant's included (a target takes its
measured runs' time, whatever threads they had), so a re-grant moves only
the room it packs beside the job, as the next forecast (within the minute) sees it. The
lead logs a departure only when a Mac's jobs of a step differ from the forecast's by 2 or more and
by more than half, each Mac and step at most once an hour, and never of a step the forecast
doesn't model (`forecast::UNFORESEEN`, docs/building.md, The forecast: the planet's fetch, a new
pass, the backup, the redos, GC), whose grants depart from no forecast. Of those it logs a Mac
granted more than `forecast::UNFORESEEN_HOURLY` jobs of one in the last hour (30: GC goes on through
its chunks in one job, the backup is daily), each Mac and step at most once an hour ("many grants
of a step the forecast doesn't model: m6: 558 job(s) of gc granted in the last hour, more than 30").

**A Mac granted fewer** (task #1163) is said with what the lead answered its asks since the
forecast: the coordinator keeps each worker's asks answered over the last hour
(`Coordinator::answers`, `coord::Answer`: when, the jobs wanted and granted, the steps offered,
and why fewer: the packing's rule, `sched::Held`'s `sched::Rule`, or the ask refused before any
packing: the lead moving, the build paused, another app). The line says, after the counts: no
ask from it since (its agent asks only while a slot is free and nothing holds it back: its jobs
running past their measures, an update's restart, its memory guard, a stalled loop); else its
asks, the jobs they wanted and got and how many were offered the step (none: the lead's plan
hadn't listed it yet, where the run offers a target as soon as what it waits on ends), and each
rule that held them back, counted, with the last one's words. The rules deciding by live state
the forecast can't know say so (`Rule::live`): the memory guard (macOS's memory now) and cores
busy outside the pool, where the run takes each Mac at its sustained best, and the NAS budget,
whose gate on the NAS's own load is a reading of now the run holds only while the jobs out run; a
refusal before packing is the lead's or the agent's state now. Each ask granted all it wanted
says its slots freed later than the run's lanes. The threads' re-grant (§7.1) reads the rule
too: an ask held back by cores, cores busy outside the pool or the load's minute was short of
cores (`Rule::short_of_cores`). For example: `agent: the
grants departed from the forecast: m4: 0 job(s) of unit granted, the forecast's 3; its 3 ask(s)
since wanted 5 job(s) and got 0, 2 offered unit; held back by memory ×2 (live: macOS's memory
now; the forecast takes each Mac at its sustained best), cores ×1; the last: cores: 0.5 of 8
left, a job of unit takes 1.0`.

Rows of note: `trees`/`terrain-hi`/`slope-hi` per z6 tile with `*-lo` assemblies per z3 tile,
`terrain-deep`/`slope-deep` per z10 tile (docs/steps.md);
`bldprep`/`bldtiles` per z6 tile, `bld-fetch` the lead's (docs/buildings3d.md §3.6); `inputs`, a
target per z6 tile, the lead's, planned before every other job (docs/inputs.md §4.3).

- **Every step to every member** (`crate::agent::every`, `crate::coord`'s `pick_every`): all but the
  lead's own steps (`every::LEAD_ONLY`: map updates, prunes, `bld-fetch`, the answer-keeping steps, the
  worldwide steps) go to any member whose measured room fits, the lead among them. A cold start
  (§7.7) goes, one target, to the first asker its provisional need fits (or one holding no job),
  while no job of its step runs anywhere.
- **The lead's own steps by grant too** (`Agent::offer_plan`, `coord`'s `pick_own`): the lead
  offers its whole plan in its order (the map update's chain among it), each offered step's targets
  where its work first comes and each of its own planned jobs whole (`Offer::job`). The authority
  rule (`every::grants`): a step kept to the lead, and every planned job of the lead's, is granted
  only to the current lead's member, asking in process; a member's ask over HTTP never gets one.
  One job of such a step runs at a time; its measures (memory, disk), the NAS budget (always, §7.7)
  and a cancel's hold hold it as they hold a member's job. Its cold start (none of it measured)
  goes by its provisional need (§7.7) like any other: beside the lead's jobs when it can; when it
  can't, the lead is granted nothing more until its jobs end, then that job before any other
  (`own_cold_waits`), so offered work never starves it. A
  planned job that can't start for a reason of the agent's own (a failure's wait, the proxy not
  up, the memory guard's hour: `wait_reason`) isn't asked for, nor one of no targets of its own
  (GC, the backup, the planet's fetch, the OSM pass: `kept_after_end`) that ended (of itself,
  failed or at a safe point) after the plan's inputs were taken (`Agent::own_ended`, against
  `PlanView::from`, the number of the agent's side the plan was made from: one counter, no clock,
  so a plan begun just after the end from inputs taken before it counts as before it). That plan
  was made from what came before the run, and a plan comes no sooner after the last than that one
  took, so a job of moments would be granted at every round until the next plan's offers (task
  #1392: GC granted 192 times in the 100 s after a run that removed 1,427 files, each run removing
  none; the backup up to 206 times a day); the next plan says whether there is more for it. A job
  with targets of its own (a map update, a redo, a gate check) isn't held so: the coordinator's
  done work keeps it while a plan offers it at the key it was done under, and one at another key
  (a map update planned while the last ran) goes at once. A job stopped otherwise (the Mac asleep,
  its lease lapsed, the agent stopping) isn't held. The ends are kept in memory: a restarted lead
  grants none of its own jobs before its first plan. One that
  fails (or the memory guard
  stops) ends its lease failed (`finish_failed`): its targets are kept from the lead an hour,
  doubling each time, as a member's failed work is.
- **Packing** (`coord`'s `pack`): a Mac may hold several leases. An ask says how many jobs it would
  start (`want`); while every member reports the new app (§7.1, Mixed versions; else one each) the
  coordinator grants up to that many in one answer (`Grant::more`, each its own
  lease), packed in plan order into the ask's room: each after the first the Mac holds must fit,
  by its measures (memory, disk, and cores: `needs::Book::cores`, §7.1;
  cores not said count as one thread) or a cold start's provisional need (§7.7), within
  what's left: the ask's memory and disk (what its jobs may hold above its memory floor,
  §7.1, `Agent::build_room`, less each job it holds at its expected peak or what it holds, the
  larger; and its free disk) and its Mac's cores (less the in-use share's
  while its owner uses it, and less the cores busy outside the pool: those its last minute's CPU
  keeps busy (`Capacity::cores_free`, in each ask's capacity, §7.1) past the cores its jobs count
  for (§7.1, Threads granted), an excess of one core counting none (the minute's cores are
  rounded up; a job just ended counts until the minute shows it gone, a minute at most), and at most half the cores it has
  for jobs (`coord::ask_cores`: less the in-use share; `coord::outside_cores`; task #440: a Mac swamped by other work, builds and tests, gets no job
  beside those it holds past the other half, and its refusal says the cores busy outside the
  pool; task #684: the cores busy can't tell a niced test build (`tools/app/remote-tests.sh`
  runs at nice 20 on the Mac mini, the lead) from work that competes, and on 26 Oct one held the
  lead's load at 47 of 12 cores, its free cores none, for two hours; the owner's own use keeps
  the in-use share besides), less the measured needs of every job it holds and of those granted
  with it. A lease counts until it ends or lapses: a restarted Mac's old leases hold its room
  until their TTL runs out. The room left is never more than what the Mac's jobs hold now leaves
  (the ask's memory less `Capacity::held_mb`, its memory guard's last sample): a cold start holding
  past its provisional need counts at what it holds. **Room held for a big job** (`Sched::pack`,
  task #965): the first offer in plan order refused for its room alone (memory, disk or cores),
  one that would fit the Mac once its jobs end, has that room held, so small jobs later in the
  plan filling each slot as it opens don't starve it. While it waits, a later job still goes
  (backfilling, `Backfill`, task #973) when it fits beside it once the jobs it waits for end (the
  Mac's jobs expected to end soonest by their measured times (`Sched::batch_secs`, less what
  they've run), as few as free its room, any past its time or not measured first; every other job
  still running), or when, its time measured, it ends by its runs' 90th percentile
  (`needs::Book::secs_p90`) before those are expected to end; none of the second kind while one
  of them is past its time or not measured. So a hold never idles a Mac for as long as its
  running jobs last (on 5 Nov the mini ran one terrain-hi job, 7 of its 12 cores idle, while room
  was held for timetables-todo), never waits for a long job while a shorter one frees its room,
  and the big job waits only for jobs expected to end no later than those it first waited for,
  until one overruns: never starved. The refusal
  names the target it waits for (its step and tile), the jobs it waits for and when they're
  expected to end, also when nothing offered after it was held back (task #1170: that case had
  said only the room left, and the lead's status showed no hold). The forecast packs by the same
  rule, its leases run as long as the record says (`Leased::granted_at`: its grant, or its job's start when that's before, `Lease::began`). With placement on, a member holding a job whose cores, as its last ask
  showed, are filled by load from outside the pool with its jobs' (`Shared::swamped`; its own
  jobs alone filling them don't count) isn't among those a target is drawn to: the others take it. Each job is as many targets as fit the batch length by their measured
  times on its Mac (§7.7, Batch length; a target of no run of its own nor source: its step's mean, the latest earlier way's when larger while the way it
  runs now has fewer than `needs::PRIOR_RUNS` runs, those being the quick ones first to end), a step kept to the lead in one job. The agent starts every job granted, each in a slot of
  its own; the slots past the first two are made as jobs are granted and dropped once empty. Each
  slot has its own files in the agent's folder (`job-<k>.json`, `control-<k>`, `costs-<k>.jsonl`,
  `done-<k>.txt`, …, the first slot's without a number) and scratch folder (`scratch-<k>/`), and
  its own name as a worker (`<host> #<k>`). Each job runs with the threads granted it (§7.1,
  Threads granted).
- **Order and locality:** a Mac gets a contiguous run of plan targets, from the front or just
  after its last target, as many as fit the batch length (§7.7) of measured time. Every
  ask, the lead's own and a member's, and the forecast's, tries the offers in plan order (task
  #909: the members' fixed step ranking is gone). The plan lists each region's work, then the ready
  work in it of the steps reading that work (its slope), each moved ahead of the steps it reads,
  directly or not (`steps::readers_first`, `steps::reads_from`): finishing what's started, so a
  ready slope-deep job no longer waits while any terrain-deep job fits. Steps the reads don't
  relate keep the plan's order (the regions' order, a done region's slope and tree cover first).
  No step's rank is kept by hand: what's upstream needs none, since what reads it isn't offered until it's built
  (`steps::readiness`), and a hand-set "terrain before slope" only starved ready slope work into
  the gaps. A step another reads several targets of for one of its own (`steps::read_together`,
  probed from the reads: terrain-deep's 3×3 for a slope-deep tile, an assembly's z6 tiles) has
  its targets planned in Z-order (`steps::block_order`, `build::blocks`) within each region, so a
  run is a square and what reads it is ready a neighbourhood at a time, not once a row is done.
  Measured on the real state of 31 Oct (3,705 terrain-deep and 8,082 slope-deep z10 tiles left,
  10 lanes, the lead's forecast's pace, a model of `pick_every`; the 24 hours before, the pool
  made 1,605 terrain-deep tiles and 244 slope-deep): slope-deep's wait once ready went
  from a median of 10.2 h (p90 15.2 h) to 0.1 h (p90 2.8 h), a tile's lag from its terrain-deep
  to its slope-deep from 9.0 h to 0.1 h, the whole from 16.0 h to 16.2 h (the last terrain job's
  slope after it).
- **Placement by how crucial** (`crate::agent::placement`, the pool's setting `placement`, §12): a
  member's asks are tried on steps by their forecast slack (least first, plan order between
  equals; `Sched::pack`, so the forecast's run tries its members' offers in the same order, task
  #1163; the lead's own asks in plan order); within a step, the targets in the asker's
  last one's z3 tile first (`placement::group`, from just after it in plan order: its jobs read
  the same neighbourhood's inputs, task #552), then from the longest. Each target is drawn among
  the members asking lately that would take it now (`Sched::would_take_now`: by their last ask,
  a slot free and their free memory fitting it; a busy Mac is never drawn a target it would leave
  untaken), the lead's own agent among them (`placement::draw`, stable per minute window); an
  asker passed by goes on down the offer to the next target drawn to it. A worker's weight
  (`Sched::draw_weight`) is its Mac's speed at the step against the lead's, the inverse of its
  pace (§7.7, Each Mac's pace; task #934: the coordinator's one measure, from the same targets on
  both Macs: at the step, else over its steps, else the lead's); times `CACHE_PREFER` (4) where its
  ask's capacity says it holds the step's shared cache (`needs::CACHES`: the whole copies) whole. Trade-offs: weights only, so a Mac asking alone takes what it can and the
  weights act only where Macs ask together (no Mac idles for a draw); a
  minute's window keeps a target drawn to one Mac until the next window. The forecast runs the
  same draws (the placement is in the scheduler's state it's given), its lanes asking as its
  model has them. A task offered
  ahead goes to a slower worker only while its head start covers that worker's run. The lead's own
  planned jobs keep plan order.
- **Resuming** (`crate::agent::resume`, in the pool): a job interrupted with progress
  on a Mac's disk (built for the OSM pass) is offered to that Mac alone for 24 hours from its first
  start, or until `scenic pool release <member> "<job>"` (kept by the lead). Progress is kept on
  its member's own disk (`progress.json` in its agent's folder) and carried whole in each of its
  beats; the lead keeps each member's in its store (`progress/<member>`, docs/coordination.md §3.1). A finished one is held unoffered until merged. A long target resumes at the block after the last it finished when it runs on
  that Mac again, not offered back (crate::resume_log): a depth target (`depth`, `depth-lo`: its
  composing's blocks, crate::depth::resume), a terrain-hi z6 tile (each level's blocks, a z8
  tile's tiles at z12, then the level's end) and a terrain-deep z10 tile (its levels' blocks
  likewise, z16 up to z13; crate::terrain_resume's `HiLog`). slope-hi and slope-deep keep none: their
  targets take minutes at most. A drain also stops these steps mid-target, at **one shared safe
  point** (task #885): each block a resume log keeps is one (`resume_log::Log::keep`
  calls `control::piece_saved`: the block's record written, nothing of the next begun; a log that
  can't be written makes none, as its work would be lost), and the terrain's sampling of the
  elevation pool checks before each chunk (16 tiles) or tile read alone (terrain-hi, -deep, -lo,
  -root; a level's sampling ran 7.7 h on 3/7/3, task #705). Both check `SCENIC_CONTROL`
  (`control::draining_lately`, read at most once a second; the channel's say a line, the file
  replaced whole by the agent, `control::tell`, so a read never sees part of one, task #957) and end with `control::Paused`; the
  step (depth and depth-lo too) catches it once its uploads in flight have landed and exits as
  paused, the target unsaved and not noted done, its log kept (a level's sampling not yet in a
  block is redone). pyramids-fill stops before each tile it would ask (the fetches under way finish),
  what it fetched packed and handed off, the z6 tile not done; its next fill fetches the rest
  (the same tiles' bytes; its store's archives split where it stopped).
  Steps whose target is a run of independent pieces keep each as it's made (`resume_log::Pieces`,
  task #889; the identity, as the logs above: the step, its declared output version(s), the target
  and the manifest's names of what it reads, never the program, so a pause to restart into a newly
  installed app resumes its pieces; a change to the output bumps the step's version; a resumed
  run reads each kept piece from the log as it takes it, task #1182, never all at once), each a safe
  point: bldtiles each z8 tile's results (made here or by a worker; a resumed run offers only the
  rest), bldprep each chunk of 4 row groups read, bldfield-lo each z6 tile's levels, trees each
  row of a z6 tile's blocks (kept by the trees program, `--keep`, which exits paused, 75, its safe points counted in the
  job's), peaks
  the summits a z6 tile's peaks see and then each 512 of its peaks measured, polar-terrain each
  chunk of 16 tiles sampled at every zoom it samples (its finest, and the coarser ones past the
  cap), polar-root each such chunk of a pole's z0–3 (`polar::sample_pieces`), polar-slope each 64
  tiles shaded there, slope-lo each row of a z3 tile's z6 tiles shaded from their terrain, lo each
  16 tiles of a zoom of a z3 tile's road and rail tiles (z4–8; its z4 and z5 a piece each; the
  identity names the 16), reach each z6 tile's reach (task #890), water
  each z5 tile's subtree (z5–9, drawn side by side, the pieces locked only to take and keep them:
  `Pieces::take`, `Pieces::keep`), regions-extract (task #897) each 256 MiB range of the file in
  each of its three scans (`extract::RANGE_BYTES`: the blobs whose headers start there, their
  finds; a resumed run passes a kept range by its blobs' headers alone), names-todo each 16
  hidata files' roads placed (`namestodo::roads_in`, replayed in order on resume; a unit's ways
  read once a record names it), and the regions' input check each region without water resolved
  and each region with water's land and water (`inputs::regions::kept`, by the resolution's keys:
  a digest of everything it reads; a pause met while regions resolve side by side gives the rest
  up, `Kept::stopped`). trees-lo keeps none: one assembly of its z6 tiles' mids. A paused target
  is not put, nor noted done.
  Steps whose records are their pieces need no log (task #890): pass-sets stops after each set
  saved (a resumed run makes only the sets the records lack) and each 1 % of its copy of the
  filtered planet (appended to on resume); heritage after its clip of the World Heritage sources
  (held in the cache by set and cover), after each script of its chain (what it fetched kept
  in the pass's answers), and within heritagewd and areadetails after each Wikidata batch kept
  there (task #999), a resumed run redoing the
  minutes of clips before it and running the scripts done on their answers. A pause any step's piece returns that the step doesn't stop on
  itself ends its job as paused (scenic-build's `main`), so every safe point stops the job.
  A `unit` z6 tile's pieces are its preparing (the piece copied, the buildings staged,
  `prepare_folder`) and each program of its tail run here, each kept with its folders' digest
  (`unit::folder_digest`: names, lengths, times written), the folders kept beside the log: a
  resumed run takes them only while its folders are as the last piece left them, else starts the
  tile afresh; a pause there ends the job as a pause before a tile does (its tails out with
  workers settled first). Its longest gap is its longest program (its preparing, or `elev`); a
  tail out with a worker keeps none. Resumed, the programs its last run kept are taken as kept,
  not offered to a worker again (`unit::split_kept`). The folder pieces are `Pieces::folder_piece` (the digest
  `resume_log::folder_digest`), whose last kept `Pieces::folders_as_kept` checks. A `pois` z6 tile keeps its extract's candidates (one
  program, no safe point inside it). Every job's timings record
  says how often it reached a safe point and the longest it went without one (`safe_points`,
  `safe_gap_s`, `control::safe_gaps`; its log's `timings: … safe points` line): each step's time
  between safe points, measured on every run. **The bound** (task #890): one declared value,
  `control::SAFE_GAP_BOUND_S`, derived as a third of the drain's grace (`DRAIN_GRACE`, 15
  minutes: 300 s), so a job asked to stop reaches a safe point before it's
  frozen, on the slowest Mac too. A run past it says so in its log (`timings: over the bound: …`,
  `control::over_bound`), and the build page marks a step whose last runs did: the gap after its
  count on the Jobs tab, its card a "Safe points" line (the lead's phase shares'
  `over_bound`: how many of its last runs, the longest gap, from timings.jsonl's `safe_gap_s`).
  Steps still past it by design: heritage's World Heritage clip and pass-sets' sets (one osmium
  run each), unit's longest program, and the regions' check's stages after the waters (the
  lakes, the one coastline, each region's area, the hand-outs: one stretch over every region
  with water, no pieces). catalog keeps none. names-todo's labels' phase is still one stretch, past the bound on the slower Macs (288–364 s measured in the M4's last runs; the landmarks' 222 s). A log at the top of its step's scratch folder
  (`<z>_<x>_<y>.resume`, one append a block, each record with its length and BLAKE3, read a
  record at a time; a slot's moved to the slot running it), taken only for the same inputs (its
  step's versions and encodings, the target, the sources' and water's pins, and what else it
  reads: the terrain's tiles made here, the fine datasets' lines), so its
  outputs are the same bytes. Counted in the job's disk as its sampler counts its scratch's top.
  Deleted when its packs are written, on a cancel, with a given-back job's scratch by
  room-making, and, untouched for 24 hours, as the step's next run on that Mac starts.
- **The forecast runs off the loop** (task #544): the lead makes its input on the loop and runs it
  on a thread of its own (`Agent::start_forecast`); the status keeps the last one done, so reaping,
  starting jobs and writing the status never wait on it. Its three runs go side by side (three
  threads, each at the utility QoS: `forecast::low_priority`), each cut once it has computed
  `forecast::BUDGET` (15 s of its thread's CPU time, not wall time, #584: a loaded Mac starves
  utility threads, 4 s of CPU took 15 s on the mini at load 34; a run of 7 Nov's worldwide queue,
  some 19,000 items on 30 lanes, computes 5-12 s on the M4 under load: §7.9, Where the forecast's
  time goes), or
  after 120 s of wall time (a backstop, so a starved run can't hold the next forecast or the NAS
  self-heal); the trade-off: the three runs may take up to 3 × 15 s of CPU together: one
  cut is `partial`, its `why` says so, no finish shown, and the build page marks its lanes and next
  jobs partial. On 21 Oct's live input (4,155 targets, 20 lanes) it took 3.7 s (205 s before #544).

**A job waiting on a worker** (`crate::offload`'s `WAITING_ENV`): a z6 tile job waiting on a faster
worker for its tail writes `SCENIC_WAITING`; while it exists, on the lead and on a member alike,
its agent names the job's lease in its capacity with what it holds now (`Capacity::waiting`,
task #462), and the lead packs that lease at that memory (never above its measure) and none of
its cores (`Sched::pack`), within the Mac's room as ever; its disk
stays held. Only the asking Mac's own leases count so. A job started beside a waiting one
**yields** to it (stopped at once, given back, not counted as a failure, no floor learned) when it
resumes and its Mac is below its memory floor (its Mac's memory guard). A restarted agent no longer
holds the job's slot, so it names nothing: the lease counts by its measures until it lapses.

**A job leaving the cores idle** (task #665): a running job that has used under a core-minute
of CPU for `agent::IDLE_CPU_S` (10 minutes; its sampler's `busy_at`), waiting on the network,
not on a worker, is named in its Mac's capacity (`Capacity::idle`), and the lead packs its lease
at its measures (memory and disk as ever) but none of its cores, so CPU work goes beside it
within the Mac's slots (`job_slots`) and room. Only the asking Mac's own leases count so. A job
that uses its cores again is not stopped and nothing yields: it counts its cores again from its
Mac's next ask once it has used a core-minute, and the Mac is granted nothing more until they fit.

**The memory guard** (`crate::agent::memguard`; always on):

- A thread samples each job's processes' footprint every 5 s, per target under way (from
  `started` lines in `SCENIC_COSTS`, else `SCENIC_DONE`), and writes `memory.json` for the status.
- Every child started through `store::spawn` (the agent's jobs, their programs) is started without
  macOS malloc's cache of freed large blocks (`MallocLargeCache=0`, an undocumented libmalloc
  setting, `store::spawn::NO_LARGE_CACHE`; their children inherit it): with it, freed blocks stay in a
  process's footprint, and a dense z6 tile's scenic-build held 6–7 GB of them while its programs ran.
  The agent, started by launchd with it, starts itself again in its place without it as it starts
  (`store::spawn::without_large_cache`): the lead's held 1.5 GB of freed blocks after 18 h (3 Nov,
  #930), and macOS 27's `malloc_zone_pressure_relief` gives none back. So does the map's server
  (#1092: on the mini's map of 10 Nov, 146 MB after its start and a few requests with it off, 781 MB
  on).
  The cost is speed: each block over 32 KB is mapped afresh, ~3 µs a block and ~50 ms a GB
  allocated (a C loop of malloc, touch, free on the M4, against ~1 ms a GB with the cache).
- **Below the floor** (§7.1, The memory floor: the jobs together past their room, what they hold
  and what's available less the swap grown and the floor): with the largest fitting alone, another
  job stops at its next safe point (`memguard::decide_by`): one that yields first, else the newest
  (by its start's minute), then the least progressed (the work lost least), never the largest; one
  at a time, and none more while the jobs not draining fit the room. Nothing new starts beside the
  largest meanwhile (a member asks for nothing). The largest alone past the room stops at its next
  safe point too. While the brake acts (§7.1: critical, or on with the Mac below its floor by the
  numbers) no safe point is waited for: the job that would drain is
  frozen at once (SIGSTOP; the largest too once it alone passes the room, the sampler's thread
  freezing it first), then the next while the rest still pass the room. Freezing is the last
  resort: it stops a job's growth and lets macOS compress it while its work is kept. A frozen job
  goes on once the Mac is back (the brake off, available at or above the floor), and is
  stopped (given back) after a minute frozen with the brake still acting (`PRESSURE_HOLD`: twelve of
  the sampler's 5 s readings, enough for a freeze that stopped the fall to show in them, short
  beside the five minutes the swap grown counts over), or after the 15 minutes a drain gets with
  the Mac between the two (the brake no longer acting, not yet back): that band is the freeze's
  hysteresis, so a job isn't let go and frozen again on each sample.
  A freeze outlives its agent's restart (docs/building.md §2, Restarts: a frozen job), held by the
  next agent under the same rule, the disk guard's too (task #1137).
  This holds on every Mac's slots. A drain that finds no safe point in 15 minutes is stopped. A
  stopped job is given back and kept from that Mac for an hour. A job holding more than its
  expected peak is not stopped for that alone: only the floor gives a job back.
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
  floor exceeds the most this Mac's jobs could hold (its memory less its memory floor) goes to a Mac
  with room; past every Mac's, it waits; a lead-only step past the lead's waits, the status saying
  to see to the lead's memory floor or the step's. `scenic pool floors [--clear
  [<step> [<target>]]]` lists or clears them: the clear is a request to the running lead's coordinator
  (`/api/floors`), which takes them out of its memory, its files and its next state at once (no
  restart), and the command says how many floors and disk floors it cleared. A floor learned before a
  step's fix stops counting once the step's `cost_version` is raised for the fix (`bld-fetch` 1, task #557).

### 7.3 Results: hand-offs

- **A job's hand-off goes to the lead by HTTP** (`crate::coord::handoffs`, docs/coordination.md
  §5.3): its member's driver keeps it whole in its saved state
  (`journal::Mine`, keyed `<day>/<epoch>-<n>`: its job id, the UTC day of first try); the
  member's beat lists the keys it keeps (every beat, whatever the lead last answered: the lead's
  GC queue takes the list as every lease the member may still hand off under), the lead's answer
  says which it lacks (posted next,
  `POST /pool/handoff`, into the lead's store before its 200) and which a saved records snapshot
  names (dropped). The lead's agent merges what its coordinator took from memory, with its own
  jobs' hand-offs (never written anywhere first). Nothing goes to the NAS; the hand-offs an older
  app's saved state lists as written to the NAS's old journal (`state/journal/`) and
  unacknowledged are read back whole once and posted (`Mine::read_back`, under a new member id
  too). A job's folder is removed only once the saved state holding its hand-off is durable, its
  rename flushed (the hand-off's only other copy). A lead says `handoff` in every beat's answer,
  so a member on an older app never falls back to that journal.
- **The lead validates at merge** (`agent::poolside::check`, `records::Check`): every change within the
  step's write-set, content names matching logical names, the lease's targets. Write-sets are one
  function per step (`agent::steps::writes`) that both the job's `Out` (`Out::open` for its step)
  and the merge use; `Out` refuses a name outside before it lands. An entry outside its write-set
  from a member on a newer app is **held** (`check_from`, `records::HOLD`) until the lead runs that
  app; otherwise it's refused (with why, in the records' `rejected`). So is one GC removed a name
  of under its lease (`agent::gcqueue::refusal`), and a redo not the lead's. The lint
  `tests/write_sets.rs` gates publish.
- **Work the merge doesn't take is done again:** the coordinator holds a job's targets done at
  their keys from the hand-off's acceptance (`Sched::done`, docs/coordinator.md: never granted
  again while a plan made before the record was read offers them at those keys); the merge's result names the
  targets of the entries it refused, or passed over (below) while the records hold them under
  other keys (`records::Merged::again`, weighed by `Keys::holds` once the whole merge is through),
  and the lead's coordinator lets go of exactly those (step, target) entries at those keys
  (`Coordinator::again`), so the next ask grants them again; the status's "done (its record not
  read yet)" and the forecast's work to come follow. A held entry keeps them done (its record
  is read later). Each refusal is in the history ("refused", on the build page as a problem,
  with why) and the log ("merged 0 entries (0 passed over, 1 refused; 1 target to do again)").
  Nothing stops a job whose hand-off is refused every time from coming round again: each round
  shows.
- **A redo is a hand-off too:** `scenic redo` (§11) clears targets' keys through a job of the
  lead's whose entry carries only a done record, merged only by the lead whose member it is
  (`agent::poolside::by_its_lead`).
- **Sizes ride along:** `Out` records each file it writes with its size, and the hand-off carries
  them (`Handoff::sizes`); the merge keeps those of the files the entry names, so the `catalog` step
  never looks at them on the NAS. Entries from before carry none.
- **Lease order holds:** an older lease's entry for targets a newer one set changes nothing.
- **Nothing is lost between the lead's save and its member's drop:** a hand-off is acknowledged
  only once a snapshot naming it is saved (and the lead's driver's state saying which snapshot it
  saved last, `Saved::records`, is on its disk), and a take-up never starts from records older than
  those (`driver::take_up`, `records::Older`: a stale read of the newest waits, tried again next
  step).
- **A removed step's names leave the records the same way** (task #701): the `gc` job's daily
  duties remove from the manifest every name under `agent::gc::RETIRED` (a removed step's outputs'
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

In the pool, a pass is complete when its records name `sources/osm/<date>/pass`
(`crate::osmpass::latest_pass_by`); out of it, a `pass.*.json` file still says it. A map update runs only
when the lead is caught up. A resuming job is given its member's unacknowledged hand-offs of its
step (`SCENIC_UNMERGED`), which `Out::open` lays over the snapshot.

### 7.5 The coordinator across restarts

- **Its state** is in the pool's store on the lead's own disk (`crate::pool::store`,
  `crate::coord::kept`; docs/coordination.md §4): each change (a grant, a finish, a
  lapse, a measure, the pause) in the store's log, `fsync`ed by its writer thread before the
  request that made it is answered (a grant is never answered before it's durable); a restart, an
  update's or a crash's, opens the store, its leases back unconfirmed (test
  `agent::poolside::tests::the_leads_restart_keeps_its_leases`). Renewals aren't saved (a loaded lease
  gets a fresh deadline). A lapsed lease stays in the table (a member's word may hold it again)
  until its target is granted again, which ends it (`Leases::grant_at`; task #1038: kept, a restart
  gave it a fresh deadline beside the new lease).
- **Job ids** (task #1103; jobrt::jobid, docs/formats.md, Job ids): each job run is named by the
  lease it was first granted under, `<epoch>-<n>` (`Lease::job`): what its spans are kept by on the
  lead (task #1104, docs/formats.md, Job spans; `GET /api/job/<id>`), and what a job's page on the
  build page keys a run by (`job/<id>`, task #1105). Never given twice: each start of the lead's
  store leads an epoch past every earlier one's (§4, invariant 2), and within an
  epoch the store's lease numbers only go up, from past the time in milliseconds, its saved next,
  every lease it takes up and the highest number its history names (`Leases::load`, `restore`,
  `past`), so a lead whose state is lost and whose clock was set back gives none again (test
  `coord::tests::a_restart_with_its_state_lost_gives_no_number_its_history_names`); the epochs
  tell numbers that meet apart (`coord::lease::tests::a_job_id_is_never_given_twice_across_a_restart_or_a_lost_state`),
  and the simulator checks every coordinator's grants of a run, across restarts (§13). A job its agent's successor takes over keeps its id, and so does one whose lease is held
  again under a new number (its own lapsed while it ran on, or ended with a lead's restart:
  `Coordinator::hold_for`, the lease's `job`; test
  `agent::tests::a_job_survives_its_agents_restart_taken_over_by_the_next`); a paused job resumed,
  a failed one tried again, is a new run under a new lease. The id goes with the run into every
  record of it (docs/formats.md, Job ids), and into its job's environment (`SCENIC_JOB`). A record
  from before it names its run by the lease's number alone, read as `0-<n>`.
- Workers are a member id and an agent instance; a take-up drops only instances that no longer
  exist.
- **What a job waited on** (task #791): the lead's coordinator writes on each "start" and "lease"
  event `after`, the numbers of the "end"/"done" events of the jobs it waited on: for each step
  its targets wait on in the forecast's dependencies (`forecast::Forecast::graph`, handed to the
  history after each forecast, `history::Waits`, worked out on the forecast's thread; what an
  earlier forecast's waited on and finished since is kept), the one of that step that finished
  last. Only jobs that finished after the forecast before the one naming the item are named
  (after the history's load for the first): a job from before a restart or a
  `scenic redo` is never named, so a start then names fewer rather than an older job. A few
  numbers an event (one per step waited on, ~10 bytes each: some 2 to 6 % on the lead's
  history.jsonl, ~8,600 starts and leases in its 5.2 MB on 27 Oct); events before it have none,
  and readers take none as nothing known. The build page's schedule draws its arrows from it.
- **The history** is the lead's coordinator's file on its disk (`<agent>/coord/history.jsonl`, the
  last `history::DAYS` (7) days', its coordinator's start among them: its "agent" event, how
  `history::mend_runs` knows a restart; a member's notes among them, its start, its conditions,
  its caches and its broker's tasks, carried by its beats: docs/coordination.md decision 4), held
  in its memory for the page (§10, The page's history); no copy on the NAS (task #1119). Each day before today is packed as one tar,
  `state/coord/history-archive/<day>.tar` (member `<day>/<member>.jsonl`, the declared lead's), by
  the `gc` job's daily duties (`--history`), so the lead alone, outside its loop and the pool's lock
  (`agent::histarchive`, task #531): the day's lines merged into what its tar holds (a line there
  already not again), so a day finished is on the NAS by the next run, and a lead's disk lost loses
  the history since then (a lead moved by hand takes its history with its `coord/`, §6.1). The day folders an app before task
  #1119 wrote (`state/coord/history/<day>/`, a file per writer) are packed the old way as each
  passes the week, then removed: its files read whole and merged into
  what the day's tar holds already (a late write, an old day's event appended after the day was
  packed, is added to the tar, never written over it; a tar that can't be read is never
  replaced), the tar built in memory and written in one stream to a temporary name (one fsync),
  read back and checked, renamed into place; only then the folder's files and folder removed,
  and only if the folder still lists the same names and sizes (an append between that listing
  and the removal, milliseconds, is the one way a line can be lost). A stopped run's temporary
  file is removed once an hour old. A day that fails (a folder holding anything but files, over
  512 MB, a deadline, an unreadable tar) is left as it is and said as a warning in the job's
  note. Every call goes through `nasread`'s deadlines. `pubtimes::load_history` (`scenic
  timings publish`, the forecast check) reads the days' tars and the lead's file (on another Mac,
  asked of the lead: `/api/history` with `data`, which the lead reads from its file); a lead's planned notes after a change of lead, the tars.
- **The contact** (`coordwire::lead`, task #1119): each Mac's agent keeps its lead's in its folder
  (`lead.json`: member, host, addresses, token, page), the lead's agent its own coordinator's
  (`Coordinator::reach`, each 5 minutes), a member's from its beats' answers (`reach`, while the
  lead leads); every other process of the Mac (the CLI, the map server, the updater and `scenic
  releases`, the `gc` job, the dev tools) finds the lead by it, with no NAS read, so an update
  reaches a Mac whose share is away. With none kept, or one naming another member than the
  declaration, the declared lead's host is reached by its tailnet addresses (`tailscale status`'s
  device of that name: its computer name or DNS name, letters and digits alone) and its LAN name,
  with the pool's token (`state/coord/token`); a kept contact of the declared lead has those
  addresses after its own. A member finds it again when a request to the coordinator it has
  fails, and when that coordinator last answered as a broker (epoch 0) or as another member than
  the lead its pool view has (`x-pool-epoch`, `x-pool-member`): an old lead's coordinator kept on
  as its broker answers every ask with nothing (task #815).
- **Its port is free for the next:** children are started (`store::spawn`) only while no listener
  or connection is being made or closed, and one at a time, under one lock, so no child inherits a
  socket or another child's half-marked pipe (Darwin has no `pipe2`; task #900: two caffeinates held
  a `scutil`'s output pipe and its reader hung). The long-lived `caffeinate`s are started
  `store::spawn::detached`, every descriptor but /dev/null on 0–2 closed. clippy.toml's
  disallowed-methods, checked by publish.sh, keeps every start, listen and accept going through it.
- With a lead declared, `check_writer` refuses every direct save of the records: everything hands
  off.

### 7.6 The lead's own slots

Jobs call the lead through one interface re-resolved each call (`crate::agent`'s `beat`,
`end_lease`): in process while this Mac leads, over HTTP otherwise, so a job carries on across the
owner's move of the lead (§6.1). A job's tasks are offered through its own Mac's coordinator or broker; one that stopped
answers nothing and the job runs the task itself. A process's parts are its role's, read as it
starts (docs/roles.md §1): the declared lead's coordinator, built then, grants in the epoch its
take-up leads; a change of the declaration naming another role restarts the agent at once, its duties
in flight stopped and every other job (a task leased from the lead too) handed over to the next
process; a lead whose coordinator failed to start restarts once its first slot is free. A lead drains the hand-offs its coordinator journaled on its disk (those a member out of the
pool told it, `journaled: false`) into its own hand-offs (`Run::drain`); every member drains,
once a process, the jobs a process of its ran out of the pool and left in its outbox untold
(`Run::drain_outbox`). A lead whose update waits on its jobs keeps the lead and restarts into the new app at a safe
point like any member (docs/building.md §2, A newly installed app). The lead's own asks always carry its capacity, so the lead never
holds packing off for the pool (§7.1, Mixed versions); its heartbeat says whether the pool is all on
the new app (`all_new`) for its members' heartbeats.

**A lease names its slot** (task #863): granted to a Mac (a member asks as one, the lead holds
its own under its name), it says which slot runs its job once that slot does: the lead's loop by
`keep_own`, a member's beats (`Beat::slot`), kept on the lease (`Lease::slot`, saved; an older
app's has none). The forecast's input puts the lease on that slot's lane and gives that slot its
time left and target count (`Coordinator::for_forecast` naming it `<host> #k`); one not said yet
goes on a lane of its Mac running its step (`forecast::lease_lanes`).

**The lead's running jobs' leases never lapse** (task #666): its coordinator runs inside its agent,
and each loop the agent says which leases its slots run jobs under, each with its slot
(`Coordinator::keep_own`, `Leases::mine`, its own name's alone): those are live past `coord::TTL`, beats or not, so `expire`
passes over them and they can always be renewed and finished, and a loop stuck past the TTL (10 min
on a slow NAS) no longer kills its running jobs. A lease whose job ended without giving it back
leaves the set at the next loop and lapses as any; a stopped agent's are dropped as the next one
starts (`drop_where`, `drop_workers` at the take-up, sparing the jobs it adopts), the set not
saved; after the owner's move of the lead the old lead's leases are a member's, lapsing as any. A beat that still
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
| input size | measured per target beside its cost (`sizes` in `costs.jsonl`, `buildstore::costin`: bytes read, threads, area, GeoTIFFs opened, the bytes and files its `Out` made on the NAS (`wrote`, `creates`) and the bytes its uploads read back (`read_back`, apart from `bytes`), and the counts a step has: ways and vertices, buildings, peaks); read by the step's model (below) |
| disk | measured per target: bytes written through `store::cachefile` (not the fetcher's kept ranges, `cachefile::put_spare`: made only into the room past the room target, never needed by a job) and the scratch writers (`SCENIC_WRITTEN`), plus a look each minute at the largest files in a slot's scratch folder; kept as the target's disk floor (`disks.json`). **Disk guard:** when free space falls below the owner's room target (less a thirtieth of it, so a job just let in at the target isn't frozen at once: `memguard::disk_short_of`), the fastest-growing job is frozen (another too if the disk falls past half the target), room is made from caches no job holds (`room::toward`), and it goes on once the target is free again or is given back (the freeze held across its agent's restart, task #1137: docs/building.md §2, Restarts). No target is set until the owner picks one (Off is none); with none, the guard fires only at an actually full disk, with no other floor. Given-back jobs' scratch folders are deleted oldest first when room is short (`room::free_given_back`) |
| jobs on one Mac | a job is granted beside others only if their measured memory, disk and cores fit (packing, §7.2; one only waiting on a worker by what it holds and no cores, §7.2, A job waiting on a worker; one leaving the cores idle by its measures and no cores, §7.2, A job leaving the cores idle), and only while every member reports the new app (§7.1, Mixed versions); a cold start by its provisional need (§7.7) |
| raw tiles | `store::cachefile`'s locks; packing deletes only tiles no job holds; a job reopens its shard's archives when the index changed (`RawTiles::get`, `refresh_shard`; a shard is the z6 tile, z3 tile or `root` whose archives hold a tile: docs/macs.md) |
| Wikimedia budget | `crate::budget`: one budget for the pool, enforced at egress. Every job's connections go through its agent's proxy (`crate::budget::proxy`, `HTTPS_PROXY`; its clients served on an event loop of two threads, what blocks (a token waited for, a connection opened) on at most 16 more, at most four waiting for each class's token at once: task #908), the job sandboxed (`sandbox-exec`) to deny any other way out. A metered connection takes a token first, of a ten-second window's: from the lead, one a take (`POST /pool/budget/take`, its windows counted in memory by its clock, docs/coordination.md §3.1). `wiki` four requests a second; `query` (WDQS, QLever) one query per two seconds of query time. A 429 told to the proxy (`POST /hold`, `budget::seen`; not QLever's 429 for a query past its time limit, which is no ask to wait: landmarks' wikiask asks such a batch again in halves) holds every Mac (told the lead, each Mac stopping at its next token). The budget, our User-Agent and the back-off are how the APIs and data downloads it carries are asked politely: they read no robots.txt, which is for crawlers (docs/inputs.md §6.9). The lead away (or not known yet, as an agent starts), a Mac takes nothing, asking again each window (a share of its own would add to what the lead gives the Macs that reach it). Lint: `tests/budgets.rs` |
| NAS and network budget | `crate::budget::nas`: the lead's own jobs always; the members' too unless the pool's setting `nas-budget` says `off` (on while not set; the lead's, `/pool/settings`, §12). A job's **NAS weight** is its share of the NAS in thousandths: its targets' measured rate (`needs::Book::nas`: `bytes` read, less those of this Mac's own disk or its page cache (`local`, task #551; a NAS file copied whole counts what of it this Mac's page cache held as the copy began, `mincore` over a mapping never read, of the copy's own descriptor, `store::sys::resident`; a file of no more than one copy read, 1 MB, isn't asked: mapping a NAS file is a round trip of its own, 15-19 ms; task #939: slope-deep copied its z6 tile's terrain hi pack, about 200 MB, once a target, every copy after the first from the cache in a second or less (a second copy of a 233 MB pack took 0.05 s, en0 receiving 5 MB), counted as the NAS's), plus `wrote` and `read_back` (what its uploads read back to check them, task #1142: each verified upload reads its whole file back, store::naming's `write_atomic`; a line without `read_back`, from before it was measured, counts its `wrote`, every file costin counts made having been read back whole, so no `cost_v` was bumped for it), and `creates`, each over its wall time, a halo step's (`pack`, `lo`) `wrote` and `read_back` alone, its job's read rate (`nas_read` over `nas_secs`: what the job read of the halo's base packs from the NAS and the time that took, on each of its targets; a measure without `nas_secs`, task #396's first, counts no reads) in place of its `bytes` (the halo's base packs' files); its own run's, else its step's 90th percentile of each, each run weighed by its time (the rate its runs stay under 90 % of the time they run: counted run by run, slope-deep's thousands of one-second runs made it 248 MB/s, three times the NAS, task #939), its runs the way it runs now (`cost_v`: an earlier way's count for nothing), each put at the lead's pace by its Mac's and the percentile taken at the pace of the Mac it's weighed for (the asker's, the lease's; an aged target's held room at the lead's: `Book::nas_for`, Each Mac's pace below); none measured, 0, but a step known to load the NAS (`NAS_STEPS`: `unit`, `pack`, `lo`, `gc`, whose deletes load it, and `planet-fetch`, whose download is written to the NAS and read back) weighs the whole, task #499 review), and its metadata work (task #451: `listed`, `stats`, `deletes` from `costin::meta`, the folder entries it listed and the files it statted and deleted (renames counted as deletes), each over its wall time; counted by GC's walks, which note a cost line `gc gc` in their job's costs file, and the inputs step's inputs-folder listing), each over the NAS's limit, added: one combined weight, at most the whole (1000); a job of several targets, what they move over their times together, as it runs them one after another (each target's rate times its time: its own run's, else its time as its step's model predicts it on that Mac; `Sched::job_nas`, `NasWeigher::job`, task #939: weighed by its heaviest target, a slope-deep job of 52 targets weighed the whole NAS for 32 minutes while it moved 12 MB/s), and a job with a target of no measured rate or no known time, its heaviest target's (a job of no targets of its own, its lease's, its step's). A target joins a member's batch (`pick_every`) only while the job it makes still fits. **A leased job that has run past its predicted time** (its targets' times for its worker summed, `Sched::secs_of`; a target of no known time counting `OWN_AGE`) weighs instead what its job has moved on the NAS (bytes read, its uploads' read-backs among them, and written, files made, its metadata work: its processes' counts, carried by each lease beat, `agent::jobs::Live`), by the same limits: the greater of its mean rate since its lease's first beat (`Lease::mean`: what it moved from that beat's view to the newest over the time between; a beat counts what its job moved since it began, and a lease taken up at a lead's restart or held again at an agent's take-over is younger than its job, task #991: put over the lease's own time, on 6 Nov a 5 MB/s download taken over 5 hours in weighed two thirds of the NAS, and the lead granted itself nothing beside it on an idle NAS) and its rate over its last minute or so of beats (`Lease::lately`, `WINDOW`), so a job quiet early and heavy late is weighed by its heavy stretch within a minute or two, and one pausing between heavy stretches keeps its mean (`Sched::job_weight`, task #925: a slope-deep batch of 30 targets weighed at the whole by their runs on the other MacBook, where each read its 280 MB of terrain in 2 to 22 seconds, ran 52 minutes on the M1 at about 3 MB/s, and while it ran the lead granted no other job anywhere in the pool). A beat that doesn't say all it moved (an older agent's says its bytes alone) leaves it weighed by its targets. Within its predicted time its targets' weight stands, so a job not yet at its heavy reads is never weighed light; past it, a burst a job's targets didn't predict can overcommit the NAS for up to two minutes, and the gate on the NAS's own load below holds new grants meanwhile. The forecast weighs leased jobs by their targets alone. **The limits** are measured: the lead's coordinator probes the NAS every 30 minutes, whatever the switch says (`probe`, 4 streams at once (`STREAMS`, task #534: on 21 Oct one stream measured 5 creates/s and 25 MB/s where four made 41 creates/s and eight 57 MB/s, so a single stream set limits a fifth to a half of the NAS's): in each, 12 files of 4 KB made, then in each one of 16 MB written, synced and read back past the cache; the 48 small files deleted one by one, and a kept folder of its own (`<host>-meta` beside the probe's folders, 48 files, never removed by the stale-folder clean-up) listed and its files statted, measured only once it has gone unwritten 5 minutes so the answers come from the NAS, not this Mac's SMB cache of what it just wrote (missing or young: made, and 0 this probe; a 0 weighs nothing and the day's best is kept), in a folder of its own under `state/pool/nas-probe/<host>-<pid>`, all removed after; as it starts, the coordinator removes its host's folders a stopped process left, never another host's) and takes the best of the last day's probes (a probe under the build's load measures what's left, so it's kept: the day's best passes over it, and dropping it would leave a lead started on a busy NAS no limits). The probes are kept in the lead's store (`nas-probes`, `coord::kept::PROBES`: saved every 30 minutes by the lead's prober alone) and read once the store is open, so a restart keeps the day's best (task #603: on 23 Oct a restart under load cut the limits from 86 MB/s to 66.5 and inflated every weight). **The gate on the NAS's own load** (`nas::Loop`, task #534): every 10 s the lead reads each disk's busy share from the Synology over SNMPv3 (`crate::budget::snmp`, SYNOLOGY-STORAGEIO-MIB `storageIOLA`, the one SNMP reader, for the build page's figures too; its keys `NAS_SNMP_USER` and `NAS_SNMP_AUTH` from the project's `inputs/keys.env` (a value with whitespace or `#` is refused, as net-snmp's conf can't carry it: no usable keys, the gate open; the coordinator logs it once, naming the key, never its value), handed to `snmpget` in a private `snmp.conf` (0600, in a 0700 folder under the temp folder, `SNMPCONFPATH`; made once per lead, removed when its coordinator stops, and a stopped process's folder left behind (`scenic-snmp-<pid>`, pid gone) removed as a new session starts; `~/.snmp` untouched), never on a command line, never logged; a poll killed after 8 s) and smooths the busiest disk's (a new reading weighs 0.3: about half a minute's memory). Once that load is over the target the budget is held, and it reopens only when the load is below the target less 3 points (`nas::BAND`, so a load hovering at the target doesn't open and shut it every reading) (`Limits::held`): a job that weighs anything is granted, the lead's or a member's, only beside leased jobs that weigh nothing (`Limits::open`, on top of every rule above; the lead's own jobs asking to go alone (`pick_own`) count the members' jobs alone as the leased ones, so while held a job of the lead's own may still go beside the lead's other own jobs, as intended: the whole-budget rules bind them by weight as ever; weights and room are the probes' as ever, so a held NAS never admits a job an unheld one refuses), whatever loads the NAS (an outside repack counts); jobs running go on. Held, the lead's waiting lines say so: a line for the NAS ("held by NAS load (91 % > target 85 %)") and each target held back for the budget names it too (`Coordinator::nas_held`, `agent::nas_wait_lines`). With no keys or no answer, the gate is open (the probes' limits alone). **The target** is the owner's, pool-wide: the pool's setting `nas-target` (its share, 30–95, with who set it and when), kept by the lead in its store (`settings`, docs/coordination.md §3.1), set by `scenic nas-target [<percent> | off]` on any Mac (asked of its agent, which sends it to the lead; off: the default; with none, how it stands as the agent last had it); the lead alone enforces it. Not set, 85 % (`DEFAULT_TARGET_PCT`): never 100, after #451's stall at a NAS held busy, and above the 65–80 % an outside repack alone kept the disks at on 21 Oct, so the pool still works beside one. `scenic pool nas-probe [--dir <folder>]` runs one. (The NAS's space is apart: the owner's NAS reserve, `scenic nas-room`, docs/building.md.) **Enforced at grant** (`coord`'s `pick_every` and `pick_own`, so `pack` too, the lead's own jobs among them): a member's target is granted only while the weights of every job leased in the pool (the lead's own among them) leave room for its own within the whole (1000), or while no other member's job weighs anything (the lead's own alone never hold it back), or, once it has waited 10 minutes for the budget alone and no leased job weighs the whole, if it weighs the whole itself or more than the leased jobs together (`Limits::fits_job`, `OWN_AGE`: the members' jobs never starve it; it waits for heavier work to end, never for lighter; once it's in the leased jobs outweigh any other, so aged ones don't pile up); the lead's own within the whole, or while nothing leased weighs anything, or, once it has waited 10 minutes for the budget alone, while no leased job weighs the whole and the lead's own leased jobs leave it room within the whole (its every-step targets, `pick_every`: `Limits::fits_own_target`); a planned job of the lead's own (`own_nas_fits`) also, not waiting, once it weighs the whole itself while no leased job weighs the whole (`Limits::fits_own_job`; both `OWN_AGE`, the wait timed from when it was first held back and forgotten once granted: members' small jobs never hold it back for good, and the lead's own aged jobs don't pile up past the budget; the simulator checks each start of the lead's own against the same rule, with the waits as the coordinator kept them); **an aged target holds its room** (`Shared::nas_reserved`, task #444): of the targets held back for the budget alone for 10 minutes or more (and passed over in the last two minutes), the one held back first has its weight kept free: no other job weighing anything is granted, a member's or the lead's, unless the leased jobs, it and that one fit within the whole, so a target weighing less than the whole but more than the room left (depth-lo's) goes as the leased jobs end instead of waiting behind a stream of smaller ones; it never lets the budget past the whole, the reserved target itself aside (by `fits_job`'s, `fits_own_target`'s or `fits_own_job`'s aged path), and the reserved target always has a way in once no leased job weighing the whole runs (task #603: on 23 Oct a pack of 946 aged beside the M4's terrain of 103 could pass no rule, a member's aged path then taking only whole ones and the lead's every-step targets none, and the room held for it stopped every other job for 41-62 minutes); a target is noted waiting only when its asker would take it but for the budget (its measured memory and disk fit, or a cold start it's the one to make), so one no asker can take never holds room; a step's grant restarts the waits of its other aged targets weighing no more than the job granted, weighed by the NAS's own limits whoever asked (task #552: it isn't starved, it just took room; else a step granted target by target holds the reservation for good, each grant handing it to its next target, as 52 units aged an hour did on 22 Oct while every other step and the lead's slots waited); a cold job of its own waits for the lead's jobs to end (§7.2) only when it would fit with the members' jobs alone. **The lead's own jobs are always held to it**, whatever the switch says (the limits probed: `nas_own`); the switch, unless `off`, holds the members' too (`nas`). Switched `off`, the members' jobs are not held and the lead's own are held by their own weights alone (`Shared::nas_room_heaviest`: a member's unheld jobs no longer keep the lead idle). The rest wait for a job to end; a target held back for over 30 minutes is a waiting line in the lead's status (`Coordinator::nas_waits`), once each (`agent::nas_wait_lines`); an ask granted fewer jobs than it wanted, with nothing else offered fitting its room, while targets it would take wait for the budget says so first in its refusal ("the NAS budget: 4 targets it would take wait for the NAS's room; else lo 3/6/3 needs …", `Sched::nas_waits_at`, task #931: on 4 Nov each slope-deep job weighed the whole NAS by its targets' runs (their reads a median 2.2 times the NAS's probed rate), the pool ran them mostly one at a time, and the idle Macs' refusals named only their memory). **The clean-up waits for a quiet NAS** (`QUIET_STEPS`: `gc`; task #451, after GC beside two depth-lo jobs held the NAS at 34 % IO wait and all three stalled): it starts only beside leased jobs weighing nothing, or within the whole with them by its measured weight (`Limits::fits_quiet`), and the room held for an aged target. Its wait isn't noted (it holds no room) until it has been held back 24 hours since it was last granted (`QUIET_AGE`, `Shared::quiet_since`, kept in memory: a new coordinator counts afresh): then it goes as an aged job of the lead's own (`fits_own_job`, its wait noted, its room held). Unmeasured the way it runs now (none of its runs of its step's `cost_v`), it isn't held for a quiet NAS: it weighs the whole, as any unmeasured step known to load the NAS, and goes as the lead's own such jobs do, beside no leased job weighing the whole (`fits_own_job`), never past the gate on the NAS's own load, so its first run measures it and nothing else weighing anything is granted beside it meanwhile (task #1000: GC's step 8b ran no longer as the daily job, whose last run weighed the whole NAS by its reads, and held for a quiet NAS a busy pool never had, it never ran; task #1001: its runs hashing the pending uploads, 813 GB in 4,844 s, weighed it the whole NAS again: `gc`'s `cost_v` 2, the uploads checked by their sizes, puts those runs away). Its weight is its deletes, folder entries listed and files statted a second over the probes' rates for them, its bytes next to none; the probes delete 4 KB files, the clean-up larger ones. Running, it yields at its next safe point (each removal of its queue's, each folder of its sweeps, each batch of the pending uploads' check; `control::draining`, its cost line noted first, ending as paused) when a target held back for the budget `OWN_AGE` doesn't fit beside the leased jobs but would without it (`Coordinator::nas_yield`, asked by the agent each loop). **Inside a job** (task #1096, the owner's of 10 Nov): no allowance is handed out; each process holds its own reads in flight on the NAS to a limit it finds as TCP finds its window (`store::aimd::NAS`: from 2, decided once a window on its reads' median latency over their size class's best lately (never under 2 ms; this Mac's cache's answers no samples): doubled until its first halving and one more a window after while near it, halved past three times it or when a read times out on the share, 1 to 16; every read of a stored file, a pack, a mid or a whole copy takes its turn), so the processes and Macs reading the NAS settle into shares between them and a quiet one holds nothing; the weights above still count what each job moves. (The lead's smoothed SNMP load is not passed to the readers: it would ride the beat's answer to the agent and then to each job, which no channel carries today.) **Downloads:** a job of a fetching step (steps table kind `Fetch`) counts one, capped pool-wide by the switch's `downloads <n>` (no cap without it): the lead's own fetching steps (`bld-fetch`, `rail-feeds`, `planet-fetch`; none is offered to members). Not held to it: tasks |
| a pass's answers | `crate::answers`: a lease around each sync (renewed every 150 s, lapsing after 10 min), the lead's (asked through the agent's proxy, `/pool/lease/answers`; in the lead's store); on a Mac a reader/writer lock (`answers::Local`) |
| write-set | `agent::steps::writes`, enforced by `Out` and the merge (§7.3). Raw archives, answers and `bld-fetch`'s sources are outside the manifest, each written whole under its module's names |
| targets per job | as many as fit the batch length of measured time on the Mac (the pool's `batch-minutes`, else 15: `Sched::batch_len`, Batch length below), the lead's and a member's alike; a step kept to the lead in one job |

**One model per step** (`agentcore::needs::Book`: `need(step, target, threads, guess)`, and `mem`
alone for the picks): only what was measured the way the step runs now counts
(`coord::cost_version`, the steps table's `cost_v`); a run or floor of an older way of it (a z6 tile's before v3, its elevations sampled whole; the heritage job's before v1, its clip of the filtered planet, whose 21.9 GB floor no run since 20 Oct came near: 6.4 GB) is neither a
target's memory or time measure, nor a fit's run, nor part of a prior (disk floors, `disks.json`, carry no version and always count). A target's memory, disk and time are its own
run's (memory never below its floor; a floor alone counts as its measure); else, a tile target's
memory, **by its place** (task #931, `needs::Near`; not a `GUESS_FIRST` step's, units' and
candidates', whose piece's guess no backtest has weighed it against): the most its nearest measured targets of the
step took (rings of tiles at its zoom taken outward until they hold `NEAR_RUNS`, 5, or
`NEAR_RINGS`, 8, are taken; a floor counts as its tile's measure) × 1.1, where that is judged good
on the step's own measures (each predicted from the measured tiles before it, row by row, as a plan
reaches a place from one side: at most 5 % predicted under, a median error at most 50 %) and erred
less there than the step's best good fit. What a target holds is its place's, whichever Mac runs it:
terrain-deep's neighbouring tiles differ by a median 6–8 % run on two Macs (no Mac's the higher by
over 2 %), 2 % on one; its peaks run from 3 GB where the fine data is sparse to past 9 GB over dense
lidar, where its prior (7.8 GB) reserved a sparse target twice its memory and gave dense ones back.
Replayed on the lead's runs of 15 Oct – 4 Nov, each target predicted at its grant from the runs
known by the minute it began (`needs`' `backtest_mem_on_the_leads_measures`, by hand): terrain-deep missed 5.2 % of
its targets (2.0 % by over a GB) and reserved 1.28× their memory, against 15.3 % (3.9 %) and 1.64×
by its prior; slope-deep 5.8 % and 1.12× against 5.6 % and 1.27×; terrain-hi 4.3 % (0.2 %) and
1.24× against 4.9 % (0.2 %) and 1.21×; bldtiles 3.8 % (3.1 %) and 1.24× against 26.8 % (2.6 %) and
1.13×, the one step it misses by over a GB more often. (Replayed an hour at a time, a target
granted late in an hour predicted without the neighbours that ended earlier in it, terrain-hi
looked worse by place: 8.1 % against 5.0 %.) Else base + k × size
fitted over the step's measured runs (least squares raised to cover every run, × 1.1, capped at the
most a run it was made from measured; threads a term when the runs differ in them), on whichever size
fits best (a `sizes` measure the target has from an earlier run, or its tile's area); else, for z6 tiles
and landmark candidates, their extract-size first guess (`coord::unit_peak`), so one dense z6 tile doesn't
set every other's; else the step's prior: memory and disk, the 90th percentile of its targets'
measures (memory: their floors among them), a heavy target held by its own floor; time, the mean
of its targets' runs, the expected time a job's targets are counted to the batch length by and the
forecast sums (an update's drain alone waits by their 90th percentile, `Book::secs_p90`, so a long
tail's long jobs aren't cut short). A step whose time is the work its target finds, not the target
(`needs::PER_WORK`: `pyramids-fill`, by `fetched`, the tiles a z6 tile asked of GSI; task #830),
counts its runs in that time (its time prior, its fits, `secs_p90`, the forecast's per-target time)
weighted by that work: a run that did none, or doesn't say, counts for nothing there (five z6 tiles
already stored, 17–37 s each, had made the step's time 30 s against a fetching one's 4,950 s),
while its own target keeps its own time; nor is the lead's own pace of its jobs (`step_secs`)
read for it. **Drops** (task #837): a target offered under its step's "none" key
(`agent::build::is_none_key`: a terrain-hi, slope-hi, terrain-lo, slope-lo, terrain-deep or
slope-deep target whose tile no target is any more, its packs still named) only drops its packs
from the manifest and reads nothing, a second or two where a build takes minutes and gigabytes
(on 29 Oct 4,275 terrain-deep drops in Alaska were sized at the step's 76.5 s and 7 GB: about 91
lane-hours). Every step's drops run through one helper (`scenic-build`'s `run_drop`), whose cost
line carries a `drops` measure (`needs::DROPS`, the packs dropped); such a run counts for nothing
in its step's model (its priors, fits, time weight, the lead's `step_secs`), and a drop not yet
measured itself as a drop (its own run as a build before it became one says nothing of it: task
#1087's hollow deep targets had each run) is sized by its step's drops (`Book::drop_need`: their mean time, the 90th
percentile of their memory), before any of its step's ran by every step's drops (a drop of any
step reads the manifest alone), and before any drop at all by its step's model, as a build: one
job, after which it is measured (no figure is typed in for it). The scheduler marks them as
offered (`Sched::drops`); the plan puts every step's drops first of the terrain, slope and tree cover work
(`seam::Planned::drops`), the forecast lists them first as runs of their own (`Stretch::drop`, never merged
with a build's run), shown "drop to come" on the build page. A drop never runs under a job that
reads what it drops: what reads its tile waits for it as before (the z3 tiles with a none target
left are terrain or slope left), it has one key per target so it is never leased beside a build of
the same target, and its tiles are no build target's input. Else the caller's first guess (the size
the plan offered, where it derives one): a cold start. The 3D buildings (`needs::OFFERED_BY`) are
offered by the rows their tile's sources hold (`bld::sources::Sources::rows_for`), not a memory
guess: their runs note the rows they read (bldprep its sources', bldtiles its normalized files',
at most those), and a target not run yet is fitted by its offered rows like a size of its own. A fit is judged on runs held out of it
(5 folds) and not used when it predicts under on more than 5% of them, is off by more than 50% on
the median one, or has fewer than 10 runs. The models are kept until a cost, floor or disk floor
changes. The coordinator's picks (`pick`, `pick_every`, `measured`, `secs_of`, `peak`), its floors
and the forecast all read it. `scenic pool needs --fit` prints each step's fits and their held-out
errors (memory, wall time, disk) beside the prior's, and lists the steps that don't fit.

**Each Mac's pace** (`crate::paces`, task #926): the Macs run a step at different paces, and a
step's model mixed their runs as they came, so a step mostly run on a slow Mac sized a fast one's
jobs by its times and NAS rates. Each Mac's pace at each step is measured on the same targets: a
pair is a target of the step done well as a job of its own on the Mac and on the lead, the step run
the same way (`Event::v`, the cost version the lead stamps on each end and done) and both in the
last 5 days (`PACE_S`); its ratio the Mac's time over the lead's (the lead's run nearest in time).
A Mac's pace at a step is the median of its pairs' log ratios, drawn toward its pace over its steps
(the median of its steps' medians, drawn toward the lead's 1 by their count) by 10 pairs' weight
(`PRIOR_PAIRS`). A Mac with no pair is at the lead's pace, in the cost book, the scheduler, the
forecast's lanes and costs and the plan's lanes alike. The same target, each side a job of one
target, takes out what a Mac was given: its targets' size (placement sends the M4 the
memory-heavy terrain-deep tiles) and its jobs' size (which `secs_of` sets by the pace: a pace from
per-target means over its jobs would feed back on itself). Per-target means did move with them: the
M4's terrain-deep read 4.49 times the mini's over a day, 1.26 over a week, where its same targets
took 1.39 day after day. A pair's runs keep what loaded the Mac then (its owner's builds); the
forecast takes each Mac's room at its sustained best instead (§7.2, task #951). **The window and the weight** were picked by a held-out test: the history of 8 Oct to 4 Nov
(19,137 jobs, 1,973 pairs), each day's pairs scored against the paces of the window before it,
for windows of 1 to 28 days and weights of 0 to 30 pairs: the mean |log error| was 0.932 with no
pace, 0.834 at 5 days and 10 (the best of the windows the lead's history holds, 7 days; 0.833 at
14), 1.018 at 3 days and none. The same test on each target's share of every job (21,705 pairs)
gained less (1.000 to 0.920), and depends on the jobs' size. Pairs are few, and none at the steps
run most: a pair needs a target run twice, and a target is built once unless its step changes. On
6 Nov 16:08 UTC, over 5 days (the lead's history, `Paces::of`'s rule), the M4 had 87 (terrain-hi
26, polar-terrain 25, bldtiles 15, depth 7, pois 4, trees 4, pack 3, depth-lo, terrain-lakes and
unit 1 each) and the M1 48 (polar-terrain 22, bldtiles 10, terrain-hi 4, depth 3, lo, pack, pois
and terrain-lakes 2 each, depth-lo 1); none at terrain-deep or slope-deep, though the M4 ran 543
terrain-deep jobs of one target, the M1 340 and the lead 485, so those steps' paces are each Mac's
pace over its steps. The lead works
the paces out from its history's runs of one target (`paces::reads`, of the jobs done well the agent
keeps, `paces::job`, only the new events read each loop; the jobs' starts from all of them, Batch
length below) when one comes or every 10 minutes (`Coordinator::set_paces`, in the scheduler's
state, so the forecast's run has them). The step's model reads every run's time (its
prior, its fits, `secs_p90`) and NAS rate at the lead's pace by its Mac's, and its prediction for a
worker is at that worker's Mac's pace (`Book::secs_for`, `nas_for`; the scheduler's `secs_of`: a
job's targets counted to the batch length, a leased job's predicted time). A target's own run is
read at the pace of the Mac it would run on (Batch length, below).

**Planned, not built: a size-fit residual pace** (task #934). A second source for a step without
pairs, from every run of it: each run of one target done well, its time against what the step's
size fit (crate::needs' fits, `Fit`) predicts for its target, as a log residual; a Mac's pace at the
step the median of its residuals less the lead's, drawn toward its pace over its steps as the
pairs' are, and weighed against its pairs' by the same held-out test that picked `PACE_S` and
`PRIOR_PAIRS`. The fit would be made from the lead's runs alone: one made from runs put at the
lead's pace by the paces would feed the pace back into itself. A step with no good fit (`Fit::good`)
keeps its pairs alone. Not built: the forecast error that asked for it (#926's backtest, the end
under 15 min worse on train) didn't hold on the archived inputs (§7.9, code 4), so nothing measured
yet shows the sparse pairs costing the forecast or the placement accuracy.

**Batch length** (`Sched::batch_len`, `Sched::batch_secs`, task #927): a job of a shared step takes
its targets in plan order (its first whatever its time) while their measured time on the Mac asking
stays within the batch length: the pool's setting `batch-minutes` (`scenic pool set batch-minutes
<minutes>|off`, or the build page's Settings, the pool box, from any device (docs/coordinator.md §7); a
minute to a day, kept by the lead (`/pool/settings`); not set, 15: `coord::JOB_S`, the
owner's choice), carried in the scheduler's state so the forecast's runs size by it too. (At
slope-deep's 5 to 12 s of start a job, below, a quarter hour spends about 1 % of a job on its start;
a longer batch saves little of that and holds more targets behind one lease when its Mac pauses,
drains or goes away.) Before task #927 a target's own run counted twice its time on any other worker, and one
not run its step's mean: a 30-target slope-deep batch sized at 15 minutes by the other MacBook's
runs ran 52 on the M1 (#925), and on 4 Nov an M4 batch of 47 full targets ran 5 hours. A job's time
is its Mac's jobs' start at the step (`Paces::with_starts`: the median, over its jobs done well in
the paces' window, of a job's time past its targets' own runs, each its cost record run on that Mac
the way the job ran and the step runs now (a job of an older way, or one whose event doesn't say its
way, counts nothing); slope-deep's 5 to 12 s, the time a batch of empty targets is mostly made of)
plus each target's time on that Mac (`needs::Book::secs_on`) at the threads the job would be granted
alone (`Sched::sizing_threads`, the grant rule's):
- **its own run**: on its Mac, as it ran; another Mac's at the lead's pace by that one's and at this
  one's (Each Mac's pace); on fewer threads than its CPU time kept busy, its CPU time over them, where
  that's longer;
- **not run, with a source run** (the same target of a step it reads, the first in its steps table
  reads naming it: slope-deep's terrain-deep, slope-hi's terrain-hi, run before it can start): the mean
  of the step's runs whose source took about as long (doubling bins of the source's time at the lead's
  pace, `needs::BySource`): this Mac's own runs in that bin where it has `needs::MIN_RUNS` (10) of them,
  its own pace at the step; else the step's good fit (above) at this Mac's pace; else every Mac's runs
  in the bin at the lead's pace times this one's. The bins are used only for a step where they predict
  its own runs better than their mean (each run as it would be read, its Mac's bin else every Mac's,
  left out of what predicts it, by the median error a fit is judged by).
  What the source took
  says what the target holds: on the lead's runs to 5 Nov a slope-deep target whose terrain-deep took
  under 2 s took a median 0 s, one whose terrain-deep took 64-127 s a median 57 (their logs correlate
  0.92), where the step's mean, 25 s, sized 30 empty targets and 30 full ones alike at about 12 minutes;
- **else** its step's model at this Mac's pace (above).
Every one of these reads only runs of the way its step runs now (`coord::cost_version`): a target's
own run, its source's, the bins, the fits, the Macs said to have runs and the jobs' starts. A target's
own run is read by one rule wherever it's read (`needs::own`, `Book::own`): its time and memory here,
its time at its threads (`Sched::target_most`, `per_thread_mb`), a drop's own drop (`Sched::drop_of`),
the forecast's cost table (`forecast::target_cost`: known at the lead's pace, else its step's
mean) and a measure its run takes as a job ends (`Pool::learn`: kept where the target has one
already), so the forecast times the jobs the grants size. Replayed with its table made by the rule
(`fccheck::recost`), the archived input of 12 Nov 02:39 UTC, whose table had known 155 of its 165
terrain-deep targets at 0-8 s from runs of cost version 0, foresees 5 terrain-deep jobs on the M4,
2 on the mini and 2 on the M1 in the 293 s to the next plan (the lead granted 2, 5 and 2), where
it had foreseen 18, 5 and 17. After
task #929's merge raised slope-deep's, terrain-deep's and the terrain steps' versions, every one
of their targets' last runs counts for nothing, as in the threads granted (§7.1, `crate::uses`).
The one place an older way enters is the step's mean while the way it runs now has fewer than
`needs::PRIOR_RUNS` runs: its latest earlier way's mean where larger (the first runs to end are the
quick ones, above), so it can only shorten a batch, never lengthen one.
A Mac with no runs of the step the way it runs now is sized as before task #927 (another worker's
run counted twice, the model at its pace), and says so: the grant's log line names the time each job
was sized to and how ("sized to 14.8 min by its Mac's runs", or "as before task #927: <Mac> has no
runs of <step>"), and the history's lease event keeps it (`data`: `sized_s`, `own`). A leased job's
predicted time (`job_weight`) is the same sum. The forecast's runs size by the same code, but offer
an ask the first 64 ready targets of a step (`forecast::OFFER_MAX`), where the live plan offers all
of them and the coordinator grants up to 256 (`JOB_MAX`). Two gaps follow: a run of more than 64
targets that fit the batch length (slope-deep's empty ones, a second or less each past the job's
start) is one job live and two or more in the forecast, each paying its start; and with placement
on, a run starts at the target measured to take longest among those offered (`placed_start`),
among all of the step's live and among the first 64 in the forecast. They stay because the offer's
size is the forecast's CPU: task #951 offered 256 and the live-scale test's three runs took 463 s
of CPU against 135 at 64 (its bound 180 s), and memoizing `placed_start`'s time of each target
(`secs_of`) didn't bring it down: the cost is the work each ask does per target offered (the
offers made, each target's checks and placement's draws), which an incremental run would have to
keep between asks. **Measured** (`scenic forecast-check --replay`, its
last table, `fccheck::batches`): each job the history ended against what the scheduler of the
forecast input recorded before its grant sizes it to (that state, each Mac's pace and jobs' start
from the history to then), its time over that, by step and Mac; over 4-5 Nov (the inputs archived
since #933), median and 90th percentile, before task #927 and after: slope-deep on the M4 0.69 and
4.51 to 0.64 and 1.29 (36 jobs), on the mini 0.51 and 2.07 to 1.03 and 1.53 (63), on the M1 0.97
and 4.78 to 1.17 and 1.77 (30); slope-hi, 24 jobs nearly all of one z6 tile (whose length the
batch doesn't set), 0.79-1.12 at the median before, 0.30-0.64 after (the review's rerun the same):
sized at up to three times what they took. Why (task #927 review): slope-hi's runs of cost version 4
were of two kinds its source's time doesn't tell apart. A z6 tile whose z10 tiles had no slope-deep
deep mid made their z9-12 itself from its hi terrain (1-2 Nov on the M1: 130-1474 s, 150-7,350 s of
CPU, its "working out the slope tiles" phase nearly all of it); one whose deep mids were there read
them in (3-5 Nov: 9-78 s, 1-16 s of CPU, the rest waiting on the NAS and its uploads), with terrain-hi
times and inputs alike (200-600 s, ~700 MB). The bins of long terrain-hi times held the first kind,
so the second was sized by them; the step's mean, blending both, had happened to land near. The
sizing measured what the runs held; their work had moved under them within one version. Task #952
(slope-hi makes every z10 tile's z10-12 itself) raised slope-hi to version 5 in #929's merge, so
none of those runs counts now; the replay above was run on the code before that merge.

**A cold start** (no target of its step measured the way it runs now; task #499) is packed by its
*provisional need*: the most its step's runs of its latest earlier way measured (`cost_version`
raised; their 90th percentile, or the target's own run of it, the larger: `needs::Book::older_mem`,
task #979; floors of that way stay out) or it was offered with (unit's and pois' extract-size guess,
terrain-hi's bound), times 3/2 (`coord::sched::provisional_mb`), its disk its step's measures'
(else none); with neither, it has no figure and runs alone (`Sched::no_figure`); its cores its step's
use on the Mac where its timings have any (§7.1, task #947), else one thread (task #896)
when its step is known to honour the threads it's granted (`coord::THREADS_HONOURED`: `slope-hi`,
`slope-deep` and `terrain-deep`, on the rayon pool the agent sizes, terrain-deep's
reads waiting on the network; `bld-fetch`, its downloads bounded by `--jobs`; `pyramids-fill`, its fetches a host's requests in
flight, the fetcher's limit a host, store::aimd), else the whole
Mac. It goes beside other jobs when that fits the room left, and other jobs go beside it by the
same figures; one bigger than a Mac's whole room goes only to a Mac holding no job. At most one
cold start runs on a Mac (two unknown needs never meet), and a step never measured in any way (no
cost, floor or disk of any way it ran: `never_measured`), or one with no provisional need, runs
alone, nothing beside it, as every cold start did before task #499. A cold job of the lead's own that can't go beside its jobs
holds the lead's asks until they end, then goes before any other work (`own_cold_waits`). One job of the step
runs in the pool at a time (`pick_every`, the lead's `cold_under_way`), so the step is measured
before it fans out. Its Mac's room left is never more than what its jobs hold now leaves (§7.2,
`Capacity::held_mb`), so one holding past its provisional need is counted at what it holds as soon
as the guard samples it; one that grows until its Mac is below its memory floor is the memory
guard's (§7.2), which learns its floor. After one run, the step's other targets are offered by its model (above).
**A records-only step** (the steps table's `records_only`: `redo`, whose jobs only change build
records; task #838) is packed at one thread counted for no cores and no disk, its memory measured
as any step's (task #979: a redo job notes its run under "redo redo", `agent::redo::note_cost`, so
each new ask's job is sized by its step's runs): measured, it goes beside any jobs (a long one
holding the whole Mac included); never measured, its first job waits for the lead's jobs to end, as
any step's (`own_cold_waits`). A floor never stalls the build: the first slot starts once
room-making has done what it can, the disk guard as backstop.

Planned: `cost_version` derived from the step's key version (`crate::agent::build`'s `*_V`) rather
than kept by hand; progress of steps other than the OSM pass resumable on its Mac.

### 7.8 The agent never waits on one NAS call

An open on the SMB share can stay stuck in the kernel for many minutes while the same file reads at
once from a shell. Every NAS call of the agent is a store::deadline call (docs/nas-access.md §6): on
its loop's root (`no_retry`) and in `crate::agent::nasread`, waited for 20 s at most
(`deadline::SHORT`; the planner's keys 5 min, below); on the threads it starts and never waits on
(`long_retry`), 120 s. Past it, the caller gets a timeout and takes it as any call that fails now,
logged once per path until that path answers again (`nasread`'s calls). The call is left to the
kernel; while the process holds its cap of such calls ("NAS calls held", the build page), a call
fails at once, so stuck threads never pile up. `nasread` keeps no threads of its own: each of its
calls is a store::deadline call, under `nasread::DEADLINE` (`deadline::SHORT`) but the planner's
keys (`with_big`):

- **Reads** (`read`, `read_to_string`, `with`): the plan's and the page's NAS files, the history
  and the forecasts' archives, the latency probe (`probe`).
- **The pool's NAS** (`Deadlined`, which the agent opens its member over): the driver's reads of
  the declaration and the one path, and its saves of the records and their brief.
- **Writes** (`write`): the archives' appends. A write
  to a path (and a `Deadlined` write or create) is refused at once (a timeout) while an earlier
  write to it is held in the kernel: a late write would land over the newer. The path is free
  again once that write ends.

A healthy share's calls are unchanged.

The agent is in parts (docs/agent-parts.md, task #871), each on a thread of its own, so no slow
duty holds another: **control** takes the owner's asks, **beat** writes the heartbeat, **nas** owns
the mount, **jobs**' thread reaps and cancels, the lead's **plan** plans (crate::agent::plan), and
the round (`Agent::round`, **jobs**' own: its slots, guards, the pool's step, grants and
starts) never plans: it grants from the last plan. One stall threshold holds them,
`beat::LOOP_STUCK` (5 min): a round or a plan past it is a stuck loop in the status (`loop_age_s`,
`loop_phase`: the round's phase, or "the plan: <its phase>"), and a plan running past it is logged with its phase and the agent
sampled by `/usr/bin/sample` for 5 s into `plan-stall-sample-<UTC date>-<time>Z.txt` in the agent's
folder (once per plan; the newest five kept; never two at once; a sampler still running at 300 s is
killed), and the status says "planning has taken N min so far" while it lasts, grants going on from
the last plan meanwhile. The round names each phase as it enters it and logs one that took 10 s or
more as it ends, but the wait between rounds ("the loop's phase \"the grants: the starts\" took 12 s");
the plan's thread does the same with its own phases ("the plan's phase \"build::plan\" took …"), each
part's kept apart (`nasread::LOOP`, `nasread::PLAN`), so a plan's phase never ends or stretches the
round's. These lines carry the time of day (UTC).

The heartbeat (the agent's `status.json`, or a helper's `helper.json`, in its folder) is not
the round's to write: the agent's beat writes it (crate::agent::beat, docs/agent-parts.md §4.2,
task #871) every 10 s on a timer of its own and at once on a change, from the view the round last
published, so a long round or a hung share leaves it beating (and the plan and the checklist,
task #795's 94–212 s and 261 s, are off the round altogether). Its content is as of the loop's last view, with its age (`loop_age_s`); its
time says the agent is alive. The beat is its only writer, so no beat
lands over a newer one (each a whole file put in place: a temporary file and a rename; written in
place, a reader could read it half-written). No copy goes to the NAS (task #1119): the lead's is
asked of its coordinator (`/pool/view`'s `status`, `coordwire::lead::status`) by `scenic status`,
`scenic pool status` and the map servers. A heartbeat goes out of touch when the beat itself stops (ten minutes,
`OUT_OF_TOUCH_S`, served as the status's `out_of_touch_s`). (The pool's heartbeat is kept by the
pool's own thread, §6.7.) A member's beat goes to the lead too (`POST /pool/beat`, its own
poster thread: docs/coordination.md §5.1), and nowhere else.

The NAS's mount and probe are the agent's **nas** part's (crate::agent::nas, docs/agent-parts.md
§4.3): on a thread of its own, each look (find, mount every 5 min at most, probe) on a worker
waited for 30 s; past it the round says "the NAS is mounting (stuck since …)", only NAS work waits,
and the loop, the beat and control go on. The planner's keys (`pool::records::Keys::load_with`) are read
under a 5-min deadline (`nasread::with_big`).

The plan's calls (the records, the held report, the helpers, the plan's digest) are on its
thread's roots, each within its deadline: a hung share holds the plan's thread for a deadline a
call at most, its last plan staying the one granted from meanwhile.

### 7.9 The forecasts' archive

Every forecast the lead makes is kept, so its times can be held to what happened (task #621;
`agent::fcarchive`, the record in docs/formats.md, The forecasts' archive):

- **What:** a line per forecast: when it was made and its finish; every target its expected run
  placed (lane, step, target, from and until); and what shaped it: each lane (its Mac, speed,
  time left on its job under way), each Mac's slots, room, conditions (its last ask's memory and
  cores free, limits, in use, on power, battery, NAS) and what held it back over the day, each
  step's time a target and its pace against its costs (code 2 on, below), the NAS budget,
  `forecast::VERSION` (raised whenever a change moves the run's times) and the app.
- **Where:** `state/coord/forecasts/<day>.jsonl.zst`, one file a day (UTC), lines appended in
  batches, each batch one zstd frame. Handed over only by the lead's pool step: the forecast's
  own thread makes the entry, the lead's pool step hands it to the archive's thread (a bounded
  channel, never waiting: full while the thread waits on the NAS, the entry is dropped, said;
  nothing on the NAS under the pool's lock), and the thread appends a batch once 30 wait or the oldest has waited
  10 minutes (a few appends an hour; one write each, under `nasread`'s write deadline). A batch
  that fails is kept for the next (300 at most, the oldest dropped, said); one waiting when the
  agent stops is lost (10 minutes at most). A lead process just replaced (a restart) may still append its
  last batch beside the next one's, and appends over SMB aren't atomic: a reader skips a frame that
  doesn't read and resumes at the next frame's magic number, so a crossed append costs its frames,
  never the day.
- **Not twice:** a forecast whose schedule, finish, lanes, Macs (but their passing conditions),
  each step's time and code version are those of the last one written, its times in the same
  2-minute buckets (`fcarchive::GRAIN`), isn't written (a time either side of a bucket's edge
  differs: a repeat is sometimes written, a changed forecast never skipped).
- **Kept for good:** the `gc` job's daily duties pack each day the history's window no longer holds
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

**`scenic forecast-check [--from T] [--to T] [--replay [--split T]] [--json] [--root DIR]`**
(`agent::fccheck`, reads only) holds the forecasts made in the span (default the last 7 days; T unix
seconds, `YYYY-MM-DD[THH:MM]` UTC, or back from now: `12h`, `3d`) to the history's jobs that ended
in it (the history's events once each: a member that led before keeps the same events in its own
file, `fccheck::dedup`). A job's targets in one forecast are taken together (its forecast start the
earliest of theirs, its end the latest), so each (forecast, job) is a sample: its end's error where
the job ended after the forecast; where it started after it, its start's (the start delay's
error) and its run's (its start to its end; task #988), so the end's error is the two summed; the
error forecast less actual (+ the forecast was late). It reports, for the end, the start and the
run, n, bias (the mean), and the median and 90th percentile of the absolute error, overall and by
horizon (the forecast's time until it, a run's until its end: under 15 min, to 1 h, 4 h, 12 h,
more), by step, by the Mac that ran it and by code version; how many jobs some forecast placed, how often it had the right Mac and the right slot
(its lane's name the history's worker); the **order**: of each forecast's pairs of jobs that both
started after it (neither at the same time, nor forecast to), the share that started in the
forecast's order, overall and of the pairs both forecast to start within 4 h (`fccheck::NEAR_S`);
the targets forecast to be done by the history's end that weren't, by step; and the **work**
(`Report::work`): over the forecasts made 6 h or more before the history's end (`fccheck::WORK_S`),
the targets each placed to end in its first 6 h against the targets the jobs that ended then did,
by step, summed. As text, a table in
minutes; `--json`, the same in seconds.

**The forecast inputs' archive** (`agent::fcinputs`, task #933; the record in docs/formats.md, The
forecast inputs' archive): what the backtest (below) reruns, the forecast's whole input
(`forecast::Record`, §10) by its lead's epoch (the key `term`) and time, on a write path of its
own. Inputs before the archive are the terms' `state/build/term/<E>/forecast-input.json` (to 31
Oct 13:35 UTC, term 673); those between then and the archive's start weren't kept anywhere.

- **What:** an input at most every 15 minutes (`fcinputs::EVERY_S`, the shortest horizon the check
  reports, `fccheck::HORIZONS`): the first forecast's after it. A replay costs ~9 s an input on
  the M4 (below), so the forecast a minute the lead makes would be more than a backtest can rerun.
- **Where:** `state/coord/forecast-inputs/<day>.jsonl.zst`, one file a day (UTC). Handed over with
  the forecast's entry to the forecasts' archive's thread (above: never waiting, nothing on the
  NAS under the pool's lock or in a coordinator's handler), which appends a frame of the inputs
  waiting once they hold 64 MB of JSON (the frame's window, `fcinputs::FRAME_RAW`) or the oldest
  has waited an hour (4 inputs): one append an hour. The frame's long-distance matching stores an
  input mostly as what changed since the ones before it: measured on terms 668-671's four inputs
  (31 Oct 10:05-13:20, 46.6 MB of JSON, 3.8 MB as the terms' files held them; `fcinputs`'
  `recorded_frame_size`), one frame of 0.97 MB: about 23 MB a day. A frame that fails is tried
  again after an hour (the inputs waiting past 64 MB of JSON dropped, oldest first, said; one
  whose write landed after its deadline is then there twice, read once); the
  frame waiting when the agent stops is lost (an hour at most), as the forecasts' batch is.
- **Kept:** 7 days (`fcinputs::KEEP_S`, the span `forecast-check` reads by default,
  `fccheck::SPAN_S`): the `gc` job's daily duties remove the days before it (8 day files, about 190 MB).

**The backtest** (`--replay`, `fccheck::backtest`): each forecast input made in the span (the
archive's, and the terms' at times it hasn't: `fccheck::inputs`), read one at a time as it's rerun,
is rerun twice,
as the code before made it (old) and as the code is (new), each archived as the lead archives its
forecasts (`Entry::of`) and checked as above (`fccheck::replay`). Both: the costs noted from the
inputs before it and its own (`forecast::Noted`), each step's pace from the history before it
(its workers' speeds its lanes', their Mac's code 2 hold taken off), each lane's job under way
capped (below; a record of code 2 on holds its `busy_s` capped already), the record's lanes, each
lane's speed with its Mac's code 2 hold taken off, the build's pause and each Mac's time by
step read from the history (code 3, below), and each Mac's pace at each step read from the history
(`fccheck::repace`, code 4 below), the record's cost table made again from its scheduler's
measures as the lead makes it now (`fccheck::recost`: a target known by its own run of the way its
step runs now, Batch length in §7.7). Old and new differ by what the code now does and the one before
didn't (`Input::code`): the tables below ran their own code's pair. The inputs from `--split` (default a day before the last) are the test
set, scored apart from those before it (train): what the model was chosen on is never what it's
scored on. The terms' files held a term's last input alone (81 over 24-27 Oct: a run took ~12 min
on the M4, each input's three runs, twice, each to `forecast::BUDGET`); the archive holds 96 a
day. Run it on a copy of `state/coord` and `state/build/term` (`--root`), or on the NAS (reads
only).

**What the archive showed** (24 Oct 00:00 to 27 Oct 18:40 UTC: 1,175 forecasts, all code 1, held
to 2,082 jobs; 303,000 (forecast, job) samples). Changes to the run since #621 (25 Oct: its budget; 26 Oct:
task #688's cores in the scheduler it asks) left `VERSION` at 1, so the archive can't tell them
apart. End error, median / p90 of the absolute error (min): all 917 / 7,825;
under 15 min 496 / 1,816 (bias −696: jobs forecast to start now started hours later); 15 min-1 h
110; 1-4 h 163; 4-12 h 324; 12 h + 2,610 / 12,204 (bias +4,331: the jobs the pool did in a day or
two were placed days out). The right Mac 50 % of the time, the right slot 14 %; jobs in the
forecast's order 66 % of pairs. Its largest sources:

- **The work placed isn't the work done.** 2.5 million targets forecast to be done by the history's
  end weren't: terrain-deep 1.65 M, terrain-fine 0.31 M. In its first 6 h a forecast ran ~65 lane-hours
  of work; the pool ran ~19.5 (3.25 jobs at once, no job at all 19 % of the minutes, from 25 Oct
  12:00): terrain-deep on the MacBook Pro 15.4 lane-hours against 0.5. Work the pool
  did soon (terrain-hi on the MacBook Pro, pack) waited behind it in the forecast: the 12 h +
  lateness. The pool was paused from 24 Oct 08:09 to 25 Oct 07:39; the forecasts made then placed
  75-119 lane-hours a day of terrain-fine that never ran.
- **Running jobs' time left.** A lane's `busy_s` is its job's targets left at its slot's pace (an
  unmeasured slot's 0.21 a guess, its Mac's hold on top) or the job's own estimate, the longer: of
  1,022 lanes said busy 4 h or more, those whose job the history shows were free after a quarter of
  that or less (median), those said busy a day or more after 0.2 %; nearly all the MacBook Pro's.
  Its lanes sat blocked in the run meanwhile.
- **Steps' costs.** A target's cost is its own run of the way its step runs now (at its worker's
  pace) or its step's mean; jobs started within 30 min of a forecast took, of the time the forecast gave
  them (geometric means): terrain-hi 0.47, terrain-deep 0.38, unit 0.41, bldtiles 0.16; pack 2.5,
  pois 2.4, terrain-lo 1.9 (what a job takes whatever its targets: few targets each). It drifts
  by the half day (terrain-deep 0.13 to 1.21).
- **Starts forecast now.** Of the starts forecast within 15 min, two thirds (the median forecast's)
  began over an hour later; pack's forecast on the MacBook Pro, 95 % of the time. Each Mac ran no job
  25-76 % of each day.

**What changed (code 2).** Two things, each in the record (`Input::step_pace`, a lane's capped
`busy_s`) so a member's page reruns the same, and archived (`Entry::step_pace`):

- **Each step's pace** (`forecast::step_pace`, `Noted`): the lead notes the cost each target had
  while it was work left (a target leaves the work left once built, so the measure its own job leaves
  never replaces it; kept 12 h after it was last seen; ~2 ms a forecast for 27,000 costs); each
  forecast, over the jobs done well in the last 12 h whose every target was noted, a step's time at
  the lead's pace (the job's seconds times its worker's speed) over its targets' noted costs, three
  jobs at least, is the step's pace, and the run takes each of its targets at its cost times it.
  Only a step faster than its costs (at most fourfold): one slower is so by what each job takes
  whatever its targets, which a target's pace lays on the run's jobs of many targets; with it, a backtest of the 80
  inputs of a few hours before had its test end error at 2,054 / 25,682 against the old 995 / 10,645,
  pack's own p90 9,163 to 23,049. A lead that has just started notes for a while before it says a pace.
- **A lane's job under way held to its step's longest job of the last 12 h** (`cap_busy`,
  `longest_jobs`; three jobs at least).

Tried and left out: a Mac's lanes past those it ran at once (`Held::lanes`) kept for their job alone,
taking nothing after: the near horizons gained (under 15 min median 17 to 12, the order within 4 h
76 to 79 %), 12 h + lost (median 6,187 to 6,775) and so did the whole.

With code 2, #805's backtest (code 1 against 2, same inputs and split) had the test end error at
580 / 10,001 against 592 / 10,045, train 2,567 / 20,762 against 3,619 / 28,375.

**What code 2 still got wrong** (task #808: the code 2 replay of the 81 inputs, its forecasts'
first 6 h against the jobs done then). The largest error was where the work went, not how long it
took:

- **Each Mac's steps.** The pool's two working Macs did different work: on 26-27 Oct the MacBook
  Pro's lanes gave their time to terrain-hi (42 lane-hours of a day), terrain-deep and units, the
  Mac mini's to pack, pois, units and terrain-deep (26 h); the MacBook Pro ran 8 pack targets in a
  day against the mini's 298, terrain-hi none on the mini. The run's scheduler, its Macs all asking
  at once with their room free, gave any step to any Mac: in the test set it placed 1,431 pack and
  657 unit targets on the MacBook Pro in forecasts' first 6 h against 3 and 3 it did, while
  terrain-hi, which it did (1,842), waited 65 h (median) behind them. The right Mac 54 % of the
  time.
- **Slowed lanes.** Each lane ran at its Mac's share of busy time (`Held::util`, 0.25-1), but jobs
  took less than their costs (§ above) and the work the pool did soon was already forecast late
  (test end bias +3,455 min): slowing the lanes placed it later still. Without it, the test set's
  first 6 h placed 10,354 targets against 10,523 done (6,503 with it).
- **A paused build.** Its forecasts placed work as if it ran (24 Oct 08:09 to 25 Oct 07:39:
  75-119 lane-hours a day of terrain-fine that never ran).
- **A Mac asleep or away** (the MacBook-Pro-de-Brandon, the M1): with no job in the day it had no
  hold, so its lanes took work at full room. In the inputs of 24-27 Oct it has no lanes (out of
  the pool), so the backtest doesn't measure this; the step rule below gives it none of a step
  the others did.

**What changed (code 3).** Each in the record (`Input::paused`, the skips, a lane's speed) so a
member's page reruns the same:

- **The build paused, nothing placed** (`Input::paused`, the lead's pause): the forecast says
  `forecast::PAUSED` and places nothing (no finish, no lanes); the replay reads the pause from
  the history's pause and resume (`forecast::paused`).
- **Each Mac takes the steps it took**, a skip of a step it gave under a tenth of the day's time,
  and **lanes cut, not slowed**: both removed in code 5 (task #951: they copied outcomes, not their
  causes, and lagged a fix by up to a day).

Tried and left out: offering each Mac's steps in the order of its time on them (terrain-hi's test
median 4,580 to 210 min, but pack's 4,228 to 10,066 and terrain-lo's 184 to 2,872: the order of
the plan's work matters more than a Mac's habit); the busy share over all unpaused minutes rather
than the pool's working ones (slower still: the whole and its main steps worse).

**The backtest, code 2 against 3** (81 inputs, test from 26 Oct 18:37 UTC: 28 inputs, 634 jobs,
4,654 end samples; train: 53 inputs, 6,012 jobs). Minutes, median / p90 of the absolute error:

| | test old | test new | train old | train new |
|---|---|---|---|---|
| end, all | 614 / 10,394 | 566 / 8,097 | 2,491 / 21,734 | 1,054 / 10,538 |
| end bias | +3,455 | +2,675 | +11,585 | +4,888 |
| start, all | 589 / 10,367 | 519 / 8,099 | 2,458 / 21,706 | 1,036 / 10,304 |
| end < 15 min | 9 / 198 | 9 / 155 | 1,133 / 1,909 | 1,569 / 2,538 |
| end 15 min-1 h | 33 / 330 | 49 / 323 | 163 / 1,860 | 103 / 1,844 |
| end 1-4 h | 159 / 612 | 199 / 668 | 211 / 1,139 | 102 / 441 |
| end 4-12 h | 192 / 423 | 206 / 405 | 271 / 1,372 | 259 / 980 |
| end 12 h + | 5,840 / 12,286 | 4,855 / 10,335 | 5,192 / 37,917 | 3,618 / 34,264 |
| end, pack | 4,228 / 8,192 | 3,092 / 6,561 | 1,726 / 18,284 | 1,682 / 9,712 |
| end, terrain-hi | 4,580 / 8,186 | 3,624 / 6,549 | 6,485 / 37,898 | 4,651 / 33,455 |
| end, terrain-deep | 198 / 588 | 203 / 529 | 486 / 2,614 | 428 / 2,539 |
| end, unit | 3,664 / 8,408 | 2,733 / 7,260 | 3,446 / 36,946 | 1,741 / 10,758 |
| end, depth | 14,206 / 18,234 | 11,330 / 14,613 | 3,117 / 35,814 | 2,455 / 33,136 |
| end, bldtiles | 10,761 / 16,299 | 8,327 / 12,964 | 383 / 8,689 | 324 / 6,494 |
| end, slope-hi | 10,827 / 14,512 | 8,762 / 11,707 | 8,574 / 32,372 | 7,891 / 29,795 |
| end, terrain-lo | 184 / 3,666 | 195 / 2,898 | 6,901 / 69,828 | 716 / 35,031 |
| end, trees | 11,702 / 15,959 | 9,332 / 13,903 | 7,480 / 25,519 | 6,845 / 18,365 |
| end, pois | 9,927 / 14,880 | 8,107 / 12,927 | 1,513 / 166,612 | 1,513 / 47,958 |
| placed in 6 h / done | 6,503 / 10,523 | 9,142 / 10,523 | 31,650 / 24,940 | 20,687 / 24,940 |
| not done by the end | 22,111 | 29,369 | 171,799 | 162,244 |
| same Mac / slot | 54 / 18 % | 55 / 20 % | 43 / 20 % | 65 / 38 % |
| in order (all / within 4 h) | 65 / 77 % | 65 / 81 % | 58 / 51 % | 62 / 49 % |

Better overall on both sets, and for every step of note on both but terrain-deep on the test set
(median 5 min worse, p90 60 better) and terrain-lo there (70 samples, median 11 min worse, p90 770
better). On train the end < 15 min rows are worse (n 982 to 393: inputs paused then place nothing). Worse
from 15 min to 12 h ahead (median 16-40 min): work brought forward from days out now crowds the
near term. The test's targets forecast done that weren't rose (22,111 to 29,369): it now places as
much in its first 6 h as the pool did, but not all of the same targets. What's left: the order of
the plan's work (pois, slope-hi, trees and depth done the next day, forecast days out); the steps
on the lead's own clock (catalog, redo, inputs: a tenth of the targets done) placed nowhere; a
running job's time left capped, not measured.

**What changed (code 4, task #926).** A lane ran every step at its Mac's one speed (`speeds`: the
median over the shared steps of its time a target against the lead's), and a measured cost was put
at the lead's pace by that speed; but a Mac's pace differs by step (the same targets on the M4
against the mini over the week to 4 Nov: 1.39 at terrain-deep, 2.2 at terrain-hi, 4.6 at pois).
Each Mac's pace at each step (Each Mac's pace, §7.7: in the scheduler's state, so in the record)
now puts each measured cost at the lead's pace (`forecast::at_lead`, each step's mean by
`step_sums`), a lane takes an item at its Mac's pace at the item's step (`Input::lane_secs`), and
a member's lane's job under way is timed at it; a Mac without one is at the lead's pace, the lead's
other slots too (with no paces at all, a record before code 4, each at its speed as before). Each step's pace (code 2's) reads a job's time at the
lead's pace by its Mac's pace at the step too. The replay of a record before it works the paces
out from the history at the record's time and puts the record's shared steps' costs at the lead's
pace by them from the record's own measures (`repace`, by `recost`: a target's own run of the way
its step runs now, else its step's mean; a cost the record holds no measure of stays).

**The backtest, code 3 against 4** (69 inputs, 30 Oct 00:00 to 1 Nov; test from 31 Oct 00:00 UTC:
29 inputs, 1,154 jobs, 15,748 end samples; train: 40 inputs, 2,214 jobs). Minutes, median / p90 of
the absolute error:

| | test old | test new | train old | train new |
|---|---|---|---|---|
| end, all | 540 / 3,010 | 397 / 2,591 | 572 / 2,183 | 608 / 2,077 |
| end bias | +998 | +766 | +260 | +221 |
| start, all | 440 / 2,853 | 366 / 2,505 | 593 / 2,120 | 628 / 2,027 |
| end < 15 min | 44 / 455 | 52 / 460 | 45 / 292 | 59 / 647 |
| end 15 min-1 h | 65 / 501 | 76 / 504 | 198 / 663 | 196 / 995 |
| end 1-4 h | 183 / 587 | 184 / 573 | 279 / 859 | 297 / 887 |
| end 4-12 h | 242 / 582 | 234 / 563 | 424 / 1,228 | 424 / 1,392 |
| end 12 h + | 1,386 / 3,917 | 1,214 / 3,880 | 1,064 / 2,630 | 1,122 / 2,495 |
| end, terrain-deep | 443 / 1,501 | 283 / 1,001 | 447 / 1,960 | 473 / 1,771 |
| end, depth | 288 / 796 | 273 / 780 | 631 / 1,303 | 625 / 1,427 |
| end, pack | 310 / 2,305 | 301 / 1,498 | 454 / 1,982 | 448 / 1,774 |
| end, unit | 238 / 1,905 | 253 / 1,451 | 417 / 1,556 | 403 / 1,445 |
| end, slope-deep | 2,633 / 5,395 | 2,208 / 4,414 | 1,867 / 3,017 | 1,891 / 3,066 |
| end, slope-hi | 2,499 / 3,355 | 1,725 / 2,596 | 1,646 / 2,420 | 1,526 / 2,560 |
| end, terrain-lo | 317 / 2,145 | 331 / 1,951 | 1,158 / 2,149 | 1,199 / 2,269 |
| end, pois | 296 / 476 | 250 / 422 | 292 / 825 | 316 / 1,072 |
| placed in 6 h / done | 43,685 / 39,021 | 52,061 / 39,021 | 93,865 / 43,358 | 96,778 / 43,358 |
| same Mac / slot | 43 / 16 % | 44 / 16 % | 46 / 13 % | 48 / 15 % |
| in order (all / within 4 h) | 62 / 74 % | 62 / 72 % | 70 / 67 % | 70 / 66 % |

On the test set the whole end error is better (median 143 min less, p90 419), its bias 232 min
less late, and every step of note at the p90 (terrain-lo's and unit's medians a little worse);
the nearest hour's medians are 8-11 min worse. On train the p90 and bias are better, the
median 36 min worse, and the near term (under an hour) and 4-12 h worse at the p90 (what
moves them there isn't known). Paces from pairs are near the lead's where pairs are few
(drawn toward 1), but code 4 moves every lane whose Mac's speed wasn't 1: with paces worked out, no
lane runs at its speed any more, and a Mac of few or no pairs runs at about the lead's pace.

**The near term, measured again (task #934, 6 Nov).** The p90 of the end error under 15 min, in
minutes. Live (`forecast-check` over each code's own span, the forecasts as the lead made them):
code 3 (2 Nov 07:00 to 4 Nov 10:50 UTC, 10,800 samples) 284, code 4 (to 5 Nov 23:34, 2,365) 76,
code 5 (to 6 Nov 16:07, 4,299) 300. The three spans ran different work, so these don't weigh the
codes against each other (the replay below does). What the error is made of (task #934's review,
the archived forecasts of code 5's span joined to the history by hand, a sample a forecast and job,
5,504 under 15 min): the end's p90 224, the start's lateness 222, the job's own time against its
forecast 14. A target forecast to run within 15 min ran hours later, or not by the history's end;
it didn't run slower. Most were bldtiles (p90 332, 1,303 samples) and depth (369, 289): the
forecasts placed about 27 depth targets to end in their first 15 min, where the pool did about 2 a
quarter hour then, mostly terrain-hi instead. Code 4's span had little of either (bldtiles 497
samples, p90 150; depth 63, 26). So the near term's error is which targets run next, not how fast
a Mac runs them (the paces' part, the job's time, erred 14 at p90). Replayed on the inputs' archive (`--replay --from 2026-11-04T22:00
--split 2026-11-05T22:00`, release build on the M1, 2.3-2.8 GB): code 3 against 4, train 171 to
172 (n 403 to 440), test 506 to 447 (n 893 to 847); code 4 against 5, train 172 to 138, test 449 to
459. So no near-term loss from the paces since the archive began. Replayed on #926's own window
(30 Oct to 1 Nov, split 31 Oct, the terms' inputs) it is still there, train 557 to 808, test 386
to 520; with each Mac the paces don't know at its recorded lane speed instead of the lead's 1, 767
and 520: the dropped speed fallback isn't its cause. What is isn't known; it doesn't recur on the
inputs archived since.

**What changed (code 5, task #951).** The owner's spec (4 Nov): on 4 Nov the schedule showed
almost all of slope-deep left on the M1 while all three Macs ran it. The run had every Mac ask
with its room as its last ask said (one moment's: a compile's spike, a 10.2 GB margin making the
M4 look full), and #808's skips gave a Mac none of a step it had done little of over the day
before, whether it couldn't or wouldn't (the mini, its memory fixed by #931/#935, still got no
slope-deep). Now (§7.2):

- **Pulled, as live:** a Mac asks only as one of its lanes frees, for a job each of its free lanes,
  one plan order for every ask; the lead's first lane's order of its own is gone.
- **Each Mac at its sustained best** from now on: its memory for jobs and its cores not used
  outside the build, the highest held for 30 minutes over 48 hours (the owner's figures), from its
  live minute ring, carried in its ask (`Capacity::best`). The replay works it out from the
  archived inputs' asks before each (`fccheck`'s `Rooms`: each input's room, a run broken where
  an input was lost), so a run needs three inputs at least.
- **Gone:** the skips (`took`, `SKIP_SHARE`, `SKIP_KEEP_H`, `SKIP_FOR_S`) and the day before's hold
  on each Mac's lanes (`held_back`).
- **Readiness through the plan:** a target the plan holds because an input of it can't be worked
  out yet waits for what works it out (the slope tiles reading the terrain's root: 5 Nov, slope-hi
  6/0/48 and 6/15/48 were granted to the M1 while they waited on terrain-root).

Two faults of the run the review found (both in code 5's own run, so code 4's column replays them
fixed too):

- **A step measured only its old way ran one job at a time.** Its targets' measures were all of
  an older `cost_version` (slope-deep's, raised by #929 on 5 Nov), so each was a cold start, one
  of its step at a time in the pool; the run left a job's measures as it ended only for a step with
  no measure at all, and kept a stale one. Now a job of a step with none of the way it runs now
  leaves its targets' measures, replacing old ones, as a live hand-off does (`Pool::learn`,
  `forecast::tests::a_step_measured_only_its_old_way_fans_out_after_its_first_job`). Before, the
  replays of 4-5 Nov put nearly all slope-deep on the lead's first lane, the other lanes idle: the
  late bias.
- **The replay ran code 5 on the day before's lanes.** A record made before code 5 has each Mac's
  lanes cut by the hold; the replay now gives each Mac back its slots, as code 5's lead makes them
  (`fccheck`'s `unhold`): some 30 lanes where the records had 3 to 5.

**The backtest, before against after** (`forecast-check --replay --from 2026-11-04 --to
2026-11-05T16:00 --split 2026-11-05T08:00`, M4, release build so no run is cut at its budget; the
inputs archived since #933: 24 train, 15 test; code 4 the run with each Mac's room as it asked and
the record's lanes; code 5 each Mac at its sustained best on its slots; both packing with the room
held for a big job, #965's `Sched::pack`). Minutes; median / p90 of the absolute end error:

| | test, code 4 | test, code 5 | train, code 4 | train, code 5 |
|---|---|---|---|---|
| right Mac | 44 % | 48 % | 49 % | 47 % |
| end, all | 65 / 358 | 52 / 113 | 119 / 417 | 156 / 421 |
| end bias | +139 | +17 | -128 | -187 |
| end, slope-deep | 49 / 168 | 14 / 62 | 120 / 416 | 157 / 421 |
| end, bldtiles | 91 / 307 | 56 / 107 | | |
| end, depth | 596 / 2,679 | 184 / 532 | | |

On the test set code 5 is better everywhere, the right Mac too. On train it is
early (-187 min): a best case, each Mac on all its slots at its best room, where that night's pool
ran about one job a Mac (the M4's cores taken by test builds, the M1's memory at 3-5 GB). The 4 Nov
case itself is before the inputs' archive (it begins 4 Nov 22:40 UTC), so a test replays it
(`forecast::tests::each_mac_runs_at_its_sustained_best_room`): the mini's floor lowered at noon,
its best room follows within the half hour and the run gives it its share of slope-deep then,
where the skips held it to none for the day.

**Its cost.** With each Mac on its slots the lead's run has some 30 lanes where the hold left 3 to
5, and each ask does more: the live-scale test's three runs took 132 s of CPU against 85 s at the
code before (M4, test build, under load); the replay's debug build cut 11 of its 39 code 5 runs at
the 15 s budget, its release build none.

**What held the near term back (task #988).** `forecast-check` over code 5's archived forecasts
(5 Nov 23:34 to 7 Nov 00:40 UTC), its samples under 15 min: the end's p90 247 min, the start's 260
(by its own horizon) and the run's 11: the start delay, not the run. Each forecast held to the input
archived just before it (the plan's offers and the scheduler's NAS waits then), of the targets it
placed to start within 15 min: depth 394 not offered by the plan (44 offered), peaks 192 not
offered, pack 52, bldprep 29; bldtiles 55 passed over for the NAS budget (137 offered and free),
slope-hi 33, bldprep 28. The plan held the depth because a target waits while a target it reads
waits (docs/readiness.md 3b, `held`): a depth tile reads the lake-depth packs of the tiles its
sources meet, built as they stand, and those read the terrain left under them. The forecast stopped
at such a read: it listed only the work left, so a read built as it stands waited for nothing, and
it ran the depth at once (22:51 on 6 Nov: 39 depth tiles placed in the first 15 min, the plan
offering none; 40 depth tiles and 48 lake-depth tiles ready that the plan held). The NAS budget's
holds the forecast's scheduler reproduces: the lead's record of 22:51 asked as the mini asked
grants two bldprep jobs and holds the rest for the NAS's room, as the live lead did.

**What changed (code 6).** A target waits through a read that isn't listed but passes waits on
(`forecast::Waits`, `steps::read_targets_passing`): for what that read waits on, as the planner's
`readiness` holds it (`held`, and a step read whole none of whose targets is listed, `whole_left`:
`steps::read_steps`). The 22:51 record's chains then wait as the plan offers them (no depth ready
against none offered, lake-depth 433 ready against 433 offered). The read itself, made again once
what it reads is built, isn't placed (its job's time goes uncounted). Its cost: a read is worked out
once a forecast, those that wait on nothing kept as their hash (6 Nov: some 1 M slope-deep parts the
slope-hi tiles read, 2,300 waiting), a step none of whose reads may wait not looked into; the
forecast lists the work once for its three runs (`forecast_by`, each a copy), where each run
listed it again (11 s a listing in the test build on 6 Nov's record, 16 s with the waits; in the
release build on the M1, 7 Nov 02:34 UTC's record of 20,613 items 9.6 s to 12.5 s, and four train
records of 5 to 6 Nov 12 s to 17-18 s). The listing is outside the runs' `BUDGET`, which bounds each
run's CPU alone: the forecast takes about 3-6 s more on the wall, its three runs as cut as before
(that record's under either code at 15 s, partial; 17 of the 40 train inputs' replays under
either). The
backtest runs its two codes one after the other (side by side its replay of 60 inputs on the M1
peaked at a 4.7 GB footprint).

**The runs across a restart.** The NAS's history held no coordinator start ("agent" events: the
lead's agent appended its coordinator's events only from its first pass on), so
`history::mend_runs` mended nothing there: forecast-check and the backtest's inputs (each step's
pace, the longest jobs, each Mac's pace) took 378 runs over 5 to 7 Nov as written, 123 hours short.
The lead's agent now appends its coordinator's start with its first pass (§7.5). The backtest
below ran on a copy of the archive with the lead's own starts added.

**The backtest, code 5 against 6** (`forecast-check --replay --from 2026-11-05T12:00 --to
2026-11-07T00:40 --split 2026-11-06T06:00`, release build on the M1, 49 min, a 4.0 GB peak
footprint; 40 train inputs, 20 test: the archive lost the inputs of 6 Nov 09:30-19:18). Minutes;
median / p90 of the absolute error:

| | test, code 5 | test, code 6 | train, code 5 | train, code 6 |
|---|---|---|---|---|
| end, all | 185 / 458 | 191 / 430 | 206 / 6,517 | 193 / 7,010 |
| end < 15 min | 72 / 296 | 51 / 280 | 98 / 548 | 105 / 582 |
| end 15 min-1 h | 114 / 269 | 111 / 242 | 94 / 620 | 90 / 592 |
| start < 15 min | 82 / 288 | 72 / 276 | 114 / 561 | 133 / 581 |
| start 15 min-1 h | 128 / 283 | 131 / 267 | 100 / 620 | 95 / 581 |
| run < 15 min | 1 / 9 | 1 / 9 | 0 / 5 | 0 / 5 |
| end, depth | 112 / 463 | 68 / 152 | 332 / 1,254 | 325 / 1,225 |
| end, bldtiles | 265 / 618 | 241 / 547 | 121 / 1,148 | 130 / 1,122 |
| in order within 4 h | 67 % | 71 % | 64 % | 64 % |

On the test set the near term is better (under 15 min the end's median 72 to 51, p90 296 to 280;
15 min to 1 h p90 269 to 242), the depth most (p90 463 to 152), and the whole end's p90 (458 to
430). On train the hour ahead is mixed: under 15 min worse (end p90 548 to 582, start 561 to 581),
15 min to 1 h better (620 to 592), the whole end's p90 worse (6,517 to 7,010) where its median is
better (206 to 193). The train set's near term is worse without any target waiting more than the
plan holds it (task #988's review): over five of its inputs, code 6 gives no target the plan
offered a wait code 5 didn't, and of the targets placed to start within 15 min that the plan didn't
offer, the peaks go from 3,862 to none and the depth from 65 to 26. The lanes those held targets
took go in code 6's run to work the plan offered but the live pool started later: bldtiles placed
within 15 min of the plan's offers 503 to 651, bldprep 763 to 1,018, depth 237 to 319, each started
early already (bldtiles' start under 15 min: bias -312 to -332 min, p90 656 to 687; n 469 to 564),
while the pois' start p90 within 15 min to 1 h goes from 918 to 217. Why they started later: task
#993, below. What's left near: the run's error stays under 10 min at p90; the start's is the order
and room of the work (the replay's near-term p90 is above the live forecasts' 247 on the same days).

**Was it the NAS budget? (task #993).** No. #988's review took those early starts for the budget's
holds, which the run was said not to model. It does: it asks `Sched::pack`, so its grants go through
`nas_room`, `job_nas` and the fits of §7.7, on the leases and limits recorded. The live budget seldom
bound: the pool's own NAS traffic (the lead's `pool_nas_*` series, 20-minute means, as weight) had a
median of 180 and a p90 of 738 of the 1,000 on the train set, 128 and 355 on the test set.

**Nor the room, nor the lanes' time left.** Measured on the span below, code 7's run at 15 s against the
history:

- **The run fills about the lanes the pool did:** lanes busy over a forecast's first 15 minutes, 7.3
  against 6.8 on train, 6.8 against 6.2 on test. Most of the Macs' asks have under 1.5 GB of room (57-63 %),
  but that is the room left past their jobs' needs more than a smaller Mac: what the jobs may hold together
  (`build_mb`) is under 70 % of the sustained best in 24-32 % of the asks that say one, and the memory brake is
  on in 6-11 %.
- **Each Mac as it asked gives about one job an input fewer at the start, half of it memory:** the jobs a
  run's first asks are granted, an input, with the sustained best 3.0 on train and 2.8 on test; with
  its memory as asked 2.4 and 2.0; with its cores as asked (its load outside the pool) 2.4 and 1.9;
  with both 2.1 and 1.5.
- **Replayed so, it doesn't hold on the test set:** each Mac's memory and cores as it asked while its
  jobs out at the record run, its best after (a throwaway run, each at 4 s a run; end under 15 min): train
  115 / 587 to 78 / 460, test 49 / 265 to 63 / 290; the start's bias stays -219 to -183 min on
  train, -100 to -111 on test. Its memory alone, at 15 s: train 105 / 582 to 70 / 472, test 50 / 280
  to 68 / 318. So the sustained best stays.
- **Each lane's job's true end** from the history in place of its `busy_s`, an upper bound: train 96 /
  568, test 49 / 279.
- **What's left is work the input doesn't hold:** of the lane time on the jobs the pool started in a
  forecast's first hour, about half went to jobs none of whose targets that forecast placed: 3.1
  lanes against 3.2 on the 23 train inputs whose run finished (polar-terrain, trees, terrain-hi,
  terrain-deep: what the plan listed only later; 22 of the 89 inputs of 5-6 Nov say `blind` and list no
  regions at all), 2.5 against 1.9 on test (cut runs, so some of it is the cut's).
  The placed work that lane time displaces starts late against the forecast. That's the plan's work to
  come, which no change to the run's room can show: a forecast is as near as its input is whole.

**What changed (code 7).** `Limits::held` is a reading of the NAS's disks now (§7.7). Recorded shut, it
stayed so for the whole run, every job that weighs anything going one at a time (two of the 113 inputs
archived, 5 Nov 19:36 and 19:53 UTC, their depth placed a day late). It now holds while the jobs out at
the record run; no passing load is foreseen after them, as for the Macs (§7.2). Those two inputs' end, code 6
against 7: 838 / 4,191 to 88 / 518.

Tried and left out: each job under way weighing what its beats said it moved, carried in the record
as the live scheduler weighs it. The archive holds no beats, so no replay can show it; with each job's rate
taken from the pool's traffic, split evenly among the jobs out (an upper bound, a throwaway run), on the
20 inputs with such waits, the end under 15 min is 55 / 430 to 54 / 419 on train, 64 / 280 to 72 / 280
on test. A field in every record wasn't worth that.

**The backtest, code 6 against 7** (the same command and copy as below, `--replay --from
2026-11-05T12:00 --to 2026-11-07T00:40 --split 2026-11-06T06:00`, release build on the M1, 51 min). Minutes; median / p90 of the absolute error:

| | test, code 6 | test, code 7 | train, code 6 | train, code 7 |
|---|---|---|---|---|
| end, all | 182 / 415 | 181 / 416 | 188 / 7,287 | 168 / 7,558 |
| end < 15 min | 51 / 280 | 51 / 280 | 105 / 582 | 108 / 582 |
| end 15 min-1 h | 111 / 243 | 111 / 242 | 90 / 593 | 92 / 593 |
| end 1-4 h | 235 / 410 | 234 / 411 | 125 / 686 | 115 / 688 |
| end 12 h + | 814 / 3,022 | 673 / 2,739 | 8,647 / 11,102 | 10,459 / 11,144 |
| start < 15 min | 72 / 276 | 72 / 276 | 133 / 581 | 133 / 581 |
| start 15 min-1 h | 131 / 267 | 131 / 267 | 94 / 593 | 95 / 593 |
| end, depth | 68 / 143 | 68 / 143 | 325 / 1,225 | 205 / 590 |
| end, bldtiles | 239 / 537 | 240 / 543 | 129 / 1,118 | 116 / 1,122 |
| end, bldprep | 21 / 84 | 21 / 84 | 154 / 298 | 156 / 299 |
| in order within 4 h | 70 % | 70 % | 63 % | 63 % |

The near term doesn't move, and train's under-15-minute p90 stays 582: the gate wasn't what raised
it, nor the budget. The depth on train gains most, from the two inputs whose gate was shut, its 221 samples
at 12 h + (error about 1,100) all coming within 1 to 4 h. That is why train's 12 h + row and its whole p90
read worse (8,647 to 10,459, 7,287 to 7,558): those samples left a row of larger errors, not that any got
worse. Elsewhere the two codes' columns differ only by where a cut run stops: bldtiles' p90 537 to 543 on
test is that.

**Where the forecast's time goes** (task #995; `sample` of `forecast::tests::archived_timing`, the archive's
inputs as the lead recorded them, release build on the M4, its load 20-90 from the pool's tests). A run
asks the scheduler some 50,000 times, 88 % of them refused: a lane idle for its room asks again at each
item's end anywhere in the pool. Each ask weighs every target of every offer (up to `OFFER_MAX` a step)
and every job its Mac holds, through the live scheduler's own `Sched::pack`. What it looked up again at
each ask, and now keeps:

- in the cost book (`needs::Book`), with its models while the measures stand: each step's 90th
  percentile time (`step_p90`, which scanned every cost, some 45,000, for each target with no measure of
  its own: 70 % of `pack`), each target's source's time (`source_secs`, its source's names worked out from
  the steps table each time, slope-hi's 256 deep packs among them), each `secs_on` asked, and each
  target's own run's NAS rate (`own_rate`); the models themselves behind an `Arc`;
- in the scheduler: each leased job's NAS weight and its packing need and time (`Sched::kept_leases`), and,
  within one `pack`, the NAS budget's sums and each step's targets held (`Sched::packing`, made again at
  each grant); a cold target's provisional need (`provisional_at`, as `measured_of` is);
- in the run: each Mac's offers, until the work ready changes (`Ready::rev`).

The per-target maps are looked up by FxHash (`roadcore::fx`), not SipHash. Each answer is the one the code
worked out before, as of the same measures: every forecast below is the same bytes before and after.

| 7 Nov 09:35 UTC's input (19,365 items, 30 lanes) | before | after |
|---|---|---|
| runs at `BUDGET` | cut, 640-1,146 items placed | whole, 7.3-8.5 s each |
| each run without a budget, wall | 607 s | 23 s, its listing (13 s) among it |

Over the 64 inputs of 6 Nov 00:50 to 7 Nov 11:41 UTC (all regions on), each run's CPU time at `BUDGET`:
before, 59 of the 60 cut (a median 4 % of their items placed); after, none cut, the slowest run of each
input a median 6.5 s, p90 8.4 s, at most 12.3 s (7 Nov 05:10 UTC's input, 20,300 items). Its listing, once
for the three runs and outside the budget, a median 21 s before and 13 s after, at most 18 s: most of it
`steps::declared` making and sorting the names of what each target reads. The same forecast (its JSON,
`SCENIC_FORECAST_HASH`) at no budget before and after: the 61 inputs of 4 Nov 21:43 to 5 Nov 23:56 UTC (the
slowest 206 s before, 7 s after) and four with all regions on (6 Nov 19:18-19:50 and 7 Nov 09:35 UTC: 383-607
s before, 23-29 s after). `forecast-check --replay` of those 61, `BUDGET` raised in both
copies: the same report byte for byte (codes 6 and 7, train and test), 46 minutes before and 7 after.

## 8. Tasks

A job is a step's run over targets on a Mac with the NAS. A **task** is a pure piece a running job
offers: programs over a folder of files, giving files back (kinds and protocol: coordinator.md §3, §5).
Any member's idle slot takes it (`scenic run-task`). Pages take none (#875: their asks are refused,
coordinator.md §5).

- **Brokers** (`crate::agent::peers`, `crate::coord::brokers`, in the pool): each
  member's agent runs a broker for its jobs' tasks (`coord::Broker`, docs/roles.md §3.2, on the
  pool's port; jobs given its address as `SCENIC_COORD`; its answers say epoch 0). Its heartbeat's `broker` gives its addresses. A
  task lives as long as its job, whoever leads.
- **Cross-member calls:** members allow each other's page origins (CORS, preflight for
  `Content-Type`): every address a member's page is served at (`coord::brokers::origins_of`, #919),
  from its heartbeat's `broker`: its tailnet address, its LAN name (`<host>.local`, where a member
  sends a LAN device, docs/roles.md §3.3), its LAN addresses (`lan`, from its interfaces: a LAN
  device typing one; origins alone, never contacted), its HTTPS page (`tailscale serve`),
  MagicDNS's names for it over HTTP (that page's name, full and short, at its port), and the public
  names a proxy on it serves its page under (its broker's `pages`, from `~/.config/scenic/names`,
  written by `services.zsh setup caddy`, docs/macs.md, Services: the M6's https://map.brandontsang.net/build). A loopback
  origin (`localhost`) is a Mac's own page only from that Mac: from a member's address it is that
  member's page opened on itself, allowed; from any other, refused, as is any other origin. So
  a member's page reads the others' live charts and settings (docs/pipeline.md §5). HTTPS needs
  `tailscale serve` on each Mac (§14). Chrome's Local Network Access may still block calls to
  another member. The agents' token is the pool's (`state/coord/token`).

## 9. The pool's API

Every Mac's agent answers on port 8090: the lead's coordinator, a member's broker. A member talks
to the lead alone, by HTTP (the calls, their timing and what a member keeps until the lead has it:
docs/coordination.md §5):

- **Its beat** (`POST /pool/beat`, every 10 s and on a change, from **beat**'s poster thread):
  its heartbeat and status (§10), the leases it holds (docs/coordination.md §6), its hand-offs'
  keys (§7.3), its capacity and resume progress, and its jobs' spans and logs the lead lacks
  (docs/formats.md, Job spans). The answer carries the lead's epoch, who leads (`leading`), the
  leases ended, the others' heartbeats, this Mac's hold, the settings' generation, the hand-offs
  merged and those the lead lacks (posted next, `POST /pool/handoff`), and what it has of each
  job's spans.
- **Jobs** go through the lead's coordinator (`/work/*`, coordinator.md), tasks through each
  member's broker (§8), the records' brief by `GET /pool/brief` (§6.2).
- **Its owner's asks:** job cancels through `/work/cancel` (§11); another Mac's pause and a pool
  setting through `/pool/hold` and `/pool/settings` (crate::coord::asks, §11), queued on its disk
  and sent with its beat; a page's retry by mail (§9.1).

A member that can't reach the lead keeps its jobs running and starts nothing new
(docs/coordination.md §7).

### 9.1 Members' mail

**What goes by mail** (`crate::pool::driver::Msg`, `crate::agent::poolside`): to the lead, the build
page's `Cancel` and `Retry` a member passes on (rare); from the lead, none. Hand-offs go by their
own posts (§7.3). A message an app doesn't know (an older app's) is read past. Each sender
numbers its messages and keeps its last 64 to each member (`KEPT`, its `mail.json`); a
recipient's read marks are in its own folder (`mail.json`), so each is taken once. The owner's
asks passed on are one-shot: the lead's store keeps them (below).

**To the lead**: `POST /pool/mail` on its coordinator, JSON
`{"from": <member>, "to": <member>, "msgs": [[<n>, <Msg>], …]}` (at most 64; `crate::coord::inbox`).
- **Auth**: the build's key (the coordinator's contact token, as every agent request: no page may
  call it, a viewer refuses it). Sent only to the lead's tailnet addresses (`mail_urls`: 100.64/10
  or `.ts.net`; never `.local` or a LAN address, any device's on the network to claim), its
  heartbeat's broker's, else its contact's. The key is the whole build's, not one member's: `from`
  is what the post says; the lead takes a sender it doesn't know only once its heartbeat is heard,
  and drops a post from one without, so no post makes up a member.
- **Taken only by the member it's to**: the coordinator answers 409 unless `to` is its own member
  (`Shared::pool_member`) and its agent gave it an inbox.
- **Durable before the answer**: the messages go into the lead's store (`inbox/<member>`,
  crate::pool::store) and are on its disk before the 200 `{"took": <n>}`; a crash after the answer
  loses nothing. (An agent before the store kept them in `inbox.json`: taken into the store once.)
- **Read once**: the lead's agent reads its inbox, each sender's messages taken in its numbering's
  order, each number once; its marks written, then what it read dropped from the store.
- **The member**: each message to the lead is posted with every earlier one its coordinator hasn't
  taken (`Mail::posted`), while the lead answers the member's beats; anything but a 200 taking them
  all, they stay queued and go with the next.

**From the lead**: none: what the lead tells a member is in its beat's answer.

## 10. Seeing the pool

- **Heartbeat** (its own writer): label, app, resources, conditions
  (`conds`: home, power, battery, paused), slots' jobs, tasks, addresses (`broker`), reach (§6.8),
  the epoch it leads in (`leads`, while it leads). Sent in each member's beat to the lead every
  10 s (`POST /pool/beat`, §9), which answers with the others' (its own among them); a member not
  heard for twenty minutes is forgotten. Nothing on the NAS (`crate::coord::beats`).
- **Build status**, written by the lead (its agent's `status.json`, asked of its coordinator by other Macs): the
  checklist (a line per stage with an item per step: docs/pipeline.md §4), forecast, regions,
  waits, what the map updates do (`publishing`), how they stand (`published`: the last map update and its
  kind, the map tiles being drawn by Mac, each Mac's adopted map), recent jobs, pause, the last
  map update, the lead.
  A member writes `helper.json` on its own disk instead, and sends its status in its beat.
- **Flags** (task #953, `agent::flags`): what needs a look, worked out by the lead at each round
  from its own status and its coordinator, in its status (`flags`: kind, Mac, a line) and in its
  beat answers' lead line, so `scenic status` ("Flag:" lines), the build page's alerts and the menu
  bar (the lead's own status; a member's lead line) show the same. Nothing is flagged while the
  build is paused, nor on a Mac paused alone.
  - *idle*: a Mac whose last ask was refused (`packing`: it asked for more jobs than it got, so it
    has a slot free) while work it could take waits. With targets offered and not leased of a step
    it may run, the flag says how many, of which steps, and the refusal, which says what in its
    capacity held the job back (memory and its floor, cores, disk, the NAS budget, the grants, a
    cold start). Refused for its room (`sched`'s `Refused`, task #1170), the refusal names the first
    target offered past the room (its step and tile), what it needs (memory, disk) and each of
    memory and disk this Mac is short of, by how much ("lo 3/6/3 needs 12.7 GB of memory and 35.2
    GB of disk: disk short by 19.2 GB (46 GB free · 16 GB for jobs (30 GB room target))"; the
    whole copies counted for it said among its disk), then how many of the other targets offered
    are held by jobs, done, in backoff after failing here, or free but held back otherwise (no
    measure yet, the NAS budget or placement); with no target past the room, those
    counts alone ("nothing offered is free to take: 3 held by jobs"). Room held for that target
    while the Mac's jobs end is the refusal ("room held for a job of lo 3/6/3 (…)"), whether or
    not anything offered after it was held back. Each Mac's flag says its own room, so the flags
    together say what each Mac lacks. With none free but work of a step any Mac takes held behind other steps (the
    plan's waits with a step: slope-hi behind terrain-root), it names those waits, and the wait of
    each step a wait names that waits too. Nothing left it could take: no flag.
  - *stuck*: a leased job whose work hasn't moved, by its worker's beats (each minute), for more
    than the overdue factor times its step's usual stillness (task #986). Its work moved at a beat
    when anything of it changed since the one before (`Lease::still`): its progress line, the
    target under way, its targets ended, its phase, its NAS bytes, files made, listed, statted or
    deleted (a whole file copied counts its bytes as each read comes, `store::nasfs`'s
    `copy_counter`, not once copied: a big file copied from a slow NAS moves; an upload as each
    write goes, `store::nasio::written`, not once it lands, task #990), its internet bytes.
    Its step's usual stillness is the 90th percentile, over its jobs kept in the history that ended
    well, of each one's longest stretch between two beats that saw its work move (`still_s` on its
    "done" or "end" event, `History::still_p90s`). A step none of whose jobs measured it (its first
    runs; every step as the stillness was first recorded, older events having none) is held to its
    target's time instead, as measured on that Mac at its threads: its own run's, a target never
    run itself the larger of that and its step's runs' 90th percentile (#978's rule), its start's
    added before its first target (`Sched::target_most`): a target's start and end each move a job,
    so no still stretch of a working one outlasts its target. A step never run the way it runs now
    is held to the longest usual stillness of any step; with nothing measured at all, nothing is
    flagged. The flag says how long it has been still, the target and phase it is in, which usual
    it is held to, its batch's targets done and its threads. A job is held to what it is doing,
    not to how long its targets took before: on 6 Nov a first GSI fetch (pyramids-fill, 19-25
    min), bldprep and pack while the NAS was slow and terrain-hi copying GLO-30 at 4 MB/s were
    flagged by their targets' earlier times while their bytes moved. A job slow but moving isn't
    flagged stuck (it may be slow, below); one that stopped is (5 Nov: terrain-hi jobs at 0 tiles and no CPU, replayed from the
    records: found 16 min after their last move, terrain-hi's usual stillness 5 min). Beats not
    heard say nothing (a member out of touch is a warning of its own); a lead restarted counts
    each job's stillness from its first beat after. A job whose lease is younger than it (taken up
    at a lead's restart, or held again at its agent's take-over: `Lease::taken_up`, by its job's
    start, `Lease::began`) notes none of `still_s`, `rates`, `gap_s` and `threads` on its event: the
    lead heard only part of its run, its longest stretches cut short (task #992).
  - *slow*: a leased job whose progress went on, over its window, at less than its step's usual
    rate divided by the overdue factor (task #990, `flags::slow`). Its progress is its progress
    line's numbers alone, by its worker's beats (`coord::lease::Pace`): within its part under way
    (one line's run on one target: the target under way, the line's unit, its total), the most the
    line has said, so a line gone back on the same target (work done again) counts nothing until
    it passes it, and its job view's NAS and internet bytes never count (a retry sending its bytes
    again moves no line). Its rate is in that unit (tiles, z6 tiles, packs, zooms…: the unit the
    step's progress line says, a word in brackets after it left out); a line that counts bytes
    itself (unit's "MB of the DEM cache copied here", lakefill's "bytes uploaded") is held to its
    most said the same way. Its window is the factor times
    its step's usual longest wait for progress, back from its last beat, judged only once its part
    under way is that old: a window that long holds a working job's progress however bursty its
    line (terrain-hi says its tiles a few hundred at a time, after minutes of sampling), and one
    with none in it is at a rate of nothing (a copy before its first tile, a retry loop). A long
    stretch of work in a job's own process moves its line as it goes (docs/building.md §2: a unit's
    staging by the packs it reads, task #1156, where its line had sat for 26 minutes as the staging
    read at 6 MB/s, flagged at 0 z6 tiles a minute), so it is judged on its rate. Its
    step's usual rate in a unit is the 10th percentile, over its jobs kept that ended well, of each
    one's progress in that unit over the time its parts in it took (`rates` on its "done" or "end"
    event, units a second); its usual wait the 90th percentile of each one's longest stretch from
    its grant or a beat that saw its progress go on to the next (`gap_s`). Both are taken from its
    jobs that had no more threads than the job judged has had over its part under way at its
    fewest (`threads` on the event, the most its job had: `History::paces`, `Paces::usual`): more
    threads may make a job faster, never slower, so a job on one thread isn't held to runs on
    three (six 2-thread packs were 3.1–4.4 times under the slowest tenth of packs on 2 and 3
    threads, within 3 times that of packs on 2). Each from ten such jobs at least (`history::ENOUGH`: fewer,
    and the tenth either percentile leaves out is no whole job, the percentile just the one
    slowest or longest). A step with fewer, a step whose jobs never said their progress, a job
    whose threads aren't said and a part younger than its window are not judged; a job flagged
    stuck isn't flagged slow too. The flag says its rate and its step's usual, a minute, the
    threads that usual is of, its window, where it is, its batch's targets done and its threads.
    Replayed on 5–6 Nov from the lead's timings (each run's threads and time) and the lead's and
    the M1's job logs (2,009 runs with three progress lines or more; the M4's logs not read). No
    record keeps a job's progress beat by beat, so each run is judged by its rate in each unit
    over its whole run's time (lower than over its parts in the unit, so this flags more than the
    rule would), against all runs of its step in the timings: 40 runs fall under it, 47 without
    the threads rule (six 2-thread packs and terrain-deep 10/246/339 on 3 threads, whose step
    has fewer than ten runs on 3 threads or fewer, drop out). Most are slow outright: polar-terrain
    blocks on one thread (all but 7 of its 154 runs had one) at 0.02–0.04 polar blocks a minute
    against a slowest tenth of 0.22, trees jobs at one thread (every trees run had one) 3 to 8
    times under theirs, terrain-hi 6/14/26 at no tiles in its run. The M1's terrain-hi job
    copying GLO-30 at 4 MB/s (6/23/36, 67 min) is not: its 175 tiles a minute over its run sit
    above terrain-hi's slowest tenth, its copy a stretch of a job otherwise fast, which only a
    window inside it can show.
  - *stale*: the lead's map update due (held back, planned, or waiting out its minute:
    `PublishLine::due_since`) for more than the overdue factor times the most the plan holds one
    (`CATALOGUE_CAP_S` and `CATALOGUE_EVERY_S`, 11 min), counted from the last map update's going out
    where later, with no `catalog` job leased (that one is stuck by its own stillness). A
    map merely old, nothing new to list, isn't stale.
  - *repeat*: the lead's newest run of GC or of the backup ended well having done nothing (its
    report, the job's last lines: GC no chunk of its queue taken and no sweep run, the backup no
    day written or pruned; `Done::empty`) and ended less than that duty's time after its run before
    ended (`flags::REPEATS`: GC's queue's grace, an hour, as a name is due that long after nothing
    holds it; the backup's day), flagged while that run is as recent (task #1392). A duty planned
    again with nothing for it to do; the next day's backup finding nothing new isn't one (planned a
    day after the last ended well, it ends a day after it at the soonest). The lead's log says the
    same churn by its grants (§7.2's "many grants of a step the forecast doesn't model", any of
    those steps, whatever its runs did); this flag says which runs found nothing.
  - The factor (stuck's, slow's and stale's) is the pool's setting `overdue-factor` (kept by the lead; `scenic pool set
    overdue-factor <times>|off`, or the build page's Settings, the pool's box), 1.1 to 100; not
    set, 3 (`OVERDUE_DEFAULT`, the owner's call of 4 Nov).
  - `scenic status --flags [--since <unix s>]` prints what needs a look alone, a line each: the
    flags, the pool's warnings (no lead, the lead or a member out of touch, a Mac the lead holds a
    capacity of with no status heard, the lead's loop stuck) and the jobs failed since then (every
    Mac's, the last 6 hours by default): tools/dev/health.sh's.
- **A job's progress** in its status (`JobView::progress`): a job of several targets says its
  batch's, its targets ended (its costs file) plus the share of the one under way, and its time
  left at the batch's pace since it began; a job of one target, that target's own line and pace
  (a 36-target job had read "99%, about 0 min left" for 24 minutes by its target's). An update's
  drain still waits by the target under way's time left (its next safe point).
- **Coordinator view** (`Coordinator::view`): the view the
  build page's `/api/swarm` takes from the coordinator's memory: the pause, each lease (worker,
  step, targets' count, progress, when granted, its last beat and when it lapses), each
  worker (label, kind, memory, cores, done/failed, `can`, `fit`, when last seen), the tasks by
  state. Times are unix seconds, so it is the
  same until something changes. Served by the lead's coordinator from memory (`POST /pool/view`,
  docs/coordination.md §5.1). Each time is taken from one
  anchor (an `Instant` and the wall clock read together, taken again only when they drift apart
  by more than 2 s), so an idle pool doesn't rewrite it. Read by each Mac's page data (below).
- **Forecast input** (`forecast::Record`): the forecast's whole input as data (the work left, each region target's
  cost in a table by step and target, the lanes, what runs now, each Mac's room, the cadence) and
  what the lead adds to its answer (each step's time a target, the slots waiting on a tail), so `Record::forecast` gives the lead's forecast on
  any Mac. Made each time the lead makes the forecast (at most once a minute); zstd-compressed
  when its JSON is over 2 MiB (`forecast::ZSTD_OVER`; a reader tells by zstd's magic number). The
  last ones on the NAS (`state/build/term/<E>/forecast-input.json`, 31 Oct 2026, terms
  664-673): 10.6-11.8 MB of JSON, 0.84-0.98 MB
  compressed. Every time and speed in it is made finite before it's written (`Record::clamp`).
  Handed to the lead's coordinator for `/pool/view` and to the forecast inputs' archive (§7.9). Read by each Mac's page data (below).
- **The page's data on the lead** (`agent::pageview`, task #409; the lead's alone since #882 step 4,
  docs/roles.md §3.3: a member's port sends the page there): in the pool, the lead's port 8090
  answers `/api/swarm` and `/api/tocome` from its pageview (`/api/steps` is the steps table), and
  `/api/history` and the swarm's history parts (`seq`, `epoch`, `rates`) from its coordinator's
  memory (task #1160: the history below); out of the pool the coordinator answers all from its
  memory. While a page has asked in the last 60 s, a thread of the agent asks the lead's view every
  2 s (its coordinator in process, as `POST /pool/view` answers): its coordinator's view, the pause
  (on the lead the answer's is its coordinator's own), the Macs' addresses, and the lead's status,
  the forecast's input and the regions when their time changed (the last status kept meanwhile);
  with the lead down, the last view is kept and the answer says its age (`view_age_s`) and since
  when the lead isn't reached (`view_unreached`). The agent's loop hands it only what it holds
  already (this Mac's app, its own coordinator): no NAS read on the loop or on any page's path, and
  this Mac's lanes worked out only while a page asks. Until the lead's first view (a second or two
  after a page first asks) a Mac answers the swarm 503, and the page says it's reading the build's
  state (quietly, not as an error) and asks again every 2 s, saying it as an error only after a
  minute of 503s (`sums.js` `notRead`; no retry while the page is hidden).
- **The page's history** is the coordinator's, held in its memory (`coord::history`, task #1160):
  the week's events (`history::KEEP_S`, its one bound), each without its `data` (the catalogues',
  merges' and leases' details, which only publishing's times read: from the file and the archive;
  a `planned`'s kept, which a restarted lead reads back). Measured 13 Nov on a copy of the lead's
  file (39 MB, 99,737 lines; task #1160 review, the M1, the tests' build): the week was 57,800
  events (~8,300 a day, ~2.5 MB of JSON a day), ~55 MB in memory (~0.95 KB an event), the process
  ~80 MB larger once loaded; ten times that volume, ~0.55 and ~0.75 GB. The file is read once, a
  line at a time, as the coordinator starts (0.24 s for the 39 MB, 2.2 s at ten times, before it
  listens: a restarted lead's page has its week at its first answer); it is rewritten under the
  coordinator's lock with the week's lines once it holds twice as many (0.1 s, 0.7 s at ten times;
  a failed one tried again 10 minutes later); `/api/history` takes the events after a number by a binary search
  (1,000 in ~0.6 ms, ~160 KB of JSON), and the swarm's `rates` are summed from a shared copy off the
  coordinator's lock (~14 ms for the week, ~57 ms at ten times; an event coming meanwhile copies
  the week, 0.03 s, 0.3 s at ten times). One numbering, the coordinator's (`epoch`
  `coordinator <member>`): its numbers go on across its restarts, as they're in the file, so a
  page keeps its cursor over a lead's restart; the page starts its cursor again when the epoch
  changes (another Mac leads) or `seq` falls below its last event (a file lost), and the Jobs tab's
  past jobs are asked further back by `since`, at most 5 000 a time ("since you last looked" goes
  by the events' time). `/api/tocome`'s `after` is in the same numbering. Older than the week, the
  history is in the NAS's archive (§7.5), which the page doesn't read (its schedule's
  "Older history" card says so); `scenic timings publish` and the forecast check read it.
  The pageview's forecast is run from the lead's input, its time brought
  to now and this Mac's own lanes as its slots are now, worked out as the lead works out its own
  (`forecast::SlotJob`: its own estimate, or its targets left at its speed, a minute at least; its
  running job too); a target missing from the cost table is a bug, logged once. It runs on a
  thread of its own, again when the input changes or a minute has passed (`pageview::Runs`): the
  answer carries the last one done. Times are given
  the page as ages (`coord::ages`). The answer carries the app serving it (`app`). With a lead on
  an app from before these records (no `view.json`, no `forecast-input.json`), the answer has no
  leases, workers nor tasks, and the forecast is the one in the lead's status. A member with
  neither a coordinator nor a broker answers its port from a viewer (`Coordinator::viewer`): the
  page sent to the lead as a broker sends it (docs/roles.md §3.3), its live store, settings and
  hold, every other call refused (its key's too); no state folder of the agent's (its token alone,
  in a scratch folder removed as it stops, a dead process's as the next starts), no history, no
  background loop. The usual Origin rules hold.
- **Phase shares** (task #511): for each step, its top-level phases' mean share of a run's wall
  time and of the bytes its phases counted as read or written (local disk, NAS and internet alike:
  `timings::count`; the page says "I/O"), over its last 10 runs that ended well
  (`timings::SHARES_RUNS`; the bytes' mean over the runs that counted any), largest by time first.
  The lead's status carries them as `phase_shares`, by step id (the steps table's; a leased task's
  runs, timed as "task <kind>", are its kind's: `timings::step_of`; its job's id names its kind too,
  "<kind> <unit>", so the Machines tab shows it, task #849):
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
- **Time by phase** (task #844): each step's runs summed by when they ended, so the build page's
  step rows break a step's time down over the range its footer picks ("Charts show the last": 1 min
  to 7 days). The same thread reads every run of the log (failed ones too) and, every minute, sums
  those that ended within the last 7 days by the first of the page's ranges their end is within
  (`timings::TIMES_EDGES`: 60 s, 5, 10, 30 min, 1, 2, 6, 24, 72 h, 7 days; the page adds up the
  ranges up to its own). The lead's status carries them as `phase_times`
  (`timings::PhaseTimes`): `{"at", "edges", "steps": {"<step>": {"phases": [[name, class], …],
  "b": [[edge, runs, failed, wall s, CPU s, [[phase, wall s, CPU s, NAS MB, internet MB], …]],
  …], "w": [[edge, phase, measured wall s, NAS s, network s, disk s, other work s], …], "o": [[edge,
  phase, [[pair, s], …]], …], "u": [[edge, phase, s], …]}}}`, `u` of a phase's CPU (its own work's,
  task #1116) the seconds that may be other phases' (`timings::uncertain_cpu`; the phase card's
  "Uncertain CPU"); a
  step's top-level phases of its main thread, largest over the week first, at most 8
  (`TIMES_PHASES`: past that, the 7 largest and the rest as "other phases"); a run's time in no
  phase is its wall less its phases'.
  **Measured waits, not classes** (task #870): no phase is classed by hand. Every NAS call
  (store::nasfs, range, iopool, naming, blobs), network request (fetch's `Net`, the coordinator's
  client, the other HTTP callers), local-disk read of the store's, and wait on other work (another
  worker's subtrees, a thread copying ahead) holds a store::waits guard; each phase span takes the
  totals as it opens and closes (`timings::Waits` in its record: NAS, network, disk, other work
  seconds, and the NAS's and network's bytes meanwhile). Waits count by the job's wall clock, not
  summed over threads: an instant is the NAS's when any of the job's threads is in a NAS call, else
  the network's, else the disk's, else other work's, else computing — so a phase's parts add up to
  its wall time. A phase running a child program takes in the waits of the child's main-thread
  phases (the Python twin, dem/timings.py, measures its own the same way: `wait`, `reading`,
  `lines`), held to its wall. `w` sums them over the runs
  that measured them; the page splits each phase's bar into "waiting on the NAS", "waiting on the
  network", "reading/writing the local disk", "waiting on other work" and "computing", the rest of
  its wall (older runs') grey as "not measured", its card giving the numbers and the rule. A phase's
  `class` is said only for records from before (and the twin's and pages'); its bytes are the
  measured ones, an older record's by class as before.
  **What ran alongside** (task #891): beside the kinds' times (which alone add up to the wall),
  store::waits counts the seconds two ran at once on parallel threads, by `PAIRS` (the waits two by
  two, then each with computing): two kinds' guards both held (exact), and a kind with computing
  (while that kind held the instant, the process's CPU time meanwhile, at most that time: an
  estimate, which also counts the waiting threads' own CPU in their calls, small for the NAS and the
  network, and other waits' threads'; not for the local disk, whose reads from memory are nearly
  all their own CPU: that pair stays 0 and isn't drawn; the strip's card's "How"
  says so). They are `Waits::both_s` in a record (absent when all 0,
  and in records from before) and `o` in `phase_times` (pairs of 0.1 s or more). The page draws each
  as a 2px strip along the foot of the first kind's segment (the one the instant went to), in the
  other's colour, as long as their time at once, its card naming it ("computing ran alongside for
  6 min"); the phase's card lists them under "Alongside". Extra: never in the bar's sum.
  Cost: ~620 ns a guard taken and dropped alone (store::waits' test, release; ~70–115 ns before
  #891), the process CPU clock read (300–750 ns on macOS, more with more threads) at each change of
  the kind holding the instant, outside the lock; while some guard is always held, as at 16 threads
  reading page-cached files, next to none (within noise of main, 30 Oct). Off outside a timed run;
  NAS and network calls take hundreds of µs and more. Measured on the lead's log of 29 Oct
  (6,251 runs, 49 steps over 4 days): ~32 KB of the status (~290 KB: +11 %).
  Phases are named as doings ("reading the depth sources' windows"). A phase renamed while kept
  data still holds its old name is mapped by step (`jobrt::phase_names`: two steps may say the
  same words of different work): a record read, on any Mac, takes the current name of the old one
  its step's phase had, so the step's history keeps its phases; a row goes once no data holds its
  old name. Each line of the page's phase descriptions (web/buildpage/sums.js `PHASE_ABOUT`) is
  of a phase some step emits now, and a renamed phase's current name has one
  (crates/jobrt/tests/phase_names.rs, grepping the code's phase names).
- **Each Mac's jobs** (the build page's machines, `scenic status`, task #246): a block per Mac,
  the lead's first, marked coordinator (`scenic status` with its ramp, §7.1); its capacity as the coordinator last
  heard it (its memory against its floor: available, the floor, reserved for its jobs' growth and
  left for a new job; cores busy, disk reserved and free, on the page as bars); a block per
  running job numbered by its real slot (`JobView::slot`: slots may be sparse, so a Mac running
  only its fourth slot shows slot 4), (`scenic status`: with that slot's lane's next work); and (`scenic status` alone) why nothing more
  runs: the packing's refusal as its last ask was packed (`Coordinator::refusals_with`, the status's
  `packing`: a cold job of the lead's own waiting for room, cores short for the next job open, a job
  offered refused for its room (the first that didn't fit, what it needs against what there is,
  said even beside work offered to other Macs), the targets offered all held by jobs, done or in
  backoff ("nothing offered is free to take: 1 done (its record not read yet)": offered by a plan
  made from records read before its job's hand-off was merged, such as one under way as the job
  ended or while the merge holds the entry; never granted again, and gone at the first plan that
  doesn't offer it at that key, or as soon as the merge refuses its hand-off or passes it over with
  the records under another key: `Coordinator::again`), steps offered but not to this Mac (the
  lead's own, a step it doesn't run; `Rule::Away`: "3 steps offered are the lead's alone:
  names-todo, rail, pack"), or no job offered at all (`Rule::Empty`, also while the plan has no
  pass). The lead's status words the last two with the plan's waits (`flags::nothing_words`, each
  list cut to its first four): the work left a Mac could take waiting behind other work or an
  input, with its wait ("the work left waits: slope-hi: …"); on a member, the steps left only the
  lead runs; the waits of no step ("waiting: Building: the build's records can't be read now", on
  a member "the lead waits: …"); and only with none of these, as the verdict's "Nothing to build",
  "the plan is complete: no job is left to offer"; never a memory or disk figure. An ask refused
  before any packing (the lead moving, the build paused, an older app) lets the refusal go: it
  was of offers that ask wasn't weighed against), shown while the refusal itself is younger than two minutes
  (`CAP_FRESH_S`), older ones dropped. The forecast's lanes are per slot
  (`<host>`, `<host> #2`, …), as many as the slots each Mac may run now (`forecast::slot_lanes`):
  the lead's its ramp's, each member's `agent::job_slots` of the cores of its capacity (else a
  lane per job it runs: an older agent's jobs, not saying their slots, by their place), never
  fewer than its last running slot's number, and the lead's slots its ramp grows to later (one
  each clean stretch, opening then). Each lane's next job is the coordinator's scheduler's grant to
  its Mac (§7.2, One scheduler), the Mac's room shared by its lanes as the scheduler packs it (a Mac
  whose ask isn't recorded: `forecast::MacRoom`); each
  lane's runs (`forecast::Stretch`) are a step's jobs one after another (two minutes apart at most),
  each job as granted (task #834): a run's `cuts` say where each job after its first starts and how
  many of the run's targets (`/api/tocome`'s `runs`, in time order) come before it, so the page
  draws every job to come, over the whole forecast, as a block of its own with its targets (~10
  bytes a job: 29 Oct's record, 5,397 jobs over 2.5 days in 571 runs, its lanes 35 KB without the
  cuts and 91 KB with them, the status ~236 KB to ~291 KB, +23 %); each
  lane starts with its slot's running job and its time left (a member's without progress said:
  its step's measured time). The page names a lane by its Mac's chip and slot ("M4 Pro 2";
  `cond::Resources::chip`). Each lane runs at its slot's measured speed (else the lead's other
  slots' guess, four fifths of its pace, or a member's): slots on one Mac share its cores, so the
  forecast is optimistic until each slot's speed is measured under that sharing.
- **Regions** (`agent::regionsview`, task #512): each enabled region's `crumbs` (its folders'
  names in the regions' tree, a folder without a `_group.toml` name by its own in words, then its
  own), its `bytes` (the map's files a download of it copies, `areafiles::files_of` as the map
  server's downloads count them; not the basemap's pieces nor its terrain detail; null while the
  map lacks it), and its outline for the world maps of the Overview and Regions tabs: its
  drawn shapes in Web Mercator, the world 0–1000 each way, simplified to about 7 km, `fill` the
  rings, outer and inner (an island in a water region is a hole of it; outer wound positive, inner
  negative, filled nonzero: a hole unfilled, two shapes overlapping filled) and `edge` the lines
  to stroke (a stretch two of its shapes share along a parallel or meridian left out). Made by the lead, on a thread of its own, once per regions' accepted
  version and map (the status's `catalog`, the map's `seq`; a failed read not tried again until one
  of them changes): the drawn files read then, every region's sizes once per map update, from
  the lead's log's map (`agent::publish::Here`: the head's catalogue, made of the map while a
  reader holds it; the plan's regions on the map, the map updates' times,
  `pubtimes::PlannedNotes`, and this view read the same one). regions.json is about 0.96 MB. Handed by
  the lead's pool step to its coordinator for `/pool/view`, read by each Mac's page data when its
  time changes. Every Mac's port answers `POST /api/regions` with
  no key: in the pool from that read, out of it from this Mac's agent's; a page that has the
  outlines of the answer's version says so (`{"have": version}`) and gets it without `fill` and
  `edge` (asked again with each map update for its sizes, as often as each poll while the lead
  updates the map; on 7 Nov's 1,025 regions 0.1 MB without them, 3.1 MB with them). Until there is one it answers 200 with `{making: why}` (task #798: never an error
  while the agent is busy; the lead's loop can take minutes after a restart before it makes them), and
  the Regions tab says why in place of its rows.
  Steps (task #636): `POST /api/regions` with `{"steps": true}` answers each region's targets
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
  region), nothing written to the NAS. Not counted yet (a member's port, or the lead's before its first count): 200
  with `{making: why}`; the page then asks the lead's addresses.
  To come (tasks #622, #685): the lead's port answers `POST /api/tocome` (`{}`; no key) with every
  Mac's tiles in its last forecast (`forecast::ToCome`, made of `Forecast::placed` on the
  forecast's thread before `placed` is dropped): `{at, macs: {mac: {step: [[z/x/y, s from at]…]}}}`,
  each tile once a Mac and step at its first start, targets naming no tile left out: the page's
  map's queue; and the schedule's runs (`forecast::Runs`, task #787): `runs`, each run's targets,
  the runs numbered as the forecast's `lanes` hold them (the lanes by name in byte order, each
  lane's runs in order); `jobs`, how many jobs each run is (its `Stretch::cuts` and one), the jobs
  numbered in turn in the runs' order; and `deps`, [a, b] when an item of job b waits on one of
  job a (task #879: its declared reads, `steps::read_targets`, a release's; a set it reads whole by
  the job of the set's item ending last), each pair once; and `after` (task #797), for each job (in the jobs' order) the history's event numbers of the finished jobs its items waited on (per step the last such job over the job's items, sorted, a few numbers a job: the `WaitsBasis` lookup the forecast's thread already makes for the history's waits, `Waits::by_job`; none for an item new since the last forecast), with the `epoch` they're numbered in: the coordinator's history's (`coordinator <lead>`, the swarm's), in and out of the pool. In the pool the answer is the lead's pageview's (§10), worked out by the same lookup from its own run's items and the coordinator's history (`Coordinator::history_waits_basis`: its waits kept apart from the history's own, `History::waits_basis_from`; kept on the lead's disk, `<agent home>/pageview-waits.json`, written whole (`store::whole::write`) every 5 minutes at most, each number with its numbering's name, and read back for its first run after a start (task #863), so that run's `after` names the jobs finished since they were kept; none when no file is there, or it was kept over a day before). Measured on the 26 Oct 20:15
  record (31,132 items, 850 runs): `macs` 642 KB, `runs` 366 KB, `deps` 117 KB (12,046 pairs);
  ~275 KB gzipped whole, and sent gzipped to a client that sends `Accept-Encoding: gzip` (the agent gzips every answer of 32 bytes or more except images, event streams, raw bytes (octet-stream, tar, zip) and parts of a file). In the pool (task #836) every Mac's port, the lead's too, answers from
  its own pageview (§10, `PageView::tocome`): the tiles and runs of the very forecast run its
  `/api/swarm` carries (the lead's record run there at now), so its `at` is the status's forecast's
  and `after` is in that pageview's numbering, with its `epoch` (none before its history's first
  read); the page sends the `at` it read (`{"at"}`) and gets that run's, the last or the one
  before (a run ended between the two asks), else the last. The lead's agent's own (its forecast
  thread's, the coordinator's numbering) had been one forecast off its page's status (seen
  29 Oct: 1793299450 against 1793299556), so every page dropped its own Mac's answer and asked
  the lead, whose port refuses an HTTP page's cross-origin read: the map drew only the running
  jobs' tiles. A port with no pageview forecast yet answers its agent's own, or 200 with
  `{making: why}` (it made none); the page takes its own Mac's answer only when its `at` is the
  status's forecast's, else asks the lead's addresses; a lead change drops the old answer at
  once and asks the new lead. Asked once a forecast; never in the 10-second poll. Measured on the 24 Oct forecast
  (3,618 targets placed, 3,615 tiles): about 62 KB (the Mac mini's 52 KB, 3,053 tiles, the M4's
  10 KB); held by the agent: ~150 KB.
  Every Mac's port also answers `POST /api/built` (no key; task #674) for the map's finished
  tiles: `{at, steps: {step: [z/x/y…]}, empty: {step: [i…]}, error}`, every tile target the build's keys
  (`state/build/jobs.json`) record, by step, and `empty`: indexes into a step's list of the targets
  whose published files the manifest doesn't name (`agent::steps::output_named`: a deep tile with
  no fine data, #839; steps whose files aren't told by name left out), keys and manifest from one
  read of the records' brief (a `-root` step's as 0/0/0; worldwide steps', records of steps the app doesn't know (a
  retired step's) and terrain-hi's, slope-hi's and tree cover's old z3 whole-run records left out:
  `agent::builtview`), `dropped` ({step: [z/x/y]}, the targets whose record is a drop's none
  key: the page leaves out their drop runs' job ends), and, once the agent's view of the map updates has read the map's log, `published`
  (indexes the same way, for every step whose files are told by name, an empty list when none) and
  `catalogue`, the map's `seq`: the targets one of whose published files the map still names as an
  `f/` entry (task #866: a batch deleting it ends it; the page draws no other told tile as published),
  read by the agent on a
  thread of its own while a page asks, again a minute after the last read at most, from the
  records' brief its coordinator holds (in process; the NAS's brief while it holds none), the last
  read kept when the brief can't be read now. The page asks each minute while the
  map is drawn.
  Every Mac's port also answers `POST /api/catalogs` (`{"after": n, "steps": [step…]}`, both
  optional; no key) for the
  map's age controls (task #629, `agent::cataloguesview`; the page asks the lead's): each batch of
  the map's log (docs/formats.md, The map and its log; `n` its `seq`, `t` its time) as
  `{n, t, first?, z3, z6, z3_gone?, z6_gone?, z10?, z10_gone?}`, the tiles as `y * 2^z + x`, with
  `latest`, `reading` and `error`: a batch's `set` and `del` keys say which tiles it changed and
  dropped. A batch changes a z6 tile when it sets or deletes a key of it
  in a tile section (base pack, road values, hi data, landmark points, a layer's hi pack, the deep
  terrain under it) or changes the file (`f/<name>`, its content name) of a name such a key gives,
  a z10 tile by its deep packs alone, a z3 tile by its layers' lo packs and area overlays (a pack
  built again keeps its name and gets a new file: #833 review; a file set again with only its size
  changed changes no tile, the view keeping each name's file by its hash). `first` marks the view's start: with
  nothing kept, it takes the map whole and names everything it has. Those entries are every step's
  (the timeline's marks). A soloed step's tiles are aged by its own outputs instead (task #994): with
  `steps`, the answer adds `steps: {step: [{n, t, first?, z3|z6|z10: [i…]}] | null}`, each tile the
  map names at the step's zoom grouped by the newest last publish of its names there
  (`agent::steps::output_names`; a step making several layers' packs: the newest of them; the view
  keeps each `f/` name's last publish, the batch that gave a key that name or changed its file);
  null for a step whose files aren't told by name, which the page ages by any step's entries. The
  page asks for a step as the map first needs it (a solo, a queued tile's card) and again after a
  new map: terrain-deep's is 35 KB of JSON (7 Nov), the answer without `steps` the same as
  before (146 KB whole, 45 KB gzipped, read once a page; then the batches after its last). The
  agent follows the log on a thread of its own, each loop and each ask (20 s apart at most): the
  lead's log's feed in process, on the lead alone. Told to reset
  (the log no longer holds its batches: a new lead's log), it takes the map again and says in
  `error` that the batches it missed aren't shown. Kept in the agent's folder
  (`catalogues-view.json.zst`, written when a batch was applied): the entries, the `seq` they reach, each tile section's
  key with its name's hash, and each `f/` name's hash with its file's (what `/api/built`'s
  `published` reads, #866) and its last publish (a view kept before #994 takes each name's from its
  deepest tile's newest entry, any layer's), never a map; so a restarted agent goes on from where it was, as
  deep as it has followed the log (the lead's feed holds the last 64 MB of batches, `HELD_BYTES`;
  behind them, a reset). `scenic catalogs [--after N]` prints the same JSON from this
  Mac's kept file. 503 until the agent has made it.
  The page's own files carry the world's countries' outlines for the same map (`countries.json`,
  Natural Earth 1:50m, the same projection; made by `tools/worldmap/countries.py` from the NAS's
  `sources/basemap/natural_earth_vector.sqlite.zip`, read only) and the large lakes drawn as the sea (`lakes.json`, Natural
  Earth 1:10m lakes of scalerank ≤ 3, simplified, #831; made by `tools/worldmap/lakes.py` from the NAS's
  `sources/fetched/natural-earth-lakes-10m`, read only).
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
  threads, never its loop: every second the load (`load1`), the kernel's memory-free percentage
  (`mem_free_pct`, `kern.memorystatus_level`), the memory available (`mem_avail_gb`,
  `sys::memory_available`, as the memory floor reads it, §7.1; what compressing frees among it,
  `mem_compressible_gb`), its pressure (`mem_pressure`: 1, 2, 4) and the swap (`swap_mb`),
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
  memory in use, the Mac's less what's available, less the build's); the bytes written to the Mac's
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
  they use the Mac), the memory floor (`mem_floor_gb`), the owner's room
  target (`disk_room_gb`) and the jobs' disk reservations (`disk_reserved_gb`). An answer's `mac`
  also says the idleness counted away (`away_s`). The NAS's latency (`nas_ms`) every 5 s
  only while a build runs here (a job of this Mac's, or its coordinator's leases out): a listing
  of the project's folder under `nasread`'s deadline, timed inside the call (`nasread::probe`; a
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
  (store::deadline, §7.8) every second: `nas_calls`, in flight now (held ones included), and
  `nas_missed`, those that passed their deadline or were refused at the cap of calls held, over
  the second; each member's ask carries them too (`Capacity::nas_calls`, `Store::calls_now`: in
  flight now, missed a second over its last 30 s, its cap of calls held), summed by the lead every
  10 s into `pool_nas_calls`, `pool_nas_missed` and `pool_nas_held_cap`
  (task #634; memory only, like every series). The lead's status says the rest (`nas`, `coord::NasView`): the
  day's probes as its prober keeps them (in its store, `nas-probes`, read as it opens and saved
  there), the limits they give, the gate's smoothed load, target and
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
  robots.txt as a job crawling its pages told it (`robots`: `{state, t}`, `POST
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
  nas_wrote, net_in, net_out, ran_s}` (bytes since the job began; `ran_s` how long it has run, from
  its process's start, carried through a take-over: the lead takes its lease's job start from it,
  `Lease::began`, and keeps it off the view it shows, task #992). A job's run (the time on its
  "done" or "fail" event, the forecast's and the page's time run, the NAS budget's time past its
  prediction) is from that start, not its lease's grant: a lease read back at a lead's restart is
  younger than its job. Read back, an end whose lease began before a lead's restart reads at
  least its time since its "lease" event (`history::mend_runs`). The phase is the top-level phase its
  main thread has open (jobrt::timings: its name and class, since when, unix s), said in the
  job's counts file after its counts (`store::nasio::set_phase`: a sequence number, odd while it's
  written, its time and 48 bytes of text, a few stores in the mapped page, ~40 ns a call measured; never
  summed), read from the newest set among the folder's running processes' files, each read twice and taken only when both reads give the same even sequence number (so a text torn by a write mid-copy is never taken). The target is
  the one its costs file says is under way (`started`, not yet ended) and since when; of its
  targets (the lease's), how many ended; the page weighs `now - target_since` against the step's
  `each_s` (the forecast's). Each field missing: not said (a step that times no phases, notes no
  costs, was given no tag or folder; a job taken over from an earlier agent counts no internet bytes, its tag begun by that agent's proxy). The counted relay copies in 64 KB
  chunks: 4 GiB over loopback in ~1.0 s on the M1, against ~2.6 s with `io::copy`'s 8 KB it
  replaced (task #516), so the counting costs nothing measurable. Every Mac's port
  answers `POST /api/live` with no key: `{range}` (seconds back from now, an hour by default,
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
  port the store. Another Mac's page reads it directly: `/api/swarm`'s `macs` lists each member's
  host and addresses (`{host, urls, https}`, its heartbeat's broker; none with the brokers'
  switch off), and the answer is allowed to the members' origins (CORS as for tasks, §8; the
  M6's https://map.brandontsang.net among them), the page reading by the scheme it was served
  over. The build page's Machines tab draws it (docs/pipeline.md §5, Machines), asking with
  `net: false` (the proxy's counts left out); an answer also says the Mac's size, `mac: {cores,
  cores_e, mem_gb, away_s}` (read once), the charts' scales. Its NAS section (`web/buildpage/nas.js`) reads the same answers:
  the lead's `nas_ms`, `pool_nas_*` and `nas_disk_busy_pct` (missing from a member's answer); beside the
  status's `nas` (`NasView`). The
  Activity tab (`web/buildpage/activity.js`) asks every Mac every 2 s while it shows, with no series:
  their `files` and `net` for the file stream and the internet by website, each host's `robots`
  among them: a job crawling a host's pages (`FETCHES = "crawl"`) tells its proxy the robots.txt
  state (`POST /robots?host=…&state=unchecked|allowed|disallowed[&t=<read, unix ms>]`, crate::budget::net::Robots: a host name, at most 4096 hosts,
  with when), null for a host never told: an API's, a data download's or a page read as a person would, without robots.txt
  (docs/inputs.md §6.9).
- **Views** (menu bar, build page, `scenic lead`, `scenic status`) read the
  agent's `pool.lead` (`crate::agent::lead::View`) or the fresher `agent/pool/view.json`
  (`lead::own_status`): the declared lead (`LeadOf`: member, host, app; on the lead's own view,
  `since`, when it took the records up), whether this Mac leads, each member's state, "no lead"
  and why, the last change of lead.
- **The alert** (`lead::alert`, `ALERT_S`: an hour with no lead) says the members finish their jobs
  and wait, and to see to the lead's Mac; the menu bar's status line shows it.
- **History:** take-ups and step-downs are `term` events (the kind kept) in the member's
  history file (`crate::agent::lead::notes`): "took the records up in epoch E, from …", "stepped
  down (epoch E): …". Members joining and leaving are the lead's `member`
  events (`lead::membership`): at each read of the heartbeats, a member whose beat came into
  touch ("joined the pool") or went out of it (`OUT_OF_TOUCH_S`, "left the pool (out of touch)");
  a lead's first read only sets what it compares with, so a new lead or a restart notes none.

## 11. Controls

`crate::agent::lead`, `tools/status/main.swift`, `web/buildpage/pool.js`:

- **No control of who leads** (task #881): the menu bar, the build page and `scenic lead` show the
  pool (the lead, each member's state, who can't reach whom), and none moves the lead.
  The owner moves it only by editing the declaration (§6.1).
- **Build page:** Pause, and the asks below, from a page on any device (task #521: no page of the
  Mac's own first). On a Mac not leading, these go to its agent, which passes them on (the pause as
  its menu's, `crate::control::request`; the inputs' acceptances from their queues, as on the
  lead).
- **Cancel and try again from the page** (task #409): a page on any device (`/api/act`)
  cancels a running job
  and tries a failed target again. A cancel goes the way `scenic job cancel` does: on the lead to
  its agent; on a member, an ask in its agent's folder (`crate::control::request_cancel`), passed
  on to the lead whatever app it runs. A retry: this Mac's own back-off for the target forgotten at
  once; on the lead, its coordinator's failures of the target too (no worker held off it,
  `Coordinator::clear_failed`, in the state it writes this step, so a restart keeps it); on a
  member, mailed to the lead (`Msg::Retry`), only to a lead on its own app: with none, or one on
  another app, `/api/act` answers 409 and the page says why. Each mailed message is read alone, so
  one an app doesn't know is passed over and read past.
- **`scenic lead`** shows the pool; `scenic status` too.
- **Cancelling a job** (`crate::control::Cancel`): `scenic job cancel <target> [--step <step>]
  [--rerun-after-update]`, or the menu bar's "Cancel <job>" on the Mac running it (Option: "…,
  rerun on a newer app"), writes an ask to that Mac's agent (a file in `cancel-requests/`; one older than two minutes is dropped). The agent
  stops a slot of its own running the target at once, whatever its loop waits on (its jobs' own
  thread, `crate::agent::slots`, docs/agent-parts.md §4.5: SIGTERM, SIGKILL 30 s on), records it at
  its next round, and tells the lead's coordinator (in process, or
  `/work/cancel` from a member), which marks every live lease of the target and offers it to no one
  again from a plan begun before the cancel (`coord::lease::Cancelled`, saved with the leases: the
  next plan offers it as usual, a member's agent dropping a grant no newer than a cancel it saw
  as a second guard); a member's job hears
  it in its beat's answer (within a minute) and stops (a member on an app from before the cancel ignores it, and its job runs on). A cancelled job ends as given back, not
  failed: no retry delay, no backoff or guard hold against its Mac, its finished targets handed
  off, and its scratch entries named for the target (exactly, or with an extension) removed, so it starts again from the start.
  With `--rerun-after-update` the coordinator holds the target (`coord::lease::Hold`, saved with
  the leases and kept in the lead's store) from every offer and the lead's own starts until it
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
  that have a key to the lead's coordinator (`/api/redo`, the build's key: not a page's), whose
  agent keeps the ask in its folder (`redo.json`, over restarts) and plans a job of the `redo`
  step for it, the lead's own (`every::LEAD_ONLY`). That job changes no file (its write-set is
  empty) and hands off a done record of `<step> <target>` with no key; the merge checks it as any
  entry (§7.3), and takes it only when its member is the merging lead itself
  (`agent::poolside::by_its_lead`: another member's, or a lead's before the owner moved the lead,
  refused, its redo asked again), and `Keys::record` forgets those targets' keys and nothing else, the lease order
  kept by `<step>, <target>` as a prune's is (a build of the target leased before the redo,
  merged after, is passed over). The job succeeded, the ask leaves `redo.json`. The plan then
  offers the targets again under the keys it has; what reads their outputs follows by content
  (slope-hi's key names the terrain hi pack's content, the assemblies' keys their z6 tiles'
  mids), and the old files leave the manifest as the new ones replace them, for GC (its queue
  on the lead: an hour's grace once nothing holds them, docs/storage.md, GC). The coordinator answers the ask with the last lease it granted before it
  (`after`); the redo's job is leased later. The command then follows the newest records until
  the redo's merge lands in them: each target's last merged lease (`Records::last`) is later than
  `after`, so a job leased before the ask that merges meanwhile isn't taken for the redo; a lead
  without `after` in its answer is followed by each target's lease before the ask. Not until the
  keys are gone: the plan rebuilds the targets as the merge lands, and a rebuilt one has its key
  again within minutes. 15 minutes at most; a longer wait fails with that said, and an unreadable
  read is said and read again. A
  lead on an app from before `scenic redo` answers the post with an error: the command says so
  and asks nothing. An entry of a redo merged by such a lead clears nothing (that app records `<step> <target>` under the lo keys, as an unknown step's): the
  targets keep their keys, and `scenic redo`'s next dry run says so.
- **Pausing one Mac** (`crate::control::Hold`, task #224): `scenic pause --here` / `scenic resume
  --here`, `scenic pause <member>` / `scenic resume <member>` (host name or member id) from another
  Mac, or the menu bar's "Pause this Mac" / "Resume this Mac". The ask goes to this Mac's agent (a
  file in its folder, `hold-request.json`; `scenic pause` prints the agent's answer, and for another
  member's waits up to 20 s for the lead's: refused, it exits 1 with the lead's reason). A Mac's hold
  is kept in its agent's folder (`hold.json`, so a restart keeps it) and by the lead
  (`/pool/hold`, crate::coord::asks, docs/coordination.md §5.3): another Mac's ask goes to the
  lead with that Mac's next beat (queued on its disk until the lead takes it), a Mac's own is told
  the lead once it's the later, and each member has its own from the answer to its next beat, the
  later setting winning (a Mac's own ask is always later than its setting so far). A member whose app is older than the asking Mac's is refused ("too old to pause alone"). A paused Mac's running jobs stop
  at their next safe point and are given back (as the build's pause stops them, frozen past the
  grace), it asks for no work and starts none (`Conds::held`), and a lead paused keeps the lead,
  granting the others' work, and runs none of its own.
  It keeps beating. This Mac's own ask is taken up at once by the agent's control
  (crate::agent::asks, docs/agent-parts.md §4.1, task #871; #846 its first slice): a thread of
  its own woken by a kqueue watcher on the agent's folder the moment the ask's file lands (a
  sweep every 2 s behind it), never by the loop: the hold kept and answered, the running jobs
  told "drain" through their channels, and the beat told, which writes the heartbeat with the
  hold's stage then, here and to the lead (the lead's status on the NAS too), within a second on a
  hung NAS or during a plan. The agent's HTTP endpoint asks the
  same way (`POST /api/hold` `{"held": true|false, "member"?: …}`, from any device, answered
  once it's taken up), and so does an older CLI's file (the menu bar's quit writes its own). Another member's ask is taken at once by the jobs' own thread
  (crate::agent::slots, docs/agent-parts.md §4.5: the pool's lock tried every 100 ms, never
  waited on) and queued for the lead. This Mac's hold as the lead keeps it comes back in each
  beat's answer, and the loop applies it when it is the later (`Agent::tend_hold`); this Mac's
  own, when later, is told the lead. The status says the hold's stage
  (`Status::hold`, control::HoldView): asked, pausing (its jobs finishing, named), paused,
  resuming; and the Mac's state (`Status::verdict`). `scenic status`, the build page's machine
  card and its pool section (a Pause/Resume per member, task #873) and the menu bar print these (`Status::held` kept for older readers; the pool's view says
  "paused"); the forecast gives it no lanes. The
  rest of the pool goes on. `--now` stays the whole build's.

## 12. What is built

Built and always on: the declared lead and its epochs, coordination by HTTP on the lead
(docs/coordination.md: its store, the beat, hand-offs, mail, holds, settings, budgets, the
records' brief), the records on the lead's disk and at one path on the NAS, the driver, the views,
the steps table, the memory and disk guards, write-sets, every step to every member, every job by
grant (the lead's too) and packing, the equal-machines forecast, resuming (§7.2, §7.4) and brokers
(§8). There is no election, handover or takeover, no term, no shadow run and no pool switch: the
pool is always on.

A depth, terrain-hi or terrain-deep job resumes its target at the block after the last it
finished, from a log in its scratch folder on its Mac (§7.2, Resuming; crate::resume_log).

The pool's settings are the lead's (`/pool/settings`, crate::coord::asks; `scenic pool set`):
`placement` (§7.2; off while not set), `nas-budget`, `nas-target`, `batch-minutes` (§7.7),
`overdue-factor` (§10, Flags), and the map updates' `hold-catalog` and `near-slope` (docs/building.md §2;
off while not set), with no switch file. A new setting is made only once every member's
heartbeat shows the app that has it. A role is read as the agent starts (docs/roles.md §1) and is
no longer switched: `state/pool/slots` isn't read, but stays `on` on the NAS while an older app may
read it (one reading it missing or `off` restarts to change part). `state/pool/enabled` and
`state/pool/budgets` are read by no app.

Planned: resuming the terrain's and the slope's z6 tiles (after #639). Packing a terrain z6 tile's fetched raw tiles (`pack_raw_with`) stays inside that
z6 tile's job.

## 13. Testing

- **The simulator** (`crate::pool`'s `sim`, in the crate's tests; seeded, repeatable; task
  #1038): two to four Macs, any of which may be the declared lead, each in a
  thread of its own and one at a time. **Real:** the driver; the lead's coordinator in process
  (`Coordinator::in_process`: its routes, scheduler, lease table, hand-offs' registry, its store on
  a folder of its own with its durability wait, its epoch bumped as it opens);
  the member's beat on coordwire's client over a network fake (`Client::through`); the records'
  brief a job opens (`Briefs::fresh`, `GET /pool/brief`); GC on the lead (its queue, the map's
  feed and its clients' cursors, `/gc/chunk` and `/gc/gone`, the merge's refusal) with its grace
  (1 h) and cursor TTL (6 h). **Modelled:** the agent's loop around them (`sim::mac`: its slots,
  asks, beats, role and restarts), the jobs and their hand-offs, the map's publishing and the map
  servers' asks, the owner's app installs, the clocks, and the share: create-new whose bytes land late or never,
  renames landing late (a Mac asleep between), busy renames, stale reads (`Cfg::stale`), outages,
  lost answers, slow and stalling operations. **Faults** (each a knob, mixed by the seed, or all
  at once): requests dropped, delayed past the client's time (done then, unanswered), answers lost;
  Macs cut off for minutes to hours, one-way links; the lead killed mid-append (its log's last line
  torn or whole); crashes; skewed clocks, sleeps, app upgrades, development builds and rollbacks,
  saved states lost (the records on the Mac's disk kept), members leaving for good; reads failing
  each on its own, at 5 % or 20 % (`Cfg::flaky`, task #1154). The owner's hand move of the lead
  (§6.1) isn't modelled: the agent's test of it is below. **Checked
  at every step:** no target under two live leases (the jobs running and each coordinator's
  table), two coordinators never granting at once, no job id granted twice, every hand-off merged
  once or refused with why, acknowledgements only of what a landed snapshot names and no landed
  snapshot (but a late one, older than one landed, counted) lacking one, §4's epoch and records
  invariants, a job's brief never
  older than the one its lease was granted from, GC removing no name the records it saw name nor
  one a map server heard within the TTL serves; at the end, the declared lead leading alone and
  caught up, the one path naming every hand-off owed (but the double fault's, docs/coordination.md §5.3, and
  a Mac gone for good's), its take-ups settled (the run going on while the declared
  lead's take-up waits). Runs: 160 mixed seeds, 60 of every fault and 12 of GC through hours away
  in the suite, and 20 seeds at each of 5 % and 20 % of reads failing; by hand (`#[ignore]`):
  2,000 mixed and 2,000 every-fault seeds, 2,000 of four hours' faults, 2,000 at each rate of
  failing reads, each knob alone, and each of 400 seeds run twice for the same events.
- **Not modelled by the simulator:** the HTTP transport itself (coordwire's client is driven
  through `Client::through`), the agent's real loop, planning and jobs (`sim::mac` stands in), torn
  reads and other I/O errors of the share, the member lock, a wall clock set while awake, the map
  server's own reads, more than four Macs.
- **The placement simulator** (`sim::place`, in the crate's tests, a few seconds a run): N
  Macs, each with its memory, cores and disk, work a plan through the lead's real coordinator on a
  simulated clock, every job by grant. The plan is offered in its order, the steps kept to the lead
  (`bld-fetch`, `catalog`) as its own planned jobs. Members ask over HTTP with their capacity; the
  lead asks its coordinator in process (`ask_own`) as its agent does (`lead_grants`: its slots by
  its cores, alone for one job with its disk not bounded, else for its slots free with its disk
  room); each takes what `pack` grants. Each job takes its step's true memory and cores, and its
  disk written evenly over its run, the rest its reservation (an ask says its free space now less
  its room target and its reservations); its measures go back into the coordinator (crate::needs reads
  them), and its beats say what it has moved on the NAS so far (evenly over its run). A Mac may be
  slower to the NAS (`Cfg::pace`: its jobs take that many times their step's time, the same bytes
  over it, its NAS weight that much less; task #925). Checked at every start, first where a job may go (no Mac past its slots; lead-only steps
  on the lead, granted as its own jobs; one cold start of a step in the pool at a time, and one on a
  Mac), then what it takes (past what a Mac's jobs may hold above its default memory floor only a cold start, stopped by the guard;
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
- **The agent's tests** (`crate::agent::poolside`, `crate::agent::lead`, the agent's `poolside_tests`): two
  agents in one process, a lead and a member: a member never claims the lead, and with the lead
  gone keeps its jobs' hand-offs and waits (`pool::driver::tests::a_member_never_leads`,
  `with_the_lead_gone_a_member_keeps_its_jobs_hand_offs_and_waits`,
  `agent::lead::tests::runs::a_member_never_claims_the_lead_and_waits_for_it`); the lead's restart
  keeps its leases (`agent::poolside::tests::the_leads_restart_keeps_its_leases`); the owner's hand
  move of the lead, its folders copied to another member's
  (`agent::poolside::tests::a_lead_moved_by_hand_leads_from_its_copied_folders`); hand-offs by HTTP. Each piece of the protocol has its own tests (docs/coordination.md §8).
- **Two Macs on the real NAS** (`tools/check/pool-two-macs.sh`): the real agents on a scratch
  folder of the NAS, jobs played by a script. Planned: load.
- **The share itself** (`tools/check/nas-probe.py <peer> <scratch folder>`, run on one Mac, which
  times everything on its own clock): contended create-new and stale reads, stats and listings
  (§3's figures).

## 14. Decisions

- **The Mac mini is the fixed lead** (task #881, the owner, 30 Oct): always on, at home, on power.
  Members never lead; with the mini gone they finish their jobs and wait. Named by a declaration on
  the NAS made once (§6.1), not a setting on each Mac (two could disagree)
  nor anything a Mac's process makes (that would move it).
- **`tailscale serve` on every Mac** (`tailscale serve --bg 8090`, port 443) gives pages HTTPS;
  the agent finds it via `tailscale serve status --json` (on any port: the address keeps it) and
  puts it in its heartbeat. Without it, plain HTTP on the tailnet reaches every member, and an
  HTTPS page reaches only members with HTTPS. The owner also allows `scenic` through each Mac's
  firewall (§6.8).
- **The M6's on 8444** (`tailscale serve --bg --https=8444 8090`, #550): its port 443 is Caddy's,
  for https://map.brandontsang.net (docs/macs.md, Devices), and `tailscale serve` on 443 would take
  every connection to the tailnet address there, whatever name it asks (it can't share the port by
  name). Caddy serves the build page under `/build` too (`/build/`), and the page asks its own
  member under that path; its broker address stays the 8444 one (a broker's address is a root:
  the members' pages call `/api/…` on it).
- **Work order:** every slot from the front of the plan, in contiguous runs.
