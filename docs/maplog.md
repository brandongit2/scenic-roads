# The map and its log

**Task #883: steps 1 to 9 of §8 built; 10 planned (status and what's left: §11).** This doc says how catalogues and GC give way to one current map of the served
files plus an append-only log of its changes, which clients follow by cursor, and a GC that runs all
the time in small steps it can resume. No catalogue is written or read since step 6. Until step 10
lands, docs/storage.md (GC) and the code are what is true of GC.

**Built (steps 1 and 2):** `store::map` (entries ⇄ `Catalogue`, batches, the digest, applying a
batch, the NAS snapshot) and `store::map::log` (the store: `snapshot.json.zst` and segments of
`fsync`ed lines, in the lead agent's `maplog/` folder), written by `agent::publish::MapLog` on a
thread of its own (since step 6, the map the `catalog` step posts: §6.7). The lead's snapshot and
`map/<seq>.json.zst` are written when the log starts and then once an hour while batches come.
Where it departs from this doc:

- **No snapshot at a clean stop** (§4.4): the lead's log replays its segments on a start instead.
- **The log's thread starts with the lead** (`MapLog::here`, from the agent's loop once its
  coordinator runs): the log is opened at once and the coordinator answers from it before the next
  publish.
- **Memory:** the lead holds the map once, each entry's value as its canonical JSON
  (`store::map::Val`, the bytes its hash is over: §4.3), parsed only where it's read. #936,
  measured on the lead's log of 4 Nov (seq 1484, 52,089 entries, 80 MB of JSON of which the 223
  regions are 76 MB): the map holds 85 MB (691 MB as `serde_json::Value`s); a snapshot's decode
  peaks at 89 MB (734 MB), the log's open at 194 MB (978 MB), and a batch
  setting a 27 MB region is parsed and applied in place at 54 MB (403 MB). A batch's values are
  shared with the map it's applied to. The catalogue made of it (`Map::catalogue`) is 376 MB as
  before, parsed from the values in about 0.3 s (0.1 s when it cloned them). A posted map's entries
  are made one at a time to diff and check it, a batch is applied in place (to a copy only while a
  reader holds the map), and the snapshots are streamed into zstd.

**Built (step 3):** the lead's coordinator answers `GET /map/changes` and `/map/snapshot`
(`pipeline::coord::maproutes`, with the build's key) from `store::map::feed::Feed`, which the
shadow's writer keeps: the map, the batches it appended or replayed at its open (64 MB of them at
most; a `since` older than those, or past the head, is told to reset), the cursors heard (saved with
the log's snapshot) and the snapshot made once per `seq`; no NAS, no disk. A long poll looks each
50 ms whether a batch came. The map server (on the Mac mini alone, task #874) follows it
(`crates/server/src/maplog.rs`, docs/macs.md): batches applied in place to its own map (no reader holds it), the catalogue made of
it handed over whole; the NAS's snapshot with the lead away; its map kept in
`~/Library/Caches/scenic/map/`. Where it departs from §5 and §6.1:

- **Wherever the server runs:** the Mac mini alone since #874, so no gate of its own. Its map
  costs what the lead's does (above), and the catalogue made of it at each batch beside it (the
  one before still held while it's swapped). Measured on a copy of its saved map (seq 1457, 51,473
  entries) through the server's steps in a test (#936's review, not the server process itself):
  about 0.52 GB with the map and its catalogue held (1.1 GB with `Value`s), 0.89 GB while the next
  catalogue is made beside it (1.46 GB), 0.91 GB at a save (1.49 GB).
- **The catalogue is made from the map at each batch** (`Map::catalogue`), as the consumers still
  read a `Catalogue`.
- **No save at a clean stop** (§5.3): at most every 10 minutes; a start asks the batches since.
- **The lead's contact** is `state/coordinator.json`, kept on the Mac (`lead.json` in the cache
  folder, the key in it, mode 600) for a start away from the NAS.
- **The CLI** (`scenic catalogs`) reads this Mac's agent's Catalogues map as it keeps it (step 4).

**Built (step 4):** the agent and the build page read the map from the log, not catalogues:

- **The map the plan reads** (`served()` before): `agent::publish::Here`, which the shadow keeps
  beside its feed: the catalogue it last checked equal to the map, else one made of the map
  (`Here::catalogue`), kept only while a reader holds it (#930: ~370 MB, made again in ~0.1 s, so a
  plan may make it twice); the plan's `map_now`. Without the lead's log (a lead whose log hasn't
  started, a dry run) the newest NAS snapshot, read once a `seq` (§5.2). The status's catalogue is
  its `seq` and its batch's time; the plan's regions on the map (`on_map`), its map tiles' owners
  (`pubtimes::PlannedNotes`) and the page's regions (`regionsview`) read that catalogue. `served`'s
  per-folder cache and its `read_n_from` are gone.
- **The log's marks** (§6.8): the log store keeps, for each watch the lead opens it with, the last
  batch that marked it, saved with its snapshot and kept up as batches are replayed and appended
  (`store::map::log::Log::open_watching`, `marks`; `store::map::marks`). A watch is a key prefix,
  or `why:` and a part of a batch's `why` (parts after the first, each after `; `). A prefix newly
  watched starts at the snapshot's `seq`. The feed carries them to the agent.
- **names-todo** keys on `map <seq>`, the last batch touching any of `namestodo::INPUTS` (the
  sections §6.8 lists, `m/units` and `m/regions`, and their files: `f/base/`, `f/hidata/`,
  `f/markdata/`, `f/ovdata/`, `f/layers/labels/hi/`, `f/global/spoken`, `f/global/roaden/`, by the
  steps' names for them). Those file prefixes are checked against the map at each plan: a file one
  of those sections names outside them (a step renaming its outputs) makes the key the map's own
  `seq`, said in the log (the lists made each batch, never a change missed). The first plan after
  the deploy makes the lists once (the key changes from the hash to `map <seq>`); `inputs_key` stays the job's own key of its roads' pieces (it reads a
  catalogue until step 6).
- **`LastCatalogue`'s `at` and `low_at`** go: when the last catalogue went out is the head batch's
  time; when the low zooms last went out is the last batch whose `why` says `the low zooms`
  (`publish::LOW`: the shadow writes "catalogue N; the low zooms" when the plan that made it had
  them, `PlanView::catalogue_low`). A log with no such batch yet says none (the low zooms' gate
  open, as with no record before). `held_at` and the cadenced steps' runs stay in `publishing.json`.
  The pre-#254 `map-update.json` migration goes with `low_at`.
- **The adoptions** (§6.1, §6.6): a cursor keeps when it was first heard at its `seq`
  (`store::map::Cursor::at`); the status's `adopted` is the `server/<host>` cursors, and the lead
  notes each one's move in the history (`adopt`, its `worker` the Mac) for `pubtimes`; and the
  `mirror/<host>` cursors' rounds (`mirror-start`, `mirror-end`) the same way (step 7b).
- **The Catalogues map** (`cataloguesview`, docs/pool.md §10) follows the log, each loop and each
  ask (20 s apart at most): the lead's feed in process, on the lead alone (the build page is the
  lead's; a member's port sends `/work/catalogs` to it, `coord::memberpage::answer`). It keeps, in the agent's folder
  (`catalogues-view.json.zst`), its entries, the `seq` they reach, each tile section's key with its
  name's hash and each `f/` name's hash with its file's and last publish (a soloed step's tiles' ages,
  #994): a batch setting a file again unchanged (its
  size alone) publishes nothing, as before. It reads no catalogue and keeps no hashed file name per
  tile key; `whole` goes from its entries. Where it departs from §6.6: a view with nothing kept
  starts at the map's `seq` with one `first` entry (every tile the map names: what was there before
  isn't known), and one told to reset (the log no longer holds its batches: a new lead's log, a
  member away long) takes the map again with no entry for what it missed, said in `error`. Its depth
  is what it has followed since it began, not the log's (the lead's log keeps an hour of batches on
  disk). `/work/built`'s `published` reads its `f/` names.
- **Departures:** `regionsview` sums every region once a `seq`, not only those the batch's keys
  touch.

