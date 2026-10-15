//! How the build reads the regions (docs/inputs.md §6.1.4, §6.1.7, §6.1.10).
//!
//! From the accepted version (`Source::Accepted`): the regions listed by the index (each region
//! file's `facts`: its id, name and outline entries), the coverage made from their stored geometry
//! (`Coverage::accepted`), and the catalog's drawn regions from their drawn files. Until the
//! records say the readers switched (`sources/inputs/regions/@switched`, which the regions' check
//! records once the migration's proof, `scenic-build regions-proof`, says the same of exactly the
//! accepted version the records name), the readers read the drop box as before
//! (`Source::DropBox`): the recipes in `inputs/regions/`, the pass's outlines and
//! `inputs/outlines/`. The switch is in the records, so a reader never decides it from a file it
//! may fail to read.

use super::stored::{Drawn, Geometry};
use crate::agent::recipes::Recipe;
use crate::coverage::{Coverage, DrawnRegion, Shape};
use crate::inputs::Accepted;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The migration's proof (`scenic-build regions-proof`): the readers read the accepted version
/// once it says `same: true`.
pub const PROOF: &str = "state/inputs/regions-proof.json";

/// What the proof found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Proof {
    pub fmt: u32,
    /// The accepted index it compared with the drop box's path.
    pub index: String,
    /// The pass's date.
    pub pass: String,
    /// Fingerprints and shapes keys the same on every z6 tile and job box compared, and no build
    /// step's key changed.
    pub same: bool,
    pub regions: usize,
    pub tiles: usize,
    pub boxes: usize,
    /// The build steps whose key a dry plan would change.
    pub steps_changed: usize,
    /// Each difference, in words (the first hundred).
    #[serde(default)]
    pub differences: Vec<String>,
    pub at: u64,
    /// The app the proof ran on: once the switch is recorded (with these bytes), no Mac on an older
    /// app leads or is handed work (`switch_app`).
    #[serde(default)]
    pub app: String,
}

/// The app the regions' switch was proved on, from the records' `@switched` (its `app`): Ok(None)
/// when not switched. One that can't be read or parsed, or names no app, is an error: unknown,
/// never taken for an app every Mac passes.
pub fn switch_app(manifest: &BTreeMap<String, String>, read: &dyn Fn(&str) -> Result<Vec<u8>>) -> Result<Option<String>> {
    let Some(name) = manifest.get(&crate::inputs::switched_logical(super::UNIT)) else { return Ok(None) };
    let v: serde_json::Value = serde_json::from_slice(&read(name)?).with_context(|| format!("parse {name}"))?;
    let app = v.get("app").and_then(|a| a.as_str()).filter(|a| !a.is_empty()).with_context(|| format!("{name} names no app"))?;
    Ok(Some(app.to_string()))
}

/// Whether a Mac on app `app` may lead, or be handed the lead or work, given the regions' switch's
/// app as read (`switch_app`): not on an older one; not while that can't be read (unknown).
pub fn may_lead(app: &str, regions: &std::result::Result<Option<String>, String>) -> std::result::Result<(), String> {
    match regions {
        Err(e) => Err(format!("the app the regions' switch was proved on can't be read now ({e})")),
        Ok(Some(min)) if !crate::pool::term::app_at_least(app, min) => Err(format!("its app {app} is older than the one the regions' switch was proved on ({min})")),
        Ok(_) => Ok(()),
    }
}

/// The app a member needs to lead: the term's and the regions' switch's, whichever is newer.
pub fn needed_app(term: Option<&str>, regions: Option<&str>) -> Option<String> {
    match (term, regions) {
        (Some(t), Some(r)) => Some(if crate::pool::term::app_at_least(t, r) { t } else { r }.to_string()),
        (t, r) => t.or(r).map(str::to_string),
    }
}

/// The proof: None when none has been written; an error when it can't be read now or doesn't
/// parse (never taken for none).
pub fn proof(root: &Path) -> Result<Option<Proof>> {
    match std::fs::read(root.join(PROOF)) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b).with_context(|| format!("parse {PROOF}"))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {PROOF}")),
    }
}

/// Whether the records say the readers read the regions' accepted version.
pub fn switched(manifest: &BTreeMap<String, String>) -> bool {
    manifest.contains_key(&crate::inputs::switched_logical(super::UNIT))
}

/// Where the readers read the regions from now.
#[derive(Clone, Debug)]
pub enum Source {
    /// The accepted version the records name.
    Accepted(Accepted),
    /// The drop box, as before the input check (until the proof passes).
    DropBox,
}

