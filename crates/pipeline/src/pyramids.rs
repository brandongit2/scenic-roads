//! The elevation pyramids published at one zoom and read tile by tile (GSI's five layers:
//! crate::terrain_base::by_tile), kept on the NAS so a terrain job reads each tile there, not over
//! the network (task #806: GSI's tiles were 41 % of terrain-hi's rebuild time, every run fetching
//! them all again, its blend and terrain-deep again on top).
//!
//! Each pyramid's tiles are packed as AWS's raw tiles are (crate::rawpack: content-named archives by
//! shard, a z15 tile's its z6 tile's, an index read once and each tile read by byte range; an empty
//! entry for a tile the server hasn't), in `sources/pyramids/<folder>/` (`folder`: the URL template's
//! host and path, so a layer moved to another URL is stored afresh). Filled by its own step, the
//! lead's alone (`fill`, `scenic-build pyramids-fill`): a target's tiles (`want`, those its
//! terrain-hi run reads: the native tiles under the pixels of its z9–12 tiles that aren't open sea),
//! fetched once each with the build's fetcher (its User-Agent, its per-host limit), then packed.
//! Jobs only read it (`Stored`): a tile it lacks is fetched as before, never written back.

use crate::dem::declared::{Plan, Source};
use crate::fetch::{Fetch, Within};
use anyhow::{Context, Result};
use roadcore::archive::{tile_key, Entry};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use store::nasfs::NasPath;
use store::range::RangeRead;
use store::sys::PosIo;

/// The stored pyramids' folder, under the build's root.
pub const DIR: &str = "sources/pyramids";

/// The archives' metadata (crate::rawpack::Packer::with_meta).
const META: &str = r#"{"kind":"elevation pyramid tiles"}"#;

/// Tiles fetched at once by `fill` (the fetcher holds a host to crate::fetch's HOST_REQUESTS).
pub const FETCHES: usize = 16;

/// A pyramid's folder under `DIR`: its URL template's host and path up to `{z}`, `/` as `-`
/// (`https://cyberjapandata.gsi.go.jp/xyz/dem5a_png/{z}/{x}/{y}.png` →
/// `cyberjapandata.gsi.go.jp-xyz-dem5a_png`).
pub fn folder(template: &str) -> String {
    let t = template.split_once("://").map_or(template, |(_, r)| r);
    let t = t.split("{z}").next().unwrap_or(t);
    t.trim_end_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' })
        .collect()
}

/// Tile z/x/y of template `t`, when `url` is one of its tiles.
fn tile_of(t: &str, url: &str) -> Option<(u8, u32, u32)> {
    let (pre, rest) = t.split_once("{z}")?;
    let suf = rest.strip_prefix("/{x}/{y}")?;
    let mid = url.strip_prefix(pre)?.strip_suffix(suf)?;
    let mut it = mid.split('/').map(|v| v.parse::<u32>().ok());
    let (z, x, y) = (it.next()??, it.next()??, it.next()??);
    (it.next().is_none() && z <= 30).then_some((z as u8, x, y))
}

/// The templates of plan `plan`'s pyramids read tile by tile, with their zoom and cover.
pub fn of(plan: &Plan) -> Vec<(String, u32, Option<Vec<[f64; 4]>>)> {
    let mut v: Vec<(String, u32, Option<Vec<[f64; 4]>>)> = Vec::new();
    for d in plan.datasets.iter().filter(|d| crate::terrain_base::by_tile(d)) {
        if let Source::Tiles { url, zoom, .. } = &d.source {
            if !v.iter().any(|p| p.0 == *url) {
                v.push((url.clone(), *zoom, d.cover.clone()));
            }
        }
    }
    v
}

/// A shard's archives as read: each one's file and entries, newest first.
type Opened = Vec<(std::fs::File, Vec<Entry>)>;

