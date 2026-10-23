//! The data servers whose files a task may read through the coordinator (`/work/net/…/web/…`),
//! derived from the accepted declarations (docs/inputs.md §5.2), never named in code: the hosts of
//! the land-cover source's files (crate::landcover). Nothing else is fetched for a worker: the
//! roads' heights read the terrain staged in the unit's folder (crate::road_terrain).

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
    let mut out = BTreeSet::new();
    let mut add = |u: &str| {
        if let Some(h) = host(u) {
            out.insert(h.to_string());
        }
    };
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

/// The version a set was derived from: the sources' index name.
type Key = Option<String>;

/// The last derived set (its versions, the set, when the records were last read for it), and
/// whether a request is reading them again now.
#[derive(Default)]
struct State {
    set: Option<(Key, BTreeSet<String>, std::time::Instant)>,
    refreshing: bool,
}

/// The derived set, read again at most every `RECHECK`.
#[derive(Default)]
pub struct Cache(std::sync::Mutex<State>);

impl Cache {
    /// The hosts as the records under `root` name the accepted versions (`at`: now):
    /// Cache::get_with reading `state/build/manifest.json` and deriving with `of`.
    pub fn get(&self, root: &Path, at: std::time::Instant) -> Result<BTreeSet<String>> {
        self.get_with(at, || crate::out::read_manifest(root), |m| of(root, m))
    }

    /// The set, the records read again (`read`) at most every `RECHECK`, derived again (`derive`)
    /// only when the versions changed. One request reads them, without the lock held (a slow NAS
    /// blocks no other request): the others serve the last set meanwhile, or, with none derived
    /// yet, are told so (an error). A failed read keeps the last set (a NAS hiccup fails no task).
    pub fn get_with(
        &self,
        at: std::time::Instant,
        read: impl FnOnce() -> Result<BTreeMap<String, String>>,
        derive: impl FnOnce(&BTreeMap<String, String>) -> Result<BTreeSet<String>>,
    ) -> Result<BTreeSet<String>> {
        let had = {
            let mut g = self.0.lock().unwrap();
            if let Some((_, h, last)) = &g.set {
                if g.refreshing || at.saturating_duration_since(*last) < RECHECK {
                    return Ok(h.clone());
                }
            } else if g.refreshing {
                anyhow::bail!("the declarations' hosts are being read");
            }
            g.refreshing = true;
            g.set.as_ref().map(|(k, _, _)| k.clone())
        };
        let fresh =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(Key, Option<BTreeSet<String>>)> {
                let m = read()?;
                let key = m.get(&crate::inputs::logical(crate::inputs::sources::UNIT)).cloned();
                let same = had.as_ref() == Some(&key);
                Ok((key, if same { None } else { Some(derive(&m)?) }))
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("reading them panicked")));
        let mut g = self.0.lock().unwrap();
        g.refreshing = false;
        match (fresh, g.set.as_mut()) {
            (Ok((key, Some(h))), _) => {
                g.set = Some((key, h.clone(), at));
                Ok(h)
            }
            (Ok((_, None)), Some(c)) => {
                c.2 = at;
                Ok(c.1.clone())
            }
            (Ok((_, None)), None) => anyhow::bail!("the declarations' hosts: none derived"),
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

    /// The staged land cover and the fixture's elevations: the land cover's host alone.
    #[test]
    fn derived() {
        let d = tempfile::tempdir().unwrap();
        let mut m = BTreeMap::new();
        assert!(of(d.path(), &m).unwrap().is_empty());
        crate::inputs::sources::accept_fixture(d.path(), &mut m);
        crate::inputs::elevation::accept_fixture(d.path(), &mut m);
        let h = of(d.path(), &m).unwrap();
        assert_eq!(h.into_iter().collect::<Vec<_>>(), ["esa-worldcover.s3.eu-central-1.amazonaws.com"]);
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

    /// A recheck whose versions are the same derives nothing again.
    #[test]
    fn same_versions_not_derived() {
        let c = Cache::default();
        let t0 = std::time::Instant::now();
        let m = BTreeMap::from([(crate::inputs::logical(crate::inputs::sources::UNIT), "v1".to_string())]);
        let n = std::cell::Cell::new(0);
        let derive = |_: &BTreeMap<String, String>| {
            n.set(n.get() + 1);
            Ok(BTreeSet::from(["a.org".to_string()]))
        };
        assert_eq!(c.get_with(t0, || Ok(m.clone()), derive).unwrap().len(), 1);
        assert_eq!(c.get_with(t0 + RECHECK, || Ok(m.clone()), derive).unwrap().len(), 1);
        assert_eq!(n.get(), 1);
    }

    /// A get while another reads the records (a slow NAS) answers at once: the last set, or with
    /// none derived yet, an error.
    #[test]
    fn refresh_blocks_none() {
        use std::sync::{mpsc, Arc};
        use std::time::{Duration, Instant};
        let c = Arc::new(Cache::default());
        let t0 = Instant::now();
        let one = |h: &str| BTreeSet::from([h.to_string()]);
        // A get on another thread whose read waits for `go`, once it has begun.
        let slow = |c: Arc<Cache>, at: Instant, h: &'static str| {
            let (began, begun) = mpsc::channel();
            let (go, hold) = mpsc::channel::<()>();
            let t = std::thread::spawn(move || {
                c.get_with(
                    at,
                    || {
                        began.send(()).unwrap();
                        hold.recv().unwrap();
                        Ok(BTreeMap::from([(crate::inputs::logical(crate::inputs::sources::UNIT), h.to_string())]))
                    },
                    |m| Ok(BTreeSet::from([m.values().next().unwrap().clone()])),
                )
            });
            begun.recv().unwrap();
            (t, go)
        };
        let unread = || -> Result<BTreeMap<String, String>> { panic!("read twice") };
        let underived = |_: &BTreeMap<String, String>| -> Result<BTreeSet<String>> { panic!("derived twice") };
        // None derived yet, one reading: another is told so at once.
        let (t, go) = slow(c.clone(), t0, "a.org");
        assert!(c.get_with(t0, unread, underived).is_err());
        go.send(()).unwrap();
        assert_eq!(t.join().unwrap().unwrap(), one("a.org"));
        // Stale, one reading again (new versions): another gets the last set at once.
        let later = t0 + RECHECK + Duration::from_secs(1);
        let (t, go) = slow(c.clone(), later, "b.org");
        assert_eq!(c.get_with(later, unread, underived).unwrap(), one("a.org"));
        go.send(()).unwrap();
        assert_eq!(t.join().unwrap().unwrap(), one("b.org"));
        assert_eq!(c.get_with(later, unread, underived).unwrap(), one("b.org"));
    }
}
