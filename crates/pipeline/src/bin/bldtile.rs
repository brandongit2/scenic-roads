//! A z8 area's 3D buildings tiles from its task folder (`pipeline::bld::task`; docs/buildings3d.md
//! §3.6): the program a `bldtiles` job's task runs, natively or as WebAssembly in a page.
//!
//! usage: bldtile <dir> <8/x/y> [--workers n]
//!            dir: the area's task folder (its work files' blocks, `coverage.sect`); writes there
//!            `area.tiles` (its z12–14 tiles, RDTILES) and `area.json` (what it made)

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

const USAGE: &str = "usage: bldtile <dir> <8/x/y> [--workers n]";

fn main() -> Result<()> {
    let mut rest: Vec<String> = Vec::new();
    let mut workers: Option<usize> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--workers" {
            workers = Some(args.next().context("--workers n")?.parse::<usize>().context("--workers")?.max(1));
        } else if a.starts_with("--") {
            bail!("unknown option {a}\n{USAGE}");
        } else {
            rest.push(a);
        }
    }
    let [dir, area] = &rest[..] else { bail!("{USAGE}") };
    let a = pipeline::legacy::Unit::parse(area).filter(|u| u.z == 8).with_context(|| format!("not a z8 area: {area}\n{USAGE}"))?;
    let dir = PathBuf::from(dir);
    // (Threads as many as said, else rayon's; in WebAssembly, none can be made: this one.)
    let mut pool = rayon::ThreadPoolBuilder::new();
    if let Some(w) = workers {
        pool = pool.num_threads(w);
    }
    let pool = pool.build().ok();
    let t0 = std::time::Instant::now();
    pipeline::dem::sample::within(pool.as_ref(), &|| {
        let s = pipeline::bld::task::run(&dir, a)?;
        eprintln!("bldtile {}: {} buildings and {} parts in {} tiles ({:.1} s)", a.slash(), s.buildings, s.parts, s.tiles.iter().sum::<u64>(), t0.elapsed().as_secs_f64());
        Ok(())
    })
}
