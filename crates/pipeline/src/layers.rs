//! Layers as packs (docs/formats.md "Pack"): every tile in exactly one pack — the root pack
//! (z0–2), a lo pack per z3 tile (z3–8) or a hi pack per z6 tile (z9–14; the terrain's z13 and finer
//! in deep packs per z10 tile, crate::terrain_deep) — uploaded content-named
//! under `layers/<layer>/`. A small layer made whole by one job may instead be one bundle
//! (`layers/<layer>/all`, docs/formats.md "Bundles"): every tile in one pack, which the catalogue
//! names for each pack key its tiles fall in (`bundle_keys`).

use crate::out::Out;
use anyhow::Result;
use roadcore::archive::Archive;
use std::collections::BTreeMap;

/// Which pack a tile belongs to: (scope, z, x, y of the pack's root tile).
pub fn pack_of(z: u8, x: u32, y: u32) -> (&'static str, u8, u32, u32) {
    match z {
        0..=2 => ("root", 0, 0, 0),
        3..=8 => ("lo", 3, x >> (z - 3), y >> (z - 3)),
        _ => ("hi", 6, x >> (z - 6), y >> (z - 6)),
    }
}

/// A layer's packs as the catalogue lists them.
#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayerOut {
    pub encoding: String,
    pub minzoom: u8,
    pub maxzoom: u8,
    pub root: Option<String>,
    pub lo: BTreeMap<String, String>,
    pub hi: BTreeMap<String, String>,
    /// The terrain's deep packs by z10 tile (crate::terrain_deep).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deep: BTreeMap<String, String>,
    /// The native zoom of each deep pack's z10 tile (crate::terrain_deep::native_zoom).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_zoom: BTreeMap<String, u8>,
    /// A polar layer's block packs by z4 tile of its pole's grid (crate::polar).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub polar: BTreeMap<String, String>,
}

impl LayerOut {
    pub fn new(encoding: &str) -> Self {
        LayerOut { encoding: encoding.to_string(), minzoom: u8::MAX, maxzoom: 0, ..Default::default() }
    }
    /// Record a pack's logical name, and widen the zoom range by `zs`.
    pub fn add(&mut self, scope: &str, root: (u8, u32, u32), logical: String, zs: (u8, u8)) {
        let k = format!("{}/{}/{}", root.0, root.1, root.2);
        match scope {
            "root" => self.root = Some(logical),
            "lo" => {
                self.lo.insert(k, logical);
            }
            "deep" => {
                self.deep.insert(k, logical);
            }
            "polar" => {
                self.polar.insert(k, logical);
            }
            _ => {
                self.hi.insert(k, logical);
            }
        }
        self.minzoom = self.minzoom.min(zs.0);
        self.maxzoom = self.maxzoom.max(zs.1);
    }

    /// A bundle's pack keys (`bundle_keys`) named in the catalogue as one pack: `logical` for each.
    pub fn add_bundle(&mut self, logical: &str, keys: &BundleKeys) {
        if keys.root {
            self.root = Some(logical.to_string());
        }
        for k in &keys.lo {
            self.lo.insert(k.clone(), logical.to_string());
        }
        for k in &keys.hi {
            self.hi.insert(k.clone(), logical.to_string());
        }
    }
}

/// A layer bundle's logical name (docs/formats.md "Bundles").
pub fn bundle_logical(layer: &str) -> String {
    format!("layers/{layer}/all")
}

/// The pack keys a bundle's tiles fall in (`pack_of`): whether it has root tiles, and its lo
/// ("3/x/y") and hi ("6/x/y") keys.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct BundleKeys {
    pub root: bool,
    pub lo: std::collections::BTreeSet<String>,
    pub hi: std::collections::BTreeSet<String>,
}

/// `BundleKeys` of a pack's index.
pub fn bundle_keys(idx: &store::pack::PackIndex) -> BundleKeys {
    let mut k = BundleKeys::default();
    for e in &idx.entries {
        let (z, x, y) = e.zxy();
        match pack_of(z, x, y) {
            ("root", ..) => k.root = true,
            ("lo", pz, px, py) => {
                k.lo.insert(format!("{pz}/{px}/{py}"));
            }
            (_, pz, px, py) => {
                k.hi.insert(format!("{pz}/{px}/{py}"));
            }
        }
    }
    k
}

