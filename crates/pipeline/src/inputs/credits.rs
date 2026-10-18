//! Credits and licences from the inputs' own descriptions (docs/inputs.md §4.8, §5.1). Every input
//! whose data reaches the map has a description (`about.toml`, `<name>.toml` beside each dataset, or
//! the declaration itself) with its `credit` and `licence`; its unit's checks parse it
//! (`parse_description`) and keep it in the index with the file's facts (`description`), with the
//! extent read from the data (`extent`: a box, or `"world"`), never typed.
//!
//! The catalog's credits are the accepted descriptions' beside `rules::CREDITS` (until every input
//! has its description): the same rule for which to list (`rules::listed`), a description winning
//! over a `CREDITS` entry of the same `what`. The catalog's key names the descriptions' credit
//! fields (`digest`), so an edited credit gets a new catalog without rebuilding anything.

use super::{Finding, Level};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;

/// A description (§5.1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Description {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// What the map shows from it: the credit's first part.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub what: String,
    /// The source as its terms ask it be named.
    pub credit: String,
    /// An SPDX id where there is one, else the terms' name.
    pub licence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub licence_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub retrieved: String,
    /// ISO 3166 codes, or `["world"]`.
    #[serde(default)]
    pub territory: Vec<String>,
    #[serde(default = "yes")]
    pub redistribute: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The short name a list on the map gives it (an elevation dataset's, by its `code`: §6.8);
    /// none: its `name`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

fn yes() -> bool {
    true
}

/// Where a description's data lies, read from the data (§4.8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Extent {
    /// `"world"`: anywhere.
    World(String),
    /// A box (w, s, e, n).
    Box([f64; 4]),
    /// Boxes (an elevation dataset's 1° cells merged: crate::dem::declared::Measured::cover).
    Boxes(Vec<[f64; 4]>),
}

/// A description file parsed (TOML): it, and the findings its shape raises (an error for each
/// required field missing or of the wrong type). `keyed`: the digest of the fields that enter keys
/// (`territory`, `redistribute`).
pub fn parse_description(path: &str, bytes: &[u8]) -> (Option<Description>, Vec<Finding>, String) {
    let err = |what: &str, msg: String| {
        Finding::new("description", Level::Error, &[path], &[what], vec![path.to_string()], msg)
    };
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return (None, vec![err("utf-8", format!("{path} isn't UTF-8 text"))], String::new()),
    };
    let v: toml::Table = match toml::from_str(text) {
        Ok(v) => v,
        Err(e) => return (None, vec![err("toml", format!("{path} isn't TOML: {}", e.message()))], String::new()),
    };
    let mut fs = Vec::new();
    for req in ["credit", "licence"] {
        if !v.get(req).and_then(|x| x.as_str()).is_some_and(|s| !s.trim().is_empty()) {
            fs.push(err(
                req,
                format!("{path}: `{req}` is required (a source without its credit would break its terms)"),
            ));
        }
    }
    for (k, x) in &v {
        let ok = match k.as_str() {
            "name" | "what" | "credit" | "licence" | "licence_url" | "source" | "notes" | "label" => x.is_str(),
            "retrieved" => x.is_str() || x.is_datetime(),
            "redistribute" => x.is_bool(),
            "territory" => {
                x.as_str() == Some("world")
                    || x.as_array().is_some_and(|a| a.iter().all(|c| c.as_str().is_some_and(iso_3166)))
            }
            // (A declaration's own table, and anything the unit's shape adds, are its checks'.)
            _ => true,
        };
        if !ok {
            fs.push(err(k, format!("{path}: `{k}` isn't of its type (docs/inputs.md §5.1)")));
        }
    }
    if !fs.is_empty() {
        return (None, fs, String::new());
    }
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let territory: Vec<String> = match v.get("territory") {
        Some(t) if t.as_str() == Some("world") => vec!["world".into()],
        Some(t) => {
            t.as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect()).unwrap_or_default()
        }
        None => Vec::new(),
    };
    let redistribute = v.get("redistribute").and_then(|x| x.as_bool()).unwrap_or(true);
    let retrieved =
        v.get("retrieved").map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).unwrap_or_default();
    let d = Description {
        name: s("name"),
        what: s("what"),
        credit: s("credit"),
        licence: s("licence"),
        licence_url: s("licence_url"),
        source: s("source"),
        retrieved,
        territory: territory.clone(),
        redistribute,
        notes: s("notes"),
        label: s("label"),
    };
    let keyed =
        store::naming::hash16(format!("territory {}\nredistribute {redistribute}", territory.join(",")).as_bytes());
    (Some(d), Vec::new(), keyed)
}

