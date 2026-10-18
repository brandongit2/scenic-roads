//! The translations (docs/inputs.md §6.3): a request–fulfil input. The app lists the names it has no
//! English for (`todo/`, crate::namestodo); the answers are `**/*.jsonl` files dropped anywhere else
//! in `inputs/translations/`, one translation a line, `{"n", "kind", "langs", "main", "sub", "via"}`,
//! read by the map's server (crate `names`) from the accepted copies.
//!
//! The checks:
//! - **Errors:** a file that isn't `.jsonl` or isn't UTF-8; lines that aren't a translation line
//!   (not JSON, no `n`, `kind`, `langs` or `main`, a `kind` or language that can't be read, a `main`
//!   or `sub` that isn't a string or null), one finding per file listing them; lines in the area
//!   tables' old format (no `kind` or `langs`), likewise.
//! - **Warnings:** `tools/names/check.py`'s rules (crate::inputs::lint), one finding per file
//!   listing the lines it flags. Its id names those lines' content (`Finding::new`), so accepting it
//!   accepts exactly them: a line flagged later, or a flagged line edited, is a new finding.
//!
//! What it shares with the reader: a line's shape is `names::line_shape`'s, the tables' own reading.

use super::{Checks, FileCheck, Finding, Level};
use anyhow::Result;

/// The unit's name, and its drop box's old place at the NAS's root (before #137).
pub const UNIT: &str = "translations";

/// The translations' checks.
pub struct Translations;

const SHAPE: &str = "tr-shape";
const LINE: &str = "tr-line";
const OLD: &str = "tr-old";
const CHECK: &str = "tr-check";

/// A file's lines, split as the tables read them: (line number from 1, its text), blank lines
/// passed over, a byte order mark at the start dropped.
pub(crate) fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.split('\n')
        .enumerate()
        .map(|(i, l)| (i + 1, l.strip_suffix('\r').unwrap_or(l)))
        .filter(|(_, l)| !l.trim().is_empty())
}

/// One finding about lines of a file: its id names the lines' content (`raw`), its `lines` what's
/// shown of each (`shown`).
pub(crate) fn about_lines(
    check: &str,
    level: Level,
    path: &str,
    raw: &[&str],
    shown: Vec<(usize, String)>,
    message: String,
) -> Finding {
    let mut f = Finding::new(check, level, &[path], raw, vec![path.to_string()], message);
    f.lines = shown;
    f
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Checks for Translations {
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
        if !path.ends_with(".jsonl") {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not jsonl"], vec![path.into()], format!("{path} isn't the drop box's shape: translations takes .jsonl files of translation lines (docs/inputs.md §6.3); notes go in how/")));
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
        let (mut bad, mut old) = (Vec::new(), Vec::new());
        let (mut kept, mut not_yet) = (0usize, 0usize);
        for (n, l) in lines(text) {
            let shape = names::line_shape(l.as_bytes());
            // (`main` is required, a string or null; `sub` a string or null when there: the
            // tables' reading lets either be missing.)
            let fields_ok = || {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { return false };
                let str_or_null = |k: &str| v.get(k).is_some_and(|x| x.is_string() || x.is_null());
                str_or_null("main") && (v.get("sub").is_none() || str_or_null("sub"))
            };
            match shape {
                names::LineShape::Line | names::LineShape::NotYet if fields_ok() => {
                    if shape == names::LineShape::Line {
                        kept += 1;
                    } else {
                        not_yet += 1;
                    }
                }
                names::LineShape::Old => old.push((n, l)),
                _ => bad.push((n, l)),
            }
        }
        fn shown(ls: &[(usize, &str)]) -> Vec<(usize, String)> {
            ls.iter().map(|(n, l)| (*n, l.chars().take(300).collect::<String>())).collect()
        }
        fn raw<'a>(ls: &[(usize, &'a str)]) -> Vec<&'a str> {
            ls.iter().map(|(_, l)| *l).collect()
        }
        if !bad.is_empty() {
            c.findings.push(about_lines(LINE, Level::Error, path, &raw(&bad), shown(&bad), format!("{path}: {} not a translation line ({{\"n\", \"kind\", \"langs\", \"main\", \"sub\"}}: kind road, settlement or other; langs ISO 639 subtags; main and sub a string or null)", plural(bad.len(), "line", "lines"))));
        }
        if !old.is_empty() {
            c.findings.push(about_lines(OLD, Level::Error, path, &raw(&old), shown(&old), format!("{path}: {} in the area tables' old format (no kind or langs): give each its kind and langs, as the converted tables have", plural(old.len(), "line", "lines"))));
        }
        c.facts.insert("lines".into(), kept.into());
        if not_yet > 0 {
            c.facts.insert("not_done".into(), not_yet.into());
        }
        c
    }

    fn lint(&self, path: &str, bytes: &[u8]) -> Result<Vec<Finding>> {
        // (Only a file in the shape: one with errors is held whatever check.py says.)
        let Ok(text) = std::str::from_utf8(bytes) else { return Ok(Vec::new()) };
        if !path.ends_with(".jsonl") {
            return Ok(Vec::new());
        }
        let flags = super::lint::flagged(bytes)?;
        if flags.is_empty() {
            return Ok(Vec::new());
        }
        let all: Vec<&str> = text
            .strip_prefix('\u{feff}')
            .unwrap_or(text)
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();
        let line = |n: usize| all.get(n - 1).copied().unwrap_or("");
        let mut raw: Vec<String> = Vec::new();
        let mut shown = Vec::new();
        for f in &flags {
            // (By the lines' content alone, not their numbers nor the checker's words: a line
            // moved by an edit above it is the same line. A stretch is named by all its lines, so
            // an edit to any of them is a new finding.)
            if f.from == f.to {
                raw.push(line(f.from).to_string());
                shown.push((f.from, f.problem.clone()));
            } else {
                raw.push((f.from..=f.to).map(line).collect::<Vec<_>>().join("\n"));
                shown.push((f.from, format!("lines {}–{}: {}", f.from, f.to, f.problem)));
            }
        }
        let refs: Vec<&str> = raw.iter().map(String::as_str).collect();
        Ok(vec![about_lines(
            CHECK,
            Level::Warning,
            path,
            &refs,
            shown,
            format!("{path}: {} the translators' checker (check.py) flags", plural(flags.len(), "line", "lines")),
        )])
    }

    fn context(&self, _manifest: &std::collections::BTreeMap<String, String>) -> Vec<String> {
        // (The checker and the Python it runs in: a change to either checks the unit again, so a
        // held or changed file is read again under it; files already taken in stay as they are.)
        vec![
            format!("check.py {}", store::naming::hash16(super::lint::CHECK.as_bytes())),
            format!("python {}", super::lint::PYTHON_VERSION.trim()),
        ]
    }
}
