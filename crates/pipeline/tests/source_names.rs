//! No data source is named in the app's code (docs/inputs.md §7.2): its sources are declarations
//! (§6), so a source's address or name written into `crates/` (outside tests), `dem/`, `web/src/`,
//! `tools/` (not `tools/taskboard/`) or `scripts/` would be one read around them. `producers/`, where
//! the declarations are made and staged, is exempt.
//!
//! What counts as naming one, in a line's code or strings (comments aside):
//! - a declared source's host, from the declarations staged in `producers/` (each `.toml`'s `source`,
//!   `url`/`urls`, and its index's files' URLs; a path-style S3 URL's bucket with its host), and its
//!   `name` and `credit`, whole or up to their first `,`, `(`, `:` or `;` (two words or more), the
//!   name's first two words in any case, and its id (the file's first word) where it's a name;
//! - any other URL, but those of this machine, the tailnet (100.64.0.0/10), the documentation names
//!   (`example.*`, `.test`, `.invalid`, `192.0.2.*`) and a host of one label (a templated host judged
//!   by what follows its template); and the allowed links (`LINKS`):
//!   OpenStreetMap itself, and Wikidata's and Wikipedia's pages opened from an item. An IRI in angle
//!   brackets (`<http://…>`, a SPARQL query's vocabulary) is the query language's own term.
//!
//! What may: `EXEMPT`, by file, item (a Rust function's or `const`'s, a Python function's or
//! top-level name's; `-` in other languages) and the line's anchor, each with why. Those kept in
//! code by §6.9's table cite its row (`KEPT`), which must say so. An exemption matching no line
//! fails (`every_exemption_matches_a_line`), so the list can't go stale. A name built at run time
//! (joined from parts) isn't seen.
//!
//! scans: crates dem web/src tools scripts producers

mod common;

use common::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The folders scanned, from the repository's root.
const SCOPE: &[&str] = &["crates", "dem", "web/src", "tools", "scripts"];

/// Folders never scanned: the task board (its history), and what's built or installed.
const SKIP_DIRS: &[&str] =
    &["tests", "target", "node_modules", ".venv", "__pycache__", "taskboard", "dist", "dist-publish"];

/// The file types read, by extension.
const EXTS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "toml", "yaml", "yml", "sh", "zsh", "swift", "c", "h",
    "css", "html",
];

/// Links the app may write: the W3C's namespaces (SVG's, a format's), OpenStreetMap (its data and tags are the map's schema; a link opens a
/// feature or a place there), and Wikidata's and Wikipedia's pages, opened from an item's id or
/// title.
const LINKS: &[&str] = &[
    "http://www.w3.org/",
    "https://www.openstreetmap.org/",
    "https://planet.openstreetmap.org/",
    "https://www.wikidata.org/wiki/",
    ".wikipedia.org/wiki/",
];

/// What §6.9's table keeps in code, cited by an exemption's reason as `kept (§6.9, <row>)`; the row
/// must be in the table and say "kept in code".
const KEPT: &[&str] = &["OSM planet", "Planetiler and its data"];

