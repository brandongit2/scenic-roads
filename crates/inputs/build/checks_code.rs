//! What each gate unit's checks are, as code (docs/inputs.md §4.3, task #1467): a digest of the code
//! a unit's checks compile to, which `inputs::check_key` and a unit's check state
//! (`Listed::derived_by`) name in place of a typed version. build.rs makes one for every module
//! `src/inputs/mod.rs` declares and one for the gate; the crate's tests compile this file in too
//! (lib.rs), and hold the typed versions left (the regions' `RESOLUTION`, elevation's `Step::rule`)
//! to the code they cover.
//!
//! **The code.** Every crate the inputs crate reaches by its path dependencies is read whole, as the
//! app compiles it: each file with its child modules (`mod x;`, `#[path]` too) and the bytes of every
//! file it compiles in (`include_str!`, `include_bytes!`, `include!`, by a plain string literal, or a
//! build script's output by `concat!(env!("OUT_DIR"), …)`, which counts as itself: anything else is
//! an error, never passed over). Left out: comments and doc comments, layout and what rustfmt changes
//! (a trailing comma in `[]`, `{}` or a `()` holding another comma, a match arm's body put in braces
//! or taken out), and what the app's build doesn't compile: items under a `cfg` false without tests
//! and the `testkit` feature, and `#[test]` functions.
//!
//! **What a unit's code is.** Each item (a function, a type with its impls in the same module, a
//! method, a constant, a `use`) has its own digest. A unit's code is the items of its module and its
//! modules', and every item those name, transitively, across modules and crates: a path
//! (`super::credits::parse_description`, `crate::dem::declared::WORLD`, `roadcore::geo::…`, through
//! the module's `use` names and re-exports), and a name of another item of the same module (a method
//! called on `self` among them). A path naming a module (or a glob `use`) takes the module's every
//! item. The gate's code is `gate`'s items and every item of `mod.rs`, so with all they name. One cut:
//! `mod.rs`'s items naming its own child modules (the registry: `UNITS`, `checks`, the units'
//! statics) reach no unit's code, else every unit's would hold every other's.
//!
//! **What it doesn't see:** a method called on a value of a type whose impl is in another module
//! than the type (and not named otherwise); code of crates from crates.io (their versions: only
//! through Cargo.lock, not here), and of macros' expansions (serde's derives); files read at run time
//! (a unit's `Checks::context` names those). A name it can't place (a local shadowing nothing it
//! knows) names nothing; a name it can place over-approximately (a local variable named like an item
//! of its module) takes that item too, which can only check a unit again.

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::visit_mut::VisitMut;

/// Every crate read, by its name in code (`roadcore`), and every file read (build.rs reruns when one
/// changes).
pub struct World {
    pub crates: BTreeMap<String, Crate>,
    pub files: Vec<PathBuf>,
}

/// A crate: its modules by path (`[]` its root), and the crates its code may name (its path
/// dependencies, by their names in code).
pub struct Crate {
    pub modules: BTreeMap<Vec<String>, Module>,
    pub deps: BTreeSet<String>,
}

/// A module: its items, in order, and its child modules' names.
#[derive(Default)]
pub struct Module {
    pub items: Vec<Item>,
    pub children: BTreeSet<String>,
    /// Its `use` names: the name in scope, the path it stands for.
    pub uses: BTreeMap<String, Vec<String>>,
    /// Its glob `use`s' paths.
    pub globs: Vec<Vec<String>>,
}

/// An item: the names it's found by, its digest, the names and paths it holds.
pub struct Item {
    pub names: Vec<String>,
    pub digest: u128,
    pub idents: BTreeSet<String>,
    pub paths: Vec<Vec<String>>,
}

/// An item's place: its crate, its module, its index there.
pub type Id = (String, Vec<String>, usize);

impl World {
    /// The crate whose folder is `dir` (`crates/inputs`) and every crate its path dependencies reach.
    pub fn read(dir: &Path) -> Result<World, String> {
        let mut w = World { crates: BTreeMap::new(), files: Vec::new() };
        let mut todo = vec![dir.to_path_buf()];
        let mut seen = BTreeSet::new();
        while let Some(d) = todo.pop() {
            let name = crate_name(&d)?;
            if !seen.insert(name.clone()) {
                continue;
            }
            let toml = d.join("Cargo.toml");
            let deps = path_deps(&toml)?;
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
            let mut names = BTreeSet::new();
            for (n, p) in deps {
                names.insert(n);
                todo.push(d.join(p));
            }
            w.crates.insert(name, Crate { modules, deps: names });
        }
        Ok(w)
    }

