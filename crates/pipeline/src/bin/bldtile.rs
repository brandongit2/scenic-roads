//! A z8 tile's 3D buildings tiles from its task folder (`pipeline::bld::task`; docs/buildings3d.md
//! §3.6): the program a `bldtiles` job's task runs, natively or as WebAssembly in a page. Writes in
//! the folder `area.tiles` (its z14 tiles, RDTILES) and `area.json` (what it made). Its command
//! line: pipeline::cli::progs::Bldtile.

use anyhow::{Context, Result};
use clap::Parser;
use pipeline::cli::progs::Bldtile;

fn main() -> Result<()> {
    let Bldtile { dir, tile, workers } = Bldtile::parse();
    let a = pipeline::legacy::TileId::parse(&tile)
        .filter(|u| u.z == 8)
        .with_context(|| format!("not a z8 tile: {tile}"))?;
    // (Threads as many as said, else rayon's; in WebAssembly, none can be made: this one.)
    let mut pool = rayon::ThreadPoolBuilder::new();
    if let Some(w) = workers {
        pool = pool.num_threads(w.max(1));
    }
    let pool = pool.build().ok();
    let t0 = std::time::Instant::now();
    pipeline::dem::sample::within(pool.as_ref(), &|| {
        let s = pipeline::bld::task::run(&dir, a)?;
        eprintln!(
            "bldtile {}: {} buildings and {} parts in {} tiles ({:.1} s)",
            a.slash(),
            s.buildings,
            s.parts,
            s.tiles,
            t0.elapsed().as_secs_f64()
        );
        Ok(())
    })
}
