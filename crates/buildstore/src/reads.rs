//! The manifest as a key reads it (docs/incremental-planning.md §4.1, task #880 step 3): what a
//! derivation reads of the build manifest (and of the planner's inputs), recorded by construction.
//!
//! A key's context (pipeline's `KeyCtx`) holds its manifest as a [`Reads`]: a view with no way to
//! the map but its reads, each recorded into the key being derived (`Recorder`, the plan's own:
//! its key memo's) while one is. Code that reads the manifest for a key takes a [`Manifest`]: the
//! map itself (a job's, a target list's) or the view; one that took the map whole doesn't compile
//! with the view, so none can read past it (`Manifest::whole`, the one way to the map, is recorded
//! as a read of all of it). What each key read ([`Graph`]) and which entries changed since
//! ([`Diff`]) tell which keys the change can't have moved ([`Graph::moved`]).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

/// The build manifest (logical → content), or the planner's inputs (name → digest).
pub type Map = BTreeMap<String, String>;

/// What reads a manifest by name: the map, or the recording view (`Reads`). Each read is the
/// map's own, and the view records it as what it is (an entry's content, its presence, the
/// entries under a prefix, ...): a change it can't see moves no key that read so.
pub trait Manifest<'a>: Copy {
    /// Entry `l`'s content (an absent one read too: a later one's adding finds the key).
    fn get(self, l: &str) -> Option<&'a String>;
    /// Whether entry `l` is there (its content not read).
    fn contains_key(self, l: &str) -> bool;
    /// The entries named under `prefix`, in order, with their contents.
    fn under(self, prefix: &str) -> Under<'a>;
    /// Whether some entry's content is `content`.
    fn holds(self, content: &str) -> bool;
    /// The entries whose name `f(name, false)` takes (a step's write set: its outputs read whole).
    fn matching(self, f: fn(&str, bool) -> bool) -> Matching<'a>;
    /// The map whole: what reads it so reads all of it.
    fn whole(self) -> &'a Map;
    /// A derivation met state the entries don't show (a terrain index that can't be read now:
    /// pipeline's `TerrainTiles`).
    fn met_load(self) {}
    /// Its reads as a view (unrecorded for the map).
    fn view(self) -> Reads<'a>;
}

/// The entries under a prefix (`Manifest::under`).
pub struct Under<'a> {
    it: std::collections::btree_map::Range<'a, String, String>,
    prefix: String,
}

impl<'a> Iterator for Under<'a> {
    type Item = (&'a String, &'a String);
    fn next(&mut self) -> Option<Self::Item> {
        self.it.next().filter(|(l, _)| l.starts_with(&self.prefix))
    }
}

fn under<'a>(m: &'a Map, prefix: &str) -> Under<'a> {
    Under { it: m.range(prefix.to_string()..), prefix: prefix.to_string() }
}

/// The entries a write set takes (`Manifest::matching`).
pub struct Matching<'a> {
    it: std::collections::btree_map::Iter<'a, String, String>,
    f: fn(&str, bool) -> bool,
}

impl<'a> Iterator for Matching<'a> {
    type Item = (&'a String, &'a String);
    fn next(&mut self) -> Option<Self::Item> {
        self.it.by_ref().find(|(l, _)| (self.f)(l, false))
    }
}

impl<'a> Manifest<'a> for &'a Map {
    fn get(self, l: &str) -> Option<&'a String> {
        BTreeMap::get(self, l)
    }
    fn contains_key(self, l: &str) -> bool {
        BTreeMap::contains_key(self, l)
    }
    fn under(self, prefix: &str) -> Under<'a> {
        under(self, prefix)
    }
    fn holds(self, content: &str) -> bool {
        self.values().any(|v| v == content)
    }
    fn matching(self, f: fn(&str, bool) -> bool) -> Matching<'a> {
        Matching { it: self.iter(), f }
    }
    fn whole(self) -> &'a Map {
        self
    }
    fn view(self) -> Reads<'a> {
        Reads::plain(self)
    }
}

