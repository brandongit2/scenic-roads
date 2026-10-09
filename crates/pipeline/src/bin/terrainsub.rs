//! A terrain piece's z8 subtrees made from a task's file (`pipeline::terrain_task`): the program a
//! `terrainsub` task runs, natively or as WebAssembly in a page.
//!
//! usage: terrainsub --in in.sect --out dir [--workers n]
//!            the group's tiles (dir/hi.sect) and its mid's part (dir/mid.sect), made from what the
//!            file holds alone (its raw tiles, water and GLO-30's windows): no NAS, no network

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

const USAGE: &str = "usage: terrainsub --in in.sect --out dir [--workers n]";

fn main() -> Result<()> {
    pipeline::timings::job("terrainsub", run)
}

fn run() -> Result<()> {
    let (mut input, mut out, mut workers) = (None, None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().with_context(|| format!("{a} needs a value\n{USAGE}"));
        match a.as_str() {
            "--in" => input = Some(PathBuf::from(v()?)),
            "--out" => out = Some(PathBuf::from(v()?)),
            "--workers" => workers = Some(v()?.parse::<usize>().context("--workers")?.max(1)),
            _ => bail!("unexpected {a}\n{USAGE}"),
        }
    }
    let (input, out) = (input.context(USAGE)?, out.context(USAGE)?);
    // (Threads as many as said, else rayon's; in WebAssembly, none can be made: this one.)
    let mut pool = rayon::ThreadPoolBuilder::new();
    if let Some(w) = workers {
        pool = pool.num_threads(w);
    }
    let pool = pool.build().ok();
    let t0 = std::time::Instant::now();
    pipeline::dem::sample::within(pool.as_ref(), || -> Result<()> {
        let inp = pipeline::terrain_task::Inputs::open(&input)?;
        let piece = inp.piece()?;
        let (hi, mid) = inp.make()?;
        std::fs::create_dir_all(&out)?;
        pipeline::terrain_task::write_hi(&out.join(pipeline::terrain_task::HI), &inp.meta.piece, &inp.meta.subtrees, &hi)?;
        pipeline::terrain_pack::write_mid(&out.join(pipeline::terrain_task::MID), piece, &mid)?;
        eprintln!("terrainsub {} {}: {} tiles in {:.1} s", inp.meta.piece, inp.meta.subtrees, hi.len(), t0.elapsed().as_secs_f64());
        Ok(())
    })
}