/// Writes a whole layer's tiles (z, x, y, blob, raw_len; any order) as its bundle and uploads it,
/// put off (crate::out::Out::put_file_later: named once landed); its logical name and zoom range,
/// None when there are no tiles. One file where a pack per tile key made hundreds (task #449: each
/// file made on the NAS is a second or so).
pub fn write_layer_bundle(
    out: &mut Out,
    layer: &str,
    encoding: &str,
    gzip: bool,
    mut tiles: Vec<(u8, u32, u32, Vec<u8>, u32)>,
) -> Result<Option<(String, (u8, u8))>> {
    if tiles.is_empty() {
        return Ok(None);
    }
    tiles.sort_by_key(|t| roadcore::archive::tile_key(t.0, t.1, t.2));
    let logical = bundle_logical(layer);
    let local = out.scratch_file(&format!("{logical}.pack"));
    let meta = serde_json::json!({"layer": layer, "scope": "all", "encoding": encoding});
    let mut w = store::pack::PackWriter::create(&local, meta, gzip)?;
    let (mut zmin, mut zmax) = (u8::MAX, 0u8);
    for (z, x, y, blob, raw) in tiles {
        w.add(z, x, y, &blob, raw)?;
        zmin = zmin.min(z);
        zmax = zmax.max(z);
    }
    w.finish()?;
    out.put_file_later(&logical, "pack", &local)?;
    Ok(Some((logical, (zmin, zmax))))
}

/// Writes files made per tile (z, x, y, local path; any order) as one bundle under `logical` and
/// uploads it, put off (crate::out::Out::put_file_later): an RDPACK whose blobs are the files' bytes
/// as they are, not gzip'd, each starting on a 64-byte boundary, so a sectioned file in it maps and
/// casts in place as on its own (docs/formats.md "Bundles"). The local files are removed. False
/// when there were none.
pub fn write_file_bundle(
    out: &mut Out,
    logical: &str,
    meta: serde_json::Value,
    mut files: Vec<(u8, u32, u32, std::path::PathBuf)>,
) -> Result<bool> {
    if files.is_empty() {
        return Ok(false);
    }
    files.sort_by_key(|f| roadcore::archive::tile_key(f.0, f.1, f.2));
    let local = out.scratch_file(&format!("{logical}.pack"));
    let mut w = store::pack::PackWriter::create(&local, meta, false)?.aligned(BUNDLE_ALIGN);
    for (z, x, y, p) in &files {
        let b = std::fs::read(p)?;
        w.add(*z, *x, *y, &b, b.len() as u32)?;
    }
    w.finish()?;
    for (.., p) in &files {
        std::fs::remove_file(p).ok();
    }
    out.put_file_later(logical, "pack", &local)?;
    Ok(true)
}

/// Where a file bundle's members start (RDSECT's sections are 64-byte aligned within a file).
pub const BUNDLE_ALIGN: u64 = 64;

/// Write one pack of tiles (key, blob, raw_len) and upload it; returns its logical name.
pub fn write_pack(
    out: &mut Out,
    layer: &str,
    encoding: &str,
    gzip: bool,
    scope: &str,
    root: (u8, u32, u32),
    tiles: &mut dyn Iterator<Item = (u8, u32, u32, Vec<u8>, u32)>,
) -> Result<Option<(String, (u8, u8))>> {
    let Some(p) = write_pack_local(out, layer, encoding, gzip, scope, root, tiles)? else { return Ok(None) };
    out.put_file(&p.logical, "pack", &p.local)?;
    Ok(Some((p.logical, p.zooms)))
}

/// A pack written in the scratch folder (`write_pack_local`), not yet uploaded.
pub struct LocalPack {
    pub logical: String,
    pub local: std::path::PathBuf,
    pub zooms: (u8, u8),
}

impl LocalPack {
    /// The content name it would be uploaded under (crate::out::Out::put_file).
    pub fn content_name(&self) -> Result<String> {
        Ok(store::naming::content_name(&self.logical, &store::naming::hash16_file(&self.local)?, "pack"))
    }
}

