//! Elevations at every road vertex from the declared elevation datasets (`pipeline::dem`), by the
//! plan the unit job wrote in the build folder (`elevation.json`): its outputs (elev.f32, src.u8,
//! dem-stats.json, the cache's dem-cache.*). Its command line: pipeline::cli::progs::Elev.
//!
//!   SCENIC_STORE         the stores' folder (sources/fetched/): a dataset declared `store` is
//!                        read there alone (a file it lacks is an error naming it)
//!   SCENIC_FETCH_*       mirror folders, recording, no network (`pipeline::fetch`)

use anyhow::{Context, Result};
use clap::Parser;
use pipeline::cli::progs::Elev;
use pipeline::dem::{self, Config};
use pipeline::fetch::Fetcher;
use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    let a = Elev::parse();
    // (A server that gave no answer: its own exit status, pipeline::fetch::UNREACHABLE_EXIT.)
    pipeline::fetch::exit_with(run(a))
}

fn run(Elev { build_dir: build, workers, cache, no_cache }: Elev) -> Result<()> {
    let cache = cache.unwrap_or_else(|| build.parent().unwrap_or(&build).join("cache"));
    let env_dir = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let cfg = Config { build, workers, cache, no_cache, store: env_dir("SCENIC_STORE") };
    let plan = dem::declared::read_plan(&cfg.build)?.with_context(|| {
        format!(
            "{}: no {} (the unit job writes the declared elevation datasets' plan there)",
            cfg.build.display(),
            dem::declared::PLAN_FILE
        )
    })?;
    // (Its phases, for the unit job's phase that runs it: pipeline::timings.)
    pipeline::timings::job("elev", || dem::generic::run(&cfg, &plan, &Fetcher::from_env()).map(|_| ()))?;
    Ok(())
}
