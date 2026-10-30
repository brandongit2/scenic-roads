# The map and its log

**Planned (task #883), steps 1 and 2 of §8 built.** This doc says how catalogues and GC give way
to one current map of the served files plus an append-only log of its changes, which clients follow
by cursor, and a GC that runs all the time in small steps it can resume. Until each step of §8
lands, docs/formats.md (Catalogue, Delta catalogues), docs/storage.md (GC), docs/macs.md (the map
server, the mirror) and the code are what is true.

**Built (steps 1 and 2):** `store::map` (entries ⇄ `Catalogue`, batches, the digest, applying to a
copy, the NAS snapshot) and `store::map::log` (the store: `snapshot.json.zst` and segments of
`fsync`ed lines, in the lead agent's `maplog/` folder). The lead writes the log in shadow:
`agent::publish::Shadow`, on a thread of its own, takes each catalogue the `catalog` step published
(as the agent reads it when the job ends) and appends its batch, `seq` its number; the first one
it's told of seeds the log. It checks the map equals that catalogue (digest, then entries; the
whole catalogue given back once a process) and logs `maplog: MISMATCH` otherwise. The lead's
snapshot and `map/<seq>.json.zst` are written when the log starts and then once an hour while
batches come (not yet at a clean stop). Nothing reads the log: the routes (`GET /map/changes`,
`/map/snapshot`) come with step 3, and the step still writes the catalogue itself rather than
posting its map to the agent (§6.7) until step 6.

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

### 4.4 The NAS snapshot

The lead writes the whole map to `map/<seq>.json.zst` (zstd with its checksum, temporary name, then
rename) once an hour while batches come, and at a clean stop: one whole write of about 9 MB an hour
instead of one create a minute plus a whole one each `whole_due`. It keeps the newest three. Who
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
name, rename), written at most every 10 minutes and at a clean stop, not at every batch (9 MB a
minute would be waste). On a start it loads that and asks for the batches since. Its digest is
checked on load; a file that doesn't match is set aside and the snapshot fetched.

### 5.4 Cursors

Every `changes` request says the client's `seq` and id; the lead keeps, per client, the last `seq`
it applied (the `since` of its next request) and when it was heard (in the log store, saved with
its snapshot, not `fsync`ed per request). The clients:

| Client | Id | Holds files on the NAS? |
| --- | --- | --- |
| each Mac's map server | `server/<host>` | yes: it reads the NAS for what its mirror lacks |
| each Mac's mirror sync | `mirror/<host>` | yes, while it copies a `seq`'s files |
| the lead's agent (plan, status, page views) | in process | no: it reads names, not files |
| a job leased with files of the map as inputs (names-todo, regions' checks) | its lease | yes, until it ends (§7.2) |
| the NAS snapshot | `snapshot` | yes: a reader may start from it |

A client unheard for `CURSOR_TTL` (7 days) is dropped from the set: a laptop away for a month
doesn't hold GC. When it comes back its `since` is likely older than the log's oldest batch, and it
resets from the snapshot; its own mirror holds its downloads' files whatever happened on the NAS
(§6.2).

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
  `mirror-start`/`mirror-end` events ride on its own client's requests (`&synced=<seq>`).
- `main.rs`'s start (read the newest local catalogue, else the NAS's) → the saved map, else the
  lead's snapshot, else the NAS snapshot.

### 6.2 The mirror and offline downloads (`store::mirror`, `server/src/downloads.rs`)

