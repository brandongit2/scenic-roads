use super::*;
use crate::catalog::{FileRef, Layer};
use crate::naming::{write_atomic, Source};
use crate::pack::PackWriter;
use crate::pmtiles::Compression;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};

/// Bytes of every file under `dir`, partial copies included: what the fake disk holds.
fn used(dir: &Path) -> u64 {
    fs::read_dir(dir).map_or(0, |rd| {
        rd.flatten()
            .map(|e| {
                let ft = e.file_type().unwrap();
                if ft.is_dir() {
                    used(&e.path())
                } else {
                    e.metadata().unwrap().len()
                }
            })
            .sum()
    })
}

/// A mirror on a fake disk whose capacity can change (the user filling or emptying it), holding
/// only the mirror's files.
fn mirror_on(root: &Path, capacity: &Arc<AtomicU64>, reserve: u64) -> Mirror {
    let (m, cap) = (root.join("mirror"), capacity.clone());
    Mirror::open(root.to_owned(), reserve).unwrap().with_free_space(move |_| Ok(cap.load(SeqCst).saturating_sub(used(&m))))
}

fn big(root: &Path) -> Mirror {
    mirror_on(root, &Arc::new(AtomicU64::new(1 << 40)), 0)
}

fn bytes(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

struct Nas {
    dir: tempfile::TempDir,
    pool: Arc<IoPool>,
}

fn nas() -> Nas {
    let dir = tempfile::tempdir().unwrap();
    let pool = IoPool::new(2, Duration::from_secs(60), dir.path().to_owned());
    Nas { dir, pool }
}

impl Nas {
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Puts a file on the "NAS" and lists it in `cat`.
    fn put(&self, cat: &mut Catalog, logical: &str, ext: &str, body: &[u8]) -> String {
        let name = write_atomic(self.root(), logical, ext, Source::Bytes(body)).unwrap();
        cat.files.insert(logical.into(), FileRef { file: name.clone(), size: body.len() as u64, fmt: 1, extra: Default::default() });
        name
    }
}

fn gz(b: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
    e.write_all(b).unwrap();
    e.finish().unwrap()
}

/// `len` bytes that don't compress, from `seed` and `tag`.
fn noise(seed: u8, tag: &str, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    blake3::Hasher::new().update(&[seed]).update(tag.as_bytes()).finalize_xof().fill(&mut out);
    out
}

/// A basemap's tiles: a few of z0–10, and some of z11–14 under the z6 tiles 32/21 and 33/21.
fn basemap_tiles(seed: u8) -> Vec<(u8, u32, u32, Vec<u8>)> {
    let mut t = Vec::new();
    for (z, x, y) in [(0, 0, 0), (6, 32, 21), (10, 512, 340)] {
        t.push((z, x, y, gz(&noise(seed, &format!("{z}/{x}/{y}"), 3000))));
    }
    for (x6, y6) in [(32u32, 21u32), (33, 21)] {
        for z in 11..=14u8 {
            let s = z - 6;
            for k in 0..4u32 {
                t.push((z, (x6 << s) + k, (y6 << s) + k, gz(&noise(seed, &format!("{z}/{k}/{x6}"), 2000))));
            }
        }
    }
    t
}

/// Two areas' files, A (the z6 tile 6/32/21) and B (6/33/21), the worldwide ones and a basemap.
/// `seed` changes their contents; `b_base` B's base pack's alone.
fn two_areas(nas: &Nas, n: u64, seed: u8, b_base: u8) -> Catalog {
    let mut c = Catalog::new(n);
    let s = |k: u8| seed.wrapping_add(k);
    let mut lay = Layer { encoding: "rt7".into(), minzoom: 4, maxzoom: 14, ..Default::default() };
    nas.put(&mut c, "global/railfreq", "bin", &bytes(s(0), 1_000));
    nas.put(&mut c, "sources/osm/2026-09-28/outlines", "sect", &bytes(s(1), 30_000));
    nas.put(&mut c, "layers/roads/root", "pack", &bytes(s(2), 10_000));
    nas.put(&mut c, "layers/roads/lo/3-4-2", "pack", &bytes(s(3), 20_000));
    nas.put(&mut c, "markdata/6-32-21", "sect", &bytes(s(4), 2_000));
    nas.put(&mut c, "ovdata/3-4-2", "sect", &bytes(s(5), 1_000));
    nas.put(&mut c, "layers/basemap/world", "pmtiles", &pieces::archive(&basemap_tiles(s(6)), Compression::Gzip));
    for (t, k) in [("6-32-21", 10u8), ("6-33-21", 20)] {
        let key = t.replace('-', "/");
        nas.put(&mut c, &format!("layers/roads/hi/{t}"), "pack", &bytes(s(k), 50_000));
        let b = if t == "6-33-21" { b_base } else { s(k + 1) };
        nas.put(&mut c, &format!("base/{t}"), "sect", &bytes(b, 9 << 20 >> 4));
        nas.put(&mut c, &format!("global/roads/{t}"), "sect", &bytes(s(k + 2), 5_000));
        nas.put(&mut c, &format!("hidata/{t}"), "sect", &bytes(s(k + 3), 10_000));
        lay.hi.insert(key.clone(), format!("layers/roads/hi/{t}"));
        c.base.insert(key.clone(), format!("base/{t}"));
        c.roads.insert(key.clone(), format!("global/roads/{t}"));
        c.hidata.insert(key, format!("hidata/{t}"));
    }
    lay.root = Some("layers/roads/root".into());
    lay.lo.insert("3/4/2".into(), "layers/roads/lo/3-4-2".into());
    c.layers.insert("roads".into(), lay);
    c.basemap = vec!["layers/basemap/world".into()];
    c.markdata.insert("6/32/21".into(), "markdata/6-32-21".into());
    c.ovdata.insert("3/4/2".into(), "ovdata/3-4-2".into());
    c.global.insert("railfreq".into(), "global/railfreq".into());
    c.global.insert("outlines".into(), "sources/osm/2026-09-28/outlines".into());
    c.validate().unwrap();
    c
}

fn basemap(cat: &Catalog) -> String {
    cat.content("layers/basemap/world").unwrap().to_string()
}

/// The World download: the essentials, then the basemap's lo piece.
fn world(cat: &Catalog) -> Vec<Item> {
    let mut v: Vec<Item> = essentials(cat).into_iter().map(|n| Item::File { size: cat.files.values().find(|f| f.file == n).unwrap().size, name: n }).collect();
    v.push(Item::Piece { archive: basemap(cat), piece: Piece::Lo });
    v
}

/// An area's download: its hi pack, base pack, road values and hi data, then its basemap piece.
fn area(cat: &Catalog, tile: &str) -> Vec<Item> {
    let mut v: Vec<Item> = ["global/roads", "hidata", "base", "layers/roads/hi"].iter().map(|d| cat.files[&format!("{d}/{tile}")].clone()).map(|f| Item::File { name: f.file, size: f.size }).collect();
    let (x, y) = tile.strip_prefix("6-").unwrap().split_once('-').unwrap();
    v.push(Item::Piece { archive: basemap(cat), piece: Piece::Tile(x.parse().unwrap(), y.parse().unwrap()) });
    v
}

fn wanted(parts: &[Vec<Item>]) -> Wanted {
    let mut w = Wanted::default();
    let mut seen = HashSet::new();
    for i in parts.iter().flatten() {
        w.push(i.clone(), &mut seen);
    }
    w
}

/// Of `cat`'s files, the logical names of those here, sorted.
fn here(m: &Mirror, cat: &Catalog) -> Vec<String> {
    cat.files.iter().filter(|(_, f)| m.has(&f.file)).map(|(l, _)| l.clone()).collect()
}

fn sync(m: &Mirror, cat: &Catalog, w: &Wanted, nas: &Nas) -> SyncStats {
    m.sync(cat, w, nas.root(), &nas.pool, &Control::FREE).unwrap()
}

#[test]
fn nothing_is_copied_that_wasnt_downloaded() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    let s = sync(&m, &cat, &Wanted::default(), &nas);
    assert_eq!((s.copied, s.pending, s.end), (0, 0, SyncEnd::Done));
    assert_eq!(m.usage(), (0, 0));
}

