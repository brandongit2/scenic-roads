//! The timetable feeds (docs/inputs.md §6.7.1): source declarations, `inputs/timetables/feeds/`,
//! one `<id>.toml` per GTFS feed the app fetches (its URL, how to fetch it, its credit, what it
//! replaces, which counters take it) and one for the Mobility Database's catalogue (its URL and the
//! corrections to it: feeds left out, countries, copies, and the territories' own codes). The
//! `rail-feeds` job reads the accepted declarations (`of_accepted`), fetches the feeds and keeps each
//! version by content; the rail and ferry counters read what it fetched.
//!
//! The checks:
//! - **Errors:** a file that isn't a `<id>.toml` (an id of letters, digits, `-` and `_`); a
//!   description without its credit and licence (`credits::parse_description`); a `[data]` without
//!   a known `format` (`gtfs`, `mdb-catalogue`) or a `url`; an unknown fetch `method`, or a keyed
//!   method without its `header` and a valid `key` name (or `link`, for `get-key-json-link`); a
//!   `refresh` not `days:<n>`; `modes` other than `rail` and `ferry`; an `order` not a whole number;
//!   `replaces`, `exclude` or `country` naming a malformed id; a `country` or `own_code` value that
//!   isn't ISO 3166 syntax; two catalogue declarations.
//! - **Warnings:** `replaces` or `exclude` naming a feed neither declared nor in the catalogue's
//!   pinned copy (when there is one); an `own_code` key no outline of the pass has; two declarations
//!   with one URL.

use super::{Checks, FileCheck, Finding, Level, Version};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The unit's name.
pub const UNIT: &str = "timetables/feeds";

/// The formats a declaration may have.
pub const GTFS: &str = "gtfs";
pub const CATALOGUE: &str = "mdb-catalogue";

/// The fetch methods (docs/inputs.md §5.2) the fetching job knows.
pub const METHODS: [&str; 3] = ["get", "get-key-header", "get-key-json-link"];

/// The counters a feed may be read by.
pub const MODES: [&str; 2] = ["rail", "ferry"];

/// Days before a fetched copy out of date is fetched again, unless the declaration says.
pub const REFRESH_DAYS: i64 = 7;

const SHAPE: &str = "tf-shape";
const DATA: &str = "tf-data";
const TWO_CATALOGUES: &str = "tf-catalogues";
const UNKNOWN: &str = "tf-unknown";
const NO_OUTLINE: &str = "tf-own-code";
const SAME_URL: &str = "tf-same-url";

/// The feeds' checks: with the catalogue's feed ids and the pass's territory codes once prepared.
#[derive(Default)]
pub struct Feeds {
    pub catalogue_ids: Option<BTreeSet<String>>,
    pub territories: Option<BTreeSet<String>>,
}

