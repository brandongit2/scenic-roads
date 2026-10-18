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

/// The last derived set, by the accepted versions it was derived from.
static READ: std::sync::Mutex<Option<((Option<String>, Option<String>), BTreeSet<String>)>> =
    std::sync::Mutex::new(None);

/// The hosts as the build's records name the accepted versions now (`state/build/manifest.json`
/// under the NAS project folder `root`), derived once per pair of versions.
pub fn now(root: &Path) -> Result<BTreeSet<String>> {
    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json"))?;
    let key = (
        m.get(&crate::inputs::logical(crate::inputs::elevation::UNIT)).cloned(),
        m.get(&crate::inputs::logical(crate::inputs::sources::UNIT)).cloned(),
    );
    if let Some((k, h)) = READ.lock().unwrap().as_ref() {
        if *k == key {
            return Ok(h.clone());
        }
    }
    let h = of(root, &m)?;
    *READ.lock().unwrap() = Some((key, h.clone()));
    Ok(h)
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
        assert_eq!(
            h.into_iter().collect::<Vec<_>>(),
            ["e.org", "esa-worldcover.s3.eu-central-1.amazonaws.com"]
        );
    }
}
