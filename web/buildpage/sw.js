// A one-off cleanup (task #1117, docs/coordinator.md §7, The page's version): the build page had a
// service worker here; this one ends it. A device that still has it asks for this file on its next
// update check (the page loaded, or the old page's own check, open for days: back in front, half
// hourly), takes it over the old, which drops the files the old one kept (`shell-<version>`: the
// map's "shell" through Caddy is another name), unregisters, and reloads the pages it held from the
// coordinator (the old page also reloads by itself on the change). No page registers it.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) =>
  e.waitUntil(
    (async () => {
      for (const k of await caches.keys()) if (k.startsWith("shell-")) await caches.delete(k);
      await self.registration.unregister();
      for (const c of await self.clients.matchAll({ type: "window" })) c.navigate(c.url).catch(() => {});
    })(),
  ),
);