/// A feed id: letters, digits, `-` and `_` (the catalogue's ids and ours).
pub fn valid_feed_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A key's name in `inputs/keys.env`: upper-case letters, digits and `_`, not starting with a digit.
pub fn valid_key_name(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// A day, `YYYY-MM-DD`.
pub fn valid_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && s.bytes().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// The id a declaration's path gives (`sncf.toml` → `sncf`), or None when it isn't one.
pub fn id_of(path: &str) -> Option<&str> {
    path.strip_suffix(".toml").filter(|id| valid_feed_id(id))
}

/// A declaration as the fetching job reads it (`facts.feed`): what of it a step reads, as JSON.
/// `keyed` is its digest.
fn parse(path: &str, bytes: &[u8]) -> (Option<Value>, Vec<Finding>) {
    let mut fs = Vec::new();
    let Some(id) = id_of(path) else {
        fs.push(Finding::new(SHAPE, Level::Error, &[path], &["not a declaration"], vec![path.into()], format!("{path} isn't the drop box's shape: timetables/feeds takes <id>.toml declarations (an id of letters, digits, - and _: docs/inputs.md §6.7.1); notes go in how/")));
        return (None, fs);
    };
    let (d, mut df, _) = super::credits::parse_description(path, bytes);
    fs.append(&mut df);
    let Some(d) = d else { return (None, fs) };
    // (It parsed as TOML: parse_description said so.)
    let t: toml::Table = toml::from_str(std::str::from_utf8(bytes).unwrap_or("")).unwrap_or_default();
    let mut bad = |what: &str, msg: String| {
        fs.push(Finding::new(
            DATA,
            Level::Error,
            &[path],
            &[what],
            vec![path.into()],
            format!("{path}: {msg} (docs/inputs.md §6.7.1)"),
        ))
    };
    let ids = |v: Option<&toml::Value>, what: &str, bad: &mut dyn FnMut(&str, String)| -> Vec<String> {
        match v {
            None => Vec::new(),
            Some(toml::Value::Array(a)) if a.iter().all(|x| x.as_str().is_some_and(valid_feed_id)) => {
                a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
            }
            Some(_) => {
                bad(what, format!("`{what}` isn't a list of feed ids"));
                Vec::new()
            }
        }
    };
    let Some(data) = t.get("data").and_then(|x| x.as_table()) else {
        bad("data", "no [data] table".into());
        return (None, fs);
    };
    let format = data.get("format").and_then(|x| x.as_str()).unwrap_or("");
    if format != GTFS && format != CATALOGUE {
        bad("format", format!("`format` is {format:?}, not one this app reads ({GTFS:?}, {CATALOGUE:?})"));
    }
    let url = data.get("url").and_then(|x| x.as_str()).unwrap_or("");
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bad("url", "`url` isn't an http(s) URL".into());
    }
    let mut fetch = Map::new();
    match data.get("fetch") {
        None => {
            fetch.insert("method".into(), "get".into());
        }
        Some(toml::Value::Table(f)) => {
            let method = f.get("method").and_then(|x| x.as_str()).unwrap_or("get");
            if !METHODS.contains(&method) {
                bad("method", format!("fetch `method` {method:?} isn't one of {METHODS:?}"));
            }
            fetch.insert("method".into(), method.into());
            if method != "get" {
                let key = f.get("key").and_then(|x| x.as_str()).unwrap_or("");
                if !valid_key_name(key) {
                    bad("key", format!("fetch `key` {key:?} isn't a key's name (upper-case letters, digits and _)"));
                }
                let header = f.get("header").and_then(|x| x.as_str()).unwrap_or("");
                if header.is_empty() || !header.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    bad("header", "fetch `header` isn't a header's name".into());
                }
                fetch.insert("key".into(), key.into());
                fetch.insert("header".into(), header.into());
            }
            if method == "get-key-json-link" {
                let link = f.get("link").and_then(|x| x.as_str()).unwrap_or("");
                if link.is_empty() {
                    bad("link", "fetch `link` (the download link's JSON path in the answer) is missing".into());
                }
                fetch.insert("link".into(), link.into());
            }
        }
        Some(_) => bad("fetch", "[data.fetch] isn't a table".into()),
    }
    let refresh = match data.get("refresh") {
        None => REFRESH_DAYS,
        Some(x) => match x
            .as_str()
            .and_then(|s| s.strip_prefix("days:"))
            .and_then(|n| n.parse::<i64>().ok())
            .filter(|n| *n >= 1)
        {
            Some(n) => n,
            None => {
                bad("refresh", "`refresh` isn't days:<n>".into());
                REFRESH_DAYS
            }
        },
    };
    let modes: Vec<String> = match t.get("modes") {
        None => Vec::new(),
        Some(toml::Value::Array(a)) if a.iter().all(|x| x.as_str().is_some_and(|m| MODES.contains(&m))) => {
            a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
        }
        Some(_) => {
            bad("modes", format!("`modes` isn't a list of {MODES:?}"));
            Vec::new()
        }
    };
    let order = match t.get("order") {
        None => 0,
        Some(toml::Value::Integer(n)) => *n,
        Some(_) => {
            bad("order", "`order` isn't a whole number".into());
            0
        }
    };
    let mut feed = json!({
        "id": id, "format": format, "url": url, "fetch": fetch, "refresh_days": refresh,
        "name": d.name, "licence": d.licence, "source": d.source, "territory": d.territory, "redistribute": d.redistribute,
    });
    if format == CATALOGUE {
        let table = |k: &str, bad: &mut dyn FnMut(&str, String)| -> Map<String, Value> {
            match t.get(k) {
                None => Map::new(),
                Some(toml::Value::Table(m)) => m.iter().map(|(a, b)| (a.clone(), toml_json(b))).collect(),
                Some(_) => {
                    bad(k, format!("`{k}` isn't a table"));
                    Map::new()
                }
            }
        };
        let exclude = table("exclude", &mut bad);
        if exclude.iter().any(|(k, v)| !valid_feed_id(k) || !v.is_string()) {
            bad("exclude", "`exclude` isn't a table of feed id → why".into());
        }
        let country = table("country", &mut bad);
        if country.iter().any(|(k, v)| !valid_feed_id(k) || !v.as_str().is_some_and(super::credits::iso_3166)) {
            bad("country", "`country` isn't a table of feed id → ISO 3166 code".into());
        }
        let replaces = table("replaces", &mut bad);
        if replaces.iter().any(|(k, v)| {
            !valid_feed_id(k) || !v.as_array().is_some_and(|a| a.iter().all(|x| x.as_str().is_some_and(valid_feed_id)))
        }) {
            bad("replaces", "`replaces` isn't a table of feed id → feed ids".into());
        }
        let own = table("own_code", &mut bad);
        if own.iter().any(|(k, v)| {
            !(k.contains('-') && super::credits::iso_3166(k))
                || !v
                    .as_str()
                    .is_some_and(|c| c.is_empty() || (c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())))
        }) {
            bad("own_code", "`own_code` isn't a table of ISO 3166-2 code → alpha-2 code or \"\"".into());
        }
        feed["exclude"] = exclude.into();
        feed["country"] = country.into();
        feed["replaces"] = replaces.into();
        feed["own_code"] = own.into();
    } else {
        feed["replaces"] = ids(t.get("replaces"), "replaces", &mut bad).into();
        let routes = match t.get("ferry_routes") {
            None => Vec::new(),
            Some(toml::Value::Array(a)) if a.iter().all(|x| x.is_str()) => {
                a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
            }
            Some(_) => {
                bad("ferry_routes", "`ferry_routes` isn't a list of route ids".into());
                Vec::new()
            }
        };
        feed["ferry_routes"] = routes.into();
        feed["modes"] = modes.into();
        feed["order"] = order.into();
        match t.get("pin") {
            None => {}
            Some(toml::Value::Table(p)) => {
                let zip = p.get("zip").and_then(|x| x.as_str()).unwrap_or("");
                let day = p.get("day").and_then(|x| x.as_str()).unwrap_or("");
                if !zip.ends_with(".zip") || zip.starts_with('/') || zip.split('/').any(|c| c == ".." || c.is_empty()) {
                    bad("pin", "`pin`'s zip isn't a .zip's path from the project folder".into());
                }
                if !valid_day(day) {
                    bad("pin", "`pin`'s day isn't YYYY-MM-DD".into());
                }
                feed["pin"] = json!({"zip": zip, "day": day});
            }
            Some(_) => bad("pin", "`pin` isn't a table { zip, day }".into()),
        }
    }
    if !fs.is_empty() {
        return (None, fs);
    }
    (Some(feed), fs)
}

