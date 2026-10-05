//! A unit's area flags (`grid.areas.u8`) on its own z11 grid (`grid.idx`), from the flagged
//! polygons near it: the same bytes as dem/areaflags.py (pipeline::areaflags).
//!
//! usage: areaflags <unit_dir> <area-shapes.geojsonseq>

use anyhow::{bail, Result};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        bail!("usage: areaflags <unit_dir> <area-shapes.geojsonseq>");
    }
    pipeline::areaflags::run(Path::new(&args[1]), Path::new(&args[2]))
}
