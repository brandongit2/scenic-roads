//! A unit's area flags (`grid.areas.u8`) on its own z11 grid (`grid.idx`), from the flagged
//! polygons near it: codecs::areaflags. Its command line: agentcore::cli::progs::Areaflags.

use anyhow::Result;
use clap::Parser;

pub fn main(args: Vec<String>) -> Result<()> {
    let a = agentcore::cli::progs::Areaflags::parse_from(args);
    codecs::areaflags::run(&a.unit_dir, &a.shapes)
}
