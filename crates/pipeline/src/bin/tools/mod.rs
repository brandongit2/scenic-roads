//! The programs `scenic-build tool <name> …` runs (pipeline::cli::TOOLS; task #917): once binaries
//! of their own, each linking the library again; folded into scenic-build so the shared code links
//! once. Each keeps its command line (pipeline::cli::progs) and its exit status; jobs and tasks run
//! them by name through pipeline::cli::program.

pub mod areaflags;
pub mod bldtile;
pub mod coastcheck;
pub mod elev;
pub mod extract;
pub mod lakefill;
pub mod landcover;
pub mod metrics;
pub mod polar;
pub mod railfreq;
pub mod terrainsub;
pub mod tile;
pub mod trees;

use std::process::{ExitCode, Termination};

/// Runs tool `args[0]` with its command line `args` (its name first, as a program's own line);
/// None when it isn't one (scenic-build's own command line then says so, with the tools' list).
pub fn run(args: &[String]) -> Option<ExitCode> {
    let a = args.to_vec();
    Some(match args.first()?.as_str() {
        "areaflags" => areaflags::main(a).report(),
        "bldtile" => bldtile::main(a).report(),
        "coastcheck" => coastcheck::main(a).report(),
        "elev" => elev::main(a).report(),
        "extract" => extract::main(a).report(),
        "lakefill" => lakefill::main(a).report(),
        "landcover" => landcover::main(a),
        "scenic-metrics" => metrics::main(a),
        "scenic-polar" => polar::main(a).report(),
        "railfreq" => railfreq::main(a).report(),
        "terrainsub" => terrainsub::main(a).report(),
        "tile" => tile::main(a).report(),
        "trees" => trees::main(a).report(),
        _ => return None,
    })
}
