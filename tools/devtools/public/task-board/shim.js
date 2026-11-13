// Gives the page the document-database API it was written against (the Claude artifact runtime's
// `window.claude.use("db" | "user" | "assets")`) over this app's HTTP API and event stream.
(() => {
  "use strict";
  const err = (code, message) => Object.assign(new Error(message || code), { code, message: message || code });
  // Counts the writes answered so far: a read that began before the latest answer may predate that
  // write, and showing it would put the old state back over the change (the page's own layer over
  // the store ends when the write is answered).
  let answered = 0;
  async function call(method, url, body, headers) {
    if (method !== "GET") return call1(method, url, body, headers).finally(() => { answered++; });
    return call1(method, url, body, headers);
  }
  async function call1(method, url, body, headers) {
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
    return { docs, size: docs.length, total: j.total, empty: !docs.length, metadata: META, forEach: (f) => docs.forEach(f) };
  };

  // One event stream, shared: it tells which collection changed, and each listener reads again (a read
  // that a write was answered during is dropped and made again: it may predate the write). A listener's
  // first read waits for the stream to open, so no change between the two is missed and none needs
  // reading twice (up to 2 s: a stream slower than that, and what was read is read again once it opens).
  const listeners = new Set(); // {col, run}
  let ready = null;
  function stream() {
    if (ready) return ready;
    let open = false, late = false;
    const connect = (opened) => {
      const es = new EventSource("../api/stream");
      es.addEventListener("change", (e) => { const col = JSON.parse(e.data).col; for (const l of listeners) if (l.col === col) l.run(); });
      es.onopen = () => { opened(); if (open || late) for (const l of listeners) l.run(); open = true; }; // (after a drop, everything is read again)
      // (A stream refused, as a proxy's 502 while the server restarts, is closed for good: opened anew.)
      es.onerror = () => { if (es.readyState === EventSource.CLOSED) setTimeout(() => connect(() => {}), 2000); };
    };
    ready = new Promise((r) => { setTimeout(() => { late = true; r(); }, 2000); connect(r); });
    return ready;
  }
  // A subscription: `read` run now and on each change of `col`. One that fails ends, as Firestore's do
  // (the page subscribes again: kept, each retry would add a listener reading on every change).
  function listen(col, read, wrap, onNext, onError) {
    const opened = stream();
    let busy = false, again = false, dead = false;
    const stop = () => { dead = true; listeners.delete(l); };
    const run = async () => {
      if (dead) return;
      if (busy) { again = true; return; }
      busy = true;
      const was = answered;
      try {
        const j = await read();
        if (answered !== was) again = true;
        else if (!dead) onNext(wrap(j));
      } catch (e) { if (!dead) { stop(); onError && onError(e); } }
      busy = false;
      if (again && !dead) { again = false; run(); }
    };
    const l = { col, run };
    listeners.add(l);
    opened.then(run);
    stop.run = run; // (read again now)
    return stop;
  }

  function docRef(col, id) {
    const url = `../api/c/${encodeURIComponent(col)}/${encodeURIComponent(id)}`;
    const ref = {
      id,
      get: async () => docSnap(id, await call("GET", url)),
      update: (patch) => call("PATCH", url, patch), // (the server's reply: the item's id, num, order, added)
      delete: () => call("DELETE", url).then(() => {}),
      restore: () => call("POST", url + "/restore", {}), // (what was just deleted, back as it was)
      onSnapshot: (next, error) => listen(col, () => call("GET", url), (j) => docSnap(id, j), next, error),
    };
    return ref;
  }
  function query(col, opts) {
    const qs = () => { const p = new URLSearchParams(); if (opts.orderBy) { p.set("orderBy", opts.orderBy); p.set("dir", opts.dir || "asc"); } if (opts.limit) p.set("limit", opts.limit); return `../api/c/${encodeURIComponent(col)}?${p}`; };
    return {
      orderBy: (orderBy, dir) => query(col, { ...opts, orderBy, dir }),
      limit: (limit) => query(col, { ...opts, limit }),
      get: async () => querySnap(await call("GET", qs())),
      onSnapshot: (next, error) => listen(col, () => call("GET", qs()), querySnap, next, error),
    };
  }
  // A subscription to a read of the page's own: `read(get)` is run (get(path): the answer of GET ../api/<path>)
  // when it starts and whenever `col` changes; `refresh()` runs it again at once (what it reads has changed).
  const get = (path) => call("GET", `../api/${path}`);
  const watch = (col, read) => {
    let cur = null;
    return { onSnapshot: (next, error) => (cur = listen(col, () => read(get), (j) => j, next, error)), refresh: () => cur && cur.run() };
  };
  const db = {
    get,
    watch,
    // add(content): the server makes the doc (its id, and an item's num, order and added time) and says them.
    collection: (col) => Object.assign(query(col, {}), { doc: (id) => docRef(col, id), add: (data) => call("POST", `../api/c/${encodeURIComponent(col)}`, data) }),
    doc: (path) => { const [col, id] = path.split("/"); return docRef(col, id); },
  };
  const assets = {
    upload: async (file) => {
      const r = await call("POST", "../api/blob", file, { "content-type": file.type || "application/octet-stream" });
      return { id: r.id };
    },
  };
  const user = { can: async () => true };
  window.claude = { use: async (name) => ({ db, user, assets }[name] || null) };
})();
