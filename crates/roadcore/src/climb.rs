//! Precomputed climbs (sustained uphill stretches of a road, in an allowed direction).

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ClimbRec {
    /// Way index at the start of the climb (opens its road profile).
    pub way: u32,
    /// Way index at the middle of the climb (its name/ref label the climb).
    pub label_way: u32,
    pub gain_m: f32,
    pub length_m: f32,
    pub start_elev: f32,
    pub top_elev: f32,
    /// Steepest 100 m within the climb, percent.
    pub max_grade: f32,
    /// lon, lat of the climb's midpoint (1e-7 degrees), used for viewport queries.
    pub mid: [i32; 2],
    pub geom_start: u32,
    pub geom_count: u32,
    /// Highest road class along the climb.
    pub class: u8,
    /// 1 if any part is unpaved.
    pub unpaved: u8,
    pub _pad: [u8; 2],
}

const _: () = assert!(std::mem::size_of::<ClimbRec>() == 48);
