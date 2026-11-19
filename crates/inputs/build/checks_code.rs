//! What each gate unit's checks are, as code (docs/inputs.md §4.3, task #1467): a digest of the code
//! a unit's checks compile to, which `inputs::check_key` and a unit's check state
//! (`Listed::derived_by`) name in place of a typed version. build.rs makes one for every module
//! `src/inputs/mod.rs` declares and one for the gate; the crate's tests compile this file in too
//! (lib.rs), and hold the typed versions left (the regions' `RESOLUTION`, elevation's `Step::rule`)
//! to the code they cover.
//!
//! **The code.** The inputs crate and every crate its manifest's dependencies reach by path (each
//! manifest's `[dependencies]` and `[target.….dependencies]`, in any form Cargo takes; a form this
//! can't read fails the build), each read whole as the app compiles it: each file with its child
//! modules (`mod x;`, `#[path]` too) and the bytes of every file it compiles in (`include_str!`,
//! `include_bytes!`, `include!`, by a plain string literal, or a build script's output by
//! `concat!(env!("OUT_DIR"), …)`, which counts as itself: anything else is an error, never passed
//! over). Left out: comments and doc comments, layout and what rustfmt changes (a trailing comma in
//! `[]`, `{}` or a `()` holding another comma, a match arm's body put in braces or taken out), and
//! what the app's build doesn't compile: items under a `cfg` false without tests and the `testkit`
//! feature, and `#[test]` functions.
//!
//! **What a unit's code is.** Each item has its own digest: a function, a type, a trait, a constant
//! or static, a `macro_rules!`, an item macro (`det!(f64, …)`; `thread_local!`'s are named by their
//! statics), an impl's header (its associated consts and types), each of its methods. A unit's code
//! is the items of its modules and every item they name, transitively, across modules and crates:
//! - a name (`check`, `THEMES`, a `{name}` in a string literal: a format string's), as its module
//!   places it: its own items, its `use`s and the item's own (a function's `use super::*`), its
//!   globs' modules' names (an enum's variants by `use Kind::*`: the enum);
//! - a path, from where its first segment is (`crate::`, `super::`, `self::`, `::store::`, a crate
//!   it depends on, a `use` name, a child module, an item of its module) to the item it names (a
//!   re-export followed), or the module whole where it names none there; a path in a string literal
//!   of an attribute (serde's `default = "one"`) too;
//! - a macro it invokes (`det!`): each `macro_rules!` of that name in its crate and theirs;
//! - a type's impls, wherever they are: an impl's header once its type is reached (and its trait,
//!   for a trait of these crates); a method of it once its header is and its name is called
//!   anywhere reached (`.m`, `Type::m`, `Self::m`, `<T as Tr>::m`), every method of an impl of a
//!   crates.io trait (`Display`, `Drop`, `Deserialize`: called by code not seen);
//! - when any item of a module is reached, its item macros, its traits and those its `use`s bring
//!   in (what method calls there may call: `use det::Det` and `x.dcos()`), and its globs' enums.
//!
//! A `use` names nothing itself: what a name it brings in stands for is taken where the name is
//! used.
//!
//! The gate's code is `gate`'s items and every item of `mod.rs`, with all they name. One cut: the
//! registry's items (`REGISTRY`) reach none of `mod.rs`'s child modules, else every unit's code would
//! hold every other's; any other item of `mod.rs` naming one fails the build.
//!
//! **What it doesn't see:** code of crates from crates.io (their versions: only through Cargo.lock,
//! not here), what their macros and derives expand to (serde's), the names in a file `include!`d
//! (its bytes count, its names aren't followed), a method of a value whose type no code reached
//! names (only inferred through code not seen), and what's read at run time (a unit's
//! `Checks::context` names those). A name it can't place (a local shadowing nothing it knows)
//! names nothing; a name it places over-approximately (a local named like an item of its module, a
//! method of another type of the same name, a field named like a method) takes that item too,
//! which can only check a unit again.

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::visit_mut::VisitMut;

/// The inputs crate's name, and its gate's module's.
const INPUTS: &str = "inputs";

/// The items of `src/inputs/mod.rs` that name its child modules, by name: the registry of units and
/// their checks. Their names of a child module reach nothing (the cut above).
pub const REGISTRY: [&str; 7] = ["UNITS", "REGIONS", "HERITAGE", "MATCHES", "ELEVATION", "SOURCES", "checks"];

/// How deep `use`s, re-exports and globs are followed: past it, a cycle.
const DEPTH: usize = 12;

/// Every crate read, by its name, and every file read (build.rs reruns when one changes).
pub struct World {
    pub crates: BTreeMap<String, Crate>,
    pub files: Vec<PathBuf>,
    impls: Impls,
}

/// A crate: its modules by path (`[]` its root), and the crates its code may name (its path
/// dependencies: their names in code, the crates they are).
pub struct Crate {
    pub modules: BTreeMap<Vec<String>, Module>,
    pub deps: BTreeMap<String, String>,
}

/// A module: its items, in order, its child modules' names, and its `use`s.
#[derive(Default)]
pub struct Module {
    pub items: Vec<Item>,
    pub children: BTreeSet<String>,
    pub scope: Scope,
}

/// `use`s: each name in scope with the paths it stands for, and the globs' paths.
#[derive(Default)]
pub struct Scope {
    pub uses: BTreeMap<String, Vec<Vec<String>>>,
    pub globs: Vec<Vec<String>>,
}

/// An item: what it is, the names it's found by, its digest, and what it names (`Names`) and the
/// `use`s within it.
pub struct Item {
    pub kind: Kind,
    pub names: Vec<String>,
    pub digest: u128,
    pub holds: Names,
    pub scope: Scope,
}

#[derive(Clone, PartialEq)]
pub enum Kind {
    /// A struct, enum, union or type alias.
    Type,
    Trait,
    MacroRules,
    /// An impl's header: its type's path and its trait's.
    Impl {
        ty: Option<Vec<String>>,
        tr: Option<Vec<String>>,
    },
    /// An impl's method: its header's index, its name.
    Method(usize, String),
    /// An item macro but `thread_local!` (`det!(f64, …)`): taken with its module.
    Unnamed,
    Other,
}

/// What an item's tokens name: names alone, paths, names called (after `.` or a path's `::`), and
/// macros invoked.
#[derive(Default)]
pub struct Names {
    pub idents: BTreeSet<String>,
    pub paths: BTreeSet<Vec<String>>,
    pub calls: BTreeSet<String>,
    pub macros: BTreeSet<Vec<String>>,
}

/// An item's place: its crate, its module, its index there.
pub type Id = (String, Vec<String>, usize);

/// Where a path leads: a module, or items.
enum Place {
    Module(String, Vec<String>),
    Items(Vec<Id>),
}

/// The impls of every crate read, placed once the world is read.
#[derive(Default)]
struct Impls {
    /// Each type's and trait's impl headers.
    of: BTreeMap<Id, Vec<Id>>,
    /// Each header's types (None: none of these crates', or not placed) and trait (None: an inherent
    /// impl; Some(empty): a crates.io trait's).
    on: BTreeMap<Id, (Option<Vec<Id>>, Option<Vec<Id>>)>,
    /// Each header's methods, by name.
    methods: BTreeMap<Id, Vec<(String, Id)>>,
    /// Each method name's methods.
    by_name: BTreeMap<String, Vec<Id>>,
    /// Each module's headers whose type isn't placed and that no trait of these crates reaches: taken
    /// with their module.
    loose: BTreeMap<(String, Vec<String>), Vec<Id>>,
    /// Each crate's `macro_rules!`, by name.
    macros: BTreeMap<String, BTreeMap<String, Vec<Id>>>,
}

