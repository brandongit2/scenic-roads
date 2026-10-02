//! Reading base packs (docs/formats.md "Base pack") and road values from local files (the build
//! Mac's cache), memory-mapped, as typed slices.

use crate::legacy::Unit;
use roadcore::packs::{RoadRec, Sub9};
use anyhow::{bail, ensure, Context, Result};
use memmap2::Mmap;
use roadcore::scenic::{ch, Sample};
use roadcore::WayRec;
use std::collections::HashMap;
use std::path::Path;

/// A memory-mapped RDSECT file.
pub struct Sect {
    map: Mmap,
    sections: HashMap<String, (usize, usize)>,
    pub meta: serde_json::Value,
}

impl Sect {
    pub fn open(path: &Path) -> Result<Sect> {
        let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: content-named files are never modified after they're written.
        let map = unsafe { Mmap::map(&f)? };
        ensure!(map.len() >= 28 && &map[..8] == b"RDSECT01", "{}: not a sectioned file", path.display());
        let version = u32::from_le_bytes(map[8..12].try_into()?);
        ensure!(version == 1, "{}: sectioned file version {version}", path.display());
        let count = u32::from_le_bytes(map[12..16].try_into()?) as usize;
        let table = u64::from_le_bytes(map[16..24].try_into()?) as usize;
        let mlen = u32::from_le_bytes(map[24..28].try_into()?) as usize;
        let meta: serde_json::Value = serde_json::from_slice(&map[28..28 + mlen])?;
        let mut sections = HashMap::new();
        for i in 0..count {
            let e = &map[table + i * 48..table + (i + 1) * 48];
            let name = String::from_utf8_lossy(&e[..24]).trim_end_matches('\0').to_string();
            let off = u64::from_le_bytes(e[24..32].try_into()?) as usize;
            let len = u64::from_le_bytes(e[32..40].try_into()?) as usize;
            ensure!(off + len <= map.len(), "{}: section {name} out of bounds", path.display());
            sections.insert(name, (off, len));
        }
        Ok(Sect { map, sections, meta })
    }

    pub fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.sections.get(name).map(|&(o, l)| &self.map[o..o + l])
    }

    pub fn slice<T: bytemuck::Pod>(&self, name: &str) -> Result<&[T]> {
        match self.bytes(name) {
            Some(b) => bytemuck::try_cast_slice(b).map_err(|e| anyhow::anyhow!("section {name}: {e}")),
            None => bail!("no section {name}"),
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }
}

/// One unit's base pack, with its road values.
pub struct BasePack {
    pub unit: Unit,
    pub sect: Sect,
    roads: Sect,
    pub strings: Vec<String>,
    /// [west, south, east, north], E7, of all its geometry.
    pub extent: [i32; 4],
    bboxes: std::sync::OnceLock<Vec<[i32; 4]>>,
}

impl BasePack {
    pub fn open(base: &Path, roads: &Path) -> Result<BasePack> {
        let sect = Sect::open(base)?;
        let roads = Sect::open(roads)?;
        let unit = sect.meta.get("unit").and_then(|v| v.as_str()).and_then(Unit::parse).context("base pack without a unit")?;
        let extent: Vec<i32> = serde_json::from_value(sect.meta.get("extent").cloned().unwrap_or_default())?;
        ensure!(extent.len() == 4, "base pack extent");
        let strings = String::from_utf8_lossy(sect.bytes("strings").unwrap_or(b"")).split('\n').map(str::to_owned).collect();
        let bp = BasePack { unit, sect, roads, strings, extent: [extent[0], extent[1], extent[2], extent[3]], bboxes: std::sync::OnceLock::new() };
        ensure!(bp.road_vals()?.len() == bp.ways()?.len(), "road values out of step with the base pack of {}", unit.slash());
        Ok(bp)
    }

    pub fn ways(&self) -> Result<&[WayRec]> {
        self.sect.slice("ways")
    }
    pub fn verts(&self) -> Result<&[[i32; 2]]> {
        self.sect.slice("verts")
    }
    pub fn elev(&self) -> Result<&[i16]> {
        self.sect.slice("elev")
    }
    pub fn grade(&self) -> Result<&[u8]> {
        self.sect.slice("grade")
    }
    pub fn src(&self) -> Result<&[u8]> {
        self.sect.slice("src")
    }
    pub fn scenic(&self) -> Option<&[[u8; ch::N]]> {
        self.sect.slice("scenic").ok()
    }
    pub fn drape(&self) -> Option<&[i16]> {
        self.sect.slice("drape").ok()
    }
    pub fn samples(&self) -> Result<&[Sample]> {
        self.sect.slice("samples")
    }
    pub fn samplech(&self) -> Result<&[[u8; ch::N]]> {
        self.sect.slice("samplech")
    }
    pub fn sub9(&self) -> Result<&[Sub9]> {
        self.sect.slice("sub9")
    }
    pub fn road_vals(&self) -> Result<&[RoadRec]> {
        self.roads.slice("roads")
    }
    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }
    /// Each way's [west, south, east, north] (E7), computed once.
    pub fn bboxes(&self) -> Result<&[[i32; 4]]> {
        if let Some(b) = self.bboxes.get() {
            return Ok(b);
        }
        let (ways, verts) = (self.ways()?, self.verts()?);
        let b: Vec<[i32; 4]> = ways
            .iter()
            .map(|w| {
                verts[self.range(w)].iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |bb, p| [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])])
            })
            .collect();
        Ok(self.bboxes.get_or_init(|| b))
    }

    /// The vertex range of way `i`.
    pub fn range(&self, w: &WayRec) -> std::ops::Range<usize> {
        w.vstart as usize..(w.vstart + w.vcount as u64) as usize
    }
}
