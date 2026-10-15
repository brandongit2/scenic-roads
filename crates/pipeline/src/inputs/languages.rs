//! The languages by territory (docs/inputs.md §6.2): a declare input, generated once and rarely
//! touched. Its drop box, `inputs/languages/`, holds `territories.tsv`, one territory a line,
//! `<code>\t<language>,<language>…`, most spoken first (`names::Table`'s reading; `#` lines are
//! comments), and `about.toml` (§5.1: CLDR's version, the Unicode licence, its credit). The
//! `spoken` job builds `global/spoken` from the accepted table (crate::agent::build::spoken_work).
//!
//! The checks:
//! - **Errors:** a file that isn't one of those two, or isn't UTF-8; lines of `territories.tsv`
//!   that aren't two tab-separated fields, whose code isn't ISO 3166 syntax (`FR`, `CA-QC`), whose
//!   list is empty or holds a language that isn't a BCP 47 primary subtag (two or three letters:
//!   `fr`, `yue`), or whose code a line above already has, one finding per file and check listing them;
//!   an `about.toml` without its credit and licence; a version without `territories.tsv` or
//!   without `about.toml` (and so the table's removal).
//! - **Warnings,** one finding per check listing the lines: a code the newest pass's outlines don't
//!   have (its line would have no effect: `outlines::spoken_codes`); a subdivision whose country
//!   has no line.
//!
//! The two files are held together: a change to one is never taken in while the other's is held.

use super::{Checks, FileCheck, Finding, Level, Version};
use crate::inputs::translations::{about_lines, lines};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The unit's name.
pub const UNIT: &str = "languages";

/// The table, and the input's description, at the drop box's top.
pub const TABLE: &str = "territories.tsv";
pub const ABOUT: &str = "about.toml";

/// The languages' checks: `codes`, the codes of the newest pass's outlines when they're known
/// (`Checks::prepared` reads them for the check job).
pub struct Languages {
    pub codes: Option<BTreeSet<String>>,
}

const SHAPE: &str = "la-shape";
const LINE: &str = "la-line";
const TWICE: &str = "la-twice";
const MISSING: &str = "la-missing";
const OUTLINE: &str = "la-outline";
const COUNTRY: &str = "la-country";

/// Whether `c` is an ISO 3166-1 alpha-2 code (`FR`) or ISO 3166-2's syntax (`CA-QC`, `GB-WLS`,
/// `FR-971`).
pub fn valid_code(c: &str) -> bool {
    let two = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_uppercase());
    match c.split_once('-') {
        None => two(c),
        Some((country, sub)) => two(country) && (1..=3).contains(&sub.len()) && sub.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
    }
}

/// Whether `l` is a language as the table names one: a BCP 47 primary language subtag (ISO 639,
/// two or three lower-case letters), which `names::Lang` reads as itself.
pub fn valid_lang(l: &str) -> bool {
    (2..=3).contains(&l.len()) && names::Lang::parse(l).is_some_and(|x| x.as_str() == l)
}

/// What's wrong with a line of the table (its code and languages when nothing is).
fn fault(l: &str) -> std::result::Result<(&str, Vec<&str>), String> {
    let fields: Vec<&str> = l.split('\t').collect();
    let [code, langs] = fields[..] else { return Err("not two tab-separated fields".into()) };
    if !valid_code(code) {
        return Err(format!("{code:?} isn't an ISO 3166 code"));
    }
    if langs.trim().is_empty() {
        return Err("an empty list".into());
    }
    let langs: Vec<&str> = langs.split(',').collect();
    if let Some(bad) = langs.iter().find(|x| !valid_lang(x)) {
        return Err(format!("{bad:?} isn't a language subtag"));
    }
    Ok((code, langs))
}

