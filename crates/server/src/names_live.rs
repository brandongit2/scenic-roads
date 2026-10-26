//! Display names, live (docs/names.md): the translations' accepted version (docs/inputs.md §6.3:
//! the files the input check took in) is copied from the NAS to this Mac (so it works offline and
//! loads fast: crate::accepted), compiled by the `names` crate, and attached to everything the
//! server serves. The records are read every minute, but only while the map is in use (a request
//! in the last ten minutes), so an idle server leaves the NAS alone.
//!
//! The languages spoken where a thing is come from the catalogue's `global/spoken`; a catalogue without
//! one has them made from its outlines (the pass's) and the languages input's accepted table
//! (docs/inputs.md §6.2: kept here as the translations are, crate::accepted), made into a raster
//! once per outlines file and table and kept in the home (`names/spoken-<content>+<table>.bin`), so they hold offline too.

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

const AREAS: names::mvt::LayerRule<'static> = names::mvt::LayerRule {
    layer: "a",
    name_keys: &["name", "n"],
    en_keys: &["en"],
    kind: names::mvt::KindRule::Fixed(Kind::Other),
};
const STATIONS: names::mvt::LayerRule<'static> = names::mvt::LayerRule {
    layer: "s",
    name_keys: &["n"],
    en_keys: &["en"],
    kind: names::mvt::KindRule::Fixed(Kind::Other),
};

fn rules(r: Rules) -> &'static [names::mvt::LayerRule<'static>] {
    match r {
        Rules::Labels => &[names::mvt::LABELS],
        Rules::Basemap => &[names::mvt::OPENMAPTILES],
        Rules::Areas => &[AREAS],
        Rules::Stations => &[STATIONS],
    }
}

pub struct NamesState {
    /// The lines as last read (refreshed on a copy, then swapped in).
    names: RwLock<Option<Names>>,
    /// What lookups read: the lines and the spoken languages together, behind an `Arc` (a lookup
    /// takes a reference, never a copy). None until both are there: before the spoken languages
    /// no line would be found but by OSM's tags, so names count as not loaded.
    current: RwLock<Option<Arc<Namer>>>,
    /// The languages spoken where, and the outlines' content name they were made from.
    spoken: RwLock<Option<(String, Arc<Spoken>)>>,
    /// The local copy of the accepted version.
    accepted: PathBuf,
    /// An earlier app's copy of the old folder, removed once the copy here is the records' version.
    old: PathBuf,
    /// Where the rasters are kept.
    spoken_dir: PathBuf,
    /// The local copy of the languages' accepted version (for a catalogue without `global/spoken`).
    languages: PathBuf,
    /// Seconds since the epoch of the last request that used names.
    last_use: AtomicU64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl NamesState {
    pub fn new(home: &Path) -> Arc<NamesState> {
        Arc::new(NamesState {
            names: RwLock::new(None),
            current: RwLock::new(None),
            spoken: RwLock::new(None),
            accepted: crate::accepted::local_dir(home, pipeline::inputs::translations::UNIT),
            // (An earlier app's copy of the old folder: removed by a sync, crate::accepted::sync.)
            old: home.join(pipeline::inputs::translations::UNIT),
            spoken_dir: home.join("names"),
            languages: crate::accepted::local_dir(home, pipeline::inputs::languages::UNIT),
            last_use: AtomicU64::new(now()),
        })
    }

    /// Record a use (keeps the folder polled).
    pub fn touch(&self) {
        self.last_use.store(now(), Ordering::Relaxed);
    }

    /// The lines and the spoken languages as they are now (shared: nothing is copied, and nothing
    /// waits on it); None before both have loaded.
    pub fn snapshot(&self) -> Option<Arc<Namer>> {
        self.current.read().unwrap().clone()
    }

    /// Puts the lines and the spoken languages together for lookups, once both are there.
    fn publish(&self) {
        let names = self.names.read().unwrap().clone();
        let spoken = self.spoken.read().unwrap().as_ref().map(|(_, s)| s.clone());
        *self.current.write().unwrap() = match (names, spoken) {
            (Some(names), Some(spoken)) => Some(Arc::new(Namer { names, spoken: Some(spoken) })),
            _ => None,
        };
    }

    /// Sets the spoken languages, made from `source` (a content name), and publishes them.
    fn set_spoken_from(&self, source: String, s: Spoken) {
        *self.spoken.write().unwrap() = Some((source, Arc::new(s)));
        self.publish();
    }

    /// A name's display form (`osm`: the languages OSM gives it, where known). Before the lines
    /// and the spoken languages have loaded: the name and its own English.
    pub fn display(
        &self,
        kind: Kind,
        name: &str,
        own_en: Option<&str>,
        osm: &[Lang],
        lon: f64,
        lat: f64,
    ) -> names::Display {
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

    /// Each language's version and the raster's (for the catalogue's status).
    pub fn versions(&self) -> serde_json::Value {
        let g = self.names.read().unwrap();
        let mut langs = serde_json::Map::new();
        if let Some(n) = g.as_ref() {
            for (l, v) in n.summary().langs {
                langs.insert(l, serde_json::Value::from(v));
            }
        }
        let spoken = self
            .spoken
            .read()
            .unwrap()
            .as_ref()
            .map(|(c, s)| serde_json::json!({"outlines": c, "version": s.version()}));
        let mut out = serde_json::json!({"langs": langs, "spoken": spoken});
        if self.current.read().unwrap().is_none() {
            out["waiting"] = serde_json::Value::from(if g.is_none() {
                "the translations"
            } else {
                "the spoken languages (names show their own English alone until then)"
            });
        }
        out
    }

    /// A version over every language (for files that span them all).
    pub fn version_all(&self) -> u64 {
        version_all(self.snapshot().as_deref())
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
    /// read back) the spoken languages for the catalogue's outlines.
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
                me.reload();
                let mut next_sync = std::time::Instant::now();
                loop {
                    if std::time::Instant::now() >= next_sync {
                        next_sync = std::time::Instant::now() + Duration::from_secs(60);
                        let recent = now().saturating_sub(me.last_use.load(Ordering::Relaxed)) < 600;
                        if recent && me.sync(&data) {
                            me.reload();
                        }
                    }
                    std::thread::sleep(
                        next_sync.saturating_duration_since(std::time::Instant::now()).max(Duration::from_secs(1)),
                    );
                }
            })
            .ok();
    }

