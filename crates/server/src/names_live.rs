//! Display names, live (docs/plan.md §7): the user's translation files are copied from the NAS
//! folder to this Mac (so they work offline and load fast), compiled by the `names` crate, and
//! attached to everything the server serves. The folder is polled every minute, but only while the
//! map is in use (a request in the last ten minutes), so an idle server leaves the NAS alone.

use crate::data::Data;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Which tiles: our label tiles (name `n`, English `en`), or the basemap (OpenMapTiles schema).
#[derive(Clone, Copy)]
pub enum Rules {
    Labels,
    Basemap,
}

fn rules(r: Rules) -> &'static [names::mvt::LayerRule<'static>] {
    match r {
        Rules::Labels => &[names::mvt::LABELS],
        Rules::Basemap => &[names::mvt::OPENMAPTILES],
    }
}

pub struct NamesState {
    names: RwLock<Option<names::display::Names>>,
    /// The local copy of the NAS folder.
    dir: PathBuf,
    /// Seconds since the epoch of the last request that used names.
    last_use: AtomicU64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl NamesState {
    pub fn new(home: &Path) -> Arc<NamesState> {
        Arc::new(NamesState { names: RwLock::new(None), dir: home.join("translations"), last_use: AtomicU64::new(now()) })
    }

    /// Record a use (keeps the folder polled).
    pub fn touch(&self) {
        self.last_use.store(now(), Ordering::Relaxed);
    }

    /// A name's display form: road names (`Kind::Road`) read the roads tables first, every other
    /// name the places tables. Before the tables have loaded: the name and its own English.
    pub fn display(&self, kind: names::Kind, name: &str, own_en: Option<&str>, lon: f64, lat: f64) -> names::display::Display {
        self.touch();
        match self.names.read().unwrap().as_ref() {
            Some(n) => n.display(kind, name, own_en, lon, lat),
            None => names::DisplayRef::new(name, own_en).to_display(),
        }
    }

    /// A version for a tile's names: the versions of the reading areas its features can fall in
    /// (so a drop for Japan doesn't change the ETags of tiles in France). `buffer`: how far, in
    /// tiles, its features reach beyond it (the basemap's labels: a whole tile; ours: none).
    pub fn version_for_tile(&self, z: u8, x: u32, y: u32, buffer: f64) -> u64 {
        let [w, s, e, n] = names::mvt::tile_bounds(z as u32, x, y, buffer);
        let areas = names::areas_in(w, s, e, n);
        let g = self.names.read().unwrap();
        let Some(nm) = g.as_ref() else { return 0 };
        let mut h: u64 = 0xcbf29ce484222325;
        for a in areas {
            h ^= nm.version(a);
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Each reading area's translations version (for the catalog's status).
    pub fn versions(&self) -> std::collections::BTreeMap<&'static str, u64> {
        let g = self.names.read().unwrap();
        names::area::AREAS.iter().map(|a| (*a, g.as_ref().map(|n| n.version(a)).unwrap_or(0))).collect()
    }

    /// A version over every reading area (for files that span them all).
    pub fn version_all(&self) -> u64 {
        let g = self.names.read().unwrap();
        let Some(nm) = g.as_ref() else { return 0 };
        let mut h: u64 = 0xcbf29ce484222325;
        for a in names::area::AREAS {
            h ^= nm.version(a);
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// A gzip'd vector tile with main/sub attached (the original when nothing changed or it can't be
    /// read).
    pub fn attach_gz(&self, gz: &[u8], z: u8, x: u32, y: u32, r: Rules) -> Vec<u8> {
        let Ok(raw) = names::mvt::gunzip_if_gzip(gz) else { return gz.to_vec() };
        let g = self.names.read().unwrap();
        let Some(nm) = g.as_ref() else { return gz.to_vec() };
        match names::mvt::attach(&raw, z as u32, x, y, nm, &rules(r)) {
            Ok(Some(t)) => names::mvt::gzip(&t).unwrap_or_else(|_| gz.to_vec()),
            _ => gz.to_vec(),
        }
    }

    /// A raw (not gzip'd) vector tile with main/sub attached.
    pub fn attach_raw(&self, raw: &[u8], z: u8, x: u32, y: u32, r: Rules) -> Vec<u8> {
        let g = self.names.read().unwrap();
        let Some(nm) = g.as_ref() else { return raw.to_vec() };
        match names::mvt::attach(raw, z as u32, x, y, nm, &rules(r)) {
            Ok(Some(t)) => t,
            _ => raw.to_vec(),
        }
    }

    /// Load the local copy, then keep it in step with the NAS while the map is in use.
    pub fn spawn(self: &Arc<Self>, data: Arc<Data>) {
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

    /// How long until a translation file waiting to settle may be read.
    fn pending(&self) -> Option<Duration> {
        self.names.read().unwrap().as_ref().and_then(|n| n.pending())
    }

    /// Reads new and changed translation files into a copy of the tables (they share their
    /// unchanged parts), then swaps it in: requests never wait for the files.
    fn reload(&self) {
        let t = std::time::Instant::now();
        let cur = self.names.read().unwrap().clone();
        let res = match cur {
            Some(mut n) => n.refresh().map(|changed| (n, changed)),
            None => names::display::Names::load(&self.dir).map(|n| (n, true)),
        };
        match res {
            Ok((mut n, changed)) => {
                for w in n.take_warnings() {
                    eprintln!("translations: {w}");
                }
                if changed {
                    eprintln!("translations: {} lines in {} areas ({:.1?})", n.entries(), n.areas().count(), t.elapsed());
                }
                *self.names.write().unwrap() = Some(n);
            }
            Err(e) => eprintln!("translations: {e:#}"),
        }
    }


    /// Copy new and changed files from the NAS folder (once they've stopped changing), and drop
    /// local files gone from it. True when anything changed.
    fn sync(&self, data: &Data) -> anyhow::Result<bool> {
        crate::livefolder::sync(data, "translations", &self.dir)
    }
}