/// The table's lines that aren't comments: (line number from 1, the line).
fn table_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    lines(text).filter(|(_, l)| !l.starts_with('#'))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The content name of the newest pass's outlines the records `manifest` name.
fn outlines_of(manifest: &BTreeMap<String, String>) -> Option<&String> {
    let date = crate::osmpass::latest_pass_in(manifest)?;
    manifest.get(&format!("sources/osm/{date}/outlines"))
}

impl Checks for Languages {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        if path == ABOUT {
            let (d, findings, keyed) = super::credits::parse_description(path, bytes);
            c.findings = findings;
            if let Some(d) = d {
                if d.territory == ["world"] {
                    c.facts.insert("extent".into(), "world".into());
                }
                c.facts.insert("description".into(), serde_json::to_value(&d).unwrap_or_default());
                c.keyed = Some(keyed);
            }
            return c;
        }
        if path != TABLE {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not the table"], vec![path.into()], format!("{path} isn't the drop box's shape: languages takes {TABLE} and {ABOUT} (docs/inputs.md §6.2); notes go in how/")));
            return c;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not utf-8"], vec![path.into()], format!("{path} isn't UTF-8 text")));
            return c;
        };
        let (mut bad, mut twice, mut orphan) = (Vec::new(), Vec::new(), Vec::new());
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut subs: Vec<(usize, &str, &str)> = Vec::new();
        for (n, l) in table_lines(text) {
            match fault(l) {
                Err(why) => bad.push((n, l, why)),
                Ok((code, _)) if !seen.insert(code) => twice.push((n, l, format!("{code} a second time"))),
                Ok((code, _)) => {
                    if let Some((country, _)) = code.split_once('-') {
                        subs.push((n, l, country));
                    }
                }
            }
        }
        for (n, l, country) in subs {
            if !seen.contains(country) {
                orphan.push((n, l, format!("no line for {country}")));
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
        push(LINE, Level::Error, &bad, format!("{path}: {} not a territory's line (<ISO 3166 code>, a tab, its languages' subtags separated by commas)", plural(bad.len(), "line", "lines")));
        push(TWICE, Level::Error, &twice, format!("{path}: {} a code an earlier line has (one line a code)", plural(twice.len(), "line names", "lines name")));
        push(COUNTRY, Level::Warning, &orphan, format!("{path}: {} whose country has no line", plural(orphan.len(), "subdivision", "subdivisions")));
        // (What affects the raster: the table as read, so a comment edited reruns nothing.)
        let table = names::Table::parse(text);
        c.keyed = Some(store::naming::hash16(table.to_tsv(&[]).as_bytes()));
        c.facts.insert("territories".into(), table.iter().count().into());
        c
    }

    fn removed(&self, path: &str, _was: &super::FileEntry) -> Vec<Finding> {
        if path != TABLE {
            return Vec::new();
        }
        vec![Finding::new(MISSING, Level::Error, &[path], &["removed"], vec![path.into()], format!("{path} removed: the languages spoken where are built from it; put it back (a new one dropped replaces it)"))]
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        let all: Vec<String> = v.paths.iter().cloned().collect();
        for (want, what) in [(TABLE, "the table"), (ABOUT, "its description (CLDR's version, licence and credit: docs/inputs.md §5.1)")] {
            if !v.paths.contains(want) {
                out.push(Finding::new(MISSING, Level::Error, &[want], &["absent"], all.clone(), format!("languages has no {want}: {what}")));
            }
        }
        let (Some(codes), true) = (&self.codes, v.paths.contains(TABLE)) else { return Ok(out) };
        let bytes = (v.read)(TABLE)?;
        let text = String::from_utf8_lossy(&bytes);
        let mut none: Vec<(usize, &str, String)> = Vec::new();
        for (n, l) in table_lines(&text) {
            if let Ok((code, _)) = fault(l) {
                if !codes.contains(code) {
                    none.push((n, l, format!("{code}: no outline")));
                }
            }
        }
        if !none.is_empty() {
            let raw: Vec<&str> = none.iter().map(|(_, l, _)| *l).collect();
            let shown = none.iter().map(|(n, _, why)| (*n, why.clone())).collect();
            out.push(about_lines(OUTLINE, Level::Warning, TABLE, &raw, shown, format!("{TABLE}: {} the newest pass's outlines don't have (no effect on the map)", plural(none.len(), "code", "codes"))));
        }
        Ok(out)
    }

    fn partners(&self, path: &str, all: &BTreeSet<String>) -> Vec<String> {
        // (The table and its description held together: neither is taken in without the other.)
        let other = match path {
            TABLE => ABOUT,
            ABOUT => TABLE,
            _ => return Vec::new(),
        };
        all.contains(other).then(|| other.to_string()).into_iter().collect()
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        // (The newest pass's outlines: a new pass checks the table again.)
        outlines_of(manifest).map(|o| format!("outlines {o}")).into_iter().collect()
    }

    fn prepared(&self, root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Box<dyn Checks>>> {
        let Some(o) = outlines_of(manifest) else { return Ok(None) };
        // (Outlines that can't be read now don't fail the check: it goes on without the "no
        // outline" warnings, which the next pass's check raises.)
        match crate::outlines::Outlines::open(&root.join(o)) {
            Ok(outlines) => Ok(Some(Box::new(Languages { codes: Some(outlines.spoken_codes()) }))),
            Err(e) => {
                eprintln!("inputs: {UNIT}: the pass's outlines, {o}, can't be read ({e:#}): checked without the codes they have");
                Ok(None)
            }
        }
    }
}

/// The table of the accepted version `a` (its `territories.tsv`).
pub fn table_of(a: &super::Accepted) -> Result<names::Table> {
    let b = a.read(TABLE)?;
    Ok(names::Table::parse(std::str::from_utf8(&b).map_err(|_| anyhow::anyhow!("{TABLE} isn't UTF-8"))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inputs::Level;

    fn ids(fs: &[Finding]) -> Vec<(&str, Level)> {
        fs.iter().map(|f| (f.check(), f.level)).collect()
    }

    #[test]
    fn codes_and_languages() {
        assert!(valid_code("FR") && valid_code("CA-QC") && valid_code("GB-WLS") && valid_code("FR-971"));
        assert!(!valid_code("fr") && !valid_code("FRA") && !valid_code("CA-") && !valid_code("CA-QUEB") && !valid_code("C1"));
        assert!(valid_lang("fr") && valid_lang("yue") && !valid_lang("FR") && !valid_lang("hant") && !valid_lang("latn") && !valid_lang("zh_Hant") && !valid_lang("f") && !valid_lang(""));
    }

    #[test]
    fn the_tables_lines_are_checked() {
        let l = Languages { codes: None };
        let ok = l.file(TABLE, b"# CLDR\nCA\ten,fr\nCA-QC\tfr,en\nHK\tyue,en\n");
        assert!(ok.findings.is_empty(), "{:?}", ok.findings);
        assert_eq!(ok.facts["territories"], 3);
        // A comment edited: the same keyed digest.
        assert_eq!(l.file(TABLE, b"# CLDR 47\nCA\ten,fr\nCA-QC\tfr,en\nHK\tyue,en\n").keyed, ok.keyed);
        let bad = l.file(TABLE, b"CA\ten,fr\nFR fr\nfr\tfr\nDE\t\nIT\tit,IT\nCA\ten\nES-CT\tca,es\n");
        assert_eq!(ids(&bad.findings), [("la-line", Level::Error), ("la-twice", Level::Error), ("la-country", Level::Warning)]);
        assert_eq!(bad.findings[0].lines.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [2, 3, 4, 5]);
        assert_eq!(bad.findings[1].lines, [(6, "CA a second time".to_string())]);
        assert_eq!(bad.findings[2].lines, [(7, "no line for ES".to_string())]);
        // Its id names the lines: the same lines elsewhere in the file, the same finding.
        let moved = l.file(TABLE, b"# moved\nCA\ten,fr\nFR fr\nfr\tfr\nDE\t\nIT\tit,IT\nCA\ten\nES-CT\tca,es\n");
        assert_eq!(moved.findings.iter().map(|f| &f.id).collect::<Vec<_>>(), bad.findings.iter().map(|f| &f.id).collect::<Vec<_>>());
        assert_eq!(ids(&l.file("notes.txt", b"x").findings), [("la-shape", Level::Error)]);
        assert_eq!(ids(&l.file(TABLE, b"\xff").findings), [("la-shape", Level::Error)]);
        assert_eq!(ids(&l.file(ABOUT, b"credit = \"CLDR\"\n").findings), [("description", Level::Error)]);
        let about = l.file(ABOUT, b"credit = \"Unicode CLDR\"\nlicence = \"Unicode-3.0\"\nterritory = \"world\"\n");
        assert!(about.findings.is_empty() && about.facts["extent"] == "world");
    }

    #[test]
    fn the_version_whole() {
        let files: BTreeMap<&str, &[u8]> = BTreeMap::from([(TABLE, b"CA\ten,fr\nCA-QC\tfr,en\nXX\tfr\n" as &[u8]), (ABOUT, b"" as &[u8])]);
        let read = |p: &str| -> Result<std::sync::Arc<Vec<u8>>> { Ok(std::sync::Arc::new(files[p].to_vec())) };
        let v = |paths: &[&str]| Version { paths: paths.iter().map(|p| p.to_string()).collect(), read: &read };
        let known = Languages { codes: Some(["CA", "CA-QC", "FR"].map(String::from).into()) };
        assert!(known.whole(&v(&[TABLE, ABOUT])).unwrap().iter().all(|f| f.check() == "la-outline"));
        let w = known.whole(&v(&[TABLE, ABOUT])).unwrap();
        assert_eq!(ids(&w), [("la-outline", Level::Warning)]);
        assert_eq!(w[0].lines, [(3, "XX: no outline".to_string())]);
        // The outlines not known: no such warning.
        assert!(Languages { codes: None }.whole(&v(&[TABLE, ABOUT])).unwrap().is_empty());
        // Without either file: an error.
        assert_eq!(ids(&Languages { codes: None }.whole(&v(&[ABOUT])).unwrap()), [("la-missing", Level::Error)]);
        assert_eq!(ids(&known.whole(&v(&[TABLE])).unwrap()), [("la-missing", Level::Error), ("la-outline", Level::Warning)]);
        let e = super::super::FileEntry { file: String::new(), size: 0, keyed: String::new(), facts: Default::default() };
        assert_eq!(ids(&known.removed(TABLE, &e)), [("la-missing", Level::Error)]);
        assert!(known.removed(ABOUT, &e).is_empty());
    }

    #[test]
    fn the_newest_pass_outlines_are_its_context() {
        let l = Languages { codes: None };
        let m = BTreeMap::from([
            ("sources/osm/2026-08-01/pass".to_string(), "p1".to_string()),
            ("sources/osm/2026-08-01/outlines".to_string(), "o1".to_string()),
            ("sources/osm/2026-09-28/pass".to_string(), "p2".to_string()),
            ("sources/osm/2026-09-28/outlines".to_string(), "o2".to_string()),
        ]);
        assert_eq!(l.context(&m), ["outlines o2"]);
        assert!(l.context(&BTreeMap::new()).is_empty());
        assert!(l.prepared(Path::new("/nonexistent"), &BTreeMap::new()).unwrap().is_none());
        // Outlines named but missing, or not outlines: no error, checked without their codes.
        let d = tempfile::tempdir().unwrap();
        assert!(l.prepared(d.path(), &m).unwrap().is_none());
        std::fs::create_dir_all(d.path().join("o2").parent().unwrap()).unwrap();
        std::fs::write(d.path().join("o2"), b"not outlines").unwrap();
        assert!(l.prepared(d.path(), &m).unwrap().is_none());
    }
}