/// Which map a read is of: the manifest, or the planner's inputs (`KeyCtx::inputs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Space {
    Manifest,
    Inputs,
}

/// A map read through, recording into `rec` while a key is derived (`Recorder::begin`).
#[derive(Clone, Copy)]
pub struct Reads<'a> {
    map: &'a Map,
    rec: Option<&'a Recorder>,
    space: Space,
}

impl<'a> Reads<'a> {
    /// `map`'s reads, recorded nowhere.
    pub fn plain(map: &'a Map) -> Reads<'a> {
        Reads { map, rec: None, space: Space::Manifest }
    }

    /// This view of the planner's inputs (`Space::Inputs`).
    pub fn inputs(self) -> Reads<'a> {
        Reads { space: Space::Inputs, ..self }
    }

    /// This view recording into `rec`.
    pub fn recording(self, rec: &'a Recorder) -> Reads<'a> {
        Reads { rec: Some(rec), ..self }
    }

    /// The map, recording nothing: for what is never a key's read (the context's stamp; a cache
    /// filled on other threads, whose answers each key finds by what it reads itself).
    pub fn unrecorded_map(self) -> &'a Map {
        self.map
    }

    fn note(self, kind: Kind, name: &str) {
        if let Some(r) = self.rec {
            r.note(kind.of(self.space), name);
        }
    }
}

impl<'a> Manifest<'a> for Reads<'a> {
    fn get(self, l: &str) -> Option<&'a String> {
        self.note(Kind::Entry, l);
        self.map.get(l)
    }
    fn contains_key(self, l: &str) -> bool {
        self.note(Kind::Has, l);
        self.map.contains_key(l)
    }
    fn under(self, prefix: &str) -> Under<'a> {
        self.note(Kind::Under, prefix);
        under(self.map, prefix)
    }
    fn holds(self, content: &str) -> bool {
        self.note(Kind::Holds, content);
        self.map.values().any(|v| v == content)
    }
    fn matching(self, f: fn(&str, bool) -> bool) -> Matching<'a> {
        if let Some(r) = self.rec {
            r.note_matching(self.space, f);
        }
        Matching { it: self.map.iter(), f }
    }
    fn whole(self) -> &'a Map {
        self.note(Kind::All, "");
        self.map
    }
    fn met_load(self) {
        self.note(Kind::Load, "");
    }
    fn view(self) -> Reads<'a> {
        self
    }
}

/// What a read is of, a read id's top bits (`Kind::of`): an entry's content, its presence, the
/// entries under a prefix, a content held by some entry, a write set's entries, the map whole;
/// `Load`, state the entries don't show. The manifest's kinds, then the inputs' (`INPUTS` on).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Entry = 0,
    Has = 1,
    Under = 2,
    Holds = 3,
    Matching = 4,
    All = 5,
    Load = 6,
}

const KIND_SHIFT: u32 = 60;
const INPUTS: u64 = 8;
const ID: u64 = (1 << KIND_SHIFT) - 1;

impl Kind {
    fn of(self, space: Space) -> u64 {
        (self as u64 + if space == Space::Inputs { INPUTS } else { 0 }) << KIND_SHIFT
    }
}

/// A name as a read holds it: its hash, in a read's low bits (`ID`). Names aren't kept: the first
/// plan after a start reads 1.7 M of them (absent ones among them: each z6 tile's 256 deep mids
/// looked up), 226 MB as text; two names of one hash (one in 2^60) only move a key needlessly.
fn hashed(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish() & ID
}

/// The reads of the keys derived with one memo (pipeline's `KeyMemo`): one node per key, its own
/// reads and the keys read inside it (a key derived, or found derived, while it was). Not `Sync`:
/// the plan's own, on its thread.
#[derive(Default)]
pub struct Recorder(RefCell<Rec>);

#[derive(Default)]
struct Rec {
    /// The write sets read whole, by index (`Kind::Matching`).
    preds: Vec<fn(&str, bool) -> bool>,
    /// The derivations under way, innermost last.
    stack: Vec<Node>,
    nodes: Vec<Node>,
    /// The keys kept from a plan before (`Recorder::after`): their graph, and each node let go or
    /// not. The nodes recorded now are numbered after its.
    base: Option<(Graph, Vec<bool>)>,
}

