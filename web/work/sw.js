// The worker page's service worker (docs/workers.md, The page as an app). The page and its scripts
// come from the build Mac whenever it answers (a newly published app's page at once), and from what
// was kept when it doesn't; the programs' WebAssembly is kept by version (its address's ?v=), so the
// page starts again without fetching megabytes a program, and only each program's newest version
// is kept. A task's files and every request to the coordinator pass straight through.
//
// VERSION is the page's own (the coordinator's hash of its files): a new one is a new worker, which
// takes over; the page then reloads into it once no task is running.
const VERSION = "__VERSION__";
const SHELL = `shell-${VERSION}`;
const PROGS = "progs";

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (e) => {
  e.waitUntil((async () => {
    for (const k of await caches.keys()) {
      if (k.startsWith("shell-") && k !== SHELL) await caches.delete(k);
    }
    await self.clients.claim();
  })());
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== self.location.origin || !url.pathname.startsWith("/work/")) return;
  if (url.pathname.startsWith("/work/prog/")) {
    e.respondWith(program(e.request, url));
  } else if (!url.pathname.startsWith("/work/in/") && !url.pathname.startsWith("/work/out/")) {
    e.respondWith(page(e.request));
  }
});

// The page's files: the build Mac's, else as kept.
async function page(req) {
  const cache = await caches.open(SHELL);
  try {
    const r = await fetch(req);
    if (r.ok) await cache.put(req.url.split("#")[0], r.clone());
    return r;
  } catch (err) {
    const kept = await cache.match(req.url.split("#")[0], { ignoreSearch: true });
    if (kept) return kept;
    throw err;
  }
}

// A program's WebAssembly: as kept for its version, else fetched (with the page's token) and kept,
// its other versions dropped.
async function program(req, url) {
  const cache = await caches.open(PROGS);
  const kept = await cache.match(url.href);
  if (kept) return kept;
  const r = await fetch(req);
  if (r.ok) {
    for (const k of await cache.keys()) {
      const u = new URL(k.url);
      if (u.pathname === url.pathname && u.href !== url.href) await cache.delete(k);
    }
    await cache.put(url.href, r.clone());
  }
  return r;
}