#[test]
fn the_world_and_an_area_are_copied_in_order_and_verified() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    let w = wanted(&[world(&cat), area(&cat, "6-32-21")]);
    // The essentials in copy order: worldwide files, root and lo packs, the per-tile records.
    let names: Vec<&str> = essentials(&cat).iter().map(|c| parse_content_name(c).unwrap().logical).map(|l| cat.files.keys().find(|k| k.as_str() == l).unwrap().as_str()).collect();
    assert_eq!(names, ["global/railfreq", "layers/roads/lo/3-4-2", "layers/roads/root", "markdata/6-32-21", "ovdata/3-4-2"]);
    let s = sync(&m, &cat, &w, &nas);
    assert_eq!((s.copied, s.failed, s.pending, s.end), (11, 0, 0, SyncEnd::Done));
    for i in &w.items {
        if let Item::File { name, .. } = i {
            assert_eq!(fs::read(m.local(name).unwrap()).unwrap(), fs::read(nas.root().join(name)).unwrap());
        }
    }
    // Not B, nor the outlines.
    assert!(!m.has(cat.content("base/6-33-21").unwrap()) && !m.has(cat.content("sources/osm/2026-09-28/outlines").unwrap()));
    // The pieces: each tile of theirs, as the archive has it; no other.
    let src = pieces::open_file(&nas.root().join(basemap(&cat))).unwrap();
    for p in [Piece::Lo, Piece::Tile(32, 21)] {
        let a = pieces::open_file(&m.piece_local(&basemap(&cat), p).unwrap()).unwrap();
        for (z, x, y, _) in basemap_tiles(6) {
            let want = if Piece::of(z, x, y) == Some(p) { src.get(z, x, y).unwrap() } else { None };
            assert_eq!(a.get(z, x, y).unwrap(), want, "{p:?} {z}/{x}/{y}");
        }
    }
    assert!(m.piece_local(&basemap(&cat), Piece::Tile(33, 21)).is_none());
    assert!(!home.path().join("mirror").join(PARTIAL).exists());
    assert_eq!(m.room(&w).unwrap().missing, 0);

    // A reopened mirror finds its files and pieces.
    drop(m);
    let m = big(home.path());
    assert!(w.items.iter().all(|i| m.is_here(i)));
    let s = sync(&m, &cat, &w, &nas);
    assert_eq!((s.copied, s.removed, s.pending), (0, 0, 0));
    assert!(m.local("../../etc/passwd").is_none());

    // Cut short while the app wasn't running: found, copied again.
    drop(m);
    let hi = cat.content("layers/roads/hi/6-32-21").unwrap();
    let p = home.path().join("mirror").join(hi);
    let body = fs::read(&p).unwrap();
    fs::write(&p, &body[..100]).unwrap();
    let m = big(home.path());
    let s = sync(&m, &cat, &w, &nas);
    assert_eq!((s.copied, s.failed, s.pending), (1, 0, 0));
    assert_eq!(fs::read(m.local(hi).unwrap()).unwrap(), body);
}

