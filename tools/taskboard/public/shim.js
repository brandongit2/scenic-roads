// Gives the page the document-database API it was written against (the Claude artifact runtime's
// `window.claude.use("db" | "user" | "assets")`) over this app's HTTP API and event stream.
(() => {
  "use strict";
  const err = (code, message) => Object.assign(new Error(message || code), { code, message: message || code });
  async function call(method, url, body, headers) {
    let r;
    try {
      r = await fetch(url, { method, headers: body !== undefined && !headers ? { "content-type": "application/json" } : headers, body: body === undefined ? undefined : headers ? body : JSON.stringify(body) });
    } catch { throw err("unavailable", "the server can't be reached"); }
    const j = await r.json().catch(() => ({}));
    if (!r.ok) throw err(j?.error?.code || "unavailable", j?.error?.message);
    return j;
  }
  const META = { fromCache: false, hasPendingWrites: false };
  const docSnap = (id, j) => ({ id, exists: !!j.exists, data: () => (j.exists ? j.data : undefined), metadata: META });
  const querySnap = (j) => {
    const docs = j.docs.map((d) => ({ id: d.id, exists: true, data: () => d.data, metadata: META }));
    return { docs, size: docs.length, empty: !docs.length, metadata: META, forEach: (f) => docs.forEach(f) };
  };

  // One event stream, shared: it tells which collection changed, and each listener reads again.
  const listeners = new Set(); // {col, run}
  let es = null;
  function stream() {
    if (es) return;
    es = new EventSource("/api/stream");
    es.addEventListener("change", (e) => { const col = JSON.parse(e.data).col; for (const l of listeners) if (l.col === col) l.run(); });
    es.onopen = () => { for (const l of listeners) l.run(); }; // (after a drop, everything is read again)
  }
  function listen(col, read, wrap, onNext, onError) {
    stream();
    let busy = false, again = false, dead = false;
    const run = async () => {
      if (busy) { again = true; return; }
      busy = true;
      try { onNext(wrap(await read())); } catch (e) { if (!dead) onError && onError(e); }
      busy = false;
      if (again && !dead) { again = false; run(); }
    };
    const l = { col, run };
    listeners.add(l);
    run();
    return () => { dead = true; listeners.delete(l); };
  }

  function docRef(col, id) {
    const url = `/api/c/${encodeURIComponent(col)}/${encodeURIComponent(id)}`;
    const ref = {
      id,
      get: async () => docSnap(id, await call("GET", url)),
      set: (data) => call("PUT", url, data).then(() => {}),
      update: (patch) => call("PATCH", url, patch).then(() => {}),
      delete: () => call("DELETE", url).then(() => {}),
      onSnapshot: (next, error) => listen(col, () => call("GET", url), (j) => docSnap(id, j), next, error),
    };
    if (col === "meta" && id === "counter") ref.acquire = ({ holder, ttlMs }) => call("POST", "/api/lease", { name: "counter", holder, ttlMs });
    return ref;
  }
  const rnd = () => { const a = "abcdefghijklmnopqrstuvwxyz0123456789"; return Array.from(crypto.getRandomValues(new Uint8Array(20)), (x) => a[x % a.length]).join(""); };
  function query(col, opts) {
    const qs = () => { const p = new URLSearchParams(); if (opts.orderBy) { p.set("orderBy", opts.orderBy); p.set("dir", opts.dir || "asc"); } if (opts.limit) p.set("limit", opts.limit); return `/api/c/${encodeURIComponent(col)}?${p}`; };
    return {
      orderBy: (orderBy, dir) => query(col, { ...opts, orderBy, dir }),
      limit: (limit) => query(col, { ...opts, limit }),
      get: async () => querySnap(await call("GET", qs())),
      onSnapshot: (next, error) => listen(col, () => call("GET", qs()), querySnap, next, error),
    };
  }
  const db = {
    collection: (col) => Object.assign(query(col, {}), { doc: (id) => docRef(col, id || rnd()) }),
    doc: (path) => { const [col, id] = path.split("/"); return docRef(col, id); },
  };
  const assets = {
    upload: async (file) => {
      const r = await call("POST", "/api/blob", file, { "content-type": file.type || "application/octet-stream" });
      return { id: r.id, url: r.url };
    },
  };
  const user = { can: async () => true };
  window.claude = { use: async (name) => ({ db, user, assets }[name] || null) };
})();
