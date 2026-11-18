// The task board's list brought up to date in place (#1428, public/task-board/patch.mjs): a node
// that didn't change stays the same node, one that changed keeps its node, and only what is out of
// order moves. Run: node --test tools/devtools/test/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { keysOf, placement, patchChildren } from "../public/task-board/patch.mjs";

// Just enough of the DOM for patchChildren: elements with attributes and children, text, and a
// count of the insertions and removals made on a parent.
class El {
  constructor(tag, attrs = {}, kids = []) {
    this.nodeType = 1; this.tagName = tag.toUpperCase(); this.parentNode = null; this.childNodes = []; this.ops = 0;
    this.attributes = Object.entries(attrs).map(([name, value]) => ({ name, value: String(value) }));
    for (const k of kids) this.insertBefore(typeof k === "string" ? new Text(k) : k, null);
  }
  get className() { return this.getAttribute("class") || ""; }
  get dataset() { return Object.fromEntries(this.attributes.filter((a) => a.name.startsWith("data-")).map((a) => [a.name.slice(5), a.value])); }
  hasAttribute(n) { return this.attributes.some((a) => a.name === n); }
  getAttribute(n) { return this.attributes.find((a) => a.name === n)?.value ?? null; }
  setAttribute(n, v) { const a = this.attributes.find((x) => x.name === n); if (a) a.value = String(v); else this.attributes.push({ name: n, value: String(v) }); }
  removeAttribute(n) { this.attributes = this.attributes.filter((a) => a.name !== n); }
  isEqualNode(o) {
    return o.nodeType === 1 && o.tagName === this.tagName && o.attributes.length === this.attributes.length
      && this.attributes.every((a) => o.getAttribute(a.name) === a.value)
      && o.childNodes.length === this.childNodes.length && this.childNodes.every((c, i) => c.isEqualNode(o.childNodes[i]));
  }
  insertBefore(n, ref) {
    if (n.parentNode) n.parentNode.childNodes.splice(n.parentNode.childNodes.indexOf(n), 1);
    const i = ref ? this.childNodes.indexOf(ref) : this.childNodes.length;
    this.childNodes.splice(i, 0, n);
    n.parentNode = this;
    this.ops++;
    return n;
  }
  replaceChildren(...ns) { for (const c of this.childNodes) c.parentNode = null; this.childNodes = []; for (const n of ns) this.insertBefore(n, null); }
  remove() { if (this.parentNode) { this.parentNode.ops++; this.parentNode.childNodes.splice(this.parentNode.childNodes.indexOf(this), 1); this.parentNode = null; } }
}
class Text {
  constructor(s) { this.nodeType = 3; this.data = s; this.parentNode = null; }
  isEqualNode(o) { return o.nodeType === 3 && o.data === this.data; }
  remove() { El.prototype.remove.call(this); }
}
const row = (id, title = `task ${id}`, cls = "row task") => new El("div", { class: cls, "data-id": id }, [new El("span", { class: "ttl" }, [title])]);
const frag = (nodes) => new El("frag", {}, nodes);
const list = (nodes) => { const h = new El("div", { id: "list" }, nodes); h.ops = 0; return h; };
const ids = (host) => host.childNodes.map((n) => n.dataset?.id ?? n.dataset?.grp ?? n.dataset?.for);

test("each node's key: a row its item, a history, a heading or an add line theirs, else its tag and class; the nth of one its own", () => {
  const ns = [row("i1"), new El("div", { class: "hist-list", "data-for": "i1" }), new El("div", { class: "grp", "data-grp": "none" }), new El("div", { class: "addrow", "data-folder": "f1" }),
    new El("p", { class: "empty" }), new El("p", { class: "empty" }), new Text("x"), new El("div", { class: "hist-list", "data-for": "" })];
  assert.deepEqual(keysOf(ns), ["row:i1#1", "hist:i1#1", "grp:none#1", "add:f1#1", "P.empty#1", "P.empty#2", "#text#1", "hist:#1"]);
});

test("placement: the old node of each new key, and the longest run already in order stays", () => {
  assert.deepEqual(placement(["a", "b", "c"], ["a", "b", "c"]), { from: [0, 1, 2], stay: new Set([0, 1, 2]) });
  // One moved later: only it is out of the run.
  const later = placement(["a", "b", "c", "d", "e"], ["a", "c", "d", "e", "b"]);
  assert.deepEqual(later.from, [0, 2, 3, 4, 1]);
  assert.deepEqual([...later.stay].sort(), [0, 1, 2, 3]);
  // One moved earlier, one new, one gone.
  const p = placement(["a", "b", "c", "d"], ["d", "a", "x", "b"]);
  assert.deepEqual(p.from, [3, 0, -1, 1]);
  assert.deepEqual([...p.stay].sort(), [1, 3]);
  assert.deepEqual(placement([], ["a"]), { from: [-1], stay: new Set() });
});

test("patchChildren: an unchanged row stays the same node; a changed one keeps its node and takes the new content; nothing else is touched", () => {
  const a = row("i1"), b = row("i2"), c = row("i3", "old title");
  const host = list([a, b, c]);
  const next = [row("i1"), row("i2", "task i2", "row task sel"), row("i3", "new title")];
  patchChildren(host, frag(next));
  assert.equal(host.childNodes[0], a);
  assert.equal(host.childNodes[1], b);
  assert.equal(host.childNodes[2], c);
  assert.equal(b.getAttribute("class"), "row task sel");
  assert.equal(c.childNodes[0].childNodes[0].data, "new title");
  assert.equal(host.ops, 0, "no row added, moved or removed");
});

test("patchChildren: a row moved far down is the one node moved, the rows it passed stay put; new ones added, gone ones removed", () => {
  const rows = Array.from({ length: 50 }, (_, i) => row(`i${i}`));
  const host = list([...rows]);
  // i3 goes to the end (a priority dragged down, or set elsewhere).
  patchChildren(host, frag([...rows.filter((_, i) => i !== 3).map((r) => row(r.dataset.id)), row("i3")]));
  assert.deepEqual(ids(host), [...rows.filter((_, i) => i !== 3), rows[3]].map((r) => r.dataset.id));
  assert.ok(host.childNodes.every((n, i) => n === [...rows.filter((_, j) => j !== 3), rows[3]][i]), "every row is the node it was");
  assert.equal(host.ops, 1, "one move");
  // i10 archived (gone), a heading and a new row added before i20.
  host.ops = 0;
  const want = host.childNodes.map((r) => r.dataset.id).filter((id) => id !== "i10");
  const at = want.indexOf("i20");
  want.splice(at, 0, "grp", "i99");
  patchChildren(host, frag(want.map((id) => (id === "grp" ? new El("div", { class: "grp", "data-grp": "none" }, ["No priority"]) : row(id)))));
  assert.deepEqual(ids(host), want.map((id) => (id === "grp" ? "none" : id)));
  assert.equal(host.ops, 3, "one removal, two additions");
});

test("patchChildren: a node whose key is kept but whose tag changed is replaced", () => {
  const host = list([new El("p", { class: "empty" }, ["Loading"])]);
  patchChildren(host, frag([row("i1"), row("i2")]));
  assert.deepEqual(ids(host), ["i1", "i2"]);
  const keep = host.childNodes[0];
  patchChildren(host, frag([new El("section", { "data-id": "i1" }), row("i2")]));
  assert.notEqual(host.childNodes[0], keep);
  assert.equal(host.childNodes[0].tagName, "SECTION");
  assert.equal(keep.parentNode, null);
});