#[test]
fn what_no_download_wants_goes_and_the_rest_stays_as_it_is() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    // Everything the old mirror would have copied: both areas, the outlines, the whole basemap.
    let all: Vec<Item> = cat.files.values().map(|f| Item::File { name: f.file.clone(), size: f.size }).collect();
    sync(&m, &cat, &wanted(std::slice::from_ref(&all)), &nas);
    fs::write(home.path().join("mirror").join(OLD_USES), b"{}").unwrap();
    drop(m);
    // The switch: the World and A downloaded. B, the outlines and the whole basemap go; the
    // essentials and A's files stay (not copied again); the pieces are made.
    let m = big(home.path());
    assert!(!home.path().join("mirror").join(OLD_USES).exists(), "the old use times are dropped");
    let removed: Arc<Mutex<Vec<String>>> = Default::default();
    let r2 = removed.clone();
    m.on_remove(move |n| r2.lock().unwrap().extend(n.iter().cloned()));
    let w = wanted(&[world(&cat), area(&cat, "6-32-21")]);
    let s = sync(&m, &cat, &w, &nas);
    let gone = ["base/6-33-21", "global/roads/6-33-21", "hidata/6-33-21", "layers/basemap/world", "layers/roads/hi/6-33-21", "sources/osm/2026-09-28/outlines"];
    assert_eq!((s.removed, s.copied), (gone.len() as u32, 2), "only the two pieces are copied");
    let mut told: Vec<String> = removed.lock().unwrap().iter().map(|c| parse_content_name(c).unwrap().logical.to_string()).collect();
    told.sort();
    assert_eq!(told, gone);
    assert_eq!(here(&m, &cat).len(), cat.files.len() - gone.len());

    // A removed: its files and piece go, the World's stay.
    removed.lock().unwrap().clear();
    let w = wanted(&[world(&cat)]);
    let s = sync(&m, &cat, &w, &nas);
    assert_eq!((s.removed, s.copied), (5, 0));
    assert!(removed.lock().unwrap().contains(&format!("{BASEMAP}/{}/6-32-21.pmtiles", hash_of(&basemap(&cat)).unwrap())));
    assert!(m.piece_local(&basemap(&cat), Piece::Tile(32, 21)).is_none() && m.piece_local(&basemap(&cat), Piece::Lo).is_some());
    // Nothing downloaded: nothing here.
    sync(&m, &cat, &Wanted::default(), &nas);
    assert_eq!(m.usage(), (0, 0));
    assert!(!home.path().join("mirror").join(BASEMAP).join(hash_of(&basemap(&cat)).unwrap()).join("lo.pmtiles").exists());
}