/// A key's reads (`Kind` on top, then the name's hash: `hashed`), and the nodes of the keys read
/// inside it.
#[derive(Default, Clone)]
struct Node {
    reads: Vec<u64>,
    kids: Vec<u32>,
}

impl Recorder {
    fn note(&self, kind: u64, name: &str) {
        if let Some(n) = self.0.borrow_mut().stack.last_mut() {
            n.reads.push(kind | hashed(name));
        }
    }

    fn note_matching(&self, space: Space, f: fn(&str, bool) -> bool) {
        let mut r = self.0.borrow_mut();
        if r.stack.is_empty() {
            return;
        }
        let i = match r.preds.iter().position(|&g| std::ptr::fn_addr_eq(g, f)) {
            Some(i) => i,
            None => {
                r.preds.push(f);
                r.preds.len() - 1
            }
        } as u64;
        r.stack.last_mut().unwrap().reads.push(Kind::Matching.of(space) | i);
    }

    /// A key's derivation starts: what is read until its `end` is its.
    pub fn begin(&self) {
        self.0.borrow_mut().stack.push(Node::default());
    }

    /// The derivation `begin` started ends: its node, a kid of the one it was derived in.
    pub fn end(&self) -> u32 {
        let mut r = self.0.borrow_mut();
        let mut n = r.stack.pop().expect("a derivation begun");
        n.reads.sort_unstable();
        n.reads.dedup();
        n.reads.shrink_to_fit();
        n.kids.sort_unstable();
        n.kids.dedup();
        let id = (r.base.as_ref().map_or(0, |(g, _)| g.nodes) + r.nodes.len()) as u32;
        r.nodes.push(n);
        if let Some(p) = r.stack.last_mut() {
            p.kids.push(id);
        }
        id
    }

    /// A key derived before (node `id`) found while another is being derived: read inside it.
    pub fn found(&self, id: u32) {
        if let Some(p) = self.0.borrow_mut().stack.last_mut() {
            p.kids.push(id);
        }
    }

    /// The reads recorded, with their reverse index (the tests'; `renumbered` with a kept graph).
    #[cfg(test)]
    fn graph(self) -> Graph {
        self.renumbered().0
    }

    /// A recorder going on from the keys of graph `g` (theplan's shadow plan,
    /// docs/incremental-planning.md step 4): its nodes kept but those `gone` names
    /// (`Graph::moved`), each found by its id as before (`found`); the nodes recorded now numbered
    /// after them.
    pub fn after(g: Graph, gone: Vec<bool>) -> Recorder {
        assert_eq!(g.nodes, gone.len(), "each node let go or kept");
        Recorder(RefCell::new(Rec { preds: g.preds.clone(), base: Some((g, gone)), ..Default::default() }))
    }

    /// The first node id it records under: the number of nodes of the graph it went on from
    /// (`after`), let go or not.
    pub fn first(&self) -> u32 {
        self.0.borrow().base.as_ref().map_or(0, |(g, _)| g.nodes as u32)
    }

    /// The reads recorded and those kept (`after`), with their reverse index, the nodes numbered
    /// anew without those let go; and each node's id as it was (the kept graph's, then those
    /// recorded) to its new one (`u32::MAX`: let go).
    pub fn renumbered(self) -> (Graph, Vec<u32>) {
        Graph::of(self.0.into_inner())
    }
}

/// What each key of a plan read (`Recorder::renumbered`), as the reverse index alone: (read, node) by
/// read, (kid, reader) by kid. A node's own reads aren't kept apart (the diff asks only who read
/// what changed).
#[derive(Clone)]
pub struct Graph {
    preds: Vec<fn(&str, bool) -> bool>,
    nodes: usize,
    by_read: Vec<(u64, u32)>,
    parents: Vec<(u32, u32)>,
    /// The bytes held while it was built (`of`: the nodes' reads and the index beside them).
    peak: usize,
}

