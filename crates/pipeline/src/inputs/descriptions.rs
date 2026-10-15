//! The descriptions (docs/inputs.md §6.4): a request–fulfil input. The app lists the landmarks and
//! areas without a description (`todo/`, crate::namestodo); the answers are `**/*.jsonl` files
//! dropped anywhere else in `inputs/descriptions/`, a line a thing, `{"qid" | "id", "long", "src",
//! "drop"}`, laid over the popups by the map's server from the accepted copies. Beside them,
//! `about.toml` may describe the input (docs/inputs.md §5.1): its credit goes into the catalog's
//! credits (crate::inputs::credits) in place of `rules::CREDITS`' "Heritage descriptions".
//!
//! The checks:
//! - **Errors:** a file that isn't `.jsonl` (or the top's `about.toml`) or isn't UTF-8; lines that
//!   aren't a description (not a JSON object; neither `qid` nor `id`, or both; a `qid` not
//!   `Q<digits>`, an `id` not `n`, `w` or `r` and digits; `long` missing or empty without `drop`, or
//!   not a string; `src` not a string or a list; `drop` not a boolean), one finding per file listing
//!   them; an `about.toml` without its credit and licence (`credits::parse_description`).
//! - **Warnings,** one finding per file and check listing the lines: a `long` over the writers'
//!   brief's 55 words; a description without `src`.

use super::{Checks, FileCheck, Finding, Level};
use crate::inputs::translations::{about_lines, lines};

/// The unit's name, and its drop box's old place at the NAS's root (before #137).
pub const UNIT: &str = "descriptions";

/// The input's description, at the drop box's top.
pub const ABOUT: &str = "about.toml";

/// The writers' brief's most words (tools/names/descriptions-todo.md).
pub const MOST_WORDS: usize = 55;

/// The descriptions' checks.
pub struct Descriptions;

const SHAPE: &str = "de-shape";
const LINE: &str = "de-line";
const LONG: &str = "de-long";
const SRC: &str = "de-src";

/// Whether `s` is a letter then one or more digits, the first not a 0 for a Wikidata item.
fn id_like(s: &str, first: &[char]) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| first.contains(&c)) && s.len() > 1 && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// What's wrong with a description line, or None.
fn fault(l: &str) -> Option<&'static str> {
    let Ok(serde_json::Value::Object(v)) = serde_json::from_str::<serde_json::Value>(l) else { return Some("not a JSON object") };
    match (v.get("qid"), v.get("id")) {
        (Some(_), Some(_)) => return Some("both qid and id"),
        (None, None) => return Some("neither qid nor id"),
        (Some(q), None) if !q.as_str().is_some_and(|q| id_like(q, &['Q']) && !q[1..].starts_with('0')) => return Some("a qid not Q<digits>"),
        (None, Some(i)) if !i.as_str().is_some_and(|i| id_like(i, &['n', 'w', 'r'])) => return Some("an id not n, w or r and digits"),
        _ => {}
    }
    let drop = match v.get("drop") {
        None => false,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(_) => return Some("a drop not true or false"),
    };
    match v.get("long") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {}
        Some(serde_json::Value::String(_)) | None if drop => {}
        Some(serde_json::Value::String(_)) | None => return Some("no long (and no drop)"),
        Some(_) => return Some("a long not a string"),
    }
    if v.get("src").is_some_and(|s| !s.is_string() && !s.is_array()) {
        return Some("a src not a string or a list");
    }
    None
}

impl Checks for Descriptions {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
    }

    fn recursive(&self) -> bool {
        true
    }

    fn old_folder(&self) -> Option<&'static str> {
        Some(UNIT)
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        if path == ABOUT {
            let (d, findings, keyed) = super::credits::parse_description(path, bytes);
            c.findings = findings;
            if let Some(d) = d {
                // (Its extent: descriptions are of things anywhere; one limited to a territory is
                // listed by it once the things' places are read, not yet: listed nowhere.)
                if d.territory == ["world"] {
                    c.facts.insert("extent".into(), "world".into());
                }
                c.facts.insert("description".into(), serde_json::to_value(&d).unwrap_or_default());
                c.keyed = Some(keyed);
            }
            return c;
        }
        if !path.ends_with(".jsonl") {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not jsonl"], vec![path.into()], format!("{path} isn't the drop box's shape: descriptions takes .jsonl files of description lines and about.toml (docs/inputs.md §6.4); notes go in how/")));
            return c;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not utf-8"], vec![path.into()], format!("{path} isn't UTF-8 text")));
            return c;
        };
        let (mut bad, mut long, mut nosrc) = (Vec::new(), Vec::new(), Vec::new());
        let mut n_lines = 0usize;
        for (n, l) in lines(text) {
            if let Some(why) = fault(l) {
                bad.push((n, l, why.to_string()));
                continue;
            }
            n_lines += 1;
            let v: serde_json::Value = serde_json::from_str(l).unwrap_or_default();
            if v.get("drop").and_then(|d| d.as_bool()) == Some(true) {
                continue;
            }
            let words = v.get("long").and_then(|x| x.as_str()).map_or(0, |s| s.split_whitespace().count());
            if words > MOST_WORDS {
                long.push((n, l, format!("{words} words")));
            }
            if v.get("src").is_none() {
                nosrc.push((n, l, v.get("qid").or_else(|| v.get("id")).and_then(|x| x.as_str()).unwrap_or("").to_string()));
            }
        }
        let mut push = |check: &str, level: Level, ls: &[(usize, &str, String)], message: String| {
            if ls.is_empty() {
                return;
            }
            let raw: Vec<&str> = ls.iter().map(|(_, l, _)| *l).collect();
            let shown = ls.iter().map(|(n, _, why)| (*n, why.clone())).collect();
            c.findings.push(about_lines(check, level, path, &raw, shown, message));
        };
        let k = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        push(LINE, Level::Error, &bad, format!("{path}: {} not a description ({{\"qid\": \"Q…\"}} or {{\"id\": \"n…\"|\"w…\"|\"r…\"}}, \"long\" unless \"drop\": true)", k(bad.len(), "line", "lines")));
        push(LONG, Level::Warning, &long, format!("{path}: {} over the brief's {MOST_WORDS} words", k(long.len(), "description", "descriptions")));
        push(SRC, Level::Warning, &nosrc, format!("{path}: {} without src (what it was written from)", k(nosrc.len(), "description", "descriptions")));
        c.facts.insert("lines".into(), n_lines.into());
        c
    }
}