    /// Sets the spoken languages (tests: their catalogues have no outlines).
    #[cfg(test)]
    pub fn set_spoken(&self, s: Spoken) {
        self.set_spoken_from("test".into(), s);
    }

    /// Reads the accepted files new since the last read into a copy of the tables (they share
    /// their unchanged parts), then swaps it in: requests never wait for the files.
    pub(crate) fn reload(&self) {
        let t = std::time::Instant::now();
        let files = crate::accepted::files(&self.accepted).unwrap_or_default();
        let (mut n, changed) = match self.names.read().unwrap().clone() {
            Some(mut n) => {
                let changed = n.set_files(&files);
                (n, changed)
            }
            None => (Names::from_files(&files), true),
        };
        for w in n.take_warnings() {
            eprintln!("translations: {w}");
        }
        if changed {
            let s = n.summary();
            eprintln!(
                "translations: {} lines, {} names in {} files, languages {:?} ({:.1?})",
                s.lines,
                s.names,
                s.files,
                s.langs.keys().collect::<Vec<_>>(),
                t.elapsed()
            );
        }
        *self.names.write().unwrap() = Some(n);
        if changed || self.current.read().unwrap().is_none() {
            self.publish();
        }
    }

    /// The spoken languages: the catalogue's `global/spoken` (the `spoken` job's, mirrored as an
    /// essential: offline too); in a catalogue without it, made from the catalogue's outlines (read
    /// from the mirror or the NAS) and kept in the home, or read back from there. Without either
    /// (offline, never made here): the newest kept, else none, and names stay unloaded.
    fn ensure_spoken(&self, data: &Data) {
        let cat = data.catalogue();
        let have = self.spoken.read().unwrap().as_ref().map(|(c, _)| c.clone());
        if let Some(logical) = cat.global.get("spoken") {
            let content = data.content(logical);
            if content.is_some() && have == content {
                return;
            }
            match data.global(logical).map(|b| b.map(|b| Spoken::from_bytes(&b))) {
                Ok(Some(Ok(s))) => {
                    eprintln!(
                        "names: the spoken languages from {} ({} regions)",
                        content.as_deref().unwrap_or(logical),
                        s.regions().count()
                    );
                    self.set_spoken_from(content.unwrap_or_else(|| logical.clone()), s);
                    return;
                }
                Ok(Some(Err(e))) => eprintln!("names: {logical}: {e:#}"),
                Ok(None) => {}
                Err(e) => eprintln!("names: {logical}: {e:#}"),
            }
        }
        let want = cat.global.get("outlines").and_then(|l| data.content(l));
        let table = self.table(data);
        let want = want.zip(table.as_ref()).map(|(c, (_, tag))| format!("{c}+{tag}"));
        if want.is_some() && have == want {
            return;
        }
        if let Some(content) = &want {
            let file = self.spoken_dir.join(format!("spoken-{}.bin", content.replace('/', "_")));
            if let Some(s) = std::fs::read(&file).ok().and_then(|b| Spoken::from_bytes(&b).ok()) {
                self.set_spoken_from(content.clone(), s);
                return;
            }
            let t = std::time::Instant::now();
            match table.as_ref().map(|(t, _)| crate::regions::spoken_from_outlines(data, t)).unwrap_or(Ok(None)) {
                Ok(Some(s)) => {
                    eprintln!(
                        "names: the spoken languages from {content} ({} regions, {:.1?})",
                        s.regions().count(),
                        t.elapsed()
                    );
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
                    self.set_spoken_from(content.clone(), s);
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
                    .filter(|e| {
                        e.file_name().to_string_lossy().starts_with("spoken-")
                            && e.file_name().to_string_lossy().ends_with(".bin")
                    })
                    .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            });
            if let Some(e) = newest {
                if let Some(s) = std::fs::read(e.path()).ok().and_then(|b| Spoken::from_bytes(&b).ok()) {
                    let content = e
                        .file_name()
                        .to_string_lossy()
                        .trim_start_matches("spoken-")
                        .trim_end_matches(".bin")
                        .to_owned();
                    self.set_spoken_from(content, s);
                }
            }
        }
    }