impl World {
    /// The crate whose folder is `dir` (`crates/inputs`) and every crate its path dependencies reach.
    pub fn read(dir: &Path) -> Result<World, String> {
        let mut w = World { crates: BTreeMap::new(), files: Vec::new(), impls: Impls::default() };
        let mut todo = vec![dir.to_path_buf()];
        let mut seen = BTreeSet::new();
        while let Some(d) = todo.pop() {
            let toml = d.join("Cargo.toml");
            let name = crate_name(&toml)?;
            if !seen.insert(name.clone()) {
                continue;
            }
            let deps = path_deps(&d, &mut w.files)?;
            w.files.push(toml);
            let lib = d.join("src/lib.rs");
            let mut file = parse(&lib)?;
            let mut v = Strip { dir: d.join("src"), file_dir: d.join("src"), files: vec![lib], err: None };
            v.visit_file_mut(&mut file);
            if let Some(e) = v.err {
                return Err(e);
            }
            w.files.extend(v.files);
            let mut modules = BTreeMap::new();
            collect(&file.items, Vec::new(), &mut modules);
            let mut names = BTreeMap::new();
            for (n, p) in deps {
                names.insert(n, crate_name(&p.join("Cargo.toml"))?);
                todo.push(p);
            }
            w.crates.insert(name, Crate { modules, deps: names });
        }
        w.files.sort();
        w.files.dedup();
        w.impls = w.place_impls();
        Ok(w)
    }

