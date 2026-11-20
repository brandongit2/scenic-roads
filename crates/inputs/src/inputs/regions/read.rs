//! How the build reads the regions (docs/inputs.md §4.6, §6.1.4, §6.1.10): from the accepted version
//! alone, the regions listed by the index (each region file's `facts`: its id, name and outline
//! entries), the coverage made from their stored geometry (`accepted_coverage`), the catalogue's
//! drawn regions from their drawn files, and the Regions list's tree from the index's paths.

use super::stored::{Drawn, Geometry};
use super::tiles::Cell;
use crate::inputs::Accepted;
use anyhow::{Context, Result};
use buildstore::coverage::{Coverage, DrawnRegion, Shape};
use buildstore::recipes::Recipe;
use det::Det;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
        if newest.as_deref().is_none_or(|n| !buildstore::records::lead::app_at_least(n, app)) {
            newest = Some(app.to_string());
        }
    }
    Ok(newest)
}

/// Whether a Mac on app `app` may build, given the readers' switches' app as read (`switch_app`):
/// not on an older one; not while that can't be read (unknown).
pub fn may_lead(app: &str, regions: &std::result::Result<Option<String>, String>) -> std::result::Result<(), String> {
    match regions {
        Err(e) => Err(format!("the app the newer readers' switch (the regions', the elevation datasets') was recorded on can't be read now ({e})")),
        Ok(Some(min)) if !buildstore::records::lead::app_at_least(app, min) => Err(format!("its app {app} is older than the one the newer readers' switch (the regions', the elevation datasets') was recorded on ({min})")),
        Ok(_) => Ok(()),
    }
}

/// The app a member needs (its view's "app too old" else): the term's and the readers' switches',
/// whichever is newer.
pub fn needed_app(term: Option<&str>, regions: Option<&str>) -> Option<String> {
    match (term, regions) {
        (Some(t), Some(r)) => Some(if buildstore::records::lead::app_at_least(t, r) { t } else { r }.to_string()),
        (t, r) => t.or(r).map(str::to_string),
    }
}

/// `source`, the records read from the NAS.
pub fn source_now(root: &store::nasfs::NasRoot) -> Result<Accepted> {
    let m = buildstore::records::read_manifest(root)?;
    source(root, &m)
}

