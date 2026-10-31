// The build page's service worker (docs/workers.md §7, The page as an app). The page's files are kept
// as this version installs, and come from the lead whenever it answers well within WAIT_MS (a
// newly published app's page at once), else as kept: a Mac that doesn't (asleep, away, or its
// agent restarting: tailscale serve answers 502) is taken for away for AWAY_MS, what was kept used
// at once meanwhile. Everything else (every request to the coordinator) isn't touched.
//
// VERSION is the page's own (the coordinator's hash of its files) and PAGE their addresses (the
// coordinator's list): a new version is a new worker, which takes over; the page then reloads into
// it.
// ROOT: the path before /work/ where this worker was found ("" straight, "/build" through
// tools/caddy's proxy); the coordinator's list names the page's files under /work/.
const VERSION = "__VERSION__";
const ROOT = self.location.pathname.slice(0, self.location.pathname.lastIndexOf("/work/"));
const PAGE = __PAGE__.map((p) => ROOT + p);
const SHELL = `shell-${VERSION}`;
const WAIT_MS = 4000;
const AWAY_MS = 60000;

self.addEventListener("install", (e) => {
  self.skipWaiting();
  // (A file that doesn't come, the Mac away, is kept the next time the page asks for it.)
  e.waitUntil((async () => {
    const cache = await caches.open(SHELL);
    await Promise.all(PAGE.map(async (p) => {
      const r = await fetch(p, { cache: "no-store" }).catch(() => null);
      if (r && r.ok) await put(cache, p, r);
    }));
  })());
});

self.addEventListener("activate", (e) => {
  e.waitUntil((async () => {
    for (const k of await caches.keys()) {
      // (And the programs' WebAssembly an older page kept to help with the build: "progs".)
      if ((k.startsWith("shell-") && k !== SHELL) || k === "progs") await caches.delete(k);
    }
    await self.clients.claim();
  })());
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== self.location.origin) return;
  const p = url.pathname === `${ROOT}/work` ? `${ROOT}/work/` : url.pathname;
  if (PAGE.includes(p)) e.respondWith(page(e, p));
});

let awayUntil = 0;

// One of the page's files: the lead's when it answers well in time (kept), else as kept; with
// nothing kept, the Mac's answer, however late.
async function page(e, key) {
  const cache = await caches.open(SHELL);
  // (Kept is this version's set as it installed: a fresher answer, maybe a newer version's file,
  // is never written over it, so the files used while the Mac is away are one version's, never a
  // mix; a file missing from the install is kept as it first comes.)
  const kept = await cache.match(key);
  const net = fetch(e.request).then(async (r) => {
    if (r.ok) {
      awayUntil = 0;
      if (!kept) await put(cache, key, r.clone());
    }
    return r;
  });
  if (!kept) return net;
  e.waitUntil(net.catch(() => {}));
  if (Date.now() < awayUntil) return kept;
  const late = new Promise((done) => setTimeout(() => done(null), WAIT_MS));
  const r = await Promise.race([net.catch(() => null), late]);
  if (r && r.ok) return r;
  awayUntil = Date.now() + AWAY_MS;
  return kept;
}

// A cache write that may fail (the device's storage full) without failing the answer.
async function put(cache, key, r) {
  try {
    await cache.put(key, r);
  } catch {
    /* kept next time, if there's room */
  }
}