**Built (step 5):** each download keeps its own list (§6.2): `downloads.json` (fmt 5) holds, by
`seq`, each download's content names and sizes and basemap pieces, written for the `seq` served
while it's served (`partial` while some of its ways' owners aren't known, their hidata unread:
made again from its catalogue once they are, and while one is read no base pack or old catalogue
goes) and kept for each `seq` a download is at or updating to
(`server::downloads::Downloads::keep_lists`). What a download keeps, what changed and what an
Update copies are its list against its part of the map served. Where it departs from §6.2: the
batches since a download's `seq` aren't read (§6.2 says why). The served map's part is made from
the catalogue made of the map. A download with no list at its `seq` (the map moved on before the
mirror's round listed it, and the server restarted since; a file of fmt 4) reads at the map served,
as one whose catalogue is lost (`Plan::eff`): no catalogue is kept on the Mac (step 6).

**Built (step 6):** catalogues stop. The `catalog` step builds the map from the records as before
and posts it whole to its agent (`--post`: `maplog/posted.json.zst`, the catalogue encoding, a
temporary name renamed into place); at the job's end the agent's writer takes it (renamed
`posted.taken.json.zst`), diffs it against the map, appends the batch and lets the file go (one
taken and not appended when the agent stopped is appended when the log opens again). Its `seq` is
the step's number, the map's next (the head's next if that's taken). The step reads the map from the
lead (`coordwire::map_now`: `GET /map/snapshot` from the coordinator; the NAS's newest snapshot
with no coordinator or none open yet), as names-todo and `scenic` status do; each file's size is the
records' or the map's, the NAS looked at for the rest (the `catalog-stats:` line's `stated`). Held
for review, the step writes the map it would publish whole to `map-held/<seq>.json.zst` (a snapshot,
the newest three kept) and posts nothing; the plan reads its regions from there; the hold lifted,
the step's next run publishes that map as one batch (made again from the records). The map server
serves the map alone: the map this Mac kept (`current.json.zst`), else the NAS's newest snapshot, at
its start (before it answers), then the lead's batches; a `--root` server or a test reads the root's
newest snapshot. GC removes by its queue on the lead since step 8b (§7.3). Where it departs from §6.7: the map is
posted as a file the job leaves for its agent, not a `POST /map/publish`: the job runs on the lead,
and the agent takes it when the job ends, which a request would only make sooner.


Words: **the map** is the current name → file map (what a catalogue holds today); **the log** is
its batches of changes, each with a sequence number (`seq`); a **client** is anything that holds a
copy of the map at some `seq`. "Manifest" stays the build's (the records' logical → content map,
docs/storage.md); this doc never uses the word for the map.

## 1. The problem

Catalogues and GC were designed for a rare publish of a whole region: a catalogue is a whole
snapshot of everything served, and GC's roots are every catalogue of the last 14 days. Publishing
is now a catalogue a minute while the build runs (catalogues 471 to 791 in a few days), each a
small change, and three costs grew with that:

- **Readers rebuild the map from files.** A catalogue is a whole file (about 9 MB) or a delta on
  one; a reader lists `catalog/` over SMB and applies a chain. #895 cut the chain's read from 755 s
  to 19.6 s (one JSON value patched in place, digests at the ends only, the last two kept on the
  Mac), but every reader still lists the folder, every new chain needs a 9 MB whole write now and
  then (`whole_due`), and a Mac away from the NAS sees no new map at all.
- **GC never finishes.** Each run reads every catalogue of 14 days (hundreds, through their chains)
  to learn what's referenced, then lists and stats every file under every top folder on the share
  (`agent::gc::run_once`, "sweeping folders"): millions of SMB metadata calls, at the share's
  speed, beside the build. It learns what became garbage by looking at everything, every day.
- **Reuse costs a write.** A job reusing a file already on the NAS touches it (`Out::put_file`) so
  GC's age rule doesn't take it: a per-file SMB write in the upload path, there only for GC.

The root cause is one: the publish records only *what is*, so every reader and GC must rediscover
*what changed* by comparing snapshots or by listing the share. The fix is to record the change
itself, once, where it happens (the lead, which alone publishes and merges), and to let readers and
GC consume changes.

#872 (a stopgap for GC) is abandoned; this replaces it.

## 2. What #895 taught, and what carries over

- **Never read back from a whole snapshot to move forward.** A client applies only what's new to
  what it holds. Here that is the protocol, not an optimisation: a client asks for the batches
  after its `seq`.
- **Check the result, not every step.** A chain checked its digests at the ends. Here each batch
  carries the map's digest after it, and the digest is a sum of per-entry hashes (§4.3), so a
  client checks every batch at the cost of the entries it changed, not the whole map.
- **Keep the materialized map on the Mac.** #895's `~/Library/Caches/scenic/catalogue/` becomes the
  client's saved map (§5.3), so a restart asks only for the batches since.
- **A damaged chain is found by bisection** (`first_wrong`): here there is no chain to bisect; a
  batch whose digest doesn't match makes the client fetch the snapshot (§5.2), and the lead logs it.

What does not carry over: the delta format (`{"=", "o", "a"}` patches of a nested document), the
chain and `MAX_CHAIN`, `whole_due`'s timings, the kept files' digests in their names. A flat map
needs none of them (§4.1).

## 3. Where it lives: on the lead

The map and its log are kept by the lead's agent on its own disk, beside the pool store of
docs/coordination.md (#878), and served by its coordinator (port 8090) over HTTP. The NAS keeps a
whole snapshot of the map now and then (§4.4), for readers that can't reach the lead and for
recovery.

Why the lead, not the NAS:

1. **One writer already sits there.** Publishing is the lead's alone (`LEAD_ONLY`: the `catalog`
   step), and so is merging hand-offs into the records. Both of GC's sources of "this file is no
   longer named" happen in that one process; on the NAS they'd be rediscovered by reading.
2. **A batch is a few kB; SMB is slow at small files.** One create per publish is affordable, but
   readers *polling* the share for it are not: a listing of `catalog/` each 10 s from every Mac,
   listings stale for up to 33 s, names for 40 min (coordination.md §1). An HTTP request answered
   from the lead's memory costs the share nothing and is never stale.
3. **Members away on Tailscale.** A Mac away from home reaches the lead over the tailnet but the
   share only slowly or not at all. With the map on the lead, its server follows every publish
   (and serves what its mirror holds); today it follows none.
4. **coordination.md's rule holds:** the lead's handlers answer from memory and its own disk,
   never the NAS (its §2, rule 3). The map's routes are handlers of the same kind.

What it costs: with the lead down nothing new is published anyway (publishing is the lead's), so
clients keep the map they hold, which is what happens today when the catalogue job stops. A Mac
that starts with no saved map and no lead reads the NAS snapshot (§5.2). A lead disk lost is §4.5.

The log is its own store (`agent/maplog/`), not a part of #878's pool store, so this work doesn't
wait on #878; whether to fold it in once #878 lands is open decision 1.

## 4. The map, the log and the snapshot

### 4.1 The map: flat entries

The map is a set of entries `key → value`, flat. Every catalogue field (docs/formats.md, Catalogue)
becomes entries at the grain a change touches:

| Key | Value | From the catalogue |
| --- | --- | --- |
| `f/<logical>` | `{file, size, fmt}` | `files` |
| `l/<layer>` | the layer's head: encoding, zooms, `root` | `layers.<l>` without its maps |
| `l/<layer>/<lo\|hi\|deep\|polar\|native_zoom>/<tile>` | a logical name (or a zoom) | `layers.<l>.<section>` |
| `s/<section>/<key>` | a logical name | `base`, `roads`, `hidata`, `markdata`, `ovdata`, `global` |
| `u/<z6 tile>` | `true` | `units` |
| `r/<region id>` | the region, with its place in the recipes' order | `coverage.regions` |
| `m/<name>` | the value | `meta`, `credits`, `dem_labels`, `basemap`, `coverage.recorded`, `app`, `fmt` |

Readers don't see entries: the client library (`store::map`) materializes them into today's
`store::catalogue::Catalogue` (`n` = the map's `seq`, `created` = its batch's time), so the
consumers in §6 change how they *get* a catalogue, not what they read in it. A test holds that
entries → `Catalogue` → entries is the identity on every catalogue on the NAS today.

### 4.2 The log: batches

One publish is one batch, appended and `fsync`ed before it's answered:

```json
{"seq": 812, "at": "2026-11-02T04:05:06Z", "app": "<version>", "why": "map tiles 6/32/21 …",
 "set": {"f/layers/roads/hi/6-32-21": {"file": "layers/roads/hi/6-32-21.<hash>.pack", "size": 123, "fmt": 1}},
 "del": ["f/base/6-1-1", "s/base/6-1-1"],
 "freed": ["base/6-1-1.<hash>.pack", "layers/roads/hi/6-32-21.<oldhash>.pack"],
 "digest": "<32 hex>"}
```

- **`seq` continues the catalogues' numbers**: the first batch is the last catalogue's `n` + 1.
  Every place that compares catalogue numbers (the page's `n`, `downloads.json`'s `cat`,
  `pubtimes`, the history's `catalog` events) keeps working with no translation.
- **`freed`**: the content names the map named before this batch and doesn't after (§7 reads it).
  The writer works it out from the entries it replaced; no reader diffs anything.
- **A batch is atomic for its clients** (§5.1): the set of z6 tiles, packs and coverage it carries
  is what the catalogue gate (`build::CATALOGUE_GATE`) let through as one publish, exactly as a
  catalogue is today, so the map is never seen half-published.
- An empty change makes no batch (today's `catalogue_key` rule: nothing changed, nothing written).

The log lives in segments, `agent/maplog/<first seq>.jsonl`, a new one each 10,000 batches or
64 MB; a segment whose batches are all older than the oldest kept cursor (§5.4) *and* older than
the newest NAS snapshot is removed.

### 4.3 The digest

The map's digest is the sum, modulo 2^128, of XXH3-128 over each entry's key and canonical value
(a multiset hash). A batch changes it by subtracting each replaced entry's hash and adding each new
one's: the writer and every client update it in time proportional to the batch, never the map. A
client checks it after every batch; a mismatch means its copy is wrong, and it fetches the snapshot
(§5.2). The snapshot carries the digest too; a test holds the incremental digest equal to the
from-scratch one over every real catalogue converted (§8, step 1).

What it catches: a map holds each key once, so the entries are a set, never a multiset with an
entry twice (where a sum can't tell `2·h(a)` from `h(b) + h(c)`). The digest kept is always the sum
over what the map really holds (each entry removed subtracts its own hash, each added adds its own),
so a batch that leaves a map other than the writer's passes only if the two sets' differing entries
sum to the same, a 2^-128 chance for hashes that behave as random (XXH3 isn't keyed: nothing here
is adversarial). The hashed bytes are injective: key, a zero byte, then JSON, which holds no raw
zero byte. A stronger check at the same incremental cost isn't needed.

### 4.4 The NAS snapshot

The lead writes the whole map to `map/<seq>.json.zst` (zstd with its checksum, temporary name, then
rename) once an hour while batches come, and at a clean stop: one whole write an hour (34 MB: the
NAS's `map/1601` to `1607`, 6 Nov) instead of one create a minute plus a whole one each `whole_due`. It keeps the newest three. Who
reads it: a server with no saved map that can't reach the lead, the CLI on a Mac without the lead
(`scenic catalogs`), the lead rebuilding a lost disk (§4.5), and the tools that write test roots
(`lakefill catalog-add`, `coastcheck`, `layers.rs`' tests), which write a snapshot where they wrote
a catalogue.

### 4.5 Recovery

- **The lead restarts:** its log is on its disk; it loads the newest local snapshot (`agent/maplog/
  snapshot.json.zst`, written as the NAS one is) and replays the segments after it. A torn last line
  is dropped: it was never answered.
- **The lead's disk is lost:** the map comes back from the newest NAS snapshot. Batches after it are
  lost, which costs nothing lasting: the next `catalog` run computes the map from the records (as it
  does today) and the batch it appends is the difference. Clients ahead of the restored `seq` are
  told to reset (§5.2). The GC queue is rebuilt as §7.5 says.

## 5. Clients

### 5.1 Asking for changes

`GET /map/changes?since=<seq>&client=<id>` on the lead's coordinator answers, from memory:

- the batches after `since`, up to 4 MB of them (`more: true` past that), or
- `{"reset": <seq>}` when `since` is before the oldest batch the lead holds, or ahead of the head (a
  restored lead): the client fetches `GET /map/snapshot` (the whole map at the head, zstd) instead.

The client applies a batch only whole, to a copy, then swaps the copy in (`Arc` swap, as the
server's `cat` today): a request reads one map from start to end, as now (docs/macs.md: the map
never reads one tile from two catalogues). Several batches in one answer are applied in turn and
swapped once. Files are content-named, so applying a batch moves no data: a new name means a new
file already on the NAS (the gate published it only once its upload had landed).

`?wait=30` long-polls: the lead answers when a batch is appended or after 30 s, so a client follows
a publish within a second without polling every 10 s. The agent's POST of `/api/catalog/refresh`
after a catalogue (docs/macs.md) goes; the long poll does its job on every Mac.

### 5.2 When the lead can't be reached

The client keeps the map it holds and tries again each 10 s (logged once). One with no map at all
(a fresh Mac, a lost cache) reads the newest NAS snapshot and goes on from its `seq` once the lead
answers. Nothing reads the log from the NAS: there is none there.

### 5.3 What a client keeps

The materialized map at its `seq`, in `~/Library/Caches/scenic/map/current.json.zst` (temporary
name, rename), written at most every 10 minutes and at a clean stop, not at every batch (34 MB a
minute would be waste). On a start it loads that and asks for the batches since. Its digest is
checked on load; a file that doesn't match is set aside and the snapshot fetched.

### 5.4 Cursors

Every `changes` request says the client's `seq` and id; the lead keeps, per client, the last `seq`
it applied (the `since` of its next request) and when it was heard (in the log store, saved with
its snapshot, not `fsync`ed per request). The clients:

| Client | Id | Holds files on the NAS? |
| --- | --- | --- |
| each Mac's map server | `server/<host>` | yes: it reads the NAS for what its mirror lacks |
| each Mac's mirror sync (step 7b) | `mirror/<host>` | yes, while it copies a `seq`'s files |
| the lead's agent (plan, status, page views) | in process | no: it reads names, not files |
| a job leased with files of the map as inputs (names-todo, regions' checks) | its lease | yes, until it ends (§7.2) |
| the NAS snapshot | `snapshot` | yes: a reader may start from it |

A client unheard for `CURSOR_TTL` (7 days) is dropped from the set: a laptop away for a month
doesn't hold GC. When it comes back its `since` is likely older than the log's oldest batch, and it
resets from the snapshot; its own mirror holds its downloads' files whatever happened on the NAS
(§6.2). A mirror (`mirror/<host>`) is reset too whenever the lead didn't hold its cursor, its
`since` within the log or not (`store::map::feed::MIRROR`): one never heard (a new Mac, the switch,
a cursor lost with a lead restart), unheard `CURSOR_TTL`, or held at one `seq` behind the head that
long (an Update that never gets through, its files missing or no room, holds GC a week at most).

The cursors are the lead's memory, saved with its snapshot (hourly, `SNAPSHOT_EVERY_S`) and
restored at its start. A restart loses only what was heard since: a cursor that moved on is
restored older (it holds more), and one first heard since is gone until it asks again (a server
within its long poll, a mirror at its next round); a mirror that was gone is then reset, as above.

The browser pages are not clients. A page holds logical names and versions (`?v=`) and asks its
Mac's server, which resolves them in the map it holds now; a page with an older map gets current
files for its logical names, as today. The page's switch to a new map stays keyed by `n` (now
`seq`) through `/api/catalog`, unchanged.

## 6. Every reader of catalogues today, and what it reads instead

### 6.1 The map server (`crates/server`)

- `Data::refresh_catalogue` → `store::catalogue::latest_nas_from` on a listing of `catalog/`: a
  thread of `store::map::Client` long-polling the lead (§5.1), applying batches; `cat` and
  `generation` as today. `poll_every`'s schedule (10 s / 30 s / 10 min by the lead's heartbeat) and
  its read of `state/status.json` for it go: the long poll is as fast and costs the share nothing.
- The local copies in `catalog/` (offline starts) → the client's saved map (§5.3).
- `publish_event(.., "adopt", n)` to `state/publish/<host>.jsonl` → the cursor itself: the lead
  logs each client's first request past a `seq` as its adoption (§6.6). The mirror's
  `mirror-start`/`mirror-end` events ride on its own client's requests (`&copying=<seq>`,
  `&synced=<seq>`: §6.2).
- `main.rs`'s start (read the newest local catalogue, else the NAS's) → the saved map, else the
  lead's snapshot, else the NAS snapshot.

### 6.2 The mirror and offline downloads (`store::mirror`, `server/src/downloads.rs`)

- A download is pinned at a `seq` (`downloads.json`'s `cat`, the same number). It keeps its own
  list of content names (each names its logical name) with their sizes, and its basemap pieces, at
  that `seq` (`downloads.json`'s `lists`, by `seq`): written for every download at the `seq`
  served while it's served, kept for each `seq` a download is at or updating to. No whole catalogue
  is kept on the Mac for a download (built, step 5; a file of format 4 has each list made once from
  the catalogue kept for it, then those go).
- "Out of date, with what changed and the size to copy": the download's list against what it is in
  the map served, one pass over its own items, however far behind it is. (Built so rather than
  from the batches since its `seq`: what an Update copies is the download's part of the map served,
  which the server works out anyway, and new keys of its region aren't in its old list for a
  batch to meet; so the batches add nothing, and the server keeps none.)
- `Data::set_older`/`content` (the download's older files read first) are unchanged: they read the
  download's list.
- The mirror's sync is a client (`mirror/<host>`, step 7b: `crates/server/src/maplog.rs`'s
  `MirrorLink`). Each round with the NAS, before it copies, it asks `GET /map/changes?since=<s>
  &client=mirror/<host>&copying=<n>&synced=<y>` (no wait): `s` its cursor, the oldest `seq` whose
  files it still copies from the NAS (`downloads::Plan::copies_from`: the map served; an Update's
  older target while its files aren't all here; a download's own `seq` while what it keeps there
  isn't, which the downloads' advance moves on), so GC keeps them until it's done or moves on
  (today the 14-day window did that, by accident of its length); `n` the map its round copies and
  `y` the last it got through at, kept on the cursor (`Cursor::copying`, `synced`, each with when
  first said) for publishing's times (§6.6). A round through says it at once.
  - **Durable:** the cursor is made each round from what's on the Mac's disk (`downloads.json`'s
    pins, the files in the mirror), nothing of its own, so a restart says the same one, and it is
    never newer than what the mirror lacks. A pin moves only once its files are all here (files
    content-named, each copied under a temporary name): a round cut short (a file missing, a new
    map, a kill) leaves the cursor where it was.
  - **Reset** (`{"reset"}`: older than the log, the Mac away long enough for the lead to hold no
    batch from there; or a cursor the lead didn't hold, §5.4: never heard, unheard `CURSOR_TTL`, or
    a week at one `seq` behind the head): the mirror's sync is whole each round (what's wanted
    against what's here), so it needs no snapshot; what a reset changes is that an older map's
    files may be gone from the NAS, so each Update of a map older than the one served goes on at
    the one served (`Downloads::resync`). What a download keeps is on the Mac and stays.
  - **Away from the NAS or the lead:** with no NAS the mirror copies nothing and says nothing (it
    lets go only what no download wants); with the lead away it copies as before, its cursor said
    again next round. The map serves what's here either way.
  - **Old servers:** a server from before 7b says no mirror cursor; step 8b deletes by the queue
    only once every Mac's server is a release past 7b (met on 7 Nov: §7.3).
  - The batches answered are passed over: the downloads compare their own lists (above), and the
    server's own client holds the map.

### 6.3 The map app (`web/src/catalogue.ts`, `sw.js`)

Unchanged in shape: `/api/catalog` answers the server's map as a catalogue status, `n` its `seq`.
The page's minute poll stays (it asks its own Mac, not the NAS). The versions (`/api/meta`'s
`packs`, `?v=`) are per pack and come from the map as before, so a new batch re-fetches only the
changed packs' tiles, and the in-place switch (`onSwitch`'s `changed`) can now be given the batch's
changed keys rather than a diff of two metas.

### 6.4 The consistency of tile sets

What makes a publish consistent today is the gate (`CATALOGUE_GATE`, `CATALOGUE_CAP_S`, the
hand-offs waited for) deciding *what* goes into catalogue n, and readers taking a catalogue whole.
Both stay: the gate decides what goes into batch n; clients take a batch whole (§5.1). A z6 tile's
base pack, road values, hi data and hi packs change in one batch, never split across two. The
held catalogue (`inputs/hold-catalog`, `catalog-held/`) becomes a held batch: the lead appends
nothing while the hold is on and writes the batch it would append to `map-held/<seq>.json.zst` for
review, whole (one file); lifting the hold appends the newest such batch (recomputed against the
map) as one.

### 6.5 The agent

- `served()` / `served_catalogue` (`agent/mod.rs`, kept per folder, read from the last with
  `read_n_from`, #684's fix) → the lead's in-process map: the log store's current `Arc<Catalogue>`,
  no read at all. On a member (its page, `on_map`) → the same client as the server's. The held
  folder's catalogue → the held batch applied to the map.
- `publish::LastCatalogue` (`publishing.json`: `at`, `low_at`, `held_at`) and the status's
  `published` → the log's head batch (`seq`, `at`) and the newest batch with the low zooms (the
  batch's `why` says which parts it carries). `held_at` stays: it's the gate's clock.
- `build::catalogue_key` (whether anything to publish changed) stays as the batch's trigger; the
  batch is the difference, so an unchanged key and an empty difference agree (a test).
- `regionsview` (bytes per region once per catalogue) and `builtview` (a tile is published while
  the latest catalogue names its output) → the map, with the batch's changed keys saying which
  regions' sums and which tiles to look at again, not every one each catalogue.
- `forecast.rs`, `every.rs`, `steps.rs`, `placement`/`pool` (the `catalog` step as `LEAD_ONLY`, its
  folder names) → the step's new output (§6.7).

### 6.6 The build page: publish history and ages

- `cataloguesview` (`/work/catalogs`, the Catalogues map, task #629) reads every catalogue on the
  NAS, oldest first, to find what each one published: a tile is published by a catalogue when a file
  it names for that tile is new or other. From the log this is direct: a batch's `set` and `del`
  keys *are* the tiles it published and dropped. The view tails the log in process (on the lead) or
  by `GET /map/changes` (a member's page), keeps the same entries (number, time, whole or not →
  always a batch, the z3/z6/z10 tiles), and no longer keeps 18,000 hashed file names to tell a
  whole catalogue's tiles from the last's. Its depth is the log's: GC's window no longer bounds it
  (the "first" catalogue that publishes every tile it names goes).
- A tile's **age** on the page (published at) is the `at` of the last batch whose keys touch it,
  kept per tile by the view as it tails.
- `pubtimes` (`scenic timings publish`): the catalogue events (`catalog` in `coord/history.jsonl`,
  the `catalog-stats:` line) stay as the step's; "the servers adopted it" comes from the cursors'
  adoption times (§6.1), and the mirrors' rounds from their cursors (§6.2), instead of
  `state/publish/<host>.jsonl`, which went with step 7b.

### 6.7 The catalogue step (`scenic-build catalog`)

Today: read the last catalogue (through its chain), build the new one from the records, write it
whole or as a delta (`whole_due`), each file's size from the records or the last catalogue.
Instead: build the new map from the records as now (`stage`, `areafiles`, the coverage, credits and
`dem_labels` code is unchanged: it fills a `Catalogue`), convert it to entries, and post it to the
lead's agent (`POST /map/publish`, local, carrying the full entry set's digest and the `seq` it was
built against). The agent's writer diffs it against the map, appends the batch (with `freed`), and
answers its `seq`. A map built against a `seq` that's no longer the head is diffed against the head
all the same (the step's map is whole; the difference is what's new). The step reads the map from
the agent (in process memory, one local request), not the NAS. Sizes: the records' as now; a file
neither the records nor the map size is looked at once, as now.

`state/pool/delta-catalog` and `agent::publish::Switches::delta` go: there is one way to publish.

### 6.8 names-todo (`namestodo::inputs_key`)

Today it hashes every relevant logical and content name of the whole catalogue each time it's
asked. Instead the key is the `seq` of the last batch that touched any of its prefixes (`u/`,
`r/`, `s/global/spoken`, `l/labels/hi/`, `s/base/`, `f/global/roaden/`, `s/hidata/`, `s/markdata/`,
`s/ovdata/`) and of their files: the log store keeps, per prefix the agent registers, the last
`seq` that touched it. names-todo reruns exactly when its inputs changed, as now, without hashing
the map. Its read of the catalogue (`read_n_from`, the phase "reading the catalog, …") → the map
from the agent.

### 6.9 GC (`agent::gc`)

§7. Its "reading the catalogs" phase and the folder sweep as the way to find garbage go.

### 6.10 Everything else that reads catalogues

| Reader | Today | Instead |
| --- | --- | --- |
| `scenic catalogs` (`cli/scenic.rs`), `cli/build.rs`, `cli/progs.rs` | list and read `catalog/` | the agent's map or `GET /map/…`; the NAS snapshot without a lead |
| `pubtimes` | reads catalogues for their times | the log's batches |
| `coverage.rs`, `areafiles.rs`, `stage.rs`, `layers.rs`, `places.rs`, `summary.rs`, `marks.rs`, `depth`, `polar.rs`, `rail.rs`, `ovconv.rs`, … | take a `Catalogue` value | unchanged: they get it from the client |
| `lakefill catalog-add`, `coastcheck`, test roots (`layers.rs`, `agent/mod.rs` tests, `downloads/tests.rs`) | write a catalogue into a test root | write a NAS snapshot (`map/<seq>.json.zst`) into it; the server under test starts from it |
| `scenic-build`'s `catalogue::keep_read_in`, the server's, `scenic`'s | the kept chain ends | the client's saved map (§5.3) |
| `pool/sim/place.rs` | the step's name | the step's name |

Every stored or wire name that keeps the old spelling today (docs/formats.md) keeps it where it
survives: the step `catalog`, the route `/api/catalog`, the history event `catalog`, JSON keys
`catalog_t` and the like, the page's `/work/catalogs`.

## 7. GC: continuous, by the log

### 7.1 What holds a file

A content-named file on the NAS is **held** while any of these names it:

1. **the map** at any `seq` a client in the cursor set (§5.4) still holds;
2. **the records** (the merged manifest, keys' outputs, pending uploads), which the lead alone saves;
3. **a hand-off taken and not yet merged**, which the lead alone holds (with #878; before it, the
   journal entries `unmerged` reads today).

The lead sees every change to 1 and 2 as it makes it, and 3 as it takes it. So GC doesn't look for
garbage: the lead **queues** it as it's made.

### 7.2 The queue

When a batch's `freed` names (§4.2) or a records save drops a content name, the lead checks the
other holders (an in-memory index of the records' and the map's content names, kept by the same
writer) and, if none holds it, appends `{name, freed_seq, freed_at}` to the GC queue
(`agent/maplog/gc.jsonl`). A name taken up again before it's deleted (a hand-off or a batch names
it) leaves the queue. A file is **due** when:

- every client's cursor is past its `freed_seq` (the map no client holds names it), and
- `GRACE` (2 days) has passed since `freed_at` (a margin for clients outside the cursor set, and
  for the owner to look at a bad publish before its files go), and
- every live lease was granted after its `freed_at` (a job running since then may have looked it up
  by name; leases are clients too, §5.4), and
- the records and the map still don't name it (checked again at the moment it goes).

Built (step 7, `store::map::gc`, used by `agent::gcqueue`): the queue is one file of JSON lines in the map log's folder,
appended and `fsync`ed once per change (a batch, a records save, a deletion), rewritten whole once
it has grown to four times what it holds; it survives a lead restart. The holders' index is memory
alone: the map's content names from the log's map at the open and each batch, the records' (the
manifest's and the pending uploads') from each records save the lead's agent makes; nothing is due
until the records have been seen. The first records seen start the index: what a lead that wasn't
running missed is the orphan sweep's (§7.5), as is the queue lost with a lead disk. A hand-off
taken and not merged isn't indexed until the merge takes it: a reused name is either held by a
lease granted before it was freed (the third rule), or refused at the merge (§7.4); one the merge
takes holds its names from then until the records after it are seen (`GcQueue::bringing`, step
8b). The NAS snapshot is a client (`snapshot`, its cursor the oldest of the snapshots kept there).
Deletions are kept for the merge's refusal while a lease up to theirs may still be handed off (§7.4).
The mirror's client (`mirror/<host>`, §5.4, §6.2, step 7b) holds the files of the oldest `seq` a
download still copies, so the queue's deletions keep them.

### 7.3 Running

The `gc` job stops being daily. The lead's agent takes a **chunk** whenever the NAS budget leaves
GC's weight room (`Limits::fits_quiet`, as now) : up to 200 due names, deleted one by one over SMB
(the share deletes no faster; there's no per-file write besides the deletion itself), then one line
appended to the queue's log saying they're gone (`{deleted: [...], at}`, local `fsync`). A chunk
stopped part way (the agent quits, the share stalls) is taken up from its first name not yet marked
gone; deleting a name already gone is a success. So GC runs whenever the share is quiet, a minute
here and there, and never has a "run" to finish: the queue's length and the oldest due name's age
are what the status shows ("GC: 41,200 files due, 312 GB; oldest freed 3 days ago").

Built (step 8b, `agent::gc`, the queue's side `store::map::gc`). Where it departs from the above:

- **A job, not the agent itself:** the `gc` job (the lead's own, `LEAD_ONLY`) is planned whenever
  the queue has a chunk for it or a sweep's pass is due (`Planner::gc_work`), no longer daily, and
  granted by the NAS budget as a quiet step as before; so its NAS work is measured and weighed as
  any job's, and it yields to a job held back for it (`nas_yield`) at its next safe point, before
  each removal. It asks the lead's coordinator: `POST /gc/chunk` (up to `gc::CHUNK_NAMES`, 200),
  `/gc/gone`, `/gc/orphans`, `/gc/removing` (a sweep's names), each answered from the queue in
  memory and its file on the lead's disk.
- **Marked before removed, re-checked as told:** a chunk's names are checked against the holders as
  the lead answers (the map, the records, a hand-off being merged, or queued and not outlasted
  the grace, a cursor or a live lease) and marked deleted with the newest lease then (`Removing`
  lines), before the answer; removed, they're said gone (`Gone`: off the queue, the mark's lease
  renewed to the newest then, so a lease granted while they went is refused too). A tell or a gone
  that fails stops the run (the job fails).
- **Killed mid-chunk:** the names told and not said gone stay marked, being removed; the next
  chunk is them first, told again (those held now kept: off the chunk, but still marked, since the
  killed job may have removed them; the rest marked again), then due names. A file not there
  counts as gone while the share answers (its `map/` folder there: a lost mount finds no file, and
  the run fails instead), so nothing is deleted twice and nothing kept for ever; the first name a
  resumed chunk deletes is the first not gone. **Yielding mid-chunk:** the job says what went and
  lets the rest go (`Released`: queued as they were, unmarked), so a hand-off bringing one
  meanwhile isn't refused; the next chunk starts with them. A name told more than once is never
  let go unmarked (the job before may have removed it): it's kept, marked, still queued.
- **What a chunk takes** (`gc::by_the_queue`): a name in a folder the map indexes, a removed step's
  (`RETIRED`), or a source of a pass older than the newest complete one; never the rest of
  `sources/`, and not the gate's copies, whose holders (the accepted indexes) the queue doesn't
  see: the inputs sweep's (§7.6).
- **The status:** the job's card says how many names it has ("Removing 24,999 files nothing holds
  (GC's queue)"); the queue knows no sizes, so no bytes, and no status line of its own yet.
- **The chunk of 200** is the design's; what bounds it: its names are refused to every hand-off
  while they're being removed, some 24 s of the share's deletions at the median of its probes
  (8.4 a second, `state/pool/nas-probes.json`, 6 and 7 Nov), 80 s at the tenth percentile (2.5);
  its two asks of the lead and one `fsync` there cost 12.5 ms (measured on the M4, 100 chunks
  through a lead's coordinator over HTTP, deletions free), noise beside 200 SMB calls. A yield
  doesn't wait for a chunk's end.
- **Old servers:** the gate of §6.2 (every Mac's server a release past 7b) is met: every member's
  heartbeat on 7 Nov names app 20261107-1045. The code doesn't check it again: a server rolled back
  past 7b would copy without a cursor.

### 7.4 Reuse without a touch

A job that finds its file already on the NAS (`Out::put_file`) no longer touches it. Instead the
merge checks each content name a hand-off brings against the queue's deleted names since the lease
was granted: one deleted meanwhile refuses the hand-off's targets, which are leased again (a redo,
which uploads the file afresh). With §7.2's third rule this can only happen to a name freed before
the lease, found by the job, and deleted during it: rare, and correct when it happens. The touch
(an SMB write per reused file, in the upload path) and `remove_unless_touched` go.

Built (steps 7 and 8b): "since the lease was granted" by lease ids, no clocks: each deletion keeps
the newest lease granted when it was marked, renewed when it's said gone, and a hand-off of that
lease or an older one bringing the name is refused; a hand-off bringing a name being removed (told,
not yet gone) is refused whatever its lease. **A mark is kept while a lease up to its own may still
be handed off** (`GcQueue::beat`, `granted`, `leases`; crate::coord::held::tell_gc), with no window
of days: each job's lease granted to a member is owed by it (a `Granted` line in the queue's file)
until a beat of its, speaking of its leases and its hand-offs, lists neither that lease held nor a
hand-off of it kept (`Owed`: its held leases, its hand-offs' leases, those the lead still holds
for it); the lead's own Mac's are its jobs' leases while out (its hand-offs merge from memory). The
oldest lease owed, or out now, is the floor: a mark whose lease is older goes; while a member the
lead knows hasn't beaten since the queue began keeping this (an app before it, a Mac away at the
deploy), none goes. The owner's release of a member (`scenic pool release`, gone for good) owes
nothing for it. So the M4 away for weeks keeps every deletion since its oldest lease out: about
70 bytes a name in the queue's file, some 150 in the lead's memory, so 100,000 deletions (four
times the 24,999 of §11.4) cost some 7 MB on the disk and 15 MB in memory; a member that never
comes back keeps them until its owner releases it. An app before this one owes as granted (never
cleared) and every Mac is past it. A reused file's check is pending as an upload's (`Out`), so the
uploads' verification finds one missing too (its job may have found the file before it went). What
a hand-off brings: its changes, its uploads, and the files its job found on the NAS and took as
they were (`Handoff::reused`, which `Out` keeps: the gate's copies, reused without a manifest name,
among them). One the merge takes holds its names from every chunk until the records after it are
seen; those they don't name are queued again from when it was brought. The touch is gone (step 8b):
nothing writes a reused file, and nothing goes by a file's time but the sweeps' age rules.

### 7.5 What the queue can't see: orphans

A file uploaded but never handed off (a job killed after its upload, a refused hand-off, a lead
disk lost with its queue) was never held, so it's never queued. A slow **orphan sweep** finds them:
it lists one folder per step (resuming from the folder it reached, kept in the log store), and
queues a content-named file that nothing holds and that is older than `ORPHAN_AGE` (14 days: no job
runs that long between its upload and its hand-off), with `freed_at` its mtime. It takes the same
quiet room, after the queue's chunks; one pass of the share takes days, and that's fine: orphans
are few. After a lead disk restore (§4.5), every name in the restored queue that was due is taken
as deleted (the merge refuses it, §7.4), and the sweep queues what the lost queue held.

Built (step 8b, `gc::orphan_walk`): the folders are the top folders of what the map and the records
name (the lead's, `GcQueue::tops`), the removed steps' and the retired passes', walked a folder at
a time in order, each folder before what's in it, never through a link; the folder it reached is
kept in `agent/maplog/gc-sweeps.json` after each, so a run stopped goes on after it and lists again
only the folders above it. Where it departs: a name is queued freed **when found**, not at its
mtime: a lease granted before then holds it, and it waits its grace from then. A pass begins
`ORPHAN_AGE` after the last ended, so an orphan goes within twice its age, and the share isn't
listed daily. No lead disk is restored from anything (there's no copy of the queue): a lost disk
starts an empty queue, which the sweep fills. It removes a `.tmp` file 2 days old and a retired
pass's other files as it meets them (§7.6).

### 7.6 What stays of today's GC

The sweeps that aren't about the map keep their rules and move into the same chunked runner, each
its own resumable cursor: stray `.tmp` files (2 days), retired passes' sources, the gate's checked
copies (`sources/inputs/`, by their own holders: the accepted indexes), removed steps' names
(`RETIRED`) and depth packs no target names (`drop_names`, which only drop names from the records:
their files then enter the queue like any other freed name), the pending uploads' verification over
SSH, the history and forecast archives, and the journal's GC until #878 removes the journal. The
tombstones' put-back (`TOMB`) goes with the folder sweep it served.

Built (step 8b, `gc::Sweeps`): three passes, each with its own cursor, after the queue's chunks:
the duties once a day (the names to drop, the archives, the forecast inputs' old days, the old
catalogues once asked, the uploads' check: each resumable by what it saved as it went, so a pass
cut short is taken again whole); the inputs sweep (`gc::inputs_walk`) a pass every 14 days, its
holders read as it starts and again once `KEEP_FRESH_S` (a day, half the grace for which the lead holds a copy a merge took) old, each copy told the lead (`/gc/removing`) before it goes, the lead
answering those it holds now; and the orphan sweep (§7.5), which takes the stray `.tmp` files and a
retired pass's planet as it lists. A removed step's files and a retired pass's content-named ones
are the queue's (`by_the_queue`), the orphan sweep finding those named before the queue was. A
tombstone the GC before this step left (`….gc-tomb`) is never taken for a content-named file and
stays.

## 8. Migration: one piece at a time, the map never down

Each step ships alone and keeps the map served throughout. The catalogues go on being written until
every reader has moved (step 6); a reader moves by a release, and the old readers keep working on
the catalogues in between. A server says what it reads in its `changes` requests and in its
heartbeat (`speaks: ["maplog"]`); the lead drops the catalogues one release after every Mac's
server has spoken the log for a day (the #642 gate, as coordination.md §8).

| # | Step | Deletes | Tests |
| --- | --- | --- | --- |
| 1 (built) | `store::map`: entries ⇄ `Catalogue`, batches, the multiset digest, apply to a copy (in place with an undo while no reader holds the map); the log store's segments, snapshot and replay (`store::map::log`), no routes yet | — | `every_catalogue_round_trips` (each catalogue on a test root, and a recorded real one: entries → `Catalogue` → bytes identical); `each_batch_makes_the_next_catalogue` (for consecutive real catalogues, the batch applied gives the next); `incremental_digest_is_the_whole_digest`; `a_torn_last_line_is_dropped`; `replay_after_snapshot_skips_what_it_holds` |
| 2 (built, routes moved to 3) | **Dual write, shadow:** the `catalog` step writes the catalogue as now and the lead's agent appends its batch after (the step posting its map is step 6's); the hourly NAS snapshot | — | `the_log_s_map_is_each_catalogue` (the recorded chain: the log's map at each `seq` is that catalogue); `a_map_the_lead_s_step_posts_is_appended_to_its_log` (the catalog job's end tended by a lead); `a_batch_of_a_catalogue_is_its_entries_batch` |
| 3 (built: shadow, `map-serve` to serve; deletions in 6) | **Servers follow the log:** `GET /map/changes`, `/map/snapshot` on the coordinator (from step 2); `store::map::Client` (long poll, saved map, snapshot fallback) in the server and the CLI; the catalogue reader stays as the fallback while the lead doesn't answer `changes` (an older lead) | with step 6: `poll_every`, the server's `catalog/` listing and local `catalog/` copies, `/api/catalog/refresh` and `net::catalogue_written` (`state/publish/<host>.jsonl` and `publish_event` with step 7b) | built: `changes_answers_without_the_nas` (#871's slow-NAS fake blocks every call; the routes answer, a long poll at the batch); `an_old_since_resets`; `a_server_follows_publishes_and_serves_byte_identical_tiles` (within a second each, its NAS without catalogues); `a_lead_restart_mid_poll`; `the_nas_snapshot_is_used_while_the_lead_is_down`; `offline_the_map_kept_here_is_served` (step 6) |
| 4 (built) | **The agent and the page on the log:** `served()`, `LastCatalogue`, `regionsview`, `builtview`, `cataloguesview`, `pubtimes`, `namestodo::inputs_key` (by prefix `seq`) | `served`'s per-folder cache and `read_n_from` uses, `cataloguesview`'s catalogue reads and hashed names, `publishing.json`'s `at`/`low_at` | `catalogues_view_from_the_log_matches_from_catalogues` (same entries on a recorded run); `names_todo_reruns_exactly_when_its_prefixes_change`; `published_age_is_the_last_batch_touching_the_tile` |
| 5 (built) | **Downloads by their own lists:** each download keeps its logical → content list (`downloads.json` fmt 5); "out of date" from its list against the map served (§6.2) | the Mac's old whole catalogues kept for downloads (`Mirror::keep_catalogues`, `prune`'s kept set) | `an_update_s_size_from_its_list_equals_the_two_catalogues_diff` (on recorded catalogues); `a_download_behind_the_log_compares_its_list`; `downloads_json_fmt4_migrates` |
| 6 (built) | **Catalogues stop** (after the gate: every server on the log a day): the step only posts; `catalog-held/` → `map-held/` | the catalogue writer and reader (`write`, `write_delta*`, `Delta`, `delta.rs`, `chain`, `whole_due`, `MAX_CHAIN`, `keep_read_in`/`kept_dir`, `latest*`, `read_n*`, `list`, `prune`, `next_n`), `state/pool/delta-catalog` and `Switches::delta`, the `catalog-stats:` sizes' look-ups of the last catalogue | `the_step_publishes_with_no_catalog_folder`; `a_held_batch_is_appended_whole_when_the_hold_lifts`; `a_posted_map_is_appended_once`; `the_maps_keep_their_files_however_old` (GC); test roots (`lakefill catalog-add`, `coastcheck water-packs`, the server's tests, `store::testkit::map_root`) on snapshots |
| 7 (built; its shadow ended by 8b) | **GC's queue, shadow:** the lead queues freed names (batches and records saves), leases and clients as holders; the merge's refusal of names deleted under a lease; a dry-run report beside today's GC, which still deletes | the shadow (8b): `ShadowDue`, `ShadowCheck`, `SHADOW_DUE`, `SHADOW_CHECK`, `MISSED`, `shadow_checked`, `shadow_gc`, `--shadow-due`, `GcQueue::map_names`, `freed`, `deleted` | `a_name_freed_and_taken_again_leaves_the_queue`; `due_waits_for_every_cursor_the_grace_and_older_leases`; `a_reuse_of_a_deleted_name_is_refused_and_redone` |
| 7b (built) | **The mirror is a client** (§6.2): the server's mirror sync asks `/map/changes` as `mirror/<host>` each round with the oldest `seq` whose files it still copies (its cursor, made from the downloads' pins and the files here), the map its round copies and the last it got through at (`&copying=`, `&synced=`); the lead keeps them on the cursor and notes them in the history (`mirror-start`, `mirror-end`) for `pubtimes`, as it does the adoptions; a reset moves the Updates of older maps on to the one served | `data::publish_event`, the mirror's `MIRROR_MARKS`, `pubtimes::SERVERS`, `SERVERS_MAX`, `append_server`, `read_servers`, `ServerEvent`, `MirrorMarks`, `state/publish/` | `a_mirror_copying_an_older_seq_holds_its_cursor` (an Update of an older map holds it while the map served moves on, through rounds cut short and a restart; caught up once its files are here); `a_cursor_older_than_the_log_resyncs_the_updates`; `a_client_is_answered_what_follows_its_seq_and_its_cursor_noted` (a mirror the lead didn't hold is reset); `mirror_times_come_from_the_history` |
| 8a (built) | **The map's roots remove** (§11.4): every name the `gc` job removes is told the lead's coordinator first (`gc::Tell`, `POST /gc/removing`, `gc::TheLead`), 200 at a time: one something holds now is answered and stays, the rest marked deleted in its queue with the newest lease then, so the merge refuses a hand-off reusing any of them; `REPORT_ONLY` and its assert are gone. The date gate (15 Nov 07:12, the last catalogue plus 14 days) lifted by the owner, 7 Nov: no old browser tab or long-offline Mac holds an old catalogue | `REPORT_ONLY`, `REPORTS_ONLY`, the assert, `report_only*` in the report, the check's `report-only` reason, the job's marking of what it removed at the plan after it (`shadow_checked` marks only what it found gone) | `every_removal_is_marked_deleted_before_it_goes` (by the queue's chunks since 8b); `a_hand_off_reusing_a_name_gc_removes_is_refused` (by HTTP to a lead's coordinator); `the_lead_holds_back_what_is_held_as_gc_tells_it` |
| 8b (built) | **GC runs by the queue** (§7.3): the `gc` job planned whenever the queue has a chunk or a sweep's pass is due, chunks of 200 (`POST /gc/chunk`, `/gc/gone`) marked deleted before they go, resumed after a kill (told again) or a yield (the rest let go); the orphan sweep (§7.5); the other sweeps as passes with their own cursors (`gc::Sweeps`, §7.6); no touch: hand-offs carry what they reused (`Handoff::reused`), the merge refuses a name being removed and holds what it takes (§7.4) | `run_once`'s map reading and folder sweep (the phases "reading the catalogs", "sweeping folders", "sweeping retired passes", "sweeping removed steps' files"), `sweep_retired`, `sweep_retired_names`, `Removals`, `Due`, `Kind`, `Shadow`, `remove_unless_touched`, `still_old`, `remove`, `TOMB`, `put_back`, `restore`, `Tell` and its `Fn` impl, `TELL_CHUNK`, the 14-day window (`keep_days`, `--days`), the `gc` job's daily schedule, the report's `catalogs_kept`, `referenced`, `files_seen`, `young`, `held`, `removed_bytes`, `retired_bytes`, `tombs`, `restored`, `shadow`; `Out`'s touches (`put_file`'s, `upload_hashed`'s, `store_bytes`'s, with their uploads again); GC's queue's shadow (step 7's row) | `gc_resumes_mid_chunk_after_a_kill` (killed between two deletions mid-chunk: the next run's first deletion is the first file not gone, nothing deleted twice, the queue empty after); `gc_yields_mid_chunk_and_resumes_where_it_left_off`; `an_orphan_is_queued_only_after_its_age`; `the_queue_rebuilds_after_a_lead_disk_restore`; `the_orphan_sweep_goes_on_from_the_folder_it_reached`; `a_chunk_told_again_keeps_what_is_held_now` (and a yield on it keeps the retold names marked); `a_rewrite_keeps_every_mark`, `deletions_go_once_no_older_lease_is_owed` (store); `a_member_away_ten_days_reusing_a_removed_name_is_refused_and_redone`; the lost share in `gc_resumes_mid_chunk_after_a_kill`; `a_reuse_of_a_name_being_removed_is_refused_and_a_merge_holds_its_names`; `a_file_reused_is_handed_off_as_reused_untouched` (buildstore); the budget's yield: `the_clean_up_waits_for_a_quiet_nas_and_yields_to_a_held_job` (coord) |
| 9 (built; runs once every member is on it) | **The old catalogues go:** the lead's daily `gc` job, asked `--old-catalogues` once every member's heartbeat says its app's NAS check looks at `map/` (`Beat::checks_map`, `pool::Side::all_check_map`), removes `catalog/`, `catalog-held/` and the switches `state/pool/delta-catalog` and `state/pool/map-serve` on the NAS (`gc::remove_old_catalogues`: one `rm -rf` over SSH from the lead, else over SMB file by file, a safe point between folders); each Mac's agent removes its `catalog/` (the map server's, in the app's folder) and `~/Library/Caches/scenic/catalogue/` at its start (`gc::remove_this_macs_catalogues`: every Mac runs an agent, the map server runs on the mini alone and a laptop's on demand). Nothing reads a catalogue since step 6 and GC roots on none, so a file only an old catalogue named is GC's as any file nothing names (on 7 Nov, 5,827 of catalogue 1063's 21,233 files weren't in map 1689). The gate: the agent's NAS check looked at `catalog/` until this step (`agent::answers`, now `agent::NAS_CHECK`, `map/`), and an older app sees the NAS away without it. A member away on an older app holds the pass: its heartbeat on the NAS stays and says no `checks_map`; only a heartbeat file gone and unheard `FORGET_AFTER` (20 min) stops counting. A rollback past the pass: that app's agent sees the NAS away and its build stops (the map stays served); `mkdir catalog` in the project folder brings it back (it only stats the folder), and the pass doesn't run again while its heartbeat lacks `checks_map` | the folders and switches | `the_old_catalogues_folders_go_whole`; `the_old_catalogues_wait_for_every_member_s_map_check`; `the_old_catalogues_go_only_once_every_app_checks_by_the_map` (on a private root, after the pass an agent checking by `catalog/` sees the NAS away, this app's sees it); `this_macs_old_catalogues_go` |
| 10 | docs: formats.md (Catalogue → the map and its snapshot), storage.md (GC), macs.md (the server's sync, the mirror), building.md (publishing, the switches), pool.md §7.2/§10; this doc folded in | — | — |

Order: 1, 2, 3 in turn; 4 and 5 after 3, in either order; 6 after 3, 4 and 5 and the gate; 7 after
2 (it needs the batches' `freed`); 8a after 6 and 7 (§11.4);
7b after 3, any time; 8b after 8a and 7b (on every Mac: an older server's mirror says no cursor); 9 once every member's app checks the NAS by `map/`;
10 with each. Steps 3 and 8b are what the owner sees: servers following each publish within a
second, from home or away, and GC that keeps up; 8b frees the space held today (§11.4).

## 9. What gets deleted

The code of every item below went with step 6 (built), GC's with steps 8a and 8b (built), and the server's `publish_event`
with step 7b (built); the folders and files on the NAS
and the Macs go with step 9, GC's with steps 8a and 8b.

- **On the NAS:** `catalog/`, `catalog-held/`, `state/pool/delta-catalog`, `state/pool/map-serve`, `state/publish/`.
- **On each Mac:** `catalog/` in the server's home, `~/Library/Caches/scenic/catalogue/` (the whole
  catalogues kept for downloads went with step 5). (The map-serve switch's copy, `serve` in
  `~/Library/Caches/scenic/map/`, a server removes at its start since step 6.)
- **In `store::catalogue`:** everything but the `Catalogue` type and its parts (`FileRef`, `Layer`,
  `references`, `validate`, the coverage types): `NAME`/`HELD_NAME` as folders, `file_name`,
  `delta_file_name`, `write`, `write_copy`, `write_delta`, `write_delta_of`, `read`, `read_n`,
  `read_n_from`, `path_n`, `list`, `latest`, `latest_on`, `latest_nas`, `latest_nas_from`,
  `keep_read_in`, `kept_dir`, `whole_due`, `chain`, `next_n`, `prune`, `digest`, `Delta` and
  `catalogue/delta.rs`, `MAX_CHAIN`, `first_wrong`.
- **In the server:** `refresh_catalogue`'s NAS read, `poll_every`, `refresh_h`
  (`/api/catalog/refresh`), `publish_event`, the start's catalogue search.
- **In the agent:** `served`'s cache, `publish::Switches::delta`, `LastCatalogue`'s `at`/`low_at`
  (the log has them), `net::catalogue_written`, `cataloguesview`'s catalogue reads and name hashes.
- **In GC:** `run_once`'s "reading the catalogs", "sweeping folders" and "removing old catalogs",
  the 14-day window, `remove_unless_touched`, `TOMB`/`put_back`, the daily schedule; `Out::put_file`'s
  touch.
- **In docs:** formats.md's Catalogue and Delta catalogues sections (replaced), storage.md's GC
  roots, macs.md's catalogue polling.

## 10. Open decisions

1. **Its own store or #878's:** the map log as its own store on the lead (proposed: lands without
   waiting on #878; one writer thread of its own), folded into the pool store once #878's step 1 has
   landed, or built inside it from the start (one snapshot, one backup, but waits for #878).
2. **`GRACE`:** 2 days proposed. It's the only time a bad publish's previous files survive the
   cursors passing them. Longer costs room (about a day's churn of packs per day of grace).
3. **`CURSOR_TTL`:** 7 days proposed for a client that stops asking. Shorter frees sooner; a Mac
   away longer resets from the snapshot either way and loses nothing (its mirror is its own).
4. **The orphan sweep over SSH:** listing on the NAS itself (`find -printf`, one call a folder,
   seconds for the whole share) instead of SMB listings, if the lead (the Mac mini) can use SSH
   unattended; today's GC verifies uploads over SSH only where it can. Proposed: SMB, chunked, which
   needs nothing new; SSH as a later speed-up.
5. **Rolling back a publish:** the log makes "map as of `seq` s" cheap to compute while its files
   are within `GRACE`. Proposed: not built now (no one asks for it); the grace keeps the option.
6. **Clients' cursors durable:** proposed not `fsync`ed per request (saved with the log's snapshot):
   a lead restart that loses an hour of cursor advances only delays GC by that hour.

## 11. Where it stands (6 Nov 2026), and what's left

**Status: steps 7b (6 Nov), 8a, 9 and 8b (7 Nov) built; 10 planned.** The rest of this doc is the design; this section is
the walk of every reader and writer as the code is today (references at commit 36bb70a2), and the
steps left.

### 11.1 Every catalogue reader and writer, today

| Who | Where | What it reads now | Left |
| --- | --- | --- | --- |
| The map server's sync | `crates/server/src/maplog.rs:73` (`spawn`), `:323` (the long poll), `:232` (`Map::catalogue` each batch); its start `crates/server/src/data.rs:128` (`store::map::newest_nas`) | the lead's `/map/changes`, its saved map, the NAS's snapshot | nothing |
| The mirror and offline downloads | `crates/store/src/mirror.rs:643` (`Mirror::sync`, given the catalogue made of the map), `crates/server/src/downloads.rs` (`keep_lists`; `once`, its cursor told each round: `Plan::copies_from`, `Downloads::resync`), `crates/server/src/maplog.rs` (`MirrorLink`) | each download's own list against the map served; its cursor `mirror/<host>` | nothing (7b built) |
| The map app | `web/src/catalogue.ts:107` (`/api/catalog` each minute, its own Mac's server) | the server's map as a catalogue status | nothing |
| Consistency of tile sets | the gate picks a batch's contents; `crates/pipeline/src/bin/scenic-build.rs:500` (the `catalog` step posts its map); `crates/pipeline/src/agent/publish.rs:67` (`MapLog`, one batch appended whole); clients swap a batch in whole | the batch | nothing |
| Publish history and ages | `crates/pipeline/src/agent/cataloguesview.rs:322` (`CataloguesView`), built only on the lead (`crates/pipeline/src/agent/mod.rs:7728`, `tend_pageview`); adoptions and the mirrors' rounds noted there (`publish_marks`); `crates/pipeline/src/pubtimes.rs:414` (the history's `catalog` events), the mirrors' `mirror-start`/`mirror-end` events | the lead's feed in process | nothing |
| `/work/catalogs` | `crates/pipeline/src/coord/mod.rs:4900`; a member's port sends it to the lead (`crates/pipeline/src/coord/memberpage.rs:56`) | the view above | nothing |
| `scenic catalogs` | `crates/pipeline/src/cli/scenic.rs:274` | the view this Mac's agent kept (`cataloguesview::KEPT`) | nothing |
| publish.rs and coordwire | `crates/coordwire/src/lib.rs:39` (`map_now`: `/map/snapshot`, else the NAS's newest), `publish.rs:67` (`MapLog`), `:95` (`Here`: the head's catalogue, made of the map while read) | the lead's log | nothing |
| gc.rs and the queue | `crates/pipeline/src/agent/gc.rs:841` (`run`), `:903` (`drain`: the queue's chunks), `:935` (`remove_told`), `:1052` (`orphan_walk`), `:1156` (`inputs_walk`), `:50` (`Lead`), `:219` (`by_the_queue`); `crates/pipeline/src/coord/mod.rs:4527` (`/gc/chunk`, `/gc/gone`, `/gc/orphans`, `/gc/removing`); `crates/pipeline/src/agent/gcqueue.rs:42` (`refusal`); `crates/pipeline/src/agent/plan.rs:1132` (`gc_work`); `crates/store/src/map/gc.rs:452` (`GcQueue::chunk`), `:539` (`bringing`), `:27` (`GRACE_S`) | GC's queue on the lead (the map's names, the records', what a hand-off brings) | nothing |
| The plan, names-todo, regions | `crates/pipeline/src/agent/plan.rs:2214` (`map_now`), `crates/landmarks/src/namestodo.rs:405` (`inputs_key`, keyed by the log's marks), `regionsview`, `builtview` | the head's catalogue (`Here`) | nothing |
| The menu app | `tools/status/main.swift:445`-`467` | the status's regions' `map_at` alone | nothing: it never read catalogues |
| Test roots | `lakefill catalog-add` (`crates/pipeline/src/cli/progs.rs:354`), `coastcheck`, `store::testkit::map_root` | they write a snapshot | nothing |
| The NAS's old folders | `catalog/` (1,041 files, 479 MB, the newest `1066.delta.json.zst` of 1 Nov 07:11), `catalog-held/` (176 kB) | no catalogue read; `catalog/` looked at by an older app's NAS check (this one's: `map/`) | 9 (built): removed by the `gc` job once every member's app checks by `map/` |

Cut with the next step that touches it: `cataloguesview::Source` (`:288`) has one implementation,
the feed (`:295`; the tests' is the feed too): the trait can go.

### 11.2 The design in brief

One current map (key → value, flat: §4.1), kept by the lead's agent in memory and on its own disk
(segments of `fsync`ed lines, a snapshot); every publish is one batch (`seq`, `set`, `del`, `freed`,
the map's digest after it: §4.2, §4.3). Clients ask `GET /map/changes?since=<seq>&client=<id>`
(long poll) and apply each answer whole to a copy, swapped in once (§5.1); files are content-named,
so a batch moves no data. Each request is the client's cursor (§5.4). GC queues a name when a batch
or a records save stops naming it, and frees it once no holder names it, every cursor is past its
`freed_seq`, `GRACE` has passed and every live lease is newer (§7.2), in chunks of 200 it resumes
from the queue's own log (§7.3). Publish history is the log's batches (§6.6).

**On the NAS.** Nothing in a hot path writes the share per publish or per file: a batch is a line on
the lead's own disk; the share gets one whole snapshot an hour (34 MB, temporary name and rename,
the newest three kept: three deletions an hour) and no listing by any reader. GC's deletions are one SMB call per file, which the
share can't batch; 8b drops the folder sweep (listing and stat of every file each day) and the touch
in the upload path (`crates/buildstore/src/out.rs:412`), the one per-file write left there for GC.

**How the lead serves it.** The lead's coordinator (port 8090, `crates/pipeline/src/coord/maproutes.rs:40`)
answers `/map/changes` and `/map/snapshot` from the feed in memory (§3, coordination.md §2 rule 3:
never the NAS), with the build's key; a long poll looks each 50 ms for a batch. With the lead away,
clients keep their map, and one with none reads the NAS's snapshot (§5.2).

### 11.3 What's left, each step alone

1. **8a, the map's roots remove** (built): each removal told the lead first.
2. **8b, GC by the queue** (built): chunks, the orphan sweep, the other sweeps as passes with
   cursors; the daily job, the folder sweep, the 14-day window and the touch gone. It frees the
   ~890 GB nothing names (§11.4).
3. **9, the old catalogues go** (built): `catalog/` and `catalog-held/` on the NAS (479 MB) by the
   `gc` job once every member's app checks the NAS by `map/`, not `catalog/`; each Mac's copies at
   its agent's start.
4. **10, docs**: formats.md, storage.md, macs.md, building.md, pool.md, and this doc folded in.

**With #916 step 6d** (docs/crates.md §6.3). #883's steps left touch one place in cli: `scenic
gc`'s flags (`crates/pipeline/src/cli/scenic.rs:198`; 8b, built: `--shadow-due` and `--days` gone,
`--sweeps` the passes' file). Nothing left edits `cli/mod.rs`, `cli/build.rs`,
`cli/progs.rs` or `net.rs` (`net.rs` has no catalogue code since step 6). So 6d moved
`cli::program` (with `TOOLS`) to jobrt::sys without waiting for #883.

**With #882** (each Mac a role; the mini serves). The log, its feed, GC's queue and the Catalogues
map are the lead's already, so with the lead fixed on the mini they are where #882 puts them, and
#882 moves none of them. What #882 leaves on the laptops is what is a client here: their own map
server for the offline map (the owner's decision of 1 Nov, 2) and its mirror (7b), each a cursor
that `CURSOR_TTL` drops while a laptop is away, so a laptop never holds GC up for long. #874
retired the laptops' servers, a regression #882 step 7a mended: a laptop's server runs on demand,
started by its menu bar, and its cursors exist only while it runs (docs/roles.md §8.3). A lead moved
to another Mac starts its log from the NAS's snapshot and loses its queue (§4.5; the orphan sweep
finds what it held, within twice `ORPHAN_AGE`); #882's fixed lead makes that rare, which is what
8b relies on.

### 11.4 What frees the 890 GB

About 890 GB of the NAS is content-named files nothing names, old enough to go: a dry run of the
8a code on the live share (7 Nov, the lead's due set of that hour, 14 min on the M1) counted 24,999
files, 889,986,014,777 bytes, 1,077 of them the gate's copies, none named by the map, its log, GC's
queue, the records, the lead's store or any index of the window. 8a's first real run never finished:
each try listed the share again from its start before it removed (the last read 114 GB in 1,197 s,
then yielded to a held job). **Step 8b frees them by the queue** (§7.3): they're its due names, taken
200 at a time from the first run on, each removal kept whatever stops the run, nothing listed first.
The gate's 1,077 are the inputs sweep's (§7.6), by the indexes that hold them.

- **Its pace.** One removal is one SMB call, no listing, no stat, no rename (8a's was three, with
  the listing of every folder before). On a private root (the M4's disk, through a lead's
  coordinator over HTTP, 20,000 files) the job drains 6,251 files a second; the chunks' own asks cost
  12.5 ms each, so on the NAS the job deletes as fast as one stream of SMB calls does: at the
  probes' rates (`state/pool/nas-probes.json`, 64 probes, 6 and 7 Nov under the build's load: 8.4 a
  second at the median, 2.5 at the tenth percentile, 3.7 at the last) the 24,999 take some 50 minutes
  of the job's time at the median, 1.9 hours at the last probe's rate, 2.8 hours at the tenth
  percentile.
- **During the rebuild.** The job runs only in the NAS budget's quiet room (beside jobs that leave
  its measured weight room, or held back `QUIET_AGE`, 24 hours, as an aged job of the lead's), and
  yields to a job held back for it `OWN_AGE` (10 min) or more, at its next removal. Each grant now
  removes from its first second: at 8a's last grant's length (some 20 minutes) that's about 10,000
  files at the median rate, 3,000 at the tenth percentile, so the 24,999 go in some three to eight
  grants: within a day where the build leaves quiet gaps, and within some three to eight days if it
  never does and each grant waits its full `QUIET_AGE`. Not measured on the live share (the owner's
  to run).
- **The holders as each chunk is told.** The lead answers against its holders then: the map or the
  records name it, a hand-off being merged brings it, or it's queued and hasn't outlasted `GRACE_S`,
  a client's cursor heard within `CURSOR_TTL_S` or a live lease. With the records not seen yet the
  lead answers 503, and the run stops, removing nothing. A mark is kept while a lease as old may
  still be handed off, however long its member is away (§7.4).

## Decisions (PM, 30 Oct night)

1. The map log is its own store on the lead, so it lands without waiting for #878; #878's pool store may absorb it later.
2. `GRACE` is 2 days.
3. `CURSOR_TTL` is 7 days.
4. The orphan sweep runs over SMB in chunks for now.
5. Rolling back a publish is not built; the grace keeps the option open.
6. Cursors are saved with the log's snapshot. A lead restart can only bring a cursor back older, which keeps files longer and never frees one early.

## Decisions (PM, 1 Nov)

1. **Step 6's gate is met.** One map server (the Mac mini's: #874 retired the M1's and the M4's),
   a day on the log with no `maplog: MISMATCH`, its cursor advancing (1019 against the head's 1020).
   The catalogue-first race and a 15 to 20 s fallback on a NAS hiccup (the `map-serve` switch read a
   NAS NotFound as off, `crates/server/src/maplog.rs`) were not the log's fault: both went with the
   switch and the catalogues, in the one release (deleting the switch alone would have turned the map
   off on a hiccup with nothing to fall back on).
2. **The laptops' offline map is kept** (the owner's): it follows the log and its snapshot (the map
   kept in `~/Library/Caches/scenic/map/current.json.zst`, else the NAS's snapshot), never a
   catalogue.
3. **No catalogue kept for downloads.** #883 step 5 kept the last three catalogues on the Mac for a
   download whose list wasn't written; at step 6 no Mac had a download pinned at another `seq` than
   its list's (the mini: no download; the M1 and M4: no downloads file), and nothing writes
   catalogues, so the fallback went: a download without a list reads at the map served.

