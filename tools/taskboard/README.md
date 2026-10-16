# Task board

The Scenic Roads task board: a standalone web app with its data in SQLite.

```
node server.mjs [--port 8077] [--host 127.0.0.1] [--db taskboard.db]
```

Open http://127.0.0.1:8077. Needs Node 22.5+ (`node:sqlite`); no packages.

- `public/index.html` is the page; `public/shim.js` gives it its database API (`window.claude.use("db" | "user" | "assets")`) over `server.mjs`'s HTTP API. Open tabs hear of changes through `/api/stream`.
- `taskboard.db` is the source of truth: `items` (folders and tasks), `events` (history), `meta` (`status`, the status bar; `counter`, the next task number), `blobs` (photos, at `/_blob/<id>`). Back it up with `sqlite3 taskboard.db ".backup copy.db"`.
- From a script: `PATCH /api/c/meta/status` with `{"now", "updated"}`; `PUT /api/c/events/<id>` with `{"item", "t", "text", "kind"}`; items under `/api/c/items/<id>`. A new task takes its number from `meta/counter` (then set `next` one higher).
- `node seed.mjs [db]` fills an empty database from `seed/`, the board's original export.
- No sign-in: keep it bound to localhost.
