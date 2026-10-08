//! Display names, live (docs/plan.md §7): the user's translation files are copied from the NAS
//! folder to this Mac (so they work offline and load fast), compiled by the `names` crate, and
//! attached to everything the server serves. The folder is polled every minute, but only while the
//! map is in use (a request in the last ten minutes), so an idle server leaves the NAS alone.
//!
//! The languages spoken where a thing is come from the catalog's outlines (the pass's), made into
//! a raster once per outlines file and kept in the home (`names/spoken-<content>.bin`), so they
//! hold offline too.

use crate::data::Data;
use names::{Kind, Lang, Namer, Names, Spoken};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Which tiles: our label tiles (name `n`, English `en`), the basemap (OpenMapTiles schema), or
/// the area overlays (layer `a`: name `name`, World Heritage outlines `n`; English `en`), or the
/// rail stops (layer `s`: name `n`, English `en`).
#[derive(Clone, Copy)]
pub enum Rules {
    Labels,
    Basemap,
    Areas,
    Stations,
}

const AREAS: names::mvt::LayerRule<'static> = names::mvt::LayerRule { layer: "a", name_keys: &["name", "n"], en_keys: &["en"], kind: names::mvt::KindRule::Fixed(Kind::Other) };
const STATIONS: names::mvt::LayerRule<'static> = names::mvt::LayerRule { layer: "s", name_keys: &["n"], en_keys: &["en"], kind: names::mvt::KindRule::Fixed(Kind::Other) };

fn rules(r: Rules) -> &'static [names::mvt::LayerRule<'static>] {
    match r {
        Rules::Labels => &[names::mvt::LABELS],
        Rules::Basemap => &[names::mvt::OPENMAPTILES],
        Rules::Areas => &[AREAS],
        Rules::Stations => &[STATIONS],
    }
}

