//! The regions (docs/inputs.md §6.1): a declare input, `inputs/regions/`, read recursively. One
//! region per file, `<id>.toml` anywhere under it (crate::inputs::regions::file), shapes as GeoJSON
//! (`<name>.geojson`, crate::inputs::regions::shapes), `_pins.toml` naming the OSM pass the folder is
//! written against, the symbols' images and bundles (crate::inputs::regions::symbols); anything
//! else ignored.
//!
//! The check (`Regions`, on the gate) resolves every region against the build's current pass (its
//! outline set, and the regions' extract for any other relation or way: crate::inputs::regions::
//! extract) into its stored geometry (crate::inputs::regions::resolve, ::stored, ::geom), which it
//! stores with the accepted version: per region a `.rgeo` the coverage reads and a drawn `.json` the
//! catalogue and the map's region layer draw, named in the index's `facts.geometry` and `facts.drawn`.
//! Every finding is an error (`rg-*`, §6.1.8). A region whose pin names another pass than the
//! build's is held with its file unchanged (`Checks::holds_unchanged`), keeping its accepted
//! geometry.
//!
//! What's resolved is reused (§6.1.4, task #1423): a region without water whose key
//! (`resolve::key`: everything it reads) is the one its accepted geometry was resolved from takes
//! that geometry; and the folder's whole outcome (its problems, what it needs, which regions
//! resolve from what) is kept with the check's state (`Recorded`, `Checks::memo`) by its key
//! (`resolve::outcome_key`), so a check finding the same inputs as the last resolves nothing.
//!
//! The readers (crate::inputs::regions::read): `accepted_coverage` and the regions as accepted,
//! and, until the migration's proof (§6.1.10), the inputs folder's recipes as before.

pub mod data;
pub mod extract;
pub mod file;
pub mod geom;
pub mod kept;
pub mod land;
pub mod progress;
pub mod read;
pub mod resolve;
pub mod shapes;
pub mod stored;
pub mod symbols;
pub mod tiles;

use super::{Checks, Derived, FileCheck, Finding, Level, Version};
use anyhow::{Context, Result};
use buildstore::outlines::Outlines;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// The unit's name.
pub const UNIT: &str = "regions";

/// The regions' checks: with what they read from the records (`prepared`), or without (a file's
/// own checks, the listing's `takes`).
#[derive(Default)]
pub struct Regions {
    pub ctx: Option<Prepared>,
    memo: resolve::Memo,
    /// Its resolutions kept as pieces (task #897: `Checks::keep_in`).
    kept: kept::Kept,
    needs: Mutex<Vec<String>>,
    /// The accepted version's regions, by path: what each was resolved from and where it's stored,
    /// told by `seed`.
    accepted: Mutex<BTreeMap<String, Stored>>,
    /// The outcomes the last check kept (`recall`), and this check's, made or taken (`memo`), by key.
    recalled: Mutex<BTreeMap<String, Recorded>>,
    recorded: Mutex<BTreeMap<String, Recorded>>,
    /// The step times of the last resolve the last check kept (`Memo::times`): what a resolve
    /// before this check's first weighs its progress by (progress::Weights).
    recalled_times: Mutex<Vec<(progress::Step, f64)>>,
    /// The last outcome made, whole, by its key: a check resolves the candidate, the next version
    /// and derives from it, often the same version.
    last: Mutex<Option<(String, Arc<resolve::Outcome>)>>,
    /// The cells a scratch check of some regions looks at (`resolve::Context::window`;
    /// read::check_folder's `only`); None, as the input check's always is: every cell.
    pub window: Option<std::collections::BTreeSet<tiles::Cell>>,
    /// The last resolve's steps and its slowest named items (`progress::Progress::steps`,
    /// `slowest`): a scratch check's report.
    steps: Mutex<Steps>,
}

/// A resolve's steps, and its `SLOWEST` named items (`Regions::last_steps`).
pub type Steps = (Vec<progress::Took>, Vec<(progress::Step, String, f64)>);

/// How many of a resolve's slowest named items `Regions::last_steps` keeps.
const SLOWEST: usize = 25;

/// An accepted region's resolution: what it was resolved from (`resolved_from`), and its stored
/// geometry's and drawing's content names.
#[derive(Clone, Debug)]
struct Stored {
    from: String,
    geometry: String,
    drawn: String,
}

/// A folder's outcome as a check keeps it for the next (`Checks::memo`), by its key
/// (`resolve::outcome_key`): everything a check reads of a resolution but the geometry, which the
/// accepted version stores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recorded {
    pub key: String,
    pub problems: Vec<Said>,
    /// What it needs of the extract (`extract::Id`s).
    pub needs: Vec<String>,
    /// The regions resolved, each with its key (`resolve::Outcome::keys`).
    pub resolved: BTreeMap<String, String>,
}

/// A problem as kept (`resolve::Problem`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Said {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub warning: bool,
    pub check: String,
    pub about: Vec<String>,
    pub found: Vec<String>,
    pub files: Vec<String>,
    pub message: String,
}

impl Recorded {
    fn of(key: &str, out: &resolve::Outcome) -> Recorded {
        Recorded {
            key: key.to_string(),
            problems: out
                .problems
                .iter()
                .map(|p| Said {
                    warning: p.warning,
                    check: p.check.to_string(),
                    about: p.about.clone(),
                    found: p.found.clone(),
                    files: p.files.clone(),
                    message: p.message.clone(),
                })
                .collect(),
            needs: out.needs.iter().map(ToString::to_string).collect(),
            resolved: out.keys.clone(),
        }
    }