- A download is pinned at a `seq` (`downloads.json`'s `cat`, the same number). Today it keeps the
  whole catalogue it's at in `catalog/` "however old" to know its files; instead the download keeps
  its own logical → content list (the keys its region and detail cover), written when it's
  downloaded or updated. The old whole catalogues on the Mac go.
- "Out of date, with what changed and the size to copy": the batches since the download's `seq`
  name exactly the keys that changed; the panel intersects their keys with the download's instead
  of comparing two whole catalogues. A download behind by more than the log holds compares its list
  with the current map (one pass over its own keys).
- `Data::set_older`/`content` (the download's older files read first) are unchanged: they read the
  download's list.
- The mirror's sync is a client (`mirror/<host>`): while it copies files of `seq` s from the NAS
  its cursor stays at s, so GC keeps them until it's done or moves on (today the 14-day window did
  that, by accident of its length).

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
- `build::LastCatalogue` (`publishing.json`: `at`, `low_at`, `held_at`) and the status's
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
  adoption times (§6.1) instead of `state/publish/<host>.jsonl`, which goes.

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

### 7.3 Running

The `gc` job stops being daily. The lead's agent takes a **chunk** whenever the NAS budget leaves
GC's weight room (`Limits::fits_quiet`, as now) : up to 200 due names, deleted one by one over SMB
(the share deletes no faster; there's no per-file write besides the deletion itself), then one line
appended to the queue's log saying they're gone (`{deleted: [...], at}`, local `fsync`). A chunk
stopped part way (the agent quits, the share stalls) is taken up from its first name not yet marked
gone; deleting a name already gone is a success. So GC runs whenever the share is quiet, a minute
here and there, and never has a "run" to finish: the queue's length and the oldest due name's age
are what the status shows ("GC: 41,200 files due, 312 GB; oldest freed 3 days ago").

### 7.4 Reuse without a touch

A job that finds its file already on the NAS (`Out::put_file`) no longer touches it. Instead the
merge checks each content name a hand-off brings against the queue's deleted names since the lease
was granted: one deleted meanwhile refuses the hand-off's targets, which are leased again (a redo,
which uploads the file afresh). With §7.2's third rule this can only happen to a name freed before
the lease, found by the job, and deleted during it: rare, and correct when it happens. The touch
(an SMB write per reused file, in the upload path) and `remove_unless_touched` go.

### 7.5 What the queue can't see: orphans

A file uploaded but never handed off (a job killed after its upload, a refused hand-off, a lead
disk lost with its queue) was never held, so it's never queued. A slow **orphan sweep** finds them:
it lists one folder per step (resuming from the folder it reached, kept in the log store), and
queues a content-named file that nothing holds and that is older than `ORPHAN_AGE` (14 days: no job
runs that long between its upload and its hand-off), with `freed_at` its mtime. It takes the same
quiet room, after the queue's chunks; one pass of the share takes days, and that's fine: orphans
are few. After a lead disk restore (§4.5), every name in the restored queue that was due is taken
as deleted (the merge refuses it, §7.4), and the sweep queues what the lost queue held.

### 7.6 What stays of today's GC

The sweeps that aren't about the map keep their rules and move into the same chunked runner, each
its own resumable cursor: stray `.tmp` files (2 days), retired passes' sources, the gate's checked
copies (`sources/inputs/`, by their own holders: the accepted indexes), removed steps' names
(`RETIRED`) and depth packs no target names (`drop_names`, which only drop names from the records:
their files then enter the queue like any other freed name), the pending uploads' verification over
SSH, the history and forecast archives, and the journal's GC until #878 removes the journal. The
tombstones' put-back (`TOMB`) goes with the folder sweep it served.

## 8. Migration: one piece at a time, the map never down

Each step ships alone and keeps the map served throughout. The catalogues go on being written until
every reader has moved (step 6); a reader moves by a release, and the old readers keep working on
the catalogues in between. A server says what it reads in its `changes` requests and in its
heartbeat (`speaks: ["maplog"]`); the lead drops the catalogues one release after every Mac's
server has spoken the log for a day (the #642 gate, as coordination.md §8).

| # | Step | Deletes | Tests |
| --- | --- | --- | --- |
| 1 (built) | `store::map`: entries ⇄ `Catalogue`, batches, the multiset digest, apply to a copy; the log store's segments, snapshot and replay (`agent::maplog`), no routes yet | — | `every_catalogue_round_trips` (each catalogue on a test root, and a recorded real one: entries → `Catalogue` → bytes identical); `batch_between_equals_the_delta` (for consecutive real catalogues, the batch applied gives the delta's result); `incremental_digest_is_the_whole_digest`; `a_torn_last_line_is_dropped`; `replay_after_snapshot_skips_what_it_holds` |
| 2 (built, routes moved to 3) | **Dual write, shadow:** the `catalog` step posts its map to the agent, which appends a batch, then writes the catalogue as now; `GET /map/changes`, `/map/snapshot` on the coordinator; the hourly NAS snapshot | — | `the_log_s_map_is_each_catalogue` (every catalogue the step writes, the log's map at that `seq` has its digest; in the agent's two-agents harness); `changes_answers_without_the_nas` (#871's slow-NAS fake blocks every call; the route answers); `an_old_since_resets` |
| 3 | **Servers follow the log:** `GET /map/changes`, `/map/snapshot` on the coordinator (from step 2); `store::map::Client` (long poll, saved map, snapshot fallback) in the server and the CLI; the catalogue reader stays as the fallback while the lead doesn't answer `changes` (an older lead) | `poll_every`, the server's `catalog/` listing and local `catalog/` copies, `/api/catalog/refresh` and `net::catalogue_written`, `state/publish/<host>.jsonl` and `publish_event` | `a_server_follows_a_publish_within_a_second`; `a_server_away_follows_over_the_lead_alone` (NAS absent, lead reachable); `a_batch_is_applied_whole` (a request during apply sees one `seq`); `a_wrong_digest_fetches_the_snapshot`; `a_fresh_server_starts_from_the_nas_snapshot_with_the_lead_down` |
| 4 | **The agent and the page on the log:** `served()`, `LastCatalogue`, `regionsview`, `builtview`, `cataloguesview`, `pubtimes`, `namestodo::inputs_key` (by prefix `seq`) | `served`'s per-folder cache and `read_n_from` uses, `cataloguesview`'s catalogue reads and hashed names, `publishing.json`'s `at`/`low_at` | `catalogues_view_from_the_log_matches_from_catalogues` (same entries on a recorded run); `names_todo_reruns_exactly_when_its_prefixes_change`; `published_age_is_the_last_batch_touching_the_tile` |
| 5 | **Downloads by their own lists:** each download keeps its logical → content list; "out of date" from the batches since its `seq` | the Mac's old whole catalogues kept for downloads | `an_update_s_size_from_batches_equals_the_two_catalogues_diff` (on recorded catalogues); `a_download_behind_the_log_compares_its_list`; `downloads_json_fmt4_migrates` |
| 6 | **Catalogues stop** (after the gate: every server on the log a day): the step only posts; `catalog-held/` → `map-held/` | the catalogue writer and reader (`write`, `write_delta*`, `Delta`, `delta.rs`, `chain`, `whole_due`, `MAX_CHAIN`, `keep_read_in`/`kept_dir`, `latest*`, `read_n*`, `list`, `prune`, `next_n`), `state/pool/delta-catalog` and `Switches::delta`, the `catalog-stats:` sizes' look-ups of the last catalogue | `the_step_publishes_with_no_catalog_folder`; `a_held_batch_is_appended_whole_when_the_hold_lifts`; test roots (`lakefill catalog-add`, `coastcheck`, the server's tests) on snapshots |
| 7 | **GC's queue, shadow:** the lead queues freed names (batches and records saves), leases and clients as holders; the merge's refusal of names deleted under a lease; a dry-run report beside today's GC, which still deletes | — | `a_name_freed_and_taken_again_leaves_the_queue`; `due_waits_for_every_cursor_the_grace_and_older_leases`; `a_reuse_of_a_deleted_name_is_refused_and_redone`; the shadow's due set ⊆ today's GC's deletions on the same day (a check job) |
| 8 | **GC runs by the queue:** chunks of 200 whenever quiet, resumable; the orphan sweep; today's other sweeps as chunked cursors | `run_once`'s catalogue reading and folder sweep, `remove_unless_touched` and `Out::put_file`'s touch, `TOMB` and `put_back`, the 14-day catalogue window, the `gc` job's daily schedule | `gc_resumes_mid_chunk_after_a_kill` (agent killed between two deletions; next chunk starts at the first not gone, nothing deleted twice or kept forever); `gc_yields_to_the_budget`; `an_orphan_is_queued_only_after_its_age`; `the_queue_rebuilds_after_a_lead_disk_restore` |
| 9 | **The old catalogues go:** one GC pass removes `catalog/` and `catalog-held/` on the NAS (their files are held by the map and records already), and each Mac's server its `catalog/` | the folders | — |
| 10 | docs: formats.md (Catalogue → the map and its snapshot), storage.md (GC), macs.md (the server's sync, the mirror), building.md (publishing, the switches), pool.md §7.2/§10; this doc folded in | — | — |

Order: 1, 2, 3 in turn; 4 and 5 after 3, in either order; 6 after 3, 4 and 5 and the gate; 7 after
2 (it needs the batches' `freed`); 8 after 6 and 7 (once no server reads catalogues, no catalogue
window needs keeping); 9 after 8; 10 with each. Steps 3 and 8 are what the owner sees: servers
following each publish within a second, from home or away, and GC that keeps up.

## 9. What gets deleted

- **On the NAS:** `catalog/`, `catalog-held/`, `state/pool/delta-catalog`, `state/publish/`.
- **On each Mac:** `catalog/` in the server's home, `~/Library/Caches/scenic/catalogue/`, the whole
  catalogues kept for downloads.
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

## Decisions (PM, 30 Oct night)

1. The map log is its own store on the lead, so it lands without waiting for #878; #878's pool store may absorb it later.
2. `GRACE` is 2 days.
3. `CURSOR_TTL` is 7 days.
4. The orphan sweep runs over SMB in chunks for now.
5. Rolling back a publish is not built; the grace keeps the option open.
6. Cursors are saved with the log's snapshot. A lead restart can only bring a cursor back older, which keeps files longer and never frees one early.
