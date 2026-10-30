// The pool on the build page (docs/pool.md §10, §11): who leads and since when (the Mac mini, the
// fixed lead: nothing on the page moves it), each member with its state, and "no lead" when the lead
// is out of touch. From the agent's status the coordinator serves (`/work/swarm`'s agent: its
// `pool.lead`, crates/pipeline/src/agent/lead.rs View).

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

/** The pool's section, from the agent's status `agent` (null: the pool isn't on). */
export function poolSection(agent, now) {
  const v = agent?.pool?.lead;
  if (!v) return null;
  const lead = v.lead;
  const sub = v.no_lead ? `term ${v.term}: no lead` : lead ? `${lead.host} leads term ${lead.term} since ${clock(lead.since)}` : "";
  const cards = v.members.map((m) => {
    const [state, cls] = chipFor(m);
    const card = h("div", { class: `mc ${m.out_of_touch ? "off" : ""}` },
      h("div", "top", h("span", "name", m.host), h("span", "role", [m.me ? "serves this page" : null, m.app].filter(Boolean).join(" · ")), h("span", { class: `chip ${cls}` }, state),
        m.beat ? h("span", "right", `heard from ${ago(now, m.beat)}`) : null));
    if (m.leads && lead) card.append(h("div", "sub", `Term ${lead.term}: ${lead.how}`));
    return card;
  });
  const top = [];
  if (v.no_lead) top.push(h("div", "pool-ctl warnline", h("span", null, v.alert || `No lead: ${v.no_lead}`)));
  for (const u of v.unreached || []) top.push(h("div", "pool-ctl warnline", h("span", null, u)));
  if (v.change && now - v.change.at < 86400) top.push(h("div", "sub", `${v.change.said} (${clock(v.change.at)})`));
  return h("section", { class: "d-pool", id: "pool" }, h("h2", null, "The pool", h("span", null, sub)),
    h("div", "pool-top", top), h("div", "machines", cards));
}

/** A term's event in the history, in words. */
export function termText(e) {
  return [`${e.worker || ""}: ${e.note || ""}`, /stepped down|taken back|given up|dropped/.test(e.note || "") ? "warn" : "run"];
}
