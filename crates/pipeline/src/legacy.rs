//! A unit folder's arrays (ways.bin, verts.bin and the per-vertex and per-sample arrays the unit's
//! steps write) as its base pack (docs/formats.md "Base pack"); and the units' tile (`Unit`).

use anyhow::{ensure, Result};
use rayon::prelude::*;
use roadcore::elev::{self, Elevs};
use roadcore::scenic::{ch, Sample};
use roadcore::{class, merc, Array, Ways, WayRec};
use std::collections::BTreeMap;
use std::path::Path;

/// A unit's tile (z6), or any z/x/y tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unit {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl Unit {
    pub fn key(&self) -> u64 {
        roadcore::archive::tile_key(self.z, self.x, self.y)
    }
    /// "6/32/21".
    pub fn slash(&self) -> String {
        format!("{}/{}/{}", self.z, self.x, self.y)
    }
    /// "6-32-21", for file names.
    pub fn dash(&self) -> String {
        format!("{}-{}-{}", self.z, self.x, self.y)
    }
    pub fn of_point(z: u8, p: [i32; 2]) -> Unit {
        let (x, y) = merc(p[0] as f64 * roadcore::E7, p[1] as f64 * roadcore::E7);
        let n = (1u64 << z) as f64;
        let c = |v: f64| ((v * n).floor().max(0.0) as u32).min((1u32 << z) - 1);
        Unit { z, x: c(x), y: c(y) }
    }
    pub fn parse(s: &str) -> Option<Unit> {
        let mut it = s.split(['/', '-']).map(|t| t.parse::<u32>().ok());
        let (z, x, y) = (it.next()??, it.next()??, it.next()??);
        Some(Unit { z: z as u8, x, y })
    }
}

/// A unit folder's arrays, memory-mapped.
pub struct Legacy {
    pub ways: Ways,
    pub strings: Vec<String>,
    pub elev: elev::Stored,
    pub raw: Array<f32>,
    pub grade: Array<u8>,
    pub src: Array<u8>,
    pub scenic: Option<Array<[u8; ch::N]>>,
    pub drape: Option<Array<i16>>,
    pub samples: Array<Sample>,
    pub samplech: Array<[u8; ch::N]>,
    /// (OSM way id, route relation id) of rail tracks, sorted by way id.
    pub rail_rels: Vec<(i64, i64)>,
}

impl Legacy {
    pub fn open(dir: &Path) -> Result<Self> {
        let ways = Ways::open(dir)?;
        let nv = ways.verts().len();
        let a = |n: &str| -> Result<Array<u8>> { Array::open(&dir.join(n)) };
        let s = Legacy {
            strings: roadcore::read_strings(dir)?,
            elev: elev::Stored::open(dir)?,
            raw: Array::open(&dir.join("elev.f32"))?,
            grade: a("grade.u8")?,
            src: a("src.u8")?,
            scenic: Array::open(&dir.join("scenic.u8")).ok().filter(|x: &Array<[u8; ch::N]>| x.get().len() == nv),
            drape: Array::open(&dir.join("vterrain.i16")).ok().filter(|x: &Array<i16>| x.get().len() == nv),
            samples: Array::open(&dir.join("samples.bin"))?,
            samplech: Array::open(&dir.join("samples.ch.u8"))?,
            rail_rels: std::fs::read(dir.join("rail-rels.bin"))
                .map(|b| b.chunks_exact(16).map(|c| (i64::from_le_bytes(c[..8].try_into().unwrap()), i64::from_le_bytes(c[8..].try_into().unwrap()))).collect())
                .unwrap_or_default(),
            ways,
        };
        ensure!(s.elev.get().len() == nv && s.raw.get().len() == nv && s.grade.get().len() == nv && s.src.get().len() == nv, "per-vertex arrays out of step with verts.bin");
        ensure!(s.samples.get().len() == s.samplech.get().len(), "samples.ch.u8 out of step with samples.bin");
        Ok(s)
    }

    pub fn first_vertex(&self, w: &WayRec) -> [i32; 2] {
        self.ways.verts()[w.vstart as usize]
    }

    /// The way's vertex range.
    pub fn range(w: &WayRec) -> std::ops::Range<usize> {
        w.vstart as usize..(w.vstart + w.vcount as u64) as usize
    }

    /// Every way's unit (z6 tile of its first vertex), and the ways of each unit in base-pack order:
    /// by the z9 tile of the first vertex, then the folder's (Morton) order.
    pub fn units(&self) -> BTreeMap<Unit, Vec<u32>> {
        let ways = self.ways.ways();
        let keyed: Vec<(Unit, u64, u32)> = ways
            .par_iter()
            .enumerate()
            .map(|(i, w)| {
                let p = self.first_vertex(w);
                let u = Unit::of_point(6, p);
                let z9 = Unit::of_point(9, p).key();
                (u, z9, i as u32)
            })
            .collect();
        let mut by: BTreeMap<Unit, Vec<(u64, u32)>> = BTreeMap::new();
        for (u, z9, i) in keyed {
            by.entry(u).or_default().push((z9, i));
        }
        by.into_iter()
            .map(|(u, mut v)| {
                v.sort_unstable();
                (u, v.into_iter().map(|x| x.1).collect())
            })
            .collect()
    }
}

/// Little-endian bytes of a slice of plain records.
pub fn bytes<T: bytemuck::Pod>(v: &[T]) -> &[u8] {
    bytemuck::cast_slice(v)
}

pub use roadcore::packs::{RailRel, RoadRec, Sub9};

