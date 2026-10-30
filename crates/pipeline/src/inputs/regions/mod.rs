//! The regions (docs/inputs.md §6.1): a declare input, `inputs/regions/`, read recursively. One
//! region per file, `<id>.toml` anywhere under it (crate::inputs::regions::file), shapes as GeoJSON
//! (`<name>.geojson`, crate::inputs::regions::shapes), `_pins.toml` naming the OSM pass the folder is
//! written against; anything else ignored.
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
//! The readers (crate::inputs::regions::read): `Coverage::accepted` and the regions as accepted,
//! and, until the migration's proof (§6.1.10), the inputs folder's recipes as before.

pub mod data;
pub mod extract;
pub mod file;
pub mod geom;
pub mod kept;
pub mod land;
pub mod read;
pub mod resolve;
pub mod shapes;
pub mod stored;
pub mod tiles;

use super::{Checks, Derived, FileCheck, Finding, Level, Version};
use crate::outlines::Outlines;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

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
    /// The accepted version's regions, by path: what each was resolved from (`resolved_from`),
    /// told by `seed`.
    accepted_from: Mutex<BTreeMap<String, String>>,
}

/// The version of what a region resolves to (its stored geometry and drawing), in each region's
/// `resolved_from` and the checks' version (`Checks::version`). A region's geometry is pinned to
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
/// pass's own, by cell.)
pub const RESOLUTION: u32 = 10;

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
    pub data_read: std::sync::OnceLock<Option<data::Datasets>>,
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

    /// The folder `v` holds, resolved. The process is stopped (exit 3, saying so) if its footprint
    /// passes `max_mb()` while it resolves: a check never takes the Mac down.
    pub fn resolve(&self, v: &Version) -> Result<(resolve::Folder, resolve::Outcome)> {
        crate::sys::footprint_guard("the regions' check", max_mb());
        geom::watch(30, op_budget_s());
        let folder = resolve::Folder::read(v.paths.iter().cloned(), &|p| Ok((v.read)(p)?.to_vec()))?;
        let empty = BTreeMap::new();
        let c = self.ctx.as_ref();
        let data = self.data(&folder);
        let ctx = resolve::Context {
            pass: c.and_then(|c| c.pass.as_deref()),
            outlines: c.and_then(|c| c.outlines.as_ref()),
            extract: c.and_then(|c| c.extract.as_ref()),
            others: c.map_or(&empty, |c| &c.others),
            memo: Some(&self.memo),
            kept: Some(&self.kept),
            names: c.map(|c| c.names.clone()).unwrap_or_default(),
            data,
        };
        let out = resolve::resolve(&folder, &ctx);
        // (A pause a piece kept met: the check stops there.)
        self.kept.stopped()?;
        if let Some(d) = data {
            d.land_failed()?;
        }
        Ok((folder, out))
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
    /// names them.
    fn data(&self, folder: &resolve::Folder) -> Option<&data::Datasets> {
        let c = self.ctx.as_ref()?;
        let wanted = folder.regions.values().any(|(_, r)| r.needs_data());
        c.data.as_ref().or_else(|| {
            let (root, s) = c.data_from.as_ref().filter(|_| wanted)?;
            c.data_read
                .get_or_init(|| match c.land_open(root).and_then(|l| data::Datasets::read(root, s, l)) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("regions: the datasets can't be read: {e:#}");
                        None
                    }
                })
                .as_ref()
        })
    }
}