/// Lines naming a source that may: by file, item, anchor (a part of the line), with why.
const EXEMPT: &[(&str, &str, &str, &str)] = &[
    // The credits.
    ("crates/pipeline/src/rules.rs", "CREDITS", "source: \"", "the credits' fallback (§4.8): each entry of a declared source is replaced by its declaration's credit once one is accepted (crate::inputs::credits), and stands in until then, so a catalog never lacks a licence's notice"),
    ("crates/pipeline/src/rules.rs", "CREDITS", "open.canada.ca/en/open-government-licence-canada", "the credits' fallback (§4.8): the licence's own notice"),
    // Content already made, named by its content.
    ("crates/pipeline/src/lakefill/run.rs", "fill", "\"source\": \"CanVec (Natural Resources Canada)", "the lake fill's own metadata, part of its content name: changed, the fill and the water, terrain and catalog reading it would be made again for no change in what they hold"),
    ("crates/pipeline/src/leaftype/mod.rs", "NALCMS_SOURCE", "\"NALCMS 2020 land cover 30 m (CEC)", "a made square's tag, as above"),
    ("crates/pipeline/src/trees/squares.rs", "complete", "starts_with(\"NALCMS\")", "reads the made squares' tags, as above"),
    ("crates/pipeline/src/trees/mod.rs", "META", "Copernicus HRL DLT 2018; NALCMS 2020", "the tree tiles' metadata, part of their content: changed, every tree tile would be made again for nothing"),
    ("crates/pipeline/src/leaftype/mod.rs", "fetch_nalcms", "NALCMS", "the reader of NALCMS's zip, made for its layout (a member streamed out by the declared offset and size): its messages, and its timing phase's name (its key in the timings kept), name what they read"),
    ("crates/pipeline/src/leaftype/mod.rs", "nalcms_centre", "NALCMS", "the reader of NALCMS's GeoTIFF's projection, made for it (the warp is PROJ's Lambert azimuthal equal-area, as GDAL warped it): its messages name what they read"),
    ("crates/pipeline/src/leaftype/mod.rs", "north_america_squares", "\"the NALCMS squares resampled\"", "a timing phase's name, its key in the timings kept (the same phase over time)"),
    ("crates/pipeline/src/bin/scenic-build.rs", "lakefill_step", "\"the lakes filled from CanVec's zips\"", "a timing phase's name, its key in the timings kept (the same phase over time)"),
    ("crates/pipeline/src/bld/overture.rs", "row_group", "Overture's columns changed", "the reader of Overture's GeoParquet schema (the `geoparquet` format is its layout): its messages name what they read"),
    ("crates/pipeline/src/bld/overture.rs", "bbox_spans", "Overture's columns changed", "the reader of Overture's GeoParquet schema, as above"),
    ("crates/pipeline/src/bld/overture.rs", "boxes", "Overture's columns changed", "the reader of Overture's GeoParquet schema, as above"),
    ("crates/pipeline/src/bld/fetch.rs", "overture_jobs", "Overture's release", "the lister of Overture's release folders on S3, its layout: its message names what it reads"),
    ("crates/pipeline/src/leaftype/mod.rs", "EEA_SOURCE", "\"Copernicus HRL Dominant Leaf Type 2018 (EEA), 10 m", "a made square's tag (its GeoTIFF's `source`), as every square already made says it: a square's tag is how it's known (crate::trees::squares::complete), so it doesn't change with the declaration's wording"),
    // Allowlists by host.
    ("crates/pipeline/src/budget/mod.rs", "QUERY_DOMAINS", "\"qlever.dev\"", "the polite-use budget is by host (docs/pool.md §7.7): the query services' own limits, which hold for any request to them"),
    // Links that open a place elsewhere.
    ("web/src/main.ts", "-", "https://www.google.com/maps/", "opens the place in Google Maps"),
    // Tools that check the app.
    ("tools/check/pool-two-macs.sh", "-", "http://en.wikipedia.org/budget-test", "the pool's check of the budgets' proxy: a request the proxy must meter, not a read of the source"),
    // Kept in code (§6.9).
    ("tools/nas/fetch-planet.sh", "-", "MIRRORS=", "kept (§6.9, OSM planet): the NAS's own script, run outside the app"),
    ("scripts/fonts.sh", "-", "base=\"https://raw.githubusercontent.com/protomaps/basemaps-assets", "the app's own assets, the map's fonts (§6.9, also kept), not a data source"),
    ("tools/app/install.sh", "-", "https://docs.astral.sh/uv/", "a link to the tool's documentation in a message"),
];

