//! The heritage matches (docs/inputs.md §6.5.4): a declare input, the owner's judgment calls where
//! the merging rule gets two registers' entries wrong. Its drop box, `inputs/heritage-matches/`,
//! holds `*.jsonl` files of lines `{"a": "<register>:<id>", "b": "<register>:<id>", "same": bool,
//! "note": "…"}`: `same: true`, one place (transitively); `same: false`, never merged by the rule.
//! The heritage-sites job applies them before the rule (dem/heritage.py).
//!
//! The checks:
//! - **Errors:** a file that isn't `.jsonl` or isn't UTF-8; lines that aren't a match (not a JSON
//!   object; `a` or `b` not `<register>:<id>`; `same` not true or false; `note` not a string; `a`
//!   equal to `b`), one finding per file listing them; contradictions across the version (a chain
//!   of `same` pairs joining two entries a `same: false` pair separates).
//! - **Warnings:** pairs naming an entry no accepted register has (a register's update dropped it),
//!   one finding per file listing them, when the registers' accepted version can be read.

use super::{Checks, FileCheck, Finding, Level, Version};
use crate::inputs::translations::{about_lines, lines};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// The unit's name.
pub const UNIT: &str = "heritage-matches";

const SHAPE: &str = "hm-shape";
const LINE: &str = "hm-line";
const CONTRA: &str = "hm-contra";
const NOWHERE: &str = "hm-nowhere";

/// The matches' checks: the accepted registers' entries (`<register>:<id>`) when they're known
/// (`Checks::prepared` reads them for the check job).
#[derive(Default)]
pub struct Matches {
    pub entries: Option<HashSet<String>>,
}

/// A match line read.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub a: String,
    pub b: String,
    pub same: bool,
}

/// Whether `s` is `<register>:<id>`.
fn entry_ref(s: &str) -> bool {
    s.split_once(':').is_some_and(|(r, id)| super::heritage::valid_id(r) && !id.is_empty())
}

/// A line's match, or what's wrong with it.
pub fn parse(l: &str) -> std::result::Result<Match, &'static str> {
    let Ok(serde_json::Value::Object(v)) = serde_json::from_str::<serde_json::Value>(l) else {
        return Err("not a JSON object");
    };
    let side = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|s| entry_ref(s)).map(str::to_string);
    let (Some(a), Some(b)) = (side("a"), side("b")) else { return Err("a or b not <register>:<id>") };
    let Some(same) = v.get("same").and_then(|x| x.as_bool()) else { return Err("same not true or false") };
    if v.get("note").is_some_and(|n| !n.is_string()) {
        return Err("a note not a string");
    }
    if a == b {
        return Err("a and b the same entry");
    }
    Ok(Match { a, b, same })
}

/// Union–find over entry refs.
#[derive(Default)]
struct Groups(HashMap<String, String>);

impl Groups {
    fn find(&mut self, a: &str) -> String {
        let mut x = a.to_string();
        loop {
            let p = self.0.entry(x.clone()).or_insert_with(|| x.clone()).clone();
            if p == x {
                return x;
            }
            x = p;
        }
    }

    fn join(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0.insert(hi, lo);
        }
    }
}

impl Checks for Matches {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        if !path.ends_with(".jsonl") {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not jsonl"], vec![path.into()], format!("{path} isn't the drop box's shape: heritage-matches takes .jsonl files of match lines (docs/inputs.md §6.5.4); notes go in how/")));
            return c;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            c.findings.push(Finding::new(
                SHAPE,
                Level::Error,
                &[path],
                &["not utf-8"],
                vec![path.into()],
                format!("{path} isn't UTF-8 text"),
            ));
            return c;
        };
        let (mut bad, mut nowhere) = (Vec::new(), Vec::new());
        let mut n = 0usize;
        for (k, l) in lines(text) {
            match parse(l) {
                Err(why) => bad.push((k, l, why.to_string())),
                Ok(m) => {
                    n += 1;
                    if let Some(es) = &self.entries {
                        let missing: Vec<&str> =
                            [m.a.as_str(), m.b.as_str()].into_iter().filter(|e| !es.contains(*e)).collect();
                        if !missing.is_empty() {
                            nowhere.push((k, l, format!("no accepted register has {}", missing.join(" or "))));
                        }
                    }
                }
            }
        }
        let mut push = |check: &str, level: Level, ls: &[(usize, &str, String)], message: String| {
            if ls.is_empty() {
                return;
            }
            let raw: Vec<&str> = ls.iter().map(|(_, l, _)| *l).collect();
            let shown = ls.iter().map(|(k, _, why)| (*k, why.clone())).collect();
            c.findings.push(about_lines(check, level, path, &raw, shown, message));
        };
        let k = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        push(
            LINE,
            Level::Error,
            &bad,
            format!(
                "{path}: {} not a match ({{\"a\": \"<register>:<id>\", \"b\": …, \"same\": true|false}})",
                k(bad.len(), "line", "lines")
            ),
        );
        push(
            NOWHERE,
            Level::Warning,
            &nowhere,
            format!("{path}: {} naming an entry no accepted register has", k(nowhere.len(), "match", "matches")),
        );
        c.facts.insert("matches".into(), n.into());
        c
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut g = Groups::default();
        let mut never: Vec<(String, usize, String, Match)> = Vec::new();
        for p in &v.paths {
            let b = (v.read)(p)?;
            let Ok(text) = std::str::from_utf8(&b) else { continue };
            for (k, l) in lines(text) {
                let Ok(m) = parse(l) else { continue };
                if m.same {
                    g.join(&m.a, &m.b);
                } else {
                    never.push((p.clone(), k, l.to_string(), m));
                }
            }
        }
        let mut by_file: BTreeMap<String, Vec<(usize, String, String)>> = BTreeMap::new();
        for (p, k, l, m) in never {
            if g.find(&m.a) == g.find(&m.b) {
                by_file.entry(p).or_default().push((k, l, format!("{} and {} are joined by same pairs", m.a, m.b)));
            }
        }
        Ok(by_file
            .into_iter()
            .map(|(p, ls)| {
                let raw: Vec<&str> = ls.iter().map(|(_, l, _)| l.as_str()).collect();
                let shown = ls.iter().map(|(k, _, why)| (*k, why.clone())).collect();
                let mut f = about_lines(
                    CONTRA,
                    Level::Error,
                    &p,
                    &raw,
                    shown,
                    format!(
                        "{p}: {} a chain of same: true pairs joins",
                        if ls.len() == 1 {
                            "a same: false pair".to_string()
                        } else {
                            format!("{} same: false pairs", ls.len())
                        }
                    ),
                );
                f.files = v.paths.iter().cloned().collect();
                f
            })
            .collect())
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        // (The registers' accepted version: a register's update checks the matches again.)
        vec![format!(
            "heritage {}",
            manifest.get(&super::logical(super::heritage::UNIT)).map(String::as_str).unwrap_or("-")
        )]
    }

    fn prepared(&self, root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Box<dyn Checks>>> {
        let Some(a) = super::open(root, manifest, super::heritage::UNIT)? else { return Ok(None) };
        let mut es = HashSet::new();
        for r in super::heritage::registers(&a) {
            let b = std::fs::read(root.join(&r.geojson))?;
            for e in &super::heritage::summary(&b).entries {
                es.insert(format!("{}:{}", r.id, e.id));
            }
        }
        Ok(Some(Box::new(Matches { entries: Some(es) })))
    }
}
