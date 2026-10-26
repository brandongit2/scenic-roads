//! A dataset kept in the store (`[data] store = true`, docs/inputs.md §6.8) filled once
//! (`scenic-build store-fetch <id> --index <its index>`): every file of its index, and each file's fill mask, that
//! `sources/fetched/<id>/` lacks, downloaded as published (crate::fetch: the build's User-Agent,
//! answers of 429 and 5xx waited out and asked again). Area builds never fetch a stored dataset: a
//! file the index names and the store lacks is an error naming it (crate::dem::generic::stored).
//!
//! Each file goes in beside its name (`<name>.<host>.<pid>.tmp`), is checked whole (its TIFF
//! directories read: crate::whole) and then takes its name; a file the server hasn't (404) is noted
//! `<name>.none`, as the sampler reads it. A file the store has under its name is skipped, so a run
//! stopped anywhere is started again as it is.

use crate::fetch::Fetch;
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Files downloaded at once.
pub const AT_ONCE: usize = 8;

/// What a run did.
#[derive(Debug, Default)]
pub struct Done {
    pub stored: usize,
    pub bytes: u64,
    pub skipped: usize,
    pub none: usize,
    pub failed: Vec<(String, String)>,
}

/// The URLs of the files and fill masks of tile index `index` (its GeoJSON's bytes), in its order:
/// a stored dataset's (or one about to be: its store is filled before it's declared so).
pub fn urls(index: &[u8]) -> Result<Vec<String>> {
    use super::declared::Where;
    let ix = super::declared::parse_index(index).map_err(|e| anyhow::anyhow!("the index: {e}"))?;
    let mut out = Vec::new();
    for (w, m) in ix.files.into_iter().map(|(w, _)| w).zip(ix.masks) {
        let Where::Url(u) = w else {
            bail!("the index names a dropped file: a stored dataset's files are named by URL")
        };
        out.push(u);
        out.extend(m);
    }
    Ok(out)
}

/// Every one of `urls` that store `dir` lacks, fetched by `fetch`, `at_once` at a time; `say` told
/// each hundredth.
pub fn run(
    fetch: &dyn Fetch,
    urls: &[String],
    dir: &store::nasfs::NasPath,
    at_once: usize,
    say: &(dyn Fn(String) + Sync),
) -> Result<Done> {
    dir.create_dir_all().with_context(|| dir.display().to_string())?;
    let host = store::sys::hostname().unwrap_or_else(|| "unknown".into());
    // (This Mac's temporary files from a run stopped part way go first: never taken for a file.)
    let mine = format!(".{host}.");
    for e in dir.read_dir()? {
        if e.name.ends_with(".tmp") && e.name.contains(&mine) {
            e.path.remove_file().ok();
        }
    }
    let (stored, bytes, skipped, none, seen) =
        (AtomicUsize::new(0), AtomicU64::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let pool = rayon::ThreadPoolBuilder::new().num_threads(at_once.max(1)).build()?;
    let failed: Vec<(String, String)> = pool.install(|| {
        urls.par_iter()
            .filter_map(|url| {
                let name = super::generic::stored_name(url);
                let to = dir.join(&name);
                let r = (|| -> Result<()> {
                    if to.exists() || dir.join(format!("{name}.none")).exists() {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        return Ok(());
                    }
                    let Some(b) = fetch.get(url)? else {
                        dir.join(format!("{name}.none")).write(b"")?;
                        none.fetch_add(1, Ordering::Relaxed);
                        return Ok(());
                    };
                    let tmp = dir.join(format!("{name}.{host}.{}.tmp", store::sys::pid()));
                    tmp.write(&b)?;
                    if !tmp.open().is_ok_and(|f| crate::whole::tiff_whole(&f)) {
                        let _ = tmp.remove_file();
                        bail!("not a whole GeoTIFF ({} bytes)", b.len());
                    }
                    tmp.rename(&to)?;
                    stored.fetch_add(1, Ordering::Relaxed);
                    bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
                    Ok(())
                })();
                let n = seen.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 100 == 0 || n == urls.len() {
                    say(format!(
                        "store-fetch: {n}/{} ({} stored, {:.1} GiB)",
                        urls.len(),
                        stored.load(Ordering::Relaxed),
                        bytes.load(Ordering::Relaxed) as f64 / (1u64 << 30) as f64
                    ));
                }
                r.err().map(|e| (name, format!("{e:#}")))
            })
            .collect()
    });
    Ok(Done {
        stored: stored.into_inner(),
        bytes: bytes.into_inner(),
        skipped: skipped.into_inner(),
        none: none.into_inner(),
        failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_what_the_store_lacks_and_notes_what_the_server_hasnt() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("glo30");
        let tif = |v: f32| {
            crate::geotiff::write_f32(4, 4, &[v; 16], 4, [0.0, 0.25, 0.0, 1.0, 0.0, -0.25], 4326, None).unwrap()
        };
        let fetch = crate::fetch::MapFetch(
            [
                ("https://e.org/a/A_DEM.tif".to_string(), Some(tif(1.0))),
                ("https://e.org/a/A_FLM.tif".to_string(), Some(tif(0.0))),
                ("https://e.org/b/B_DEM.tif".to_string(), None),
                ("https://e.org/c/C_DEM.tif".to_string(), Some(b"II*\0short".to_vec())),
            ]
            .into(),
        );
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("K_DEM.tif"), b"kept").unwrap();
        let host = store::sys::hostname().unwrap_or_else(|| "unknown".into());
        std::fs::write(store.join(format!("A_DEM.tif.{host}.1.tmp")), b"half").unwrap();
        let urls: Vec<String> = ["a/A_DEM.tif", "a/A_FLM.tif", "b/B_DEM.tif", "c/C_DEM.tif", "k/K_DEM.tif"]
            .map(|p| format!("https://e.org/{p}"))
            .into();
        let done = run(&fetch, &urls, &store::nasfs::NasPath::local_for_test(&store), 2, &|_| {}).unwrap();
        assert_eq!((done.stored, done.skipped, done.none), (2, 1, 1), "{done:?}");
        assert_eq!(done.failed.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["C_DEM.tif"]);
        let mut names: Vec<String> =
            std::fs::read_dir(&store).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["A_DEM.tif", "A_FLM.tif", "B_DEM.tif.none", "K_DEM.tif"], "no temporary file left");
        assert_eq!(std::fs::read(store.join("A_DEM.tif")).unwrap(), tif(1.0));
        // Again: everything there, nothing asked.
        let again = run(
            &crate::fetch::MapFetch(Default::default()),
            &urls[..3],
            &store::nasfs::NasPath::local_for_test(&store),
            2,
            &|_| {},
        )
        .unwrap();
        assert_eq!((again.stored, again.skipped), (0, 3));
    }
}
