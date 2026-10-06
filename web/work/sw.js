// The worker page's service worker (docs/workers.md, The page as an app). The page's files are kept
// as this version installs, and come from the build Mac whenever it answers well within WAIT_MS (a
// newly published app's page at once), else as kept: a Mac that doesn't (asleep, away, or its
// agent restarting: tailscale serve answers 502) is taken for away for AWAY_MS, what was kept used
// at once meanwhile. The programs' WebAssembly is kept by version (its address's ?v=), so the page
// starts again without fetching megabytes a program, and only each program's newest version is
// kept. Everything else (a task's files, every request to the coordinator) isn't touched.
//
// VERSION is the page's own (the coordinator's hash of its files) and PAGE their addresses (the
// coordinator's list): a new version is a new worker, which takes over; the page then reloads into
// it once no task is running.
const VERSION = "__VERSION__";
const PAGE = __PAGE__;
const SHELL = `shell-${VERSION}`;
const PROGS = "progs";
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
      if (k.startsWith("shell-") && k !== SHELL) await caches.delete(k);
    }
    await self.clients.claim();
  })());
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== self.location.origin) return;
  const p = url.pathname === "/work" ? "/work/" : url.pathname;
  if (p.startsWith("/work/prog/")) e.respondWith(program(e.request, url));
  else if (PAGE.includes(p)) e.respondWith(page(e, p));
});

let awayUntil = 0;

// One of the page's files: the build Mac's when it answers well in time (kept), else as kept; with
// nothing kept, the Mac's answer, however late.
async function page(e, key) {
  const cache = await caches.open(SHELL);
  const net = fetch(e.request).then(async (r) => {
    if (r.ok) {
      awayUntil = 0;
      await put(cache, key, r.clone());
    }
    return r;
  });
  const kept = await cache.match(key);
  if (!kept) return net;
  e.waitUntil(net.catch(() => {}));
  if (Date.now() < awayUntil) return kept;
  const late = new Promise((done) => setTimeout(() => done(null), WAIT_MS));
  const r = await Promise.race([net.catch(() => null), late]);
  if (r && r.ok) return r;
  awayUntil = Date.now() + AWAY_MS;
  return kept;
}

// A program's WebAssembly: as kept for its version, else fetched (with the device's key) and kept,
// its other versions dropped.
async function program(req, url) {
  const cache = await caches.open(PROGS);
  const kept = await cache.match(url.href);
  if (kept) return kept;
  const r = await fetch(req);
  if (r.ok) {
    try {
      for (const k of await cache.keys()) {
        const u = new URL(k.url);
        if (u.pathname === url.pathname && u.href !== url.href) await cache.delete(k);
      }
    } catch {
      /* the others go next time */
    }
    await put(cache, url.href, r.clone());
  }
  return r;
}

// A cache write that may fail (the device's storage full) without failing the answer.
async function put(cache, key, r) {
  try {
    await cache.put(key, r);
  } catch {
    /* kept next time, if there's room */
  }
}