    fn problems(&self) -> Vec<resolve::Problem> {
        self.problems
            .iter()
            .map(|p| resolve::Problem {
                warning: p.warning,
                check: kept::check_name(&p.check),
                about: p.about.clone(),
                found: p.found.clone(),
                files: p.files.clone(),
                message: p.message.clone(),
            })
            .collect()
    }
}

/// What the last check kept for the next (`Checks::memo`): this version's outcomes, and the step
/// times of the last resolve made (progress::Weights: the next resolve's progress by them; none
/// made in this check, those it was told).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Memo {
    fmt: u32,
    outcomes: Vec<Recorded>,
    /// By the step's name: one this app doesn't know (an older or newer app's) is passed over, the
    /// rest kept (task #1581: one such step failed the memo's whole parse, every region resolved
    /// again).
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "known_steps")]
    times: Vec<(progress::Step, f64)>,
}

/// `Memo::times` read: the steps whose names this app knows, with their times.
fn known_steps<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<(progress::Step, f64)>, D::Error> {
    let named: Vec<(serde_json::Value, f64)> = Deserialize::deserialize(d)?;
    Ok(named.into_iter().filter_map(|(step, secs)| Some((serde_json::from_value(step).ok()?, secs))).collect())
}

/// An outcome: made now (whole), or as a check recorded it (its geometry the accepted version's).
enum Got {
    Made(Arc<resolve::Outcome>),
    Recorded(Recorded),
}

impl Got {
    fn problems(&self) -> Vec<resolve::Problem> {
        match self {
            Got::Made(o) => o.problems.clone(),
            Got::Recorded(r) => r.problems(),
        }
    }

    fn needs(&self) -> Vec<String> {
        match self {
            Got::Made(o) => o.needs.iter().map(ToString::to_string).collect(),
            Got::Recorded(r) => r.needs.clone(),
        }
    }
}

/// The version of what a region resolves to (its stored geometry and drawing, and the folder's
/// outcome with its problems' words), in each region's `resolved_from` and the outcome's key
/// (`resolve::outcome_key`). A region's geometry is pinned to
/// what it's resolved from (its file, the shapes and regions it names, the pins, the pass, the
/// datasets: `resolve::Outcome::keys`) and this: a check that finds those as they were keeps the
/// accepted geometry, whatever else changed (a group file's name, another region's file), so
/// terrain keyed by it doesn't move. **Bump it in the same commit as any change to the code that
/// resolves (resolve, tiles, geom, land, stored, shapes, data, the outlines' simplify) which would
/// change what an unchanged region resolves to**: that, and only that, re-derives every region.
/// (2: `disabled`, #495: files an older app held as an unknown field are checked again. 3: #600: a
/// region resolved by cell is drawn with its cells dissolved. 4: #611: the drawn shapes simplified
/// at each point's own latitude (outlines::simplify), Antarctica's no longer five points. 5: #468:
/// a region with water stores its land and its water as shapes apart. 6: #648: the cover check's
/// gaps joined across the cells' edges before the fill and the sizes. 7: #649: one coastline: land
/// and sea in no region along the coastline go to the region with water beside them. 8: #468: the
/// coast's land and sea within COAST_REACH_M go to the nearest region, split at the median line.
/// 9: #468: land mostly in two regions with water said as an overlap; nearest-coast splits reach
/// past 450 km. 10: #468: the 12-nautical-mile line stored with the pins; the coastline's land the
/// pass's own, by cell. 11: #862: roadcore::geo's one Earth and even-odd test, `det`'s maths.
/// 12: #1127: with cover, a stretch in no region over sliver_km2 that is some country's (touching
/// land, or inside a 200-nmi zone) goes to the regions with water it borders, never the rest.
/// 13: #1427: the cover check moves nothing: no fills, no slivers, a hand-out's shared stretch
/// stays the handing region's. 14: #1451: land in no region is a `gap` past `LAND_MIN_KM2`, not
/// the hair, judged by land pieces (past LAND_MIN_KM2, SIMPLIFY_M / 2 wide), in overlaps as in
/// gaps; a remainder's edge in free land is coast to the coast step; the gap says the box round its land.
/// 15: #1452: an area's size measured with its edges straight in longitude and latitude, as
/// drawn (a sliver between an edge and the same line through a point more is none).)
pub const RESOLUTION: u32 = 15;

/// The digest of the code `RESOLUTION` covers, as it was when `RESOLUTION` was last set
/// (build/checks_code.rs `resolution_code`: the resolution's modules, what of this file shapes a
/// resolution, and all they name but the code moving bytes and counting, `PLUMBING` there). A test (`checks_code_tests`) fails when that code changes and
/// this doesn't: bump `RESOLUTION` when the change moves what an unchanged region resolves to (or
/// its problems' words), and set this to the digest the test names either way.
pub const RESOLUTION_CODE: &str = "30f2a9ff8eb0cf8840973748fe3c329b";

/// What `derive` records a region was resolved from (`resolved_from`): its resolution key
/// (`resolve::Outcome::keys`) with `RESOLUTION`.
pub fn resolved_from(key: &str) -> String {
    format!("{RESOLUTION} {key}")
}

