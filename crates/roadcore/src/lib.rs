//! On-disk formats shared by the pipeline and the server.
//!
//! Build directory layout (all little-endian):
//!   ways.bin     16-byte header ("RDWAYS02", u64 count) + `WayRec` records
//!   verts.bin    [i32; 2] per vertex: lon, lat in 1e-7 degrees (densified geometry)
//!   strings.txt  newline-separated string table; line 0 is the empty string
//!   elev.f32     raw DEM sample per vertex (NaN = no data)          — dem stage
//!   src.u8       DEM source per vertex (see `DemSource`)             — dem stage
//!   final.u16    processed elevation per vertex, decimetres + 5000   — tile stage (`elev`; older
//!                folders: final.i16, signed)
//!   grade.u8     |grade| per vertex, 0.5 % units                     — tile stage
//!   roads.tiles  tile archive, see `archive`
//!   climbs.bin   `climb::ClimbRec` records; climbs.geom: [i32; 2] polylines they index into

use det::Det;
pub mod archive;
pub mod climb;
pub mod elev;
pub mod grid;
pub mod packs;
pub mod lsum;
pub mod scenic;
pub mod slope;
pub mod tile;

use anyhow::{bail, Context, Result};
use bytemuck::{Pod, Zeroable};
#[cfg(not(target_os = "wasi"))]
pub use memmap2::Mmap;

/// A file's bytes where nothing can be memory-mapped (WASI, the WebAssembly programs workers run:
/// docs/workers.md): read whole, into 8-byte-aligned memory so typed views cast as they do from a
/// mapping.
#[cfg(target_os = "wasi")]
pub struct Mmap {
    words: Vec<u64>,
    len: usize,
}

#[cfg(target_os = "wasi")]
impl std::ops::Deref for Mmap {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &bytemuck::cast_slice(&self.words)[..self.len]
    }
}
use std::fs::File;
use std::path::Path;

pub const WAYS_MAGIC: &[u8; 8] = b"RDWAYS02";

/// Road classes, ordered minor → major so that sorting by class gives draw order, then the
/// passenger rail groups (rails.tiles).
pub mod class {
    pub const SERVICE: u8 = 0;
    pub const LIVING_STREET: u8 = 1;
    pub const RESIDENTIAL: u8 = 2;
    pub const UNCLASSIFIED: u8 = 3;
    pub const TERTIARY: u8 = 4;
    pub const SECONDARY: u8 = 5;
    pub const PRIMARY: u8 = 6;
    pub const TRUNK: u8 = 7;
    pub const MOTORWAY: u8 = 8;
    pub const FERRY: u8 = 9;
    /// Passenger rail: the way's most important service group (`WayRec::rail` has them all).
    pub const TRAM: u8 = 10;
    pub const METRO: u8 = 11;
    pub const COMMUTER: u8 = 12;
    pub const INTERCITY: u8 = 13;
    /// Heritage & tourist railways, rack railways and funiculars.
    pub const HERITAGE: u8 = 14;
    pub const COUNT: usize = 15;
    /// Road classes (and ferries) come first.
    pub const NROAD: usize = 10;
    pub const NAMES: [&str; COUNT] = [
        "service", "living_street", "residential", "unclassified", "tertiary",
        "secondary", "primary", "trunk", "motorway", "ferry",
        "tram", "metro", "commuter", "intercity", "heritage",
    ];
    pub fn is_rail(c: u8) -> bool {
        c >= TRAM
    }
}

