// The Scenic Roads dev tools app (README.md): one Node process (no dependencies; Node 22.5+ for
// node:sqlite) serving its pages and their data, each part in its own SQLite file.
//
//   node server.mjs [--port 8077] [--host 127.0.0.1] [--data <folder>]
//
//   /             the index: a link to each tool
//   /task-board/  the task board (board.mjs, its API under /api/c/…, photos at /_blob/<id>)
//   /jobs/        the dev job dashboard (jobs.mjs, its API under /api/jobs/…)
//   /api/stream   an event stream telling open pages which collection changed
// The pages' addresses are relative (`../api/…`), so the app works under a proxy's path (tools/caddy's
// /dev/). The dashboard's timeline is the build page's own (web/buildpage/timeline.js and what it imports,
// served from the checkout at /lib/buildpage/), as are its elements and words (dom.js, words.js) and a run's lanes (runlanes.js).
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { openStore } from "./store.mjs";
import { makeBoard } from "./board.mjs";
import { openJobs, makeJobs } from "./jobs.mjs";
import { Refusal, send, fail } from "./http.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json" };
// The build page's modules the dashboard imports (its timeline, and that's; its elements and words).
const PAGE_DIR = path.join(here, "../../web/buildpage");
const PAGE_LIB = new Set(["timeline.js", "runlanes.js", "sums.js", "chartmath.js", "dom.js", "words.js"]);

export function makeServer({ board: boardStore, jobs: jobsStore }) {
  // ---- change notices: which collection changed, to every open page (a burst sent once) ----------
  const streams = new Set();
  let pending = new Set(), notifyT = null;
  const changed = (...cols) => {
    for (const c of cols) pending.add(c);
    if (notifyT) return;
    notifyT = setTimeout(() => {
      for (const c of pending) for (const res of streams) res.write(`event: change\ndata: ${JSON.stringify({ col: c })}\n\n`);
      pending = new Set();
      notifyT = null;
    }, 25);
  };
  const ping = setInterval(() => { for (const res of streams) res.write(": ping\n\n"); }, 25000);
  ping.unref();

  const board = makeBoard(boardStore, changed);
  const jobs = makeJobs(jobsStore, changed);

  // A page or its files: [file, its type], or null.
  function page(p) {
    if (p === "/") return path.join(here, "public/index.html");
    // (A task's own address, /task-board/709, is the board's page too: it opens with #709 selected.)
    if (p === "/task-board/" || /^\/task-board\/\d+$/.test(p)) return path.join(here, "public/task-board/index.html");
    // (schema.mjs and priority.mjs are the board page's too: it checks what it shows by the same
    // rules, and places a priority by the same rule.)
    if (p === "/task-board/schema.mjs" || p === "/task-board/priority.mjs") return path.join(here, p.slice(12));
    if (p === "/jobs/") return path.join(here, "public/jobs/index.html");
    if (p.startsWith("/lib/buildpage/")) return PAGE_LIB.has(p.slice(15)) ? path.join(PAGE_DIR, p.slice(15)) : null;
    const f = path.join(here, "public", path.normalize(p));
    return f.startsWith(path.join(here, "public") + path.sep) ? f : null;
  }

  return http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://x");
      // (The board's page from before the dev tools app, still open behind the proxy's old path, asks
      // for /task-board/api/… and /task-board/_blob/…: the same as /api/… and /_blob/….)
      if (/^\/task-board\/(api|_blob)\//.test(url.pathname)) url.pathname = url.pathname.slice("/task-board".length);
      if (url.pathname.startsWith("/api/")) {
        let parts;
        try { parts = url.pathname.split("/").slice(2).map(decodeURIComponent); } catch { return fail(res, 400, "invalid_argument", "the path is not valid percent-encoding"); }
        if (parts[0] === "stream" && req.method === "GET") {
          res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store", connection: "keep-alive" });
          res.write("retry: 1000\n\n");
          streams.add(res);
          req.on("close", () => streams.delete(res));
          return;
        }
        if (parts[0] === "jobs") return await jobs.api(req, res, url, parts.slice(1));
        return await board.api(req, res, url, parts);
      }
      if (url.pathname.startsWith("/_blob/") && req.method === "GET") return board.blob(res, decodeURIComponent(url.pathname.slice(7)));
      if (req.method !== "GET" && req.method !== "HEAD") return send(res, 405, "method not allowed", "text/plain");
      // (A page's address without its slash: to it, relative, so a proxy's path stays.)
      const m = /^\/(task-board|jobs)$/.exec(url.pathname);
      if (m) return send(res, 301, "", "text/plain", { location: `${m[1]}/${url.search}` });
      // (A task's address from when the board was the whole app, /709: the board's now.)
      if (/^\/\d+$/.test(url.pathname)) return send(res, 301, "", "text/plain", { location: `task-board${url.pathname}${url.search}` });
      const file = page(url.pathname);
      if (!file || !fs.existsSync(file) || !fs.statSync(file).isFile()) return send(res, 404, "not found", "text/plain");
      return send(res, 200, fs.readFileSync(file), TYPES[path.extname(file)] || "application/octet-stream");
    } catch (e) {
      if (e instanceof Refusal) return fail(res, e.status, e.code, e.message, e.errors);
      if (e?.code === "too_large") return fail(res, 413, "too_large", `the body is over ${e.max / 1e6} MB`);
      console.error(e);
      if (!res.headersSent) fail(res, 500, "internal", String(e?.message || e));
      else res.end();
    }
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const arg = (name, d) => { const i = process.argv.indexOf("--" + name); return i > 0 ? process.argv[i + 1] : d; };
  const PORT = Number(arg("port", process.env.PORT || 8077));
  const HOST = arg("host", process.env.HOST || "127.0.0.1");
  const DATA = path.resolve(arg("data", here));
  fs.mkdirSync(DATA, { recursive: true });
  const server = makeServer({ board: openStore(path.join(DATA, "taskboard.db")), jobs: openJobs(path.join(DATA, "jobs.db")) });
  server.listen(PORT, HOST, () => console.log(`Dev tools on http://${HOST}:${PORT}  (data: ${DATA})`));
}
