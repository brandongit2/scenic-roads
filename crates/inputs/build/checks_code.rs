//! What each gate unit's checks are, as code (docs/inputs.md §4.3, task #1467): the digest of a
//! module of `src/inputs/` as the app compiles it, which `inputs::check_key` and a unit's check
//! state (`Listed::derived_by`) name in place of a typed version. build.rs makes it for every module
//! `src/inputs/mod.rs` declares; the crate's tests compile this file in too (lib.rs).
//!
//! A module's digest is of its tokens as compiled into the app: its root file with every child
//! module's file in its place (`mod x;`, `#[path]` too), and the bytes of every file it compiles in
//! (`include_str!`, `include_bytes!`, `include!`, by a plain string literal: anything else is an
//! error, never passed over). Left out, so they move no digest: comments and doc comments, layout
//! and what rustfmt changes (a trailing comma, a match arm's body put in braces or taken out), and
//! what the app's build doesn't compile: items under a `cfg` false without tests and the `testkit`
//! feature (`#[cfg(test)]` modules, `#[cfg(any(test, feature = "testkit"))]` fixtures) and `#[test]`
//! functions. (So a change from `(x)` to `(x,)` alone moves nothing.)
//!
//! What it doesn't see: code outside the module (the gate's, `mod.rs`; other modules and crates a
//! unit's checks call) and files read at run time (a unit's `Checks::context` names those).

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use std::path::{Path, PathBuf};
use syn::visit_mut::VisitMut;

/// A module's digest and the files it was made from (build.rs reruns when one changes).
pub struct Code {
    pub digest: String,
    pub files: Vec<PathBuf>,
}

/// The digest of each module `<dir>/mod.rs` declares (and compiles into the app), by name, sorted.
pub fn modules(dir: &Path) -> Result<Vec<(String, Code)>, String> {
    let root = dir.join("mod.rs");
    let file = parse(&root)?;
    let mut out = Vec::new();
    for item in &file.items {
        let syn::Item::Mod(m) = item else { continue };
        if m.content.is_some() || !compiled(&m.attrs) {
            continue;
        }
        let name = m.ident.to_string();
        let (path, children) = child_file(m, dir, dir)?;
        out.push((name, module(&path, &children)?));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The digest of the module whose root file is `root`, its children's files in `children`.
pub fn module(root: &Path, children: &Path) -> Result<Code, String> {
    let mut file = parse(root)?;
    let mut v = Strip {
        dir: children.to_path_buf(),
        file_dir: root.parent().unwrap_or(Path::new(".")).to_path_buf(),
        files: vec![root.to_path_buf()],
        err: None,
    };
    v.visit_file_mut(&mut file);
    if let Some(e) = v.err {
        return Err(e);
    }
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    feed(file.to_token_stream(), &mut h);
    Ok(Code { digest: format!("{:032x}", h.digest128()), files: v.files })
}

fn parse(p: &Path) -> Result<syn::File, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    syn::parse_file(&text).map_err(|e| format!("parse {}: {e}", p.display()))
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
/// attributes (`#[doc = …]`, `#![doc = …]`: doc comments) and a group's trailing comma left out.
fn feed(ts: TokenStream, h: &mut xxhash_rust::xxh3::Xxh3) {
    let t: Vec<TokenTree> = ts.into_iter().collect();
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
            TokenTree::Punct(p) if p.as_char() == ',' && i + 1 == t.len() => {}
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
                feed(g.stream(), h);
                h.update(b"}");
            }
        }
        i += 1;
    }
}
