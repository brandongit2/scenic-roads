//! Elevations at every road vertex from national DEMs (`pipeline::dem`): its
//! outputs (elev.f32, src.u8, dem-stats.json, the cache's dem-cache.*).
//!
//! usage: elev <build_dir> [--workers N] [--cache DIR] [--no-cache]
//!
//!   SCENIC_MOI_DTM       Taiwan's MOI DTM GeoTIFFs (else data/sources/moi-dtm)
//!   SCENIC_FABDEM_STORE  FABDEM's store, each tile downloaded once (else read in place at Bristol)
//!   SCENIC_STORES_READ_ONLY  the store only read (a task's worker): what it lacks read in place
//!   SCENIC_FETCH_*       mirror folders, recording, no network (`pipeline::fetch`)

use anyhow::{bail, Context, Result};
use pipeline::dem::{self, Config};
use pipeline::fetch::Fetcher;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut build: Option<PathBuf> = None;
    let (mut workers, mut cache, mut no_cache) = (48usize, None::<PathBuf>, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| -> Result<String> { inline.clone().or_else(|| args.next()).with_context(|| format!("{name} needs a value")) };
        match flag.as_str() {
            "--workers" => workers = value("--workers")?.parse().context("--workers")?,
            "--cache" => cache = Some(PathBuf::from(value("--cache")?)),
            "--no-cache" => no_cache = true,
            f if f.starts_with('-') => bail!("unknown option {f}\nusage: elev <build_dir> [--workers N] [--cache DIR] [--no-cache]"),
            _ if build.is_none() => build = Some(PathBuf::from(a)),
            _ => bail!("one build folder only\nusage: elev <build_dir> [--workers N] [--cache DIR] [--no-cache]"),
        }
    }
    let build = build.context("usage: elev <build_dir> [--workers N] [--cache DIR] [--no-cache]")?;
    let cache = cache.unwrap_or_else(|| build.parent().unwrap_or(&build).join("cache"));
    let env_dir = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let moi = env_dir("SCENIC_MOI_DTM").unwrap_or_else(|| PathBuf::from("data/sources/moi-dtm"));
    let read_only = std::env::var("SCENIC_STORES_READ_ONLY").is_ok_and(|v| !v.is_empty() && v != "0");
    let cfg = Config { build, workers, cache, no_cache, moi_dtm: dem::moi_files(&moi)?, fabdem_store: env_dir("SCENIC_FABDEM_STORE"), fabdem_read_only: read_only };
    // (Its phases, for the unit job's phase that runs it: pipeline::timings.)
    pipeline::timings::job("elev", || dem::run(&cfg, &Fetcher::from_env()))?;
    Ok(())
}
