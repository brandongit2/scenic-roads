// The pool on the build page (docs/pool.md §10, §11): who leads and since when, each member with its
// state and "Make lead" (confirming), a handover's progress as it happens, and "Take it" when there's
// no lead in touch. From the agent's status the coordinator serves (`/work/swarm`'s agent: its
// `pool.lead`, crates/pipeline/src/agent/lead.rs View); the asks go to `/work/lead`, which the
// agent takes up within seconds and checks as its driver would.

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
const STAGES = { offered: "offered: waiting for it to answer (a minute at most)", settling: "settling: the lead grants nothing new and saves the records", passed: "passed: the next term names it; waiting for it to take up (two minutes at most)" };

function chipFor(m) {
  if (m.leads) return ["leads", "run"];
  if (/out of touch|no heartbeat/.test(m.state)) return [m.state, "bad"];
  if (/battery|away|clock|too old|stood down|unknown/.test(m.state)) return [m.state, "warn"];
  return [m.state, "ok"];
}

/** The pool's section, from the agent's status `agent` (null: the pool isn't on); `ask(body)` posts
 * to `/work/lead` (resolves when it's taken). */
export function poolSection(agent, now, ask) {
  const v = agent?.pool?.lead;
  if (!v) return null;
  const lead = v.lead;
  const sub = v.no_lead ? `term ${v.term}: no lead` : lead ? `${lead.host} leads term ${lead.term} since ${clock(lead.since)}` : "";
  const cards = v.members.map((m) => {
    const [state, cls] = chipFor(m);
    const card = h("div", { class: `mc ${m.out_of_touch ? "off" : ""}` },
      h("div", "top", h("span", "name", m.host), h("span", "role", [m.me ? "serves this page" : null, m.app].filter(Boolean).join(" · ")), h("span", { class: `chip ${cls}` }, state),
        m.beat ? h("span", "right", `heard from ${ago(now, m.beat)}`) : null));
    if (m.leads && v.handing) {
      card.append(h("div", "sub hand", `Handing over to ${v.handing.host}: ${STAGES[v.handing.stage] || v.handing.stage}, since ${clock(v.handing.since)}`));
    } else if (m.leads && lead) {
      card.append(h("div", "sub", `Term ${lead.term}: ${lead.how}`));
    }
    if (!m.leads && !v.no_lead) {
      const go = () => {
        const warn = m.away ? `\n\n${m.host} is away from home: the build's duties run slowly over Tailscale until it's back.` : "";
        if (confirm(`Make ${m.host} the build's lead?\n\nThe lead hands over between jobs' saves: nothing running stops, and it takes a minute or two.${warn}`)) ask({ to: m.member });
      };
      card.append(h("div", "pool-ctl",
        h("button", { disabled: !m.can_lead || !!v.handing, title: m.can_lead ? "" : m.why_not || "", onclick: go }, "Make lead"),
        !m.can_lead && m.why_not ? h("span", "sub", m.why_not) : null));
    }
    return card;
  });
  const top = [];
  if (v.no_lead) {
    const t = v.takeover || {};
    const can = !t.refused && !t.force && !t.downgrade;
    const take = () => confirm(`Take the build over on the Mac serving this page?\n\nNo lead: ${v.no_lead}.\n\nIt makes the next term naming itself and leads from the records on the NAS; nothing built is lost.`) && ask({ take: true });
    top.push(h("div", "pool-ctl warnline", h("span", null, `No lead: ${v.no_lead}`),
      h("button", { class: "primary", disabled: !can, title: can ? "" : t.refused || `needs the owner's ${t.force ? "force" : "downgrade"} (the Mac's menu, or scenic lead take): ${t.force || t.downgrade}`, onclick: take }, "Take it")));
  }
  if (v.offer) {
    top.push(h("div", "pool-ctl", h("span", null, `${v.offer.why}; ${v.offer.host} is home on power${v.auto ? " (handed over by itself after five minutes)" : ""}`),
      h("button", { onclick: () => confirm(`Hand the build to ${v.offer.host}?`) && ask({ to: v.offer.to }) }, `Hand it to ${v.offer.host}`)));
  }
  if (v.asked) top.push(h("div", "sub", `The last ask (${v.asked.by}, ${ago(now, v.asked.at)}): ${v.asked.state} — ${v.asked.said}`));
  if (v.change && now - v.change.at < 86400) top.push(h("div", "sub", `${v.change.said} (${clock(v.change.at)})`));
  return h("section", { class: "d-pool", id: "pool" }, h("h2", null, "The pool", h("span", null, sub)),
    h("div", "pool-top", top), h("div", "machines", cards));
}

/** A term's event in the history, in words. */
export function termText(e) {
  return [`${e.worker || ""}: ${e.note || ""}`, /stepped down|taken back|given up|dropped/.test(e.note || "") ? "warn" : "run"];
}