impl Source {
    /// What it is, for keys: the accepted index's content name, or "dropbox".
    pub fn name(&self) -> &str {
        match self {
            Source::Accepted(a) => &a.version,
            Source::DropBox => "dropbox",
        }
    }
}

/// Where the readers read the regions from, given the records `manifest`: the accepted version once
/// they say the readers switched (for good: later versions are the owner's edits taken in by the
/// check); the drop box before. Switched but no accepted version to read is an error, never the
/// drop box instead.
pub fn source(root: &Path, manifest: &BTreeMap<String, String>) -> Result<Source> {
    if !switched(manifest) {
        return Ok(Source::DropBox);
    }
    Ok(Source::Accepted(crate::inputs::open(root, manifest, super::UNIT)?.context("the records say the regions' readers switched, but name no accepted version")?))
}

/// `source`, the records read from the NAS.
pub fn source_now(root: &Path) -> Result<Source> {
    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json"))?;
    source(root, &m)
}

/// An accepted region: its file's path, its recipe (id, name, outline entries), its stored files.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub path: String,
    pub recipe: Recipe,
    pub geometry: String,
    pub drawn: String,
}

/// The regions of an accepted version, by id: each region file's (a `.toml` but `pins.toml` and
/// `_group.toml`). One missing a fact (its id, name, outline, geometry or drawing) is an error,
/// never left out.
pub fn listed(a: &Accepted) -> Result<Vec<Listed>> {
    let mut v: Vec<Listed> = Vec::new();
    for (p, f) in &a.index.files {
        if !p.ends_with(".toml") || super::resolve::is_pins(p) || super::resolve::is_group(p) {
            continue;
        }
        let s = |k: &str| f.facts.get(k).and_then(|v| v.as_str()).map(str::to_string).with_context(|| format!("{}: {p} has no {k} in its facts", a.version));
        let outline: Vec<String> = f.facts.get("outline").and_then(|o| serde_json::from_value(o.clone()).ok()).with_context(|| format!("{}: {p} has no outline in its facts", a.version))?;
        v.push(Listed { path: p.clone(), recipe: Recipe { id: s("id")?, name: s("name")?, outline }, geometry: s("geometry")?, drawn: s("drawn")? });
    }
    v.sort_by(|a, b| a.recipe.id.cmp(&b.recipe.id));
    Ok(v)
}

/// A folder of the regions' tree (the map's Regions list, §6.1.2): its path (`europe/portugal`),
/// its name (its `_group.toml`'s, else the folder's own), its order, and its regions' ids directly
/// in it. Only folders holding a region at some depth are in the tree (an empty one isn't shown).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub path: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    pub regions: Vec<String>,
    pub folders: Vec<Folder>,
}

/// The tree of an index's regions (the root: path ""), from the paths of its region files and its
/// folders' `_group.toml` facts: folders sorted by order (those with one first, lower first), then
/// name; regions by id. Grouping is the index's alone: it enters no coverage, key or fingerprint.
pub fn tree(index: &crate::inputs::Index) -> Folder {
    let mut groups: BTreeMap<String, super::file::Group> = BTreeMap::new();
    let mut regions: Vec<(String, String)> = Vec::new();
    for (p, f) in &index.files {
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        if let Some(g) = f.facts.get("group").and_then(|v| serde_json::from_value(v.clone()).ok()) {
            groups.insert(dir, g);
        } else if let Some(id) = f.facts.get("geometry").and(f.facts.get("id")).and_then(|v| v.as_str()) {
            regions.push((dir, id.to_string()));
        }
    }
    fn build(path: &str, groups: &BTreeMap<String, super::file::Group>, regions: &[(String, String)]) -> Folder {
        let mut subs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (d, _) in regions {
            let rest = if path.is_empty() { Some(d.as_str()) } else { d.strip_prefix(path).and_then(|r| r.strip_prefix('/')) };
            if let Some(first) = rest.filter(|r| !r.is_empty()).map(|r| r.split('/').next().unwrap()) {
                subs.insert(if path.is_empty() { first.to_string() } else { format!("{path}/{first}") });
            }
        }
        let mut folders: Vec<Folder> = subs.iter().map(|s| build(s, groups, regions)).collect();
        folders.sort_by(|a, b| (a.order.is_none(), a.order, a.name.to_lowercase(), &a.path).cmp(&(b.order.is_none(), b.order, b.name.to_lowercase(), &b.path)));
        let mut ids: Vec<String> = regions.iter().filter(|(d, _)| d == path).map(|(_, i)| i.clone()).collect();
        ids.sort();
        let g = groups.get(path);
        let own = path.rsplit_once('/').map_or(path, |(_, n)| n).to_string();
        Folder { path: path.to_string(), name: g.map_or(own, |g| g.name.clone()), order: g.and_then(|g| g.order), regions: ids, folders }
    }
    build("", &groups, &regions)
}

