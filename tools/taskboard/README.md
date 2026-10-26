# Task board

The Scenic Roads task board: a standalone web app with its data in SQLite.

```
node server.mjs [--port 8077] [--host 127.0.0.1] [--db taskboard.db]
node --test test/*.test.mjs        # the API's tests (npm test)
```

Open http://127.0.0.1:8077. Needs Node 22.5+ (`node:sqlite`); no packages.

- `schema.mjs` is the shape of the data: one schema per collection, enforced by the server on every write (and read by the page, which shows what doesn't fit). `store.mjs` is the SQLite file, `server.mjs` the HTTP API and the page's server, `repair.mjs` the one-time repair of old data (#672).
- `public/index.html` is the page; `public/shim.js` gives it its database API (`window.claude.use("db" | "user" | "assets")`) over `server.mjs`'s HTTP API. Open tabs hear of changes through `/api/stream`.
- `taskboard.db` is the source of truth: `items` (folders and tasks), `events` (history), `meta` (`status`, the status bar; `counter`, the next task number), `blobs` (photos, at `/_blob/<id>`). It is not in git: it lives on the M6, which copies it nightly to the NAS (`state/taskboard/<date>.db`, the newest 14; `launchd/backup.sh`).
- **On the M6:** `launchd/install.sh [node]` (default `mise which node`) installs it as a LaunchAgent (`local.scenic.taskboard`, `--host ::`, IPv4 and IPv6, log `~/Library/Logs/scenic/taskboard.log`) and the backup (`local.scenic.taskboard-backup`, 03:30). The other Macs use it at http://mac-mini-de-brandon:8077, and any device on the tailnet at https://map.brandontsang.net/task-board/ (Caddy, `tools/caddy`). The page's addresses are relative (`api/…`, `_blob/…`), so it works under that path.
- No sign-in, by the owner's rule: anyone on the home network or tailnet may use it. Elsewhere `--host` stays 127.0.0.1.

## The API (for the PM, agents and scripts)

Everything is under `/api/c/<collection>`, JSON in and out, `B=http://mac-mini-de-brandon:8077`. **You send content; the server assigns the rest.** A new task's `num`, `id`, `order` and `added`, a new event's `id` and `t`, `meta/counter` and `meta/status`'s `updated` are the server's: sending one is a 400. Every write is checked against the schema (below) in one transaction: a request that doesn't fit gets a **400** with every reason (`error.message`, and `error.errors` as `[{field, message}]`), and nothing is written. Errors are `{"error": {"code", "message"}}`: `invalid_argument` 400, `not_found` 404, `forbidden` 403, `conflict` 409.

**Add a task** (`POST /api/c/items`). Only `text` is required; `kind` defaults to `task`, `parent` to `""` (top level; else an item's id), `stage` to `""` (queued), `where` to `Subagent`. Also allowed: `note`, `priority`, `discuss`, `slot`.

```bash
curl -s -X POST $B/api/c/items -H 'content-type: application/json' \
  -d '{"text": "Task board: do the thing", "note": "What and why.", "priority": 15, "where": "Subagent (Sonnet)", "parent": "f-publish"}'
# → {"id":"i674","num":674,"order":52,"added":"2026-10-26T09:03:14.285Z"}      (the task is #674, id i674)
```

A task's id is `i<num>`, so `GET $B/api/c/items/i674` reads it. A folder (`"kind": "folder"`, `text`, `parent`, `note`) gets a random id and no number.

**Add a history event** (`POST /api/c/events`): `item` (an item's id, or `""` for the board itself), `kind` (one of the list below), `text`.

```bash
curl -s -X POST $B/api/c/events -H 'content-type: application/json' \
  -d '{"item": "i674", "kind": "delegate", "text": "Delegated to an Opus subagent in a worktree."}'
# → {"id":"4f0c…","t":"2026-10-26T09:05:00.113Z"}
```

**Change a task**: `PATCH /api/c/items/<id>` with only the fields that change (`null` removes an optional field). A missing id is a 404: a PATCH can't create. `num`, `id` and `added` can't be changed; `order` can (a reorder); a folder made a task (`"kind": "task"` with `stage` and `where`) gets a number, and a task made a folder keeps its own (null its task fields in the same PATCH). A change of `stage` leaves a history event (`kind: "stage"`, by the client's User-Agent) whoever makes it.

```bash
curl -s -X PATCH $B/api/c/items/i674 -H 'content-type: application/json' -d '{"stage": "working", "where": "Subagent (Opus)"}'
# → {"id":"i674","num":674,"order":52,"added":"…"}
```

`PUT /api/c/items/<id>` replaces what a client writes (it needs `kind`, `text`, `parent`, and for a task `stage` and `where`), keeping `num`, `added` and `order`; a missing id is a 404 too. **Use PATCH**: a PUT with only `{stage}` is refused, not a wipe, but PATCH is what you mean.

**The status line**: `PATCH /api/c/meta/status` with `{"now": "…"}`. The server sets `updated` (send it and it's a 400); `meta/counter` can't be written (403), nor deleted.

```bash
curl -s -X PATCH $B/api/c/meta/status -H 'content-type: application/json' -d '{"now": "In flight: #601, #672."}'
```

**Delete**: `DELETE /api/c/items/<id>` also deletes what is inside it and every event about any of them (a reference never points at nothing); `DELETE /api/c/events/<id>` one event. The page's Undo puts it back with `POST /api/c/<collection>/<id>/restore` (the server keeps the last 100 deletions in memory; a restart forgets them). A deleted task's number is never given again.

**Read**: `GET /api/c/items/<id>` (`{exists, data}`), `GET /api/c/items` (`{docs: [{id, data}], total}`; `?orderBy=<field>&dir=desc&limit=<n>`), the same for `events` and `meta`. `GET /api/schema` lists the fields and the fixed lists.

### The schema (`schema.mjs`)

- **Times** are one format everywhere: ISO 8601, UTC, to the millisecond (`2026-10-26T08:35:48.000Z`, `new Date().toISOString()`). A time you send may leave the fraction out (`…08:35:48Z`) and is stored in full. Epoch numbers or digit strings (`"1792000000000.0"`, what broke #634–#673 on 26 Oct), offsets and bare dates are refused.
- **Items**: `kind` (`task` | `folder`), `text` (non-empty, ≤ 2000), `parent` (`""` or an existing item; not itself or inside itself), `order` (number), `added`, `note`, `archived` (bool; `archivedAt` only with it), `photos` (blob ids from `POST /api/blob`), and `num` (whole number, from 1, unique; a task has one, a folder that was a task keeps its own). Tasks only: `stage`, `where`, `priority` (number ≥ 0), `slot` (whole number), `discuss` (bool), `started`, `finished` (times), `reply`. A folder with any of those is refused; unknown fields are refused.
- **Events**: `item` (an existing item's id, or `""`), `kind`, `text` (non-empty), `t`. Only `kind` and `text` can be changed afterwards.
- **Meta**: only `status` (`now`: string, `updated`: time) and `counter` (`next`).
- **Fixed lists** (the values in use on 26 Oct 2026; to allow another, add it to `schema.mjs`): stage `""` (queued), `working`, `backgrounded`, `integrating`, `review`, `waiting`, `done`, `abandoned`; event kind `note`, `owner`, `stage`, `added`, `start`, `delegate`, `progress`, `review`, `merge`, `publish`, `done`, `wait`, `stop`, `bug`, `decision`; `where`: see `WHERES` (`Subagent`, `Subagent (Opus)`, `Subagent (Sonnet)`, `You`, `PM (M4)`, …). The page's editor offers `where` from the same list.

### Old data: `repair.mjs` (one time, #672)

`node repair.mjs [db]` validates the existing data against the schema and prints what it would change, in groups with a few examples each (`--all` lists every change). It changes nothing: it does the whole repair inside a transaction, checks the result, and rolls back. `node repair.mjs [db] --run` does it for real: it first copies the file (`<db>.before-repair-<time>`), commits only if nothing is left that doesn't fit, and keeps any event it had to drop in `<db>.quarantine-<time>.json`. Stop the server first. On the M6: stop the LaunchAgent, pull (the new server and `repair.mjs`), dry run, `--run`, start. Dating what has no time: an item takes the earliest of its own times (first event, start, finish, archiving), no later than the next higher-numbered task's `added` or, for a folder, what is inside it; an event takes the time of the row written just before it (SQLite's write order), never before its item's first time.
