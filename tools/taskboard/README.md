# Task board

The Scenic Roads task board: a standalone web app with its data in SQLite.

```
node server.mjs [--port 8077] [--host 127.0.0.1] [--db taskboard.db]
```

Open http://127.0.0.1:8077. Needs Node 22.5+ (`node:sqlite`); no packages.

- `public/index.html` is the page; `public/shim.js` gives it its database API (`window.claude.use("db" | "user" | "assets")`) over `server.mjs`'s HTTP API. Open tabs hear of changes through `/api/stream`.
- `taskboard.db` is the source of truth: `items` (folders and tasks), `events` (history), `meta` (`status`, the status bar; `counter`, the next task number), `blobs` (photos, at `/_blob/<id>`). It is not in git: it lives on the M6, which copies it nightly to the NAS (`state/taskboard/<date>.db`, the newest 14; `launchd/backup.sh`).
- From a script: `PATCH /api/c/meta/status` with `{"now", "updated"}`; `PUT /api/c/events/<id>` with `{"item", "t", "text", "kind"}`; items under `/api/c/items/<id>`. A new task takes its number from `meta/counter` (then set `next` one higher).
- `node seed.mjs [db]` fills an empty database from `seed/`, the board's original export.
- **On the M6:** `launchd/install.sh [node]` (default `mise which node`) installs it as a LaunchAgent (`local.scenic.taskboard`, `--host ::`, IPv4 and IPv6, log `~/Library/Logs/scenic/taskboard.log`) and the backup (`local.scenic.taskboard-backup`, 03:30). The other Macs use it at http://mac-mini-de-brandon:8077, and any device on the tailnet at https://map.brandontsang.net/task-board/ (Caddy, `tools/caddy`). The page's addresses are relative (`api/…`, `_blob/…`), so it works under that path.
- No sign-in, by the owner's rule: anyone on the home network or tailnet may use it. Elsewhere `--host` stays 127.0.0.1.
