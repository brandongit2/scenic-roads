//! How the build reads the regions (docs/inputs.md §4.6, §6.1.4, §6.1.10): from the accepted version
//! alone, the regions listed by the index (each region file's `facts`: its id, name and outline
//! entries), the coverage made from their stored geometry (`Coverage::accepted`), the catalogue's
//! drawn regions from their drawn files, and the Regions list's tree from the index's paths.

use super::stored::{Drawn, Geometry};
use crate::agent::recipes::Recipe;
use crate::coverage::{Coverage, DrawnRegion, Shape};
use crate::inputs::Accepted;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The regions' accepted version the records `manifest` name: what every reader reads; an error
/// before the records say the readers switched (`sources/inputs/regions/@switched`: publish 1's
/// runbook, docs/inputs.md §6.1.10, comes first), or when it can't be read now.
pub fn source(root: &store::nasfs::NasRoot, manifest: &BTreeMap<String, String>) -> Result<Accepted> {
    anyhow::ensure!(
        switched(manifest),
        "the records don't say the regions' readers switched: run publish 1's runbook first (docs/inputs.md §6.1.10)"
    );
    crate::inputs::open(root, manifest, super::UNIT)?
        .context("no accepted version of the regions yet (the input check takes inputs/regions/ in)")
}

/// Whether the records say the readers read the regions' accepted version.
pub fn switched(manifest: &BTreeMap<String, String>) -> bool {
    manifest.contains_key(&crate::inputs::switched_logical(super::UNIT))
}

/// The app the newer readers' switch (the regions', the elevation datasets') was recorded on, from
/// the records' `@switched` (its `app`): Ok(None)
/// when not switched. One that can't be read or parsed, or names no app, is an error: unknown,
/// never taken for an app every Mac passes.
pub fn switch_app(
    manifest: &BTreeMap<String, String>,
    read: &dyn Fn(&str) -> Result<Vec<u8>>,
) -> Result<Option<String>> {
    // (The elevation datasets' switch too, #143: an app from before it would key and build the
    // units as before them. The newer of the two.)
    let mut newest: Option<String> = None;
    for unit in [super::UNIT, crate::inputs::elevation::UNIT] {
        let Some(name) = manifest.get(&crate::inputs::switched_logical(unit)) else { continue };
        let v: serde_json::Value = serde_json::from_slice(&read(name)?).with_context(|| format!("parse {name}"))?;
        let app = v
            .get("app")
            .and_then(|a| a.as_str())
            .filter(|a| !a.is_empty())
            .with_context(|| format!("{name} names no app"))?;
        if newest.as_deref().is_none_or(|n| !crate::pool::term::app_at_least(n, app)) {
            newest = Some(app.to_string());
        }
    }
    Ok(newest)
}

/// Whether a Mac on app `app` may lead, or be handed the lead or work, given the readers' switches'
/// app as read (`switch_app`): not on an older one; not while that can't be read (unknown).
pub fn may_lead(app: &str, regions: &std::result::Result<Option<String>, String>) -> std::result::Result<(), String> {
    match regions {
        Err(e) => Err(format!("the app the newer readers' switch (the regions', the elevation datasets') was recorded on can't be read now ({e})")),
        Ok(Some(min)) if !crate::pool::term::app_at_least(app, min) => Err(format!("its app {app} is older than the one the newer readers' switch (the regions', the elevation datasets') was recorded on ({min})")),
        Ok(_) => Ok(()),
    }
}

/// The app a member needs to lead: the term's and the readers' switches', whichever is newer.
pub fn needed_app(term: Option<&str>, regions: Option<&str>) -> Option<String> {
    match (term, regions) {
        (Some(t), Some(r)) => Some(if crate::pool::term::app_at_least(t, r) { t } else { r }.to_string()),
        (t, r) => t.or(r).map(str::to_string),
    }
}

/// `source`, the records read from the NAS.
pub fn source_now(root: &store::nasfs::NasRoot) -> Result<Accepted> {
    let m = crate::out::read_manifest(root)?;
    source(root, &m)
}

/// An accepted region: its file's path, its recipe (id, name, outline entries), its stored files,
/// and whether it's disabled (`disabled = true`: checked, never built).
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub path: String,
    pub recipe: Recipe,
    /// Its stored geometry and drawn files; None for a region that resolved to nothing (empty by
    /// design, as `hand_out_all`'s "land outside its regions" once its country is handed out): the
    /// check stores no files for it, and it has no area to cover or draw.
    pub stored: Option<Stored>,
    pub disabled: bool,
}