/// The regions as a reader lists them (sorted by id), and the files that don't read (none from an
/// accepted version: every file in it was checked).
pub fn recipes(root: &Path, src: &Source) -> (Vec<Recipe>, Vec<(String, String)>) {
    match src {
        Source::Accepted(a) => match listed(a) {
            Ok(l) => (l.into_iter().map(|l| l.recipe).collect(), Vec::new()),
            Err(e) => (Vec::new(), vec![("regions".into(), format!("{e:#}"))]),
        },
        Source::DropBox => crate::agent::recipes::load(&root.join("inputs/regions")),
    }
}

/// Reads `names` (content names under `root`), 16 at a time (on the NAS each waits a round trip or
/// two), in their order.
fn read_all(root: &Path, names: &[String]) -> Result<Vec<Vec<u8>>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let read: Vec<std::sync::OnceLock<Result<Vec<u8>>>> = names.iter().map(|_| std::sync::OnceLock::new()).collect();
    std::thread::scope(|s| {
        for _ in 0..names.len().min(16) {
            s.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(n) = names.get(k) else { break };
                read[k].set(std::fs::read(root.join(n)).with_context(|| format!("read {n}"))).ok();
            });
        }
    });
    read.into_iter().map(|r| r.into_inner().unwrap_or_else(|| Err(anyhow::anyhow!("not read")))).collect()
}

/// The coverage of stored geometry, regions in the order given (id order), each region's shapes in
/// stored order, as `Coverage::from_recipes` made them: "<id>: <from>", the tolerance as the
/// buffer, the country.
pub fn coverage_of(geoms: &[Geometry]) -> Coverage {
    let mut shapes = Vec::new();
    for g in geoms {
        for s in &g.shapes {
            let mut sh = Shape::new(format!("{}: {}", g.id, s.from), s.rings.clone(), s.tolerance_m as f64);
            sh.country = s.country.clone();
            shapes.push(sh);
        }
    }
    Coverage { shapes }
}

impl Coverage {
    /// The coverage of the regions' accepted version (§6.1.4): their stored geometry. Where there's
    /// a `cache` (this Mac's), kept there by the version (`coverage/<hash>.cov`): the geometry
    /// files are read once per version and Mac.
    pub fn accepted(root: &Path, a: &Accepted, cache: Option<&Path>) -> Result<Coverage> {
        let made = || -> Result<Coverage> {
            let l = listed(a)?;
            let names: Vec<String> = l.iter().map(|x| x.geometry.clone()).collect();
            let geoms: Vec<Geometry> = read_all(root, &names)?.iter().zip(&names).map(|(b, n)| Geometry::decode(b).with_context(|| n.clone())).collect::<Result<_>>()?;
            anyhow::ensure!(!geoms.is_empty(), "no regions in {}", a.version);
            Ok(coverage_of(&geoms))
        };
        let Some(dir) = cache.map(|c| c.join("coverage")) else { return made() };
        let at = dir.join(format!("{}.cov", store::naming::hash16(a.version.as_bytes())));
        if let Some((c, _)) = store::cachefile::read(&at).ok().flatten().and_then(|b| crate::coverage::from_kept(&b)) {
            return Ok(c);
        }
        let c = made()?;
        if store::cachefile::create_bytes(&at, &crate::coverage::kept_bytes(&c, &[])).is_ok() {
            store::cachefile::release(&at);
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if e.path() != at {
                    store::cachefile::try_remove(&e.path());
                }
            }
        }
        Ok(c)
    }
}

/// The coverage from `src`: the accepted version's stored geometry, or the drop box's recipes with
/// the pass's outlines (`outlines`, its file) as before; and the recipes that didn't read.
pub fn coverage(root: &Path, src: &Source, outlines: Option<&Path>, cache: Option<&Path>) -> Result<(Coverage, Vec<(String, String)>)> {
    match src {
        Source::Accepted(a) => Ok((Coverage::accepted(root, a, cache)?, Vec::new())),
        Source::DropBox => Coverage::load(&root.join("inputs/regions"), outlines, &root.join("inputs/outlines"), cache),
    }
}

