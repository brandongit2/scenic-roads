//! Record types of the per-area files (docs/formats.md): base packs, road values and hidata,
//! shared by the pipeline (which writes them) and the server (which reads them).

use bytemuck::{Pod, Zeroable};

/// Base pack `sub9`: the ways whose first vertex lies in one z9 tile.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Sub9 {
    pub key: u64,
    pub first: u32,
    pub count: u32,
}

/// Base pack `rail`: a rail way's primary route relation.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RailRel {
    pub way: u32,
    pub rel_lo: u32,
    pub rel_hi: u32,
    pub _pad: u32,
}

impl RailRel {
    pub fn rel(&self) -> i64 {
        ((self.rel_hi as u64) << 32 | self.rel_lo as u64) as i64
    }
}

/// Road values `roads`: one way's place on its road (the one chaining).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct RoadRec {
    /// The road's lowest OSM way id.
    pub road: u64,
    /// The whole road's length, metres.
    pub len: f32,
    /// Metres from the road's start to this way's start, along the road.
    pub offset: f32,
    /// 0: the way runs with the road; 1: against it.
    pub dir: u8,
    pub _pad: [u8; 7],
}

/// hidata `here`: a way drawn in the tile's tiles.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Here {
    pub id: u64,
    /// The owner unit's tile key.
    pub owner: u64,
    /// Its index in the owner's base pack.
    pub index: u32,
    pub class: u8,
    /// `crate::flag` bits.
    pub flags: u8,
    /// `here_extra` bits.
    pub extra: u8,
    pub _pad: u8,
    /// [west, south, east, north], E7.
    pub bbox: [i32; 4],
}

pub mod here_extra {
    /// No name and no ref.
    pub const UNNAMED: u8 = 1;
    pub const RAIL: u8 = 2;
}

/// hidata `ends`: a way end, by point.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct End {
    /// `point_key` of the end vertex.
    pub point: u64,
    /// Index into `here`.
    pub here: u32,
    pub _pad: u32,
}

/// The `End::point` of a vertex.
pub fn point_key(p: [i32; 2]) -> u64 {
    ((p[0] as u32 as u64) << 32) | p[1] as u32 as u64
}

/// hidata `parts`: one road's consecutive samples inside the tile.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Part {
    pub road: u64,
    /// Offset along the road of the first sample, metres.
    pub offset: f32,
    /// Its first sample in `psamples`, and how many.
    pub first: u32,
    pub count: u32,
    /// The whole road's length, metres (the length filter).
    pub road_len: f32,
    pub class: u8,
    /// Bit 0 unpaved, bit 1 toll, bit 2 unnamed (of the first sample's way).
    pub flags: u8,
    pub _pad: [u8; 6],
}

/// hidata `psamples`: a ~100 m sample along a road.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PSample {
    /// Index into `here`.
    pub way: u32,
    /// Offset along the road, metres.
    pub offset: f32,
    pub lon: i32,
    pub lat: i32,
    /// Observer eye elevation, metres.
    pub eye: f32,
    /// `crate::scenic::sflag` bits.
    pub flags: u8,
    pub _pad: [u8; 3],
}

/// hidata `climbs`: a climb starting in the tile.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Climb {
    /// OSM id of the way at the start (opens its road profile).
    pub way: u64,
    /// OSM id of the way at the middle (its name and ref label the climb).
    pub label: u64,
    pub gain_m: f32,
    pub length_m: f32,
    pub start_elev: f32,
    pub top_elev: f32,
    /// Steepest 100 m, percent.
    pub max_grade: f32,
    /// The whole road's length (of the middle way), metres: the length filter.
    pub road_len: f32,
    /// Midpoint, E7.
    pub mid: [i32; 2],
    /// Its polyline in `climbgeom`.
    pub geom_start: u32,
    pub geom_count: u32,
    pub class: u8,
    pub unpaved: u8,
    /// Of the middle way: bit 0 toll, bit 1 no name and no ref.
    pub flags: u8,
    pub _pad: [u8; 5],
}

const _: () = assert!(std::mem::size_of::<Sub9>() == 16);
const _: () = assert!(std::mem::size_of::<RailRel>() == 16);
const _: () = assert!(std::mem::size_of::<RoadRec>() == 24);
const _: () = assert!(std::mem::size_of::<Here>() == 40);
const _: () = assert!(std::mem::size_of::<End>() == 16);
const _: () = assert!(std::mem::size_of::<Part>() == 32);
const _: () = assert!(std::mem::size_of::<PSample>() == 24);
const _: () = assert!(std::mem::size_of::<Climb>() == 64);