/// A region's stored files (content names): its geometry (the coverage's) and its drawing (the map's).
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub geometry: String,
    pub drawn: String,
}

/// The regions of an accepted version, by id: each region file's (a `.toml` but the special files:
/// the `_` names). One missing a fact (its id, name or outline; its drawing without its geometry or
/// the other way round) is an error, never left out; one with neither resolved to nothing (empty)
/// and is listed with no stored files.
pub fn listed(a: &Accepted) -> Result<Vec<Listed>> {
    let mut v: Vec<Listed> = Vec::new();
    for (p, f) in &a.index.files {
        if !p.ends_with(".toml") || super::resolve::is_special(p) {
            continue;
        }
        let s = |k: &str| {
            f.facts
                .get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .with_context(|| format!("{}: {p} has no {k} in its facts", a.version))
        };
        let outline: Vec<String> = f
            .facts
            .get("outline")
            .and_then(|o| serde_json::from_value(o.clone()).ok())
            .with_context(|| format!("{}: {p} has no outline in its facts", a.version))?;
        let stored = match (f.facts.contains_key("geometry"), f.facts.contains_key("drawn")) {
            (false, false) => None,
            _ => Some(Stored { geometry: s("geometry")?, drawn: s("drawn")? }),
        };
        v.push(Listed {
            path: p.clone(),
            recipe: Recipe { id: s("id")?, name: s("name")?, outline },
            stored,
            disabled: f.facts.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false),
        });
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
        } else if let Some(id) = f.facts.get("id").and_then(|v| v.as_str()) {
            regions.push((dir, id.to_string()));
        }
    }
    fn build(path: &str, groups: &BTreeMap<String, super::file::Group>, regions: &[(String, String)]) -> Folder {
        let mut subs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (d, _) in regions {
            let rest =
                if path.is_empty() { Some(d.as_str()) } else { d.strip_prefix(path).and_then(|r| r.strip_prefix('/')) };
            if let Some(first) = rest.filter(|r| !r.is_empty()).map(|r| r.split('/').next().unwrap()) {
                subs.insert(if path.is_empty() { first.to_string() } else { format!("{path}/{first}") });
            }
        }
        let mut folders: Vec<Folder> = subs.iter().map(|s| build(s, groups, regions)).collect();
        folders.sort_by(|a, b| {
            (a.order.is_none(), a.order, a.name.to_lowercase(), &a.path).cmp(&(
                b.order.is_none(),
                b.order,
                b.name.to_lowercase(),
                &b.path,
            ))
        });
        let mut ids: Vec<String> = regions.iter().filter(|(d, _)| d == path).map(|(_, i)| i.clone()).collect();
        ids.sort();
        let g = groups.get(path);
        let own = path.rsplit_once('/').map_or(path, |(_, n)| n).to_string();
        Folder {
            path: path.to_string(),
            name: g.map_or(own, |g| g.name.clone()),
            order: g.and_then(|g| g.order),
            regions: ids,
            folders,
        }
    }
    build("", &groups, &regions)
}

/// The regions the build reads, sorted by id: the enabled ones (a disabled region is checked and
/// stored, others may name it, but nothing builds, plans, sizes, downloads or catalogues it), less
/// those that resolved to nothing (no area: nothing to build).
pub fn recipes(a: &Accepted) -> Result<Vec<Recipe>> {
    Ok(enabled(a)?.into_iter().map(|l| l.recipe).collect())
}

/// The enabled regions of an accepted version with an area (`listed`, the disabled and the empty
/// left out). An enabled region that resolved to nothing is skipped, never silently: a warning
/// names it (`skipped_empty`), once per version.
pub fn enabled(a: &Accepted) -> Result<Vec<Listed>> {
    let l = listed(a)?;
    if let Some(w) = skipped_empty(&a.version, &l) {
        eprintln!("{w}");
    }
    Ok(l.into_iter().filter(|l| !l.disabled && l.stored.is_some()).collect())
}