pub struct NamesState {
    names: RwLock<Option<Names>>,
    /// The languages spoken where, and the outlines' content name they were made from.
    spoken: RwLock<Option<(String, Arc<Spoken>)>>,
    /// The local copy of the NAS folder.
    dir: PathBuf,
    /// Where the rasters are kept.
    spoken_dir: PathBuf,
    /// Seconds since the epoch of the last request that used names.
    last_use: AtomicU64,
    /// The folder holds the area tables' lines and none by language (said in the status).
    only_old: std::sync::atomic::AtomicBool,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl NamesState {
    pub fn new(home: &Path) -> Arc<NamesState> {
        Arc::new(NamesState {
            names: RwLock::new(None),
            spoken: RwLock::new(None),
            dir: home.join("translations"),
            spoken_dir: home.join("names"),
            last_use: AtomicU64::new(now()),
            only_old: Default::default(),
        })
    }

    /// Record a use (keeps the folder polled).
    pub fn touch(&self) {
        self.last_use.store(now(), Ordering::Relaxed);
    }

    /// The lines and the spoken languages as they are now (a copy: cheap, the tables are shared,
    /// and nothing waits on it); None before the lines have loaded.
    pub fn snapshot(&self) -> Option<Namer> {
        let names = self.names.read().unwrap().clone()?;
        Some(Namer { names, spoken: self.spoken.read().unwrap().as_ref().map(|(_, s)| s.clone()) })
    }

    /// A name's display form (`osm`: the languages OSM gives it, where known). Before the lines
    /// have loaded: the name and its own English.
    pub fn display(&self, kind: Kind, name: &str, own_en: Option<&str>, osm: &[Lang], lon: f64, lat: f64) -> names::Display {
        self.touch();
        match self.snapshot() {
            Some(n) => n.display_at(kind, name, own_en, osm, lon, lat).to_display(),
            None => names::DisplayRef::new(name, own_en).to_display(),
        }
    }

    /// A version for a tile's names: the versions of the languages spoken within it (so a drop
    /// in Japanese doesn't change the ETags of tiles in France) and the raster's. `buffer`: how
    /// far, in tiles, its features reach beyond it (the basemap's labels: a whole tile; ours: none).
    pub fn version_for_tile(&self, z: u8, x: u32, y: u32, buffer: f64) -> u64 {
        let [w, s, e, n] = names::mvt::tile_bounds(z as u32, x, y, buffer);
        self.snapshot().map_or(0, |nm| nm.version_in(w, s, e, n))
    }

    /// Each language's version, the raster's, and whether only the area tables' lines are there
    /// (for the catalog's status).
    pub fn versions(&self) -> serde_json::Value {
        let g = self.names.read().unwrap();
        let mut langs = serde_json::Map::new();
        if let Some(n) = g.as_ref() {
            for (l, v) in n.summary().langs {
                langs.insert(l, serde_json::Value::from(v));
            }
        }
        let spoken = self.spoken.read().unwrap().as_ref().map(|(c, s)| serde_json::json!({"outlines": c, "version": s.version()}));
        let mut out = serde_json::json!({"langs": langs, "spoken": spoken});
        if self.only_old.load(Ordering::Relaxed) {
            out["warning"] = serde_json::Value::from(ONLY_OLD);
        }
        out
    }

    /// A version over every language (for files that span them all).
    pub fn version_all(&self) -> u64 {
        version_all(self.snapshot().as_ref())
    }

    /// A gzip'd vector tile with main/sub attached (the original when nothing changed or it can't be
    /// read).
    pub fn attach_gz(&self, gz: &[u8], z: u8, x: u32, y: u32, r: Rules) -> Vec<u8> {
        let Ok(raw) = names::mvt::gunzip_if_gzip(gz) else { return gz.to_vec() };
        let Some(nm) = self.snapshot() else { return gz.to_vec() };
        match names::mvt::attach(&raw, z as u32, x, y, &nm.names, nm.spoken.as_deref(), rules(r)) {
            Ok(Some(t)) => names::mvt::gzip(&t).unwrap_or_else(|_| gz.to_vec()),
            _ => gz.to_vec(),
        }
    }

    /// A raw (not gzip'd) vector tile with main/sub attached.
    pub fn attach_raw(&self, raw: &[u8], z: u8, x: u32, y: u32, r: Rules) -> Vec<u8> {
        let Some(nm) = self.snapshot() else { return raw.to_vec() };
        match names::mvt::attach(raw, z as u32, x, y, &nm.names, nm.spoken.as_deref(), rules(r)) {
            Ok(Some(t)) => t,
            _ => raw.to_vec(),
        }
    }

    /// Load the local copy, then keep it in step with the NAS while the map is in use; make (or
    /// read back) the spoken languages for the catalog's outlines.
    pub fn spawn(self: &Arc<Self>, data: Arc<Data>) {
        // The spoken languages on a thread of their own: made from the outlines on the NAS, they
        // take minutes the first time, which the translations' sync shouldn't wait for.
        let (me, d) = (self.clone(), data.clone());
        std::thread::Builder::new()
            .name("names-spoken".into())
            .spawn(move || loop {
                me.ensure_spoken(&d);
                std::thread::sleep(Duration::from_secs(60));
            })
            .ok();
        let me = self.clone();
        std::thread::Builder::new()
            .name("names".into())
            .spawn(move || {
                std::fs::create_dir_all(&me.dir).ok();
                me.reload();
                let mut next_sync = std::time::Instant::now();
                loop {
                    if std::time::Instant::now() >= next_sync {
                        next_sync = std::time::Instant::now() + Duration::from_secs(60);
                        let recent = now().saturating_sub(me.last_use.load(Ordering::Relaxed)) < 600;
                        if recent {
                            match me.sync(&data) {
                                Ok(true) => me.reload(),
                                Ok(false) => {}
                                Err(e) => eprintln!("translations: {e:#}"),
                            }
                        }
                    }
                    // Files waiting to settle (10 s unchanged) are read once they may be.
                    let until_sync = next_sync.saturating_duration_since(std::time::Instant::now());
                    match me.pending() {
                        Some(d) if d < until_sync => {
                            std::thread::sleep(d.max(Duration::from_secs(1)));
                            me.reload();
                        }
                        _ => std::thread::sleep(until_sync.max(Duration::from_secs(1))),
                    }
                }
            })
            .ok();
    }

    /// Sets the spoken languages (tests: their catalogs have no outlines).
    #[cfg(test)]
    pub fn set_spoken(&self, s: Spoken) {
        *self.spoken.write().unwrap() = Some(("test".into(), Arc::new(s)));
    }

    /// How long until a translation file waiting to settle may be read.
    fn pending(&self) -> Option<Duration> {
        self.names.read().unwrap().as_ref().and_then(|n| n.pending())
    }

    /// Reads new and changed translation files into a copy of the tables (they share their
    /// unchanged parts), then swaps it in: requests never wait for the files.
    pub(crate) fn reload(&self) {
        let t = std::time::Instant::now();
        let cur = self.names.read().unwrap().clone();
        let res = match cur {
            Some(mut n) => n.refresh().map(|changed| (n, changed)),
            None => Names::load(&self.dir).map(|n| (n, true)),
        };
        match res {
            Ok((mut n, changed)) => {
                for w in n.take_warnings() {
                    eprintln!("translations: {w}");
                }
                if changed {
                    let s = n.summary();
                    eprintln!("translations: {} lines, {} names in {} files, languages {:?} ({:.1?})", s.lines, s.names, s.files, s.langs.keys().collect::<Vec<_>>(), t.elapsed());
                    let only_old = s.only_old();
                    if only_old {
                        eprintln!("translations: WARNING: {ONLY_OLD}");
                    }
                    self.only_old.store(only_old, Ordering::Relaxed);
                }
                *self.names.write().unwrap() = Some(n);
            }
            Err(e) => eprintln!("translations: {e:#}"),
        }
    }

    /// The spoken languages for the catalog's outlines: kept as they are when made from those;
    /// else read back from the home, else made from the outlines (read from the mirror or the NAS)
    /// and kept. Without the outlines (offline, never made here): the newest kept, else none.
    fn ensure_spoken(&self, data: &Data) {
        let want = data.catalog().global.get("outlines").and_then(|l| data.content(l));
        let have = self.spoken.read().unwrap().as_ref().map(|(c, _)| c.clone());
        if want.is_some() && have == want {
            return;
        }
        if let Some(content) = &want {
            let file = self.spoken_dir.join(format!("spoken-{}.bin", content.replace('/', "_")));
            if let Some(s) = std::fs::read(&file).ok().and_then(|b| Spoken::from_bytes(&b).ok()) {
                *self.spoken.write().unwrap() = Some((content.clone(), Arc::new(s)));
                return;
            }
            let t = std::time::Instant::now();
            match crate::regions::spoken_from_outlines(data) {
                Ok(Some(s)) => {
                    eprintln!("names: the spoken languages from {content} ({} regions, {:.1?})", s.regions().count(), t.elapsed());
                    std::fs::create_dir_all(&self.spoken_dir).ok();
                    let tmp = file.with_extension("tmp");
                    if std::fs::write(&tmp, s.to_bytes()).and_then(|_| std::fs::rename(&tmp, &file)).is_ok() {
                        // Older rasters go.
                        if let Ok(rd) = std::fs::read_dir(&self.spoken_dir) {
                            for e in rd.flatten() {
                                let n = e.file_name().to_string_lossy().into_owned();
                                if n.starts_with("spoken-") && e.path() != file {
                                    std::fs::remove_file(e.path()).ok();
                                }
                            }
                        }
                    }
                    *self.spoken.write().unwrap() = Some((content.clone(), Arc::new(s)));
                    return;
                }
                Ok(None) => {}
                Err(e) => eprintln!("names: the spoken languages: {e:#}"),
            }
        }
        if have.is_none() {
            // Offline: the newest kept.
            let newest = std::fs::read_dir(&self.spoken_dir).ok().and_then(|rd| {
                rd.flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("spoken-") && e.file_name().to_string_lossy().ends_with(".bin"))
                    .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            });
            if let Some(e) = newest {
                if let Some(s) = std::fs::read(e.path()).ok().and_then(|b| Spoken::from_bytes(&b).ok()) {
                    let content = e.file_name().to_string_lossy().trim_start_matches("spoken-").trim_end_matches(".bin").to_owned();
                    *self.spoken.write().unwrap() = Some((content, Arc::new(s)));
                }
            }
        }
    }