    /// Every impl placed: its type's and its trait's items, its methods; every `macro_rules!`.
    fn place_impls(&self) -> Impls {
        let mut x = Impls::default();
        for (c, k) in &self.crates {
            for (m, module) in &k.modules {
                for (i, item) in module.items.iter().enumerate() {
                    let id = (c.clone(), m.clone(), i);
                    match &item.kind {
                        Kind::Impl { ty, tr } => {
                            let of = |p: &Option<Vec<String>>, want: Kind| -> Vec<Id> {
                                let Some(p) = p else { return Vec::new() };
                                let places = self.place(c, m, None, p, false, 0);
                                self.ids(places).into_iter().filter(|j| self.item(j).kind == want).collect()
                            };
                            let tys = Some(of(ty, Kind::Type)).filter(|v| !v.is_empty());
                            let trs = tr.as_ref().map(|_| of(tr, Kind::Trait));
                            for t in tys.iter().flatten().chain(trs.iter().flatten()) {
                                x.of.entry(t.clone()).or_default().push(id.clone());
                            }
                            // (Neither its type nor its trait placed among these crates': nothing would reach it.)
                            if tys.is_none() && trs.as_ref().is_none_or(|t| t.is_empty()) {
                                x.loose.entry((c.clone(), m.clone())).or_default().push(id.clone());
                            }
                            x.on.insert(id, (tys, trs));
                        }
                        Kind::Method(h, name) => {
                            let header = (c.clone(), m.clone(), *h);
                            x.methods.entry(header).or_default().push((name.clone(), id.clone()));
                            x.by_name.entry(name.clone()).or_default().push(id);
                        }
                        Kind::MacroRules => {
                            for n in &item.names {
                                x.macros.entry(c.clone()).or_default().entry(n.clone()).or_default().push(id.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        x
    }

    fn item(&self, (c, m, i): &Id) -> &Item {
        &self.crates[c].modules[m].items[*i]
    }

    /// The digest of the items `start` names and every item they name (`closure`), `leave_out` left
    /// out.
    pub fn digest(&self, start: Vec<Id>, leave_out: &[Id]) -> String {
        let mut h = xxhash_rust::xxh3::Xxh3::new();
        for id in &self.closure(start, leave_out) {
            let (c, m, _) = id;
            h.update(format!("{c}::{}#{:032x}\n", m.join("::"), self.item(id).digest).as_bytes());
        }
        format!("{:032x}", h.digest128())
    }

    /// The items `start` names and every item they name, transitively (the module docs), with the
    /// registry's cut; `leave_out` never taken.
    pub fn closure(&self, start: Vec<Id>, leave_out: &[Id]) -> BTreeSet<Id> {
        let mut seen: BTreeSet<Id> = BTreeSet::new();
        let mut called: BTreeSet<String> = BTreeSet::new();
        let mut modules: BTreeSet<(String, Vec<String>)> = BTreeSet::new();
        let mut todo = start;
        while let Some(id) = todo.pop() {
            if leave_out.contains(&id) || !seen.insert(id.clone()) {
                continue;
            }
            let (c, m, _) = &id;
            let item = self.item(&id);
            let mut found = self.edges(&id);
            // (Its module's first item: what's taken with the module.)
            if modules.insert((c.clone(), m.clone())) {
                found.extend(self.with_module(c, m));
            }
            // (A type's or a trait's impls; a header's methods; the methods of each name called.)
            if let Some(hs) = self.impls.of.get(&id) {
                found.extend(hs.iter().filter(|h| self.holds(h, &seen)).cloned());
            }
            if let Some(ms) = self.impls.methods.get(&id) {
                let whole = matches!(self.impls.on.get(&id), Some((_, Some(t))) if t.is_empty());
                found.extend(ms.iter().filter(|(n, _)| whole || called.contains(n)).map(|(_, j)| j.clone()));
            }
            for n in item.holds.calls.iter().filter(|n| called.insert(n.to_string())) {
                for j in self.impls.by_name.get(n).into_iter().flatten() {
                    if matches!(&self.item(j).kind, Kind::Method(h, _) if seen.contains(&(j.0.clone(), j.1.clone(), *h))) {
                        found.push(j.clone());
                    }
                }
            }
            todo.extend(found);
        }
        seen
    }

    /// Whether impl header `h`'s type (or a type of none of these crates) and trait (if it's one of
    /// theirs) are in `seen`.
    fn holds(&self, h: &Id, seen: &BTreeSet<Id>) -> bool {
        let (tys, trs) = &self.impls.on[h];
        let ty = tys.as_ref().is_none_or(|t| t.iter().any(|x| seen.contains(x)));
        let tr = trs.as_ref().is_none_or(|t| t.is_empty() || t.iter().any(|x| seen.contains(x)));
        ty && tr
    }

    /// The items item `id` names (its names, paths and macros, and what its own `use`s bring into
    /// scope), but a registry item's in `mod.rs`'s child modules.
    fn edges(&self, id: &Id) -> Vec<Id> {
        let (c, m, _) = id;
        let item = self.item(id);
        let n = &item.holds;
        let mut found = self.scoped(c, m, &item.scope);
        for x in &n.idents {
            found.extend(
                self.ids(
                    self.place(c, m, Some(&item.scope), std::slice::from_ref(x), false, 0)
                        .into_iter()
                        .filter(|p| matches!(p, Place::Items(_)))
                        .collect(),
                ),
            );
        }
        for p in &n.paths {
            found.extend(self.ids(self.place(c, m, Some(&item.scope), p, false, 0)));
        }
        for p in &n.macros {
            found.extend(self.macro_rules(c, m, &item.scope, p));
        }
        if self.registry(id) {
            found.retain(|f| !self.in_child(f));
        }
        found
    }

    /// Whether `id` is a registry item (`REGISTRY`).
    fn registry(&self, id: &Id) -> bool {
        id.0 == INPUTS
            && id.1.as_slice() == [INPUTS]
            && self.item(id).names.iter().any(|n| REGISTRY.contains(&n.as_str()))
    }

    /// Whether `id` is in one of `mod.rs`'s child modules.
    fn in_child(&self, (c, m, _): &Id) -> bool {
        c == INPUTS && m.len() > 1 && m[0] == INPUTS
    }

    /// What's taken with module `m` of crate `c`: its item macros, its traits and what its `use`s
    /// bring into scope (a method call there may call theirs), and its headers no type or trait
    /// reaches.
    fn with_module(&self, c: &str, m: &[String]) -> Vec<Id> {
        let module = &self.crates[c].modules[m];
        let mut v: Vec<Id> = (0..module.items.len())
            .filter(|&i| matches!(module.items[i].kind, Kind::Unnamed | Kind::Trait))
            .map(|i| (c.to_string(), m.to_vec(), i))
            .collect();
        v.extend(self.impls.loose.get(&(c.to_string(), m.to_vec())).into_iter().flatten().cloned());
        v.extend(self.scoped(c, m, &module.scope));
        v
    }

    /// What `scope` (in module `m` of crate `c`) brings in that's used without being named: the traits
    /// its `use`s and globs bring into scope (a method call may call theirs), and the enums a glob
    /// takes the variants of.
    fn scoped(&self, c: &str, m: &[String], scope: &Scope) -> Vec<Id> {
        let mut v = Vec::new();
        for ps in scope.uses.values() {
            for p in ps {
                let ids = self.ids(self.place(c, m, None, p, true, 0));
                v.extend(ids.into_iter().filter(|j| self.item(j).kind == Kind::Trait));
            }
        }
        for g in &scope.globs {
            for place in self.place(c, m, None, g, true, 0) {
                match place {
                    Place::Items(ids) => v.extend(ids),
                    Place::Module(gc, gm) => {
                        let module = &self.crates[&gc].modules[&gm];
                        v.extend(
                            (0..module.items.len())
                                .filter(|&i| module.items[i].kind == Kind::Trait)
                                .map(|i| (gc.clone(), gm.clone(), i)),
                        );
                    }
                }
            }
        }
        v
    }

    /// The `macro_rules!` a macro invocation by path `p`, in module `m` of crate `c`, may be: each of
    /// its name in the crate its path names (else its own and every crate it depends on), and what its
    /// name is as a `use` brings it in.
    fn macro_rules(&self, c: &str, m: &[String], scope: &Scope, p: &[String]) -> Vec<Id> {
        let Some(name) = p.last() else { return Vec::new() };
        let k = &self.crates[c];
        let crates: Vec<&str> = match p.first().map(String::as_str) {
            _ if p.len() == 1 => [c].into_iter().chain(k.deps.values().map(String::as_str)).collect(),
            Some("crate" | "$crate" | "self" | "super") => vec![c],
            Some("::") => p.get(1).and_then(|d| k.deps.get(d)).map(String::as_str).into_iter().collect(),
            Some(d) => k.deps.get(d).map(String::as_str).into_iter().collect(),
            None => Vec::new(),
        };
        let mut v: Vec<Id> = crates
            .into_iter()
            .flat_map(|x| self.impls.macros.get(x).and_then(|ms| ms.get(name)).into_iter().flatten().cloned())
            .collect();
        if p.len() == 1 {
            v.extend(self.ids(self.place(c, m, Some(scope), p, false, 0)));
        }
        v
    }

    /// Every item `places` hold (a module's every item).
    fn ids(&self, places: Vec<Place>) -> Vec<Id> {
        places
            .into_iter()
            .flat_map(|p| match p {
                Place::Items(v) => v,
                Place::Module(c, m) => self.all(&c, &m),
            })
            .collect()
    }

    /// Every item of module `m` of crate `c`.
    pub fn all(&self, c: &str, m: &[String]) -> Vec<Id> {
        let n = self.crates.get(c).and_then(|k| k.modules.get(m)).map_or(0, |x| x.items.len());
        (0..n).map(|i| (c.to_string(), m.to_vec(), i)).collect()
    }

    /// Every item of the modules under `m` of crate `c` (`m` among them).
    pub fn under(&self, c: &str, m: &[String]) -> Vec<Id> {
        let k = &self.crates[c];
        k.modules.keys().filter(|p| p.starts_with(m)).flat_map(|p| self.all(c, p)).collect()
    }

    /// The items of module `m` of crate `c` found by one of `names` (not methods nor impls: those by
    /// their types and calls).
    pub fn named(&self, c: &str, m: &[String], names: &[&str]) -> Vec<Id> {
        let Some(module) = self.crates.get(c).and_then(|k| k.modules.get(m)) else { return Vec::new() };
        module
            .items
            .iter()
            .enumerate()
            .filter(|(_, x)| x.names.iter().any(|n| names.contains(&n.as_str())))
            .map(|(i, _)| (c.to_string(), m.to_vec(), i))
            .collect()
    }

    /// Where path `p`, written in module `m` of crate `c` (in an item whose own `use`s are `scope`; as
    /// a `use`'s path when `is_use`), leads. Its first segment placed (the module docs), the rest
    /// from there (`absolute`).
    fn place(
        &self,
        c: &str,
        m: &[String],
        scope: Option<&Scope>,
        p: &[String],
        is_use: bool,
        depth: usize,
    ) -> Vec<Place> {
        let Some(first) = p.first() else { return Vec::new() };
        if depth > DEPTH {
            return Vec::new();
        }
        let Some(k) = self.crates.get(c) else { return Vec::new() };
        let Some(module) = k.modules.get(m) else { return Vec::new() };
        let rest = &p[1..];
        match first.as_str() {
            "::" => {
                return match rest.first().and_then(|d| k.deps.get(d)) {
                    Some(d) => self.absolute(d, &rest[1..], depth + 1),
                    None => Vec::new(),
                }
            }
            "crate" | "$crate" => return self.absolute(c, rest, depth + 1),
            "self" => return self.absolute(c, &[m, rest].concat(), depth + 1),
            "super" => {
                let ups = p.iter().take_while(|s| *s == "super").count();
                if ups > m.len() {
                    return Vec::new();
                }
                return self.absolute(c, &[&m[..m.len() - ups], &p[ups..]].concat(), depth + 1);
            }
            // (The impl's own type: reached already, its header being.)
            "Self" => return Vec::new(),
            _ => {}
        }
        let mut out = Vec::new();
        // (Through a `use` name: from where it points, the item's own `use`s first.)
        let through = |s: &Scope, out: &mut Vec<Place>| {
            for t in s.uses.get(first).into_iter().flatten() {
                out.extend(self.place(c, m, None, &[t.as_slice(), rest].concat(), true, depth + 1));
            }
        };
        if let Some(s) = scope {
            through(s, &mut out);
        }
        if module.children.contains(first) {
            out.extend(self.absolute(c, &[m, p].concat(), depth + 1));
        }
        let own = self.named(c, m, &[first.as_str()]);
        if !own.is_empty() {
            out.push(Place::Items(own));
        }
        if let Some(d) = k.deps.get(first) {
            out.extend(self.absolute(d, rest, depth + 1));
        }
        // (A `use`'s own name is its path's: not a name it brings in.)
        if !is_use || p.len() > 1 {
            through(&module.scope, &mut out);
        }
        // (Through a glob: as a path from the glob's.)
        if out.is_empty() {
            for g in scope.into_iter().flat_map(|s| &s.globs).chain(&module.scope.globs) {
                for place in self.place(c, m, None, g, true, depth + 1) {
                    if let Place::Module(gc, gm) = place {
                        out.extend(self.absolute(&gc, &[gm.as_slice(), p].concat(), depth + 1).into_iter().filter(
                            // (A glob's module's name, not the module itself where it names nothing.)
                            |x| !matches!(x, Place::Module(_, xm) if xm == &gm),
                        ));
                    }
                }
            }
        }
        out
    }

    /// Where absolute path `p` in crate `c` leads: the longest module it starts with, then the items
    /// its next segment names there (or a `use` name of that module, a re-export; or a name its globs
    /// bring in), else that module whole.
    fn absolute(&self, c: &str, p: &[String], depth: usize) -> Vec<Place> {
        let Some(k) = self.crates.get(c) else { return Vec::new() };
        if depth > DEPTH {
            return Vec::new();
        }
        let mut n = p.len();
        while n > 0 && !k.modules.contains_key(&p[..n]) {
            n -= 1;
        }
        let m = &p[..n];
        let Some(name) = p.get(n) else { return vec![Place::Module(c.to_string(), m.to_vec())] };
        let named = self.named(c, m, &[name.as_str()]);
        if !named.is_empty() {
            return vec![Place::Items(named)];
        }
        let module = &k.modules[m];
        let mut out = Vec::new();
        for t in module.scope.uses.get(name).into_iter().flatten() {
            out.extend(self.place(c, m, None, &[t.as_slice(), &p[n + 1..]].concat(), true, depth + 1));
        }
        for g in &module.scope.globs {
            let via = self.place(c, m, None, &[g.as_slice(), &p[n..]].concat(), true, depth + 1);
            out.extend(via.into_iter().filter(|x| !matches!(x, Place::Module(..))));
        }
        if out.is_empty() {
            out.push(Place::Module(c.to_string(), m.to_vec()));
        }
        out
    }
}

/// A crate's name, from its manifest (`[package]`'s `name`, `-` as `_`).
fn crate_name(toml: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(toml).map_err(|e| format!("read {}: {e}", toml.display()))?;
    let mut package = false;
    for l in text.lines() {
        let t = strip_comment(l);
        if t.starts_with('[') {
            package = t == "[package]";
        } else if let (true, Some((k, v))) = (package, t.split_once('=')) {
            if k.trim() == "name" {
                return Ok(unquote(v.trim())
                    .ok_or_else(|| format!("{}: its name isn't quoted", toml.display()))?
                    .replace('-', "_"));
            }
        }
    }
    Err(format!("{} names no package", toml.display()))
}

/// A dependency as a manifest declares it: its name there, and its `path`, `package` and
/// `workspace` keys.
#[derive(Default, Clone)]
struct Dep {
    name: String,
    path: Option<String>,
    package: Option<String>,
    workspace: bool,
}

/// The path dependencies of the crate in `dir` (not its dev- or build-dependencies): each one's name
/// in code and its folder. A `workspace = true` one is the workspace's (`[workspace.dependencies]`,
/// its manifest added to `files`).
fn path_deps(dir: &Path, files: &mut Vec<PathBuf>) -> Result<Vec<(String, PathBuf)>, String> {
    let toml = dir.join("Cargo.toml");
    let mut ws: Option<(PathBuf, Vec<Dep>)> = None;
    let mut out = Vec::new();
    for d in deps_of(&toml, false)? {
        let (d, base) = if d.workspace {
            if ws.is_none() {
                let abs = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
                let root = abs
                    .ancestors()
                    .skip(1)
                    .find(|a| {
                        std::fs::read_to_string(a.join("Cargo.toml"))
                            .is_ok_and(|t| t.lines().any(|l| strip_comment(l) == "[workspace]"))
                    })
                    .ok_or_else(|| {
                        format!("{}: {} is the workspace's, and no workspace holds it", toml.display(), d.name)
                    })?;
                files.push(root.join("Cargo.toml"));
                ws = Some((root.to_path_buf(), deps_of(&root.join("Cargo.toml"), true)?));
            }
            let (root, table) = ws.as_ref().unwrap();
            let w = table.iter().find(|w| w.name == d.name).ok_or_else(|| {
                format!("{}: {} is the workspace's, and its [workspace.dependencies] hasn't it", toml.display(), d.name)
            })?;
            (Dep { name: d.name.clone(), package: d.package.clone().or(w.package.clone()), ..w.clone() }, root.clone())
        } else {
            (d, dir.to_path_buf())
        };
        if let Some(p) = &d.path {
            out.push((d.name.replace('-', "_"), base.join(p)));
        }
    }
    Ok(out)
}

/// The dependencies a manifest's dependency tables declare: `[dependencies]` and
/// `[target.….dependencies]`, each as a table of its own too (`[dependencies.x]`); with `ws`,
/// `[workspace.dependencies]`. A line there of a form this doesn't read is an error.
fn deps_of(toml: &Path, ws: bool) -> Result<Vec<Dep>, String> {
    let text = std::fs::read_to_string(toml).map_err(|e| format!("read {}: {e}", toml.display()))?;
    let mut out: Vec<Dep> = Vec::new();
    // (None: another table; Some(None): a dependency table; Some(Some(i)): dependency i's own.)
    let mut table: Option<Option<usize>> = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut k = 0;
    while k < lines.len() {
        let at = format!("{}:{}", toml.display(), k + 1);
        let mut l = strip_comment(lines[k]).to_string();
        k += 1;
        if l.is_empty() {
            continue;
        }
        if l.starts_with('[') {
            let keys = split_outside(l.trim_start_matches('[').trim_end_matches(']'), '.');
            let keys: Vec<String> =
                keys.iter().map(|s| unquote(s.trim()).unwrap_or_else(|| s.trim().to_string())).collect();
            let deps_at = if ws {
                (keys.len() >= 2 && keys[0] == "workspace" && keys[1] == "dependencies").then_some(2)
            } else if keys.first().is_some_and(|s| s == "dependencies") {
                Some(1)
            } else {
                (keys.len() >= 3 && keys[0] == "target" && keys[2] == "dependencies").then_some(3)
            };
            table = match deps_at {
                _ if l.starts_with("[[") => None,
                Some(n) if keys.len() == n => Some(None),
                Some(n) if keys.len() == n + 1 => {
                    out.push(Dep { name: keys[n].clone(), ..Dep::default() });
                    Some(Some(out.len() - 1))
                }
                Some(_) => return Err(format!("{at}: a dependency table this doesn't read: {l}")),
                None => None,
            };
            continue;
        }
        let Some(t) = table else { continue };
        // (A value over several lines: an array, until its `]`.)
        while l.matches('[').count() > l.matches(']').count() && k < lines.len() {
            l.push(' ');
            l.push_str(strip_comment(lines[k]));
            k += 1;
        }
        let Some((key, val)) = l.split_once('=') else { return Err(format!("{at}: not a key = value: {l}")) };
        let (key, val) = (key.trim(), val.trim());
        match t {
            Some(i) => set(&mut out[i], key, val, &at)?,
            None => {
                let (name, sub) = match split_outside(key, '.').as_slice() {
                    [n] => (n.trim().to_string(), None),
                    [n, s] => (n.trim().to_string(), Some(s.trim().to_string())),
                    _ => return Err(format!("{at}: a dependency key this doesn't read: {key}")),
                };
                let mut d = Dep { name: unquote(&name).unwrap_or(name), ..Dep::default() };
                match sub {
                    Some(s) => set(&mut d, &s, val, &at)?,
                    None if val.starts_with('"') => {}
                    None if val.starts_with('{') && val.ends_with('}') => {
                        for kv in split_outside(&val[1..val.len() - 1], ',') {
                            if kv.trim().is_empty() {
                                continue;
                            }
                            let Some((a, b)) = kv.split_once('=') else {
                                return Err(format!("{at}: not a key = value: {kv}"));
                            };
                            set(&mut d, a.trim(), b.trim(), &at)?;
                        }
                    }
                    None => return Err(format!("{at}: a dependency this doesn't read: {l}")),
                }
                // (A dotted key adds to its dependency: `x.workspace = true`, then `x.features = …`.)
                match out.iter_mut().find(|o| o.name == d.name) {
                    Some(o) => {
                        o.path = o.path.take().or(d.path);
                        o.package = o.package.take().or(d.package);
                        o.workspace |= d.workspace;
                    }
                    None => out.push(d),
                }
            }
        }
    }
    Ok(out)
}

/// Dependency `d`'s key `key` set to `val`: `path`, `package` and `workspace` read, the keys that
/// don't change where its code is passed over, any other an error.
fn set(d: &mut Dep, key: &str, val: &str, at: &str) -> Result<(), String> {
    let s = || unquote(val).ok_or_else(|| format!("{at}: {key} isn't a string: {val}"));
    match key {
        "path" => d.path = Some(s()?),
        "package" => d.package = Some(s()?),
        "workspace" => d.workspace = val == "true",
        "version" | "features" | "default-features" | "default_features" | "optional" | "git" | "branch" | "tag"
        | "rev" | "registry" | "public" => {}
        _ => return Err(format!("{at}: a dependency key this doesn't read: {key}")),
    }
    Ok(())
}

/// A TOML line without its comment (a `#` outside a string), trimmed.
fn strip_comment(l: &str) -> &str {
    let mut quoted = None;
    for (i, ch) in l.char_indices() {
        match (quoted, ch) {
            (None, '"' | '\'') => quoted = Some(ch),
            (Some(q), _) if ch == q => quoted = None,
            (None, '#') => return l[..i].trim(),
            _ => {}
        }
    }
    l.trim()
}

/// `s` split at each `sep` outside strings, brackets and braces.
fn split_outside(s: &str, sep: char) -> Vec<String> {
    let (mut out, mut cur, mut depth, mut quoted) = (Vec::new(), String::new(), 0i32, None);
    for ch in s.chars() {
        match (quoted, ch) {
            (None, '"' | '\'') => quoted = Some(ch),
            (Some(q), _) if ch == q => quoted = None,
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            (None, c) if c == sep && depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.push(cur);
    out
}

/// A TOML string's text (`"x"`, `'x'`); None for anything else.
fn unquote(s: &str) -> Option<String> {
    let s = s.trim();
    let q = s.chars().next()?;
    (s.len() >= 2 && (q == '"' || q == '\'') && s.ends_with(q)).then(|| s[1..s.len() - 1].to_string())
}

/// The modules and items of `items` (a module at `path`, its children inlined by `Strip`).
fn collect(items: &[syn::Item], path: Vec<String>, out: &mut BTreeMap<Vec<String>, Module>) {
    let mut m = Module::default();
    for it in items {
        match it {
            syn::Item::Mod(x) => {
                let name = x.ident.to_string();
                m.children.insert(name.clone());
                if let Some((_, inner)) = &x.content {
                    let mut p = path.clone();
                    p.push(name);
                    collect(inner, p, out);
                }
            }
            syn::Item::Use(u) => {
                // (Found by no name and naming nothing: a name it brings in is placed through `scope`
                // where it's used, so only what that name stands for is taken.)
                use_tree(&u.tree, Vec::new(), &mut m.scope);
                let mut x = item(Kind::Other, Vec::new(), it.to_token_stream(), Scope::default());
                x.holds = Names::default();
                m.items.push(x);
            }
            syn::Item::Impl(x) => {
                let path_of = |t: &syn::Type| -> Option<Vec<String>> {
                    let mut t = t;
                    loop {
                        match t {
                            syn::Type::Reference(r) => t = &r.elem,
                            syn::Type::Paren(p) => t = &p.elem,
                            syn::Type::Group(g) => t = &g.elem,
                            syn::Type::Path(p) if p.qself.is_none() => {
                                let mut v: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
                                if p.path.leading_colon.is_some() {
                                    v.insert(0, "::".into());
                                }
                                return Some(v);
                            }
                            _ => return None,
                        }
                    }
                };
                let ty = path_of(&x.self_ty);
                let tr = x.trait_.as_ref().map(|(_, p, _)| {
                    let mut v: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
                    if p.leading_colon.is_some() {
                        v.insert(0, "::".into());
                    }
                    v
                });
                // (Its header, then each method as an item of its own.)
                let mut head = x.clone();
                head.items.retain(|i| !matches!(i, syn::ImplItem::Fn(_)));
                let at = m.items.len();
                m.items.push(item(
                    Kind::Impl { ty, tr },
                    Vec::new(),
                    head.to_token_stream(),
                    local_uses(|v| v.visit_item_impl_mut(&mut head.clone())),
                ));
                for i in &x.items {
                    if let syn::ImplItem::Fn(f) = i {
                        let scope = local_uses(|v| v.visit_impl_item_fn_mut(&mut f.clone()));
                        m.items.push(item(
                            Kind::Method(at, f.sig.ident.to_string()),
                            Vec::new(),
                            f.to_token_stream(),
                            scope,
                        ));
                    }
                }
            }
            _ => {
                let (kind, names) = kind_names(it);
                let scope = local_uses(|v| v.visit_item_mut(&mut it.clone()));
                m.items.push(item(kind, names, it.to_token_stream(), scope));
            }
        }
    }
    out.insert(path, m);
}

/// What item `i` is, and the names it's found by.
fn kind_names(i: &syn::Item) -> (Kind, Vec<String>) {
    use syn::Item::*;
    let one = |k: Kind, id: &syn::Ident| (k, vec![id.to_string()]);
    match i {
        Struct(x) => one(Kind::Type, &x.ident),
        Enum(x) => one(Kind::Type, &x.ident),
        Union(x) => one(Kind::Type, &x.ident),
        Type(x) => one(Kind::Type, &x.ident),
        Trait(x) => one(Kind::Trait, &x.ident),
        TraitAlias(x) => one(Kind::Trait, &x.ident),
        Const(x) => one(Kind::Other, &x.ident),
        Fn(x) => one(Kind::Other, &x.sig.ident),
        Static(x) => one(Kind::Other, &x.ident),
        Macro(x) => match &x.ident {
            Some(id) => one(Kind::MacroRules, id),
            // (`thread_local!`: found by its statics' names.)
            None if x.mac.path.segments.last().is_some_and(|s| s.ident == "thread_local") => {
                let t: Vec<TokenTree> = x.mac.tokens.clone().into_iter().collect();
                let names = t
                    .windows(2)
                    .filter_map(|w| match w {
                        [TokenTree::Ident(s), TokenTree::Ident(n)] if s == "static" => Some(n.to_string()),
                        _ => None,
                    })
                    .collect();
                (Kind::Other, names)
            }
            None => (Kind::Unnamed, Vec::new()),
        },
        // (An extern block: found by its functions' and statics' names.)
        ForeignMod(x) => {
            let names = x
                .items
                .iter()
                .filter_map(|f| match f {
                    syn::ForeignItem::Fn(f) => Some(f.sig.ident.to_string()),
                    syn::ForeignItem::Static(s) => Some(s.ident.to_string()),
                    syn::ForeignItem::Type(t) => Some(t.ident.to_string()),
                    _ => None,
                })
                .collect();
            (Kind::Other, names)
        }
        Verbatim(_) => (Kind::Unnamed, Vec::new()),
        _ => (Kind::Other, Vec::new()),
    }
}

/// The `use`s within an item (a function's own), as `visit` visits it.
fn local_uses(visit: impl FnOnce(&mut LocalUses)) -> Scope {
    let mut v = LocalUses(Scope::default());
    visit(&mut v);
    v.0
}

struct LocalUses(Scope);

impl VisitMut for LocalUses {
    fn visit_item_use_mut(&mut self, u: &mut syn::ItemUse) {
        use_tree(&u.tree, Vec::new(), &mut self.0);
    }
}

/// A `use` tree's names in scope and the paths they stand for, and its globs (a leading `::` as a
/// segment of its own).
fn use_tree(t: &syn::UseTree, prefix: Vec<String>, out: &mut Scope) {
    let with = |s: String| {
        let mut p = prefix.clone();
        p.push(s);
        p
    };
    let mut add = |n: String, p: Vec<String>| out.uses.entry(n).or_default().push(p);
    match t {
        syn::UseTree::Path(p) => use_tree(&p.tree, with(p.ident.to_string()), out),
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last() {
                    add(last.clone(), prefix.clone());
                }
            } else {
                add(name.clone(), with(name));
            }
        }
        syn::UseTree::Rename(r) => add(r.rename.to_string(), with(r.ident.to_string())),
        syn::UseTree::Glob(_) => out.globs.push(prefix),
        syn::UseTree::Group(g) => g.items.iter().for_each(|i| use_tree(i, prefix.clone(), out)),
    }
}

/// An item from its tokens: its digest (`feed`), and what it names (`scan`).
fn item(kind: Kind, names: Vec<String>, ts: TokenStream, scope: Scope) -> Item {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    feed(ts.clone(), &mut h, Delimiter::None);
    let mut holds = Names::default();
    scan(ts, false, &mut holds);
    Item { kind, names, digest: h.digest128(), holds, scope }
}

/// What `ts` names (`Names`), `attr` within an attribute: a name alone (an ident not after `.`, `::`
/// or `'`), a path from where it starts (a leading `::` its first segment), each name after `.` or a
/// path's `::` as called, a macro's path (`name!`), the names a string literal's format arguments
/// hold (`{name}`, `{name:…}`, `{:name$}`), and an attribute's string naming a path (serde's
/// `default = "one"`).
fn scan(ts: TokenStream, attr: bool, n: &mut Names) {
    let t: Vec<TokenTree> = ts.into_iter().collect();
    let punct = |i: usize, c: char| matches!(t.get(i), Some(TokenTree::Punct(p)) if p.as_char() == c);
    let sep = |i: usize| {
        matches!((t.get(i), t.get(i + 1)), (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
            if a.as_char() == ':' && a.spacing() == Spacing::Joint && b.as_char() == ':')
    };
    let mut i = 0;
    while i < t.len() {
        match &t[i] {
            TokenTree::Group(g) => {
                let attr_here = g.delimiter() == Delimiter::Bracket
                    && (i >= 1 && punct(i - 1, '#') || i >= 2 && punct(i - 1, '!') && punct(i - 2, '#'));
                // (A doc attribute: words, not code.)
                let doc = attr_here && matches!(g.stream().into_iter().next(), Some(TokenTree::Ident(d)) if d == "doc");
                if !doc {
                    scan(g.stream(), attr || attr_here, n);
                }
                i += 1;
            }
            TokenTree::Literal(l) => {
                literal(l, attr, n);
                i += 1;
            }
            TokenTree::Ident(id) => {
                // (A lifetime or a label.)
                if i >= 1 && punct(i - 1, '\'') {
                    i += 1;
                    continue;
                }
                // (A `use` within an item: its names are its scope's, placed where they're used.)
                if id == "use" {
                    while i < t.len() && !punct(i, ';') {
                        i += 1;
                    }
                    continue;
                }
                let dollar = i >= 1 && punct(i - 1, '$');
                let s = if dollar { format!("${id}") } else { id.to_string() };
                // (After `.`, not a range's `..`: a method or a field.)
                if i >= 1 && punct(i - 1, '.') && !(i >= 2 && punct(i - 2, '.')) {
                    n.calls.insert(s);
                    i += 1;
                    continue;
                }
                let mut p = vec![s];
                if i >= 2 && sep(i - 2) {
                    // (After a path's generics, `Vec::<u8>::new`, `<T as Tr>::m`: a name called; else a
                    // leading `::`.)
                    if i >= 3 && (matches!(&t[i - 3], TokenTree::Ident(_)) || punct(i - 3, '>')) {
                        n.calls.insert(p.pop().unwrap());
                        i += 1;
                        continue;
                    }
                    p.insert(0, "::".to_string());
                }
                let mut j = i + 1;
                while sep(j) {
                    let Some(TokenTree::Ident(x)) = t.get(j + 2) else { break };
                    p.push(x.to_string());
                    j += 3;
                }
                let bang = matches!(t.get(j), Some(TokenTree::Punct(b)) if b.as_char() == '!' && b.spacing() == Spacing::Alone)
                    && matches!(t.get(j + 1), Some(TokenTree::Group(_) | TokenTree::Ident(_)));
                if bang {
                    n.macros.insert(p);
                } else if p.len() == 1 {
                    n.idents.insert(p.pop().unwrap());
                } else {
                    n.calls.extend(p[1..].iter().filter(|x| *x != "::").cloned());
                    n.paths.insert(p);
                }
                i = j;
            }
            _ => i += 1,
        }
    }
}

/// What string literal `l` names: its format arguments' names, and, in an attribute, the path it
/// is (nothing for any other literal).
fn literal(l: &proc_macro2::Literal, attr: bool, n: &mut Names) {
    let Ok(s) = syn::parse_str::<syn::LitStr>(&l.to_string()) else { return };
    let v = s.value();
    let word = |w: &str| {
        !w.is_empty()
            && !w.starts_with(|c: char| c.is_ascii_digit())
            && w.chars().all(|c| c == '_' || c.is_alphanumeric())
    };
    if attr {
        let segs: Vec<&str> = v.split("::").collect();
        if segs.iter().all(|s| word(s)) {
            match segs.as_slice() {
                [one] => {
                    n.idents.insert(one.to_string());
                }
                _ => {
                    n.calls.extend(segs[1..].iter().map(|s| s.to_string()));
                    n.paths.insert(segs.iter().map(|s| s.to_string()).collect());
                }
            }
        }
    }
    let mut rest = v.as_str();
    while let Some(at) = rest.find('{') {
        rest = &rest[at + 1..];
        if let Some(r) = rest.strip_prefix('{') {
            rest = r;
            continue;
        }
        let end = rest.find('}').unwrap_or(rest.len());
        let (arg, spec) = rest[..end].split_once(':').unwrap_or((&rest[..end], ""));
        let arg = arg.trim();
        if word(arg) && arg != "_" {
            n.idents.insert(arg.to_string());
        }
        // (A width or precision by name: `{:w$}`.)
        for w in spec.split(|c: char| !(c == '_' || c == '$' || c.is_alphanumeric())) {
            if let Some(name) = w.strip_suffix('$') {
                if word(name) {
                    n.idents.insert(name.to_string());
                }
            }
        }
        rest = &rest[end..];
    }
}

fn parse(p: &Path) -> Result<syn::File, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    syn::parse_file(&text).map_err(|e| {
        let at = e.span().start();
        format!("{}:{}:{}: {e}", p.display(), at.line, at.column + 1)
    })
}

/// The file of `mod x;` (no body) declared in a file in `file_dir` whose children are in `dir`,
/// and the folder its own children are in.
fn child_file(m: &syn::ItemMod, dir: &Path, file_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let name = m.ident.to_string();
    for a in &m.attrs {
        if let syn::Meta::NameValue(nv) = &a.meta {
            if nv.path.is_ident("path") {
                let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value else {
                    return Err(format!("mod {name}'s #[path] isn't a string"));
                };
                let f = file_dir.join(s.value());
                let d = f.parent().unwrap_or(Path::new(".")).to_path_buf();
                return Ok((f, d));
            }
        }
    }
    let flat = dir.join(format!("{name}.rs"));
    if flat.exists() {
        return Ok((flat, dir.join(&name)));
    }
    let nested = dir.join(&name).join("mod.rs");
    if nested.exists() {
        return Ok((nested, dir.join(&name)));
    }
    Err(format!("mod {name}: neither {} nor {}", flat.display(), nested.display()))
}

/// Whether what `attrs` are on is compiled into the app: not under a `cfg` that's false there,
/// nor a `#[test]`.
fn compiled(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().all(|a| {
        if a.path().is_ident("test") {
            return false;
        }
        let syn::Meta::List(l) = &a.meta else { return true };
        if !l.path.is_ident("cfg") {
            return true;
        }
        cfg(l.tokens.clone()) != Some(false)
    })
}

/// A `cfg` predicate in the app's build: `test` and the `testkit` feature false, anything else
/// unknown (None), so kept.
fn cfg(ts: TokenStream) -> Option<bool> {
    let t: Vec<TokenTree> = ts.into_iter().collect();
    match t.as_slice() {
        [TokenTree::Ident(i)] if i == "test" => Some(false),
        [TokenTree::Ident(i), TokenTree::Punct(p), TokenTree::Literal(l)]
            if i == "feature" && p.as_char() == '=' && l.to_string() == "\"testkit\"" =>
        {
            Some(false)
        }
        [TokenTree::Ident(i), TokenTree::Group(g)] if g.delimiter() == Delimiter::Parenthesis => {
            let args = split_commas(g.stream());
            let vals: Vec<Option<bool>> = args.into_iter().map(cfg).collect();
            match i.to_string().as_str() {
                "not" if vals.len() == 1 => vals[0].map(|v| !v),
                "all" if vals.contains(&Some(false)) => Some(false),
                "all" if vals.iter().all(|v| *v == Some(true)) => Some(true),
                "any" if vals.contains(&Some(true)) => Some(true),
                "any" if vals.iter().all(|v| *v == Some(false)) => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}

fn split_commas(ts: TokenStream) -> Vec<TokenStream> {
    let mut out = vec![TokenStream::new()];
    for t in ts {
        match &t {
            TokenTree::Punct(p) if p.as_char() == ',' => out.push(TokenStream::new()),
            _ => out.last_mut().unwrap().extend([t]),
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

/// The macros compiling a file in.
const INCLUDES: [&str; 3] = ["include_str", "include_bytes", "include"];

/// Drops what the app doesn't compile, puts each child module's file in its place and each
/// compiled-in file's digest in place of its path.
struct Strip {
    dir: PathBuf,
    file_dir: PathBuf,
    files: Vec<PathBuf>,
    err: Option<String>,
}

impl Strip {
    fn fail(&mut self, e: String) {
        self.err.get_or_insert(e);
    }

    /// `ts` with each include macro's path literal replaced by the digest of the file it names.
    fn includes(&mut self, ts: TokenStream) -> TokenStream {
        let t: Vec<TokenTree> = ts.into_iter().collect();
        let mut out = Vec::with_capacity(t.len());
        let mut i = 0;
        while i < t.len() {
            if let (TokenTree::Ident(id), Some(TokenTree::Punct(p)), Some(TokenTree::Group(g))) =
                (&t[i], t.get(i + 1), t.get(i + 2))
            {
                if INCLUDES.contains(&id.to_string().as_str()) && p.as_char() == '!' {
                    let lit = self.included(g.stream());
                    out.extend([t[i].clone(), t[i + 1].clone()]);
                    out.push(TokenTree::Group(proc_macro2::Group::new(g.delimiter(), lit)));
                    i += 3;
                    continue;
                }
            }
            out.push(match &t[i] {
                TokenTree::Group(g) => {
                    TokenTree::Group(proc_macro2::Group::new(g.delimiter(), self.includes(g.stream())))
                }
                x => x.clone(),
            });
            i += 1;
        }
        out.into_iter().collect()
    }

    /// An include macro's argument: the digest of the file its literal names, as a literal.
    fn included(&mut self, arg: TokenStream) -> TokenStream {
        // (A build script's output, `concat!(env!("OUT_DIR"), …)`: made from what's read here, so
        // counted as itself, a word.)
        if arg.to_string().replace(' ', "").starts_with("concat!(env!(\"OUT_DIR\")") {
            return proc_macro2::Literal::string("OUT_DIR").into_token_stream();
        }
        let lit: Result<syn::LitStr, _> = syn::parse2(arg.clone());
        let Ok(lit) = lit else {
            self.fail(format!(
                "an include in {} names its file by {arg}, not a string literal",
                self.file_dir.display()
            ));
            return arg;
        };
        let p = self.file_dir.join(lit.value());
        match std::fs::read(&p) {
            Ok(b) => {
                self.files.push(p);
                let d = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&b));
                proc_macro2::Literal::string(&d).into_token_stream()
            }
            Err(e) => {
                self.fail(format!("read {}: {e}", p.display()));
                arg
            }
        }
    }
}

fn item_attrs(i: &syn::Item) -> &[syn::Attribute] {
    use syn::Item::*;
    match i {
        Const(x) => &x.attrs,
        Enum(x) => &x.attrs,
        ExternCrate(x) => &x.attrs,
        Fn(x) => &x.attrs,
        ForeignMod(x) => &x.attrs,
        Impl(x) => &x.attrs,
        Macro(x) => &x.attrs,
        Mod(x) => &x.attrs,
        Static(x) => &x.attrs,
        Struct(x) => &x.attrs,
        Trait(x) => &x.attrs,
        TraitAlias(x) => &x.attrs,
        Type(x) => &x.attrs,
        Union(x) => &x.attrs,
        Use(x) => &x.attrs,
        _ => &[],
    }
}

fn impl_attrs(i: &syn::ImplItem) -> &[syn::Attribute] {
    match i {
        syn::ImplItem::Const(x) => &x.attrs,
        syn::ImplItem::Fn(x) => &x.attrs,
        syn::ImplItem::Type(x) => &x.attrs,
        syn::ImplItem::Macro(x) => &x.attrs,
        _ => &[],
    }
}

fn trait_attrs(i: &syn::TraitItem) -> &[syn::Attribute] {
    match i {
        syn::TraitItem::Const(x) => &x.attrs,
        syn::TraitItem::Fn(x) => &x.attrs,
        syn::TraitItem::Type(x) => &x.attrs,
        syn::TraitItem::Macro(x) => &x.attrs,
        _ => &[],
    }
}

fn stmt_attrs(s: &syn::Stmt) -> &[syn::Attribute] {
    match s {
        syn::Stmt::Item(i) => item_attrs(i),
        syn::Stmt::Local(l) => &l.attrs,
        syn::Stmt::Macro(m) => &m.attrs,
        syn::Stmt::Expr(..) => &[],
    }
}

impl VisitMut for Strip {
    fn visit_file_mut(&mut self, f: &mut syn::File) {
        f.items.retain(|i| compiled(item_attrs(i)));
        syn::visit_mut::visit_file_mut(self, f);
    }

    fn visit_item_mod_mut(&mut self, m: &mut syn::ItemMod) {
        let saved = (self.dir.clone(), self.file_dir.clone());
        if m.content.is_none() {
            let (path, children) = match child_file(m, &self.dir, &self.file_dir) {
                Ok(x) => x,
                Err(e) => return self.fail(e),
            };
            let file = match parse(&path) {
                Ok(f) => f,
                Err(e) => return self.fail(e),
            };
            self.files.push(path.clone());
            m.attrs.extend(file.attrs);
            m.content = Some((Default::default(), file.items));
            self.file_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            self.dir = children;
        } else {
            self.dir = self.dir.join(m.ident.to_string());
        }
        if let Some((_, items)) = &mut m.content {
            items.retain(|i| compiled(item_attrs(i)));
        }
        syn::visit_mut::visit_item_mod_mut(self, m);
        (self.dir, self.file_dir) = saved;
    }

    fn visit_item_impl_mut(&mut self, i: &mut syn::ItemImpl) {
        i.items.retain(|x| compiled(impl_attrs(x)));
        syn::visit_mut::visit_item_impl_mut(self, i);
    }

    fn visit_item_trait_mut(&mut self, i: &mut syn::ItemTrait) {
        i.items.retain(|x| compiled(trait_attrs(x)));
        syn::visit_mut::visit_item_trait_mut(self, i);
    }

    fn visit_block_mut(&mut self, b: &mut syn::Block) {
        b.stmts.retain(|s| compiled(stmt_attrs(s)));
        syn::visit_mut::visit_block_mut(self, b);
    }

    fn visit_expr_match_mut(&mut self, m: &mut syn::ExprMatch) {
        m.arms.retain(|a| compiled(&a.attrs));
        syn::visit_mut::visit_expr_match_mut(self, m);
    }

    /// An arm as rustfmt may write it either way: its body a block of one expression unwrapped, its
    /// comma dropped.
    fn visit_arm_mut(&mut self, a: &mut syn::Arm) {
        if let syn::Expr::Block(b) = &*a.body {
            if let ([], None, [syn::Stmt::Expr(e, None)]) = (&b.attrs[..], &b.label, &b.block.stmts[..]) {
                a.body = Box::new(e.clone());
            }
        }
        a.comma = None;
        syn::visit_mut::visit_arm_mut(self, a);
    }

    fn visit_fields_named_mut(&mut self, f: &mut syn::FieldsNamed) {
        f.named = std::mem::take(&mut f.named).into_iter().filter(|x| compiled(&x.attrs)).collect();
        syn::visit_mut::visit_fields_named_mut(self, f);
    }

    fn visit_item_enum_mut(&mut self, e: &mut syn::ItemEnum) {
        e.variants = std::mem::take(&mut e.variants).into_iter().filter(|x| compiled(&x.attrs)).collect();
        syn::visit_mut::visit_item_enum_mut(self, e);
    }

    fn visit_macro_mut(&mut self, m: &mut syn::Macro) {
        // (The macro itself an include: its argument replaced; else the includes within its tokens.)
        let name = m.path.segments.last().map(|s| s.ident.to_string());
        m.tokens = if INCLUDES.contains(&name.as_deref().unwrap_or("")) {
            self.included(m.tokens.clone())
        } else {
            self.includes(m.tokens.clone())
        };
        syn::visit_mut::visit_macro_mut(self, m);
    }
}

/// Feeds `ts` to `h` token by token (their kinds, text and grouping, never their layout), doc
/// attributes (`#[doc = …]`, `#![doc = …]`: doc comments) and the trailing comma of a group `within`
/// left out where it means nothing: in `[]` and `{}`, and in a `()` holding another comma (`(x,)`, a
/// tuple, isn't `(x)`).
fn feed(ts: TokenStream, h: &mut xxhash_rust::xxh3::Xxh3, within: Delimiter) {
    let t: Vec<TokenTree> = ts.into_iter().collect();
    let commas = t.iter().filter(|x| matches!(x, TokenTree::Punct(p) if p.as_char() == ',')).count();
    let drop_last = match within {
        Delimiter::Bracket | Delimiter::Brace => true,
        Delimiter::Parenthesis => commas > 1,
        Delimiter::None => false,
    };
    let mut i = 0;
    while i < t.len() {
        if let TokenTree::Punct(p) = &t[i] {
            if p.as_char() == '#' {
                let bang = matches!(t.get(i + 1), Some(TokenTree::Punct(q)) if q.as_char() == '!');
                let at = i + 1 + bang as usize;
                if let Some(TokenTree::Group(g)) = t.get(at) {
                    let doc = g.delimiter() == Delimiter::Bracket
                        && matches!(g.stream().into_iter().next(), Some(TokenTree::Ident(d)) if d == "doc");
                    if doc {
                        i = at + 1;
                        continue;
                    }
                }
            }
        }
        match &t[i] {
            TokenTree::Punct(p) if p.as_char() == ',' && i + 1 == t.len() && drop_last => {}
            TokenTree::Ident(x) => {
                h.update(b"i");
                h.update(x.to_string().as_bytes());
                h.update(b"\0");
            }
            TokenTree::Punct(p) => {
                h.update(b"p");
                h.update(p.as_char().to_string().as_bytes());
                h.update(if p.spacing() == Spacing::Joint { b"j" } else { b"a" });
            }
            TokenTree::Literal(l) => {
                h.update(b"l");
                h.update(l.to_string().as_bytes());
                h.update(b"\0");
            }
            TokenTree::Group(g) => {
                h.update(match g.delimiter() {
                    Delimiter::Parenthesis => b"(",
                    Delimiter::Brace => b"{",
                    Delimiter::Bracket => b"[",
                    Delimiter::None => b"_",
                });
                feed(g.stream(), h, g.delimiter());
                h.update(b"}");
            }
        }
        i += 1;
    }
}

/// The code of each module `src/inputs/mod.rs` declares, by name: its items and its modules', and
/// all they name; the gate's (`gate`) with every item of `mod.rs` too. An error where an item of
/// `mod.rs` but the registry's names a child module (the cut, the module docs).
pub fn codes(w: &World) -> Result<Vec<(String, String)>, String> {
    let top = vec![INPUTS.to_string()];
    let module = &w.crates[INPUTS].modules[&top];
    let mut named: Vec<(Id, Option<String>)> =
        w.scoped(INPUTS, &top, &module.scope).into_iter().map(|f| (f, None)).collect();
    for (i, x) in module.items.iter().enumerate() {
        let id = (INPUTS.to_string(), top.clone(), i);
        if !w.registry(&id) {
            named.extend(w.edges(&id).into_iter().map(|f| (f, x.names.first().cloned())));
        }
    }
    if let Some(((_, m, _), by)) = named.iter().find(|(f, _)| w.in_child(f)) {
        return Err(format!(
            "src/inputs/mod.rs: {} names {}, a child module's, and only the registry may ({}; build/checks_code.rs)",
            by.as_deref().map_or("a `use`".to_string(), |n| format!("`{n}`")),
            m.join("::"),
            REGISTRY.join(", ")
        ));
    }
    let mut out = Vec::new();
    for x in &module.children {
        let mut start = w.under(INPUTS, &[top.clone(), vec![x.clone()]].concat());
        if x == "gate" {
            start.extend(w.all(INPUTS, &top));
        }
        out.push((x.clone(), w.digest(start, &[])));
    }
    Ok(out)
}