/// The most memory (MB, as `sys::footprint` counts it) a process checking the regions may take:
/// `SCENIC_REGIONS_MAX_MB`, else half the Mac's memory (4,000 MB where that can't be read).
pub fn max_mb() -> u64 {
    std::env::var("SCENIC_REGIONS_MAX_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| crate::sys::memsize().map_or(4_000, |b| b / 2 / 1_000_000))
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

/// A file check's fault as a finding.
fn finding(f: &file::Fault, path: &str) -> Finding {
    let check = format!("rg-{}", f.check);
    Finding::new(&check, Level::Error, &[path], &[&f.message], vec![path.to_string()], format!("{path}: {}", f.message))
}

/// A resolution's problem as a finding.
pub fn problem_finding(p: &resolve::Problem) -> Finding {
    let about: Vec<&str> = p.about.iter().map(String::as_str).collect();
    let found: Vec<&str> = p.found.iter().map(String::as_str).collect();
    Finding::new(p.check, Level::Error, &about, &found, p.files.clone(), p.message.clone())
}

impl Checks for Regions {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        RESOLUTION
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
                "{name}: a file whose name starts with _ is special, and the special files are {} and {} (at the top) and {} (in any folder)",
                resolve::PINS,
                resolve::RULES,
                resolve::GROUP
            );
            c.findings.push(finding(&file::Fault { check: "special", message }, path));
        } else if known == Some(resolve::Special::Group) {
            match file::parse_group(text) {
                Ok(g) => {
                    c.facts.insert("group".into(), serde_json::to_value(&g).unwrap_or_default());
                    // (A group's name reruns nothing: no build reads it.)
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
                }
                Err(fs) => c.findings.extend(fs.iter().map(|f| finding(f, path))),
            }
        } else if let Err(e) = shapes::read(bytes) {
            c.findings.push(finding(&file::Fault { check: "shape", message: format!("{e:#}") }, path));
        }
        c
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let (_, out) = self.resolve(v)?;
        *self.needs.lock().unwrap() = out.needs.iter().map(|i| i.to_string()).collect();
        Ok(out.problems.iter().map(problem_finding).collect())
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
        let (folder, out) = self.resolve(v)?;
        let mut d = Derived::default();
        let accepted = self.accepted_from.lock().unwrap().clone();
        for (id, res) in &out.resolved {
            let path = &folder.regions[id].0;
            if keep.contains(path) {
                continue;
            }
            // (Resolved from what it was accepted from, by this version of the resolution: its
            // accepted geometry stays, so terrain keyed by it doesn't move.)
            let from = out.keys.get(id).map(|k| resolved_from(k));
            if from.is_some() && from.as_ref() == accepted.get(path) {
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
        let id = crate::resume_log::job_id("inputs", &format!("{UNIT} {RESOLUTION}"), UNIT, &[]);
        self.kept.open(scratch, &id, crate::resume_log::on());
    }

    fn kept_done(&self) {
        self.kept.finish();
    }

    fn seed(&self, accepted: &super::Index) {
        *self.accepted_from.lock().unwrap() = accepted
            .files
            .iter()
            // (An index of another version of the checks: everything derived again.)
            .filter(|_| accepted.checks == format!("{UNIT} {RESOLUTION}"))
            .filter(|(_, e)| e.facts.contains_key("geometry") && e.facts.contains_key("drawn"))
            .filter_map(|(p, e)| Some((p.clone(), e.facts.get("resolved_from")?.as_str()?.to_string())))
            .collect();
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        let (date, o, e) = current(manifest);
        vec![
            format!("regions pass {}", date.as_deref().unwrap_or("-")),
            format!("regions outlines {}", o.map(String::as_str).unwrap_or("-")),
            format!("regions extract {}", e.map(String::as_str).unwrap_or("-")),
            format!(
                "regions land {}",
                date.as_ref().and_then(|d| manifest.get(&land::logical(d))).map(String::as_str).unwrap_or("-")
            ),
        ]
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
        for (l, c) in manifest.range("sources/osm/".to_string()..).take_while(|(l, _)| l.starts_with("sources/osm/")) {
            let Some(d) = l.strip_prefix("sources/osm/").and_then(|r| r.strip_suffix("/outlines")) else { continue };
            if Some(d) != date.as_deref() && crate::osmpass::is_date(d) {
                if let Ok(x) = Outlines::open(&root.join(c)) {
                    others.insert(d.to_string(), x);
                }
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
            None => (None, crate::inputs::sources::open(root, manifest).ok().flatten().map(|s| (root.clone(), s))),
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
            data_read: Default::default(),
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
    recipes: &[crate::agent::recipes::Recipe],
    manifest: &mut BTreeMap<String, String>,
) -> String {
    use crate::inputs::{FileEntry, Index};
    let mut index = Index { fmt: 1, unit: UNIT.into(), checks: format!("{UNIT} 1"), ..Default::default() };
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
                        vec![crate::coverage::fixture_circle(n[0], n[1], n[2])]
                    }
                    None => crate::coverage::read_poly(
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
mod tests;
#[cfg(test)]
mod water_tests;