/// One pyramid's store as read.
struct Layer {
    template: String,
    store: NasPath,
    /// Its index, read when first wanted (None: none, or it couldn't be read: then every tile is
    /// fetched).
    index: OnceLock<Option<crate::rawpack::Index>>,
    shards: Mutex<HashMap<String, Arc<OnceLock<Option<Opened>>>>>,
}

impl Layer {
    fn index(&self) -> Option<&crate::rawpack::Index> {
        self.index
            .get_or_init(|| match crate::rawpack::Index::load(&self.store) {
                Ok(ix) if ix.shards.is_empty() => None,
                Ok(ix) => Some(ix),
                Err(e) => {
                    eprintln!("pyramids: {}: {e:#} (its tiles fetched)", self.store.display());
                    None
                }
            })
            .as_ref()
    }

    /// Tile z/x/y from the store: Some(None) when the server hasn't it, None when the store hasn't
    /// it whole.
    fn get(&self, z: u8, x: u32, y: u32) -> Option<Option<Vec<u8>>> {
        let shard = crate::rawpack::shard(z, x, y);
        let cell = self.shards.lock().unwrap().entry(shard.clone()).or_default().clone();
        let opened = cell.get_or_init(|| {
            let ix = self.index()?;
            let mut v = Vec::new();
            for p in ix.of(&shard).iter().rev() {
                match crate::rawpack::entries_of(&self.store.join("packs").join(&p.name)) {
                    Ok(e) => v.push(e),
                    Err(e) => {
                        eprintln!("pyramids: {shard} of {}: {e:#} (its tiles fetched)", self.store.display());
                        return None;
                    }
                }
            }
            Some(v)
        });
        let key = tile_key(z, x, y);
        for (f, entries) in opened.as_ref()? {
            let Ok(i) = entries.binary_search_by_key(&key, |e| e.key) else { continue };
            let mut b = vec![0u8; entries[i].len as usize];
            if let Err(e) = f.read_exact_at(&mut b, entries[i].offset) {
                eprintln!("pyramids: {z}/{x}/{y} of {}: {e}", self.store.display());
                return None;
            }
            if b.is_empty() {
                return Some(None);
            }
            if crate::whole::png_whole(&b) {
                return Some(Some(b));
            }
            eprintln!("pyramids: {z}/{x}/{y} of {} isn't whole: fetched", self.store.display());
            return None;
        }
        None
    }
}

/// The fetch a terrain job reads through: the stored pyramids' tiles from the NAS (`dir`: `DIR`
/// under the root), every other file, and a tile the store lacks, from `inner`.
pub struct Stored<'a> {
    inner: &'a dyn Fetch,
    layers: Vec<Layer>,
    /// The tiles read from the store, and those it lacked (fetched).
    pub hits: AtomicU64,
    pub misses: AtomicU64,
}

impl<'a> Stored<'a> {
    /// The pyramids of `plan` read tile by tile, from their stores under `dir`.
    pub fn new(inner: &'a dyn Fetch, dir: &NasPath, plan: &Plan) -> Stored<'a> {
        let layers = of(plan)
            .into_iter()
            .map(|(template, _, _)| Layer {
                store: dir.join(folder(&template)),
                template,
                index: OnceLock::new(),
                shards: Default::default(),
            })
            .collect();
        Stored { inner, layers, hits: AtomicU64::new(0), misses: AtomicU64::new(0) }
    }

    /// `url`'s tile from its store, when it's a stored pyramid's and the store has it.
    fn stored(&self, url: &str) -> Option<Option<Vec<u8>>> {
        let (l, (z, x, y)) = self.layers.iter().find_map(|l| tile_of(&l.template, url).map(|t| (l, t)))?;
        let got = l.get(z, x, y);
        let n = if got.is_some() { &self.hits } else { &self.misses };
        n.fetch_add(1, Ordering::Relaxed);
        got
    }
}

impl Drop for Stored<'_> {
    /// Says in the run's log how many tiles the stores served and lacked.
    fn drop(&mut self) {
        let (h, m) = (self.hits.load(Ordering::Relaxed), self.misses.load(Ordering::Relaxed));
        if h + m > 0 {
            eprintln!("pyramids: {h} tiles read from the store, {m} it lacked fetched");
        }
    }
}