    /// The digest of the items `start` names and every item they name (`closure`).
    pub fn digest(&self, start: Vec<Id>) -> String {
        let mut h = xxhash_rust::xxh3::Xxh3::new();
        for (c, m, i) in &self.closure(start) {
            h.update(format!("{c}::{}#{:032x}\n", m.join("::"), self.crates[c].modules[m].items[*i].digest).as_bytes());
        }
        format!("{:032x}", h.digest128())
    }

    /// The items `start` names and every item they name, transitively (the module docs), with the
    /// registry's cut.
    pub fn closure(&self, start: Vec<Id>) -> BTreeSet<Id> {
        let mut seen: BTreeSet<Id> = BTreeSet::new();
        let mut todo = start;
        while let Some(id) = todo.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let (c, m, i) = &id;
            let module = &self.crates[c].modules[m];
            let item = &module.items[*i];
            let mut found = Vec::new();
            for n in &item.idents {
                // (Another item of its module by name, or a name its module's `use`s bring in.)
                found.extend(
                    module
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, x)| x.names.contains(n))
                        .map(|(k, _)| (c.clone(), m.clone(), k)),
                );
                if let Some(p) = module.uses.get(n) {
                    found.extend(self.resolve(c, m, p, 0));
                }
            }
            for p in &item.paths {
                found.extend(self.resolve_relative(c, m, p));
            }
            for g in &module.globs {
                found.extend(self.resolve_relative(c, m, g).into_iter().flat_map(|(gc, gm, _)| self.all(&gc, &gm)));
            }
            // (The registry's cut: mod.rs's items reach none of its child modules' items.)
            let registry = c == "inputs" && m.as_slice() == ["inputs"];
            todo.extend(
                found.into_iter().filter(|(fc, fm, _)| !(registry && fc == c && fm.len() > 1 && fm[0] == "inputs")),
            );
        }
        seen
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

    /// The items of module `m` named `names`, and every item of a type named so (its impls).
    pub fn named(&self, c: &str, m: &[String], names: &[&str]) -> Vec<Id> {
        let module = &self.crates[c].modules[m];
        let ids: Vec<Id> = module
            .items
            .iter()
            .enumerate()
            .filter(|(_, x)| x.names.iter().any(|n| names.contains(&n.as_str())))
            .map(|(i, _)| (c.to_string(), m.to_vec(), i))
            .collect();
        ids
    }

    /// The items path `p`, as written in module `m` of crate `c`, names.
    fn resolve_relative(&self, c: &str, m: &[String], p: &[String]) -> Vec<Id> {
        let Some(first) = p.first() else { return Vec::new() };
        let k = &self.crates[c];
        let module = &k.modules[m];
        let (crate_, abs): (&str, Vec<String>) = match first.as_str() {
            "crate" | "$crate" => (c, p[1..].to_vec()),
            "self" => (c, [m, &p[1..]].concat()),
            "super" => {
                let ups = p.iter().take_while(|s| *s == "super").count();
                if ups > m.len() {
                    return Vec::new();
                }
                (c, [&m[..m.len() - ups], &p[ups..]].concat())
            }
            f if module.uses.contains_key(f) => {
                let mut v = self.resolve(c, m, &module.uses[f], 0);
                if p.len() > 1 {
                    // (A path through a `use` name: resolved from where that name points.)
                    let target = &module.uses[f];
                    v = self.resolve(c, m, &[target.as_slice(), &p[1..]].concat(), 0);
                }
                return v;
            }
            f if module.children.contains(f) => (c, [m, p].concat()),
            f if k.deps.contains(f) => return self.absolute(f, &p[1..], 0),
            _ => return Vec::new(),
        };
        self.absolute(crate_, &abs, 0)
    }

    /// A `use` path (`target`) of module `m`: resolved as a path written there.
    fn resolve(&self, c: &str, m: &[String], target: &[String], depth: usize) -> Vec<Id> {
        if depth > 8 {
            return Vec::new();
        }
        let Some(first) = target.first() else { return Vec::new() };
        let k = &self.crates[c];
        match first.as_str() {
            "crate" | "$crate" | "self" | "super" => self.resolve_relative(c, m, target),
            f if k.modules[m].children.contains(f) => self.absolute(c, &[m, target].concat(), depth + 1),
            f if k.deps.contains(f) => self.absolute(f, &target[1..], depth + 1),
            // (A uniform path from the crate's root: `use geo::Earth` in lib.rs.)
            _ if m.is_empty() => self.absolute(c, target, depth + 1),
            _ => Vec::new(),
        }
    }

    /// The items absolute path `p` names in crate `c`: the longest module it starts with, then the
    /// item it names there (or a `use` name of that module, a re-export), else that module whole.
    fn absolute(&self, c: &str, p: &[String], depth: usize) -> Vec<Id> {
        let Some(k) = self.crates.get(c) else { return Vec::new() };
        let mut n = p.len();
        while n > 0 && !k.modules.contains_key(&p[..n]) {
            n -= 1;
        }
        let m = &p[..n];
        let Some(name) = p.get(n) else { return self.all(c, m) };
        let named = self.named(c, m, &[name.as_str()]);
        if !named.is_empty() {
            return named;
        }
        if let Some(t) = k.modules[m].uses.get(name) {
            return self.resolve(c, m, &[t.as_slice(), &p[n + 1..]].concat(), depth + 1);
        }
        self.all(c, m)
    }
}