/// A declared source's markers: what names it, and the declaration it's from.
fn markers(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut tomls = Vec::new();
    files(&root.join("producers"), "toml", &mut tomls);
    tomls.retain(|t| t.file_name().is_some_and(|n| n != "pyproject.toml"));
    let data_words = data_words(&tomls);
    for t in tomls {
        if t.file_name().is_some_and(|n| n == "pyproject.toml") {
            continue;
        }
        let from = t.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        let d: toml::Table =
            toml::from_str(&std::fs::read_to_string(&t).unwrap()).unwrap_or_else(|e| panic!("{from}: {e}"));
        let data = d.get("data").and_then(|v| v.as_table());
        let mut urls: Vec<String> = Vec::new();
        urls.extend(d.get("source").and_then(|v| v.as_str()).map(str::to_string));
        if let Some(data) = data {
            urls.extend(data.get("url").and_then(|v| v.as_str()).map(str::to_string));
            urls.extend(
                data.get("urls")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                    .map(str::to_string),
            );
            if let Some(ix) = data.get("index").and_then(|v| v.as_str()) {
                let p = t.parent().unwrap().join(ix);
                // (An index made where it's dropped isn't staged: its files' hosts aren't known here.)
                if let Ok(s) = std::fs::read_to_string(&p) {
                    let g: serde_json::Value =
                        serde_json::from_str(&s).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                    let hosts: std::collections::BTreeSet<String> = g["features"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|f| f["properties"]["url"].as_str())
                        .filter_map(host_marker)
                        .collect();
                    out.extend(hosts.into_iter().map(|h| (h, from.clone())));
                }
            }
        }
        out.extend(urls.iter().filter_map(|u| host_marker(u)).map(|h| (h, from.clone())));
        for k in ["name", "credit"] {
            let Some(n) = d.get(k).and_then(|v| v.as_str()) else { continue };
            for m in [n, n.split([',', '(', ':', ';']).next().unwrap_or("").trim()] {
                if m.split_whitespace().count() >= 2 && m.starts_with(|c: char| c.is_alphanumeric()) {
                    out.insert(m.to_string(), from.clone());
                }
            }
        }
        // (A name's first two words, any case: `ci:` (as written when both are plain capitalised words); its declaration's id, its file's first word,
        // when that's a word of its name, five letters or more and no role's, format's or
        // encoding's word (a `[data]` value of any declaration): `w:`, as the name writes it, a
        // whole word.)
        let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let words: Vec<&str> =
            name.split(|c: char| !(c.is_alphanumeric() || c == '&' || c == '-')).filter(|w| !w.is_empty()).collect();
        if words.len() >= 2 && words[0].starts_with(|c: char| c.is_alphabetic()) {
            // (Two words of plain English capitalised, "Terrain Tiles", are matched as written: in
            // lower case they're common words.)
            let plain =
                |w: &str| w.chars().next().is_some_and(char::is_uppercase) && w.chars().skip(1).all(char::is_lowercase);
            let pair = format!("{} {}", words[0], words[1]);
            out.insert(
                if plain(words[0]) && plain(words[1]) { pair } else { format!("ci:{}", pair.to_lowercase()) },
                from.clone(),
            );
        }
        let id = t.file_stem().unwrap().to_string_lossy().split('-').next().unwrap_or("").to_lowercase();
        if id.len() >= 5 && !data_words.contains(&id) {
            if let Some(w) = words.iter().find(|w| w.to_lowercase() == id) {
                out.insert(format!("w:{w}"), from.clone());
            }
        }
    }
    out
}