/// How big a plan's read sets are (`Graph::size`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub nodes: usize,
    /// The reads told apart (a name read by many keys, once).
    pub distinct: usize,
    pub reads: usize,
    pub edges: usize,
    /// The bytes held: the reverse index and the edges.
    pub bytes: usize,
    /// The bytes held at most while it was built (`Recorder::renumbered`).
    pub peak: usize,
}

impl Graph {
    fn of(r: Rec) -> (Graph, Vec<u32>) {
        assert!(r.stack.is_empty(), "a derivation left unended");
        let (mut by_read, mut parents, kept, gone) = match r.base {
            Some((g, gone)) => (g.by_read, g.parents, g.nodes, gone),
            None => (Vec::new(), Vec::new(), 0, Vec::new()),
        };
        let mut renumber = vec![u32::MAX; kept + r.nodes.len()];
        let mut nodes = 0u32;
        for (i, n) in renumber.iter_mut().enumerate() {
            if i >= kept || !gone[i] {
                *n = nodes;
                nodes += 1;
            }
        }
        // (The kept graph's pairs naming a node let go dropped, the rest renumbered, in place.)
        by_read.retain_mut(|(_, n)| {
            *n = renumber[*n as usize];
            *n != u32::MAX
        });
        parents.retain_mut(|(k, p)| {
            (*k, *p) = (renumber[*k as usize], renumber[*p as usize]);
            *k != u32::MAX && *p != u32::MAX
        });
        let n: usize = r.nodes.iter().map(|n| n.reads.len()).sum();
        let held: usize = std::mem::size_of::<Node>() * r.nodes.capacity()
            + r.nodes.iter().map(|n| 8 * n.reads.capacity() + 4 * n.kids.capacity()).sum::<usize>();
        by_read.reserve(n);
        for (i, n) in r.nodes.into_iter().enumerate() {
            let id = renumber[kept + i];
            by_read.extend(n.reads.iter().map(|&x| (x, id)));
            // (A key found inside it is one its memo holds: never one let go.)
            parents.extend(n.kids.iter().map(|&k| (renumber[k as usize], id)));
        }
        assert!(parents.iter().all(|&(k, _)| k != u32::MAX), "a key let go found inside one recorded");
        let peak = held + 16 * by_read.capacity() + 8 * parents.capacity();
        by_read.sort_unstable();
        by_read.shrink_to_fit();
        parents.sort_unstable();
        parents.shrink_to_fit();
        (Graph { preds: r.preds, nodes: nodes as usize, by_read, parents, peak }, renumber)
    }

    /// Its size and the bytes it holds.
    pub fn size(&self) -> Size {
        let distinct = self.by_read.chunk_by(|a, b| a.0 == b.0).count();
        Size {
            nodes: self.nodes,
            distinct,
            reads: self.by_read.len(),
            edges: self.parents.len(),
            bytes: 16 * self.by_read.capacity() + 8 * self.parents.capacity(),
            peak: self.peak,
        }
    }