/// `write_pack`'s pack, written in the scratch folder and not uploaded; None when it has no tiles.
pub fn write_pack_local(
    out: &Out,
    layer: &str,
    encoding: &str,
    gzip: bool,
    scope: &str,
    root: (u8, u32, u32),
    tiles: &mut dyn Iterator<Item = (u8, u32, u32, Vec<u8>, u32)>,
) -> Result<Option<LocalPack>> {
    let logical = format!("layers/{layer}/{scope}/{}-{}-{}", root.0, root.1, root.2);
    let local = out.scratch_file(&format!("{logical}.pack"));
    let meta = serde_json::json!({"layer": layer, "scope": scope, "root": format!("{}/{}/{}", root.0, root.1, root.2), "encoding": encoding});
    let mut w = store::pack::PackWriter::create(&local, meta, gzip)?;
    let (mut zmin, mut zmax, mut n) = (u8::MAX, 0u8, 0usize);
    for (z, x, y, blob, raw) in tiles {
        w.add(z, x, y, &blob, raw)?;
        zmin = zmin.min(z);
        zmax = zmax.max(z);
        n += 1;
    }
    w.finish()?;
    if n == 0 {
        std::fs::remove_file(&local).ok();
        return Ok(None);
    }
    Ok(Some(LocalPack { logical, local, zooms: (zmin, zmax) }))
}

/// Split a tile archive into packs, keeping zooms up to `max_z`, saying how far it is as
/// `packs written` (crate::agent::jobs::report).
pub fn split_archive(
    out: &mut Out,
    arc: &Archive,
    layer: &str,
    encoding: &str,
    gzip: bool,
    max_z: u8,
) -> Result<LayerOut> {
    split_archive_with(out, arc, layer, encoding, gzip, max_z, &|k, n| {
        crate::agent::jobs::report(k, n, "writing packs")
    })
}

/// `split_archive`, telling `on` the packs written and how many there are, at most once a second.
pub fn split_archive_with(
    out: &mut Out,
    arc: &Archive,
    layer: &str,
    encoding: &str,
    gzip: bool,
    max_z: u8,
    on: &dyn Fn(u64, u64),
) -> Result<LayerOut> {
    let mut groups: BTreeMap<(&'static str, u8, u32, u32), Vec<usize>> = BTreeMap::new();
    let entries = arc.entries();
    for (i, e) in entries.iter().enumerate() {
        let (z, x, y) =
            ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
        if z > max_z {
            continue;
        }
        let (scope, pz, px, py) = pack_of(z, x, y);
        groups.entry((scope, pz, px, py)).or_default().push(i);
    }
    let mut lo = LayerOut::new(encoding);
    let total = groups.len();
    let mut said: Option<std::time::Instant> = None;
    for (k, ((scope, pz, px, py), idx)) in groups.into_iter().enumerate() {
        // (Each pack uploaded: labels' 1,600 take twenty minutes.)
        if said.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1)) {
            said = Some(std::time::Instant::now());
            on(k as u64, total as u64);
        }
        let mut it = idx.into_iter().map(|i| {
            let e = entries[i];
            let (z, x, y) =
                ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
            let blob = arc.get(z, x, y).map(<[u8]>::to_vec).unwrap_or_default();
            (z, x, y, blob, e.raw_len)
        });
        if let Some((logical, zs)) = write_pack(out, layer, encoding, gzip, scope, (pz, px, py), &mut it)? {
            lo.add(scope, (pz, px, py), logical, zs);
        }
        if k % 50 == 0 {
            out.save()?;
            eprintln!("{layer}: {}/{total} packs", k + 1);
        }
    }
    out.save()?;
    on(total as u64, total as u64);
    Ok(lo)
}

/// Split one z11 analysis grid layer of a unit folder (`grid.<var>.u8` over `grid.idx`) into hi packs of
/// zstd-compressed 256 × 256 u8 tiles.
pub fn split_grid(out: &mut Out, dir: &std::path::Path, var: &str) -> Result<LayerOut> {
    let idx = roadcore::grid::GridIndex::load(dir)?;
    let layer = roadcore::grid::Layer::<u8>::open(&dir.join(format!("grid.{var}.u8")))?;
    let data = layer.data();
    anyhow::ensure!(data.len() == idx.tiles.len() * roadcore::grid::CELLS, "grid.{var}.u8 out of step with grid.idx");
    let mut groups: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (s, t) in idx.tiles.iter().enumerate() {
        groups.entry((t[0] >> 5, t[1] >> 5)).or_default().push(s);
    }
    let name = format!("grid-{var}");
    let mut lo = LayerOut::new("u8-zstd");
    for ((px, py), slots) in groups {
        let mut it = slots.into_iter().map(|s| {
            let t = idx.tiles[s];
            let cells = &data[s * roadcore::grid::CELLS..(s + 1) * roadcore::grid::CELLS];
            let blob = zstd::encode_all(cells, 9).expect("zstd");
            (11u8, t[0], t[1], blob, roadcore::grid::CELLS as u32)
        });
        if let Some((logical, zs)) = write_pack(out, &name, "u8-zstd", false, "hi", (6, px, py), &mut it)? {
            lo.add("hi", (6, px, py), logical, zs);
        }
    }
    out.save()?;
    Ok(lo)
}

