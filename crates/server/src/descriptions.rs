//! Descriptions, live (docs/plan.md §7, docs/inputs.md §6.4): the descriptions' accepted version
//! (the `.jsonl` files the input check took in), lines `{"qid": "Q243", "long": "…", "src": […]}`
//! (or `{"id": "n123" | "w123" | "r123", …}` for things without a Wikidata item), `{"qid": …,
//! "drop": true}` removing an earlier one. Later files (by path) win. Copied to this Mac like the
//! translations (crate::accepted), and laid over the popup details when served, so a description
//! taken in shows within a minute of the check, with nothing rebuilt. (While the descriptions move
//! under `inputs/`, the old folder's copy is read until it's empty: crate::accepted::choose.)

use crate::data::Data;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// A description: its text and sources, or None where it's dropped.
type Table = HashMap<String, Option<Value>>;

pub struct Descriptions {
    /// The local copy of the old NAS folder (while the move's first publish reads it).
    dir: PathBuf,
    /// The local copy of the accepted version.
    accepted: PathBuf,
    from: std::sync::Mutex<crate::accepted::From>,
    table: RwLock<Arc<Table>>,
}

impl Descriptions {
    pub fn new(home: &Path) -> Arc<Descriptions> {
        let unit = pipeline::inputs::descriptions::UNIT;
        let (dir, accepted) = (home.join(unit), crate::accepted::local_dir(home, unit));
        let from = if crate::accepted::files(&accepted).is_some() && !dir.exists() { crate::accepted::From::Accepted } else { crate::accepted::From::Old };
        Arc::new(Descriptions { dir, accepted, from: std::sync::Mutex::new(from), table: RwLock::new(Arc::new(HashMap::new())) })
    }

    /// The files to read, in path order (later ones win): the accepted version's `.jsonl` files
    /// by their drop-box paths' components, or the old folder's copy's.
    fn files(&self) -> Vec<PathBuf> {
        if *self.from.lock().unwrap() == crate::accepted::From::Accepted {
            let mut fs: Vec<(String, PathBuf)> = crate::accepted::files(&self.accepted).unwrap_or_default().into_iter().filter(|(p, _)| p.ends_with(".jsonl")).map(|(p, (local, _))| (p, local)).collect();
            fs.sort_by(|a, b| Path::new(&a.0).cmp(Path::new(&b.0)));
            return fs.into_iter().map(|(_, l)| l).collect();
        }
        let mut files: Vec<PathBuf> = Vec::new();
        let mut stack = vec![self.dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "jsonl") {
                    files.push(p);
                }
            }
        }
        files.sort();
        files
    }

    /// Reads every file, in path order (later ones win).
    fn load(&self) {
        let files = self.files();
        let mut t: Table = HashMap::new();
        for f in &files {
            let Ok(text) = std::fs::read_to_string(f) else { continue };
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                let Some(key) = v.get("qid").or_else(|| v.get("id")).and_then(|k| k.as_str()).map(str::to_string) else { continue };
                if v.get("drop").and_then(|d| d.as_bool()) == Some(true) {
                    t.insert(key, None);
                } else if v.get("long").and_then(|l| l.as_str()).is_some_and(|l| !l.trim().is_empty()) {
                    t.insert(key, Some(v));
                }
            }
        }
        eprintln!("descriptions: {} from {} files", t.len(), files.len());
        *self.table.write().unwrap() = Arc::new(t);
    }

    /// Keeps the local copy in step with the NAS (every minute while the map is in use).
    pub fn spawn(self: &Arc<Self>, data: Arc<Data>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("popup-descriptions".into())
            .spawn(move || {
                me.load();
                // (Once at the start whatever the use: a Mac idle since would never copy.)
                let mut first = true;
                loop {
                    if (first || crate::updater::in_use(600)) && me.sync(&data) {
                        me.load();
                    }
                    first = false;
                    std::thread::sleep(Duration::from_secs(60));
                }
            })
            .ok();
    }

    /// Brings the local copies up to date: whether the files to read changed.
    fn sync(&self, data: &Data) -> bool {
        let unit = pipeline::inputs::descriptions::UNIT;
        let (from, changed) = crate::accepted::choose(data, unit, &self.accepted, unit, &self.dir);
        let mut cur = self.from.lock().unwrap();
        let moved = *cur != from;
        *cur = from;
        changed || moved
    }

    /// A popup record with its description laid over: the record's own `long` replaced, or removed
    /// where dropped. Unchanged when there's none. The record's Wikidata item (`qid` for heritage
    /// sites, the first of `wikidata` for sights) is looked up first, then its OSM id.
    pub fn apply<'a>(&self, record: &'a str) -> std::borrow::Cow<'a, str> {
        let t = self.table.read().unwrap().clone();
        if t.is_empty() {
            return record.into();
        }
        let Ok(mut v) = serde_json::from_str::<Value>(record) else { return record.into() };
        let qid = v.get("qid").and_then(Value::as_str).or_else(|| v.get("wikidata").and_then(Value::as_str).and_then(|w| w.split(';').next()).map(str::trim)).filter(|q| !q.is_empty());
        let osm = v.get("osm").and_then(Value::as_str);
        let Some(entry) = qid.and_then(|q| t.get(q)).or_else(|| osm.and_then(|o| t.get(o))).cloned() else { return record.into() };
        let Some(o) = v.as_object_mut() else { return record.into() };
        match entry {
            None => {
                o.remove("long");
                o.remove("long_src");
            }
            Some(d) => {
                o.insert("long".into(), d["long"].clone());
                match credit(o, &d) {
                    Some(c) => o.insert("long_src".into(), c),
                    None => o.remove("long_src"),
                };
            }
        }
        v.to_string().into()
    }
}