/// An ISO 3166-1 alpha-2 code (`FR`) or 3166-2 subdivision (`CA-QC`), by its syntax.
pub fn iso_3166(c: &str) -> bool {
    let (country, sub) = c.split_once('-').map_or((c, None), |(a, b)| (a, Some(b)));
    country.len() == 2
        && country.bytes().all(|b| b.is_ascii_uppercase())
        && sub.is_none_or(|s| {
            (1..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
}

/// An accepted description, as the catalog lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Described {
    pub unit: String,
    pub path: String,
    pub description: Description,
    /// None: its extent isn't known (no data read yet): listed nowhere.
    pub extent: Option<Extent>,
    /// An elevation dataset's source byte (§6.8 `code`, 1–255), which its label names on the map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u8>,
}

impl Described {
    /// Its areas for `rules::listed`: none (anywhere) for the world, its box, or (unknown) a box
    /// that meets nothing.
    fn areas(&self) -> Option<Vec<[f64; 4]>> {
        match &self.extent {
            Some(Extent::World(w)) if w == "world" => Some(Vec::new()),
            Some(Extent::Box(b)) => Some(vec![*b]),
            Some(Extent::Boxes(b)) => Some(b.clone()),
            _ => None,
        }
    }

    /// As the catalog carries a credit (docs/formats.md, Catalog `credits`): a heritage register's
    /// with its id (`register`), which the map's popups name their source by.
    pub fn credit(&self) -> serde_json::Value {
        let d = &self.description;
        let what = if d.what.is_empty() { &d.name } else { &d.what };
        let mut v = serde_json::json!({ "what": what, "source": d.credit, "terms": d.licence });
        if let Some((rid, "toml")) =
            (self.unit == super::heritage::UNIT).then(|| super::heritage::split(&self.path)).flatten()
        {
            v["register"] = rid.into();
        }
        match &self.extent {
            Some(Extent::Box(b)) => v["areas"] = serde_json::json!([b]),
            Some(Extent::Boxes(b)) => v["areas"] = serde_json::json!(b),
            _ => {}
        }
        v
    }
}

/// The descriptions an index lists (`facts.description`, `facts.extent`).
pub fn of_index(i: &super::Index) -> Vec<Described> {
    i.files
        .iter()
        .filter_map(|(p, f)| {
            let d: Description = serde_json::from_value(f.facts.get("description")?.clone()).ok()?;
            let extent = f.facts.get("extent").and_then(|e| serde_json::from_value(e.clone()).ok());
            let code = (i.unit == super::elevation::UNIT)
                .then(|| f.facts.get("declaration")?.get("code")?.as_u64())
                .flatten()
                .and_then(|c| u8::try_from(c).ok())
                .filter(|&c| c > 0);
            Some(Described { unit: i.unit.clone(), path: p.clone(), description: d, extent, code })
        })
        .collect()
}

/// Indexes' descriptions as read, by the index's content name.
static READ: Mutex<Option<HashMap<String, Vec<Described>>>> = Mutex::new(None);

/// Every accepted description the records `manifest` name (each unit's index), in unit and path
/// order. An error when an index can't be read now.
pub fn described(root: &Path, manifest: &BTreeMap<String, String>) -> anyhow::Result<Vec<Described>> {
    let mut out = Vec::new();
    for (l, name) in
        manifest.range("sources/inputs/".to_string()..).take_while(|(l, _)| l.starts_with("sources/inputs/"))
    {
        if super::record_of(l).is_none_or(|(_, r)| r != "@index") {
            continue;
        }
        if let Some(d) = READ.lock().unwrap().get_or_insert_with(HashMap::new).get(name) {
            out.extend(d.iter().cloned());
            continue;
        }
        let d = of_index(&super::read_index(root, name)?);
        READ.lock().unwrap().get_or_insert_with(HashMap::new).insert(name.clone(), d.clone());
        out.extend(d);
    }
    Ok(out)
}

/// The digest of the accepted descriptions' credit fields (for the catalog's key: §4.8); None when
/// there are none.
pub fn digest(described: &[Described]) -> Option<String> {
    if described.is_empty() {
        return None;
    }
    let lines: Vec<String> = described
        .iter()
        .map(|d| {
            let mut l = format!("{} {}\n{}", d.unit, d.path, serde_json::to_string(&d.credit()).unwrap_or_default());
            // (An elevation dataset's label too: the catalog's `dem_labels`.)
            if let Some(c) = d.code {
                l.push_str(&format!("\nlabel {c} {}", d.label()));
            }
            l
        })
        .collect();
    Some(store::naming::hash16(lines.join("\n").as_bytes()))
}

impl Described {
    /// Its label on the map: its description's `label`, else its `name`.
    pub fn label(&self) -> &str {
        let d = &self.description;
        if d.label.is_empty() {
            &d.name
        } else {
            &d.label
        }
    }
}

/// The elevation datasets' labels by source byte (the catalog's `dem_labels`, which the map's
/// server names a road's elevation sources by): each code's first in unit and path order.
pub fn dem_labels(described: &[Described]) -> BTreeMap<u8, String> {
    let mut out = BTreeMap::new();
    for d in described {
        if let Some(c) = d.code {
            out.entry(c).or_insert_with(|| d.label().to_string());
        }
    }
    out
}

/// The credits a catalog lists (`rules::catalog_credits`' rule): `rules::CREDITS`' in its order, a
/// description in place of the entry of its `what`, then the other descriptions in unit and path
/// order.
pub fn catalog_credits(
    regions: &[crate::coverage::DrawnRegion],
    extents: &[[i32; 4]],
    described: &[Described],
) -> Vec<serde_json::Value> {
    catalog_credits_with(regions, extents, described, &[])
}

/// `catalog_credits`, the descriptions `also` names (unit, path) listed wherever they are: the depth
/// sources whose data the catalog's depth holds, which reaches 200 nautical miles past the coverage
/// (crate::depth), so a source's territory beside the coverage isn't enough to tell.
pub fn catalog_credits_with(
    regions: &[crate::coverage::DrawnRegion],
    extents: &[[i32; 4]],
    described: &[Described],
    also: &[(String, String)],
) -> Vec<serde_json::Value> {
    let listed: Vec<&Described> = described
        .iter()
        .filter(|d| {
            also.iter().any(|(u, p)| *u == d.unit && *p == d.path)
                || d.areas().is_some_and(|a| crate::rules::listed(&a, regions, extents))
        })
        .collect();
    let what = |d: &Described| {
        if d.description.what.is_empty() {
            d.description.name.clone()
        } else {
            d.description.what.clone()
        }
    };
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut used = vec![false; listed.len()];
    for c in crate::rules::CREDITS {
        match listed.iter().position(|d| what(d) == c.what) {
            Some(i) => {
                if !used[i] {
                    out.push(listed[i].credit());
                    used[i] = true;
                }
            }
            // (A description of it not listed here: it wins all the same.)
            None if described.iter().any(|d| what(d) == c.what) => {}
            None if crate::rules::listed(c.areas, regions, extents) => {
                out.push(serde_json::to_value(c).unwrap_or_default())
            }
            None => {}
        }
    }
    out.extend(listed.iter().zip(&used).filter(|(_, u)| !**u).map(|(d, _)| d.credit()));
    // (One line for the descriptions that credit alike, their areas together: GSI's five elevation
    // layers, one credit.)
    let mut merged: Vec<serde_json::Value> = Vec::new();
    for c in out {
        let same = |m: &serde_json::Value| {
            m["what"] == c["what"]
                && m["source"] == c["source"]
                && m["terms"] == c["terms"]
                && m.get("register") == c.get("register")
        };
        match merged.iter_mut().find(|m| same(m)) {
            Some(m) => match (m.get("areas").cloned(), c.get("areas")) {
                (Some(serde_json::Value::Array(mut a)), Some(serde_json::Value::Array(b))) => {
                    for x in b {
                        if !a.contains(x) {
                            a.push(x.clone());
                        }
                    }
                    m["areas"] = serde_json::Value::Array(a);
                }
                // (Either anywhere: anywhere.)
                _ => {
                    if let Some(o) = m.as_object_mut() {
                        o.remove("areas");
                    }
                }
            },
            None => merged.push(c),
        }
    }
    merged
}
