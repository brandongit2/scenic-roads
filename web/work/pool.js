// The pool on the build page (docs/pool.md §10, §11): who leads and since when (the Mac mini, the
// fixed lead: nothing on the page moves it), each member with its state, and "no lead" with what to
// do when the lead is out of touch. From the agent's status the coordinator serves (`/work/swarm`'s
// agent: its `pool.lead`, crates/pipeline/src/agent/lead.rs View). Each member a Pause/Resume of
// its own (task #873): the hold ask (`POST /work/hold`, docs/pool.md §11) of this page's agent,
// another member's by its id (written on the NAS as that member's hold, taken up when it reads it:
// so one out of touch has it queued), and its hold's stage as its own status serves it
// (`Status::hold`, control::HoldView: asked, pausing with its jobs named, paused, resuming).

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

const ago = (now, t) => {
  const s = Math.max(0, now - t);
  return s < 90 ? `${s} s ago` : s < 5400 ? `${Math.round(s / 60)} min ago` : s < 129600 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} days ago`;
};
const clock = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });

function chipFor(m) {
  if (m.leads) return ["leads", "run"];
  if (/out of touch|no heartbeat/.test(m.state)) return [m.state, "bad"];
  if (/battery|away|clock|too old|unknown/.test(m.state)) return [m.state, "warn"];
  return [m.state, "ok"];
}

/** A member's Pause/Resume, from its status's hold (`hold`: control::HoldView, null: building),
 * whether it's in touch (`fresh`) and an ask of this page's under way (`asking`: true while it's
 * posted, then its answer's words, until the status's stage changes): its label, the hold it asks
 * (`held`), whether it's disabled and its tooltip. Pure (web/test/poolhold.test.mjs). */
export function holdControl(m, hold, fresh, asking) {
  const name = m.me ? "this Mac" : m.host;
  if (asking === true) return { label: hold?.pause ? "Resuming…" : "Pausing…", held: null, disabled: true, title: `Asked of ${name}'s agent` };
  if (hold?.stage === "resuming") return { label: "Resuming…", held: null, disabled: true, title: hold.said };
  if (hold?.stage === "asked") return { label: hold.pause ? "Pausing…" : "Resuming…", held: null, disabled: true, title: hold.said };
  const held = !hold?.pause;
  const what = held ? `Pause ${name} alone: its jobs finish what they're on and it takes no new work; the other Macs go on` : `Let ${name} take work again`;
  // (Out of touch: the ask is written on the NAS as its hold, and taken up once it's back.)
  const away = !fresh && !m.me ? `\n${m.host} is out of touch: this is kept on the NAS and taken up when it's back` : "";
  return { label: held ? "Pause" : "Resume", held, disabled: false, title: what + away };
}

/** The pool's section, from the agent's status `agent` (null: the pool isn't on). `macs`: each
 * Mac's status by host (`{status, fresh}`, the Machines cards'); `asks`: the asks under way by
 * member id; `ask(m, held)`: posts one. */
export function poolSection(agent, now, macs = new Map(), asks = new Map(), ask = null) {
  const v = agent?.pool?.lead;
  if (!v) return null;
  const lead = v.lead;
  const sub = v.no_lead ? `term ${v.term}: no lead` : lead ? `${lead.host} leads term ${lead.term} since ${clock(lead.since)}` : "";
  const cards = v.members.map((m) => {
    const [state, cls] = chipFor(m);
    const x = macs.get(m.host), hold = x?.status?.hold || null;
    const card = h("div", { class: `mc ${m.out_of_touch ? "off" : ""}` },
      h("div", "top", h("span", "name", m.host), h("span", "role", [m.me ? "serves this page" : null, m.app].filter(Boolean).join(" · ")), h("span", { class: `chip ${cls}` }, state),
        hold && hold.stage !== m.state ? h("span", { class: "chip warn", title: hold.said }, hold.stage) : null,
        m.beat ? h("span", "right", `heard from ${ago(now, m.beat)}`) : null));
    if (m.leads && lead) card.append(h("div", "sub", `Term ${lead.term}: ${lead.how}`));
    if (ask) {
      const c = holdControl(m, hold, x ? x.fresh : !m.out_of_touch, asks.get(m.member));
      const said = typeof asks.get(m.member) === "string" ? asks.get(m.member) : hold?.said;
      card.append(h("div", "pool-ctl",
        h("button", { class: c.held === false ? "primary" : "", disabled: c.disabled, title: c.title, onclick: () => ask(m, c.held) }, c.label),
        said ? h("span", "sub", said) : null));
    }
    return card;
  });
  const top = [];
  if (v.no_lead) top.push(h("div", "pool-ctl warnline", h("span", null, v.alert || `No lead: ${v.no_lead}`)));
  for (const u of v.unreached || []) top.push(h("div", "pool-ctl warnline", h("span", null, u)));
  if (v.change && now - v.change.at < 86400) top.push(h("div", "sub", `${v.change.said} (${clock(v.change.at)})`));
  return h("section", "d-pool", h("h2", null, "The pool", h("span", null, sub)),
    h("div", "pool-top", top), h("div", "machines", cards));
}

/** A term's event in the history, in words. */
export function termText(e) {
  return [`${e.worker || ""}: ${e.note || ""}`, /stepped down|out of touch/.test(e.note || "") ? "warn" : "run"];
}