#[test]
fn a_new_catalog_replaces_what_changed() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat1 = two_areas(&nas, 1, 0, 1);
    sync(&m, &cat1, &wanted(&[world(&cat1), area(&cat1, "6-33-21")]), &nas);
    // B's base pack and the basemap changed: the old ones go, the new come; nothing else is copied.
    let mut cat2 = two_areas(&nas, 2, 0, 99);
    nas.put(&mut cat2, "layers/basemap/world", "pmtiles", &pieces::archive(&basemap_tiles(77), Compression::Gzip));
    let s = sync(&m, &cat2, &wanted(&[world(&cat2), area(&cat2, "6-33-21")]), &nas);
    assert_eq!((s.copied, s.removed, s.pending), (3, 3, 0), "{s:?}");
    assert!(!home.path().join("mirror").join(BASEMAP).join(hash_of(&basemap(&cat1)).unwrap()).exists(), "the old basemap's pieces go, with their sizes");
    assert!(m.piece_local(&basemap(&cat2), Piece::Tile(33, 21)).is_some());
    assert!(!m.has(cat1.content("base/6-33-21").unwrap()) && m.has(cat2.content("base/6-33-21").unwrap()));
}

#[test]
fn a_download_that_doesnt_fit_waits_and_nothing_goes_for_it() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let reserve = 100_000;
    let cat = two_areas(&nas, 1, 0, 1);
    let cap = Arc::new(AtomicU64::new(1 << 40));
    let m = mirror_on(home.path(), &cap, reserve);
    let w1 = wanted(&[world(&cat)]);
    sync(&m, &cat, &w1, &nas);
    // Room for 30 kB more above the reserve: of B, its road values and hi data come, its base
    // pack (576 kB) and hi pack (50 kB) wait, and the room says how much more they need.
    cap.store(used(&home.path().join("mirror")) + reserve + 30_000, SeqCst);
    let w2 = wanted(&[world(&cat), area(&cat, "6-33-21")]);
    let before = m.room(&w2).unwrap();
    assert_eq!(before.unknown, 1, "B's piece isn't sized yet");
    let s = sync(&m, &cat, &w2, &nas);
    let piece = m.piece_size(&basemap(&cat), Piece::Tile(33, 21)).unwrap();
    assert_eq!((s.copied, s.removed, s.waiting), (2, 0, 3), "{s:?}");
    assert_eq!(s.waiting_bytes, (9 << 20 >> 4) + 50_000 + piece);
    let r = m.room(&w2).unwrap();
    assert_eq!((r.missing, r.unknown), (s.waiting_bytes, 0));
    // (About 15 kB left above the reserve, less the pieces' sizes file.)
    assert!(r.free - reserve <= 15_000 && r.free - reserve > 14_000);
    assert_eq!(r.more, r.missing - (r.free - reserve));
    assert!(w1.items.iter().all(|i| m.is_here(i)), "nothing downloaded went");
    // Room made: they come.
    cap.fetch_add(r.more, SeqCst);
    let s = sync(&m, &cat, &w2, &nas);
    assert_eq!((s.copied, s.waiting, s.pending), (3, 0, 0));
}