/// Route networks by signage, for the street-map colourings (`WayRec::network`).
pub mod network {
    pub const NONE: u8 = 0;
    pub const US_INTERSTATE: u8 = 1;
    pub const US_HIGHWAY: u8 = 2;
    pub const US_STATE: u8 = 3;
    pub const US_COUNTY: u8 = 4;
    pub const CA_AUTOROUTE: u8 = 5;
    pub const CA_PROVINCIAL: u8 = 6;
    pub const CA_REGIONAL: u8 = 7;
    pub const FR_AUTOROUTE: u8 = 10;
    pub const FR_NATIONALE: u8 = 11;
    pub const FR_DEPARTEMENTALE: u8 = 12;
    pub const FR_METROPOLE: u8 = 13;
    pub const GB_MOTORWAY: u8 = 20;
    pub const GB_A_PRIMARY: u8 = 21;
    pub const GB_A: u8 = 22;
    pub const GB_B: u8 = 23;
    pub const IE_MOTORWAY: u8 = 24;
    pub const IE_NATIONAL_PRIMARY: u8 = 25;
    pub const IE_NATIONAL_SECONDARY: u8 = 26;
    pub const IE_REGIONAL: u8 = 27;
    pub const ES_AUTOVIA: u8 = 30;
    pub const ES_NACIONAL: u8 = 31;
    pub const ES_AUTONOMICA: u8 = 32;
    pub const ES_LOCAL: u8 = 33;
    pub const PT_AUTOESTRADA: u8 = 40;
    pub const PT_IP: u8 = 41;
    pub const PT_IC: u8 = 42;
    pub const PT_NACIONAL: u8 = 43;
    pub const PT_REGIONAL: u8 = 44;
    pub const HK_ROUTE: u8 = 50;
    pub const JP_EXPRESSWAY: u8 = 51;
    pub const JP_NATIONAL: u8 = 52;
    pub const JP_PREFECTURAL: u8 = 53;
    pub const TW_FREEWAY: u8 = 54;
    pub const TW_PROVINCIAL: u8 = 55;
    pub const TW_COUNTY: u8 = 56;
    pub const SG_EXPRESSWAY: u8 = 57;
    pub const AD_GENERAL: u8 = 60;
    pub const AD_SECUNDARIA: u8 = 61;
    pub const E_ROAD: u8 = 70;
}

