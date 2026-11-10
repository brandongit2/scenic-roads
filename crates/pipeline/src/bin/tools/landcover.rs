//! Land cover on the z11 analysis grid from the declared land-cover source (`trees::landcover`):
//! its output (grid.class.u8). The source is the unit job's, from its folder (`sources.json`:
//! roads::unit::UnitSources). Its command line: agentcore::cli::progs::Landcover.
//!
//!   SCENIC_FETCH_*  mirror folders, recording, no network (`buildstore::fetch`)

use ::trees::landcover;
use agentcore::cli::progs::Landcover;
use anyhow::{Context, Result};
use buildstore::fetch::Fetcher;
use clap::Parser;

pub fn main(args: Vec<String>) -> std::process::ExitCode {
    let a = Landcover::parse_from(args);
    // (A server that gave no answer: its own exit status, buildstore::fetch::UNREACHABLE_EXIT.)
    buildstore::fetch::exit_with(run(a))
}

fn run(Landcover { build_dir: build, workers, only }: Landcover) -> Result<()> {
    let fetch = Fetcher::from_env();
    let cover = roads::unit::UnitSources::read(&build)?
        .land_cover
        .context("the unit's sources.json names no land-cover source")?;
    match only {
        Some(p) => {
            let raw = std::fs::read(&p).with_context(|| p.display().to_string())?;
            let slots: Vec<u32> = bytemuck::pod_collect_to_vec(&raw[..raw.len() / 4 * 4]);
            landcover::only(&build, &slots, &cover, &fetch, workers)?;
        }
        None => {
            landcover::full(&build, &cover, &fetch, workers)?;
        }
    }
    Ok(())
}