/// A crate's name in code, from its manifest.
fn crate_name(dir: &Path) -> Result<String, String> {
    let toml = std::fs::read_to_string(dir.join("Cargo.toml"))
        .map_err(|e| format!("read {}/Cargo.toml: {e}", dir.display()))?;
    let line = toml
        .lines()
        .find(|l| l.trim_start().starts_with("name"))
        .ok_or_else(|| format!("{}/Cargo.toml names no package", dir.display()))?;
    let name = line.split('"').nth(1).ok_or_else(|| format!("{}/Cargo.toml: its name isn't quoted", dir.display()))?;
    Ok(name.replace('-', "_"))
}

/// A manifest's path dependencies (not its dev- or build-dependencies): each one's name in code
/// and its folder, relative to the manifest's.
fn path_deps(toml: &Path) -> Result<Vec<(String, String)>, String> {
    let text = std::fs::read_to_string(toml).map_err(|e| format!("read {}: {e}", toml.display()))?;
    let mut out = Vec::new();
    let mut on = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            on = t == "[dependencies]";
            continue;
        }
        if !on || t.starts_with('#') {
            continue;
        }
        let (Some((name, rest)), true) = (t.split_once('='), t.contains("path")) else { continue };
        let Some(p) = rest.split("path").nth(1).and_then(|r| r.split('"').nth(1)) else { continue };
        out.push((name.trim().replace('-', "_"), p.to_string()));
    }
    Ok(out)
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
                let mut pairs = Vec::new();
                use_tree(&u.tree, Vec::new(), &mut pairs, &mut m.globs);
                // (Found by no name: a name it brings in is resolved through `uses` where it's used, so only
                // what that name stands for is taken, not every name of the `use`.)
                for (n, p) in pairs {
                    m.uses.insert(n, p);
                }
                m.items.push(item(Vec::new(), it.to_token_stream()));
            }
            syn::Item::Impl(x) => {
                let mut ty = Vec::new();
                if let syn::Type::Path(tp) = &*x.self_ty {
                    ty.extend(tp.path.segments.last().map(|s| s.ident.to_string()));
                }
                if let Some((_, tr, _)) = &x.trait_ {
                    ty.extend(tr.segments.last().map(|s| s.ident.to_string()));
                }
                // (Its header, then each method as an item of its own, found by its name or its type's.)
                let mut head = x.clone();
                head.items.retain(|i| !matches!(i, syn::ImplItem::Fn(_)));
                m.items.push(item(ty.clone(), head.to_token_stream()));
                for i in &x.items {
                    if let syn::ImplItem::Fn(f) = i {
                        let mut names = ty.clone();
                        names.push(f.sig.ident.to_string());
                        m.items.push(item(names, f.to_token_stream()));
                    }
                }
            }
            _ => {
                let names = item_name(it).into_iter().collect();
                m.items.push(item(names, it.to_token_stream()));
            }
        }
    }
    out.insert(path, m);
}

