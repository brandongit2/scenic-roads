# Dev tools

The Scenic Roads dev tools app: one Node process serving the task board and the dev job dashboard,
each with its data in its own SQLite file. Its index (`/`) is a link to each.

```
node server.mjs [--port 8077] [--host 127.0.0.1] [--data <folder>]   # data: taskboard.db, jobs.db (default here)
node --test test/*.test.mjs        # the APIs' tests (npm test)
```

Open http://127.0.0.1:8077. Needs Node 22.5+ (`node:sqlite`); no packages.

- `server.mjs` is the app: its pages (`public/`), the event stream open pages hear of changes on
  (`/api/stream`, which collection changed), and its parts' APIs. `http.mjs` is what the parts
  share to answer.
- **The task board** (`/task-board/`, an item's own address `/task-board/709`; `/709`, its address
  before the dev tools app, goes there, and `/task-board/api/…` and `/task-board/_blob/…`, what its
  page asked behind Caddy's old path, are `/api/…` and `/_blob/…`): `board.mjs` its API
  (`/api/c/…`, photos at `/_blob/<id>`), `schema.mjs` the shape of its data, enforced on every write
  (and imported by the page, which shows what doesn't fit), `priority.mjs` where a priority with a
  fraction goes (below; the server's rule and the page's, one copy), `store.mjs` its file, `taskboard.db`:
  `items` (folders and tasks), `events` (history), `meta` (`now`, the now line: what Claude is on,
  shown in the page's header; `counter`, the next item number), `blobs` (photos); its form
  (`PRAGMA user_version`) is 2 (the indexes every frequent read is answered from, `store.mjs`
  INDEXES); any other form is refused. `public/task-board/index.html` is the page;
  `public/task-board/shim.js` gives it its database API (`window.claude.use("db" | "user" |
  "assets")`) over the app's: a subscription reads once the event stream is open and again on each
  change of its collection (the items: whole once, then only what changed since, `?since=`, below),
  ends when a read fails (the page subscribes again), and the stream is opened anew when it is
  refused (a proxy's 502 while the server restarts) and everything read again.
- **The dev job dashboard** (`/jobs/`, below): `jobs.mjs` its API (`/api/jobs/…`) and file,
  `jobs.db`; `public/jobs/` the page.
- The pages' addresses are relative (`../api/…`), so the app works under a proxy's path.
- The data is not in git: it lives on the M6 (its data folder, `--data`), with no copy elsewhere
  (an update's copy before it switches versions stays on the M6, below).
- **A change of a database's form sets its `PRAGMA user_version`** (or changes its schema, which
  SQLite's `schema_version` counts): the M6's update puts a database back as it was when a version
  that changed its form doesn't start (tools/app/services.zsh).
- **On the M6** it ships with the app and its agent installs each version (docs/macs.md, Services:
  `services.zsh setup devtools`, once, writes the LaunchAgent `local.scenic.devtools`, `--host ::`,
  IPv4 and IPv6, log `~/Library/Logs/scenic/devtools.log`). The other Macs use it at
  http://mac-mini-de-brandon:8077, and any device on the tailnet at https://map.brandontsang.net/dev/
  (Caddy, `tools/caddy`).
- No sign-in, by the owner's rule: anyone on the home network or tailnet may use it. Elsewhere `--host` stays 127.0.0.1.

## The dev job dashboard (#946)

Every run of the long dev jobs, since the dashboard was merged: `tools/dev/check.sh` and each of its
parts, `tools/app/publish.sh` and each of its steps, every test run through
`tools/app/remote-tests.sh` (check.sh's, publish's suite, agents' and reviewers' own) with each
suite and test, and every cargo build through `tools/dev/bin/cargo` (builds, clippy, tests,
`cargo run`, measurement runs and backtests; in a worktree under its lock). Recorded by the Macs themselves
(`tools/dev/devjob.zsh` and `devjob.py`, tools/dev/testing.md, "Dev jobs"); this app keeps and shows
them.

- **Where it ran (#1086):** a span's Mac is the one it ran on. A test-Mac run (`run`, its `ran_on`)
  and its compiles, suites and tests are the test Mac's; the `tests` job holding it is the Mac's it
  started on, its `ran_on` naming the test Mac as soon as the run starts there (`rerun_here` when it
  ran here after all). The API reads each span `on` the Mac it ran on: a tests job whose run is on the
  test Mac shows on that Mac, "started on the M1 Pro, ran on the M4 Pro"; one run here stays on its
  own.
- **Its form** (`PRAGMA user_version`) is 2 (the runs and the tests jobs indexed by when they
  ended, so a timeline's reads never go further back than its range); any other form is refused.
- **What a run is:** a tree of spans (`jobs.db`'s `spans`): each a kind, a name, its Mac, its start
  and end, a status (`running`, `ok`, `fail`, `stopped`, `gone`: its process ended without saying
  how, `skip`, `reuse`) and attributes (a run of its own: its checkout, branch, commit, task, whether
  an agent ran it; a cargo's command and purpose; a wrapped command's exit, CPU and largest
  process's memory; a test-Mac run's Mac, load and CPU). Kinds: `check` with a `part` each, `publish`
  with a `step` each, `tests` (a remote-tests.sh run) with its `wait`s (the test Mac's key, a slot),
  `copy`, `run` on the test Mac, `compile`s, `suite`s and `test`s; `cargo` with its `wait` for the
  shared build folder, `reset` (the units tied to another checkout), `build` and each `crate` compiled
  (cargo's `--timings` report), each test binary run as a `suite`, a `program` run.
- **Each Mac's load** (1-minute load average), memory free (`kern.memorystatus_level`, %) and swap
  used, every 10 s while any recorded job runs on it (`samples`), and its chip, memory and cores
  (`macs`). The test Mac is sampled during its runs too (remote-tests.sh starts its sampler there).
- **The page** (`public/jobs/`): the runs on a timeline, a group a Mac (the one each ran on; a
  check's or publish's tests job on the test Mac too, opening its run), a lane a kind (a cargo
  measurement or backtest its own), each Mac's load and memory as heat strips under them; a run
  pointed at shows its card and lights its checkout's other runs, a click opens it. A run opened:
  its facts, its spans on a timeline of their own (a lane a kind at each depth, deepest last), where
  its time went, its longest and failed spans, the time by kind; its Macs' load and memory over it.
  By name: one kind of span over the timeline's range (6 h to all time), each name's runs, failures, median, p90, longest, total
  and recent times. Both timelines are the build page's (`web/buildpage/timeline.js`, served at
  `/lib/buildpage/`, as are the page's elements and words, `dom.js` and `words.js`, and a run's spans laid out as lanes, `runlanes.js`, a job page's too): dragged or scrolled sideways to move, pinched or ⌥-scrolled to zoom, down to 20 s.
  The page's address keeps what's open (`?range=24h&run=<id>&kind=test&hide=cargo`).
- **Lost:** a running span its Mac's sampler hasn't named for a minute (`jobs.mjs` LOST_S) shows as
  lost: the Mac asleep, away or restarted. The sampler ends one whose process is gone as `gone`.

**The API.** `POST /api/jobs/events` takes NDJSON (or a JSON array) of events, each merged into
its span by its id, so they may come in any order and again: `start` (`id`, `parent`, `kind`,
`name`, `mac`, `t`, `attrs`), `end` (`id`, `t`, `status`, `attrs`), `span` (all at once: `t0`,
`t1`), `attr` (`id`, `attrs`), `sample` (`mac`, `t`, `load`, `memfree`, `swap`, `ncpu`, `chip`,
`mem`, `live`: the ids of the spans live on it). Times are unix seconds. Each line that fits is
taken; one that doesn't is answered in `refused` (`{line, why}`), the rest kept. Reads:
`GET /api/jobs/runs?from=&to=[&kind=]` (the runs overlapping it, each with its span count, failed
count and the Macs its spans ran on, `macs`; `away`, the tests jobs inside other runs whose run went
to another Mac, each with its run's id, `root`; and the Macs), every span read with `on`, `GET /api/jobs/runs/<id>` (the run a span is part of, whole, with its Macs'
samples), `GET /api/jobs/samples?from=&to=&n=` (each Mac's samples in at most `n` buckets: the most
load, the least memory free, the most swap), `GET /api/jobs/stats?kind=[&from=&to=]` (each name's
runs, failures, p50, p90, longest, total and last 40 times), `GET /api/jobs/kinds[?from=&to=]` (each kind and how many, begun in the range; all time without one).

## The task board's API (for the PM, agents and scripts)

Everything is under `/api/c/<collection>`, JSON in and out, `B=http://mac-mini-de-brandon:8077`. **You send content; the server assigns the rest.** A new item's `num`, `id`, `order` and `added`, a new event's `id` and `t`, `meta/counter` and `meta/now`'s `updated` are the server's: sending one is a 400. Every write is checked against the schema (below) in one transaction: a request that doesn't fit gets a **400** with every reason (`error.message`, and `error.errors` as `[{field, message}]`), and nothing is written. Errors are `{"error": {"code", "message"}}`: `invalid_argument` 400, `not_found` 404, `forbidden` 403, `conflict` 409. **Names from before #1229 are refused** (400) with the name to use: an item's `stage` is `status`, `where` is `assigned`; an event's kind `stage` is `status`; `meta/status` is `meta/now`, its `now` is `text` (`GET /api/schema`'s `renamed` lists them).

**Add a task** (`POST /api/c/items`). Only `text` is required; `kind` defaults to `task`, `parent` to `""` (top level; else an item's id), `status` to `""` (queued), `assigned` to `Opus subagent` (`DEFAULT_ASSIGNED` in `schema.mjs`; the page gives a task it makes, or a folder it turns into a task, the same). Also allowed: `note`, `priority`, `discuss`. `parent` must be a folder (only folders hold items), and not an archived one nor inside one (below).

```bash
curl -s -X POST $B/api/c/items -H 'content-type: application/json' \
  -d '{"text": "Task board: do the thing", "note": "What and why.", "priority": 15, "assigned": "Subagent (Sonnet)", "parent": "f-publish"}'
# → {"id":"i674","num":674,"order":52,"added":"2026-10-26T09:03:14.285Z"}      (the task is #674, id i674)
```

A new item's id is `i<num>`, so `GET $B/api/c/items/i674` reads it (older items keep the ids they have, e.g. `f-publish`). A folder (`"kind": "folder"`, `text`, `parent`, `note`) is numbered from the same counter as tasks: #501 a folder, then #502 a task. `order` is one order over all items, the last + 1 for a new one: the tiebreaker of every view (siblings in the tree, equal priorities in the sort), and what a drag changes.

**Add a history event** (`POST /api/c/events`): `item` (an item's id, or `""` for the board itself), `kind` (one of the list below), `text`. An archived item's history is read-only: an event on it, or on an item inside an archived folder, is refused (400), and so is a change of one already in it.

```bash
curl -s -X POST $B/api/c/events -H 'content-type: application/json' \
  -d '{"item": "i674", "kind": "delegate", "text": "Delegated to an Opus subagent in a worktree."}'
# → {"id":"4f0c…","t":"2026-10-26T09:05:00.113Z"}
```

**Change a task**: `PATCH /api/c/items/<id>` with only the fields that change (`null` removes an optional field). A missing id is a 404: a PATCH can't create. `num`, `id` and `added` can't be changed; `order` can (a reorder); a folder made a task (`"kind": "task"` with `status` and `assigned`) or a task made a folder (null its task fields in the same PATCH) keeps its number; a folder holding items can't become a task. A change of `status` leaves a history event (`kind: "status"`, "Status Queued → Under way (by curl)", by the client's User-Agent) whoever makes it.

```bash
curl -s -X PATCH $B/api/c/items/i674 -H 'content-type: application/json' -d '{"status": "working", "assigned": "Subagent (Opus)"}'
# → {"id":"i674","num":674,"order":52,"added":"…"}
```

**A priority with a fraction** (#1100) slots in between, in the same transaction as the rest of the
write, by `priority.mjs` (the page places a drag or a typed number by the same module): a POST, PATCH
or PUT sending `"priority": 6.5` gives the task 7, and the open tasks it then runs into (at 7, then
at 8 if one was moved there, …) move down one, as far as the first free number; tasks sharing a
number move together, and the rest keep theirs. Open: not done or abandoned, not archived, nor inside
an archived folder; the others keep their numbers and are never run into. A whole number is stored
as sent and moves nothing; 0.5 gives 1; a negative number or one sent as text is refused (400) with
the rest of the write, nothing moved. The whole move leaves ONE `kind: "priority"` event, on the task
placed ("Priority 12 → 7: asked for 6.5; 2 others moved down one, from priority 7–8 (#1042, #1043) (by
curl)"; its number unchanged, "Priority 7: asked for 6.5; …"; up to 10 numbers listed, then "and N
more"); the tasks pushed down get none (their new number is visible, and that event names them). The
answer adds what it got and what moved (below). The page works the moves out with the same module and sends each
as a whole number, its own edit, so Undo puts each back: those leave no `priority` event (a drag or
⌥↑ leaves the page's own note on the task it moved).

```bash
curl -s -X PATCH $B/api/c/items/i1100 -H 'content-type: application/json' -d '{"priority": 6.5}'
# → {"id":"i1100","num":1100,"order":957,"added":"…","priority":7,"moved":[{"id":"i1042","num":1042,"from":7,"to":8}]}
```

`PUT /api/c/items/<id>` replaces what a client writes (it needs `kind`, `text`, `parent`, and for a task `status` and `assigned`), keeping `num`, `added` and `order`; a missing id is a 404 too. **Use PATCH**: a PUT with only `{status}` is refused, not a wipe, but PATCH is what you mean.

**Archived items are read-only** (#1229). An item is archived with `PATCH {"archived": true}` (`archivedAt`, when, is the server's time unless sent) and unarchived with `{"archived": false}` (or `null`; its `archivedAt` goes with it). No item that isn't archived is ever inside an archived folder:

- An archived item, or one inside an archived folder, can't be changed: any other PATCH, any PUT, and a `DELETE` of it or of an event in its history are refused (400, naming it); the one change taken is unarchiving it. Its history is read-only too (above). A folder that isn't archived is deleted with everything inside it, archived or not.
- Nothing goes into an archived folder, at any depth: a POST or a PATCH whose `parent` is one, or is inside one, is refused, naming the folder. (A deleted item isn't restored into a folder archived since, nor a deleted event into the history of an item archived since: 409.)
- Archiving a folder holding items that aren't archived is refused, naming them: archive those first. The page, once you have confirmed its warning, sends `"cascade": true` with it: those are archived with it, at the same time. (`cascade` is sent, never stored, and only with `archived`.)
- Unarchiving an item inside an archived folder is refused, naming the folder: unarchive that first, or send `"cascade": true` to unarchive the archived folders around it with it (what else is inside them stays archived).
- Each archive and unarchive leaves a `kind: "archive"` event ("Archived, with the 3 inside (by curl)", "Unarchived (by the page)").

```bash
curl -s -X PATCH $B/api/c/items/i1305 -H 'content-type: application/json' -d '{"archived": true}'
# → 400 archived: #1305 "A folder" holds 2 item(s) that aren't archived (#1306, #1307): archive those first
```

**The now line** (what Claude is on, in the page's header): `PATCH /api/c/meta/now` with `{"text": "…"}`. The server sets `updated` (send it and it's a 400); `meta/counter` can't be written (403), nor deleted.

```bash
curl -s -X PATCH $B/api/c/meta/now -H 'content-type: application/json' -d '{"text": "In flight: #601, #672."}'
```

**Delete**: `DELETE /api/c/items/<id>` also deletes what is inside it and every event about any of them (a reference never points at nothing); `DELETE /api/c/events/<id>` one event. The page's Undo puts it back with `POST /api/c/<collection>/<id>/restore` (the server keeps the last 100 deletions in memory; a restart forgets them). A deleted item's number is never given again.

**Read**: `GET /api/c/items/<id>` (`{exists, data}`); `GET /api/c/items` and `GET /api/c/events` (#1344), **every document asked for, with no cap**:

- The answer is `{docs: [{id, data}], total, mark}` (and `next`, below). `total` is how many there are (one item's, with `item`); `mark` is where the board's change log stood when the read was made (`since`, below).
- The order: `?orderBy=<field>&dir=asc|desc` (a field of the collection, or `id`), then the id, the same way (`dir` alone: the default order that way). Without `orderBy`, items by `num` and events by `t`, oldest first: a stable order, new ones last. A null comes first going up, last going down.
- `limit=<n>` (any whole number from 1): at most that many; without it, all of them. **A read that stops short of the end says so**: its answer has `next`, a place in that order to pass back as `after=<next>` (with the same `orderBy` and `dir`; from another order it's a 400) for the documents after it. An answer without `next` holds everything from where it began. So `events?limit=20000` is every event while there are at most 20,000; past that, its `next` reads the rest.
- An answer is read from the file at one moment (a read-only connection of its own, in a transaction): `total`, `mark` and the documents are of the same moment, whatever is written meanwhile. It is sent as it is read, a chunk at a time, waiting for a slow reader, so the server never holds a whole large answer; a reader that takes nothing for 30 s (`board.mjs` STALL_MS) is cut off, as the read's transaction keeps the WAL from being checkpointed while it waits. One that fails part way ends the connection: the reader gets an error, never an answer that parses.
- `?since=<mark>`, alone: what changed since the read that gave that mark, `{docs: those changed, as they are now, deleted: [the ids of those gone], total, mark}`. The change log is the server's, in memory (`store.mjs` CHANGES_KEPT, the last 10,000 changes): a mark from before a restart, one older than that, or `since=0`, is answered with everything, `{docs, total, mark, full: true}`. The page's items are read so (shim.js): whole once, then on each change only what changed.
- `meta` takes no query: `GET /api/c/meta` is `{docs, total}`, both its documents.
- Any other query, field or value is a 400, as is a query given twice. `GET /api/schema` lists the fields and the fixed lists.

**History is read by item, never all at once** (#1333). `GET /api/c/events` also takes `item=<id>` (one item's events, `item=` the board's own; `total` is then that item's count, whatever the page), in pages as any list: `?item=i57&orderBy=t&dir=desc&limit=100`, then `&after=<next>` (a tie in `t` loses or repeats none). `GET /api/history` is every item's events in short, for lists that show a count and the latest change: `{"<item id>": [count, latest t, mark], …}` (`""` the board's own; the mark changes with any write or edit of the item's events, the count with a delete). `GET /api/history?since=<mark>` (one an earlier `?since=` answer gave, or `0`) is the same for the items whose events changed since: `{items: {"<item id>": [count, latest t, mark] | null (none left)}, mark}`; everything, with `full: true`, when the change log can't say (as above). The page reads that, kept live, and the newest 100 events of each item it shows open (in a row or the details) with a "Show older" for the rest (from the last page's `next`): no cap hides any history. On a change it reads what changed in the summary, then again only the open histories whose summary changed, as many events as it holds of each, so an edit or a delete shows.

### The schema (`schema.mjs`)

- **Times** are one format everywhere: ISO 8601, UTC, to the millisecond (`2026-10-26T08:35:48.000Z`, `new Date().toISOString()`). A time you send may leave the fraction out (`…08:35:48Z`) and is stored in full. Epoch numbers or digit strings (`"1792000000000.0"`, what broke #634–#673 on 26 Oct), offsets and bare dates are refused.
- **Items**: `kind` (`task` | `folder`), `text` (non-empty, ≤ 2000), `parent` (`""` or an existing folder; not itself or inside itself), `order` (number; one order over all items), `added`, `note`, `archived` (bool; `archivedAt` only with it; read-only while true, above), `photos` (blob ids from `POST /api/blob`), and `num` (whole number, from 1, unique; every item has one). Tasks only: `status`, `assigned` (who or what it is assigned to: free text, one line, ≤ 200; `""` nobody, not spaces), `priority` (number ≥ 0), `discuss` (bool), `started`, `finished` (times), `reply`. A folder with any of those is refused; unknown fields are refused.
- **Events**: `item` (an existing item's id, or `""`), `kind`, `text` (non-empty), `t`. Only `kind` and `text` can be changed afterwards.
- **Meta**: only `now` (`text`: string, `updated`: time) and `counter` (`next`).
- **Fixed lists** (the values in use on 26 Oct 2026; to allow another, add it to `schema.mjs`): status `""` (queued), `working`, `backgrounded`, `integrating`, `review`, `waiting`, `done`, `abandoned`; event kind `note`, `owner`, `status`, `archive` and `priority` (the server's, above), `added`, `start`, `delegate`, `progress`, `review`, `merge`, `publish`, `done`, `wait`, `stop`, `bug`, `decision`; `assigned` is free text: the page's editor suggests `ASSIGNED_SUGGESTIONS` (`Opus subagent`, `Subagent`, `Subagent (Opus)`, `Subagent (Sonnet)`, `You`, `PM (M4)`, …).