#[test]
fn nothing_goes_while_the_agent_here_runs_a_job() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    sync(&m, &cat, &wanted(&[world(&cat), area(&cat, "6-32-21")]), &nas);
    let n = m.usage().0;
    let s = m.sync(&cat, &wanted(&[world(&cat)]), nas.root(), &nas.pool, &Control { hold: &|| true, ..Control::FREE }).unwrap();
    assert_eq!((s.removed, m.usage().0), (0, n));
    let s = m.sync_away(&cat, &wanted(&[world(&cat)]), &|| false).unwrap();
    assert_eq!((s.removed, s.end), (5, SyncEnd::Offline), "away from the NAS too");
}

#[test]
fn while_the_build_runs_copies_keep_to_the_rate() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    let w = wanted(&[area(&cat, "6-32-21")]);
    let seen = AtomicBool::new(false);
    let slow = || {
        if m.copying().is_some_and(|c| c.slow) {
            seen.store(true, SeqCst);
        }
        true
    };
    let t = Instant::now();
    let s = m.sync(&cat, &w, nas.root(), &nas.pool, &Control { slow: &slow, ..Control::FREE }).unwrap();
    let bytes = s.copied_bytes;
    assert!(t.elapsed().as_secs_f64() >= 0.9 * bytes as f64 / SLOW_RATE as f64, "{bytes} bytes in {:?}", t.elapsed());
    assert!(seen.load(SeqCst), "the copy says it's slowed");
}

#[test]
fn the_copy_under_way_is_told() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let mut cat = two_areas(&nas, 1, 0, 1);
    let name = nas.put(&mut cat, "base/6-32-21", "sect", &bytes(3, 9 << 20));
    let w = wanted(&[vec![Item::File { name: name.clone(), size: 9 << 20 }]]);
    let seen: Mutex<Vec<u64>> = Default::default();
    let stop = || {
        if let Some(c) = m.copying().filter(|c| c.name == name) {
            seen.lock().unwrap().push(c.have);
        }
        false
    };
    m.sync(&cat, &w, nas.root(), &nas.pool, &Control { stop: &stop, ..Control::FREE }).unwrap();
    assert_eq!(*seen.lock().unwrap(), [0, CHUNK, 2 * CHUNK], "before each chunk");
    assert_eq!(m.copying(), None, "none once done");
}

#[test]
fn pause_and_resume() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let mut cat = two_areas(&nas, 1, 0, 1);
    let name = nas.put(&mut cat, "base/6-32-21", "sect", &bytes(3, 9 << 20));
    let w = wanted(&[vec![Item::File { name: name.clone(), size: 9 << 20 }], world(&cat)]);
    let part = m.partial_path(&name);
    let stop = || fs::metadata(&part).is_ok_and(|md| md.len() >= CHUNK);
    let s = m.sync(&cat, &w, nas.root(), &nas.pool, &Control { stop: &stop, ..Control::FREE }).unwrap();
    assert_eq!((s.end, s.copied), (SyncEnd::Paused, 0));
    assert_eq!(fs::metadata(&part).unwrap().len(), CHUNK);
    let s = sync(&m, &cat, &w, &nas);
    assert_eq!((s.copied, s.end, s.pending), (7, SyncEnd::Done, 0));
    assert_eq!(fs::read(m.local(&name).unwrap()).unwrap(), fs::read(nas.root().join(&name)).unwrap());
    assert!(!part.exists());
}

#[test]
fn offline_stops_the_sync() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    nas.pool.mark_offline("test");
    let s = sync(&m, &cat, &wanted(&[world(&cat)]), &nas);
    assert_eq!((s.copied, s.end, s.pending), (0, SyncEnd::Offline, 6));
}

