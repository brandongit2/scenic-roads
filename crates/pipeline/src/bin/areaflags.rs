//! A unit's area flags (`grid.areas.u8`) on its own z11 grid (`grid.idx`), from the flagged
//! polygons near it: pipeline::areaflags. Its command line: pipeline::cli::progs::Areaflags.

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let a = pipeline::cli::progs::Areaflags::parse();
    pipeline::areaflags::run(&a.unit_dir, &a.shapes)
}