/// Task #449's measure, by hand on real data (release; nothing without its variables): the live build's landmark
/// and overlay outputs (`SCENIC_BUNDLE_LIVE`, its project root, only read) uploaded to a scratch
/// root on the NAS (`SCENIC_BUNDLE_SCRATCH`) as the jobs wrote them before (a file per tile, a pack
/// per key) and as bundles: files made and time, each bundle's members checked byte for byte
/// against the files they replace; and heritage-sites' slices put again unchanged, looked at on the
/// NAS (before) and not (now). Writes catalogues of each form (`old/`, `new/`) for a server to compare.
#[cfg(test)]
mod measure {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::time::Instant;
    use store::range::PlainFile;

    fn copy(live: &Path, content: &str, to: &Path) -> std::path::PathBuf {
        let p = to.join(content.replace('/', "_"));
        std::fs::copy(live.join(content), &p).unwrap();
        p
    }

    #[test]
    fn bundles_on_real_data() {
        let (Ok(live), Ok(scratch)) = (std::env::var("SCENIC_BUNDLE_LIVE"), std::env::var("SCENIC_BUNDLE_SCRATCH"))
        else {
            return;
        };
        let (live, scratch) = (Path::new(&live), Path::new(&scratch));
        assert!(!scratch.starts_with(live), "the scratch root is outside the live one");
        let m: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(live.join("state/build/manifest.json")).unwrap()).unwrap();
        let local = tempfile::tempdir().unwrap();
        let roots = [scratch.join("old"), scratch.join("new")];
        let outs: Vec<Out> = roots
            .iter()
            .enumerate()
            .map(|(i, r)| {
                std::fs::create_dir_all(r.join("state/build")).unwrap();
                Out::unbound(&store::nasfs::NasRoot::local_for_test(r), &local.path().join(format!("s{i}"))).unwrap()
            })
            .collect();
        let [mut old, mut new]: [Out; 2] = outs.try_into().ok().unwrap();
        let layers: Vec<String> = {
            let mut v: Vec<String> = m
                .keys()
                .filter_map(|l| l.strip_prefix("layers/"))
                .filter_map(|l| l.split('/').next())
                .filter(|l| l.starts_with("marks-") || l.starts_with("ov-"))
                .map(str::to_string)
                .collect();
            v.dedup();
            v
        };
        let packs = |layer: &str| -> Vec<(&String, &String)> {
            m.iter().filter(|(l, _)| l.starts_with(&format!("layers/{layer}/")) && !l.ends_with("/all")).collect()
        };
        let markdata: Vec<(&String, &String)> = m.iter().filter(|(l, _)| l.starts_with("markdata/6-")).collect();
        eprintln!("{} markdata files, layers {layers:?}", markdata.len());

        // Before: a file per tile, a pack per key, each uploaded as the jobs did (put_file).
        let dl = local.path().join("live");
        std::fs::create_dir_all(&dl).unwrap();
        let mut olds: Vec<(String, std::path::PathBuf)> = Vec::new();
        for (l, c) in markdata.iter().copied().chain(layers.iter().flat_map(|ly| packs(ly))) {
            olds.push((l.clone(), copy(live, c, &dl)));
        }
        let bytes: u64 = olds.iter().map(|(_, p)| std::fs::metadata(p).unwrap().len()).sum();
        let t = Instant::now();
        let mut md_old = std::time::Duration::ZERO;
        for (l, p) in &olds {
            let t1 = Instant::now();
            let ext = if l.starts_with("markdata/") { "sect" } else { "pack" };
            let up = dl.join(format!("{}.up", l.replace('/', "_")));
            std::fs::copy(p, &up).unwrap();
            old.put_file(l, ext, &up).unwrap();
            if l.starts_with("markdata/") {
                md_old += t1.elapsed();
            }
        }
        old.save().unwrap();
        let old_s = t.elapsed();
        eprintln!(
            "before: {} files made ({} MB) in {:.1?} (markdata {} files {md_old:.1?}, packs {} files {:.1?})",
            olds.len(),
            bytes >> 20,
            old_s,
            markdata.len(),
            olds.len() - markdata.len(),
            old_s - md_old
        );