/// The warning for the enabled regions of `listed` that resolved to nothing (the check holds one
/// unless it's a `hand_out_all` leftover, `rg-empty`; an enabled leftover is still nothing to
/// build): None when there are none, or `version`'s were already named in this process.
pub fn skipped_empty(version: &str, listed: &[Listed]) -> Option<String> {
    static SAID: std::sync::Mutex<Option<std::collections::BTreeSet<String>>> = std::sync::Mutex::new(None);
    let ids: Vec<&str> =
        listed.iter().filter(|l| !l.disabled && l.stored.is_none()).map(|l| l.recipe.id.as_str()).collect();
    if ids.is_empty() || !SAID.lock().unwrap().get_or_insert_default().insert(version.to_string()) {
        return None;
    }
    Some(format!("warning: {version}: enabled but resolved to nothing, skipped (no area to build): {}", ids.join(", ")))
}

/// Reads `names` (content names under `root`), 16 at a time (on the NAS each waits a round trip or
/// two), in their order.
fn read_all(root: &store::nasfs::NasRoot, names: &[String]) -> Result<Vec<Vec<u8>>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let read: Vec<std::sync::OnceLock<Result<Vec<u8>>>> = names.iter().map(|_| std::sync::OnceLock::new()).collect();
    std::thread::scope(|s| {
        for _ in 0..names.len().min(16) {
            s.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(n) = names.get(k) else { break };
                read[k].set(root.join(n).read().with_context(|| format!("read {n}"))).ok();
            });
        }
    });
    read.into_iter().map(|r| r.into_inner().unwrap_or_else(|| Err(anyhow::anyhow!("not read")))).collect()
}

/// The coverage of stored geometry, regions in the order given (id order), each region's shapes in
/// stored order, as `Coverage::from_recipes` made them: "<id>: <from>", the tolerance as the
/// buffer, the country. A region's water is marked (`Shape::water`: held, no target but the depth's);
/// a region of water alone (no land: a sea zone, the high seas) is the depth's alone (`depth_only`).
pub fn coverage_of(geoms: &[Geometry]) -> Coverage {
    let mut shapes = Vec::new();
    let mut depth_only = Vec::new();
    for g in geoms {
        let to = if g.shapes.iter().all(|s| s.water) { &mut depth_only } else { &mut shapes };
        for s in &g.shapes {
            let mut sh = Shape::new(format!("{}: {}", g.id, s.from), s.rings.clone(), s.tolerance_m as f64);
            sh.country = s.country.clone();
            sh.water = s.water;
            to.push(sh);
        }
    }
    Coverage { shapes, depth_only }
}