impl Fetch for Stored<'_> {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
        match self.stored(url) {
            Some(b) => Ok(b.map(|b| Arc::new(b) as Arc<dyn RangeRead>)),
            None => self.inner.open(url),
        }
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.stored(url) {
            Some(b) => Ok(b),
            None => self.inner.get(url),
        }
    }

    fn get_within(&self, url: &str, within: Within) -> Result<Option<Vec<u8>>> {
        match self.stored(url) {
            Some(b) => Ok(b),
            None => self.inner.get_within(url, within),
        }
    }
}

/// The tiles of plan `plan`'s pyramids (by template) a terrain-hi run reads making `levels`
/// (crate::terrain_pack::piece_levels: z9–12, the zooms the pool reads them at,
/// crate::terrain_base::ROW_Z): the native tile under each pixel that isn't open sea in `water`
/// (crate::terrain_base::open_sea, which spares those pixels the pyramids), inside the dataset's
/// cover. Its terrain-deep targets read within those (their land).
pub fn want(
    plan: &Plan,
    water: &dyn crate::terrain_water::WaterSource,
    levels: &[(u8, Vec<(u32, u32)>)],
) -> Result<BTreeMap<String, BTreeSet<(u8, u32, u32)>>> {
    use crate::terrain_base::K;
    let pyr = of(plan);
    let mut out: BTreeMap<String, BTreeSet<(u8, u32, u32)>> = BTreeMap::new();
    if pyr.is_empty() {
        return Ok(out);
    }
    let side = 256 * K;
    for (z, ts) in levels {
        let z = *z;
        if z < crate::terrain_base::ROW_Z {
            continue;
        }
        for &(x, y) in ts {
            let sea = crate::terrain_base::open_sea(water, z, &[(x, y)])?;
            for (t, zn, _) in &pyr {
                let zn = *zn as u8;
                let set = out.entry(t.clone()).or_default();
                // (Pixel (c, r)'s native tiles: the one holding it, or those under it.)
                let (down, up) = ((z + 8).saturating_sub(zn), zn.saturating_sub(z + 8));
                for r in 0..256u32 {
                    for c in 0..256u32 {
                        if !sea.is_empty() && sea[(r as usize * K) * side + c as usize * K] {
                            continue;
                        }
                        let (nx, ny) = ((((x << 8) + c) >> down) << up, (((y << 8) + r) >> down) << up);
                        for dy in 0..(1u32 << up) {
                            for dx in 0..(1u32 << up) {
                                set.insert((zn, nx + dx, ny + dy));
                            }
                        }
                    }
                }
            }
        }
    }
    for (t, _, cover) in &pyr {
        if let (Some(cover), Some(set)) = (cover, out.get_mut(t)) {
            set.retain(|&(z, x, y)| {
                let b = crate::stage::tile_box_grown(z, x, y, 0.0);
                cover.iter().any(|c| b[0] < c[2] && c[0] < b[2] && b[1] < c[3] && c[1] < b[3])
            });
        }
    }
    out.retain(|_, s| !s.is_empty());
    Ok(out)
}

/// What `fill` did, by its tiles.
#[derive(Debug, Default)]
pub struct Filled {
    /// Tiles the store had already.
    pub had: u64,
    /// Tiles fetched and packed: with data, and that the server hasn't.
    pub fetched: u64,
    pub none: u64,
    /// Their bytes as fetched.
    pub bytes: u64,
    /// Tiles that couldn't be fetched (left out: fetched by a job, or the next fill), with why.
    pub failed: Vec<String>,
}

