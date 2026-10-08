// The map app's service worker on another device (an iPhone, an iPad: main.ts registers it there,
// not on the Macs, which have the data themselves). What it keeps (docs/plan.md §4, Devices):
// - the page, from the Mac whenever it answers in time and well, else as kept;
// - the app's scripts and styles (their names hashed), all of them as soon as a page is kept: the
//   ones it names and the ones those name in turn (the map's workers), so the next start needs
//   none from the Mac; those the newest page no longer names go. And the fonts and the icons;
// - the map's data whose address carries its version (`?v=`) and that the Mac says never changes
//   (`immutable`: a version that's still current), as the map asks for it, so what was looked at
//   stays to look at again without the Mac (the last KEEP files kept); the 3D buildings' tiles in a
//   cache of their own (the last KEEP_BLD), so a city's buildings don't crowd out its roads and
//   terrain;
// - the catalog's metadata, from the Mac whenever it answers, else as kept, so the map opens.
// Everything else (searches, the build's status, any write) goes straight through. A Mac that
// doesn't answer within WAIT_MS (asleep, or away) is taken for away for AWAY_MS: what was kept is
// used at once meanwhile, and what the Mac sends later still replaces it. A refusal (403: not from
// here, or not the map's address) is never hidden behind what was kept.
const SHELL = "shell";
const DATA = "data";
const BLD = "bld";
const KEEP = 12000;
// (A city's z14 building tiles are 50–300 KB, a view ~20 of them: a few hundred MB at most.)
const KEEP_BLD = 2000;
const WAIT_MS = 4000;
const AWAY_MS = 60000;

self.addEventListener("install", (e) => {
  self.skipWaiting();
  // (A failure here, the Mac away, leaves the cache to fill as the page is used.)
  e.waitUntil(keepPage().catch(() => {}));
});
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

self.addEventListener("fetch", (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== self.location.origin) return;
  const p = url.pathname;
  // (The app's page alone: another address opened in a tab, its JSON say, isn't kept as the page.)
  if (p === "/" || p === "/index.html") {
    e.respondWith(fresh(e, req, "/"));
  } else if (p === "/api/meta" || p === "/api/catalog") {
    e.respondWith(fresh(e, req, url.href));
  } else if (p.startsWith("/assets/") || p.startsWith("/fonts/") || p.startsWith("/icons/") || p === "/manifest.webmanifest") {
    e.respondWith(kept(req, SHELL));
  } else if (url.searchParams.has("v") && p.startsWith("/tiles/buildings/")) {
    e.respondWith(kept(req, BLD));
  } else if (url.searchParams.has("v") && (p.startsWith("/tiles/") || p.startsWith("/api/"))) {
    e.respondWith(kept(req, DATA));
  }
});

let awayUntil = 0;

// The Mac's answer when it gives a good one in time (kept under `key`), else what was kept (the
// page's for a page asked at a view's address); with nothing kept, the Mac's answer, however late.
async function fresh(e, req, key) {
  const cache = await caches.open(SHELL);
  const net = fetch(req).then(async (r) => {
    if (r.ok && (key !== "/" || r.headers.get("content-type")?.startsWith("text/html"))) {
      awayUntil = 0;
      await put(cache, key, r.clone());
      if (key === "/") e.waitUntil(r.clone().text().then((html) => keepNamed(cache, html)).catch(() => {}));
    }
    return r;
  });
  const k = await cache.match(key);
  if (!k) return net;
  e.waitUntil(net.catch(() => {}));
  if (Date.now() < awayUntil) return k;
  const late = new Promise((done) => setTimeout(() => done(null), WAIT_MS));
  const r = await Promise.race([net.catch(() => null), late]);
  if (r && (r.ok || r.status === 403)) return r;
  // (Away, asleep or restarting: tailscale serve answers 502 for a server that's down.)
  awayUntil = Date.now() + AWAY_MS;
  return k;
}

// What was kept, else the Mac's answer, kept when it's whole and may be (the map's data: when the
// Mac says it never changes; a version no longer current is revalidated, not kept under its old
// address).
const puts = {};
async function kept(req, name) {
  const cache = await caches.open(name);
  const k = await cache.match(req.url);
  if (k) return k;
  const r = await fetch(req);
  const cc = r.headers.get("cache-control") ?? "";
  if (r.status === 200 && !cc.includes("no-store") && (name === SHELL || cc.includes("immutable"))) {
    await put(cache, req.url, r.clone());
    // (Trimmed with this worker's first file of each, and every 500 after: a worker doesn't live
    // long.)
    if (name !== SHELL && (puts[name] = (puts[name] ?? 0) + 1) % 500 === 1) trim(cache, name === BLD ? KEEP_BLD : KEEP);
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

// The page and all it needs, kept now (an install's: the page's own first load came before this
// worker, so nothing of it was kept).
async function keepPage() {
  const cache = await caches.open(SHELL);
  const r = await fetch("/", { cache: "no-store" });
  if (!r.ok) return;
  const html = await r.clone().text();
  await put(cache, "/", r);
  // (The metadata too: the page's first load asked for it before this worker was there.)
  for (const u of ["/api/meta", "/api/catalog", "/manifest.webmanifest", "/icons/apple-touch-icon.png", "/icons/icon-192.png"]) {
    const a = await fetch(u).catch(() => null);
    if (a && a.ok) await put(cache, u, a);
  }
  await keepNamed(cache, html);
}

// The scripts and styles a page names (hashed, /assets/…) and those its scripts name in turn,
// kept; the ones it no longer names dropped.
async function keepNamed(cache, html) {
  const names = new Set(refs(html));
  if (!names.size) return;
  const todo = [...names];
  while (todo.length) {
    const u = todo.pop();
    let r = await cache.match(u);
    if (!r) {
      r = await fetch(u).catch(() => null);
      if (!r || !r.ok) continue;
      await put(cache, u, r.clone());
    }
    if (u.endsWith(".js") || u.endsWith(".css")) {
      for (const m of refs(await r.text(), u)) {
        if (!names.has(m)) {
          names.add(m);
          todo.push(m);
        }
      }
    }
  }
  for (const k of await cache.keys()) {
    const p = new URL(k.url).pathname;
    if (p.startsWith("/assets/") && !names.has(p)) await cache.delete(k);
  }
}

// The app's own files a page or a script names (not their source maps): as /assets/…, assets/…
// (from the page's folder) or ./… (beside the script naming it, in /assets/).
const refs = (text, from = "/") =>
  [...text.matchAll(/(?:^|[^\w./-])((?:\/|\.\/)?(?:assets\/)?[\w.-]+?\.(?:js|css|wasm|woff2?|png|svg))(?![\w.])/g)]
    .map((m) => m[1])
    .filter((n) => n.startsWith("/assets/") || n.startsWith("assets/") || (n.startsWith("./") && from.startsWith("/assets/")))
    .map((n) => (n.startsWith("./") ? `/assets/${n.slice(2)}` : n.startsWith("assets/") ? `/${n}` : n));

// The oldest of the map's data kept go once there are more than `keep` files (a cache keeps them in
// the order they came).
async function trim(cache, keep) {
  const keys = await cache.keys();
  for (const k of keys.slice(0, Math.max(0, keys.length - keep))) await cache.delete(k);
}