    /// The languages table for a catalogue without `global/spoken`, and what names it (its file's
    /// hash): the languages' accepted version, kept here (brought up to date first while the NAS is
    /// there); None before there's one.
    fn table(&self, data: &Data) -> Option<(names::Table, String)> {
        let unit = pipeline::inputs::languages::UNIT;
        if let Err(e) = crate::accepted::synced(data, unit, &self.languages) {
            eprintln!("names: {unit}: {e:#}");
        }
        let kept =
            crate::accepted::files(&self.languages).and_then(|f| f.get(pipeline::inputs::languages::TABLE).cloned());
        if let Some((local, content)) = kept {
            match std::fs::read_to_string(&local) {
                Ok(text) => {
                    return Some((
                        names::Table::parse(&text),
                        pipeline::inputs::hash_of(&content).unwrap_or("accepted").to_string(),
                    ))
                }
                Err(e) => eprintln!("names: {}: {e}", local.display()),
            }
        }
        None
    }

    /// Brings the local copy of the accepted version up to date: whether it changed.
    pub(crate) fn sync(&self, data: &Data) -> bool {
        match crate::accepted::sync(data, pipeline::inputs::translations::UNIT, &self.accepted, &self.old) {
            Ok(changed) => changed,
            Err(e) => {
                eprintln!("translations: {e:#}");
                false
            }
        }
    }
}