impl Coverage {
    /// The coverage of the regions' accepted version (§6.1.4): its enabled regions' stored geometry
    /// (a disabled region is no coverage). Where there's
    /// a `cache` (this Mac's), kept there by the version (`coverage/<hash>.cov`): the geometry
    /// files are read once per version and Mac.
    pub fn accepted(root: &store::nasfs::NasRoot, a: &Accepted, cache: Option<&Path>) -> Result<Coverage> {
        let made = || -> Result<Coverage> {
            let l = enabled(a)?;
            let names: Vec<String> = l.iter().filter_map(|x| Some(x.stored.as_ref()?.geometry.clone())).collect();
            let geoms: Vec<Geometry> = read_all(root, &names)?
                .iter()
                .zip(&names)
                .map(|(b, n)| Geometry::decode(b).with_context(|| n.clone()))
                .collect::<Result<_>>()?;
            anyhow::ensure!(!geoms.is_empty(), "no regions in {}", a.version);
            Ok(coverage_of(&geoms))
        };
        let Some(dir) = cache.map(|c| c.join("coverage")) else { return made() };
        let at = dir.join(format!("{}.cov", store::naming::hash16(a.version.as_bytes())));
        match store::cachefile::read(&at) {
            Ok(Some(b)) => match crate::coverage::from_kept(&b) {
                Some(c) => return Ok(c),
                // (Another format's: made again in its place, not read past forever.)
                None => drop(store::cachefile::try_remove(&at)),
            },
            _ => {}
        }
        let c = made()?;
        if store::cachefile::create_bytes(&at, &crate::coverage::kept_bytes(&c)).is_ok() {
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

/// The drawn regions of `regions` (as `recipes` lists them) from the accepted version's drawn
/// files; an empty region (none stored) has none.
pub fn drawn(root: &store::nasfs::NasRoot, a: &Accepted, regions: &[Recipe]) -> Result<Vec<DrawnRegion>> {
    let l = listed(a)?;
    let names: Vec<String> = regions
        .iter()
        .filter_map(|r| Some(l.iter().find(|x| x.recipe.id == r.id)?.stored.as_ref()?.drawn.clone()))
        .collect();
    read_all(root, &names)?
        .iter()
        .zip(&names)
        .map(|(b, n)| serde_json::from_slice::<Drawn>(b).map(|d| d.region()).with_context(|| format!("parse {n}")))
        .collect()
}

/// The coverage of a scratch folder of region files (`--regions <dir>`, for proofs: docs/inputs.md
/// §6.1.10, 10): the check's own code run on it in memory against the records' current pass,
/// storing nothing; a region it would hold is an error naming why. Disabled regions are checked
/// and resolved, and left out of the coverage.
pub fn coverage_of_folder(
    root: &store::nasfs::NasRoot,
    manifest: &BTreeMap<String, String>,
    dir: &Path,
) -> Result<(Coverage, usize)> {
    let mut paths = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, rel)) = stack.pop() {
        for e in std::fs::read_dir(&d).with_context(|| format!("list {}", d.display()))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if super::resolve::passes_over(&name) || (rel.is_empty() && (name == "how" || name == "todo")) {
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
    let read = |p: &str| -> Result<std::sync::Arc<Vec<u8>>> {
        Ok(std::sync::Arc::new(std::fs::read(dir.join(p)).with_context(|| format!("read {p}"))?))
    };
    let v = crate::inputs::Version { paths: paths.into_iter().collect(), read: &read };
    let checks: Box<dyn crate::inputs::Checks> =
        crate::inputs::Checks::prepared(&super::Regions::default(), root, manifest)?.context("the regions' checks")?;
    if let Some(why) = checks.not_ready() {
        anyhow::bail!("the regions can't be checked now: {why}");
    }
    let mut findings = Vec::new();
    let mut disabled = std::collections::BTreeSet::new();
    for p in &v.paths {
        let c = checks.file(p, &(v.read)(p)?);
        if c.facts.get("disabled").and_then(|d| d.as_bool()) == Some(true) {
            disabled.extend(c.facts.get("id").and_then(|i| i.as_str()).map(str::to_string));
        }
        findings.extend(c.findings);
    }
    findings.extend(checks.whole(&v)?);
    anyhow::ensure!(
        findings.is_empty(),
        "the folder's regions don't check: {}",
        findings.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; ")
    );
    let d = checks.derive(&v, &Default::default())?;
    let mut geoms: Vec<Geometry> = Vec::new();
    for (name, bytes) in &d.store {
        if name.ends_with(".rgeo") {
            let g = Geometry::decode(bytes)?;
            if !disabled.contains(&g.id) {
                geoms.push(g);
            }
        }
    }
    geoms.sort_by(|a, b| a.id.cmp(&b.id));
    let n = geoms.len();
    Ok((coverage_of(&geoms), n))
}

/// A scratch folder of region files checked (`scenic-build regions-check`): every finding, and each
/// region resolved, with its size. Against the records' current pass, the datasets those that
/// `declarations` (a folder of source declarations) names when given; reads, stores nothing.
pub fn check_folder(
    root: &store::nasfs::NasRoot,
    manifest: &BTreeMap<String, String>,
    dir: &Path,
    declarations: Option<&Path>,
    land: Option<&Path>,
) -> Result<serde_json::Value> {
    let mut paths = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, rel)) = stack.pop() {
        for e in std::fs::read_dir(&d).with_context(|| format!("list {}", d.display()))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if super::resolve::passes_over(&name) {
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
    let t = std::time::Instant::now();
    // (With declarations, the land: the file given, else the records' for the current pass.)
    let data = match declarations {
        Some(d) => {
            let land = match land {
                Some(p) => Some((
                    p.display().to_string(),
                    super::land::Land::open(std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?)?,
                )),
                None => {
                    let pass = crate::osmpass::latest_pass_in(manifest);
                    match pass.and_then(|d| manifest.get(&super::land::logical(&d)).cloned()) {
                        Some(n) => Some((n.clone(), super::land::Land::open(root.join(&n).open()?.into_file())?)),
                        None => None,
                    }
                }
            };
            Some(super::data::Datasets::read(root, &crate::inputs::sources::Sources::of_folder(d)?, land)?)
        }
        None => None,
    };
    eprintln!(
        "regions-check: datasets read in {:.0} s, footprint {} MB, peak resident {} MB",
        t.elapsed().as_secs_f64(),
        crate::sys::footprint().unwrap_or(0) / 1_000_000,
        crate::sys::peak_rss() / 1_000_000
    );
    let checks = super::Regions::new(Some(super::prepare(root, manifest, data)?));
    let read = |p: &str| -> Result<std::sync::Arc<Vec<u8>>> {
        Ok(std::sync::Arc::new(std::fs::read(dir.join(p)).with_context(|| format!("read {p}"))?))
    };
    let v = crate::inputs::Version { paths: paths.into_iter().collect(), read: &read };
    use crate::inputs::Checks;
    if let Some(why) = checks.not_ready() {
        anyhow::bail!("the regions can't be checked now: {why}");
    }
    let mut findings = Vec::new();
    for p in &v.paths {
        findings.extend(checks.file(p, &(v.read)(p)?).findings);
    }
    let t = std::time::Instant::now();
    super::geom::reset_peak();
    let (folder, out) = checks.resolve(&v)?;
    let peak = super::geom::reset_peak();
    eprintln!(
        "regions-check: resolved in {:.0} s, footprint {} MB, peak resident {} MB",
        t.elapsed().as_secs_f64(),
        crate::sys::footprint().unwrap_or(0) / 1_000_000,
        crate::sys::peak_rss() / 1_000_000
    );
    findings.extend(out.problems.iter().map(super::problem_finding));
    let mut regions = serde_json::Map::new();
    let (mut total, mut enabled_km2) = (0.0, 0.0);
    for (id, res) in &out.resolved {
        super::geom::set_label(format!("the report's size of {id}"));
        let km2 = km2_of(&res.geometry.shapes, folder.regions[id].1.has_water());
        total += km2;
        let off = folder.regions[id].1.disabled;
        if !off {
            enabled_km2 += km2;
        }
        regions.insert(
            id.clone(),
            serde_json::json!({"path": folder.regions[id].0, "km2": km2, "water": folder.regions[id].1.has_water(), "disabled": off,
                "geometry": store::naming::hash16(&res.geometry.encode()), "points": res.geometry.shapes.iter().map(|s| s.rings.iter().map(Vec::len).sum::<usize>()).sum::<usize>()}),
        );
    }
    // (The 12-nautical-mile line the stored version would draw: its size and time.)
    let ts12 = checks.ctx.as_ref().and_then(|c| c.data.as_ref()).filter(|d| !d.ts12.is_empty()).map(|d| {
        let t = std::time::Instant::now();
        let lines = d.ts12_line();
        serde_json::json!({"lines": lines.len(), "points": lines.iter().map(Vec::len).sum::<usize>(), "s": t.elapsed().as_secs_f64()})
    });
    if let Some(d) = checks.ctx.as_ref().and_then(|c| c.data.as_ref()) {
        d.land_failed()?;
    }
    Ok(serde_json::json!({
        "ts12_line": ts12,
        "files": v.paths.len(),
        "regions": folder.regions.len(),
        "resolved": out.resolved.len(),
        "disabled": folder.regions.values().filter(|(_, r)| r.disabled).count(),
        "km2": total,
        "enabled_km2": enabled_km2,
        "earth_km2": super::geom::area_km2(&super::geom::earth()),
        "most_points_one_operation": peak,
        "findings": findings.iter().map(|f| serde_json::json!({"check": f.check(), "message": f.message})).collect::<Vec<_>>(),
        "by_region": regions,
    }))
}

/// A resolved region's area (km²). A region with water is stored as its land and its water, two
/// shapes apart, each of cell pieces (crate::inputs::regions::tiles): each read back cell by cell as
/// tiles keeps it (`TArea::from_rings`) and summed, never unioned as one area (the high seas, one
/// overlay of ~200,000 points across the Earth, ran past the operations' budget); any other
/// region's shapes are unioned.
pub(crate) fn km2_of(shapes: &[super::stored::Shape], by_cell: bool) -> f64 {
    let ring64 = |r: &Vec<[i32; 2]>| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect::<Vec<_>>();
    if by_cell {
        return shapes
            .iter()
            .map(|s| super::tiles::TArea::from_rings(&s.rings.iter().map(ring64).collect::<Vec<_>>()).km2())
            .sum();
    }
    let a = super::geom::union_all(
        &shapes
            .iter()
            .map(|s| super::geom::Area::from_rings(&s.rings.iter().map(ring64).collect::<Vec<_>>()))
            .collect::<Vec<_>>(),
    );
    super::geom::area_km2(&a)
}
