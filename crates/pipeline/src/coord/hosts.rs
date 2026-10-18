//! The data servers whose files a task may read through the coordinator (`/work/net/…/web/…`),
//! derived from the accepted declarations (docs/inputs.md §5.2), never named in code: the hosts of
//! the files the roads' elevation datasets read over the web (crate::dem::declared: their `urls`,
//! their index's URLs, their tile template; a dataset kept in the store, `store = true`, is read
//! from the store, never fetched, and the terrain's are read by its own step) and of the land-cover
//! source's (crate::landcover). Nothing else is fetched for a worker.

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The host of an HTTPS URL (a task's reads go over HTTPS only); None for anything else.
pub fn host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let h = rest.split(['/', '?', '#']).next()?;
    (!h.is_empty() && !h.contains(['{', '}', '@'])).then_some(h)
}

/// The hosts the accepted versions the records `manifest` name allow (none before they're accepted).
pub fn of(root: &Path, manifest: &BTreeMap<String, String>) -> Result<BTreeSet<String>> {
    use crate::dem::declared::{self, Data, Where};
    let mut out = BTreeSet::new();
    let mut add = |u: &str| {
        if let Some(h) = host(u) {
            out.insert(h.to_string());
        }
    };
    if let Some(a) = crate::inputs::open(root, manifest, crate::inputs::elevation::UNIT)? {
        for d in declared::accepted(&a)? {
            match &d.declaration.data {
                Data::Tiles { url, .. } => add(url),
                Data::Geotiff { store: true, .. } => {}
                Data::Geotiff { urls, index, .. } => {
                    urls.iter().for_each(|u| add(u));
                    if let Some(i) = index {
                        let ix = declared::parse_index(&a.read(i)?).map_err(|e| anyhow::anyhow!("{i}: {e}"))?;
                        for (w, _) in &ix.files {
                            if let Where::Url(u) = w {
                                add(u);
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(s) = crate::inputs::sources::open(root, manifest)? {
        if let Ok(lc) = s.role(crate::inputs::sources::LAND_COVER) {
            lc.data.url.iter().chain(&lc.data.urls).for_each(|u| add(u));
            lc.index.iter().for_each(|f| add(&f.url));
        }
    }
    Ok(out)
}

/// How long a derived set is served before the records are read again (a task reads a file a
/// block at a time: not a NAS read per block).
pub const RECHECK: std::time::Duration = std::time::Duration::from_secs(60);

/// The versions a set was derived from: the elevations' and the sources' index names.
type Key = (Option<String>, Option<String>);

/// The last derived set: its versions, the set, and when the records were last read for it.
#[derive(Default)]
pub struct Cache(std::sync::Mutex<Option<(Key, BTreeSet<String>, std::time::Instant)>>);

impl Cache {
    /// The hosts as the records under `root` name the accepted versions, read again at most every
    /// `RECHECK` (`at`: now); derived again only when the versions changed. A failed read keeps
    /// the last set served (a NAS hiccup fails no task); only with none derived yet is it an error.
    pub fn get(&self, root: &Path, at: std::time::Instant) -> Result<BTreeSet<String>> {
        let mut g = self.0.lock().unwrap();
        if let Some((_, h, read)) = g.as_ref() {
            if at.saturating_duration_since(*read) < RECHECK {
                return Ok(h.clone());
            }
        }
        let fresh = (|| -> Result<(Key, Option<BTreeSet<String>>)> {
            let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json"))?;
            let key = (
                m.get(&crate::inputs::logical(crate::inputs::elevation::UNIT)).cloned(),
                m.get(&crate::inputs::logical(crate::inputs::sources::UNIT)).cloned(),
            );
            let same = g.as_ref().is_some_and(|(k, _, _)| *k == key);
            Ok((key, if same { None } else { Some(of(root, &m)?) }))
        })();
        match (fresh, g.as_mut()) {
            (Ok((_, None)), Some(c)) => {
                c.2 = at;
                Ok(c.1.clone())
            }
            (Ok((key, Some(h))), _) => {
                *g = Some((key, h.clone(), at));
                Ok(h)
            }
            (Ok((_, None)), None) => unreachable!("a set unchanged with none derived"),
            (Err(e), Some(c)) => {
                eprintln!("coord: the declarations' hosts not read again, the last kept: {e:#}");
                c.2 = at;
                Ok(c.1.clone())
            }
            (Err(e), None) => Err(e),
        }
    }
}

static CACHE: std::sync::LazyLock<Cache> = std::sync::LazyLock::new(Cache::default);

/// The hosts as the build's records name the accepted versions now (`state/build/manifest.json`
/// under the NAS project folder `root`): Cache::get, process-wide.
pub fn now(root: &Path) -> Result<BTreeSet<String>> {
    CACHE.get(root, std::time::Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(host("https://a.b.org/x/{z}.png"), Some("a.b.org"));
        assert_eq!(host("https://a.b.org?q"), Some("a.b.org"));
        assert_eq!(host("http://a.b.org/x"), None);
        assert_eq!(host("sources/x.tif"), None);
    }

    /// The staged land cover and the fixture's elevations: the land cover's host and the world
    /// dataset's tile host; the terrain's left out.
    #[test]
    fn derived() {
        let d = tempfile::tempdir().unwrap();
        let mut m = BTreeMap::new();
        assert!(of(d.path(), &m).unwrap().is_empty());
        crate::inputs::sources::accept_fixture(d.path(), &mut m);
        crate::inputs::elevation::accept_fixture(d.path(), &mut m);
        let h = of(d.path(), &m).unwrap();
        assert_eq!(h.into_iter().collect::<Vec<_>>(), ["e.org", "esa-worldcover.s3.eu-central-1.amazonaws.com"]);
    }

    /// The cache: the records read at most every RECHECK; a failed read keeps the last set; with
    /// none derived yet, an error.
    #[test]
    fn cached() {
        use std::time::{Duration, Instant};
        let d = tempfile::tempdir().unwrap();
        let mf = d.path().join("state/build/manifest.json");
        std::fs::create_dir_all(mf.parent().unwrap()).unwrap();
        let t0 = Instant::now();
        // A manifest that can't be read, none derived yet: an error.
        std::fs::write(&mf, b"{not json").unwrap();
        let c = Cache::default();
        assert!(c.get(d.path(), t0).is_err());
        let mut m = BTreeMap::new();
        crate::inputs::sources::accept_fixture(d.path(), &mut m);
        crate::inputs::elevation::accept_fixture(d.path(), &mut m);
        std::fs::write(&mf, serde_json::to_vec(&m).unwrap()).unwrap();
        let want = of(d.path(), &m).unwrap();
        assert_eq!(c.get(d.path(), t0).unwrap(), want);
        // Within RECHECK, nothing read: a changed manifest isn't seen yet.
        std::fs::write(&mf, b"{}").unwrap();
        assert_eq!(c.get(d.path(), t0 + RECHECK - Duration::from_secs(1)).unwrap(), want);
        // A read that fails after it: the last set kept.
        std::fs::write(&mf, b"{not json").unwrap();
        assert_eq!(c.get(d.path(), t0 + RECHECK).unwrap(), want);
        // Read again (after RECHECK more): the versions changed (none accepted), so none.
        std::fs::write(&mf, b"{}").unwrap();
        assert!(c.get(d.path(), t0 + RECHECK * 2).unwrap().is_empty());
    }
}