#[test]
fn damaged_nas_files_are_not_kept() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    // Same size, different bytes: the hash check catches it.
    let hi = cat.content("layers/roads/hi/6-32-21").unwrap().to_string();
    let mut body = fs::read(nas.root().join(&hi)).unwrap();
    body[100] ^= 1;
    fs::write(nas.root().join(&hi), &body).unwrap();
    // Wrong size: caught before copying.
    let lo = cat.content("layers/roads/lo/3-4-2").unwrap().to_string();
    fs::write(nas.root().join(&lo), b"short").unwrap();
    // A damaged tile in the basemap: its piece isn't kept.
    let bm = nas.root().join(basemap(&cat));
    let src = pieces::open_file(&bm).unwrap();
    let (off, len) = src.locate(14, (32 << 8) + 1, (21 << 8) + 1).unwrap().unwrap();
    let mut b = fs::read(&bm).unwrap();
    b[(off + u64::from(len) - 6) as usize] ^= 0x10;
    fs::write(&bm, &b).unwrap();
    let s = sync(&m, &cat, &wanted(&[world(&cat), area(&cat, "6-32-21")]), &nas);
    assert_eq!((s.copied, s.failed, s.pending, s.end), (8, 3, 3, SyncEnd::Done), "{s:?}");
    assert!(m.local(&hi).is_none() && m.local(&lo).is_none() && m.piece_local(&basemap(&cat), Piece::Tile(32, 21)).is_none());
    assert!(!m.piece_part(&basemap(&cat), Piece::Tile(32, 21)).exists());
    assert!(nas.pool.is_online());
}

#[test]
fn pieces_are_sized_from_the_directory_alone() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let cat = two_areas(&nas, 1, 0, 1);
    let a = basemap(&cat);
    let pm = pieces::open_file(&nas.root().join(&a)).unwrap();
    let all = [Piece::Lo, Piece::Tile(32, 21), Piece::Tile(33, 21)];
    assert_eq!(m.size_pieces(&a, &all, &pm, &|| false).unwrap(), 3);
    assert_eq!(m.size_pieces(&a, &all, &pm, &|| false).unwrap(), 0, "kept");
    drop(m);
    let m = big(home.path());
    let sizes: Vec<u64> = all.iter().map(|&p| m.piece_size(&a, p).unwrap()).collect();
    // Made, each is the size it was said to be.
    sync(&m, &cat, &wanted(&[all.iter().map(|&p| Item::Piece { archive: a.clone(), piece: p }).collect()]), &nas);
    for (p, s) in all.iter().zip(sizes) {
        assert_eq!(fs::metadata(m.piece_local(&a, *p).unwrap()).unwrap().len(), s);
    }
}

#[test]
fn index_cache() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    let p = nas.root().join("x.pack");
    let mut w = PackWriter::create(&p, json!({"layer": "roads"}), false).unwrap();
    w.add(3, 1, 1, b"tile", 4).unwrap();
    w.finish().unwrap();
    let body = fs::read(&p).unwrap();
    let mut cat = Catalog::new(1);
    let name = nas.put(&mut cat, "layers/roads/lo/3-1-1", "pack", &body);
    let loads = AtomicUsize::new(0);
    let load = || {
        loads.fetch_add(1, SeqCst);
        PackIndex::read_from(&body)
    };
    let a = m.index(&name, load).unwrap();
    let b = m.index(&name, || panic!("must come from the cache")).unwrap();
    assert_eq!(a, b);
    assert_eq!(loads.load(SeqCst), 1);
    let idx = home.path().join("idx").join(format!("{}.idx", parse_content_name(&name).unwrap().hash16));
    fs::write(&idx, b"junk").unwrap();
    assert_eq!(m.index(&name, || PackIndex::read_from(&body)).unwrap(), a);
    assert!(m.index("not-a-content-name", || PackIndex::read_from(&body)).is_err());
    // Pack indexes no recent catalog uses are dropped after a sync.
    sync(&m, &cat, &Wanted::default(), &nas);
    assert!(idx.exists());
    sync(&m, &Catalog::new(2), &Wanted::default(), &nas);
    assert!(!idx.exists());
}

#[test]
fn saved_catalogs() {
    let nas = nas();
    let home = tempfile::tempdir().unwrap();
    let m = big(home.path());
    assert!(m.saved_catalog().unwrap().is_none());
    for n in 1..=5 {
        m.save_catalog(&two_areas(&nas, n, n as u8, 1)).unwrap();
    }
    assert_eq!(m.saved_catalog().unwrap().unwrap().n, 5);
    assert_eq!(catalog::list(&home.path().join("catalog")).unwrap(), [5, 4, 3]);
}