/// What the check reads from the records: the build's current pass, its outline set and extract,
/// and other passes' outline sets (for a pin naming one of them).
#[derive(Default)]
pub struct Prepared {
    pub pass: Option<String>,
    pub outlines: Option<Outlines>,
    pub extract: Option<extract::Extract>,
    pub others: BTreeMap<String, Outlines>,
    /// The regions' datasets (§6.1.7), when given (a scratch check's, the tests').
    pub data: Option<data::Datasets>,
    /// Else where the accepted sources say they are: read only once a region in the folder names
    /// them (`Region::needs_data`), so declaring them costs a check that needs none nothing.
    pub data_from: Option<(store::nasfs::NasRoot, crate::inputs::sources::Sources)>,
    /// Told once: whether they're there to read (`data::absent`), an error's message when that
    /// can't be told now.
    pub data_there: std::sync::OnceLock<std::result::Result<bool, String>>,
    /// Read once: None while they aren't there to read (`Regions::data`), an error's message when
    /// they can't be read.
    pub data_read: std::sync::OnceLock<std::result::Result<Option<data::Datasets>, String>>,
    /// Where the accepted resolutions are read (`Regions::accepted_resolution`), when given.
    pub root: Option<store::nasfs::NasRoot>,
    /// The other passes' outline sets' content names (`others`), for the outcome's key.
    pub others_names: String,
    /// The pass's land's content name (crate::inputs::regions::land), once made.
    pub land: Option<String>,
    /// The outline set's, the extract's and the land's content names.
    pub names: String,
    /// Why the check can't run now: no pass yet, or the current pass's outline set not made yet
    /// (its outlines step pending).
    pub not_ready: Option<String>,
}

impl Regions {
    pub fn new(ctx: Option<Prepared>) -> Regions {
        Regions { ctx, ..Default::default() }
    }

    /// Its resolutions kept as pieces in the lasting folder `dir` too (a scratch check's `--keep`,
    /// the pool's check's in its Mac's cache: kept::Lasting), held to `cap_mb` once done
    /// (`finish_lasting`). `files`: what the check reads besides the records that their names
    /// don't tell (a scratch check's declarations and the files they name, its land file:
    /// `read::check_folder`; none for the pool's, which reads the records alone).
    pub fn keep_lasting(&self, dir: &std::path::Path, cap_mb: u64, files: &str) -> Result<()> {
        // (What the pieces' keys don't name: the other passes' outline sets, which a region
        // pinned to one is compared with, and those files.)
        let others = self.ctx.as_ref().map_or("", |c| c.others_names.as_str());
        self.kept.open_lasting(kept::Lasting::open(dir, &format!("others {others}\n{files}"), cap_mb)?)
    }

    /// The pool's check's pieces kept in its Mac's lasting folder `pool` too (kept::pool_lasting,
    /// task #1566): a check after an edit takes every piece whose reads are as they were. None
    /// given, or one that can't be made: the job's scratch alone, said.
    fn keep_pool(&self, pool: Option<(std::path::PathBuf, u64)>) {
        if let Some((dir, mb)) = pool {
            if let Err(e) = self.keep_lasting(&dir, mb, "") {
                eprintln!("regions: no lasting folder of pieces ({e:#}): kept in the job's scratch alone");
            }
        }
    }

    /// The check done with a lasting folder: what it took of it and made, and the folder held to
    /// its cap, said after `who` (kept::Kept::finish_lasting).
    pub fn finish_lasting(&self, who: &str) {
        self.kept.finish_lasting(who)
    }

    /// The steps of the last resolve made, each with its time, and its slowest named items (a
    /// region's land or water, a remainder): empty before one is made.
    pub fn last_steps(&self) -> Steps {
        self.steps.lock().unwrap().clone()
    }

    /// The step times of the last resolve made, or, before one, those the last check kept.
    fn times(&self) -> Vec<(progress::Step, f64)> {
        let made: Vec<(progress::Step, f64)> = self.steps.lock().unwrap().0.iter().map(|t| (t.step, t.secs)).collect();
        if made.is_empty() {
            self.recalled_times.lock().unwrap().clone()
        } else {
            made
        }
    }

    /// The folder `v` holds, resolved now (or as this check last made it, by the same key). The
    /// process is stopped (exit 3, saying so) if its footprint passes `max_mb()` while it resolves:
    /// a check never takes the Mac down. How far it is goes out as the check's stage under way every
    /// few seconds (crate::inputs::regions::progress, jobrt::jobs::saying).
    pub fn resolve(&self, v: &Version) -> Result<(resolve::Folder, Arc<resolve::Outcome>)> {
        let folder = resolve::Folder::read(v.paths.iter().cloned(), &|p| Ok((v.read)(p)?.to_vec()))?;
        let key = self.outcome_key(&folder)?;
        let out = self.made(&folder, &key)?;
        Ok((folder, out))
    }

    /// The folder `v` holds, its outcome: as this check last made it, or as a check recorded it
    /// (`Recorded`) under the same key; else resolved now.
    fn outcome(&self, v: &Version) -> Result<(resolve::Folder, Got)> {
        let folder = resolve::Folder::read(v.paths.iter().cloned(), &|p| Ok((v.read)(p)?.to_vec()))?;
        let key = self.outcome_key(&folder)?;
        if let Some((k, o)) = self.last.lock().unwrap().as_ref() {
            if *k == key {
                return Ok((folder, Got::Made(o.clone())));
            }
        }
        let had = self.recorded.lock().unwrap().get(&key).cloned();
        if let Some(r) = had.or_else(|| self.recalled.lock().unwrap().get(&key).cloned()) {
            eprintln!(
                "regions: the outcome of {} regions as the last check recorded it: nothing resolved",
                folder.regions.len()
            );
            self.recorded.lock().unwrap().insert(key, r.clone());
            return Ok((folder, Got::Recorded(r)));
        }
        let out = self.made(&folder, &key)?;
        Ok((folder, Got::Made(out)))
    }

