//! What each gate unit's checks are, as code (docs/inputs.md §4.3, task #1467): a digest of the code
//! a unit's checks compile to, which `inputs::check_key` and a unit's check state
//! (`Listed::derived_by`) name in place of a typed version. build.rs makes one for every module
//! `src/inputs/mod.rs` declares and one for the gate, and prints each held digest of the typed
//! versions left (the regions' `RESOLUTION`, elevation's `Step::rule`) that no longer matches the
//! code it covers, with the value to set (`stale_codes`); the crate's tests compile this file in too
//! (lib.rs), and hold those digests to that code.
//!
//! **The code.** The inputs crate and every crate its manifest's dependencies reach by path (each
//! manifest's `[dependencies]` and `[target.….dependencies]`, in any form Cargo takes, and a
//! registry's crate the workspace patches to a path, `[patch.…]` and `[replace]`; a form this
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
//! - a type's impls in any module of these crates (one in a function's body too, hoisted): an
//!   impl's header once its type is reached (and its trait, for a trait of these crates; for an
//!   impl of a crates.io trait on a crates.io type, a type of these crates in their generic
//!   arguments, `impl From<Tool> for String`, else its module); a method of it once its header is
//!   and its name is called anywhere reached (`.m`, `Type::m`, `Self::m`, `<T as Tr>::m`), every
//!   method of an impl of a crates.io trait (`Display`, `Drop`, `Deserialize`: called by code not
//!   seen);
//! - when any item of a module is reached, its item macros, its traits and those its `use`s and
//!   globs bring in (what method calls there may call: `use det::Det` and `x.dcos()`; a glob's
//!   module's traits, its re-exports and `use`s among them: `use super::*`), and its globs' enums.
//!
//! A `use` names nothing itself: what a name it brings in stands for is taken where the name is
//! used.
//!
//! The gate's code is `gate`'s items and every item of `mod.rs`, with all they name. One cut: the
//! registry's items (`REGISTRY`) reach none of `mod.rs`'s child modules, else every unit's code would
//! hold every other's; any other item of `mod.rs` naming one fails the build, and so does a unit's
//! code holding a registry item.
//!
//! **What it doesn't see:** code of crates from crates.io (their versions: only through Cargo.lock,
//! not here), what their macros and derives expand to (serde's, and the impls they make), the
//! names in a file `include!`d
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
const REGISTRY: [&str; 7] = ["UNITS", "REGIONS", "HERITAGE", "MATCHES", "ELEVATION", "SOURCES", "checks"];

/// How deep `use`s, re-exports and globs are followed: past it, a cycle.
const DEPTH: usize = 12;

/// Every crate read, by its name, and every file read (build.rs reruns when one changes).
pub struct World {
    crates: BTreeMap<String, Crate>,
    pub files: Vec<PathBuf>,
    impls: Impls,
}

/// A crate: its modules by path (`[]` its root), and the crates its code may name (its path
/// dependencies: their names in code, the crates they are).
struct Crate {
    modules: BTreeMap<Vec<String>, Module>,
    deps: BTreeMap<String, String>,
}

/// A module: its items, in order, its child modules' names, and its `use`s.
#[derive(Default)]
struct Module {
    items: Vec<Item>,
    children: BTreeSet<String>,
    scope: Scope,
}

/// `use`s: each name in scope with the paths it stands for, and the globs' paths.
#[derive(Default)]
struct Scope {
    uses: BTreeMap<String, Vec<Vec<String>>>,
    globs: Vec<Vec<String>>,
}

/// An item: what it is, the names it's found by, its digest, and what it names (`Names`) and the
/// `use`s within it.
struct Item {
    kind: Kind,
    names: Vec<String>,
    digest: u128,
    holds: Names,
    scope: Scope,
}

