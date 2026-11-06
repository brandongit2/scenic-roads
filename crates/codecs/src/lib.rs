//! The build's file formats with no build state: the terrain's and shading's tile codecs, the tree
//! cover's WebP, whole files, packs (legacy, hipack, basepack, basepart), the tiling, vector tiles,
//! marks, stations, extents, tars. No dependency on the pipeline (docs/crates.md, step 2); the pipeline
//! re-exports those its agent and binaries name under their old paths (`pipeline::tiling`, ...), the
//! root's progress bars and `morton` too.

pub mod areaflags;
pub mod basepack;
pub mod basepart;
pub mod chain;
pub mod extents;
pub mod hipack;
pub mod legacy;
pub mod marks;
pub mod shade_codec;
pub mod stations;
pub mod summary;
pub mod tar;
pub mod terrain_codec;
pub mod terrain_water;
pub mod tiling;
pub mod trailends;
pub mod vtgen;
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
