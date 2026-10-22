//! A terrain piece's z8 subtrees made from a task's file (`pipeline::terrain_task`): the program a
//! `terrainsub` task runs, natively or as WebAssembly in a page. The group's tiles (dir/hi.sect) and
//! its mid's part (dir/mid.sect), made from what the file holds alone (its raw tiles, water and
//! GLO-30's windows): no NAS, no network. Its command line: pipeline::cli::progs::Terrainsub.

use anyhow::Result;
use clap::Parser;
use pipeline::cli::progs::Terrainsub;

fn main() -> Result<()> {
    let a = Terrainsub::parse();
    pipeline::timings::job("terrainsub", || run(&a))
}

fn run(Terrainsub { input, out, workers }: &Terrainsub) -> Result<()> {
    // (Threads as many as said, else rayon's; in WebAssembly, none can be made: this one.)
    let mut pool = rayon::ThreadPoolBuilder::new();
    if let Some(w) = workers {
        pool = pool.num_threads((*w).max(1));
    }
    let pool = pool.build().ok();
    let t0 = std::time::Instant::now();
    pipeline::dem::sample::within(pool.as_ref(), || -> Result<()> {
        let inp = pipeline::terrain_task::Inputs::open(input)?;
        let piece = inp.piece()?;
        let (hi, mid) = inp.make()?;
        std::fs::create_dir_all(out)?;
        pipeline::terrain_task::write_hi(
            &out.join(pipeline::terrain_task::HI),
            &inp.meta.piece,
            &inp.meta.subtrees,
            &hi,
        )?;
        pipeline::terrain_pack::write_mid(&out.join(pipeline::terrain_task::MID), piece, &mid)?;
        eprintln!(
            "terrainsub {} {}: {} tiles in {:.1} s",
            inp.meta.piece,
            inp.meta.subtrees,
            hi.len(),
            t0.elapsed().as_secs_f64()
        );
        Ok(())
    })
}