fn toml_json(v: &toml::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// The ids a declaration names as feeds left out or replaced.
fn named_ids(feed: &Value) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    match &feed["replaces"] {
        Value::Array(a) => out.extend(a.iter().filter_map(Value::as_str).map(|s| ("replaces", s.to_string()))),
        Value::Object(m) => {
            for (k, v) in m {
                out.push(("replaces", k.clone()));
                out.extend(
                    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(|s| ("replaces", s.to_string())),
                );
            }
        }
        _ => {}
    }
    if let Some(m) = feed["exclude"].as_object() {
        out.extend(m.keys().map(|k| ("exclude", k.clone())));
    }
    out
}

impl Checks for Feeds {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        let (feed, findings) = parse(path, bytes);
        c.findings = findings;
        if let Some(feed) = feed {
            let (d, _, _) = super::credits::parse_description(path, bytes);
            // (No extent: a catalog credits it while the trains or ferries count it: `credited`.)
            c.facts.insert("description".into(), serde_json::to_value(d).unwrap_or_default());
            c.keyed = Some(store::naming::hash16(feed.to_string().as_bytes()));
            // (One that replaces another is held with it when that one is held.)
            c.refers = named_ids(&feed)
                .into_iter()
                .filter(|(what, _)| *what == "replaces")
                .map(|(_, id)| format!("{id}.toml"))
                .filter(|p| p != path)
                .collect();
            c.refers.sort();
            c.refers.dedup();
            c.facts.insert("feed".into(), feed);
        }
        c
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        let mut feeds: BTreeMap<String, Value> = BTreeMap::new();
        for p in &v.paths {
            if let (Some(f), _) = parse(p, &(v.read)(p)?) {
                feeds.insert(p.clone(), f);
            }
        }
        let catalogues: Vec<&String> = feeds.iter().filter(|(_, f)| f["format"] == CATALOGUE).map(|(p, _)| p).collect();
        if catalogues.len() > 1 {
            let files: Vec<String> = catalogues.iter().map(|s| s.to_string()).collect();
            let refs: Vec<&str> = files.iter().map(String::as_str).collect();
            out.push(Finding::new(
                TWO_CATALOGUES,
                Level::Error,
                &refs,
                &[],
                files.clone(),
                format!("{} declare the catalogue: there's one", files.join(", ")),
            ));
        }
        let declared: BTreeSet<&str> = feeds.values().filter_map(|f| f["id"].as_str()).collect();
        for (p, f) in &feeds {
            if let Some(cat) = &self.catalogue_ids {
                let unknown: Vec<String> = named_ids(f)
                    .into_iter()
                    .filter(|(_, id)| !declared.contains(id.as_str()) && !cat.contains(id))
                    .map(|(what, id)| format!("{what} {id}"))
                    .collect();
                if !unknown.is_empty() {
                    let refs: Vec<&str> = unknown.iter().map(String::as_str).collect();
                    out.push(Finding::new(
                        UNKNOWN,
                        Level::Warning,
                        &[p],
                        &refs,
                        vec![p.clone()],
                        format!("{p} names feeds neither declared nor in the catalogue: {}", unknown.join(", ")),
                    ));
                }
            }
            if let (Some(terr), Some(own)) = (&self.territories, f["own_code"].as_object()) {
                let none: Vec<&str> = own.keys().filter(|k| !terr.contains(*k)).map(String::as_str).collect();
                if !none.is_empty() {
                    out.push(Finding::new(
                        NO_OUTLINE,
                        Level::Warning,
                        &[p],
                        &none,
                        vec![p.clone()],
                        format!("{p}: own_code names territories no outline of the pass has: {}", none.join(", ")),
                    ));
                }
            }
        }
        let mut by_url: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (p, f) in &feeds {
            by_url.entry(f["url"].as_str().unwrap_or("")).or_default().push(p.clone());
        }
        for (url, ps) in by_url.into_iter().filter(|(_, ps)| ps.len() > 1) {
            let refs: Vec<&str> = ps.iter().map(String::as_str).collect();
            out.push(Finding::new(
                SAME_URL,
                Level::Warning,
                &refs,
                &[url],
                ps.clone(),
                format!("{} declare one URL, {url}", ps.join(", ")),
            ));
        }
        Ok(out)
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        let mut v: Vec<String> =
            super::languages::outlines_of(manifest).map(|o| format!("outlines {o}")).into_iter().collect();
        v.extend(manifest.get(crate::rail::CATALOGUE).map(|c| format!("catalogue {c}")));
        v
    }

    fn prepared(&self, root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Box<dyn Checks>>> {
        // (What can't be read now doesn't fail the check: it goes on without those warnings.)
        let territories = super::languages::outlines_of(manifest).and_then(|o| match crate::outlines::Outlines::open(&root.join(o)) {
            Ok(outlines) => Some(outlines.recs.iter().filter(|r| r.flags & crate::outlines::flag::ISO1 != 0).map(|r| outlines.string(r.iso).to_string()).collect()),
            Err(e) => {
                eprintln!("inputs: {UNIT}: the pass's outlines, {o}, can't be read ({e:#}): checked without the codes they have");
                None
            }
        });
        let catalogue_ids = manifest.get(crate::rail::CATALOGUE).and_then(|c| match catalogue_ids(&root.join(c)) {
            Ok(ids) => Some(ids),
            Err(e) => {
                eprintln!("inputs: {UNIT}: the catalogue, {c}, can't be read ({e:#}): checked without its ids");
                None
            }
        });
        Ok(Some(Box::new(Feeds { catalogue_ids, territories })))
    }
}