/// Each of `want`'s tiles (by template) that its store under `dir` lacks fetched by `fetch`
/// (`FETCHES` at once) and packed there (crate::rawpack::Packer, its local work in `cache`, the
/// index changed under `root`'s build lock: the lead's step alone). Restartable: a tile packed
/// stays; one cut short is fetched again.
pub fn fill(
    fetch: &dyn Fetch,
    want: &BTreeMap<String, BTreeSet<(u8, u32, u32)>>,
    dir: &NasPath,
    cache: &std::path::Path,
    root: &store::nasfs::NasRoot,
    say: &(dyn Fn(&str) + Sync),
) -> Result<Filled> {
    use rayon::prelude::*;
    let mut done = Filled::default();
    let threads = rayon::ThreadPoolBuilder::new().num_threads(FETCHES).build()?;
    for (template, tiles) in want {
        let store = dir.join(folder(template));
        let layer = Layer {
            template: template.clone(),
            store: store.clone(),
            index: OnceLock::new(),
            shards: Default::default(),
        };
        // (In shard order, as the packer packs a shard once that way.)
        let mut todo: Vec<(u8, u32, u32)> = Vec::new();
        for &(z, x, y) in tiles {
            if layer.get(z, x, y).is_some() {
                done.had += 1;
            } else {
                todo.push((z, x, y));
            }
        }
        todo.sort_by_key(|&(z, x, y)| (crate::rawpack::shard(z, x, y), tile_key(z, x, y)));
        say(&format!("pyramids: {}: {} tiles, {} to fetch", folder(template), tiles.len(), todo.len()));
        if todo.is_empty() {
            continue;
        }
        let mut p = crate::rawpack::Packer::with_meta(&cache.join(folder(template)), &store, root, META)?;
        p.keep = false;
        let n = todo.len();
        for (k, chunk) in todo.chunks(4096).enumerate() {
            let got: Vec<Result<Option<Vec<u8>>>> = threads.install(|| {
                chunk
                    .par_iter()
                    .map(|&(z, x, y)| {
                        let url = template
                            .replace("{z}", &z.to_string())
                            .replace("{x}", &x.to_string())
                            .replace("{y}", &y.to_string());
                        let b = fetch.get_within(&url, crate::dem::generic::TILE_WITHIN)?;
                        match b {
                            Some(b) if !crate::whole::png_whole(&b) => {
                                anyhow::bail!("{url}: not a whole PNG ({} bytes)", b.len())
                            }
                            b => Ok(b),
                        }
                    })
                    .collect()
            });
            for (&(z, x, y), g) in chunk.iter().zip(got) {
                match g {
                    Ok(b) => {
                        match &b {
                            Some(b) => {
                                done.fetched += 1;
                                done.bytes += b.len() as u64;
                            }
                            None => done.none += 1,
                        }
                        p.add(z, x, y, b.as_deref())?;
                    }
                    Err(e) => done.failed.push(format!("{z}/{x}/{y}: {e:#}")),
                }
            }
            say(&format!(
                "pyramids: {}: {} of {n} ({:.1} GB fetched so far)",
                folder(template),
                ((k + 1) * 4096).min(n),
                done.bytes as f64 / 1e9
            ));
            if crate::agent::stopping() {
                break;
            }
        }
        p.finish().with_context(|| format!("pack {}", store.display()))?;
    }
    Ok(done)
}

