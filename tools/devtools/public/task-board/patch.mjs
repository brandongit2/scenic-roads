// The task board's list brought up to date in place (#1428): what a render built is put into the
// list element by element, never by emptying it. A row (or other element) whose key and content are
// as they were stays the very node it was; one whose content changed keeps its node and takes the
// new attributes and children; only what is new is added, only what went is removed, and only the
// nodes out of order are moved (the rest, a longest run already in order, stay where they are).
//
// So the browser keeps what it knows of each row: a row out of view is `content-visibility: auto`,
// sized from when it was last laid out (`contain-intrinsic-size: auto`), which a new node doesn't
// have (it would be the 30 px guess, and every row above the view a guess moves the list); scroll
// anchoring keeps its hold on the row in view; a hovered row stays hovered. (Emptying the list and
// filling it again would make every row new: the list would move on each render, and a click or a
// write renders several times as its reads land.)

// Each node's key: a row its item (`data-id`), a history its item's (`data-for`), a heading its
// group (`data-grp`), a "+ Task" line its folder's (`data-folder`), anything else its tag and class;
// the nth of the same key its own (`key#n`).
export function keysOf(nodes) {
  const seen = new Map();
  return nodes.map((e) => {
    const d = e.nodeType === 1 ? e.dataset : null;
    const k = !d ? "#text"
      : d.id !== undefined ? `row:${d.id}`
      : d.for !== undefined ? `hist:${d.for}`
      : d.grp !== undefined ? `grp:${d.grp}`
      : d.folder !== undefined ? `add:${d.folder}`
      : `${e.tagName}.${e.className}`;
    const n = (seen.get(k) || 0) + 1;
    seen.set(k, n);
    return `${k}#${n}`;
  });
}

// Which new node takes which old one: `from[i]`, the index among the old nodes of the one with new
// node i's key (-1: none, it is new); `stay`, the new indexes whose old node stays where it is (the
// longest run of them already in order), every other one being moved or added.
export function placement(oldKeys, newKeys) {
  const at = new Map(oldKeys.map((k, i) => [k, i]));
  const from = newKeys.map((k) => at.get(k) ?? -1);
  // (The longest increasing run of `from`, its -1s left out: patience sorting, then back along `prev`.)
  const tails = [], prev = new Array(from.length);
  for (let i = 0; i < from.length; i++) {
    if (from[i] < 0) continue;
    let lo = 0, hi = tails.length;
    while (lo < hi) { const m = (lo + hi) >> 1; if (from[tails[m]] < from[i]) lo = m + 1; else hi = m; }
    prev[i] = lo ? tails[lo - 1] : -1;
    tails[lo] = i;
  }
  const stay = new Set();
  for (let i = tails.length ? tails[tails.length - 1] : -1; i >= 0; i = prev[i]) stay.add(i);
  return { from, stay };
}

// `host`'s children made `frag`'s (a DocumentFragment, consumed), keeping every node that can be kept.
export function patchChildren(host, frag) {
  const old = [...host.childNodes], next = [...frag.childNodes];
  const { from, stay } = placement(keysOf(old), keysOf(next));
  const used = new Set();
  const nodes = next.map((n, i) => {
    const cur = from[i] < 0 ? null : old[from[i]];
    if (!cur) return n;
    if (cur.isEqualNode(n)) { used.add(cur); return cur; }
    if (cur.nodeType !== 1 || cur.tagName !== n.tagName) { stay.delete(i); return n; }
    for (const a of [...cur.attributes]) if (!n.hasAttribute(a.name)) cur.removeAttribute(a.name);
    for (const a of n.attributes) if (cur.getAttribute(a.name) !== a.value) cur.setAttribute(a.name, a.value);
    cur.replaceChildren(...n.childNodes);
    used.add(cur);
    return cur;
  });
  for (const e of old) if (!used.has(e)) e.remove();
  // (From the end: each node not staying goes before the one after it, already in place.)
  let after = null;
  for (let i = nodes.length - 1; i >= 0; i--) {
    if (!stay.has(i) || nodes[i].parentNode !== host) host.insertBefore(nodes[i], after);
    after = nodes[i];
  }
}
