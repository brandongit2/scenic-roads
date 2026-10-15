//! The regions (docs/inputs.md §6.1): a declare input, `inputs/regions/`, read recursively. One
//! region per file, `<id>.toml` anywhere under it (crate::inputs::regions::file), shapes as GeoJSON
//! (`<name>.geojson`, crate::inputs::regions::shapes), `pins.toml` naming the OSM pass the folder is
//! written against; anything else ignored.
//!
//! The check (`Regions`, on the gate) resolves every region against the build's current pass (its
//! outline set, and the regions' extract for any other relation or way: crate::inputs::regions::
//! extract) into its stored geometry (crate::inputs::regions::resolve, ::stored, ::geom), which it
//! stores with the accepted version: per region a `.rgeo` the coverage reads and a drawn `.json` the
//! catalog and the map's region layer draw, named in the index's `facts.geometry` and `facts.drawn`.
//! Every finding is an error (`rg-*`, §6.1.8). A region whose pin names another pass than the
//! build's is held with its file unchanged (`Checks::holds_unchanged`), keeping its accepted
//! geometry.
//!
//! The readers (crate::inputs::regions::read): `Coverage::accepted` and the regions as accepted,
//! and, until the migration's proof (§6.1.10), the drop box's recipes as before.

pub mod extract;
pub mod file;
pub mod geom;
pub mod read;
pub mod resolve;
pub mod shapes;
pub mod stored;

use super::{Checks, Derived, FileCheck, Finding, Level, Version};
use crate::outlines::Outlines;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

/// The unit's name.
pub const UNIT: &str = "regions";

/// The regions' checks: with what they read from the records (`prepared`), or without (a file's
/// own checks, the listing's `takes`).
#[derive(Default)]
pub struct Regions {
    pub ctx: Option<Prepared>,
    memo: resolve::Memo,
    needs: Mutex<Vec<String>>,
}

/// What the check reads from the records: the build's current pass, its outline set and extract,
/// and other passes' outline sets (for a pin naming one of them).
#[derive(Default)]
pub struct Prepared {
    pub pass: Option<String>,
    pub outlines: Option<Outlines>,
    pub extract: Option<extract::Extract>,
    pub others: BTreeMap<String, Outlines>,
    /// The outline set's and the extract's content names.
    pub names: String,
    /// Why the check can't run now: no pass yet, or the current pass's outline set not made yet
    /// (its outlines step pending).
    pub not_ready: Option<String>,
}

impl Regions {
    pub fn new(ctx: Option<Prepared>) -> Regions {
        Regions { ctx, ..Default::default() }
    }

    /// The folder `v` holds, resolved.
    pub fn resolve(&self, v: &Version) -> Result<(resolve::Folder, resolve::Outcome)> {
        let folder = resolve::Folder::read(v.paths.iter().cloned(), &|p| Ok((v.read)(p)?.to_vec()))?;
        let empty = BTreeMap::new();
        let c = self.ctx.as_ref();
        let ctx = resolve::Context {
            pass: c.and_then(|c| c.pass.as_deref()),
            outlines: c.and_then(|c| c.outlines.as_ref()),
            extract: c.and_then(|c| c.extract.as_ref()),
            others: c.map_or(&empty, |c| &c.others),
            memo: Some(&self.memo),
            names: c.map(|c| c.names.clone()).unwrap_or_default(),
        };
        let out = resolve::resolve(&folder, &ctx);
        Ok((folder, out))
    }
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
        1
    }

    fn recursive(&self) -> bool {
        true
    }

    fn takes(&self, path: &str) -> bool {
        resolve::takes(path)
    }

    fn takes_ignored(&self, name: &str) -> bool {
        name == resolve::GROUP
    }

    fn holds_unchanged(&self) -> bool {
        true
    }

    fn not_ready(&self) -> Option<String> {
        self.ctx.as_ref().and_then(|c| c.not_ready.clone())
    }

    fn switch(&self, root: &Path, index: &str) -> Result<Option<Vec<u8>>> {
        Ok(match read::proof(root)? {
            Some(p) if p.same && p.index == index => Some(serde_json::to_vec_pretty(&p)?),
            _ => None,
        })
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
        if resolve::is_group(path) {
            match file::parse_group(text) {
                Ok(g) => {
                    c.facts.insert("group".into(), serde_json::to_value(&g).unwrap_or_default());
                    // (A group's name reruns nothing: no build reads it.)
                    c.keyed = Some("-".into());
                }
                Err(f) => c.findings.push(finding(&f, path)),
            }
        } else if resolve::is_pins(path) {
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
                    c.facts.insert("plain".into(), r.is_plain().into());
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
        for (id, res) in &out.resolved {
            let path = &folder.regions[id].0;
            if keep.contains(path) {
                continue;
            }
            let g = res.geometry.encode();
            let gname = format!("{}/@geometry/{id}.{}.rgeo", super::store_dir(UNIT), store::naming::hash16(&g));
            let dr = serde_json::to_vec(&res.drawn)?;
            let dname = format!("{}/@drawn/{id}.{}.json", super::store_dir(UNIT), store::naming::hash16(&dr));
            let mut facts = serde_json::Map::new();
            facts.insert("geometry".into(), gname.clone().into());
            facts.insert("drawn".into(), dname.clone().into());
            d.facts.insert(path.clone(), (facts, Some(gname.clone())));
            d.store.insert(gname, std::sync::Arc::new(g));
            d.store.insert(dname, std::sync::Arc::new(dr));
        }
        Ok(d)
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        let (date, o, e) = current(manifest);
        vec![format!("regions pass {}", date.as_deref().unwrap_or("-")), format!("regions outlines {}", o.map(String::as_str).unwrap_or("-")), format!("regions extract {}", e.map(String::as_str).unwrap_or("-"))]
    }

    fn prepared(&self, root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Box<dyn Checks>>> {
        let (date, o, e) = current(manifest);
        let outlines = o.map(|n| Outlines::open(&root.join(n)).with_context(|| format!("open {n}"))).transpose()?;
        let extract = e.map(|n| std::fs::read(root.join(n)).with_context(|| format!("read {n}")).and_then(|b| extract::Extract::read(&b))).transpose()?;
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
        let names = format!("{} {}", o.map(String::as_str).unwrap_or("-"), e.map(String::as_str).unwrap_or("-"));
        let not_ready = match (&date, &outlines) {
            (None, _) => Some("no OSM pass yet".to_string()),
            (Some(d), None) => Some(format!("the pass {d}'s outline set isn't made yet (its outlines step)")),
            _ => None,
        };
        Ok(Some(Box::new(Regions::new(Some(Prepared { pass: date, outlines, extract, others, names, not_ready })))))
    }
}

#[cfg(test)]
mod tests;