    /// Copy new and changed files from the NAS folder (once they've stopped changing), and drop
    /// local files gone from it. True when anything changed.
    fn sync(&self, data: &Data) -> anyhow::Result<bool> {
        crate::livefolder::sync(data, "translations", &self.dir)
    }
}

/// Said in the log and the status when the folder holds only the area tables' lines.
pub const ONLY_OLD: &str = "the translations folder holds only the old area tables' lines (no kind or languages), which this server doesn't read: no translation shows until the converted lines (translations/0-converted/) are there";

/// A version over every language of the lines and the raster (0 before the lines have loaded).
pub fn version_all(tables: Option<&Namer>) -> u64 {
    tables.map_or(0, Namer::version_all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_aged(p: &Path, text: &str, secs: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    fn france() -> Spoken {
        let pt = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        Spoken::build([names::spoken::Area { code: "FR".into(), area_km2: 1.0, polygons: vec![vec![vec![pt(-5.0, 42.0), pt(8.0, 42.0), pt(8.0, 51.0), pt(-5.0, 51.0)]]] }])
    }

    #[test]
    fn a_drop_on_the_nas_shows_and_the_old_tables_alone_are_said() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // Only the area tables: nothing shows, and the status says why.
        write_aged(&nas.path().join("translations/fr/places-fr.jsonl"), "{\"n\": \"Lac Bleu\", \"main\": \"Lac Bleu\", \"sub\": \"Blue Lake\", \"via\": \"agent:haiku\"}\n", 3600);
        let s = crate::test_state(home.path(), nas.path());
        s.names.set_spoken(france());
        assert!(s.names.sync(&s.data).unwrap());
        s.names.reload();
        let d = s.names.display(Kind::Other, "Lac Bleu", None, &[], 2.0, 46.0);
        assert_eq!((d.main.as_str(), d.sub.as_deref()), ("Lac Bleu", None));
        assert_eq!(s.names.versions()["warning"].as_str(), Some(ONLY_OLD));
        // A line by language, dropped on the NAS (settled): copied, read, shown; the warning goes.
        write_aged(&nas.path().join("translations/answers/fr-001.jsonl"), "{\"n\": \"Lac Bleu\", \"kind\": \"other\", \"langs\": [\"fr\"], \"main\": \"Lac Bleu\", \"sub\": \"Blue Lake\", \"via\": \"agent:haiku\"}\n", 20);
        let before = s.names.version_for_tile(10, 517, 360, 0.0);
        assert!(s.names.sync(&s.data).unwrap());
        // Seen, then read once it has held for ten seconds (the names thread sleeps until then).
        s.names.reload();
        let wait = s.names.pending().expect("waiting to settle");
        assert!(wait <= names::STABLE, "{wait:?}");
        std::thread::sleep(wait + Duration::from_millis(200));
        s.names.reload();
        let d = s.names.display(Kind::Other, "Lac Bleu", None, &[], 2.0, 46.0);
        assert_eq!((d.main.as_str(), d.sub.as_deref()), ("Lac Bleu", Some("Blue Lake")));
        assert!(s.names.versions()["warning"].is_null());
        assert!(s.names.versions()["langs"]["fr"].is_u64());
        // A French tile's ETag part changed.
        assert_ne!(s.names.version_for_tile(10, 517, 360, 0.0), before);
        // Not where French isn't spoken, nor as another kind.
        let d = s.names.display(Kind::Other, "Lac Bleu", None, &[], -30.0, 40.0);
        assert_eq!(d.sub, None);
        let d = s.names.display(Kind::Road, "Lac Bleu", None, &[], 2.0, 46.0);
        assert_eq!(d.sub, None);
        // A file still being written isn't copied.
        write_aged(&nas.path().join("translations/answers/fr-002.jsonl"), "{\"n\": \"Lac Noir\", \"kind\": \"other\", \"langs\": [\"fr\"], \"sub\": \"Black Lake\"}\n", 0);
        assert!(!s.names.sync(&s.data).unwrap());
    }
}