/// The popup's credit line for a description: `{"refs": sources}` for a researched one, else the
/// Wikipedia article it summarises, `{"lang", "title"}`: the record's own (built with the same
/// article, dem/heritagedetails.py), else the article the record links (`wiki` for heritage sites, `wikipedia` = "lang:Title" for sights).
fn credit(record: &serde_json::Map<String, Value>, d: &Value) -> Option<Value> {
    if let Some(src) = d.get("src").filter(|s| s.as_array().is_some_and(|a| !a.is_empty())) {
        return Some(serde_json::json!({ "refs": src }));
    }
    if let Some(own) = record.get("long_src").filter(|c| c.get("title").and_then(Value::as_str).is_some_and(|t| !t.is_empty())) {
        return Some(own.clone());
    }
    if let Some(w) = record.get("wiki").filter(|w| w.get("title").and_then(Value::as_str).is_some()) {
        return Some(serde_json::json!({ "lang": w.get("lang").cloned().unwrap_or_else(|| "en".into()), "title": w["title"] }));
    }
    let (lang, title) = record.get("wikipedia").and_then(Value::as_str)?.split_once(':')?;
    Some(serde_json::json!({ "lang": lang.trim(), "title": title.trim() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(entries: &[(&str, Option<Value>)]) -> Descriptions {
        let d = Descriptions { dir: PathBuf::new(), accepted: PathBuf::new(), from: std::sync::Mutex::new(crate::accepted::From::Accepted), table: RwLock::new(Arc::new(HashMap::new())) };
        *d.table.write().unwrap() = Arc::new(entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
        d
    }

    fn applied(d: &Descriptions, rec: Value) -> Value {
        serde_json::from_str(&d.apply(&rec.to_string())).unwrap()
    }

    #[test]
    fn credits_as_the_builds_do() {
        let d = with(&[
            ("Q1", Some(serde_json::json!({"long": "new text"}))),
            ("Q2", Some(serde_json::json!({"long": "researched", "src": [{"t": "Book", "u": "https://x"}]}))),
            ("Q3", None),
            ("n5", Some(serde_json::json!({"long": "by osm id"}))),
        ]);
        // Heritage: the built Wikipedia credit stays.
        let r = applied(&d, serde_json::json!({"qid": "Q1", "long": "old", "long_src": {"lang": "fr", "title": "Tour"}}));
        assert_eq!((r["long"].as_str(), r["long_src"].clone()), (Some("new text"), serde_json::json!({"lang": "fr", "title": "Tour"})));
        // Researched: its sources as refs, whatever was built.
        let r = applied(&d, serde_json::json!({"qid": "Q2", "long_src": {"lang": "en", "title": "T"}}));
        assert_eq!(r["long_src"], serde_json::json!({"refs": [{"t": "Book", "u": "https://x"}]}));
        // A new description: the article the record links.
        let r = applied(&d, serde_json::json!({"qid": "Q1", "wiki": {"lang": "de", "title": "Turm"}}));
        assert_eq!(r["long_src"], serde_json::json!({"lang": "de", "title": "Turm"}));
        // Sights: Wikidata in `wikidata` (first of several), the article in `wikipedia`.
        let r = applied(&d, serde_json::json!({"wikidata": "Q1; Q9", "osm": "n7", "wikipedia": "en:Ben Nevis"}));
        assert_eq!((r["long"].as_str(), r["long_src"].clone()), (Some("new text"), serde_json::json!({"lang": "en", "title": "Ben Nevis"})));
        // A researched credit isn't kept for a later plain description; no article, no credit.
        let r = applied(&d, serde_json::json!({"qid": "Q1", "long_src": {"refs": [{"t": "Old"}]}}));
        assert!(r.get("long_src").is_none());
        // Dropped; by OSM id; untouched.
        let r = applied(&d, serde_json::json!({"qid": "Q3", "long": "x", "long_src": {"lang": "en", "title": "T"}}));
        assert!(r.get("long").is_none() && r.get("long_src").is_none());
        assert_eq!(applied(&d, serde_json::json!({"osm": "n5"}))["long"], "by osm id");
        let rec = r#"{"qid":"Q8","long":"kept"}"#;
        assert_eq!(d.apply(rec), rec);
    }
}