/// A version over every language of the lines and the raster (0 before they have loaded).
pub fn version_all(tables: Option<&Namer>) -> u64 {
    tables.map_or(0, Namer::version_all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn france() -> Spoken {
        let pt = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        Spoken::build(
            &names::Table::parse("FR\tfr\n"),
            [names::spoken::Area {
                code: "FR".into(),
                area_km2: 1.0,
                polygons: vec![vec![vec![pt(-5.0, 42.0), pt(8.0, 42.0), pt(8.0, 51.0), pt(-5.0, 51.0)]]],
            }],
        )
    }

    fn line(sub: &str) -> String {
        format!("{{\"n\": \"Lac Bleu\", \"kind\": \"other\", \"langs\": [\"fr\"], \"main\": \"Lac Bleu\", \"sub\": \"{sub}\"}}\n")
    }

    #[test]
    fn names_wait_for_the_spoken_languages() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        crate::accepted::put(
            &store::nasfs::NasRoot::local_for_test(nas.path()),
            "translations",
            &[("0-converted/french.jsonl", &line("Blue Lake"))],
        );
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let st = &s.names;
        assert!(st.sync(&s.data));
        st.reload();
        // The lines, but no spoken languages: not loaded (own English alone), and said.
        assert!(st.snapshot().is_none());
        let d = st.display(Kind::Other, "Lac Bleu", Some("Own"), &[], 2.0, 46.0);
        assert_eq!(d.sub.as_deref(), Some("Own"));
        assert_eq!(st.display(Kind::Other, "Lac Bleu", None, &[], 2.0, 46.0).sub, None);
        assert!(st.versions()["waiting"].as_str().is_some_and(|w| w.starts_with("the spoken languages")));
        assert_eq!(st.version_for_tile(10, 517, 360, 0.0), 0);
        // Once they're there: loaded, one shared copy.
        st.set_spoken(france());
        let (a, b) = (st.snapshot().unwrap(), st.snapshot().unwrap());
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(st.display(Kind::Other, "Lac Bleu", None, &[], 2.0, 46.0).sub.as_deref(), Some("Blue Lake"));
        assert!(st.versions()["waiting"].is_null());
        assert_ne!(st.version_for_tile(10, 517, 360, 0.0), 0);
    }

    #[test]
    fn the_accepted_version_is_followed_and_the_old_folder_never_read() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let sub = |s: &crate::S| s.names.display(Kind::Other, "Lac Bleu", None, &[], 2.0, 46.0).sub;
        // An earlier app's copy of the old folder: removed, never read; the NAS's old folder too.
        std::fs::create_dir_all(home.path().join("translations")).unwrap();
        std::fs::write(home.path().join("translations/x.jsonl"), line("Wrong Lake")).unwrap();
        std::fs::create_dir_all(nas.path().join("translations")).unwrap();
        std::fs::write(nas.path().join("translations/late.jsonl"), line("Wrong Lake")).unwrap();
        let v = crate::accepted::put(
            &store::nasfs::NasRoot::local_for_test(nas.path()),
            "translations",
            &[("answers/fr-001.jsonl", &line("Blue Lake"))],
        );
        let index: pipeline::inputs::Index =
            serde_json::from_slice(&std::fs::read(nas.path().join(&v)).unwrap()).unwrap();
        let copy = nas.path().join(&index.files["answers/fr-001.jsonl"].file);
        let good = std::fs::read(&copy).unwrap();
        // (Its bytes not its name's yet: this Mac can't copy the records' version.)
        std::fs::write(&copy, line("Wrong Lake")).unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        s.names.set_spoken(france());
        // The old copy kept while the copy here isn't the records' version.
        assert!(!s.names.sync(&s.data));
        assert!(home.path().join("translations").exists());
        std::fs::write(&copy, good).unwrap();
        assert!(s.names.sync(&s.data));
        assert!(!home.path().join("translations").exists());
        s.names.reload();
        assert_eq!(sub(&s).as_deref(), Some("Blue Lake"));
        assert!(s.names.versions()["langs"]["fr"].is_u64());
        let before = s.names.version_for_tile(10, 517, 360, 0.0);
        // Nothing new: nothing copied, the version holds.
        assert!(!s.names.sync(&s.data));
        // A batch taken in: shown, and a French tile's ETag part changes; not where French isn't
        // spoken, nor as another kind.
        crate::accepted::put(
            &store::nasfs::NasRoot::local_for_test(nas.path()),
            "translations",
            &[("answers/fr-001.jsonl", &line("Blue Lake")), ("answers/fr-002.jsonl", &line("Azure Lake"))],
        );
        assert!(s.names.sync(&s.data));
        s.names.reload();
        assert_eq!(sub(&s).as_deref(), Some("Azure Lake"));
        assert_ne!(s.names.version_for_tile(10, 517, 360, 0.0), before);
        assert_eq!(s.names.display(Kind::Other, "Lac Bleu", None, &[], -30.0, 40.0).sub, None);
        assert_eq!(s.names.display(Kind::Road, "Lac Bleu", None, &[], 2.0, 46.0).sub, None);
        // Started again, the NAS away: the accepted version kept here.
        let again = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(&home.path().join("no-nas")));
        assert!(!home.path().join("translations").exists());
        again.names.set_spoken(france());
        again.names.reload();
        assert_eq!(sub(&again).as_deref(), Some("Azure Lake"));
    }
}