/// The coverage from `src` without a cache, the recipes given (the drop box's path: as
/// `Coverage::from_recipes` makes it).
pub fn coverage_with(root: &Path, src: &Source, recipes: &[Recipe], outlines: Option<&crate::outlines::Outlines>) -> Result<Coverage> {
    match src {
        Source::Accepted(a) => Coverage::accepted(root, a, None),
        Source::DropBox => Coverage::from_recipes(recipes, outlines, &root.join("inputs/outlines")),
    }
}

/// The drawn regions of `regions` (recipes `recipes` lists from `src`): from the accepted version's
/// drawn files, or the drop box's recipes drawn as before (`coverage::drawn`).
pub fn drawn(root: &Path, src: &Source, regions: &[Recipe], outlines: Option<&crate::outlines::Outlines>) -> Result<Vec<DrawnRegion>> {
    match src {
        Source::DropBox => crate::coverage::drawn(regions, outlines, &root.join("inputs/outlines")),
        Source::Accepted(a) => {
            let l = listed(a)?;
            let names: Vec<String> = regions.iter().filter_map(|r| l.iter().find(|x| x.recipe.id == r.id).map(|x| x.drawn.clone())).collect();
            read_all(root, &names)?.iter().zip(&names).map(|(b, n)| serde_json::from_slice::<Drawn>(b).map(|d| d.region()).with_context(|| format!("parse {n}"))).collect()
        }
    }
}

/// The regions' digest in the agent's inputs (what the catalog's key names): the accepted index's
/// content name once the readers read it; else the drop box's recipes and outline files, as before
/// (`regions_digest`). None when it can't be read now.
pub fn digest(root: &Path, dropbox: impl FnOnce() -> Option<String>) -> Option<String> {
    match source_now(root) {
        Ok(Source::Accepted(a)) => Some(a.version),
        Ok(Source::DropBox) => dropbox(),
        Err(_) => None,
    }
}

/// The coverage of a scratch folder of region files (`--regions <dir>`, for proofs: docs/inputs.md
/// §6.1.10, 10): the check's own code run on it in memory against the records' current pass,
/// storing nothing; a region it would hold is an error naming why. A folder without `pins.toml`
/// is read as recipes, as before (`Coverage::from_recipes`).
pub fn coverage_of_folder(root: &Path, manifest: &BTreeMap<String, String>, dir: &Path) -> Result<(Coverage, usize)> {
    if !dir.join("pins.toml").exists() {
        let (recipes, _) = crate::agent::recipes::load(dir);
        anyhow::ensure!(!recipes.is_empty(), "no regions in {}", dir.display());
        let outlines = crate::osmpass::latest_pass_in(manifest).and_then(|d| manifest.get(&format!("sources/osm/{d}/outlines")).map(|o| root.join(o)));
        let o = outlines.map(|p| crate::outlines::Outlines::open(&p)).transpose()?;
        return Ok((Coverage::from_recipes(&recipes, o.as_ref(), &root.join("inputs/outlines"))?, recipes.len()));
    }
    let mut paths = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, rel)) = stack.pop() {
        for e in std::fs::read_dir(&d).with_context(|| format!("list {}", d.display()))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if crate::inputs::ignored(&name) || (rel.is_empty() && (name == "how" || name == "todo")) {
                continue;
            }
            let p = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if e.file_type()?.is_dir() {
                stack.push((e.path(), p));
            } else if super::resolve::takes(&p) {
                paths.push(p);
            }
        }
    }
    paths.sort();
    let read = |p: &str| -> Result<std::sync::Arc<Vec<u8>>> { Ok(std::sync::Arc::new(std::fs::read(dir.join(p)).with_context(|| format!("read {p}"))?)) };
    let v = crate::inputs::Version { paths: paths.into_iter().collect(), read: &read };
    let checks: Box<dyn crate::inputs::Checks> = crate::inputs::Checks::prepared(&super::Regions::default(), root, manifest)?.context("the regions' checks")?;
    if let Some(why) = checks.not_ready() {
        anyhow::bail!("the regions can't be checked now: {why}");
    }
    let mut findings = Vec::new();
    for p in &v.paths {
        findings.extend(checks.file(p, &(v.read)(p)?).findings);
    }
    findings.extend(checks.whole(&v)?);
    anyhow::ensure!(findings.is_empty(), "the folder's regions don't check: {}", findings.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; "));
    let d = checks.derive(&v, &Default::default())?;
    let mut geoms: Vec<Geometry> = Vec::new();
    for (name, bytes) in &d.store {
        if name.ends_with(".rgeo") {
            geoms.push(Geometry::decode(bytes)?);
        }
    }
    geoms.sort_by(|a, b| a.id.cmp(&b.id));
    let n = geoms.len();
    Ok((coverage_of(&geoms), n))
}
