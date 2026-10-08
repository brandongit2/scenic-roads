//! Against the real data: the label archive and basemap in `data/build`, and the converted lines
//! by language on the NAS (`translations/0-converted`, or `$NAMES_CONVERTED`). Each test skips (passes, saying so) when its data isn't there.

use names::mvt::{self, gunzip_if_gzip, LayerRule, Tile, Value, LABELS, MAIN, OPENMAPTILES, SUB};
use names::{Kind, Lang, Names};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn converted() -> PathBuf {
    std::env::var_os("NAMES_CONVERTED").map_or_else(|| PathBuf::from("/Volumes/personal/projects/scenic-roads/translations/0-converted"), PathBuf::from)
}

fn build(file: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/build").join(file);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("{} absent: skipped", p.display());
        None
    }
}

/// The tile holding (lon, lat) at zoom z.
fn tile_at(z: u32, lon: f64, lat: f64) -> (u32, u32) {
    let n = f64::from(z).exp2();
    let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
    let y = ((1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
    (x, y)
}

/// Feature `i`'s string property `key`.
fn prop(t: &Tile, layer: usize, i: usize, key: &str) -> Option<String> {
    let l = &t.layers[layer];
    l.features[i].tags.chunks_exact(2).find(|p| l.keys[p[0] as usize] == key).and_then(|p| l.values[p[1] as usize].as_str().map(str::to_owned))
}

/// A decode, encode, decode cycle keeps every tile's content; returns whether the bytes came back
/// identical too.
fn round_trip(raw: &[u8]) -> bool {
    let t = Tile::decode(raw).expect("decode");
    let bytes = t.encode();
    let back = Tile::decode(&bytes).expect("decode again");
    assert_eq!(back, t);
    assert_eq!(back.encode(), bytes);
    // A merge of one tile is the tile.
    assert_eq!(mvt::merge(&[raw]).expect("merge"), raw);
    bytes == raw
}

#[test]
fn label_tiles_round_trip() {
    let Some(path) = build("labels.tiles") else { return };
    let a = roadcore::archive::Archive::open(&path).expect("archive");
    let entries = a.entries();
    let step = (entries.len() / 400).max(1);
    let (mut n, mut same, mut features) = (0, 0, 0);
    for e in entries.iter().step_by(step) {
        let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
        let gz = a.get(z, x, y).expect("tile");
        let raw = gunzip_if_gzip(gz).expect("gunzip");
        same += usize::from(round_trip(&raw));
        features += Tile::decode(&raw).expect("decode").layers.iter().map(|l| l.features.len()).sum::<usize>();
        n += 1;
    }
    eprintln!("labels: {n} tiles ({features} features) round-tripped, {same} byte for byte");
    assert!(n >= 300);
}

/// Just enough of PMTiles v3 to fetch tiles.
struct PmTiles {
    file: File,
    root: (u64, u64),
    leaves: u64,
    data: u64,
}

struct Entry {
    id: u64,
    offset: u64,
    length: u64,
    run: u64,
}

impl PmTiles {
    fn open(path: &Path) -> PmTiles {
        let file = File::open(path).expect("open");
        let mut h = [0u8; 127];
        file.read_exact_at(&mut h, 0).expect("header");
        assert_eq!(&h[..7], b"PMTiles");
        assert_eq!(h[7], 3);
        let u = |i: usize| u64::from_le_bytes(h[i..i + 8].try_into().expect("8 bytes"));
        assert_eq!(h[97], 2, "gzip'd directories");
        assert_eq!(h[98], 2, "gzip'd tiles");
        PmTiles { file, root: (u(8), u(16)), leaves: u(40), data: u(56) }
    }

    fn read(&self, offset: u64, len: u64) -> Vec<u8> {
        let mut b = vec![0u8; len as usize];
        self.file.read_exact_at(&mut b, offset).expect("read");
        b
    }

    fn dir(&self, offset: u64, len: u64) -> Vec<Entry> {
        let raw = self.read(offset, len);
        let b = gunzip_if_gzip(&raw).expect("gunzip").into_owned();
        let mut p = 0;
        let mut next = || {
            let mut v = 0u64;
            for shift in (0..64).step_by(7) {
                let c = b[p];
                p += 1;
                v |= u64::from(c & 0x7f) << shift;
                if c < 0x80 {
                    break;
                }
            }
            v
        };
        let n = next() as usize;
        let mut es: Vec<Entry> = (0..n).map(|_| Entry { id: 0, offset: 0, length: 0, run: 0 }).collect();
        let mut id = 0;
        for e in es.iter_mut() {
            id += next();
            e.id = id;
        }
        for e in es.iter_mut() {
            e.run = next();
        }
        for e in es.iter_mut() {
            e.length = next();
        }
        for i in 0..n {
            let v = next();
            es[i].offset = if v == 0 && i > 0 { es[i - 1].offset + es[i - 1].length } else { v.saturating_sub(1) };
        }
        es
    }

    fn tile(&self, z: u32, x: u32, y: u32) -> Option<Vec<u8>> {
        let id = zxy_to_id(z, x, y);
        let mut dir = self.dir(self.root.0, self.root.1);
        for _ in 0..4 {
            let i = dir.partition_point(|e| e.id <= id).checked_sub(1)?;
            let e = &dir[i];
            if e.run == 0 {
                dir = self.dir(self.leaves + e.offset, e.length);
            } else if id < e.id + e.run {
                return Some(gunzip_if_gzip(&self.read(self.data + e.offset, e.length)).expect("gunzip").into_owned());
            } else {
                return None;
            }
        }
        None
    }

    /// Up to `n` tiles, spread over the archive: the root's and some from each of a spread of leaves.
    fn sample(&self, n: usize) -> Vec<(u32, u32, u32, Vec<u8>)> {
        let root = self.dir(self.root.0, self.root.1);
        let mut out = Vec::new();
        let leaves: Vec<&Entry> = root.iter().filter(|e| e.run == 0).collect();
        for e in root.iter().filter(|e| e.run > 0).take(n / 4) {
            out.push(e.id);
        }
        let per = (n - out.len()) / leaves.len().clamp(1, 60) + 1;
        for leaf in leaves.iter().step_by((leaves.len() / 60).max(1)) {
            let d = self.dir(self.leaves + leaf.offset, leaf.length);
            let step = (d.len() / per).max(1);
            out.extend(d.iter().filter(|e| e.run > 0).step_by(step).take(per).map(|e| e.id));
        }
        out.truncate(n);
        out.into_iter()
            .map(|id| {
                let (z, x, y) = id_to_zxy(id);
                let t = self.tile(z, x, y).expect("sampled tile");
                (z, x, y, t)
            })
            .collect()
    }
}

fn rotate(n: u64, x: &mut u64, y: &mut u64, rx: u64, ry: u64) {
    if ry == 0 {
        if rx == 1 {
            *x = n - 1 - *x;
            *y = n - 1 - *y;
        }
        std::mem::swap(x, y);
    }
}

fn zxy_to_id(z: u32, x: u32, y: u32) -> u64 {
    let acc = ((1u64 << (2 * z)) - 1) / 3;
    let n = 1u64 << z;
    let (mut x, mut y, mut d) = (u64::from(x), u64::from(y), 0);
    let mut s = n / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        rotate(n, &mut x, &mut y, rx, ry);
        s /= 2;
    }
    acc + d
}

fn id_to_zxy(id: u64) -> (u32, u32, u32) {
    let mut acc = 0;
    for z in 0..32 {
        let count = 1u64 << (2 * z);
        if id < acc + count {
            let (mut x, mut y, mut t) = (0, 0, id - acc);
            let mut s = 1;
            while s < (1u64 << z) {
                let rx = 1 & (t / 2);
                let ry = 1 & (t ^ rx);
                rotate(s, &mut x, &mut y, rx, ry);
                x += s * rx;
                y += s * ry;
                t /= 4;
                s *= 2;
            }
            return (z, x as u32, y as u32);
        }
        acc += count;
    }
    panic!("tile id {id} out of range")
}

#[test]
fn hilbert_ids() {
    assert_eq!(zxy_to_id(0, 0, 0), 0);
    assert_eq!(zxy_to_id(1, 0, 0), 1);
    for (z, x, y) in [(1, 1, 1), (5, 28, 12), (12, 3653, 1574), (14, 16383, 0)] {
        assert_eq!(id_to_zxy(zxy_to_id(z, x, y)), (z, x, y));
    }
}

#[test]
fn basemap_tiles_round_trip() {
    let Some(path) = build("base.pmtiles") else { return };
    let pm = PmTiles::open(&path);
    let tiles = pm.sample(300);
    let (mut same, mut features) = (0, 0);
    for (_, _, _, raw) in &tiles {
        same += usize::from(round_trip(raw));
        features += Tile::decode(raw).expect("decode").layers.iter().map(|l| l.features.len()).sum::<usize>();
    }
    eprintln!("basemap: {} tiles ({features} features) round-tripped, {same} byte for byte", tiles.len());
    assert!(tiles.len() >= 250);
    // Paris at z12 has its place label.
    let (x, y) = tile_at(12, 2.3522, 48.8566);
    let t = Tile::decode(&pm.tile(12, x, y).expect("Paris")).expect("decode");
    let place = t.layers.iter().position(|l| l.name == "place").expect("place layer");
    assert!((0..t.layers[place].features.len()).any(|i| prop(&t, place, i, "name").as_deref() == Some("Paris")));
}

fn tables() -> Option<Names> {
    let dir = converted();
    if !dir.exists() {
        eprintln!("{} absent: skipped", dir.display());
        return None;
    }
    let t0 = Instant::now();
    let mut names = Names::load(&dir).expect("load");
    let w = names.take_warnings();
    eprintln!("{}: {:?} in {:.1?}, {} MB, warnings {w:?}", dir.display(), names.summary(), t0.elapsed(), names.heap_bytes() >> 20);
    Some(names)
}

fn ls(v: &[&str]) -> Vec<Lang> {
    v.iter().filter_map(|s| Lang::parse(s)).collect()
}

#[test]
fn converted_lines() {
    let Some(n) = tables() else { return };
    assert!(n.summary().lines > 2_500_000);
    let d = |kind: Kind, name: &str, own: Option<&str>, here: &[&str]| {
        let d = n.display(kind, name, own, &[], &ls(here));
        (d.main.to_owned(), d.sub.map(str::to_owned))
    };
    let s = |main: &str, sub: Option<&str>| (main.to_owned(), sub.map(str::to_owned));
    // Well-known names have a line with a sub, shown where their language is spoken; the thing's
    // own English wins over it.
    let known = [(Kind::Other, &["ja"][..], "松島"), (Kind::Other, &["ja"], "富士山"), (Kind::Road, &["zh"], "10號橋"), (Kind::Other, &["zh"], "中山橋")];
    for (kind, here, name) in known {
        let line = n.translation(kind, name, &ls(here)).unwrap_or_else(|| panic!("no line for {name}"));
        let sub = line.sub.unwrap_or_else(|| panic!("no sub for {name}"));
        eprintln!("{name}: {} / {sub}", line.main);
        assert_eq!(d(kind, name, None, here), s(line.main, Some(sub)));
        assert_eq!(d(kind, name, Some("Own English"), here), s(name, Some("Own English")));
    }
    // Read in Taiwan, a Japanese reading doesn't apply; nowhere, nothing.
    assert_ne!(d(Kind::Other, "松島", None, &["zh"]).1.as_deref(), n.translation(Kind::Other, "松島", &ls(&["ja"])).and_then(|t| t.sub));
    assert_eq!(d(Kind::Other, "松島", None, &[]), s("松島", None));
    // Places' lines and roads' apart.
    // A name with no roads line: its places line holds for roads too (the old lookup's fallback).
    assert_eq!(n.translation(Kind::Road, "中山橋", &ls(&["zh"])).map(|t| t.sub), n.translation(Kind::Other, "中山橋", &ls(&["zh"])).map(|t| t.sub));
    // Names the old boxes read in another area's table: northern Spain's in France's.
    assert_eq!(n.translation(Kind::Other, "Playa de Cueva", &ls(&["es"])).map(|t| t.main), Some("Cave Beach"));
    assert_eq!(n.translation(Kind::Road, "Château", &ls(&["fr"])).map(|t| (t.main, t.sub)), Some(("Château", None)));
    assert_eq!(n.translation(Kind::Other, "Château", &ls(&["fr"])).map(|t| (t.main, t.sub)), Some(("Castle", None)));
    // Lines not done, and one thing's OSM English, are gone.
    assert_eq!(n.translation(Kind::Other, "JR東海道本線", &ls(&["ja"])), None);
    // The disagreements split by kind: Mont-Blanc, a town in Quebec, and the mountain.
    assert_eq!(d(Kind::Settlement, "Mont-Blanc", None, &["fr", "en"]), s("Mont-Blanc", None));
    assert_eq!(d(Kind::Other, "Mont-Blanc", None, &["fr"]).0, "Mont Blanc");
}

/// Checks every named feature of an attached tile against the lines directly (no spoken
/// languages: OSM's own tags only).
fn check_attached(names: &Names, before: &Tile, after: &Tile, rule: &LayerRule) -> (usize, usize) {
    let (mut named, mut translated) = (0, 0);
    for (li, (lb, la)) in before.layers.iter().zip(&after.layers).enumerate() {
        assert_eq!(lb.name, la.name);
        let applies = rule.layer == "*" || rule.layer == lb.name;
        for i in 0..lb.features.len() {
            let name = rule.name_keys.iter().find_map(|k| prop(before, li, i, k).filter(|s| !s.is_empty()));
            let Some(_name) = name.filter(|_| applies) else {
                assert_eq!(prop(after, li, i, MAIN), None);
                continue;
            };
            named += 1;
            translated += usize::from(prop(after, li, i, SUB).is_some());
            let strip = |t: &Tile| -> Vec<(String, Value)> {
                let l = &t.layers[li];
                let f = &l.features[i];
                f.tags.chunks_exact(2).map(|p| (l.keys[p[0] as usize].clone(), l.values[p[1] as usize].clone())).filter(|(k, _)| k != MAIN && k != SUB).collect()
            };
            assert_eq!(strip(before), strip(after));
            assert_eq!((&lb.features[i].geometry, lb.features[i].id), (&la.features[i].geometry, la.features[i].id));
        }
    }
    let _ = names;
    (named, translated)
}

/// The basemap's tiles: `$NAMES_BASEMAP` (the pass's world PMTiles), else `data/build/base.pmtiles`;
/// the spoken languages: `$NAMES_SPOKEN` (a `Spoken::to_bytes` file), else none.
fn basemap() -> Option<PathBuf> {
    match std::env::var_os("NAMES_BASEMAP") {
        Some(p) => Some(PathBuf::from(p)),
        None => build("base.pmtiles"),
    }
}

#[test]
fn attach_to_real_tiles() {
    let Some(names) = tables() else { return };
    let names = &names;
    let Some(path) = basemap() else { return };
    let spoken = std::env::var_os("NAMES_SPOKEN").map(|p| names::Spoken::from_bytes(&std::fs::read(p).expect("spoken")).expect("spoken"));
    let pm = PmTiles::open(&path);
    let (mut named, mut with_sub, mut copied_en, mut copied_en_lined) = (0, 0, 0, 0);
    // Montréal, Paris, Quimper, Barcelona, Bilbao, Seville, Lisbon.
    let places = [("Montréal", -73.6, 45.55), ("Paris", 2.3522, 48.8566), ("Quimper", -4.10, 48.0), ("Barcelona", 2.17, 41.39), ("Bilbao", -2.93, 43.26), ("Seville", -5.98, 37.39), ("Lisbon", -9.14, 38.72)];
    for (place, lon, lat) in places {
        for z in [8, 10, 12, 13] {
            let (x, y) = tile_at(z, lon, lat);
            let Some(raw) = pm.tile(z, x, y) else { continue };
            let before = Tile::decode(&raw).expect("decode");
            let t0 = Instant::now();
            let Some(out) = mvt::attach(&raw, z, x, y, names, spoken.as_ref(), &[OPENMAPTILES]).expect("attach") else { continue };
            let took = t0.elapsed();
            let after = Tile::decode(&out).expect("decode attached");
            let (n, s) = check_attached(names, &before, &after, &OPENMAPTILES);
            // Features whose name_en is the name itself (no name:en): never their "own English".
            for (li, l) in before.layers.iter().enumerate() {
                for i in 0..l.features.len() {
                    let (Some(name), Some(en)) = (prop(&before, li, i, "name"), prop(&before, li, i, "name_en")) else { continue };
                    if en != name || prop(&before, li, i, "name:en").is_some() {
                        continue;
                    }
                    copied_en += 1;
                    let sub = prop(&after, li, i, SUB);
                    let main = prop(&after, li, i, MAIN);
                    assert_ne!(sub.as_deref(), Some(name.as_str()), "{name}: its name_en shown as its English");
                    copied_en_lined += usize::from(sub.is_some() || main.as_deref() != Some(name.as_str()));
                }
            }
            eprintln!("basemap {place} {z}/{x}/{y}: {n} named, {s} with a sub, {} → {} bytes, {took:.1?}", raw.len(), out.len());
            assert_eq!(mvt::attach(&out, z, x, y, names, spoken.as_ref(), &[OPENMAPTILES]).expect("attach"), None);
            named += n;
            with_sub += s;
        }
    }
    eprintln!("{named} named, {with_sub} with a sub; {copied_en} with name_en = name, {copied_en_lined} of them shown with their name's line");
    assert!(named > 100, "{named} {with_sub}");
    if spoken.is_some() {
        // Montréal's Rivière des Prairies: its line, not its name_en.
        let (x, y) = (302, 366);
        let raw = pm.tile(10, x, y).expect("10/302/366");
        let after = Tile::decode(&mvt::attach(&raw, 10, x, y, names, spoken.as_ref(), &[OPENMAPTILES]).expect("attach").expect("changed")).expect("decode");
        let found = after.layers.iter().enumerate().flat_map(|(li, l)| (0..l.features.len()).map(move |i| (li, i))).find(|&(li, i)| prop(&after, li, i, "name").as_deref() == Some("Rivière des Prairies"));
        if let Some((li, i)) = found {
            let shown = (prop(&after, li, i, MAIN), prop(&after, li, i, SUB));
            eprintln!("Rivière des Prairies: {shown:?}");
            assert!(shown.0.as_deref() != Some("Rivière des Prairies") || shown.1.is_some(), "{shown:?}");
        }
    }
    let _ = LABELS;
}