    /// The outcome of `folder` under `key`: this check's last, else resolved now (the last one
    /// let go first: one outcome's geometry held at a time).
    fn made(&self, folder: &resolve::Folder, key: &str) -> Result<Arc<resolve::Outcome>> {
        {
            let mut last = self.last.lock().unwrap();
            match last.as_ref() {
                Some((k, o)) if k == key => return Ok(o.clone()),
                _ => *last = None,
            }
        }
        jobrt::sys::footprint_guard("the regions' check", max_mb());
        geom::watch(30, op_budget_s());
        let empty = BTreeMap::new();
        let c = self.ctx.as_ref();
        let data = self.data(folder)?;
        let reuse = |path: &str, key: &str| self.accepted_resolution(path, key);
        let ctx = resolve::Context {
            pass: c.and_then(|c| c.pass.as_deref()),
            outlines: c.and_then(|c| c.outlines.as_ref()),
            extract: c.and_then(|c| c.extract.as_ref()),
            others: c.map_or(&empty, |c| &c.others),
            memo: Some(&self.memo),
            kept: Some(&self.kept),
            reuse: Some(&reuse),
            names: c.map(|c| c.names.clone()).unwrap_or_default(),
            data,
            progress: progress::Progress::weighed(progress::Weights::of(&self.times())),
            window: self.window.as_ref(),
        };
        let out =
            jobrt::jobs::saying(jobrt::jobs::SAY_EVERY, |_| ctx.progress.say(), || resolve::resolve(folder, &ctx));
        *self.steps.lock().unwrap() = (ctx.progress.steps(), ctx.progress.slowest(SLOWEST));
        // (A pause a piece kept met: the check stops there.)
        self.kept.stopped()?;
        if let Some(d) = data {
            d.land_failed()?;
        }
        eprintln!(
            "regions: {} regions resolved, {} of them as the accepted version stores them",
            out.resolved.len(),
            out.reused.len()
        );
        let out = Arc::new(out);
        self.recorded.lock().unwrap().insert(key.to_string(), Recorded::of(key, &out));
        *self.last.lock().unwrap() = Some((key.to_string(), out.clone()));
        Ok(out)
    }

    /// The key of `folder`'s whole outcome (`resolve::outcome_key`) against what the check reads.
    fn outcome_key(&self, folder: &resolve::Folder) -> Result<String> {
        let c = self.ctx.as_ref();
        let data = self.data_names(folder)?;
        let names = c.map(|c| c.names.clone()).unwrap_or_default();
        let a = resolve::Against { names: &names, pass: c.and_then(|c| c.pass.as_deref()), data: &data };
        let key = resolve::outcome_key(folder, &a, c.map_or("", |c| c.others_names.as_str()));
        // (A window's outcome is its cells' alone.)
        Ok(match &self.window {
            Some(w) => store::naming::hash16(format!("{key} window {w:?}").as_bytes()),
            None => key,
        })
    }

    /// A region without water's resolution as the accepted version stores it, when its file is at
    /// `path` and it was resolved from `key` (with this `RESOLUTION`): its geometry and drawing read
    /// back. None when it wasn't, or they can't be read now (it's resolved again).
    fn accepted_resolution(&self, path: &str, key: &str) -> Option<resolve::Resolved> {
        let root = self.ctx.as_ref()?.root.as_ref()?;
        let s = self.accepted.lock().unwrap().get(path).cloned()?;
        if s.from != resolved_from(key) {
            return None;
        }
        let read = || -> Result<resolve::Resolved> {
            let g = stored::Geometry::decode(&root.join(&s.geometry).read()?)?;
            let drawn = serde_json::from_slice(&root.join(&s.drawn).read()?)?;
            Ok(resolve::Resolved { geometry: g, drawn, lakes: Default::default() })
        };
        read()
            .map_err(|e| eprintln!("regions: {path}'s accepted resolution can't be read ({e:#}): resolved again"))
            .ok()
    }
}

impl Prepared {
    /// The pass's land, open (its head read), when the records name it.
    fn land_open(&self, root: &store::nasfs::NasRoot) -> Result<Option<(String, land::Land)>> {
        let Some(n) = &self.land else { return Ok(None) };
        let f = root.join(n).open().with_context(|| format!("open {n}"))?.into_file();
        Ok(Some((n.clone(), land::Land::open(f).with_context(|| format!("read {n}"))?)))
    }
}

impl Regions {
    /// The datasets: given, or read (once) where the accepted sources say, if a region of `folder`
    /// names them. None while they aren't there to read, a state of the inputs, which holds the
    /// regions naming them (`rg-data`) until a check finds them: no `regions-sea` declared, or a
    /// declared dataset's file not in `sources/fetched/` yet (`data::absent`). An error when they're
    /// there and can't be read now (the NAS, a file that doesn't parse, the pass's land): a fault, not
    /// a state, so the check fails and the pool runs it again, rather than its key being recorded
    /// with the regions held for it.
    fn data(&self, folder: &resolve::Folder) -> Result<Option<&data::Datasets>> {
        let Some(c) = self.ctx.as_ref() else { return Ok(None) };
        if let Some(d) = &c.data {
            return Ok(Some(d));
        }
        let Some((root, s)) = self.data_from(folder)? else { return Ok(None) };
        let read = c.data_read.get_or_init(|| {
            (|| -> Result<Option<data::Datasets>> { data::Datasets::read(root, s, c.land_open(root)?) })()
                .map_err(|e| format!("{e:#}"))
        });
        match read {
            Ok(d) => Ok(d.as_ref()),
            Err(e) => anyhow::bail!("the regions' datasets can't be read: {e}"),
        }
    }

    /// Where the datasets `data` reads are, when a region of `folder` names them and they're there
    /// (told once: `Prepared::data_there`). None when they aren't to be read.
    #[allow(clippy::type_complexity)]
    fn data_from(
        &self,
        folder: &resolve::Folder,
    ) -> Result<Option<&(store::nasfs::NasRoot, crate::inputs::sources::Sources)>> {
        let Some(c) = self.ctx.as_ref() else { return Ok(None) };
        let wanted = folder.regions.values().any(|(_, r)| r.needs_data());
        let Some(from) = c.data_from.as_ref().filter(|_| wanted) else { return Ok(None) };
        let there = c.data_there.get_or_init(|| match data::absent(&from.0, &from.1) {
            Ok(Some(why)) => {
                eprintln!("regions: the datasets aren't there to read: {why}");
                Ok(false)
            }
            Ok(None) => Ok(true),
            Err(e) => Err(format!("{e:#}")),
        });
        match there {
            Ok(true) => Ok(Some(from)),
            Ok(false) => Ok(None),
            Err(e) => anyhow::bail!("the regions' datasets can't be read: {e}"),
        }
    }

