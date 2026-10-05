//! Land cover on the z11 analysis grid from ESA WorldCover (`pipeline::landcover`):
//! dem/landcover.py's port, its command line and output (grid.class.u8).
//!
//! usage: landcover <build_dir> [--workers N] [--only <slots.u32>]
//!
//!   SCENIC_FETCH_*  mirror folders, recording, no network (`pipeline::fetch`)

use anyhow::{bail, Context, Result};
use pipeline::fetch::Fetcher;
use pipeline::landcover;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut build: Option<PathBuf> = None;
    let (mut workers, mut only) = (32usize, None::<PathBuf>);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| -> Result<String> { inline.clone().or_else(|| args.next()).with_context(|| format!("{name} needs a value")) };
        match flag.as_str() {
            "--workers" => workers = value("--workers")?.parse().context("--workers")?,
            "--only" => only = Some(PathBuf::from(value("--only")?)),
            f if f.starts_with('-') => bail!("unknown option {f}\nusage: landcover <build_dir> [--workers N] [--only <slots.u32>]"),
            _ if build.is_none() => build = Some(PathBuf::from(a)),
            _ => bail!("one build folder only\nusage: landcover <build_dir> [--workers N] [--only <slots.u32>]"),
        }
    }
    let build = build.context("usage: landcover <build_dir> [--workers N] [--only <slots.u32>]")?;
    let fetch = Fetcher::from_env();
    match only {
        Some(p) => {
            let raw = std::fs::read(&p).with_context(|| p.display().to_string())?;
            let slots: Vec<u32> = bytemuck::pod_collect_to_vec(&raw[..raw.len() / 4 * 4]);
            landcover::only(&build, &slots, &fetch, workers)?;
        }
        None => {
            landcover::full(&build, &fetch, workers)?;
        }
    }
    Ok(())
}
