//! Descriptions, live (docs/plan.md §7): `descriptions/**/*.jsonl` on the NAS, lines
//! `{"qid": "Q243", "long": "…", "src": […]}` (or `{"id": "n123" | "w123" | "r123", …}` for things
//! without a Wikidata item), `{"qid": …, "drop": true}` removing an earlier one. Later file names
//! win. Copied to this Mac like the translations, and laid over the popup details when served, so
//! a description dropped in shows within a minute, with nothing rebuilt.

use crate::data::Data;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// A description: its text and sources, or None where it's dropped.
type Table = HashMap<String, Option<Value>>;

pub struct Descriptions {
    dir: PathBuf,
    table: RwLock<Arc<Table>>,
}

impl Descriptions {
    pub fn new(home: &Path) -> Arc<Descriptions> {
        Arc::new(Descriptions { dir: home.join("descriptions"), table: RwLock::new(Arc::new(HashMap::new())) })
    }

    /// Reads every file, in path order (later ones win).
    fn load(&self) {
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
            .name("descriptions".into())
            .spawn(move || {
                std::fs::create_dir_all(&me.dir).ok();
                me.load();
                loop {
                    if crate::updater::in_use(600) {
                        match crate::livefolder::sync(&data, "descriptions", &me.dir) {
                            Ok(true) => me.load(),
                            Ok(false) => {}
                            Err(e) => eprintln!("descriptions: {e:#}"),
                        }
                    }
                    std::thread::sleep(Duration::from_secs(60));
                }
            })
            .ok();
    }

    /// A popup record with its description laid over: the record's own `long` replaced (with the
    /// file's sources as `long_src`), or removed where dropped. Unchanged when there's none.
    pub fn apply<'a>(&self, record: &'a str) -> std::borrow::Cow<'a, str> {
        let t = self.table.read().unwrap().clone();
        if t.is_empty() {
            return record.into();
        }
        let Ok(mut v) = serde_json::from_str::<Value>(record) else { return record.into() };
        let key = v.get("qid").and_then(|q| q.as_str()).or_else(|| v.get("osm").and_then(|o| o.as_str())).map(str::to_string);
        let Some(entry) = key.and_then(|k| t.get(&k).cloned()) else { return record.into() };
        let Some(o) = v.as_object_mut() else { return record.into() };
        match entry {
            None => {
                o.remove("long");
                o.remove("long_src");
            }
            Some(d) => {
                o.insert("long".into(), d["long"].clone());
                match d.get("src") {
                    Some(src) => o.insert("long_src".into(), serde_json::json!({ "written": true, "src": src })),
                    None => o.insert("long_src".into(), serde_json::json!({ "written": true })),
                };
            }
        }
        v.to_string().into()
    }
}
