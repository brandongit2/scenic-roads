//! A polar cap's terrain and shading tiles from the elevation pool (pipeline::polar; docs/polar.md,
//! planned): a tool, not a step. It reads the build's records and the pool's datasets and writes only
//! its `--out` folder. Its command line: pipeline::cli::progs::Polar.

use anyhow::{bail, Context, Result};
use clap::Parser;
use pipeline::cli::progs::Polar;
use roadcore::polar::Pole;

fn main() -> Result<()> {
    let a = Polar::parse();
    let pole = Pole::of_letter(&a.pole).context("--pole n or s")?;
    if a.max_z > pipeline::polar::MAX_Z {
        bail!("--max-z is {} at most", pipeline::polar::MAX_Z);
    }
    let mut pool = rayon::ThreadPoolBuilder::new();
    if let Some(w) = a.workers {
        pool = pool.num_threads(w.max(1));
    }
    let pool = pool.build()?;
    let out = pipeline::out::Out::unbound(&a.root, &a.out.join("scratch"))?;
    let lat = if pole == Pole::North { [84.9, 90.0] } else { [-90.0, -84.9] };
    let base = pipeline::terrain_base::Pool::read(out.root(), &out.manifest, [-180.0, lat[0], 180.0, lat[1]])?;
    let net = pipeline::fetch::Fetcher::from_env();
    let o = pipeline::dem::generic::Opts { fetch: &net, store: Some(base.store.clone()), pool: Some(&pool) };
    let dir = a.out.join(pole.letter().to_string());
    let t0 = std::time::Instant::now();
    let put = |z: u8, x: u32, y: u32, t: &[u8], s: &[u8]| -> Result<()> {
        for (kind, b) in [("terrain", t), ("shade", s)] {
            let d = dir.join(kind).join(z.to_string()).join(x.to_string());
            std::fs::create_dir_all(&d)?;
            std::fs::write(d.join(format!("{y}.webp")), b)?;
        }
        Ok(())
    };
    let only: Vec<(u32, u32)> = a
        .only
        .iter()
        .map(|t| {
            let (x, y) = t.split_once('/').context("--only x/y")?;
            Ok((x.parse()?, y.parse()?))
        })
        .collect::<Result<_>>()?;
    let only = (!only.is_empty()).then_some(&only[..]);
    let made = pipeline::polar::build(&base.plan, &o, pole, a.max_z, a.block, only, &put, &|m| {
        eprintln!("{:.0} s: {m}", t0.elapsed().as_secs_f64())
    })?;
    println!("{}", serde_json::json!({ "pole": a.pole, "secs": t0.elapsed().as_secs_f64(), "levels": made.levels }));
    Ok(())
}