    /// The nodes whose reads a read is among (the reverse index).
    fn readers(&self, read: u64) -> impl Iterator<Item = u32> + '_ {
        let i = self.by_read.partition_point(|&(r, _)| r < read);
        self.by_read[i..].iter().take_while(move |&&(r, _)| r == read).map(|&(_, n)| n)
    }

    /// Which nodes `d` moves: those that read something it changed, those `also` names (keys that
    /// came out unknown: never kept), and those that read a node it moves (a key found inside
    /// them). Each node's, by node id.
    pub fn moved(&self, d: &Diff, also: &[u32]) -> Vec<bool> {
        let n = self.nodes;
        let mut moved = vec![false; n];
        let mut todo: Vec<u32> = Vec::new();
        for &i in also {
            if !moved[i as usize] {
                moved[i as usize] = true;
                todo.push(i);
            }
        }
        let hit = |read: u64, moved: &mut Vec<bool>, todo: &mut Vec<u32>| {
            for i in self.readers(read) {
                if !moved[i as usize] {
                    moved[i as usize] = true;
                    todo.push(i);
                }
            }
        };
        let any = |k: Kind, s: Space| {
            let b = k.of(s);
            let i = self.by_read.partition_point(|&(r, _)| r < b);
            self.by_read.get(i).is_some_and(|&(r, _)| r & !ID == b)
        };
        // (The map whole and the load are read under the empty name.)
        let blank = hashed("");
        for (space, changes) in [(Space::Manifest, &d.manifest), (Space::Inputs, &d.inputs)] {
            if changes.names.is_empty() {
                continue;
            }
            hit(Kind::All.of(space) | blank, &mut moved, &mut todo);
            let under = any(Kind::Under, space);
            for (name, presence) in &changes.names {
                let h = hashed(name);
                hit(Kind::Entry.of(space) | h, &mut moved, &mut todo);
                if *presence {
                    hit(Kind::Has.of(space) | h, &mut moved, &mut todo);
                }
                if under {
                    for (i, _) in name.char_indices().chain(std::iter::once((name.len(), ' '))) {
                        hit(Kind::Under.of(space) | hashed(&name[..i]), &mut moved, &mut todo);
                    }
                }
                for (p, f) in self.preds.iter().enumerate() {
                    if f(name, false) {
                        hit(Kind::Matching.of(space) | p as u64, &mut moved, &mut todo);
                    }
                }
            }
            if any(Kind::Holds, space) {
                for c in &changes.contents {
                    hit(Kind::Holds.of(space) | hashed(c), &mut moved, &mut todo);
                }
            }
        }
        if d.load {
            for s in [Space::Manifest, Space::Inputs] {
                hit(Kind::Load.of(s) | blank, &mut moved, &mut todo);
            }
        }
        // (Up the kids' edges: a node reading a moved one moves.)
        while let Some(k) = todo.pop() {
            let i = self.parents.partition_point(|&(c, _)| c < k);
            for &(_, p) in self.parents[i..].iter().take_while(|&&(c, _)| c == k) {
                if !moved[p as usize] {
                    moved[p as usize] = true;
                    todo.push(p);
                }
            }
        }
        moved
    }
}

/// What changed in one map between two plans (`Diff::of`): each name added, removed or whose
/// content changed (with whether it was added or removed), and the contents gone or come.
#[derive(Default, Debug)]
pub struct Changes {
    pub names: Vec<(String, bool)>,
    pub contents: BTreeSet<String>,
}

impl Changes {
    /// `old` against `new`.
    pub fn of(old: &Map, new: &Map) -> Changes {
        let mut c = Changes::default();
        let (mut a, mut b) = (old.iter().peekable(), new.iter().peekable());
        loop {
            match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some((l, v)), None) => {
                    c.added_or_removed(l, v);
                    a.next();
                }
                (None, Some((l, v))) => {
                    c.added_or_removed(l, v);
                    b.next();
                }
                (Some((la, va)), Some((lb, vb))) => match la.cmp(lb) {
                    std::cmp::Ordering::Less => {
                        c.added_or_removed(la, va);
                        a.next();
                    }
                    std::cmp::Ordering::Greater => {
                        c.added_or_removed(lb, vb);
                        b.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if va != vb {
                            c.names.push((la.to_string(), false));
                            c.contents.insert(va.to_string());
                            c.contents.insert(vb.to_string());
                        }
                        a.next();
                        b.next();
                    }
                },
            }
        }
        c
    }

    fn added_or_removed(&mut self, l: &str, v: &str) {
        self.names.push((l.to_string(), true));
        self.contents.insert(v.to_string());
    }
}

/// What changed between two plans' contexts as their keys read them: the manifest's entries, the
/// planner's inputs, and whether the state the entries don't show did (`load`).
#[derive(Default, Debug)]
pub struct Diff {
    pub manifest: Changes,
    pub inputs: Changes,
    pub load: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kinds of read as `reads_of` names them, in `Kind`'s order.
    const KINDS: [&str; 7] = ["entry", "has", "under", "holds", "matching", "all", "load"];

