# Task board

A standalone web app: the Scenic Roads task board (the Claude artifact), with its data in SQLite.

```
node server.mjs [--port 8090] [--host 127.0.0.1] [--db taskboard.db]
```

Open http://127.0.0.1:8090. Needs Node 22.5+ (`node:sqlite`); no packages to install.

- `public/index.html` is the artifact's page, unchanged. `public/shim.js` gives it the document-database API it was written for (`window.claude.use("db" | "user" | "assets")`) over `server.mjs`'s HTTP API; every open tab hears of a change through an event stream (`/api/stream`) and reads again.
- `taskboard.db` holds the data: `items` (folders and tasks: a column per field, `extra` JSON for any other), `events` (each item's history), `meta` (`status`, the status bar's text; `counter`, the next task number), `blobs` (attached photos, served at `/_blob/<id>`). It is the source of truth: back it up with `sqlite3 taskboard.db ".backup copy.db"`.
- `seed/` is the export of the artifact's data it started from (163 items, 233 events, 2 meta docs, 1 photo). `node seed.mjs [db]` rebuilds a database from it (only into one without items).
- To update the status bar or add history from a script: `PATCH /api/c/meta/status` with `{"now": "...", "updated": "<ISO time>"}`; `PUT /api/c/events/<id>` with `{"item", "t", "text", "kind"}`; items the same under `/api/c/items/<id>`. A new task takes the next number from `meta/counter` (set `next` one higher).
- Not kept from the artifact: its sharing and owner-only writes. This app has no sign-in: bind it to localhost (the default), or put it behind something that has.