/// The feed ids of a catalogue (`feeds_v2.csv`: its `id` column).
pub fn catalogue_ids(path: &Path) -> Result<BTreeSet<String>> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("");
    let col =
        head.split(',').position(|c| c.trim_matches('"') == "id").ok_or_else(|| anyhow::anyhow!("no id column"))?;
    // (The id is in the first columns, before any quoted field with a comma.)
    Ok(lines
        .filter_map(|l| l.split(',').nth(col).map(|s| s.trim_matches('"').to_string()))
        .filter(|s| valid_feed_id(s))
        .collect())
}

/// The accepted declarations, as the fetching job reads them (`facts.feed`), in path order.
pub fn of_accepted(a: &super::Accepted) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (p, _) in a.files() {
        let (f, findings) = parse(p, &a.read(p)?);
        out.push(f.ok_or_else(|| {
            anyhow::anyhow!("{p}: accepted, but doesn't parse: {:?}", findings.first().map(|f| &f.message))
        })?);
    }
    Ok(out)
}

/// What the agent reads of an accepted index: the key names its declarations fetch with, sorted,
/// and the digest of their `keyed` digests (what rail-feeds reads of them: `facts.feed`, so an
/// edited description or note changes neither).
#[derive(Clone, Debug, PartialEq)]
pub struct Declared {
    pub key_names: Vec<String>,
    pub keyed: String,
}