/// The whole of each store under `dir` for templates `templates`: its archives' bytes and count.
pub fn sizes(dir: &NasPath, templates: &[String]) -> Result<Vec<(String, u64, usize)>> {
    templates
        .iter()
        .map(|t| {
            let ix = crate::rawpack::Index::load(&dir.join(folder(t)))?;
            let packs: Vec<&crate::rawpack::Pack> = ix.shards.values().flatten().collect();
            Ok((folder(t), packs.iter().map(|p| p.bytes).sum(), packs.len()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "https://cyberjapandata.gsi.go.jp/xyz/dem5a_png/{z}/{x}/{y}.png";

    #[test]
    fn a_folder_is_the_templates_host_and_path() {
        assert_eq!(folder(T), "cyberjapandata.gsi.go.jp-xyz-dem5a_png");
        assert_eq!(tile_of(T, "https://cyberjapandata.gsi.go.jp/xyz/dem5a_png/15/1/2.png"), Some((15, 1, 2)));
        assert_eq!(tile_of(T, "https://cyberjapandata.gsi.go.jp/xyz/dem5b_png/15/1/2.png"), None);
    }

    fn png() -> Vec<u8> {
        let mut b = Vec::new();
        let mut e = png::Encoder::new(&mut b, 2, 2);
        e.set_color(png::ColorType::Rgb);
        e.write_header().unwrap().write_image_data(&[7u8; 12]).unwrap();
        b
    }

    fn plan() -> Plan {
        Plan {
            fmt: 0,
            version: String::new(),
            tag: String::new(),
            datasets: vec![crate::dem::declared::Dataset {
                id: "g".into(),
                code: 5,
                resolution: 5.0,
                cover: None,
                source: Source::Tiles { url: T.into(), zoom: 15, encoding: "gsi-png".into() },
                convert: None,
                outline: None,
            }],
        }
    }

    struct Dry;
    impl crate::terrain_water::WaterSource for Dry {
        fn polys(&self, _: u8, _: u32, _: u32) -> Result<Vec<crate::terrain_water::Poly>> {
            Ok(Vec::new())
        }
        fn pin(&self) -> String {
            String::new()
        }
    }

    /// With no sea, a z12 tile reads the 8 × 8 z15 tiles under it, a z9 tile the 64 × 64; below
    /// ROW_Z none.
    #[test]
    fn a_level_wants_the_native_tiles_under_its_pixels() {
        let w = want(&plan(), &Dry, &[(12, vec![(3640, 1610)])]).unwrap();
        let s = &w[T];
        assert_eq!(s.len(), 64);
        assert!(s.contains(&(15, 3640 * 8, 1610 * 8)) && s.contains(&(15, 3640 * 8 + 7, 1610 * 8 + 7)));
        assert_eq!(want(&plan(), &Dry, &[(9, vec![(455, 201)])]).unwrap()[T].len(), 64 * 64);
        assert!(want(&plan(), &Dry, &[(8, vec![(227, 100)])]).unwrap().is_empty());
    }

    /// Filled once: every tile packed, the server's 404 as none; read back the same bytes without
    /// the network, a tile it lacks fetched; a second fill fetches nothing.
    #[test]
    fn a_filled_store_serves_its_tiles_and_fetches_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::from_arg(d.path().join("root").to_str().unwrap());
        let dir = root.join(DIR);
        let url = |x: u32| T.replace("{z}", "15").replace("{x}", &x.to_string()).replace("{y}", "9");
        let mut net = crate::fetch::MapFetch::default();
        net.0.insert(url(1), Some(png()));
        net.0.insert(url(2), None);
        let want = BTreeMap::from([(T.to_string(), BTreeSet::from([(15, 1, 9), (15, 2, 9)]))]);
        let f = fill(&net, &want, &dir, &d.path().join("cache"), &root, &|_| {}).unwrap();
        assert_eq!((f.had, f.fetched, f.none, f.failed.len()), (0, 1, 1, 0));
        let empty = crate::fetch::MapFetch::default();
        let s = Stored::new(&empty, &dir, &plan());
        assert_eq!(s.get(&url(1)).unwrap(), Some(png()));
        assert_eq!(s.get(&url(2)).unwrap(), None);
        assert!(s.get(&url(3)).is_err(), "not stored: asked of the network");
        assert_eq!((s.hits.load(Ordering::Relaxed), s.misses.load(Ordering::Relaxed)), (2, 1));
        let again = fill(&empty, &want, &dir, &d.path().join("cache"), &root, &|_| {}).unwrap();
        assert_eq!((again.had, again.fetched), (2, 0));
        assert_eq!(sizes(&dir, &[T.to_string()]).unwrap()[0].2, 1);
    }
}
