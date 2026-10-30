//! The elevation pyramids published at one zoom and read tile by tile (GSI's five layers:
//! crate::terrain_base::by_tile), kept on the NAS so a terrain job reads each tile there, not over
//! the network (task #806: GSI's tiles were 41 % of terrain-hi's rebuild time, every run fetching
//! them all again, its blend and terrain-deep again on top).
//!
//! Each pyramid's tiles are packed as AWS's raw tiles are (crate::rawpack: content-named archives by
//! shard, a z15 tile's its z6 tile's, an index read once and each tile read by byte range; an empty
//! entry for a tile the server hasn't), in `sources/pyramids/<folder>/` (`folder`: the URL template's
//! host and path, so a layer moved to another URL is stored afresh). Filled by the pool's
//! `pyramids-fill` step, the lead's (task #810), by z6 tile (`fill`): a z6 tile's tiles (`want`,
//! those its terrain-hi run reads: the native tiles under the pixels of its z9–12 tiles that aren't
//! open sea; and its terrain-deep targets', all under their base tiles, `deep_bases`), fetched once
//! each with the build's fetcher (its User-Agent, its per-host limit), then packed and their
//! archives handed off as the store's (crate::rawpack::handed_place), the lead naming them in the
//! store's index as it names AWS's; each z6 tile starts only while the NAS has more room past the
//! owner's reserve than the most one has put there. Its key (crate::agent::build::pyramids_key)
//! follows what `want` reads, so a z6 tile is filled again when that changes (`V` with its code).
//! Jobs only read the store (`Stored`): a tile it lacks is fetched as before, never written back.

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

/// The fill's version, in its key (crate::agent::build::pyramids_key): bumped with any change to
/// which tiles a z6 tile wants (`want`, `deep_bases`) or how they're stored.
pub const V: u32 = 1;

/// The archives' metadata (crate::rawpack::Packer::for_pyramid).
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