/// Indexes as read, by the index's content name.
static DECLARED: std::sync::Mutex<Option<BTreeMap<String, Declared>>> = std::sync::Mutex::new(None);

/// The declarations of accepted index `index` (its content name), as `Declared`: read once per
/// index.
pub fn declared(root: &Path, index: &str) -> Result<Declared> {
    if let Some(d) = DECLARED.lock().unwrap().get_or_insert_with(BTreeMap::new).get(index) {
        return Ok(d.clone());
    }
    let d = declared_of(&super::read_index(root, index)?);
    DECLARED.lock().unwrap().get_or_insert_with(BTreeMap::new).insert(index.to_string(), d.clone());
    Ok(d)
}

/// `declared` of an index read.
pub fn declared_of(i: &super::Index) -> Declared {
    let feeds: Vec<Value> = i.files.values().filter_map(|f| f.facts.get("feed").cloned()).collect();
    let lines: Vec<String> = i.files.iter().map(|(p, f)| format!("{p} {}", f.keyed)).collect();
    Declared { key_names: key_names(&feeds), keyed: store::naming::hash16(lines.join("\n").as_bytes()) }
}

/// The credited feeds as read, by the content names they were read from.
static CREDITED: std::sync::Mutex<Option<(String, Vec<String>)>> = std::sync::Mutex::new(None);