#[derive(Clone, PartialEq)]
enum Kind {
    /// A struct, enum, union or type alias.
    Type,
    Trait,
    MacroRules,
    /// An impl's header: its type's path and its trait's, and the paths of the types in their
    /// generic arguments (`Foo` of `impl From<Foo> for String`).
    Impl {
        ty: Option<Vec<String>>,
        tr: Option<Vec<String>>,
        args: Vec<Vec<String>>,
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
struct Names {
    idents: BTreeSet<String>,
    paths: BTreeSet<Vec<String>>,
    calls: BTreeSet<String>,
    macros: BTreeSet<Vec<String>>,
}

/// An item's place: its crate, its module, its index there.
type Id = (String, Vec<String>, usize);

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
                        Kind::Impl { ty, tr, args } => {
                            let of = |p: &Option<Vec<String>>, want: Kind| -> Vec<Id> {
                                let Some(p) = p else { return Vec::new() };
                                let places = self.place(c, m, None, p, false, 0);
                                self.ids(places).into_iter().filter(|j| self.item(j).kind == want).collect()
                            };
                            let mut tys = Some(of(ty, Kind::Type)).filter(|v| !v.is_empty());
                            let trs = tr.as_ref().map(|_| of(tr, Kind::Trait));
                            // (Neither its type nor its trait placed among these crates': reached with a
                            // type of these crates in their generic arguments (`impl From<Foo> for
                            // String`: only a `Foo` calls it), else with its module.)
                            if tys.is_none() && trs.as_ref().is_none_or(|t| t.is_empty()) {
                                let inner: Vec<Id> =
                                    args.iter().flat_map(|a| of(&Some(a.clone()), Kind::Type)).collect();
                                if inner.is_empty() {
                                    x.loose.entry((c.clone(), m.clone())).or_default().push(id.clone());
                                } else {
                                    tys = Some(inner);
                                }
                            }
                            for t in tys.iter().flatten().chain(trs.iter().flatten()) {
                                x.of.entry(t.clone()).or_default().push(id.clone());
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
    /// out (the guards').
    fn digest(&self, start: Vec<Id>, leave_out: &BTreeSet<Id>) -> String {
        self.digest_of(&self.closure(start, leave_out))
    }

    /// The digest of items `items`.
    fn digest_of(&self, items: &BTreeSet<Id>) -> String {
        let mut h = xxhash_rust::xxh3::Xxh3::new();
        for id in items {
            let (c, m, _) = id;
            h.update(format!("{c}::{}#{:032x}\n", m.join("::"), self.item(id).digest).as_bytes());
        }
        format!("{:032x}", h.digest128())
    }

    /// The items `start` names and every item they name, transitively (the module docs), with the
    /// registry's cut; `leave_out` never taken.
    fn closure(&self, start: Vec<Id>, leave_out: &BTreeSet<Id>) -> BTreeSet<Id> {
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
                    if matches!(&self.item(j).kind, Kind::Method(h, _) if seen.contains(&(j.0.clone(), j.1.clone(), *h)))
                    {
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
        let mut found = self.named_by(id);
        if self.registry(id) {
            found.retain(|f| !self.in_child(f));
        }
        found
    }

    /// The items item `id` names, the registry's cut aside (`edges`).
    fn named_by(&self, id: &Id) -> Vec<Id> {
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
                    Place::Module(gc, gm) => v.extend(self.traits_in(&gc, &gm, 0)),
                }
            }
        }
        v
    }

    /// The traits a glob of module `m` of crate `c` brings into scope: its own, those its `use`s
    /// bring in (a re-export, or a parent's `use` that a child's `use super::*` takes), and its globs'.
    fn traits_in(&self, c: &str, m: &[String], depth: usize) -> Vec<Id> {
        let Some(module) = self.crates.get(c).and_then(|k| k.modules.get(m)) else { return Vec::new() };
        if depth > DEPTH {
            return Vec::new();
        }
        let trait_ = |j: &Id| self.item(j).kind == Kind::Trait;
        let mut v: Vec<Id> = self.all(c, m).into_iter().filter(trait_).collect();
        for p in module.scope.uses.values().flatten() {
            v.extend(self.ids(self.place(c, m, None, p, true, 0)).into_iter().filter(trait_));
        }
        for g in &module.scope.globs {
            for place in self.place(c, m, None, g, true, 0) {
                if let Place::Module(gc, gm) = place {
                    v.extend(self.traits_in(&gc, &gm, depth + 1));
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
    fn all(&self, c: &str, m: &[String]) -> Vec<Id> {
        let n = self.crates.get(c).and_then(|k| k.modules.get(m)).map_or(0, |x| x.items.len());
        (0..n).map(|i| (c.to_string(), m.to_vec(), i)).collect()
    }

    /// Every item of the modules under `m` of crate `c` (`m` among them).
    fn under(&self, c: &str, m: &[String]) -> Vec<Id> {
        let k = &self.crates[c];
        k.modules.keys().filter(|p| p.starts_with(m)).flat_map(|p| self.all(c, p)).collect()
    }

    /// The items of module `m` of crate `c` found by one of `names` (not methods nor impls: those by
    /// their types and calls).
    fn named(&self, c: &str, m: &[String], names: &[&str]) -> Vec<Id> {
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
/// in code and its folder. A `workspace = true` one is the workspace's (`[workspace.dependencies]`);
/// one the workspace patches to a path (`[patch.…]`, `[replace]`) is that path's (the workspace's
/// manifest added to `files` when read).
fn path_deps(dir: &Path, files: &mut Vec<PathBuf>) -> Result<Vec<(String, PathBuf)>, String> {
    let toml = dir.join("Cargo.toml");
    // (The workspace's folder, its dependencies and its patches, read once if needed.)
    let mut ws: Option<Option<(PathBuf, Vec<Dep>, Vec<Dep>)>> = None;
    let mut workspace = |files: &mut Vec<PathBuf>| -> Result<Option<(PathBuf, Vec<Dep>, Vec<Dep>)>, String> {
        if ws.is_none() {
            let abs = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
            let root = abs.ancestors().skip(1).find(|a| {
                std::fs::read_to_string(a.join("Cargo.toml"))
                    .is_ok_and(|t| t.lines().any(|l| strip_comment(l) == "[workspace]"))
            });
            ws = Some(match root {
                Some(r) => {
                    let m = r.join("Cargo.toml");
                    files.push(m.clone());
                    Some((r.to_path_buf(), deps_of(&m, Tables::Workspace)?, deps_of(&m, Tables::Patch)?))
                }
                None => None,
            });
        }
        Ok(ws.clone().unwrap())
    };
    let mut out = Vec::new();
    for d in deps_of(&toml, Tables::Deps)? {
        let (d, base) = if d.workspace {
            let Some((root, table, _)) = workspace(files)? else {
                return Err(format!("{}: {} is the workspace's, and no workspace holds it", toml.display(), d.name));
            };
            let w = table.iter().find(|w| w.name == d.name).ok_or_else(|| {
                format!("{}: {} is the workspace's, and its [workspace.dependencies] hasn't it", toml.display(), d.name)
            })?;
            (Dep { name: d.name.clone(), package: d.package.clone().or(w.package.clone()), ..w.clone() }, root)
        } else {
            (d, dir.to_path_buf())
        };
        let (path, base) = match &d.path {
            Some(p) => (p.clone(), base),
            None => {
                // (A crate from a registry, unless the workspace patches it to a path.)
                let package = d.package.clone().unwrap_or(d.name.clone());
                let patched = workspace(files)?.and_then(|(root, _, patches)| {
                    patches.into_iter().find(|x| x.name == package).and_then(|x| x.path).map(|p| (p, root))
                });
                match patched {
                    Some(x) => x,
                    None => continue,
                }
            }
        };
        out.push((d.name.replace('-', "_"), base.join(path)));
    }
    Ok(out)
}

/// The tables of a manifest that `deps_of` reads.
#[derive(Clone, Copy, PartialEq)]
enum Tables {
    /// `[dependencies]` and `[target.….dependencies]`.
    Deps,
    /// `[workspace.dependencies]`.
    Workspace,
    /// `[patch.<registry>]` and `[replace]` (a `name:version` key by its name).
    Patch,
}

/// The dependencies a manifest's tables `which` declare, each as a table of its own too
/// (`[dependencies.x]`). A line there of a form this doesn't read is an error.
fn deps_of(toml: &Path, which: Tables) -> Result<Vec<Dep>, String> {
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
            let first = |k: &str| keys.first().is_some_and(|s| s == k);
            let deps_at = match which {
                Tables::Workspace => (keys.len() >= 2 && first("workspace") && keys[1] == "dependencies").then_some(2),
                Tables::Deps if first("dependencies") => Some(1),
                Tables::Deps => (keys.len() >= 3 && first("target") && keys[2] == "dependencies").then_some(3),
                Tables::Patch if first("replace") => Some(1),
                Tables::Patch => (keys.len() >= 2 && first("patch")).then_some(2),
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
                let mut name = unquote(&name).unwrap_or(name);
                // (`[replace]`'s keys are package ID specs, `"foo:1.2.3"` or
                // `"https://…#foo:1.2.3"`: crate foo, after the last '#', before ':' or '@'.)
                if which == Tables::Patch {
                    let spec = name.rsplit('#').next().unwrap_or_default();
                    name = spec.split([':', '@']).next().unwrap_or_default().to_string();
                }
                let mut d = Dep { name, ..Dep::default() };
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
            syn::Item::Impl(x) => push_impl(&mut m, x),
            _ => {
                let (kind, names) = kind_names(it);
                let scope = local_uses(|v| v.visit_item_mut(&mut it.clone()));
                m.items.push(item(kind, names, it.to_token_stream(), scope));
                for x in nested_impls(|v| v.visit_item_mut(&mut it.clone())) {
                    push_impl(&mut m, &x);
                }
            }
        }
    }
    out.insert(path, m);
}

/// Impl `x`, into module `m`: its header, then each method as an item of its own, then the impls in
/// its methods' bodies (hoisted: their type may be the module's).
fn push_impl(m: &mut Module, x: &syn::ItemImpl) {
    let ty = type_path(&x.self_ty);
    let tr = x.trait_.as_ref().map(|(_, p, _)| path_segments(p));
    let mut args = Vec::new();
    type_paths(&x.self_ty, &mut args);
    if let Some((_, p, _)) = &x.trait_ {
        path_args(p, &mut args);
    }
    let mut head = x.clone();
    head.items.retain(|i| !matches!(i, syn::ImplItem::Fn(_)));
    let at = m.items.len();
    let scope = local_uses(|v| v.visit_item_impl_mut(&mut head.clone()));
    m.items.push(item(Kind::Impl { ty, tr, args }, Vec::new(), head.to_token_stream(), scope));
    let mut nested = Vec::new();
    for i in &x.items {
        if let syn::ImplItem::Fn(f) = i {
            let scope = local_uses(|v| v.visit_impl_item_fn_mut(&mut f.clone()));
            m.items.push(item(Kind::Method(at, f.sig.ident.to_string()), Vec::new(), f.to_token_stream(), scope));
            nested.extend(nested_impls(|v| v.visit_impl_item_fn_mut(&mut f.clone())));
        }
    }
    for n in nested {
        push_impl(m, &n);
    }
}

/// The impls within an item's bodies (a function's own), as `visit` visits it: not those in an
/// impl it holds, which `push_impl` takes with that impl.
fn nested_impls(visit: impl FnOnce(&mut NestedImpls)) -> Vec<syn::ItemImpl> {
    let mut v = NestedImpls(Vec::new());
    visit(&mut v);
    v.0
}

struct NestedImpls(Vec<syn::ItemImpl>);

impl VisitMut for NestedImpls {
    fn visit_item_impl_mut(&mut self, i: &mut syn::ItemImpl) {
        self.0.push(i.clone());
    }

    fn visit_item_mod_mut(&mut self, _: &mut syn::ItemMod) {}
}

/// A path's segments (a leading `::` one of its own).
fn path_segments(p: &syn::Path) -> Vec<String> {
    let mut v: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
    if p.leading_colon.is_some() {
        v.insert(0, "::".into());
    }
    v
}

/// The path of type `t` (through references, parentheses), if it's one.
fn type_path(mut t: &syn::Type) -> Option<Vec<String>> {
    loop {
        match t {
            syn::Type::Reference(r) => t = &r.elem,
            syn::Type::Paren(p) => t = &p.elem,
            syn::Type::Group(g) => t = &g.elem,
            syn::Type::Path(p) if p.qself.is_none() => return Some(path_segments(&p.path)),
            _ => return None,
        }
    }
}

/// Into `out`, the paths of every type within type `t`, `t`'s own among them.
fn type_paths(t: &syn::Type, out: &mut Vec<Vec<String>>) {
    match t {
        syn::Type::Reference(r) => type_paths(&r.elem, out),
        syn::Type::Paren(p) => type_paths(&p.elem, out),
        syn::Type::Group(g) => type_paths(&g.elem, out),
        syn::Type::Slice(x) => type_paths(&x.elem, out),
        syn::Type::Array(x) => type_paths(&x.elem, out),
        syn::Type::Ptr(x) => type_paths(&x.elem, out),
        syn::Type::Tuple(x) => x.elems.iter().for_each(|e| type_paths(e, out)),
        syn::Type::Path(p) => {
            out.push(path_segments(&p.path));
            path_args(&p.path, out);
        }
        _ => {}
    }
}

/// Into `out`, the paths of the types in path `p`'s generic arguments.
fn path_args(p: &syn::Path, out: &mut Vec<Vec<String>>) {
    for s in &p.segments {
        if let syn::PathArguments::AngleBracketed(a) = &s.arguments {
            for g in &a.args {
                if let syn::GenericArgument::Type(t) = g {
                    type_paths(t, out);
                }
            }
        }
    }
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
        // (A path, one segment too: serde's `with = "m"` names a module.)
        if segs.iter().all(|s| word(s)) {
            n.calls.extend(segs[1..].iter().map(|s| s.to_string()));
            n.paths.insert(segs.iter().map(|s| s.to_string()).collect());
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
            "the gate's mod.rs: {} names {}, a child module's, and only the registry may ({}): move it into a child \
             module; add a name to REGISTRY (build/checks_code.rs) only for a unit's registration",
            by.as_deref().map_or("a `use`".to_string(), |n| format!("`{n}`")),
            m.join("::"),
            REGISTRY.join(", ")
        ));
    }
    // (The units: the modules the registry names.)
    let units: BTreeSet<String> = (0..module.items.len())
        .map(|i| (INPUTS.to_string(), top.clone(), i))
        .filter(|id| w.registry(id))
        .flat_map(|id| w.named_by(&id))
        .filter(|f| w.in_child(f))
        .map(|(_, m, _)| m[1].clone())
        .collect();
    let mut out = Vec::new();
    for x in &module.children {
        let mut start = w.under(INPUTS, &[top.clone(), vec![x.clone()]].concat());
        if x == "gate" {
            start.extend(w.all(INPUTS, &top));
        }
        let items = w.closure(start, &BTreeSet::new());
        // (A unit's code holding the registry would hold every unit's through it but for the cut.)
        if let Some(r) = items.iter().find(|j| units.contains(x) && w.registry(j)) {
            return Err(format!(
                "{x}'s code reaches the gate's registry item `{}`: name what it needs from a module of its own",
                w.item(r).names.join(", ")
            ));
        }
        out.push((x.clone(), w.digest_of(&items)));
    }
    Ok(out)
}

/// What the guards' code leaves out (`resolution_code`, `rule_codes`): modules that move bytes, wait,
/// retry, pace or count, and compute nothing a resolution or a measure depends on (their bytes come
/// through whole or not at all), so a change there checks nothing again. Each, with everything under
/// it, but `PLUMBING_KEPT`: jobrt's resume log is the codec a resumed resolution's pieces are read
/// back through. The units' own codes keep them all.
const PLUMBING: [&str; 15] = [
    "jobrt",
    "store::nasio",
    "store::nasfs",
    "store::nas",
    "store::waits",
    "store::aimd",
    "store::deadline",
    "store::retry",
    "store::spawn",
    "store::iopool",
    "store::clock",
    "store::cachefile",
    "buildstore::fetch",
    "inputs::inputs::pace",
    "inputs::inputs::regions::progress",
];

const PLUMBING_KEPT: [&str; 1] = ["jobrt::resume_log"];

/// The items of `PLUMBING` (each module there, asserted).
fn plumbing(w: &World) -> BTreeSet<Id> {
    let split = |s: &str| -> (String, Vec<String>) {
        let mut v = s.split("::").map(String::from);
        (v.next().unwrap(), v.collect())
    };
    let kept: Vec<(String, Vec<String>)> = PLUMBING_KEPT.iter().map(|k| split(k)).collect();
    let mut out = BTreeSet::new();
    for p in PLUMBING {
        let (c, m) = split(p);
        assert!(w.crates.get(&c).is_some_and(|k| k.modules.contains_key(&m)), "PLUMBING: no module {p}");
        out.extend(
            w.under(&c, &m)
                .into_iter()
                .filter(|(ic, im, _)| !kept.iter().any(|(kc, km)| kc == ic && im.starts_with(km))),
        );
    }
    out
}

fn path(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| s.to_string()).collect()
}

/// The method `name` of the impls in module `m` of the inputs crate of a type named `ty`.
fn method(w: &World, m: &[String], ty: &str, name: &str) -> Vec<Id> {
    let items = &w.crates[INPUTS].modules[m].items;
    let of_ty =
        |h: usize| matches!(&items[h].kind, Kind::Impl { ty: Some(p), .. } if p.last().is_some_and(|l| l == ty));
    let v: Vec<Id> = (0..items.len())
        .filter(|&i| matches!(&items[i].kind, Kind::Method(h, n) if n == name && of_ty(*h)))
        .map(|i| (INPUTS.to_string(), m.to_vec(), i))
        .collect();
    assert!(!v.is_empty(), "{}: {ty}::{name}", m.join("::"));
    v
}

/// The code the regions' `RESOLUTION` covers: the items of their resolution's modules (`resolve`,
/// `tiles`, `geom`, `land`, `stored`, `shapes`, `data`), what of `regions/mod.rs` shapes a
/// resolution (a resolution's making and its key, `Regions::made`, `outcome_key`; its budgets,
/// `max_mb`, `op_budget_s`; its data, `data_from`, `Prepared::land_open`; a stored one taken,
/// `accepted_resolution`, `Recorded`), and all they name but `PLUMBING`. Held to
/// `regions::RESOLUTION_CODE` (`stale_codes`).
pub fn resolution_code(w: &World) -> String {
    let regions = path(&[INPUTS, "regions"]);
    let mut start: Vec<Id> = ["resolve", "tiles", "geom", "land", "stored", "shapes", "data"]
        .iter()
        .flat_map(|m| w.under(INPUTS, &[regions.clone(), vec![m.to_string()]].concat()))
        .collect();
    for (ty, m) in [
        ("Regions", "made"),
        ("Regions", "outcome_key"),
        ("Regions", "accepted_resolution"),
        ("Regions", "data_from"),
        ("Prepared", "land_open"),
    ] {
        start.extend(method(w, &regions, ty, m));
    }
    let named = w.named(INPUTS, &regions, &["max_mb", "op_budget_s", "Recorded"]);
    assert_eq!(named.len(), 3, "regions/mod.rs: max_mb, op_budget_s, Recorded");
    start.extend(named);
    w.digest(start, &plumbing(w))
}

/// The code each elevation measure step's rule (`Step::rule`) covers, by the step's name: its
/// measuring code and all it names (`headers_of`, `presence_of`, `hollow_of`), `Step::rule` itself
/// and `PLUMBING` left out. Held to `elevation::RULE_CODES` (`stale_codes`).
pub fn rule_codes(w: &World) -> Vec<(&'static str, String)> {
    let m = path(&[INPUTS, "elevation"]);
    let mut leave_out = plumbing(w);
    leave_out.extend(method(w, &m, "Step", "rule"));
    [("headers", "headers_of"), ("presence", "presence_of"), ("hollow", "hollow_of")]
        .into_iter()
        .map(|(step, root)| {
            let start = w.named(INPUTS, &m, &[root]);
            assert_eq!(start.len(), 1, "elevation's {root}");
            (step, w.digest(start, &leave_out))
        })
        .collect()
}

/// The held digests that no longer match the code they cover (#1578), each as what to set:
/// `regions::RESOLUTION_CODE` (`resolution_code`) and each step's in `elevation::RULE_CODES`
/// (`rule_codes`), as their files' text holds them, `regions` (src/inputs/regions/mod.rs) and
/// `elevation` (src/inputs/elevation.rs). build.rs prints each as a warning and writes them to
/// `$OUT_DIR/stale_codes` (tools/app/guards.sh fails on one, saying it); a test holds them to none.
pub fn stale_codes(w: &World, regions: &str, elevation: &str) -> Vec<String> {
    // (The quoted value after `before` in `text`.)
    let held = |text: &str, before: &str| -> String {
        text.find(before)
            .map(|i| &text[i + before.len()..])
            .and_then(|rest| rest.find('"').map(|j| format!("\"{}\"", &rest[..j])))
            .unwrap_or_else(|| "missing".into())
    };
    let mut out = Vec::new();
    let now = format!("\"{}\"", resolution_code(w));
    let was = held(regions, "pub const RESOLUTION_CODE: &str = \"");
    if was != now {
        out.push(format!(
            "regions::RESOLUTION_CODE is {was}, and the code it covers is now {now}: set it to {now} (and bump \
             regions::RESOLUTION if what an unchanged region resolves to, or its problems' words, changes)"
        ));
    }
    let rules = elevation.find("pub const RULE_CODES").map_or("", |i| {
        let s = &elevation[i..];
        &s[..s.find("];").unwrap_or(s.len())]
    });
    for (step, now) in rule_codes(w) {
        let now = format!("\"{now}\"");
        let was = held(rules, &format!("(\"{step}\", \""));
        if was != now {
            out.push(format!(
                "elevation::RULE_CODES' {step} is {was}, and its measuring code is now {now}: set it to {now} (and \
                 raise the step's Step::rule if what it measures changes)"
            ));
        }
    }
    out
}