        // Now: the bundles, made as the jobs make them.
        let t = Instant::now();
        let files: Vec<(u8, u32, u32, std::path::PathBuf)> = olds
            .iter()
            .filter(|(l, _)| l.starts_with("markdata/"))
            .map(|(l, p)| {
                let u = crate::legacy::TileId::parse(l.trim_start_matches("markdata/")).unwrap();
                let to = dl.join(format!("{}.member", l.replace('/', "_")));
                std::fs::copy(p, &to).unwrap();
                (6, u.x, u.y, to)
            })
            .collect();
        let md_made = Instant::now();
        crate::layers::write_file_bundle(
            &mut new,
            crate::marks::MARKDATA_BUNDLE,
            serde_json::json!({"bundle": "markdata", "fmt": 1}),
            files,
        )
        .unwrap();
        let md_make = md_made.elapsed();
        let mut ntiles = 0;
        for ly in &layers {
            let mut tiles = Vec::new();
            let mut enc = String::new();
            for (l, _) in packs(ly) {
                let p = &olds.iter().find(|(o, _)| o == l).unwrap().1;
                let f = PlainFile::open(p).unwrap();
                let idx = store::pack::PackIndex::read_from(&f).unwrap();
                enc = idx.meta["encoding"].as_str().unwrap_or_default().to_string();
                for e in &idx.entries {
                    let (z, x, y) = e.zxy();
                    tiles.push((z, x, y, idx.read_blob(&f, e).unwrap(), e.raw_len));
                }
            }
            ntiles += tiles.len();
            write_layer_bundle(&mut new, ly, &enc, true, tiles).unwrap();
        }
        new.save().unwrap();
        let new_s = t.elapsed();
        eprintln!(
            "now: {} files made in {new_s:.1?} (the markdata bundle made in {md_make:.1?}; {ntiles} tiles in {} layer bundles)",
            1 + layers.len(),
            layers.len()
        );

        // The members, byte for byte: each tile's markdata, each layer's tiles and their entries.
        let nm = &new.manifest;
        let bundle = PlainFile::open(&roots[1].join(&nm[crate::marks::MARKDATA_BUNDLE])).unwrap();
        let bidx = store::pack::PackIndex::read_from(&bundle).unwrap();
        assert_eq!(bidx.entries.len(), markdata.len());
        for (l, p) in olds.iter().filter(|(l, _)| l.starts_with("markdata/")) {
            let u = crate::legacy::TileId::parse(l.trim_start_matches("markdata/")).unwrap();
            let e = bidx.find(6, u.x, u.y).unwrap();
            assert_eq!(bidx.read_blob(&bundle, &e).unwrap(), std::fs::read(p).unwrap(), "{l}");
            assert_eq!(e.offset % BUNDLE_ALIGN, 0);
        }
        for ly in &layers {
            let b = PlainFile::open(&roots[1].join(&nm[&bundle_logical(ly)])).unwrap();
            let bi = store::pack::PackIndex::read_from(&b).unwrap();
            let mut n = 0;
            for (l, _) in packs(ly) {
                let p = &olds.iter().find(|(o, _)| o == l).unwrap().1;
                let f = PlainFile::open(p).unwrap();
                let idx = store::pack::PackIndex::read_from(&f).unwrap();
                for e in &idx.entries {
                    let (z, x, y) = e.zxy();
                    let be = bi.find(z, x, y).unwrap();
                    assert_eq!((be.len, be.raw_len, be.hash), (e.len, e.raw_len, e.hash), "{l} {z}/{x}/{y}");
                    assert_eq!(bi.read_blob(&b, &be).unwrap(), idx.read_blob(&f, e).unwrap());
                    n += 1;
                }
            }
            assert_eq!(n, bi.entries.len(), "{ly}: no other tiles");
        }
        eprintln!("every member byte for byte as before");