/// The declared feeds the map's trains and ferries count (in the rail or ferry feeds' list with a
/// zip), sorted: the declarations a catalog credits (docs/inputs.md §6.7.1). None before the
/// declarations are accepted; read again only when the lists or the index change.
pub fn credited(root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Vec<String>>> {
    let Some(index) = manifest.get(&super::logical(UNIT)) else { return Ok(None) };
    let lists: Vec<&String> =
        [crate::rail::FEEDS, crate::rail::FERRY_FEEDS].iter().filter_map(|l| manifest.get(*l)).collect();
    let read_as = format!("{index} {}", lists.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "));
    if let Some((k, v)) = CREDITED.lock().unwrap().as_ref() {
        if *k == read_as {
            return Ok(Some(v.clone()));
        }
    }
    let i = super::read_index(root, index)?;
    let declared: BTreeSet<&str> = i
        .files
        .values()
        .filter_map(|f| f.facts.get("feed"))
        .filter(|f| f["format"] == GTFS)
        .filter_map(|f| f["id"].as_str())
        .collect();
    let mut ids = BTreeSet::new();
    for l in lists {
        let v: Value = serde_json::from_slice(&std::fs::read(root.join(l))?)?;
        for f in v["feeds"].as_array().into_iter().flatten() {
            if let Some(id) = f["id"].as_str().filter(|id| declared.contains(id)) {
                if f.get("zip").is_some_and(|z| !z.is_null()) {
                    ids.insert(id.to_string());
                }
            }
        }
    }
    let ids: Vec<String> = ids.into_iter().collect();
    *CREDITED.lock().unwrap() = Some((read_as, ids.clone()));
    Ok(Some(ids))
}

/// The key names the accepted declarations fetch with (their `[data.fetch] key`), sorted.
pub fn key_names(feeds: &[Value]) -> Vec<String> {
    let mut v: Vec<String> = feeds.iter().filter_map(|f| f["fetch"]["key"].as_str().map(str::to_string)).collect();
    v.sort();
    v.dedup();
    v
}

/// The territories with an ISO 3166-1 code of their own that isn't their ISO 3166-2 code's end
/// (the catalogue declaration's `own_code`): code → its own code, None for one with none.
pub fn own_code(feeds: &[Value]) -> BTreeMap<String, Option<String>> {
    feeds
        .iter()
        .filter(|f| f["format"] == CATALOGUE)
        .flat_map(|f| f["own_code"].as_object().cloned().unwrap_or_default())
        .map(|(k, v)| (k, v.as_str().filter(|c| !c.is_empty()).map(str::to_string)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNCF: &str = r#"name = "SNCF Voyageurs (TGV INOUI, OUIGO, Intercités, TER)"
what = "Rail service frequency"
credit = "SNCF Voyageurs"
licence = "Licence Ouverte 2.0"
source = "https://ressources.data.sncf.com/"
territory = ["FR"]
modes = ["rail"]
order = 1
notes = "The national operator's own feed."

[data]
format = "gtfs"
url = "https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip"
"#;

    const LTA: &str = r#"name = "Land Transport Authority, LTA DataMall (MRT and LRT)"
credit = "Land Transport Authority (LTA DataMall)"
licence = "Singapore Open Data Licence 1.0"
territory = ["SG"]
replaces = ["mdb-1076", "mdb-3051"]

[data]
format = "gtfs"
url = "https://datamall2.mytransport.sg/ltaodataservice/GTFSScheduleTrain"
refresh = "days:3"

[data.fetch]
method = "get-key-json-link"
header = "AccountKey"
key = "LTA_ACCOUNT_KEY"
link = "value[0].link"
"#;

    /// The catalogue's declaration, its own codes as they were in rail.rs's OWN_CODE (#140).
    const CATALOGUE_TOML: &str = r#"name = "Mobility Database catalogue"
credit = "MobilityData, the Mobility Database"
licence = "Mobility Database terms"
territory = "world"
exclude = { "mdb-2933" = "the MTR's lines, researched by hand" }
country = { "mdb-3051" = "SG", "mdb-3409" = "SG" }

[data]
format = "mdb-catalogue"
url = "https://files.mobilitydatabase.org/feeds_v2.csv"

[replaces]
"mdb-1076" = ["mdb-3051", "mdb-3409"]
"mdb-3051" = ["mdb-3409"]

[own_code]
"FI-01" = "AX"
"FR-971" = "GP"
"FR-972" = "MQ"
"FR-973" = "GF"
"FR-974" = "RE"
"FR-976" = "YT"
"NL-BQ1" = "BQ"
"NL-BQ2" = "BQ"
"NL-BQ3" = "BQ"
"NO-21" = "SJ"
"NO-22" = "SJ"
"RS-KM" = "XK"
"ES-CE" = ""
"ES-ML" = ""
"#;

    fn checks(f: &Feeds, path: &str, text: &str) -> FileCheck {
        f.file(path, text.as_bytes())
    }

    fn errors(c: &FileCheck) -> Vec<String> {
        c.findings.iter().filter(|f| f.level == Level::Error).map(|f| f.message.clone()).collect()
    }

    #[test]
    fn a_feed_declared() {
        let c = checks(&Feeds::default(), "sncf.toml", SNCF);
        assert!(c.findings.is_empty(), "{:?}", c.findings);
        assert_eq!(
            c.facts["feed"],
            json!({"id": "sncf", "format": "gtfs", "url": "https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip", "fetch": {"method": "get"}, "refresh_days": 7,
                   "name": "SNCF Voyageurs (TGV INOUI, OUIGO, Intercités, TER)", "licence": "Licence Ouverte 2.0", "source": "https://ressources.data.sncf.com/", "territory": ["FR"], "redistribute": true,
                   "replaces": [], "ferry_routes": [], "modes": ["rail"], "order": 1})
        );
        assert_eq!(c.facts["description"]["credit"], "SNCF Voyageurs");
        // Keyed: what the steps read, not the notes nor the credit.
        let keyed = |t: &str| checks(&Feeds::default(), "sncf.toml", t).keyed.unwrap();
        assert_eq!(keyed(&SNCF.replace("national operator's", "operator's")), keyed(SNCF));
        assert_eq!(keyed(&SNCF.replace("credit = \"SNCF Voyageurs\"", "credit = \"SNCF\"")), keyed(SNCF));
        assert_ne!(keyed(&SNCF.replace("order = 1", "order = 2")), keyed(SNCF));
        assert_ne!(keyed(&SNCF.replace("NewTripId", "TripId")), keyed(SNCF));
    }

    #[test]
    fn a_feed_behind_a_key() {
        let c = checks(&Feeds::default(), "sg-lta-train.toml", LTA);
        assert!(c.findings.is_empty(), "{:?}", c.findings);
        let f = &c.facts["feed"];
        assert_eq!(
            f["fetch"],
            json!({"method": "get-key-json-link", "header": "AccountKey", "key": "LTA_ACCOUNT_KEY", "link": "value[0].link"})
        );
        assert_eq!((f["refresh_days"].as_i64(), f["modes"].as_array().map(Vec::len)), (Some(3), Some(0)));
        // Held with a declaration it replaces, were that one held.
        assert_eq!(c.refers, vec!["mdb-1076.toml", "mdb-3051.toml"]);
        assert_eq!(key_names(std::slice::from_ref(f)), vec!["LTA_ACCOUNT_KEY"]);
    }

    #[test]
    fn declared_names_what_rail_feeds_reads() {
        let index = |text: &str| {
            let c = checks(&Feeds::default(), "sg-lta-train.toml", text);
            let e = super::super::FileEntry {
                file: format!(
                    "sources/inputs/timetables/feeds/sg-lta-train.{}.toml",
                    store::naming::hash16(text.as_bytes())
                ),
                size: text.len() as u64,
                keyed: c.keyed.unwrap(),
                facts: c.facts,
            };
            super::super::Index {
                fmt: 1,
                unit: UNIT.into(),
                checks: format!("{UNIT} 1"),
                files: [("sg-lta-train.toml".to_string(), e)].into(),
                accepted: BTreeMap::new(),
            }
        };
        let d = declared_of(&index(LTA));
        assert_eq!(d.key_names, vec!["LTA_ACCOUNT_KEY"]);
        // A note or a credit edited: the same; a field rail-feeds reads: another.
        let noted = LTA.replacen("credit = ", "notes = \"checked\"\ncredit = ", 1);
        assert_ne!(noted, LTA);
        assert_eq!(declared_of(&index(&noted)).keyed, d.keyed);
        assert_ne!(declared_of(&index(&LTA.replace("value[0].link", "value[1].link"))).keyed, d.keyed);
    }

    #[test]
    fn faults_are_errors() {
        let bad = |path: &str, text: String, what: &str| {
            let c = checks(&Feeds::default(), path, &text);
            assert!(!c.facts.contains_key("feed"), "{what}");
            assert!(errors(&c).iter().any(|m| m.contains(what)), "{what}: {:?}", errors(&c));
        };
        bad("notes.md", SNCF.into(), "isn't the drop box's shape");
        bad("sncf feed.toml", SNCF.into(), "isn't the drop box's shape");
        bad("sncf.toml", SNCF.replace("credit = \"SNCF Voyageurs\"\n", ""), "`credit` is required");
        bad("sncf.toml", SNCF.replace("format = \"gtfs\"", "format = \"netex\""), "`format`");
        bad("sncf.toml", SNCF.replace("url = \"https", "url = \"ftp"), "`url`");
        bad("sncf.toml", SNCF.replace("[data]\n", ""), "no [data]");
        bad("sncf.toml", SNCF.replace("modes = [\"rail\"]", "modes = [\"bus\"]"), "`modes`");
        bad("sncf.toml", SNCF.replace("order = 1", "order = \"first\""), "`order`");
        bad("sncf.toml", SNCF.replace("format = \"gtfs\"", "format = \"gtfs\"\nrefresh = \"weekly\""), "`refresh`");
        bad("sg-lta-train.toml", LTA.replace("get-key-json-link", "post"), "method");
        bad("sg-lta-train.toml", LTA.replace("key = \"LTA_ACCOUNT_KEY\"", "key = \"lta key\""), "fetch `key`");
        bad("sg-lta-train.toml", LTA.replace("header = \"AccountKey\"\n", ""), "fetch `header`");
        bad("sg-lta-train.toml", LTA.replace("link = \"value[0].link\"\n", ""), "fetch `link`");
        bad("sg-lta-train.toml", LTA.replace("\"mdb-1076\"", "\"mdb 1076\""), "`replaces`");
        bad(
            "mobility-database.toml",
            CATALOGUE_TOML.replace("\"SG\", \"mdb-3409\"", "\"Singapore\", \"mdb-3409\""),
            "`country`",
        );
        bad(
            "mobility-database.toml",
            CATALOGUE_TOML.replace("\"RS-KM\" = \"XK\"", "\"RS-KM\" = \"Kosovo\""),
            "`own_code`",
        );
        bad("mobility-database.toml", CATALOGUE_TOML.replace("\"FI-01\" = \"AX\"", "\"FI\" = \"AX\""), "`own_code`");
        bad(
            "sncf.toml",
            SNCF.replace("[data]", "pin = { zip = \"../x.zip\", day = \"2026-09-28\" }\n[data]"),
            "`pin`'s zip",
        );
        bad(
            "sncf.toml",
            SNCF.replace("[data]", "pin = { zip = \"old/x.zip\", day = \"28 Sep\" }\n[data]"),
            "`pin`'s day",
        );
        let pinned = checks(
            &Feeds::default(),
            "sncf.toml",
            &SNCF.replace("[data]", "pin = { zip = \"old/sncf.zip\", day = \"2026-09-28\" }\n[data]"),
        );
        assert_eq!(pinned.facts["feed"]["pin"], json!({"zip": "old/sncf.zip", "day": "2026-09-28"}));
    }

    #[test]
    fn the_catalogue_and_its_corrections() {
        let c = checks(&Feeds::default(), "mobility-database.toml", CATALOGUE_TOML);
        assert!(c.findings.is_empty(), "{:?}", c.findings);
        let f = c.facts["feed"].clone();
        assert_eq!(f["exclude"]["mdb-2933"], "the MTR's lines, researched by hand");
        assert_eq!(f["replaces"]["mdb-1076"], json!(["mdb-3051", "mdb-3409"]));
        let own = own_code(&[f]);
        assert_eq!(own.len(), 14);
        assert_eq!(own["FI-01"].as_deref(), Some("AX"));
        assert_eq!(own["RS-KM"].as_deref(), Some("XK"));
        assert_eq!((own["ES-CE"].as_deref(), own["ES-ML"].as_deref()), (None, None), "none: not Cameroon and Mali");
    }

    fn whole(f: &Feeds, files: &[(&str, String)]) -> Vec<Finding> {
        let all: BTreeMap<String, Vec<u8>> =
            files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
        let read = |p: &str| Ok(std::sync::Arc::new(all[p].clone()));
        f.whole(&Version { paths: all.keys().cloned().collect(), read: &read }).unwrap()
    }

    #[test]
    fn a_version_whole() {
        let none = Feeds::default();
        let ok = [
            ("sncf.toml", SNCF.to_string()),
            ("sg-lta-train.toml", LTA.to_string()),
            ("mobility-database.toml", CATALOGUE_TOML.to_string()),
        ];
        assert!(whole(&none, &ok).is_empty());
        // Two catalogues: an error. One URL twice: a warning.
        let two = whole(
            &none,
            &[("a.toml", CATALOGUE_TOML.into()), ("b.toml", CATALOGUE_TOML.replace("feeds_v2", "feeds_v3"))],
        );
        assert!(two.iter().any(|f| f.level == Level::Error && f.check() == TWO_CATALOGUES));
        let same = whole(&none, &[("sncf.toml", SNCF.into()), ("sncf-copy.toml", SNCF.into())]);
        assert_eq!(same.iter().map(|f| (f.check(), f.level)).collect::<Vec<_>>(), vec![(SAME_URL, Level::Warning)]);
        // With the catalogue's ids and the pass's territories: feeds named that are neither declared
        // nor in it, and own codes no outline has, are warnings.
        let known = Feeds {
            catalogue_ids: Some(["mdb-1076", "mdb-3051", "mdb-3409", "mdb-2933"].map(String::from).into()),
            territories: Some(["FI-01", "FR-971"].map(String::from).into()),
        };
        let w = whole(&known, &ok);
        assert!(!w.iter().any(|f| f.check() == UNKNOWN), "{w:?}");
        let own: Vec<&Finding> = w.iter().filter(|f| f.check() == NO_OUTLINE).collect();
        assert_eq!(own.len(), 1);
        assert!(own[0].message.contains("RS-KM") && !own[0].message.contains("FI-01"));
        let w = whole(&known, &[("sg-lta-train.toml", LTA.replace("mdb-1076", "mdb-9"))]);
        assert!(w
            .iter()
            .any(|f| f.check() == UNKNOWN && f.level == Level::Warning && f.message.contains("replaces mdb-9")));
    }

    #[test]
    fn a_catalogues_ids() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("feeds_v2.csv");
        std::fs::write(&p, "id,data_type,status\nmdb-1,gtfs,active\ntld-7,gtfs,\n\"bad id\",gtfs,\n").unwrap();
        assert_eq!(catalogue_ids(&p).unwrap(), ["mdb-1", "tld-7"].map(String::from).into());
    }
}