/// Every string in the declarations' `[data]` tables, lowercased: the roles', formats' and
/// encodings' words, never a source's own name.
fn data_words(tomls: &[PathBuf]) -> std::collections::BTreeSet<String> {
    fn walk(v: &toml::Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            toml::Value::String(s) if !s.contains("://") => {
                out.insert(s.to_lowercase());
            }
            toml::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            toml::Value::Table(t) => t.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for t in tomls {
        if let Ok(d) = toml::from_str::<toml::Table>(&std::fs::read_to_string(t).unwrap_or_default()) {
            if let Some(x) = d.get("data") {
                walk(x, &mut out);
            }
        }
    }
    out
}

/// Whether line `l` (`lower`, lowercased) names marker `m` (`ci:` any case; `w:` a whole word;
/// else as written).
fn names(l: &str, lower: &str, m: &str) -> bool {
    if let Some(x) = m.strip_prefix("ci:") {
        return lower.contains(x);
    }
    if let Some(w) = m.strip_prefix("w:") {
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        return l
            .match_indices(w)
            .any(|(k, _)| !word(l[..k].chars().next_back()) && !word(l[k + w.len()..].chars().next()));
    }
    l.contains(m)
}

/// A URL's host as a marker (with its bucket for a path-style S3 URL, whose host is shared).
fn host_marker(u: &str) -> Option<String> {
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    let shared = host
        .strip_suffix(".amazonaws.com")
        .is_some_and(|s| s == "s3" || (s.starts_with("s3.") && !s[3..].contains('.')));
    if shared {
        let bucket = rest[host.len()..].trim_start_matches('/').split(['/', '{']).next().filter(|b| !b.is_empty())?;
        return Some(format!("{host}/{bucket}"));
    }
    Some(host.to_string())
}

/// What follows a host's leading template (`${…}`, `{…}`) and its dot: None if it has none.
fn templated_rest(h: &str) -> Option<&str> {
    let close = if h.starts_with("${") || h.starts_with('{') { h.find('}')? } else { return None };
    Some(h[close + 1..].trim_start_matches('.'))
}

/// Whether a URL's host is one of this machine, the tailnet, the documentation names, or of one
/// label (a placeholder, a Mac's name): not a source.
fn local_host(h: &str) -> bool {
    let h = h.split(':').next().unwrap_or(h);
    let label = |s: &str| h == s || h.ends_with(&format!(".{s}"));
    !h.contains('.')
        || h.starts_with("127.")
        || h.strip_prefix("100.")
            .and_then(|r| r.split('.').next()?.parse::<u8>().ok())
            .is_some_and(|o| (64..128).contains(&o))
        || h.starts_with("192.168.")
        || h.starts_with("10.")
        || h.starts_with("192.0.2.")
        || h.starts_with('[')
        || ["localhost", "local", "ts.net", "test", "invalid", "example", "nip.io"].iter().any(|s| label(s))
        || h.starts_with("example.")
        || h.split('.').any(|l| l == "example")
}

/// The URLs a line names (its scheme and what follows, to a quote, space, bracket or backtick).
fn urls(l: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(k) = l[at..].find("http").map(|k| k + at) {
        let r = &l[k..];
        let len = if r.starts_with("https://") {
            8
        } else if r.starts_with("http://") {
            7
        } else {
            0
        };
        if len == 0 {
            at = k + 4;
            continue;
        }
        let end = r.find(|c: char| c.is_whitespace() || "\"'`<>)]".contains(c)).unwrap_or(r.len());
        out.push(&r[..end]);
        at = k + end.max(1);
    }
    out
}

/// A line with IRIs in angle brackets taken out (a SPARQL query's vocabulary, and the sites it
/// names as nodes).
fn without_iris(l: &str) -> String {
    let mut s = l.to_string();
    while let Some(a) = s.find("<http") {
        let Some(b) = s[a..].find('>') else { break };
        s.replace_range(a..a + b + 1, "<>");
    }
    s
}

/// What a line names: each hit's marker.
fn hits(l: &str, marks: &BTreeMap<String, String>) -> Vec<String> {
    let l = without_iris(l);
    let lower = l.to_lowercase();
    let mut out: Vec<String> = marks.keys().filter(|m| names(&l, &lower, m)).cloned().collect();
    for u in urls(&l) {
        let h = u.split("://").nth(1).unwrap_or("").split(['/', '?', '#']).next().unwrap_or("");
        // (A host behind a template, `${x}.host`: the host after it.)
        let h = templated_rest(h).unwrap_or(h);
        if h.is_empty()
            || local_host(h)
            || LINKS.iter().any(|a| u.contains(a))
            || out.iter().any(|m| u.contains(m.as_str()))
        {
            continue;
        }
        out.push(u.to_string());
    }
    out
}

/// A line's code with its comment cut, its strings kept: `//` (or `#` in `hash` languages) outside
/// a string. Rust's raw strings (`r#"…"#`) are passed whole; in `quotes` the characters that open
/// a string.
fn cut_comment(l: &str, hash: bool, quotes: &[char]) -> String {
    let b: Vec<char> = l.chars().collect();
    let mut q: Option<char> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match q {
            Some(open) => {
                if c == '\\' {
                    i += 1;
                } else if c == open {
                    q = None;
                }
            }
            None => {
                if !hash && c == 'r' && b.get(i + 1) == Some(&'#') && b.get(i + 2) == Some(&'"') {
                    let rest: String = b[i + 3..].iter().collect();
                    let skip = rest.find("\"#").map_or(b.len(), |k| i + 3 + rest[..k].chars().count() + 2);
                    i = skip;
                    continue;
                }
                if quotes.contains(&c) {
                    q = Some(c);
                } else if (hash && c == '#') || (!hash && c == '/' && b.get(i + 1) == Some(&'/')) {
                    return b[..i].iter().collect();
                }
            }
        }
        i += 1;
    }
    l.to_string()
}