        // Catalogues of each form, with the overlays' details and the summaries, for a server.
        let extra: Vec<(&String, &String)> = m
            .iter()
            .filter(|(l, _)| {
                l.starts_with("ovdata/") || *l == "global/marks/summary" || l.starts_with("global/heritage/")
            })
            .collect();
        for (i, (root, out)) in roots.iter().zip([&mut old, &mut new]).enumerate() {
            for (l, c) in &extra {
                let ext = c.rsplit('.').next().unwrap();
                let p = copy(live, c, &local.path().join(format!("s{i}")));
                out.put_file(l, ext, &p).unwrap();
            }
            out.save().unwrap();
            let mut cat = store::catalogue::Catalogue::new(1);
            let mut layers_out: BTreeMap<String, LayerOut> = BTreeMap::new();
            for (l, c) in &out.manifest {
                let size = std::fs::metadata(root.join(c)).unwrap().len();
                cat.files.insert(
                    l.clone(),
                    store::catalogue::FileRef { file: c.clone(), size, fmt: 1, ..Default::default() },
                );
                let parts: Vec<&str> = l.split('/').collect();
                match parts.as_slice() {
                    ["layers", ly, "all"] => {
                        let f = PlainFile::open(&root.join(c)).unwrap();
                        let keys = bundle_keys(&store::pack::PackIndex::read_from(&f).unwrap());
                        let enc = if ly.starts_with("marks-") { "rdmt" } else { "mvt" };
                        layers_out.entry(ly.to_string()).or_insert_with(|| LayerOut::new(enc)).add_bundle(l, &keys);
                    }
                    ["layers", ly, scope, k] => {
                        let u = crate::legacy::TileId::parse(k).unwrap();
                        let enc = if ly.starts_with("marks-") { "rdmt" } else { "mvt" };
                        layers_out.entry(ly.to_string()).or_insert_with(|| LayerOut::new(enc)).add(
                            scope,
                            (u.z, u.x, u.y),
                            l.clone(),
                            (0, 0),
                        );
                    }
                    ["markdata", "all"] => {
                        for e in &bidx.entries {
                            let (z, x, y) = e.zxy();
                            cat.markdata.insert(format!("{z}/{x}/{y}"), l.clone());
                        }
                    }
                    ["markdata", t] => {
                        cat.markdata.insert(crate::legacy::TileId::parse(t).unwrap().slash(), l.clone());
                    }
                    ["ovdata", t] => {
                        cat.ovdata.insert(crate::legacy::TileId::parse(t).unwrap().slash(), l.clone());
                    }
                    ["global", g @ ..] => {
                        cat.global.insert(g.join("/"), l.clone());
                    }
                    _ => {}
                }
            }
            for (name, mut lo) in layers_out {
                (lo.minzoom, lo.maxzoom) = if name.starts_with("marks-") { (0, 5) } else { (0, crate::ovconv::MAXZ) };
                cat.layers.insert(name, serde_json::from_value(serde_json::to_value(&lo).unwrap()).unwrap());
            }
            cat.validate().unwrap();
            std::fs::remove_dir_all(root.join("catalog")).ok();
            store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        }

        // Heritage-sites' slices put again unchanged (a sample): looked at on the NAS, as before
        // (the name dropped first, so the records don't name it), and not, as now.
        let her = scratch.join("heritage");
        std::fs::create_dir_all(her.join("state/build")).unwrap();
        let slices: Vec<(&String, &String)> = m
            .iter()
            .filter(|(l, _)| l.starts_with("work/heritage/") && (l.contains("/pos/") || l.contains("/parts/")))
            .take(150)
            .collect();
        let sd = local.path().join("her");
        std::fs::create_dir_all(&sd).unwrap();
        let read: Vec<(String, Vec<u8>)> =
            slices.iter().map(|(l, c)| (l.to_string(), std::fs::read(live.join(c)).unwrap())).collect();
        let mut o = Out::unbound(&store::nasfs::NasRoot::local_for_test(&her), &sd).unwrap();
        let t = Instant::now();
        for (l, b) in &read {
            o.put_bytes(l, "json", b).unwrap();
        }
        o.save().unwrap();
        let first = t.elapsed();
        let mut o = Out::unbound(&store::nasfs::NasRoot::local_for_test(&her), &sd).unwrap();
        let t = Instant::now();
        for (l, b) in &read {
            o.remove(l);
            o.put_bytes(l, "json", b).unwrap();
        }
        let before = t.elapsed();
        let mut o = Out::unbound(&store::nasfs::NasRoot::local_for_test(&her), &sd).unwrap();
        let t = Instant::now();
        for (l, b) in &read {
            o.put_bytes(l, "json", b).unwrap();
        }
        let now = t.elapsed();
        eprintln!(
            "heritage slices ({} files, {} kB): first upload {first:.1?}; put again unchanged: looked at {before:.1?}, not looked at {now:.1?}",
            read.len(),
            read.iter().map(|r| r.1.len()).sum::<usize>() >> 10
        );
    }
}