    /// Read `r` as `<kind> <hash>` (`in ` before the inputs').
    fn said(r: u64) -> String {
        let (k, id) = (r >> KIND_SHIFT, r & ID);
        let space = if k >= INPUTS { "in " } else { "" };
        format!("{space}{} {id:015x}", KINDS[(k % INPUTS) as usize])
    }

    /// A manifest read of `name` of kind `kind` (`KINDS`'), as `reads_of` says it.
    fn read(kind: &str, name: &str) -> String {
        let k = KINDS.iter().position(|&x| x == kind).expect("a kind of read") as u64;
        said((k << KIND_SHIFT) | hashed(name))
    }

    /// Node `n`'s reads in `g`, as `<kind> <hash>` (`said`), from the reverse index.
    fn reads_of(g: &Graph, n: u32) -> Vec<String> {
        g.by_read.iter().filter(|&&(_, i)| i == n).map(|&(r, _)| said(r)).collect()
    }

    fn map(es: &[(&str, &str)]) -> Map {
        es.iter().map(|(l, c)| (l.to_string(), c.to_string())).collect()
    }

    fn writes_b(l: &str, _: bool) -> bool {
        l.starts_with("b/")
    }

    /// Each kind of read is moved by the changes it sees and by no other: an entry's content by
    /// its change, adding or removal; its presence only by adding or removal; a prefix by any
    /// entry under it; a write set by any of its entries; a content held by its coming or going;
    /// a key found inside another moves that one too.
    #[test]
    fn each_read_is_moved_by_what_it_sees() {
        let m = map(&[("a/1", "x"), ("a/2", "y"), ("b/1", "z"), ("c", "w")]);
        let rec = Recorder::default();
        let v = Reads::plain(&m).recording(&rec);
        let key = |f: &dyn Fn()| {
            rec.begin();
            f();
            rec.end()
        };
        let entry = key(&|| {
            let _ = v.get("a/1");
        });
        let absent = key(&|| {
            let _ = v.get("a/3");
        });
        let has = key(&|| {
            let _ = v.contains_key("a/1");
        });
        let prefix = key(&|| {
            let _ = v.under("a/").count();
        });
        let set = key(&|| {
            let _ = v.matching(writes_b).count();
        });
        let held = key(&|| {
            let _ = v.holds("w");
        });
        let outer = {
            rec.begin();
            rec.found(entry);
            rec.end()
        };
        let none = key(&|| ());
        // (Nothing read outside a derivation.)
        v.get("c");
        let g = rec.graph();
        assert_eq!(reads_of(&g, entry), [read("entry", "a/1")]);
        assert!(reads_of(&g, none).is_empty());
        let moved = |new: &Map| {
            let d = Diff { manifest: Changes::of(&m, new), ..Default::default() };
            let mv = g.moved(&d, &[]);
            [entry, absent, has, prefix, set, held, outer, none].map(|n| mv[n as usize])
        };
        let mut n = m.clone();
        n.insert("a/1".into(), "x2".into());
        assert_eq!(moved(&n), [true, false, false, true, false, false, true, false], "a/1 changed");
        let mut n = m.clone();
        n.insert("a/3".into(), "v".into());
        assert_eq!(moved(&n), [false, true, false, true, false, false, false, false], "a/3 added");
        let mut n = m.clone();
        n.remove("a/1");
        assert_eq!(moved(&n), [true, false, true, true, false, false, true, false], "a/1 removed");
        let mut n = m.clone();
        n.insert("b/2".into(), "v".into());
        assert_eq!(moved(&n), [false, false, false, false, true, false, false, false], "b/2 added");
        let mut n = m.clone();
        n.insert("c".into(), "w2".into());
        assert_eq!(moved(&n), [false, false, false, false, false, true, false, false], "c's w gone");
        assert_eq!(moved(&m), [false; 8], "nothing changed");
    }

