// Fills a new task board database from an export: seed/{items,events,meta}/<id>.json and
// seed/blobs/<id>.<ext> (a photo's id is the first 32 hex digits of its SHA-256, as the server makes it).
//   node seed.mjs [db] [seed-dir]        (refuses a database that already has items)
import { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
const here = path.dirname(fileURLToPath(import.meta.url));
const dbPath = path.resolve(process.argv[2] || path.join(here, "taskboard.db"));
const dir = path.resolve(process.argv[3] || path.join(here, "seed"));

// Let server.mjs create the schema: start it on a throwaway port against this file, then stop it.
const { spawn } = await import("node:child_process");
const srv = spawn(process.execPath, [path.join(here, "server.mjs"), "--port", "0", "--db", dbPath], { stdio: "ignore" });
await new Promise((r) => setTimeout(r, 800));
srv.kill();
await new Promise((r) => setTimeout(r, 200));

const BOOL = new Set(["discuss", "archived"]);
const COLS = {
  items: ["kind", "text", "parent", "order", "num", "stage", "where", "note", "reply", "priority", "slot", "discuss", "archived", "archivedAt", "added", "started", "finished", "photos"],
  events: ["item", "kind", "t", "text"],
};
const db = new DatabaseSync(dbPath);
if (db.prepare("SELECT count(*) n FROM items").get().n) { console.error("The database already has items; not seeding."); process.exit(1); }
const read = (col) => fs.existsSync(path.join(dir, col)) ? fs.readdirSync(path.join(dir, col)).filter((f) => f.endsWith(".json")).map((f) => [f.slice(0, -5), JSON.parse(fs.readFileSync(path.join(dir, col, f), "utf8"))]) : [];
db.exec("BEGIN");
for (const col of ["items", "events"]) {
  const names = ["id", ...COLS[col], "extra"];
  const ins = db.prepare(`INSERT INTO ${col} (${names.map((n) => `"${n}"`).join(",")}) VALUES (${names.map(() => "?").join(",")})`);
  for (const [id, doc] of read(col)) {
    const extra = {};
    const vals = COLS[col].map((c) => { const v = doc[c]; return v == null ? null : BOOL.has(c) ? (v ? 1 : 0) : c === "photos" ? JSON.stringify(v) : v; });
    for (const k of Object.keys(doc)) if (!COLS[col].includes(k) && doc[k] != null) extra[k] = doc[k];
    ins.run(id, ...vals, Object.keys(extra).length ? JSON.stringify(extra) : null);
  }
}
const meta = db.prepare("INSERT INTO meta (key, value) VALUES (?, ?)");
for (const [id, doc] of read("meta")) meta.run(id, JSON.stringify(doc));
const types = { ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif", ".webp": "image/webp" };
const blobDir = path.join(dir, "blobs");
if (fs.existsSync(blobDir)) for (const f of fs.readdirSync(blobDir)) {
  const data = fs.readFileSync(path.join(blobDir, f)), sha = crypto.createHash("sha256").update(data).digest("hex");
  db.prepare("INSERT OR IGNORE INTO blobs (id, type, data, sha256, created) VALUES (?,?,?,?,?)").run(path.basename(f, path.extname(f)), types[path.extname(f).toLowerCase()] || "application/octet-stream", data, sha, new Date().toISOString());
}
db.exec("COMMIT");
for (const t of ["items", "events", "meta", "blobs"]) console.log(t, db.prepare(`SELECT count(*) n FROM ${t}`).get().n);
