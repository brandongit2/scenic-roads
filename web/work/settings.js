// The owner's settings on the build page (the Machines tab; docs/workers.md §7): this page's Mac's
// in-use share, build memory and disk room target, a block per Mac, each set on that Mac, and the
// pool's NAS reserve. Read and set through
// `/work/settings`, which sets each through the same function as its `scenic` command
// (`scenic in-use`, `memory`, `room`, `nas-room`). Changed only when a person presses Set or Off.

function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  if (typeof attrs === "string") e.className = attrs;
  else if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) e.append(k instanceof Node ? k : document.createTextNode(String(k)));
  return e;
}

const GiB = 2 ** 30;
const gb = (b) => `${(b / GiB).toFixed(1)} GB`;
const by = (s) => (s?.by ? ` · set by ${s.by}` : "");

// Each Mac's settings as last read, by its host ("" this page's own before its first answer):
// `said` (null: not yet), `error`, `at`, `asking`, `url` (the address that answered last).
const kept = new Map();
let ownHost = null;
const EVERY_MS = 30_000;

// Asks a Mac's `/work/settings`: this page's own through `call`, another at its addresses (as
// live.js's /work/live: HTTPS over an HTTPS page; its broker answers a member's page, CORS).
async function ask(call, m, st, body) {
  if (!m) {
    const [code, v] = await call("/work/settings", body);
    if (code !== 200) throw new Error(v?.error || `answered ${code}`);
    return v;
  }
  const https = location.protocol === "https:";
  const urls = https ? [m.https].filter(Boolean) : [...(m.urls || [])];
  if (!urls.length) throw new Error(https ? "no HTTPS address (this page came over HTTPS)" : "no address");
  if (st.url && urls.includes(st.url)) urls.sort((a, b) => (a === st.url ? -1 : b === st.url ? 1 : 0));
  let last;
  for (const u of urls) {
    let r;
    try {
      r = await fetch(`${u.replace(/\/$/, "")}/work/settings`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal: AbortSignal.timeout ? AbortSignal.timeout(5000) : undefined });
    } catch (e) { last = e; continue; }
    st.url = u;
    const v = await r.json().catch(() => null);
    if (!r.ok) throw new Error(r.status === 404 ? "its app is older (no settings here)" : v?.error || `answered ${r.status}`);
    return v;
  }
  throw last.name === "TimeoutError" ? new Error("no answer in 5 s") : last.message === "Failed to fetch" || last.message === "Load failed" ? new Error("doesn't answer") : last;
}

/** Reads every Mac's again when 30 s old: `call(path, body)` this page's own Mac, `macs` the
 * swarm's (host, urls, https: /work/swarm's `macs`); `done()` after each answer. */
export function loadSettings(call, macs, done) {
  const one = async (m) => {
    const host = m ? m.host : ownHost || "";
    const st = kept.get(host) || {};
    kept.set(host, st);
    if (st.asking || Date.now() - (st.at || 0) < EVERY_MS) return;
    st.asking = true;
    try {
      const v = await ask(call, m, st, {});
      if (!m && !ownHost) { ownHost = v.mac; kept.delete(""); kept.set(ownHost, st); }
      st.said = v; st.error = null;
    } catch (e) { st.error = e.message; } finally { st.asking = false; st.at = Date.now(); }
    done();
  };
  one(null);
  for (const m of macs) if (m.host !== ownHost) one(m);
}

/** The settings' section: a block per Mac (this page's first), each control setting that Mac's
 * own, and the pool's NAS reserve once; `call`, `macs` and `done` as loadSettings's;
 * `onError(text)` a refused ask. */
export function settingsSection(call, macs, done, onError) {
  const set = async (m, st, body, what) => {
    try {
      st.said = await ask(call, m, st, body); st.at = Date.now();
    } catch (e) { onError?.(`couldn't set ${what}: ${e.message}`); }
    // (A focused field keeps its section as it was, dash.js render: let go, so the new value shows.)
    document.activeElement?.blur();
    done();
  };
  const row = (put, key, what, now, note, input) => {
    const field = h("input", { type: "number", min: input.min, max: input.max, step: input.step, placeholder: input.placeholder, "aria-label": what });
    const go = () => field.value !== "" && put({ [key]: field.value }, what);
    field.addEventListener("keydown", (e) => e.key === "Enter" && go());
    return h("div", "setting",
      h("span", "k", what), h("b", null, now), h("span", "dim small", note),
      h("span", "ctl", field, h("button", { onclick: go }, "Set"),
        h("button", { class: "quiet", title: input.offTitle, onclick: () => put({ [key]: "off" }, what) }, "Off")));
  };
  const others = macs.filter((m) => m.host !== ownHost);
  const list = [[null, ownHost || ""], ...others.map((m) => [m, m.host])];
  const blocks = list.map(([m, host]) => {
    const st = kept.get(host) || {};
    const s = st.said;
    const put = (body, what) => set(m, st, body, `${what} on ${s?.mac || host}`);
    const name = s?.mac || host || "this page's Mac";
    const head = h("h3", null, name, h("span", "dim small", m ? "" : " (this page's Mac)"));
    if (!s) return h("div", "box settings", head, h("div", "setting dim small", st.error ? `can't be read: ${st.error}` : "reading…"));
    return h("div", "box settings", head,
      row(put, "in_use", "In-use share", `${s.in_use.cores} of ${s.in_use.of} cores`,
        `kept free of the build's jobs while the Mac is used${s.in_use.set ? by(s.in_use) : ` (the default, ${s.in_use.default})`}`,
        { min: 0, max: s.in_use.of - 1, step: 1, placeholder: "cores", offTitle: "Back to the default" }),
      row(put, "memory", "Build memory", gb(s.memory.mb * 2 ** 20),
        `for the build's jobs together, of ${gb(s.memory.total_mb * 2 ** 20)}${s.memory.set ? by(s.memory) : " (the memory guard's limit)"}`,
        { min: 1, max: Math.floor(s.memory.total_mb / 1024) || null, step: 0.5, placeholder: "GB", offTitle: "The memory guard's limit alone" }),
      row(put, "room", "Disk room target", s.room.bytes ? gb(s.room.bytes) : "off",
        `free space kept on ${s.mac}'s disk${s.room.free != null ? `, ${gb(s.room.free)} free now` : ""}${by(s.room)}`,
        { min: 1, max: s.room.disk ? Math.floor(s.room.disk / GiB) - 1 : null, step: 1, placeholder: "GB", offTitle: "No target" }));
  });
  // (The NAS reserve, the pool's: once, through the first Mac that reaches the NAS.)
  const nas = list.map(([m, host]) => [m, kept.get(host) || {}]).find(([, st]) => st.said?.nas_room);
  const pool = nas
    ? (() => {
      const [m, st] = nas, n = st.said.nas_room;
      return h("div", "box settings", h("h3", null, "The pool"),
        row((body, what) => set(m, st, body, what), "nas_room", "NAS reserve", n.bytes ? gb(n.bytes) : "off",
          `kept free on the NAS past what the build writes there${n.free != null ? `, ${gb(n.free)} free now` : ""}${by(n)}`,
          { min: 1, step: 1, placeholder: "GB", offTitle: "No reserve" }));
    })()
    : h("div", "box settings", h("h3", null, "The pool"), h("div", "setting dim small", "NAS reserve: no Mac shown here reaches the NAS now"));
  return h("section", "d-settings", h("h2", null, "Settings", h("span", null, "each Mac's own, set on that Mac; the NAS reserve, pool-wide")), blocks, pool);
}