    /// The datasets' names as the resolution's keys read them (`data`'s, `Datasets::names_of`),
    /// without reading them: "" when none are read.
    fn data_names(&self, folder: &resolve::Folder) -> Result<String> {
        let Some(c) = self.ctx.as_ref() else { return Ok(String::new()) };
        if let Some(d) = &c.data {
            return Ok(d.names.clone());
        }
        Ok(match self.data_from(folder)? {
            Some((_, s)) => data::Datasets::names_of(s, c.land.as_deref()).unwrap_or_default(),
            None => String::new(),
        })
    }
}

/// The most memory (MB, as `sys::footprint` counts it) a process checking the regions may take:
/// `SCENIC_REGIONS_MAX_MB`, else half the Mac's memory (4,000 MB where that can't be read).
pub fn max_mb() -> u64 {
    std::env::var("SCENIC_REGIONS_MAX_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| jobrt::sys::memsize().map_or(4_000, |b| b / 2 / 1_000_000))
}

/// The most seconds one overlay operation of the check may take (`geom::watch`):
/// `SCENIC_REGIONS_OP_BUDGET_S`, else 300.
pub fn op_budget_s() -> u64 {
    std::env::var("SCENIC_REGIONS_OP_BUDGET_S").ok().and_then(|v| v.parse().ok()).unwrap_or(300)
}

/// The current pass in the records, and its outline set's and extract's content names.
fn current(manifest: &BTreeMap<String, String>) -> (Option<String>, Option<&String>, Option<&String>) {
    let date = crate::osmpass::latest_pass_in(manifest);
    let o = date.as_ref().and_then(|d| manifest.get(&format!("sources/osm/{d}/outlines")));
    let e = date.as_ref().and_then(|d| manifest.get(&extract::logical(d)));
    (date, o, e)
}

/// The other passes' outline sets the records name (not `current`'s): each pass's date and its
/// outline set's content name.
fn others_in<'a>(manifest: &'a BTreeMap<String, String>, current: Option<&str>) -> Vec<(&'a str, &'a String)> {
    manifest
        .range("sources/osm/".to_string()..)
        .take_while(|(l, _)| l.starts_with("sources/osm/"))
        .filter_map(|(l, c)| Some((l.strip_prefix("sources/osm/")?.strip_suffix("/outlines")?, c)))
        .filter(|(d, _)| Some(*d) != current && crate::osmpass::is_date(d))
        .collect()
}

/// A file check's fault as a finding.
fn finding(f: &file::Fault, path: &str) -> Finding {
    let check = format!("rg-{}", f.check);
    Finding::new(&check, Level::Error, &[path], &[&f.message], vec![path.to_string()], format!("{path}: {}", f.message))
}

/// The symbols over a version (§6.1.8 `rg-symbol`): an image's name in two places (loose files or
/// bundles' members), a region's or folder's symbol naming an image the version hasn't. (A bundle
/// that can't be read is its own file's finding.)
pub fn symbol_findings(v: &Version, folder: &resolve::Folder) -> Result<Vec<Finding>> {
    let mut places: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &v.paths {
        let name = resolve::file_name(p);
        if symbols::is_bundle(name) {
            if let Ok(m) = symbols::bundle(&(v.read)(p)?) {
                for n in m.into_keys() {
                    places.entry(n).or_default().push(p.clone());
                }
            }
        } else if symbols::Kind::of(name).is_some() {
            places.entry(name.to_string()).or_default().push(p.clone());
        }
    }
    let mut out = Vec::new();
    for (n, ps) in places.iter().filter(|(_, ps)| ps.len() > 1) {
        let message = format!(
            "image {n} is in {} places: {} (an image's name is unique across every folder)",
            ps.len(),
            ps.join(", ")
        );
        let about: Vec<&str> = ps.iter().map(String::as_str).collect();
        out.push(Finding::new("rg-symbol", Level::Error, &about, &[n], ps.clone(), message));
    }
    let mut declared: Vec<(String, symbols::Symbol)> =
        folder.regions.values().filter_map(|(p, r)| Some((p.clone(), r.symbol.clone()?))).collect();
    for p in v.paths.iter().filter(|p| resolve::special(p) == Some(resolve::Special::Group)) {
        let g = std::str::from_utf8(&(v.read)(p)?).ok().and_then(|t| file::parse_group(t).ok());
        if let Some(s) = g.and_then(|g| g.symbol) {
            declared.push((p.clone(), s));
        }
    }
    declared.sort_by(|a, b| a.0.cmp(&b.0));
    for (p, s) in declared.iter().filter(|(_, s)| !places.contains_key(&s.image)) {
        let message = format!(
            "{p}: its symbol's image {} is in no file of the folder (a .svg, .png or .jpg, or a .zip's member)",
            s.image
        );
        out.push(Finding::new("rg-symbol", Level::Error, &[p], &[&s.image], vec![p.clone()], message));
    }
    Ok(out)
}

/// A resolution's problem as a finding.
pub fn problem_finding(p: &resolve::Problem) -> Finding {
    let about: Vec<&str> = p.about.iter().map(String::as_str).collect();
    let found: Vec<&str> = p.found.iter().map(String::as_str).collect();
    Finding::new(
        p.check,
        if p.warning { Level::Warning } else { Level::Error },
        &about,
        &found,
        p.files.clone(),
        p.message.clone(),
    )
}

