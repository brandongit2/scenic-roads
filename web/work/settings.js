// The owner's settings on the build page (the Machines tab; docs/workers.md §7): this page's Mac's
// in-use share, build memory and disk room target, and the pool's NAS reserve. Read and set through
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

// The settings as last read (null: not yet; `error`: why not), and when.
let said = null, error = null, at = 0, asking = false;
const EVERY_MS = 30_000;

/** Reads them again when 30 s old (or `force`); `call(path, body)` this page's own Mac; `done()` after. */
export async function loadSettings(call, done, force = false) {
  if (asking || (!force && Date.now() - at < EVERY_MS)) return;
  asking = true;
  try {
    const [code, v] = await call("/work/settings", {});
    if (code === 200) { said = v; error = null; } else error = v?.error || `answered ${code}`;
    at = Date.now();
  } catch (e) { error = e.message; } finally { asking = false; }
  done();
}

/** The settings' section; `call` and `done` as loadSettings's; `onError(text)` a refused ask. */
export function settingsSection(call, done, onError) {
  const set = async (body, what) => {
    try {
      const [code, v] = await call("/work/settings", body);
      if (code !== 200) throw new Error(v?.error || `answered ${code}`);
      said = v; at = Date.now();
    } catch (e) { onError?.(`couldn't set ${what}: ${e.message}`); }
    // (A focused field keeps its section as it was, dash.js render: let go, so the new value shows.)
    document.activeElement?.blur();
    done();
  };
  const row = (key, what, now, note, input) => {
    const field = h("input", { type: "number", min: input.min, max: input.max, step: input.step, placeholder: input.placeholder, "aria-label": what });
    const go = () => field.value !== "" && set({ [key]: field.value }, what);
    field.addEventListener("keydown", (e) => e.key === "Enter" && go());
    return h("div", "setting",
      h("span", "k", what), h("b", null, now), h("span", "dim small", note),
      h("span", "ctl", field, h("button", { onclick: go }, "Set"),
        h("button", { class: "quiet", title: input.offTitle, onclick: () => set({ [key]: "off" }, what) }, "Off")));
  };
  const s = said;
  const sub = s ? `on ${s.mac}; the NAS reserve, pool-wide` : error ? `can't be read: ${error}` : "reading…";
  const rows = !s ? [] : [
    row("in_use", "In-use share", `${s.in_use.cores} of ${s.in_use.of} cores`,
      `kept free of the build's jobs while the Mac is used${s.in_use.set ? by(s.in_use) : ` (the default, ${s.in_use.default})`}`,
      { min: 0, max: s.in_use.of - 1, step: 1, placeholder: "cores", offTitle: "Back to the default" }),
    row("memory", "Build memory", gb(s.memory.mb * 2 ** 20),
      `for the build's jobs together, of ${gb(s.memory.total_mb * 2 ** 20)}${s.memory.set ? by(s.memory) : " (the memory guard's limit)"}`,
      { min: 1, max: Math.floor(s.memory.total_mb / 1024) || null, step: 0.5, placeholder: "GB", offTitle: "The memory guard's limit alone" }),
    row("room", "Disk room target", s.room.bytes ? gb(s.room.bytes) : "off",
      `free space kept on this Mac's disk${s.room.free != null ? `, ${gb(s.room.free)} free now` : ""}${by(s.room)}`,
      { min: 1, max: s.room.disk ? Math.floor(s.room.disk / GiB) - 1 : null, step: 1, placeholder: "GB", offTitle: "No target" }),
    s.nas_room ? row("nas_room", "NAS reserve", s.nas_room.bytes ? gb(s.nas_room.bytes) : "off",
      `kept free on the NAS past what the build writes there${s.nas_room.free != null ? `, ${gb(s.nas_room.free)} free now` : ""}${by(s.nas_room)}`,
      { min: 1, step: 1, placeholder: "GB", offTitle: "No reserve" })
      : h("div", "setting dim small", "NAS reserve: the NAS is away from this Mac now"),
  ];
  return h("section", "d-settings", h("h2", null, "Settings", h("span", null, sub)), h("div", "box settings", rows));
}
