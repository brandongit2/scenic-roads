//! The test unit, `_gate-test` (docs/inputs.md §4.10): not real data, no build step reads it; its
//! accepted version shows in the status. Its shape: `.jsonl` files of `{"k": "<string>", "v":
//! <number>}` lines. Its checks: an error for a line that doesn't parse (or isn't that shape), a
//! warning for a negative `v`, and a warning for a file removed (so a removal can be held and
//! accepted in the gate's trial).

use super::{Checks, FileCheck, FileEntry, Finding, Level};

/// The test unit's checks, for the unit named (the test unit, or the tests' nested one).
pub struct GateTest(pub &'static str);

/// The shape's check names.
const SHAPE: &str = "gt-shape";
const LINE: &str = "gt-line";
const NEG: &str = "gt-neg";
const REMOVED: &str = "gt-removed";

impl Checks for GateTest {
    fn unit(&self) -> &'static str {
        self.0
    }

    fn version(&self) -> u32 {
        1
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        if !path.ends_with(".jsonl") {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not jsonl"], vec![path.into()], format!("{path} isn't the drop box's shape: _gate-test takes .jsonl files of {{\"k\": …, \"v\": …}} lines")));
            return c;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not utf-8"], vec![path.into()], format!("{path} isn't UTF-8 text")));
            return c;
        };
        let (mut bad, mut neg) = (Vec::new(), Vec::new());
        let mut n = 0usize;
        for (i, line) in text.split('\n').enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Option<serde_json::Value> = serde_json::from_str(line).ok();
            match v.as_ref().and_then(|v| Some((v.get("k")?.as_str()?, v.get("v")?.as_f64()?))) {
                Some((_, x)) => {
                    n += 1;
                    if x < 0.0 {
                        neg.push((i + 1, line.to_string()));
                    }
                }
                None => bad.push((i + 1, line.to_string())),
            }
        }
        let content = |ls: &[(usize, String)]| ls.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>();
        if !bad.is_empty() {
            let found = content(&bad);
            let refs: Vec<&str> = found.iter().map(String::as_str).collect();
            let mut f = Finding::new(LINE, Level::Error, &[path], &refs, vec![path.into()], format!("{path}: {} line{} not {{\"k\": <string>, \"v\": <number>}}", bad.len(), if bad.len() == 1 { "" } else { "s" }));
            f.lines = bad;
            c.findings.push(f);
        }
        if !neg.is_empty() {
            let found = content(&neg);
            let refs: Vec<&str> = found.iter().map(String::as_str).collect();
            let mut f = Finding::new(NEG, Level::Warning, &[path], &refs, vec![path.into()], format!("{path}: {} line{} with a negative v", neg.len(), if neg.len() == 1 { "" } else { "s" }));
            f.lines = neg;
            c.findings.push(f);
        }
        c.facts.insert("lines".into(), n.into());
        c
    }

    fn removed(&self, path: &str, was: &FileEntry) -> Vec<Finding> {
        let lines = was.facts.get("lines").and_then(|v| v.as_u64()).unwrap_or(0);
        vec![Finding::new(REMOVED, Level::Warning, &[path], &[&was.file], vec![path.into()], format!("{path} removed ({lines} line{} accepted)", if lines == 1 { "" } else { "s" }))]
    }
}