/// A Rust file's lines with its strings' contents blanked (raw strings and those spanning lines
/// too) and its comments cut, so its blocks can be counted.
fn blank_rs(text: &str) -> Vec<String> {
    let b: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    // (In a string: the hashes that close it, None for a plain one.)
    let mut s: Option<Option<usize>> = None;
    while i < b.len() {
        let c = b[i];
        match s {
            Some(raw) => {
                let closes = match raw {
                    None => c == '"',
                    Some(n) => c == '"' && (1..=n).all(|k| b.get(i + k) == Some(&'#')),
                };
                if raw.is_none() && c == '\\' {
                    out.push(' ');
                    out.push(if b.get(i + 1) == Some(&'\n') { '\n' } else { ' ' });
                    i += 2;
                    continue;
                }
                if closes {
                    out.push('"');
                    i += 1 + raw.unwrap_or(0);
                    s = None;
                    continue;
                }
                out.push(if c == '\n' { '\n' } else { ' ' });
            }
            None => {
                if c == '/' && b.get(i + 1) == Some(&'/') {
                    while i < b.len() && b[i] != '\n' {
                        i += 1;
                    }
                    continue;
                }
                let prev_ident = i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == '_');
                if !prev_ident && (c == 'r' || (c == 'b' && b.get(i + 1) == Some(&'r'))) {
                    let at = if c == 'b' { i + 2 } else { i + 1 };
                    let n = (at..b.len()).take_while(|&k| b[k] == '#').count();
                    if b.get(at + n) == Some(&'"') {
                        out.push('"');
                        i = at + n + 1;
                        s = Some(Some(n));
                        continue;
                    }
                }
                if c == '"' {
                    s = Some(None);
                } else if c == '\'' && (b.get(i + 2) == Some(&'\'') || b.get(i + 1) == Some(&'\\')) {
                    // (A character literal: '{', '\n'.)
                    let end = (i + 2..b.len()).find(|&k| b[k] == '\'').unwrap_or(i);
                    out.push_str("' '");
                    i = end + 1;
                    continue;
                }
                out.push(c);
            }
        }
        i += 1;
    }
    out.split('\n').map(str::to_string).collect()
}

