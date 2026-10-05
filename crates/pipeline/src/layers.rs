//! Layers as packs (docs/formats.md "Pack"): every tile in exactly one pack — the root pack
//! (z0–2), a lo pack per z3 tile (z3–8) or a hi pack per z6 tile (z9–14) — uploaded content-named
//! under `layers/<layer>/`.

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

/// A layer's packs as the catalog lists them.
#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayerOut {
    pub encoding: String,
    pub minzoom: u8,
    pub maxzoom: u8,
    pub root: Option<String>,
    pub lo: BTreeMap<String, String>,
    pub hi: BTreeMap<String, String>,
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
            _ => {
                self.hi.insert(k, logical);
            }
        }
        self.minzoom = self.minzoom.min(zs.0);
        self.maxzoom = self.maxzoom.max(zs.1);
    }
}

/// Write one pack of tiles (key, blob, raw_len) and upload it; returns its logical name.
pub fn write_pack(out: &mut Out, layer: &str, encoding: &str, gzip: bool, scope: &str, root: (u8, u32, u32), tiles: &mut dyn Iterator<Item = (u8, u32, u32, Vec<u8>, u32)>) -> Result<Option<(String, (u8, u8))>> {
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
    out.put_file(&logical, "pack", &local)?;
    Ok(Some((logical, (zmin, zmax))))
}

/// Split a legacy tile archive into packs, keeping zooms up to `max_z`, saying how far it is as
/// `packs written` (crate::agent::jobs::report).
pub fn split_archive(out: &mut Out, arc: &Archive, layer: &str, encoding: &str, gzip: bool, max_z: u8) -> Result<LayerOut> {
    split_archive_with(out, arc, layer, encoding, gzip, max_z, &|k, n| crate::agent::jobs::report(k, n, "packs written"))
}

/// `split_archive`, telling `on` the packs written and how many there are, at most once a second.
pub fn split_archive_with(out: &mut Out, arc: &Archive, layer: &str, encoding: &str, gzip: bool, max_z: u8, on: &dyn Fn(u64, u64)) -> Result<LayerOut> {
    let mut groups: BTreeMap<(&'static str, u8, u32, u32), Vec<usize>> = BTreeMap::new();
    let entries = arc.entries();
    for (i, e) in entries.iter().enumerate() {
        let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
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
            let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
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

/// Split one legacy z11 analysis grid layer (`grid.<var>.u8` over `grid.idx`) into hi packs of
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