impl Checks for Regions {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn code(&self) -> &'static str {
        super::code_of(module_path!())
    }

    fn recursive(&self) -> bool {
        true
    }

    fn takes(&self, path: &str) -> bool {
        resolve::takes(path)
    }

    fn takes_ignored(&self, name: &str) -> bool {
        resolve::is_special(name)
    }

    fn holds_unchanged(&self) -> bool {
        true
    }

    fn not_ready(&self) -> Option<String> {
        self.ctx.as_ref().and_then(|c| c.not_ready.clone())
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        let name = resolve::file_name(path);
        // (A symbol's image or bundle: no build reads it; the map's server serves it.)
        if symbols::Kind::of(name).is_some() || symbols::is_bundle(name) {
            c.keyed = Some("-".into());
            let checked = if symbols::is_bundle(name) {
                symbols::bundle(bytes).map(|m| {
                    let hashes: serde_json::Map<String, serde_json::Value> =
                        m.iter().map(|(n, b)| (n.clone(), store::naming::hash16(b).into())).collect();
                    c.facts.insert("images".into(), hashes.into());
                })
            } else {
                symbols::image(name, bytes).map_err(|e| vec![e])
            };
            if let Err(es) = checked {
                let f = |m: &String| finding(&file::Fault { check: "symbol", message: m.clone() }, path);
                c.findings.extend(es.iter().map(f));
            }
            return c;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            if name.ends_with(".toml") {
                c.findings.push(finding(&file::Fault { check: "toml", message: "not UTF-8 text".into() }, path));
            } else {
                c.findings.push(finding(&file::Fault { check: "shape", message: "not UTF-8 text".into() }, path));
            }
            return c;
        };
        let known = resolve::special(path);
        if let Some(new) = resolve::renamed(path) {
            let message = format!("{path} is now {new}: rename it");
            c.findings.push(finding(&file::Fault { check: "special", message }, path));
        } else if resolve::is_special(path) && known.is_none() {
            let message = format!(
                "{name}: a file whose name starts with _ is special, and the special files are {} and {} (at the top), {} (in any folder) and {} (in any but the top)",
                resolve::PINS,
                resolve::RULES,
                resolve::GROUP,
                resolve::REMAINDER
            );
            c.findings.push(finding(&file::Fault { check: "special", message }, path));
        } else if known == Some(resolve::Special::Group) {
            match file::parse_group(text) {
                Ok(g) => {
                    c.facts.insert("group".into(), serde_json::to_value(&g).unwrap_or_default());
                    if let Some(s) = &g.symbol {
                        c.facts.insert("symbol".into(), serde_json::to_value(s).unwrap_or_default());
                    }
                    // (A group file reruns nothing: no build reads it.)
                    c.keyed = Some("-".into());
                }
                Err(f) => c.findings.push(finding(&f, path)),
            }
        } else if known == Some(resolve::Special::Rules) {
            match tiles::parse_rules(text) {
                Ok(r) => {
                    c.facts.insert("rules".into(), serde_json::to_value(&r).unwrap_or_default());
                }
                Err(f) => c.findings.push(finding(&f, path)),
            }
        } else if known == Some(resolve::Special::Remainder) {
            if let Err(f) = file::parse_remainder(text) {
                c.findings.push(finding(&f, path));
            }
        } else if known == Some(resolve::Special::Pins) {
            match file::parse_pins(text) {
                Ok(p) => {
                    c.facts.insert("pins".into(), serde_json::to_value(&p).unwrap_or_default());
                }
                Err(f) => c.findings.push(finding(&f, path)),
            }
        } else if name.ends_with(".toml") {
            match file::parse(name, text) {
                Ok(r) => {
                    c.facts.insert("id".into(), r.id.clone().into());
                    c.facts.insert("name".into(), r.name.clone().into());
                    c.facts.insert("outline".into(), r.outline_text().into());
                    // (Only when set, so an enabled region's facts are as before.)
                    if r.disabled {
                        c.facts.insert("disabled".into(), true.into());
                    }
                    if let Some(s) = &r.symbol {
                        c.facts.insert("symbol".into(), serde_json::to_value(s).unwrap_or_default());
                    }
                }
                Err(fs) => c.findings.extend(fs.iter().map(|f| finding(f, path))),
            }
        } else if let Err(e) = shapes::read(bytes) {
            c.findings.push(finding(&file::Fault { check: "shape", message: format!("{e:#}") }, path));
        }
        c
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let (folder, out) = self.outcome(v)?;
        *self.needs.lock().unwrap() = out.needs();
        let mut found: Vec<Finding> = out.problems().iter().map(problem_finding).collect();
        found.extend(symbol_findings(v, &folder)?);
        Ok(found)
    }

    fn needs(&self) -> Vec<String> {
        let mut v = self.needs.lock().unwrap().clone();
        // (The pass they're of, for the lead's job.)
        if !v.is_empty() {
            if let Some(p) = self.ctx.as_ref().and_then(|c| c.pass.as_ref()) {
                v.insert(0, format!("pass {p}"));
            }
        }
        v
    }

    fn derive(&self, v: &Version, keep: &BTreeSet<String>) -> Result<Derived> {
        let accepted: BTreeMap<String, String> =
            self.accepted.lock().unwrap().iter().map(|(p, s)| (p.clone(), s.from.clone())).collect();
        // (Resolved from what it was accepted from, by this version of the resolution: its accepted
        // geometry stays, so terrain keyed by it doesn't move.)
        let stays = |path: &str, key: Option<&String>| {
            keep.contains(path) || key.is_some_and(|k| accepted.get(path) == Some(&resolved_from(k)))
        };
        // (An outcome recorded whose regions all stay: nothing to derive, nothing resolved.)
        let (folder, got) = self.outcome(v)?;
        let out = match got {
            Got::Made(o) => o,
            Got::Recorded(r) => {
                if r.resolved.iter().all(|(id, k)| folder.regions.get(id).is_some_and(|(p, _)| stays(p, Some(k)))) {
                    return Ok(Derived::default());
                }
                self.made(&folder, &r.key)?
            }
        };
        let mut d = Derived::default();
        for (id, res) in &out.resolved {
            let path = &folder.regions[id].0;
            let from = out.keys.get(id).map(|k| resolved_from(k));
            if stays(path, out.keys.get(id)) {
                continue;
            }
            let g = res.geometry.encode();
            let gname = format!("{}/@geometry/{id}.{}.rgeo", super::store_dir(UNIT), store::naming::hash16(&g));
            let dr = serde_json::to_vec(&res.drawn)?;
            let dname = format!("{}/@drawn/{id}.{}.json", super::store_dir(UNIT), store::naming::hash16(&dr));
            let mut facts = serde_json::Map::new();
            facts.insert("geometry".into(), gname.clone().into());
            facts.insert("drawn".into(), dname.clone().into());
            if let Some(f) = from {
                facts.insert("resolved_from".into(), f.into());
            }
            d.facts.insert(path.clone(), (facts, Some(gname.clone())));
            d.store.insert(gname, std::sync::Arc::new(g));
            d.store.insert(dname, std::sync::Arc::new(dr));
        }
        Ok(d)
    }

    fn keep_in(&self, scratch: &std::path::Path) {
        let id =
            jobrt::resume_log::job_id("inputs", &format!("{UNIT} {RESOLUTION} pieces {}", kept::FORMAT), UNIT, &[]);
        self.kept.open(scratch, &id, jobrt::resume_log::on());
        self.keep_pool(kept::pool_lasting());
    }

    fn kept_done(&self) {
        self.kept.finish();
        self.finish_lasting("regions");
    }

    fn seed(&self, accepted: &super::Index) {
        let fact = |e: &super::FileEntry, k: &str| Some(e.facts.get(k)?.as_str()?.to_string());
        *self.accepted.lock().unwrap() = accepted
            .files
            .iter()
            .filter_map(|(p, e)| {
                Some((
                    p.clone(),
                    Stored {
                        from: fact(e, "resolved_from")?,
                        geometry: fact(e, "geometry")?,
                        drawn: fact(e, "drawn")?,
                    },
                ))
            })
            .collect();
    }

    fn recall(&self, memo: &serde_json::Value) {
        // (Kept by an app whose memo this one can't read: none, resolved again.)
        let m: Memo = serde_json::from_value(memo.clone()).unwrap_or_default();
        *self.recalled.lock().unwrap() = m.outcomes.into_iter().map(|r| (r.key.clone(), r)).collect();
        *self.recalled_times.lock().unwrap() = m.times;
    }

    fn memo(&self) -> Option<serde_json::Value> {
        let outcomes: Vec<Recorded> = self.recorded.lock().unwrap().values().cloned().collect();
        let times = self.times();
        (!outcomes.is_empty()).then(|| serde_json::to_value(Memo { fmt: 1, outcomes, times }).unwrap_or_default())
    }

    fn context(&self, root: &store::nasfs::NasRoot, manifest: &BTreeMap<String, String>) -> Result<Vec<String>> {
        let (date, o, e) = current(manifest);
        // (The datasets' declarations, as `prepare` reads them from the accepted sources: those of
        // their roles alone, so a change to another source doesn't check the regions again.)
        let s = crate::inputs::sources::open(root, manifest).context("the accepted sources can't be read now")?;
        let data = data::ROLES
            .iter()
            .map(|r| s.as_ref().and_then(|s| s.role(r).ok()).map_or_else(|| format!("{r} -"), |s| s.named()))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(vec![
            format!("regions pass {}", date.as_deref().unwrap_or("-")),
            format!("regions outlines {}", o.map(String::as_str).unwrap_or("-")),
            format!("regions extract {}", e.map(String::as_str).unwrap_or("-")),
            format!(
                "regions land {}",
                date.as_ref().and_then(|d| manifest.get(&land::logical(d))).map(String::as_str).unwrap_or("-")
            ),
            format!("regions data {data}"),
            // (Other passes' outline sets: what a region pinned to one is compared with.)
            format!(
                "regions others {}",
                others_in(manifest, date.as_deref())
                    .into_iter()
                    .map(|(d, c)| format!("{d} {c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ])
    }

    fn prepared(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
    ) -> Result<Option<Box<dyn Checks>>> {
        Ok(Some(Box::new(Regions::new(Some(prepare(root, manifest, None)?)))))
    }
}

/// What the check reads from the records `manifest` under `root` (`Checks::prepared`): the
/// datasets those the accepted sources declare, or `data` when given (a scratch check's).
pub fn prepare(
    root: &store::nasfs::NasRoot,
    manifest: &BTreeMap<String, String>,
    data: Option<Option<data::Datasets>>,
) -> Result<Prepared> {
    {
        let (date, o, e) = current(manifest);
        let outlines = o.map(|n| Outlines::open(&root.join(n)).with_context(|| format!("open {n}"))).transpose()?;
        let extract = e
            .map(|n| root.join(n).read().with_context(|| format!("read {n}")).and_then(|b| extract::Extract::read(&b)))
            .transpose()?;
        // (Other passes' outline sets in the records: what a pin naming one is compared with.)
        let mut others = BTreeMap::new();
        let mut others_names = Vec::new();
        for (d, c) in others_in(manifest, date.as_deref()) {
            if let Ok(x) = Outlines::open(&root.join(c)) {
                others.insert(d.to_string(), x);
                others_names.push(format!("{d} {c}"));
            }
        }
        let land = date.as_ref().and_then(|d| manifest.get(&land::logical(d))).cloned();
        let names = format!(
            "{} {} {}",
            o.map(String::as_str).unwrap_or("-"),
            e.map(String::as_str).unwrap_or("-"),
            land.as_deref().unwrap_or("-")
        );
        let not_ready = match (&date, &outlines) {
            (None, _) => Some("no OSM pass yet".to_string()),
            (Some(d), None) => Some(format!("the pass {d}'s outline set isn't made yet (its outlines step)")),
            _ => None,
        };
        // (The datasets: given, or where the accepted sources say, read once a region needs them;
        // one that can't be read holds the regions naming it, `rg-data`, saying so.)
        let (data, data_from) = match data {
            Some(d) => (d, None),
            // (The accepted sources unreadable now: the check fails, and runs again, rather than
            // hold the regions that name the datasets.)
            None => (
                None,
                crate::inputs::sources::open(root, manifest)
                    .context("the accepted sources can't be read now")?
                    .map(|s| (root.clone(), s)),
            ),
        };
        let names = match &data {
            Some(d) => format!("{names} {}", d.names),
            None => names,
        };
        Ok(Prepared {
            pass: date,
            outlines,
            extract,
            others,
            data,
            data_from,
            data_there: Default::default(),
            data_read: Default::default(),
            root: Some(root.clone()),
            others_names: others_names.join(", "),
            land,
            names,
            not_ready,
        })
    }
}

/// For tests and scratch runs (`scenic-build regions-fixture`): an accepted version of `recipes`
/// (fixtures: `place:<lon>,<lat>,<km>` a circle, `poly:<file>` a `.poly` in `dir`, an `osm:` part a
/// small square at its id's place) stored under `root`, its index named in `manifest` (which the
/// caller then writes as the records).
pub fn accept_fixture(
    root: &store::nasfs::NasRoot,
    dir: store::nasfs::Loc<'_>,
    recipes: &[buildstore::recipes::Recipe],
    manifest: &mut BTreeMap<String, String>,
) -> String {
    use crate::inputs::{FileEntry, Index};
    let mut index = Index { fmt: 1, unit: UNIT.into(), ..Default::default() };
    let put = |name: &str, b: &[u8]| {
        root.join(name).parent().unwrap().create_dir_all().unwrap();
        root.join(name).write(b).unwrap();
    };
    for r in recipes {
        let mut shapes = Vec::new();
        for entry in &r.outline {
            let rings = match entry.strip_prefix("osm:") {
                Some(id) => {
                    let k: i32 = id.parse::<i32>().unwrap() % 100;
                    vec![vec![
                        [k * 10_000_000, 0],
                        [k * 10_000_000 + 1_000_000, 0],
                        [k * 10_000_000 + 1_000_000, 1_000_000],
                        [k * 10_000_000, 1_000_000],
                    ]]
                }
                None => match entry.strip_prefix("place:") {
                    Some(v) => {
                        let n: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        vec![buildstore::coverage::fixture_circle(n[0], n[1], n[2])]
                    }
                    None => buildstore::coverage::read_poly(
                        &dir.join(entry.trim_start_matches("poly:")).loc().read_to_string().unwrap(),
                    )
                    .unwrap(),
                },
            };
            shapes.push(stored::Shape {
                from: entry.clone(),
                country: String::new(),
                tolerance_m: 0,
                rings,
                water: false,
            });
        }
        let g =
            stored::Geometry { id: r.id.clone(), osm: "2026-09-28".into(), shapes, relations: vec![], ways: vec![] }
                .encode();
        let gname = format!("{}/@geometry/{}.{}.rgeo", super::store_dir(UNIT), r.id, store::naming::hash16(&g));
        put(&gname, &g);
        let d = serde_json::to_vec(&stored::Drawn {
            fmt: 1,
            id: r.id.clone(),
            name: r.name.clone(),
            outline: r.outline.clone(),
            shapes: Default::default(),
        })
        .unwrap();
        let dname = format!("{}/@drawn/{}.{}.json", super::store_dir(UNIT), r.id, store::naming::hash16(&d));
        put(&dname, &d);
        let text = toml::to_string(r).unwrap();
        let path = format!("{}.toml", r.id);
        let file = super::copy_name(UNIT, &path, &store::naming::hash16(text.as_bytes())).unwrap();
        put(&file, text.as_bytes());
        let mut facts = serde_json::Map::new();
        for (k, v) in [("id", r.id.clone()), ("name", r.name.clone()), ("geometry", gname.clone()), ("drawn", dname)] {
            facts.insert(k.into(), v.into());
        }
        facts.insert("outline".into(), r.outline.clone().into());
        index.files.insert(path, FileEntry { file, size: text.len() as u64, keyed: gname, facts });
    }
    let ib = serde_json::to_vec(&index).unwrap();
    let name = format!("{}/@index.{}.json", super::store_dir(UNIT), store::naming::hash16(&ib));
    put(&name, &ib);
    manifest.insert(super::logical(UNIT), name.clone());
    // (The readers' switch, as publish 1's runbook records it.)
    let sw = br#"{"fmt": 1, "same": true, "app": "development"}"#;
    let swname = format!("{}.{}.json", super::switched_logical(UNIT), store::naming::hash16(sw));
    put(&swname, sw);
    manifest.insert(super::switched_logical(UNIT), swname);
    name
}

#[cfg(test)]
mod keep_tests;
#[cfg(test)]
mod reuse_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod water_tests;