/// A file's lines as code (comments blanked; tests' lines blanked in Rust), and each line's item.
fn code_lines(rel: &str, text: &str) -> (Vec<String>, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let ext = rel.rsplit('.').next().unwrap_or("");
    let mut code = Vec::with_capacity(lines.len());
    let mut items = vec!["-".to_string(); lines.len()];
    match ext {
        "rs" => {
            let blanked = blank_rs(text);
            let tests = test_lines(&blanked);
            for (i, l) in lines.iter().enumerate() {
                let t = l.trim_start();
                code.push(if tests[i] || t.starts_with("//") { String::new() } else { cut_comment(l, false, &['"']) });
                if !code[i].is_empty() {
                    items[i] = enclosing_rs(&blanked, i).map_or_else(|| "<top level>".into(), |e| e.0);
                }
            }
        }
        "py" => {
            // (A docstring, a triple-quoted string opening a line, is a comment; one opened after
            // code, a query, is code.)
            let mut in_doc = false;
            let mut in_str = false;
            for l in &lines {
                let t = l.trim_start();
                let n = l.matches("\"\"\"").count() + l.matches("'''").count();
                if in_doc {
                    code.push(String::new());
                    in_doc = n % 2 == 0;
                    continue;
                }
                if in_str {
                    code.push(l.to_string());
                    in_str = n % 2 == 0;
                    continue;
                }
                if t.starts_with("\"\"\"") || t.starts_with("'''") || t.starts_with("r\"\"\"") {
                    code.push(String::new());
                    in_doc = n % 2 == 1;
                    continue;
                }
                code.push(cut_comment(l, true, &['"', '\'']));
                in_str = n % 2 == 1;
            }
            // (A test's function, `test_…`, is the test's.)
            for i in 0..lines.len() {
                if !code[i].is_empty() {
                    items[i] = enclosing_py(&lines, i).map_or_else(|| "<top level>".into(), |e| e.0);
                    if items[i].starts_with("test_") {
                        code[i].clear();
                    }
                }
            }
        }
        "sh" | "zsh" | "toml" | "yaml" | "yml" => code.extend(lines.iter().map(|l| {
            if l.trim_start().starts_with('#') {
                String::new()
            } else {
                cut_comment(l, true, &['"', '\''])
            }
        })),
        "css" | "html" | "json" => code.extend(lines.iter().map(|l| l.to_string())),
        _ => {
            let mut in_block = false;
            for l in &lines {
                let t = l.trim_start();
                if in_block {
                    in_block = !l.contains("*/");
                    code.push(String::new());
                } else if t.starts_with("/*") {
                    in_block = !l.contains("*/");
                    code.push(String::new());
                } else {
                    code.push(cut_comment(l, false, &['"', '\'', '`']));
                }
            }
        }
    }
    (code, items)
}

/// Whether a file is a test's (not the app's code).
fn is_test(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name.starts_with("test_")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| EXTS.contains(&x)) {
            out.push(p);
        }
    }
}

/// The lines naming a source outside the exemptions, and the exemptions that matched none.
fn scan() -> (Vec<String>, Vec<String>) {
    let root = root();
    let marks = markers(&root);
    assert!(marks.len() >= 10, "the staged declarations give too few markers: {marks:?}");
    let mut paths = Vec::new();
    for s in SCOPE {
        walk(&root.join(s), &mut paths);
    }
    let mut bad = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for f in paths {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if is_test(&rel) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let (code, items) = code_lines(&rel, &text);
        for (i, c) in code.iter().enumerate() {
            let h = hits(c, &marks);
            if h.is_empty() {
                continue;
            }
            let line = text.lines().nth(i).unwrap_or("");
            if let Some(k) =
                EXEMPT.iter().position(|(ef, en, anchor, _)| *ef == rel && *en == items[i] && line.contains(anchor))
            {
                used.insert(k);
                continue;
            }
            let why: Vec<String> = h
                .iter()
                .map(|m| marks.get(m).map_or_else(|| format!("{m} (a URL)"), |d| format!("{m} ({d})")))
                .collect();
            bad.push(format!("{rel}:{} in {}: {}\n    names {}", i + 1, items[i], line.trim(), why.join(", ")));
        }
    }
    let unused = EXEMPT
        .iter()
        .enumerate()
        .filter(|(k, _)| !used.contains(k))
        .map(|(_, (f, n, a, _))| format!("{f}: {n}: {a}"))
        .collect();
    (bad, unused)
}

#[test]
fn no_source_is_named_in_the_apps_code() {
    let (bad, _) = scan();
    assert!(bad.is_empty(), "data sources named in the app's code (docs/inputs.md §7.2): read them from their declarations, or list the line in EXEMPT with why:\n{}", bad.join("\n"));
}

/// Every exemption still matches a line it exempts.
#[test]
fn every_exemption_matches_a_line() {
    let (_, unused) = scan();
    assert!(unused.is_empty(), "exemptions matching no line naming a source (remove them):\n{}", unused.join("\n"));
}

