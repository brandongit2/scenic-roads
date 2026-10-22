pub mod agent;
pub mod answers;
pub mod areafiles;
pub mod areaflags;
pub mod basepack;
pub mod basepart;
pub mod bld;
pub mod budget;
pub mod buildings;
pub mod buildtiles;
pub mod candidates;
pub mod chain;
pub mod cli;
pub mod control;
pub mod coord;
pub mod costin;
pub mod coverage;
pub mod dem;
pub mod depth;
pub mod elev;
pub mod extents;
pub mod fetch;
pub mod filterprops;
pub mod geotiff;
pub mod handoff;
pub mod heritage;
pub mod hipack;
pub mod inputs;
pub mod interest;
pub mod lakefill;
pub mod landcover;
pub mod layers;
pub mod legacy;
pub mod localread;
pub mod markconv;
pub mod marks;
pub mod marksjob;
pub mod namestodo;
pub mod needs;
#[cfg(not(target_os = "wasi"))]
pub mod net;
pub mod offload;
pub mod osmpass;
pub mod out;
pub mod outlines;
pub mod ovconv;
pub mod peaks;
pub mod pool;
pub mod pubtimes;
pub mod rail;
pub mod rawpack;
pub mod reach;
pub mod rules;
pub mod scache;
pub mod slope_pack;
pub mod stage;
pub mod stations;
pub mod summary;
pub mod summits;
pub mod sys;
pub mod terr;
pub mod terrain_fine;
pub mod terrain_north;
pub mod terrain_pack;
pub mod terrain_sources;
pub mod terrain_task;
pub mod terrain_water;
pub mod terrain_z8;
pub mod tiling;
pub mod timetables;
pub mod timings;
pub mod trailends;
pub mod treepacks;
pub mod trees;
pub mod unit;
pub mod view;
pub mod vtgen;
pub mod water;
pub mod watercov;
pub mod webp;
pub mod whole;

use indicatif::{ProgressBar, ProgressStyle};
use std::io::Read;

pub fn bytes_bar(len: u64, msg: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(
            "{msg:>28} [{bar:40.cyan/blue}] {bytes:>10}/{total_bytes:<10} {bytes_per_sec:>12} eta {eta}",
        )
        .unwrap()
        .progress_chars("━╸ "),
    );
    pb.set_message(msg.into());
    pb
}

pub fn count_bar(len: u64, msg: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(
            "{msg:>28} [{bar:40.cyan/blue}] {human_pos:>12}/{human_len:<12} {per_sec:>14} eta {eta}",
        )
        .unwrap()
        .progress_chars("━╸ "),
    );
    pb.set_message(msg.into());
    pb
}

/// Reader wrapper that advances a progress bar by bytes consumed.
pub struct ProgressRead<R> {
    pub inner: R,
    pub pb: ProgressBar,
}

impl<R: Read> Read for ProgressRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.pb.inc(n as u64);
        Ok(n)
    }
}

/// Interleave the low 32 bits of x and y (Morton / Z-order) for spatial sorting.
pub fn morton(x: u32, y: u32) -> u64 {
    fn spread(v: u32) -> u64 {
        let mut v = v as u64;
        v = (v | (v << 16)) & 0x0000_FFFF_0000_FFFF;
        v = (v | (v << 8)) & 0x00FF_00FF_00FF_00FF;
        v = (v | (v << 4)) & 0x0F0F_0F0F_0F0F_0F0F;
        v = (v | (v << 2)) & 0x3333_3333_3333_3333;
        v = (v | (v << 1)) & 0x5555_5555_5555_5555;
        v
    }
    spread(x) | (spread(y) << 1)
}