    /// The inputs' reads are of their own space: a manifest entry of the same name moves none.
    #[test]
    fn the_inputs_reads_are_apart_from_the_manifests() {
        let (m, i) = (map(&[("k", "1")]), map(&[("k", "1")]));
        let rec = Recorder::default();
        let iv = Reads::plain(&i).inputs().recording(&rec);
        rec.begin();
        iv.get("k");
        let n = rec.end() as usize;
        let g = rec.graph();
        assert_eq!(reads_of(&g, n as u32), [format!("in {}", read("entry", "k"))]);
        let changed = map(&[("k", "2")]);
        let d = Diff { manifest: Changes::of(&m, &changed), ..Default::default() };
        assert!(!g.moved(&d, &[])[n]);
        let d = Diff { inputs: Changes::of(&i, &changed), ..Default::default() };
        assert!(g.moved(&d, &[])[n]);
    }

    /// The map whole and the load state are reads too: any change moves the first, a change of
    /// the load the second.
    #[test]
    fn the_whole_map_and_the_load_are_read() {
        let m = map(&[("a", "1")]);
        let rec = Recorder::default();
        let v = Reads::plain(&m).recording(&rec);
        rec.begin();
        v.whole();
        let whole = rec.end() as usize;
        rec.begin();
        v.met_load();
        let load = rec.end() as usize;
        let g = rec.graph();
        let d = Diff { manifest: Changes::of(&m, &map(&[("b", "1")])), ..Default::default() };
        assert_eq!((g.moved(&d, &[])[whole], g.moved(&d, &[])[load]), (true, false));
        let d = Diff { load: true, ..Default::default() };
        assert_eq!((g.moved(&d, &[])[whole], g.moved(&d, &[])[load]), (false, true));
    }

    /// A recorder going on from a kept graph (`Recorder::after`, step 4): the nodes let go are
    /// gone with their reads and edges, a node `also` names moves its readers, the kept ones read
    /// as before under their new ids, a kept key found inside a new one moves it, and the new
    /// nodes are numbered after the kept graph's.
    #[test]
    fn a_kept_graph_goes_on_without_what_moved() {
        let m = map(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let rec = Recorder::default();
        let v = Reads::plain(&m).recording(&rec);
        let key = |rec: &Recorder, f: &dyn Fn()| {
            rec.begin();
            f();
            rec.end()
        };
        let a = key(&rec, &|| {
            let _ = v.get("a");
        });
        let b = key(&rec, &|| {
            let _ = v.get("b");
        });
        rec.begin();
        rec.found(b);
        assert_eq!(rec.end(), 2, "the key b was found in");
        let g = rec.graph();
        assert_eq!(g.size().nodes, 3);
        // (b unknown: let go, its reader with it.)
        let gone = g.moved(&Diff::default(), &[b]);
        assert_eq!(gone, [false, true, true]);
        let rec = Recorder::after(g, gone);
        assert_eq!(rec.first(), 3);
        let v = Reads::plain(&m).recording(&rec);
        let c = key(&rec, &|| {
            let _ = v.get("c");
        });
        let reads_a = {
            rec.begin();
            rec.found(a);
            rec.end()
        };
        assert_eq!((c, reads_a), (3, 4));
        let (g, renumber) = rec.renumbered();
        assert_eq!(renumber, [0, u32::MAX, u32::MAX, 1, 2]);
        assert_eq!(g.size().nodes, 3);
        assert_eq!(reads_of(&g, 0), [read("entry", "a")]);
        assert_eq!(reads_of(&g, 1), [read("entry", "c")]);
        assert_eq!((g.size().reads, g.size().edges), (2, 1), "b's reads and its reader's edge let go");
        let changed = |l: &str| {
            let mut n = m.clone();
            n.insert(l.into(), "9".into());
            g.moved(&Diff { manifest: Changes::of(&m, &n), ..Default::default() }, &[])
        };
        assert_eq!(changed("a"), [true, false, true], "a's change moves a and the key it was found in");
        assert_eq!(changed("c"), [false, true, false]);
        assert_eq!(changed("b"), [false, false, false], "b let go: none reads it now");
    }
}