/// What §6.9 keeps in code, as the exemptions cite it: each row in its table, saying so.
#[test]
fn what_is_kept_in_code_is_in_the_table() {
    let doc = std::fs::read_to_string(root().join("docs/inputs.md")).unwrap();
    let s = doc.find("### 6.9").expect("docs/inputs.md §6.9");
    let sec = &doc[s..s + doc[s..].find("\n## ").unwrap_or(doc.len() - s)];
    for row in KEPT {
        assert!(sec.contains(&format!("| {row} | kept in code")), "§6.9's table has no row \"{row}\" kept in code");
    }
    for (f, _, _, why) in EXEMPT {
        if let Some(r) = why.strip_prefix("kept (§6.9, ") {
            let row = r.split(')').next().unwrap();
            assert!(KEPT.contains(&row), "{f}: cites §6.9's \"{row}\", not one KEPT names");
        }
    }
    assert!(sec.contains("Also kept: fonts (`scripts/fonts.sh`"), "§6.9 no longer keeps the fonts");
}

/// The scanner's own rules: markers from a declaration (a host, an S3 bucket, a name and its
/// lead); URLs found, local ones and allowed links passed, IRIs aside; comments cut, strings kept.
#[test]
fn the_scanner_finds_sources_by_their_declarations() {
    assert_eq!(
        host_marker("https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png").as_deref(),
        Some("s3.amazonaws.com/elevation-tiles-prod")
    );
    assert_eq!(host_marker("https://s3.us-west-2.amazonaws.com/b/x").as_deref(), Some("s3.us-west-2.amazonaws.com/b"));
    assert_eq!(
        host_marker("https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/x.tif").as_deref(),
        Some("esa-worldcover.s3.eu-central-1.amazonaws.com")
    );
    let marks: BTreeMap<String, String> =
        [("data.example-source.org".to_string(), "a.toml".to_string()), ("Some Source".into(), "a.toml".into())].into();
    assert_eq!(hits("let h = \"data.example-source.org\";", &marks), ["data.example-source.org"]);
    assert_eq!(hits("x = 'Some Source v2'", &marks), ["Some Source"]);
    assert_eq!(hits("u = \"https://other.org/x.zip\"", &marks), ["https://other.org/x.zip"]);
    for ok in [
        "\"http://127.0.0.1:8080/x\"",
        "`https://${host}/x`",
        "\"https://m4.tail1.ts.net/\"",
        "\"http://x\"",
        "`https://www.openstreetmap.org/${id}`",
        "`https://${l}.wikipedia.org/wiki/${t}`",
        "PREFIX wd: <http://www.wikidata.org/entity/>",
    ] {
        assert!(hits(ok, &marks).is_empty(), "{ok}");
    }
    let marks: BTreeMap<String, String> = [
        ("ci:overture maps".to_string(), "o.toml".to_string()),
        ("w:CanVec".into(), "c.toml".into()),
        ("s3.amazonaws.com/b".into(), "b.toml".into()),
    ]
    .into();
    assert_eq!(hits("source: 'OVERTURE Maps (OSM)'", &marks), ["ci:overture maps"]);
    assert_eq!(hits("\"CanVec's zips\"", &marks), ["w:CanVec"]);
    assert!(hits("\"canvec_lakes\", \"MyCanVecX\"", &marks).is_empty());
    assert_eq!(hits("`https://${r}.s3.amazonaws.com/x`", &marks), ["https://${r}.s3.amazonaws.com/x"]);
    assert_eq!(hits("\"http://100.1.2.3/x\"", &marks), ["http://100.1.2.3/x"]);
    assert!(hits("\"http://100.70.85.80:8080/x\"", &marks).is_empty());
    assert_eq!(
        cut_comment("let u = \"https://a.org\"; // https://b.org", false, &['"']),
        "let u = \"https://a.org\"; "
    );
    assert_eq!(
        cut_comment("const M: &str = r#\"{\"u\":\"https://a.org\"}\"#; // c", false, &['"']),
        "const M: &str = r#\"{\"u\":\"https://a.org\"}\"#; "
    );
    assert_eq!(cut_comment("x = \"#a\"  # https://b.org", true, &['"', '\'']), "x = \"#a\"  ");
    let (code, _) = code_lines(
        "a.py",
        "def f():\n    \"\"\"Reads https://b.org.\n    more\"\"\"\n    q = f\"\"\"\n    <https://c.org/>\n    \"\"\"\n",
    );
    assert_eq!(code[1], "");
    assert_eq!(code[2], "");
    assert!(code[4].contains("c.org"));
}