/// An accepted region: its file's path, its recipe (id, name, outline entries), its stored files,
/// and whether it's disabled (`disabled = true`: checked, never built).
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub path: String,
    pub recipe: Recipe,
    /// Its stored geometry and drawn files; None for a region that resolved to nothing (a divided
    /// place's waters region handed nothing): the check stores no files for it, and it has no area
    /// to cover or draw.
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
/// its name (its `_group.toml`'s, else the folder's own), its order, its symbol (its
/// `_group.toml`'s), and its regions' ids directly in it. Only folders holding a region at some
/// depth are in the tree (an empty one isn't shown).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub path: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<super::symbols::Symbol>,
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
        if let Some(mut g) =
            f.facts.get("group").and_then(|v| serde_json::from_value::<super::file::Group>(v.clone()).ok())
        {
            g.symbol = f.facts.get("symbol").and_then(|v| serde_json::from_value(v.clone()).ok());
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
            symbol: g.and_then(|g| g.symbol.clone()),
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
/// unless it's a hand-out's empty waters region, `rg-empty`; an enabled one is still nothing to
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

/// The coverage of the regions' accepted version (§6.1.4): its enabled regions' stored geometry (a
/// disabled region is no coverage). Where there's a `cache` (this Mac's), kept there by the version
/// (`coverage/<hash>.<format>.cov`): the geometry files are read once per version and Mac, by one process at
/// a time (`store::cachefile::making`), and the file read in place (mapped; a cache on a network
/// filesystem isn't: made in memory, as with none), the folder's files no process holds swept
/// (`sweep`).
pub fn accepted_coverage(root: &store::nasfs::NasRoot, a: &Accepted, cache: Option<&Path>) -> Result<Coverage> {
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
    let at = dir.join(buildstore::coverage::kept_name(&a.version));
    // (Read in place, mapped: the shapes' arrays are the file's pages, the file cache's, not this
    // process's memory, task #1109; written as a stream, never whole in memory: the file holds the
    // grids, about the coverage's own size, task #807.)
    enum Kept {
        Read(Coverage),
        /// Not there.
        None,
        /// There, but not read: another format's, or not a coverage.
        Unread,
        /// On a network filesystem: never mapped (store::cachefile::map), so no cache.
        Remote,
    }
    let read = || match store::cachefile::map(&at) {
        Ok(Some(m)) => buildstore::coverage::from_kept(m).map_or(Kept::Unread, Kept::Read),
        Ok(None) => Kept::None,
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Kept::Remote,
        Err(_) => Kept::Unread,
    };
    match read() {
        Kept::Read(c) => {
            sweep(&dir, &at);
            return Ok(c);
        }
        // (No lock file left on a network folder, never swept.)
        Kept::Remote => return made(),
        Kept::None | Kept::Unread => {}
    }
    // (One process of the Mac makes it at a time, the others waiting to read it in place: each
    // making its own at once held the whole coverage in its memory beside the others', a cost no
    // job's memory is booked for since it reads in place, Coverage::read_peak_mb. The making not
    // held: made here.)
    let making = store::cachefile::making(&at);
    match read() {
        Kept::Read(c) => {
            drop(making);
            sweep(&dir, &at);
            return Ok(c);
        }
        Kept::Remote => return made(),
        // (Another format's: made again in its place, not read past forever.)
        Kept::Unread => drop(store::cachefile::try_remove(&at)),
        Kept::None => {}
    }
    let c = made()?;
    let written = store::cachefile::create(&at, &mut |t| {
        use std::io::Write;
        let mut w =
            std::io::BufWriter::with_capacity(1 << 20, std::fs::File::options().write(true).truncate(true).open(t)?);
        buildstore::coverage::write_kept(&c, &mut w)?;
        w.flush()
    });
    if written.is_ok() {
        // (The one made let go for the file just written, read in place: the same shapes. Mapped
        // before the hold `create` took goes, so no other version's sweep takes it in between.)
        let kept = read();
        store::cachefile::release(&at);
        if let Kept::Read(kept) = kept {
            drop(making);
            sweep(&dir, &at);
            return Ok(kept);
        }
    }
    Ok(c)
}

/// Deletes every file of the kept coverage's folder `dir` but `at` (the version just read in place)
/// that no process holds (store::cachefile::try_remove), each time a coverage is read there, not
/// only as a version is made (task #1150: a version a job still mapped when the next was made had
/// stayed for good): another version's or an older format's file once the last process mapping it
/// lets go, a `.making` lock whose maker is gone (one held or waited on stays: a waiter whose lock
/// file goes takes the one named again, store::cachefile::making), a temporary file whose writer
/// died. A newer format's files are left (`buildstore::coverage::kept_newer`): a newer app's, never
/// taken by the jobs this app leaves running after an update to it.
fn sweep(dir: &Path, at: &Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.path() != at && !buildstore::coverage::kept_newer(&e.file_name().to_string_lossy()) {
            store::cachefile::try_remove(&e.path());
        }
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

/// A scratch folder of region files checked (`scenic-build regions-check`): every finding, each
/// region resolved, with its size, and how they fit (`fit`); `geometry`, a folder each region's
/// stored geometry is written to. Against the records' current pass, the datasets those that
/// `declarations` (a folder of source declarations) names when given; reads, stores nothing.
/// `only` (region ids, or folders and files of the folder): only those checked, with the regions
/// their findings need (`Only`), and only their findings kept. How far it is goes out as
/// `progress:` lines (its stages; within the resolve its steps, `regions::progress`), and each
/// stage is a timings phase; the report says each step's time and the slowest regions. `keep`: a
/// lasting folder of the resolution's pieces and its cap in MB (super::kept::Lasting): those whose
/// reads are as they were taken, the rest made and kept, the folder held to its cap after.
#[allow(clippy::too_many_arguments)]
pub fn check_folder(
    root: &store::nasfs::NasRoot,
    manifest: &BTreeMap<String, String>,
    dir: &Path,
    declarations: Option<&Path>,
    land: Option<&Path>,
    geometry: Option<&Path>,
    only: &[String],
    keep: Option<(&Path, u64)>,
) -> Result<serde_json::Value> {
    use jobrt::jobs::stage;
    use jobrt::timings::phase;
    const STAGES: u64 = 5;
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
    stage(0, STAGES, "regions-check (the datasets read)");
    let ph = phase("the datasets read");
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
    let besides = match keep {
        Some(_) => {
            Some(read_besides(declarations, data.as_ref().and_then(|d| Some(d.as_ref()?.digest.as_str())), land)?)
        }
        None => None,
    };
    let mut checks = super::Regions::new(Some(super::prepare(root, manifest, data)?));
    if let (Some((d, mb)), Some(b)) = (keep, besides) {
        checks.keep_lasting(d, mb, &b)?;
    }
    drop(ph);
    eprintln!(
        "regions-check: datasets read in {:.0} s, footprint {} MB, peak resident {} MB",
        t.elapsed().as_secs_f64(),
        jobrt::sys::footprint().unwrap_or(0) / 1_000_000,
        jobrt::sys::peak_rss() / 1_000_000
    );
    let read = |p: &str| -> Result<std::sync::Arc<Vec<u8>>> {
        Ok(std::sync::Arc::new(std::fs::read(dir.join(p)).with_context(|| format!("read {p}"))?))
    };
    // (`only`: the files of the regions checked, and the cells the passes over the Earth take.)
    stage(1, STAGES, "regions-check (the regions chosen)");
    let chosen = if only.is_empty() {
        None
    } else {
        let _ph = phase("the regions chosen");
        let all = super::resolve::Folder::read(paths.iter().cloned(), &|p| Ok(read(p)?.to_vec()))?;
        let o = Only::choose(&all, only, &accepted_cells(root, manifest)?)?;
        eprintln!(
            "regions-check: {} regions named, {} checked with the regions they need, {} cells looked at",
            o.named.len(),
            o.checked.len(),
            o.window.len()
        );
        paths.retain(|p| o.takes(p, &all));
        checks.window = Some(o.window.clone());
        Some(o)
    };
    let v = crate::inputs::Version { paths: paths.into_iter().collect(), read: &read };
    use crate::inputs::Checks;
    if let Some(why) = checks.not_ready() {
        anyhow::bail!("the regions can't be checked now: {why}");
    }
    stage(2, STAGES, "regions-check (the files checked)");
    let ph = phase("the files checked");
    let mut findings = Vec::new();
    for p in &v.paths {
        findings.extend(checks.file(p, &(v.read)(p)?).findings);
    }
    drop(ph);
    let t = std::time::Instant::now();
    super::geom::reset_peak();
    stage(3, STAGES, "regions-check (the resolve)");
    let ph = phase("the resolve");
    let (folder, out) = checks.resolve(&v)?;
    drop(ph);
    let peak = super::geom::reset_peak();
    eprintln!(
        "regions-check: resolved in {:.0} s, footprint {} MB, peak resident {} MB",
        t.elapsed().as_secs_f64(),
        jobrt::sys::footprint().unwrap_or(0) / 1_000_000,
        jobrt::sys::peak_rss() / 1_000_000
    );
    // (`only`: a region named, as resolved now, within the cells looked at; one moved past them
    // since its accepted version would have its findings and neighbours unseen.)
    if let Some(o) = &chosen {
        let now: BTreeMap<String, BTreeSet<Cell>> = out
            .resolved
            .iter()
            .filter(|(id, _)| o.named.contains(*id))
            .map(|(id, r)| (id.clone(), drawn_cells(&r.drawn)))
            .collect();
        let off = o.outside(&now);
        anyhow::ensure!(
            off.is_empty(),
            "--only: {} resolved outside its accepted cells: check the whole folder",
            off.join(", ")
        );
    }
    let steps = checks.last_steps();
    for s in &steps.0 {
        eprintln!("regions-check: {}: {:.1} s, {} of {}", s.step.what(), s.secs, s.done, s.total);
    }
    checks.finish_lasting("regions-check");
    stage(4, STAGES, "regions-check (the report)");
    let _ph = phase("the report");
    findings.extend(out.problems.iter().map(super::problem_finding));
    findings.extend(super::symbol_findings(&v, &folder)?);
    let mut regions = serde_json::Map::new();
    let (mut total, mut enabled_km2) = (0.0, 0.0);
    if let Some(d) = geometry {
        std::fs::create_dir_all(d)?;
        for (id, res) in &out.resolved {
            std::fs::write(d.join(format!("{id}.rgeo")), res.geometry.encode())?;
        }
    }
    for (id, res) in &out.resolved {
        if chosen.as_ref().is_some_and(|o| !o.named.contains(id)) {
            continue;
        }
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
    if let Some(d) = checks.ctx.as_ref().and_then(|c| c.data.as_ref()) {
        d.land_failed()?;
    }
    // (`only`: the findings about a region named, or its folders' remainders; the others' are
    // those of regions checked only for what they share with them, cut off at the window's edge.)
    let left_out = match &chosen {
        Some(o) => {
            let n = findings.len();
            findings.retain(|f| f.files.iter().any(|p| o.kept_files.contains(p)));
            n - findings.len()
        }
        None => 0,
    };
    Ok(serde_json::json!({
        "files": v.paths.len(),
        "regions": folder.regions.len(),
        "resolved": out.resolved.len(),
        "disabled": folder.regions.values().filter(|(_, r)| r.disabled).count(),
        "km2": total,
        "enabled_km2": enabled_km2,
        "earth_km2": super::geom::area_km2(&super::geom::earth()),
        "most_points_one_operation": peak,
        "fit": fit_report(&out.fit),
        "findings": findings.iter().map(|f| serde_json::json!({"check": f.check(), "message": f.message})).collect::<Vec<_>>(),
        "by_region": regions,
        "only": chosen.as_ref().map(|o| serde_json::json!({"named": o.named, "checked": o.checked.len(), "cells": o.window.len(), "findings_left_out": left_out})),
        "steps": steps.0.iter().map(|s| serde_json::json!({"step": s.step.what(), "s": (s.secs * 10.0).round() / 10.0, "done": s.done, "total": s.total})).collect::<Vec<_>>(),
        "slowest": steps.1.iter().map(|(s, id, secs)| serde_json::json!({"step": s.what(), "region": id, "s": (secs * 10.0).round() / 10.0})).collect::<Vec<_>>(),
    }))
}

/// What a scratch check reads besides the records that their content names don't tell, for its
/// lasting pieces' keys (super::kept::Lasting): each declaration's bytes, each dataset's
/// (`data`: `Datasets::digest`, the blake3 of the bytes read), and the land file's length and time
/// written (read where asked, not whole). (The records' are named by their content; a declaration
/// names its dataset by its source, version and path.)
fn read_besides(declarations: Option<&Path>, data: Option<&str>, land: Option<&Path>) -> Result<String> {
    let mut out = Vec::new();
    if let Some(d) = declarations {
        let mut names: Vec<_> =
            std::fs::read_dir(d).with_context(|| format!("list {}", d.display()))?.collect::<std::io::Result<_>>()?;
        names.sort_by_key(|e| e.file_name());
        for e in names.iter().filter(|e| e.path().is_file()) {
            out.push(format!("{:?} {}", e.file_name(), blake3::hash(&std::fs::read(e.path())?).to_hex()));
        }
    }
    out.extend(data.map(str::to_string));
    if let Some(p) = land {
        let m = std::fs::metadata(p).with_context(|| format!("stat {}", p.display()))?;
        out.push(format!("land {} {} {:?}", p.display(), m.len(), m.modified().ok()));
    }
    Ok(out.join(
        "
",
    ))
}

/// Each accepted region's cells (`tiles::cells_of` each ring's box), by id: from its drawn
/// outline (small: the catalogue's), read off the NAS. Empty when nothing is accepted yet.
fn accepted_cells(
    root: &store::nasfs::NasRoot,
    manifest: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, BTreeSet<Cell>>> {
    let Ok(a) = source(root, manifest) else { return Ok(BTreeMap::new()) };
    let l = listed(&a)?;
    let have: Vec<&Listed> = l.iter().filter(|x| x.stored.is_some()).collect();
    let names: Vec<String> = have.iter().filter_map(|x| Some(x.stored.as_ref()?.drawn.clone())).collect();
    let mut out = BTreeMap::new();
    for ((x, b), n) in have.iter().zip(read_all(root, &names)?).zip(&names) {
        let d: Drawn = serde_json::from_slice(&b).with_context(|| format!("parse {n}"))?;
        out.insert(x.recipe.id.clone(), drawn_cells(&d));
    }
    Ok(out)
}

/// A drawn outline's cells (`tiles::cells_of` each ring's box).
fn drawn_cells(d: &Drawn) -> BTreeSet<Cell> {
    let mut cells = BTreeSet::new();
    for ring in d.shapes.values().flatten().flatten() {
        let e7 = |v: f64| (v * 1e7).round() as i64;
        let xs = ring.iter().map(|p| e7(p[0]));
        let ys = ring.iter().map(|p| e7(p[1]));
        let (Some(x0), Some(x1), Some(y0), Some(y1)) = (xs.clone().min(), xs.max(), ys.clone().min(), ys.max()) else {
            continue;
        };
        cells.extend(super::tiles::cells_of([x0, y0, x1 + 1, y1 + 1]));
    }
    cells
}

/// A scratch check of some regions (`check_folder`'s `only`): those named, and what their findings
/// need: the regions they name (`region:`, `land:`, `group:`, their hand-out's folder), the members
/// of a folder whose remainder holds one of them, and their neighbours, every region whose
/// accepted cells come within a zone's `reach()` of theirs, with what those name in turn. The passes
/// over the Earth's cells take those touching theirs (`window`, one cell round them): past it the
/// neighbours' own neighbours are missing, so what they find there isn't kept. A region named that
/// resolves outside the window (moved since its accepted version) stops the check.
pub struct Only {
    pub named: BTreeSet<String>,
    /// The files whose findings are kept: the named regions', and the remainders of the folders
    /// holding one (a remainder's problem names its own file alone).
    pub kept_files: BTreeSet<String>,
    pub checked: BTreeSet<String>,
    pub window: BTreeSet<Cell>,
}

/// How far a region's neighbours are looked for, in cells (`Only`), north and south (east and
/// west as far on the ground at the cell's latitude): a 200-nmi zone's width (`geom::MAX_BUFFER_M`,
/// the widest buffer, 370 km) past a region's cells, a cell being 2° (222 km): 2. A region touching
/// it is within that (its accepted cells hold its water too); one past it can't reach it with a
/// zone. (The nearest-coast splits' `NEAREST_REACH_M` is where they first gather coasts, from the
/// datasets, doubled as needed: no bound of a neighbour.)
fn reach() -> i64 {
    let cell_m = super::tiles::CELL as f64 * 1e-7 * roadcore::geo::M_PER_DEG;
    (super::geom::MAX_BUFFER_M / cell_m).ceil() as i64
}

impl Only {
    /// `names` chosen from `folder` (a region's id, or a folder's or file's path in it; one naming
    /// nothing is an error), its neighbours found by `cells` (each accepted region's).
    pub fn choose(
        folder: &super::resolve::Folder,
        names: &[String],
        cells: &BTreeMap<String, BTreeSet<Cell>>,
    ) -> Result<Only> {
        let mut named = BTreeSet::new();
        for n in names {
            let n = n.trim_end_matches('/');
            let hit: Vec<&String> = folder
                .regions
                .iter()
                .filter(|(id, (p, _))| {
                    id.as_str() == n || p == n || p.strip_suffix(".toml") == Some(n) || super::file::in_group(n, p)
                })
                .map(|(id, _)| id)
                .collect();
            anyhow::ensure!(
                !hit.is_empty(),
                "--only {n}: no region has that id, and no file or folder of the folder that path"
            );
            named.extend(hit.into_iter().cloned());
        }
        let mut checked = needs(folder, &named);
        // (Its cells: the accepted version's, else, for a region new since, those of the regions it
        // names.)
        let mut own: BTreeSet<Cell> = BTreeSet::new();
        for id in &named {
            match cells.get(id) {
                Some(c) => own.extend(c),
                None => own.extend(checked.iter().filter_map(|d| cells.get(d)).flatten()),
            }
        }
        anyhow::ensure!(
            !own.is_empty(),
            "--only: none of {} has accepted geometry, nor does a region it names: its neighbours can't be told (check the whole folder)",
            named.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        let near = round(&own, reach());
        let neighbours: BTreeSet<String> = cells
            .iter()
            .filter(|(id, c)| folder.regions.contains_key(*id) && !c.is_disjoint(&near))
            .map(|(id, _)| id.clone())
            .collect();
        checked.extend(needs(folder, &neighbours));
        let mut kept_files: BTreeSet<String> = named.iter().map(|id| folder.regions[id].0.clone()).collect();
        for (d, (path, _)) in &folder.remainders {
            if named.iter().any(|id| super::file::in_group(d, &folder.regions[id].0)) {
                kept_files.insert(path.clone());
            }
        }
        Ok(Only { named, kept_files, checked, window: round(&own, 1) })
    }

    /// The regions of `now` (each named one's cells as resolved) not within the window.
    pub fn outside(&self, now: &BTreeMap<String, BTreeSet<Cell>>) -> Vec<String> {
        now.iter().filter(|(_, c)| !c.is_subset(&self.window)).map(|(id, _)| id.clone()).collect()
    }

    /// Whether the check takes file `p`: a region checked, the remainder of a folder holding one, a
    /// special file (its pins, its rules), and every shape and symbol (read only where named).
    pub fn takes(&self, p: &str, folder: &super::resolve::Folder) -> bool {
        if super::resolve::special(p) == Some(super::resolve::Special::Remainder) {
            let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
            return folder
                .regions
                .iter()
                .any(|(id, (path, _))| self.checked.contains(id) && super::file::in_group(dir, path));
        }
        if !p.ends_with(".toml") || super::resolve::is_special(p) {
            return true;
        }
        folder.regions.iter().any(|(id, (path, _))| path == p && self.checked.contains(id))
    }
}

/// `ids` and every region they need, at any depth (`Only`).
fn needs(folder: &super::resolve::Folder, ids: &BTreeSet<String>) -> BTreeSet<String> {
    use super::file::Part;
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut todo: Vec<String> = ids.iter().cloned().collect();
    while let Some(id) = todo.pop() {
        if !out.insert(id.clone()) {
            continue;
        }
        let Some((path, r)) = folder.regions.get(&id) else { continue };
        let mut more: Vec<String> = Vec::new();
        for p in r.parts() {
            match p {
                Part::Region(d) | Part::Land(d) => more.push(d.clone()),
                Part::Group(g) => more.extend(folder.members(g, "")),
                _ => {}
            }
        }
        if let Some(g) = &r.hand_out {
            more.extend(folder.members(g, ""));
        }
        // (A remainder over a folder holding it: that folder's regions, and what it names.)
        for (d, (_, rem)) in &folder.remainders {
            if super::file::in_group(d, path) {
                more.extend(folder.members(d, ""));
                for p in rem.parts() {
                    let mut leaves = Vec::new();
                    p.walk(&mut leaves);
                    for l in leaves {
                        match l {
                            Part::Region(x) | Part::Land(x) => more.push(x.clone()),
                            Part::Group(g) => more.extend(folder.members(g, "")),
                            _ => {}
                        }
                    }
                }
            }
        }
        todo.extend(more.into_iter().filter(|m| !out.contains(m)));
    }
    out
}

/// The cells within `n` cells of `cells`: `n` north and south, east and west as far on the ground at
/// the row's latitude (at most the whole row), across the antimeridian.
fn round(cells: &BTreeSet<Cell>, n: i64) -> BTreeSet<Cell> {
    let mut out = BTreeSet::new();
    for &(x, y) in cells {
        for dy in -n..=n {
            let row = y + dy;
            if !(-45..45).contains(&row) {
                continue;
            }
            let lat = ((row.min(y) as f64 + 0.5) * 2.0).abs().max(((row.max(y) as f64 + 0.5) * 2.0).abs());
            // (Within a hundredth of a cell of a whole number of them: that number.)
            let dx = ((n as f64 / lat.to_radians().dcos().max(1e-3) - 0.01).ceil() as i64).min(90);
            for k in -dx..=dx {
                out.insert(((x + k + 90).rem_euclid(180) - 90, row));
            }
        }
    }
    out
}

/// The cover check's measures of how the regions fit (tiles::Fit), for the report: each pair's
/// overlap, each stretch in no region (the rest's marked), each region's water touching none of
/// its land; areas in km² rounded to 0.000001 (a hair's size measured), points in degrees.
fn fit_report(f: &super::tiles::Fit) -> serde_json::Value {
    let r = |v: f64| (v * 1e6).round() / 1e6;
    let deg = |p: super::geom::Pt| [p[0] as f64 / 1e7, p[1] as f64 / 1e7];
    let edges =
        |e: &[(String, f64)]| e.iter().take(6).map(|(id, l)| serde_json::json!([id, r(*l)])).collect::<Vec<_>>();
    let mut gaps: Vec<&super::tiles::GapFit> = f.gaps.iter().collect();
    gaps.sort_by(|a, b| b.km2.total_cmp(&a.km2));
    serde_json::json!({
        "overlaps": f.overlaps.iter().map(|((a, b), v)| serde_json::json!({"a": a, "b": b, "land_km2": r(v.land_km2), "water_km2": r(v.water_km2), "largest_km2": r(v.largest_km2), "at": deg(v.at)})).collect::<Vec<_>>(),
        "gaps": gaps.iter().map(|g| serde_json::json!({
            "km2": r(g.km2), "zone_km2": r(g.zone_km2), "land_km2": r(g.land_km2), "touches_land": g.touches,
            "at": deg(g.at), "rest": g.rest, "edges_km": edges(&g.edges)})).collect::<Vec<_>>(),
        "strays": f.strays.iter().map(|s| serde_json::json!({"id": s.id, "km2": r(s.km2), "at": deg(s.at), "edges_km": edges(&s.edges)})).collect::<Vec<_>>(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> super::super::resolve::Folder {
        let m: BTreeMap<String, Vec<u8>> = files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
        super::super::resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap()
    }

    fn region(id: &str, body: &str) -> String {
        format!("id = \"{id}\"\nname = \"{id}\"\n{body}")
    }

    /// Task #1523: `--only` checks the regions named, what they name, their neighbours by the
    /// accepted cells (and what those need: a folder's remainder its folder's members), and looks
    /// at the cells within one of theirs; a name naming nothing is an error.
    #[test]
    fn only_takes_the_named_what_they_need_and_their_neighbours() {
        let (a, b, c) = (
            region("a", "outline = [\"osm:1\"]\n"),
            region("b", "outline = [\"region:a\"]\n"),
            region("c", "outline = [\"osm:3\"]\n"),
        );
        let (x, y) = (region("x", "outline = [\"osm:4\"]\n"), region("y", "outline = [\"osm:5\"]\n"));
        let n = region("n", "outline = [\"region:c\"]\n");
        let rem = "of = \"box:0.0,0.0,4.0,4.0\"\n";
        let f = folder(&[
            ("_pins.toml", "osm = \"2026-01-01\"\n"),
            ("a.toml", &a),
            ("b.toml", &b),
            ("c.toml", &c),
            ("n.toml", &n),
            ("p/x.toml", &x),
            ("p/y.toml", &y),
            ("p/_remainder.toml", rem),
            ("q/z.toml", &region("z", "outline = [\"osm:6\"]\n")),
            ("q/_remainder.toml", rem),
        ]);
        assert!(f.remainders.contains_key("p"), "the remainder read");
        let cells: BTreeMap<String, BTreeSet<Cell>> =
            [("a", (0, 0)), ("b", (1, 0)), ("c", (40, 10)), ("x", (0, 1)), ("y", (50, 20))]
                .into_iter()
                .map(|(id, c)| (id.to_string(), BTreeSet::from([c])))
                .collect();
        let ids = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<BTreeSet<String>>();
        let o = Only::choose(&f, &["b".into()], &cells).unwrap();
        assert_eq!(o.named, ids(&["b"]));
        assert_eq!(o.kept_files, ids(&["b.toml"]));
        // (a: b names it, and borders it; x borders it, and its folder's remainder needs y.)
        assert_eq!(o.checked, ids(&["a", "b", "x", "y"]));
        assert!(o.window.contains(&(0, 0)) && o.window.contains(&(2, 1)) && !o.window.contains(&(3, 0)));
        assert!(o.takes("p/_remainder.toml", &f) && o.takes("_pins.toml", &f) && o.takes("s.geojson", &f));
        assert!(o.takes("p/y.toml", &f) && !o.takes("c.toml", &f));
        assert!(!o.takes("q/_remainder.toml", &f), "a remainder of a folder holding no region checked");
        // (Moved since: a named region resolved past the window stops the check.)
        let now = |c: Cell| BTreeMap::from([("b".to_string(), BTreeSet::from([(1, 0), c]))]);
        assert_eq!((o.outside(&now((2, 1))), o.outside(&now((5, 0)))), (vec![], vec!["b".to_string()]));
        // A region in a folder with a remainder: the remainder's findings (about its file) kept.
        let o = Only::choose(&f, &["x".into()], &cells).unwrap();
        assert_eq!(o.kept_files, ids(&["p/x.toml", "p/_remainder.toml"]));
        // A folder names its regions; a file its region.
        assert_eq!(Only::choose(&f, &["p/".into()], &cells).unwrap().named, ids(&["x", "y"]));
        assert_eq!(Only::choose(&f, &["p/x.toml".into()], &cells).unwrap().named, ids(&["x"]));
        // A region new since the accepted version: the cells of those it names.
        let o = Only::choose(&f, &["n".into()], &cells).unwrap();
        assert_eq!((o.checked.clone(), o.window.contains(&(40, 10))), (ids(&["c", "n"]), true));
        assert!(Only::choose(&f, &["nope".into()], &cells).err().unwrap().to_string().contains("--only nope"));
        let none = BTreeMap::new();
        assert!(Only::choose(&f, &["a".into()], &none).err().unwrap().to_string().contains("can't be told"));
    }

    /// The cells round a cell reach as far on the ground east and west as north and south (more
    /// columns toward the poles), across the antimeridian, never past the poles' rows.
    #[test]
    fn round_wraps_and_widens_toward_the_poles() {
        let r = round(&BTreeSet::from([(89, 0)]), 1);
        assert!(r.contains(&(-90, 0)) && r.contains(&(88, 1)) && !r.contains(&(87, 0)));
        let far = round(&BTreeSet::from([(0, 40)]), 1);
        assert!(far.iter().filter(|c| c.1 == 40).count() > 3, "at 80° a cell's row spans more columns");
        assert_eq!(round(&BTreeSet::from([(0, 44)]), 2).iter().map(|c| c.1).max(), Some(44));
        assert_eq!(reach(), 2, "a 200-nmi zone, 370 km, spans 2 cells of 222 km");
    }
}