/// Per-way flag bits.
pub mod flag {
    pub const LINK: u8 = 1 << 0;
    pub const BRIDGE: u8 = 1 << 1;
    pub const TUNNEL: u8 = 1 << 2;
    pub const UNPAVED: u8 = 1 << 3;
    pub const ONEWAY: u8 = 1 << 4;
    pub const TOLL: u8 = 1 << 5;
    /// Part of a designated scenic route (byway, route touristique, scenic trail) or scenic=yes.
    pub const SCENIC: u8 = 1 << 6;
    /// Covered bridge.
    pub const COVERED: u8 = 1 << 7;
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemSource {
    None = 0,
    Hrdem = 1,
    Usgs3dep = 2,
    Mrdem = 3,
    /// FABDEM 30 m (Copernicus DEM with forests and buildings removed), elsewhere.
    Fabdem = 4,
    /// GSI 5 m, airborne lidar (Japan).
    Gsi5a = 5,
    /// GSI 5 m, photogrammetry (Japan).
    Gsi5 = 6,
    /// GSI 10 m (Japan).
    Gsi10 = 7,
}

/// Number of DEM source codes, `None` included.
pub const NDEM: usize = 9;

impl DemSource {
    pub fn label(v: u8) -> &'static str {
        match v {
            1 => "NRCan HRDEM lidar (8 m)",
            2 => "USGS 3DEP (10 m)",
            3 => "NRCan MRDEM (30 m)",
            4 => "FABDEM (30 m)",
            5 => "GSI lidar (5 m)",
            6 => "GSI (5 m)",
            7 => "GSI (10 m)",
            _ => "none",
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct WayRec {
    pub id: i64,
    /// Index of the first vertex in verts.bin and the per-vertex arrays.
    pub vstart: u64,
    pub vcount: u32,
    pub name: u32,
    pub ref_: u32,
    pub surface: u32,
    /// km/h, 0 = unknown
    pub maxspeed: u16,
    pub class: u8,
    pub flags: u8,
    pub lanes: u8,
    /// Route network by signage (`network`), roads only.
    pub network: u8,
    /// Passenger rail: bit per service group using the track (bit k = class TRAM + k).
    pub rail: u8,
    pub _pad: u8,
    /// Roads: name of the designated scenic route this way belongs to. Rail: the services
    /// using the track (" · "-separated). String index, 0 = none.
    pub route: u32,
    /// Rail: line colour 0xRRGGBB with bit 24 set (0 = none).
    pub colour: u32,
}

const _: () = assert!(std::mem::size_of::<WayRec>() == 48);

/// Temporary path for an output that is renamed into place by `commit`.
pub fn tmp(dir: &Path, name: &str) -> std::path::PathBuf {
    dir.join(format!("{name}.tmp"))
}

/// Atomically move `<name>.tmp` → `<name>` for each output. A running server keeps its
/// memory maps of the old files until it restarts, so rebuilding never disturbs it.
pub fn commit(dir: &Path, names: &[&str]) -> Result<()> {
    for n in names {
        std::fs::rename(tmp(dir, n), dir.join(n)).with_context(|| format!("commit {n}"))?;
    }
    Ok(())
}

pub fn mmap(path: &Path) -> Result<Mmap> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    #[cfg(target_os = "wasi")]
    {
        use std::io::Read;
        let len = usize::try_from(f.metadata()?.len())?;
        let mut words = vec![0u64; len.div_ceil(8)];
        (&f).read_exact(&mut bytemuck::cast_slice_mut(&mut words)[..len]).with_context(|| format!("read {}", path.display()))?;
        Ok(Mmap { words, len })
    }
    // SAFETY: build outputs are written once and never modified while mapped.
    #[cfg(not(target_os = "wasi"))]
    Ok(unsafe { Mmap::map(&f)? })
}

/// Memory-mapped view over a build directory's per-way and per-vertex arrays.
pub struct Ways {
    ways_map: Mmap,
    verts_map: Mmap,
}

impl Ways {
    pub fn open(dir: &Path) -> Result<Self> {
        let ways_map = mmap(&dir.join("ways.bin"))?;
        if ways_map.len() < 16 || &ways_map[..8] != WAYS_MAGIC {
            bail!("ways.bin: bad header");
        }
        let verts_map = mmap(&dir.join("verts.bin"))?;
        Ok(Self { ways_map, verts_map })
    }
    pub fn ways(&self) -> &[WayRec] {
        bytemuck::cast_slice(&self.ways_map[16..])
    }
    pub fn verts(&self) -> &[[i32; 2]] {
        bytemuck::cast_slice(&self.verts_map[..])
    }
}

pub fn read_strings(dir: &Path) -> Result<Vec<String>> {
    let s = std::fs::read_to_string(dir.join("strings.txt"))?;
    Ok(s.split('\n').map(str::to_owned).collect())
}

/// Typed read-only view of a flat array file (f32, i16, u8 …).
pub struct Array<T: Pod> {
    map: Mmap,
    _t: std::marker::PhantomData<T>,
}

impl<T: Pod> Array<T> {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self { map: mmap(path)?, _t: std::marker::PhantomData })
    }
    pub fn get(&self) -> &[T] {
        bytemuck::cast_slice(&self.map[..])
    }
}

pub const E7: f64 = 1e-7;
pub const EARTH_R: f64 = 6_371_008.8;

/// Fast local distance in metres between two lon/lat points (equirectangular, fine < ~10 km).
#[inline]
pub fn dist_m(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let k = std::f64::consts::PI / 180.0;
    let x = (lon2 - lon1) * k * ((lat1 + lat2) * 0.5 * k).dcos();
    let y = (lat2 - lat1) * k;
    (x * x + y * y).sqrt() * EARTH_R
}

/// Web-Mercator normalised coordinates in [0, 1).
#[inline]
pub fn merc(lon: f64, lat: f64) -> (f64, f64) {
    let x = (lon + 180.0) / 360.0;
    let s = (lat.to_radians()).dsin().clamp(-0.9999, 0.9999);
    let y = 0.5 - ((1.0 + s) / (1.0 - s)).dln() / (4.0 * std::f64::consts::PI);
    (x, y)
}
