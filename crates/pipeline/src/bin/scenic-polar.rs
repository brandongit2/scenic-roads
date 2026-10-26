//! A polar cap's terrain and shading tiles from the elevation pool into a folder, by the steps' own
//! code (pipeline::polar: each block's terrain, then each one's shading from the folder's terrain,
//! then the root; docs/polar.md): for measuring and the check page (web/test/polar.html). It reads
//! the build's records and the pool's datasets and writes only its `--out` folder. Its command
//! line: pipeline::cli::progs::Polar.

use anyhow::{bail, Context, Result};
use clap::Parser;
use pipeline::cli::progs::Polar;
use pipeline::polar::{self, BLOCK_Z};
use roadcore::polar::Pole;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let a = Polar::parse();
    let pole = Pole::of_letter(&a.pole).context("--pole n or s")?;
    if !(BLOCK_Z..=polar::MAX_Z).contains(&a.max_z) {
        bail!("--max-z is {BLOCK_Z} to {}", polar::MAX_Z);
    }
    if let Some(w) = a.workers {
        rayon::ThreadPoolBuilder::new().num_threads(w.max(1)).build_global()?;
    }
    let out = pipeline::out::Out::unbound(&a.root, &a.out.join("scratch"))?;
    // (Over the root's latitudes: z0–3 reach about 80.6° past the cap.)
    let base = pipeline::terrain_base::Pool::read(out.root(), &out.manifest, roadcore::polar::lat_box(pole, 0, 0, 0))?;
    let net = pipeline::fetch::Fetcher::from_env();
    let o = pipeline::dem::generic::Opts { fetch: &net, store: Some(base.store.clone()) };
    let plans = polar::plans(&base.plan, a.max_z);
    let samplers = polar::samplers(&plans, &o)?;
    let dir = a.out.join(pole.letter().to_string());
    let path = |kind: &str, z: u8, x: u32, y: u32| {
        dir.join(kind).join(z.to_string()).join(x.to_string()).join(format!("{y}.webp"))
    };
    let mut made: std::collections::BTreeMap<(u8, &str), (usize, u64)> = Default::default();
    let mut write = |kind: &'static str, z: u8, x: u32, y: u32, b: &[u8]| -> Result<()> {
        let p = path(kind, z, x, y);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, b)?;
        let m = made.entry((z, kind)).or_default();
        m.0 += 1;
        m.1 += b.len() as u64;
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
    let blocks: Vec<(u32, u32)> =
        polar::blocks(pole).into_iter().filter(|b| only.is_empty() || only.contains(b)).collect();
    let t0 = std::time::Instant::now();
    let said = |m: String| eprintln!("{:.0} s: {m}", t0.elapsed().as_secs_f64());
    let voids = polar::Voids::default();
    for (i, &b) in blocks.iter().enumerate() {
        said(format!("terrain {}/{} ({})", i + 1, blocks.len(), polar::target(pole, b)));
        polar::terrain_block(&samplers, pole, b, a.max_z, &voids, &mut |z, x, y, t| write("terrain", z, x, y, &t))?;
    }
    let terrain = |p: PathBuf| -> Result<Option<Vec<f32>>> { read(&p, roadcore::grid::decode_terrain) };
    for (i, &b) in blocks.iter().enumerate() {
        said(format!("shading {}/{} ({})", i + 1, blocks.len(), polar::target(pole, b)));
        polar::slope_block(
            pole,
            b,
            a.max_z,
            &|z, x, y| Ok(terrain(path("terrain", z, x, y))?.map(std::sync::Arc::new)),
            &mut |z, x, y, s| write("shade", z, x, y, &s),
        )?;
    }
    if only.is_empty() {
        said("root".into());
        polar::root(
            &samplers,
            pole,
            &|x, y| terrain(path("terrain", BLOCK_Z, x, y)),
            &|x, y| read(&path("shade", BLOCK_Z, x, y), roadcore::shade::decode),
            &voids,
            &mut |z, x, y, t, s| {
                write("terrain", z, x, y, &t)?;
                write("shade", z, x, y, &s)
            },
        )?;
    }
    let levels: Vec<serde_json::Value> =
        made.iter().map(|((z, k), (n, b))| serde_json::json!({"z": z, "kind": k, "tiles": n, "bytes": b})).collect();
    println!(
        "{}",
        serde_json::json!({ "pole": a.pole, "secs": t0.elapsed().as_secs_f64(), "levels": levels, "voids": voids.bands() })
    );
    Ok(())
}

/// A file of the folder decoded, None where there's none.
fn read<T>(p: &Path, decode: fn(&[u8]) -> Result<T>) -> Result<Option<T>> {
    match std::fs::read(p) {
        Ok(b) => Ok(Some(decode(&b).with_context(|| format!("decode {}", p.display()))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    }
}