fn item_name(i: &syn::Item) -> Option<String> {
    use syn::Item::*;
    Some(
        match i {
            Const(x) => &x.ident,
            Enum(x) => &x.ident,
            Fn(x) => &x.sig.ident,
            Static(x) => &x.ident,
            Struct(x) => &x.ident,
            Trait(x) => &x.ident,
            TraitAlias(x) => &x.ident,
            Type(x) => &x.ident,
            Union(x) => &x.ident,
            Macro(x) => x.ident.as_ref()?,
            _ => return None,
        }
        .to_string(),
    )
}

/// A `use` tree's names in scope and the paths they stand for, and its globs.
fn use_tree(t: &syn::UseTree, prefix: Vec<String>, out: &mut Vec<(String, Vec<String>)>, globs: &mut Vec<Vec<String>>) {
    let with = |s: String| {
        let mut p = prefix.clone();
        p.push(s);
        p
    };
    match t {
        syn::UseTree::Path(p) => use_tree(&p.tree, with(p.ident.to_string()), out, globs),
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last() {
                    out.push((last.clone(), prefix.clone()));
                }
            } else {
                out.push((name.clone(), with(name)));
            }
        }
        syn::UseTree::Rename(r) => out.push((r.rename.to_string(), with(r.ident.to_string()))),
        syn::UseTree::Glob(_) => globs.push(prefix),
        syn::UseTree::Group(g) => g.items.iter().for_each(|i| use_tree(i, prefix.clone(), out, globs)),
    }
}

/// An item from its tokens: its digest (`feed`), and the names and paths it holds.
fn item(names: Vec<String>, ts: TokenStream) -> Item {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    feed(ts.clone(), &mut h, Delimiter::None);
    let (mut idents, mut paths) = (BTreeSet::new(), Vec::new());
    scan(ts, &mut idents, &mut paths);
    Item { names, digest: h.digest128(), idents, paths }
}

/// The names (one ident alone) and paths (`a::b…`, from where one starts) in `ts`.
fn scan(ts: TokenStream, idents: &mut BTreeSet<String>, paths: &mut Vec<Vec<String>>) {
    let t: Vec<TokenTree> = ts.into_iter().collect();
    let sep = |i: usize| {
        matches!((t.get(i), t.get(i + 1)), (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
            if a.as_char() == ':' && a.spacing() == Spacing::Joint && b.as_char() == ':')
    };
    let mut i = 0;
    while i < t.len() {
        match &t[i] {
            TokenTree::Group(g) => {
                scan(g.stream(), idents, paths);
                i += 1;
            }
            TokenTree::Ident(id) => {
                // (`$crate`: the macro's crate.)
                let dollar = i > 0 && matches!(&t[i - 1], TokenTree::Punct(p) if p.as_char() == '$');
                let after =
                    i >= 2 && sep(i - 2) || i >= 1 && matches!(&t[i - 1], TokenTree::Punct(p) if p.as_char() == '.');
                let mut p = vec![if dollar { format!("${id}") } else { id.to_string() }];
                let mut j = i + 1;
                while sep(j) {
                    let Some(TokenTree::Ident(n)) = t.get(j + 2) else { break };
                    p.push(n.to_string());
                    j += 3;
                }
                if !after {
                    if p.len() == 1 {
                        idents.insert(p.pop().unwrap());
                    } else {
                        idents.insert(p[0].clone());
                        paths.push(p);
                    }
                }
                i = j.max(i + 1);
            }
            _ => i += 1,
        }
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
/// all they name; the gate's (`gate`) with every item of `mod.rs` too.
pub fn codes(w: &World) -> Vec<(String, String)> {
    let top = vec!["inputs".to_string()];
    let mut out = Vec::new();
    for x in &w.crates["inputs"].modules[&top].children {
        let mut start = w.under("inputs", &[top.clone(), vec![x.clone()]].concat());
        if x == "gate" {
            start.extend(w.all("inputs", &top));
        }
        out.push((x.clone(), w.digest(start)));
    }
    out
}