/// The tiles of plan `plan`'s pyramids (by template) a z6 tile's terrain runs read, inside each
/// dataset's cover: terrain-hi's making `levels` (crate::terrain_pack::piece_levels: z9–12, the
/// zooms the pool reads them at, crate::terrain_base::ROW_Z), the native tile under each pixel that
/// isn't open sea in `water` (crate::terrain_base::open_sea, which spares those pixels the
/// pyramids); and terrain-deep's, every native tile under its base tiles `deep`
/// (crate::terrain_deep::base_want under each target): its presence grids read the whole of each,
/// the sea too (the 404s over the sea most of what a deep run asked for that the store lacked).
pub fn want(
    plan: &Plan,
    water: &dyn crate::terrain_water::WaterSource,
    levels: &[(u8, Vec<(u32, u32)>)],
    deep: &[(u8, u32, u32)],
) -> Result<BTreeMap<String, BTreeSet<(u8, u32, u32)>>> {
    use crate::terrain_base::K;
    let pyr = of(plan);
    let mut out: BTreeMap<String, BTreeSet<(u8, u32, u32)>> = BTreeMap::new();
    if pyr.is_empty() {
        return Ok(out);
    }
    let side = 256 * K;
    // (Tile z/x/y's pixels' native tiles, those `sea` (open_sea's mask, empty: none) spares left out:
    // the one holding each pixel, or those under it.)
    let mut add = |z: u8, x: u32, y: u32, sea: &[bool]| {
        for (t, zn, _) in &pyr {
            let zn = *zn as u8;
            let set = out.entry(t.clone()).or_default();
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
    };
    for (z, ts) in levels {
        let z = *z;
        if z < crate::terrain_base::ROW_Z {
            continue;
        }
        for &(x, y) in ts {
            add(z, x, y, &crate::terrain_base::open_sea(water, z, &[(x, y)])?);
        }
    }
    for &(z, x, y) in deep {
        add(z, x, y, &[]);
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

/// Z6 tile `t`'s terrain-deep targets' base tiles (crate::terrain_deep::targets_in, base_want), each
/// under its own target: what `want` takes as `deep`.
pub fn deep_bases(
    sets: &[crate::terrain_deep::FineSet],
    cov: &crate::coverage::Coverage,
    t: (u32, u32),
) -> Vec<(u8, u32, u32)> {
    use crate::terrain_deep as td;
    td::targets_in(sets, cov, t)
        .into_iter()
        .flat_map(|g| {
            td::base_want(g)
                .into_iter()
                .filter(move |&(z, x, y)| z >= td::PACK_Z && (x >> (z - td::PACK_Z), y >> (z - td::PACK_Z)) == g)
        })
        .collect()
}

/// The tiles a fill fetches at once (held in memory) before packing them: a progress line each.
pub const CHUNK: usize = 4096;

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
    /// Tiles that couldn't be fetched (left out: fetched by a job, or the z6 tile's next fill), with why.
    pub failed: Vec<String>,
    /// Cut short by the build pausing (crate::control::draining): what was fetched is packed.
    pub cut: bool,
}

/// Each of `want`'s tiles (by template) that its store under `dir` lacks fetched by `fetch`
/// (`FETCHES` at once) and packed there (crate::rawpack::Packer::for_pyramid, its local work in
/// `work`): a job's archives handed off to `handoff` (`SCENIC_HANDOFF`) for the lead to name, else
/// the index changed here, under `root`'s build lock. Restartable: a tile packed stays; one cut short is
/// fetched again. `progress` is told the tiles asked of the network so far of all those the layers'
/// stores lack (at the start, then after each `CHUNK`: a z6 tile's hours-long fill says how far it
/// is every minute or two, nothing written a tile). With the build pausing (`draining`:
/// crate::control::draining_lately, a test's own), each tile not yet asked is left (task #885: the
/// stop waits for the fetches under way, seconds, not the rest of a chunk), what was fetched packed
/// and the fill `cut`.
#[allow(clippy::too_many_arguments)]
pub fn fill(
    fetch: &dyn Fetch,
    want: &BTreeMap<String, BTreeSet<(u8, u32, u32)>>,
    dir: &NasPath,
    work: &std::path::Path,
    root: &store::nasfs::NasRoot,
    handoff: Option<&std::path::Path>,
    say: &(dyn Fn(&str) + Sync),
    progress: &(dyn Fn(u64, u64) + Sync),
    draining: &(dyn Fn() -> bool + Sync),
) -> Result<Filled> {
    use rayon::prelude::*;
    let mut done = Filled::default();
    let threads = rayon::ThreadPoolBuilder::new().num_threads(FETCHES).build()?;
    // (Each layer's tiles its store lacks, worked out first: the whole fill's count for `progress`.)
    let mut todos: Vec<(&String, Vec<(u8, u32, u32)>)> = Vec::new();
    for (template, tiles) in want {
        let layer = Layer {
            template: template.clone(),
            store: dir.join(folder(template)),
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
        todos.push((template, todo));
    }
    let total: u64 = todos.iter().map(|t| t.1.len() as u64).sum();
    let mut asked = 0u64;
    progress(0, total);
    for (template, todo) in todos {
        if todo.is_empty() {
            continue;
        }
        let store = dir.join(folder(template));
        let f = folder(template);
        let mut p = crate::rawpack::Packer::for_pyramid(&work.join(&f), &store, root, META, &f, handoff)?;
        p.keep = false;
        let n = todo.len();
        for (k, chunk) in todo.chunks(CHUNK).enumerate() {
            // (None: left, the build pausing.)
            #[allow(clippy::type_complexity)]
            let got: Vec<Option<Result<Option<Vec<u8>>>>> = threads.install(|| {
                chunk
                    .par_iter()
                    .map(|&(z, x, y)| {
                        if draining() {
                            return None;
                        }
                        Some((|| {
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
                        })())
                    })
                    .collect()
            });
            // (The tiles of the chunk asked: all of them, unless the build paused it.)
            let asked_now = got.iter().filter(|g| g.is_some()).count() as u64;
            for (&(z, x, y), g) in chunk.iter().zip(got) {
                let Some(g) = g else {
                    done.cut = true;
                    continue;
                };
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
                k * CHUNK + asked_now as usize,
                done.bytes as f64 / 1e9
            ));
            asked += asked_now;
            progress(asked, total);
            // (The build pausing: what's fetched is packed, the rest left for the next fill.)
            if done.cut || draining() {
                done.cut = true;
                break;
            }
        }
        p.finish().with_context(|| format!("pack {}", store.display()))?;
        if done.cut {
            break;
        }
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

    /// Sea over every tile (more than the tile, so its neighbours' edges are sea too).
    struct Sea;
    impl crate::terrain_water::WaterSource for Sea {
        fn polys(&self, _: u8, _: u32, _: u32) -> Result<Vec<crate::terrain_water::Poly>> {
            let r = vec![[-64.0, -64.0], [320.0, -64.0], [320.0, 320.0], [-64.0, 320.0]];
            Ok(vec![crate::terrain_water::Poly { kind: crate::terrain_water::Kind::Sea, id: 0, rings: vec![r] }])
        }
        fn pin(&self) -> String {
            String::new()
        }
    }

    /// With no sea, a z12 tile reads the 8 × 8 z15 tiles under it, a z9 tile the 64 × 64; below
    /// ROW_Z none. A deep target's base tile reads the whole of it, sea or not; terrain-hi's spares the
    /// open sea.
    #[test]
    fn a_level_wants_the_native_tiles_under_its_pixels() {
        let w = want(&plan(), &Dry, &[(12, vec![(3640, 1610)])], &[]).unwrap();
        let s = &w[T];
        assert_eq!(s.len(), 64);
        assert!(s.contains(&(15, 3640 * 8, 1610 * 8)) && s.contains(&(15, 3640 * 8 + 7, 1610 * 8 + 7)));
        assert_eq!(want(&plan(), &Dry, &[(9, vec![(455, 201)])], &[]).unwrap()[T].len(), 64 * 64);
        assert!(want(&plan(), &Dry, &[(8, vec![(227, 100)])], &[]).unwrap().is_empty());
        // A deep target's base tile: the 8 × 8 under it, whatever the water.
        assert_eq!(want(&plan(), &Sea, &[], &[(12, 3640, 1610)]).unwrap()[T].len(), 64);
        assert!(want(&plan(), &Sea, &[(12, vec![(3640, 1610)])], &[]).unwrap().is_empty(), "open sea spared");
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
        // (Task #830: told how far it is, of the tiles the store lacks: at the start and by chunk.)
        let said = std::sync::Mutex::new(Vec::new());
        let f = fill(
            &net,
            &want,
            &dir,
            &d.path().join("cache"),
            &root,
            None,
            &|_| {},
            &|d, t| said.lock().unwrap().push((d, t)),
            &|| false,
        )
        .unwrap();
        assert_eq!((f.had, f.fetched, f.none, f.failed.len()), (0, 1, 1, 0));
        assert_eq!(*said.lock().unwrap(), [(0, 2), (2, 2)]);
        let empty = crate::fetch::MapFetch::default();
        let s = Stored::new(&empty, &dir, &plan());
        assert_eq!(s.get(&url(1)).unwrap(), Some(png()));
        assert_eq!(s.get(&url(2)).unwrap(), None);
        assert!(s.get(&url(3)).is_err(), "not stored: asked of the network");
        assert_eq!((s.hits.load(Ordering::Relaxed), s.misses.load(Ordering::Relaxed)), (2, 1));
        said.lock().unwrap().clear();
        let again = fill(
            &empty,
            &want,
            &dir,
            &d.path().join("cache"),
            &root,
            None,
            &|_| {},
            &|d, t| said.lock().unwrap().push((d, t)),
            &|| false,
        )
        .unwrap();
        assert_eq!((again.had, again.fetched), (2, 0));
        assert_eq!(*said.lock().unwrap(), [(0, 0)], "nothing to fetch");
        assert_eq!(sizes(&dir, &[T.to_string()]).unwrap()[0].2, 1);
    }

    /// Task #885: a fill paused at every one of its safe points in turn (before each tile it asks),
    /// then filled again until it isn't cut, serves every tile with the same bytes as a fill never
    /// paused; a paused fill leaves the tiles it didn't ask, never one half-packed.
    #[test]
    fn a_fill_paused_at_every_tile_resumes_to_the_same_tiles() {
        let url = |x: u32| T.replace("{z}", "15").replace("{x}", &x.to_string()).replace("{y}", "9");
        let mut net = crate::fetch::MapFetch::default();
        let n = 6u32;
        for x in 0..n {
            net.0.insert(url(x), if x % 3 == 2 { None } else { Some(png()) });
        }
        let want = BTreeMap::from([(T.to_string(), (0..n).map(|x| (15, x, 9)).collect::<BTreeSet<_>>())]);
        let empty = crate::fetch::MapFetch::default();
        let served = |dir: &NasPath| -> Vec<Option<Vec<u8>>> {
            let s = Stored::new(&empty, dir, &plan());
            (0..n).map(|x| s.get(&url(x)).unwrap()).collect()
        };
        let d = tempfile::tempdir().unwrap();
        let fresh_root = store::nasfs::NasRoot::from_arg(d.path().join("fresh").to_str().unwrap());
        let f = fill(
            &net,
            &want,
            &fresh_root.join(DIR),
            &d.path().join("c0"),
            &fresh_root,
            None,
            &|_| {},
            &|_, _| {},
            &|| false,
        )
        .unwrap();
        assert!(!f.cut);
        let fresh = served(&fresh_root.join(DIR));
        for at in 0..n as u64 {
            let root = store::nasfs::NasRoot::from_arg(d.path().join(format!("at-{at}")).to_str().unwrap());
            let dir = root.join(DIR);
            // (Pausing once `at` tiles were asked: which those are, the fetches' threads choose.)
            let asked = AtomicU64::new(0);
            let pausing = || asked.fetch_add(1, Ordering::SeqCst) >= at;
            let f =
                fill(&net, &want, &dir, &d.path().join(format!("c-{at}")), &root, None, &|_| {}, &|_, _| {}, &pausing)
                    .unwrap();
            assert_eq!(f.fetched + f.none, at, "paused before tile {at}: the rest left");
            assert!(f.cut, "paused before tile {at}");
            let mut fills = 1;
            loop {
                let f = fill(
                    &net,
                    &want,
                    &dir,
                    &d.path().join(format!("c-{at}")),
                    &root,
                    None,
                    &|_| {},
                    &|_, _| {},
                    &|| false,
                )
                .unwrap();
                fills += 1;
                if !f.cut {
                    break;
                }
            }
            assert_eq!(fills, 2, "one fill more finishes it");
            assert_eq!(served(&dir), fresh, "paused before tile {at}");
        }
    }

    /// Task #810: a job's fill (the pool's step) changes no index: its archives go onto the NAS and
    /// into its hand-off as its store's (`<folder>/<shard>`), which the lead's merge checks as any
    /// raw tiles' archive and names in that store's index, not AWS's; the store then serves them.
    #[test]
    fn a_jobs_fill_is_handed_off_and_named_in_its_stores_index() {
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::from_arg(d.path().join("root").to_str().unwrap());
        let dir = root.join(DIR);
        let outbox = d.path().join("outbox");
        let url = T.replace("{z}", "15").replace("{x}", "1").replace("{y}", "9");
        let mut net = crate::fetch::MapFetch::default();
        net.0.insert(url.clone(), Some(png()));
        let want = BTreeMap::from([(T.to_string(), BTreeSet::from([(15, 1, 9)]))]);
        let f = fill(&net, &want, &dir, &d.path().join("cache"), &root, Some(&outbox), &|_| {}, &|_, _| {}, &|| false)
            .unwrap();
        assert_eq!((f.fetched, f.cut), (1, false));
        let empty = crate::fetch::MapFetch::default();
        assert!(Stored::new(&empty, &dir, &plan()).get(&url).is_err(), "unnamed: not served yet");
        let raw: Vec<(String, crate::rawpack::Pack)> =
            crate::handoff::written_in(&outbox).unwrap().unwrap().into_iter().flat_map(|h| h.raw).collect();
        let place = format!("{}/6-0-0", folder(T));
        assert_eq!(raw.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), [place.as_str()]);
        assert!(crate::rawpack::is_shard(&raw[0].0) && crate::rawpack::named_for(&raw[0].1.name, &raw[0].0));
        assert!(!crate::rawpack::is_shard("../x/6-0-0") && !crate::rawpack::is_shard("a/b/6-0-0"));
        let lock = crate::out::BuildLock::take(&root).unwrap();
        let (settled, new) = crate::rawpack::name_handed_places(&root, &raw, &lock);
        assert_eq!((settled, new), (vec![raw[0].1.name.clone()], 1));
        assert!(crate::rawpack::Index::load(&root.join("sources/aws-terrarium")).unwrap().shards.is_empty());
        assert_eq!(Stored::new(&empty, &dir, &plan()).get(&url).unwrap(), Some(png()));
    }
}