/// The sections of one unit's base pack, as bytes, from the folder's arrays.
pub struct BaseSections {
    pub meta: serde_json::Value,
    pub sections: Vec<(&'static str, Vec<u8>)>,
}

/// Build a unit's base pack from the folder's ways `idx` (in base-pack order).
pub fn base_sections(lg: &Legacy, unit: Unit, idx: &[u32], built: &str) -> BaseSections {
    let all = lg.ways.ways();
    let verts = lg.ways.verts();
    let mut strings: Vec<&str> = vec![""];
    let mut sid: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut local_str = |g: u32| -> u32 {
        if g == 0 {
            return 0;
        }
        *sid.entry(g).or_insert_with(|| {
            strings.push(lg.strings[g as usize].as_str());
            (strings.len() - 1) as u32
        })
    };
    let nv: usize = idx.iter().map(|&i| all[i as usize].vcount as usize).sum();
    let mut ways: Vec<WayRec> = Vec::with_capacity(idx.len());
    let (mut v, mut el, mut raw, mut gr, mut src) = (Vec::with_capacity(nv), Vec::with_capacity(nv), Vec::with_capacity(nv), Vec::with_capacity(nv), Vec::with_capacity(nv));
    let mut sc: Vec<[u8; ch::N]> = Vec::with_capacity(if lg.scenic.is_some() { nv } else { 0 });
    let mut dr: Vec<i16> = Vec::with_capacity(if lg.drape.is_some() { nv } else { 0 });
    let mut ext = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    let mut sub9: Vec<Sub9> = Vec::new();
    let mut rail: Vec<RailRel> = Vec::new();
    // Samples: grouped by way in the folder's way order.
    let samples = lg.samples.get();
    let schan = lg.samplech.get();
    let mut srange: std::collections::HashMap<u32, (usize, usize)> = std::collections::HashMap::with_capacity(idx.len());
    {
        // Binary search each way's sample run.
        for &i in idx {
            let a = samples.partition_point(|s| s.way < i);
            let b = samples.partition_point(|s| s.way <= i);
            if b > a {
                srange.insert(i, (a, b));
            }
        }
    }
    let mut ls: Vec<Sample> = Vec::new();
    let mut lc: Vec<[u8; ch::N]> = Vec::new();
    for (li, &gi) in idx.iter().enumerate() {
        let w = &all[gi as usize];
        let r = Legacy::range(w);
        let mut nw = *w;
        nw.vstart = v.len() as u64;
        nw.name = local_str(w.name);
        nw.ref_ = local_str(w.ref_);
        nw.surface = local_str(w.surface);
        nw.route = local_str(w.route);
        for p in &verts[r.clone()] {
            ext = [ext[0].min(p[0]), ext[1].min(p[1]), ext[2].max(p[0]), ext[3].max(p[1])];
        }
        v.extend_from_slice(&verts[r.clone()]);
        let le = lg.elev.get().slice(r.clone());
        el.extend((0..le.len()).map(|i| elev::to_u16(le.dm(i))));
        raw.extend_from_slice(&lg.raw.get()[r.clone()]);
        gr.extend_from_slice(&lg.grade.get()[r.clone()]);
        src.extend_from_slice(&lg.src.get()[r.clone()]);
        if let Some(a) = &lg.scenic {
            sc.extend_from_slice(&a.get()[r.clone()]);
        }
        if let Some(a) = &lg.drape {
            dr.extend_from_slice(&a.get()[r.clone()]);
        }
        let z9 = Unit::of_point(9, verts[w.vstart as usize]).key();
        match sub9.last_mut() {
            Some(s) if s.key == z9 => s.count += 1,
            _ => sub9.push(Sub9 { key: z9, first: li as u32, count: 1 }),
        }
        if class::is_rail(w.class) {
            if let Ok(k) = lg.rail_rels.binary_search_by_key(&w.id, |x| x.0) {
                let rel = lg.rail_rels[k].1 as u64;
                rail.push(RailRel { way: li as u32, rel_lo: rel as u32, rel_hi: (rel >> 32) as u32, _pad: 0 });
            }
        }
        ways.push(nw);
        if let Some(&(a, b)) = srange.get(&gi) {
            for k in a..b {
                let mut s = samples[k];
                s.way = li as u32;
                ls.push(s);
                lc.push(schan[k]);
            }
        }
    }
    let mut sb = strings.join("\n");
    sb.push('\n');
    let meta = serde_json::json!({
        "fmt": 1,
        "unit": unit.slash(),
        "ways": ways.len(),
        "verts": v.len(),
        "samples": ls.len(),
        "extent": ext,
        "source": built,
        "scenic": lg.scenic.is_some(),
        "drape": lg.drape.is_some(),
        "summary": crate::summary::Summary::of(&ways, &v, Elevs::U16(&el)),
    });
    let mut sections: Vec<(&'static str, Vec<u8>)> = vec![
        ("ways", bytes(&ways).to_vec()),
        ("verts", bytes(&v).to_vec()),
        ("elevu", bytes(&el).to_vec()),
        ("raw", bytes(&raw).to_vec()),
        ("grade", gr),
        ("src", src),
        ("strings", sb.into_bytes()),
        ("samples", bytes(&ls).to_vec()),
        ("samplech", bytes(&lc).to_vec()),
        ("sub9", bytes(&sub9).to_vec()),
        ("rail", bytes(&rail).to_vec()),
    ];
    if lg.scenic.is_some() {
        sections.push(("scenic", bytes(&sc).to_vec()));
    }
    if lg.drape.is_some() {
        sections.push(("drape", bytes(&dr).to_vec()));
    }
    BaseSections { meta, sections }
}
