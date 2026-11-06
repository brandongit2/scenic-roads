//! The elevation pool's memory as a sampling run holds it (task #929): tiles of one zoom sampled from
//! the pool over their box (pipeline::terrain_base), one at a time, this process's footprint said
//! after each, under a footprint guard. A measure on real data, writing nothing but the Terrarium
//! tiles' local cache.
//!
//! usage: cargo run -p pipeline --example pool_memory -- <root> <raw cache dir> <max MB> <z/x/y> …
//!
//! `<root>`: the build's root (its records and stores, read only).

use pipeline::terrain_pack::RawSource;
use std::sync::atomic::AtomicU64;

fn mb() -> u64 {
    jobrt::sys::footprint().unwrap_or(0) / 1_000_000
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root, raw, max, tiles @ ..] = args.as_slice() else {
        anyhow::bail!("usage: pool_memory <root> <raw cache dir> <max MB> <z/x/y> …")
    };
    jobrt::sys::footprint_guard("pool_memory", max.parse()?);
    let root = store::nasfs::NasRoot::from_arg(root);
    let m = pipeline::out::read_manifest(&root)?;
    let ts: Vec<(u8, u32, u32)> = tiles
        .iter()
        .map(|t| {
            let v: Vec<u32> = t.split('/').map(|v| v.parse()).collect::<Result<_, _>>()?;
            anyhow::ensure!(v.len() == 3, "{t}: not z/x/y");
            Ok((v[0] as u8, v[1], v[2]))
        })
        .collect::<anyhow::Result<_>>()?;
    let z = ts.first().map_or(0, |t| t.0);
    anyhow::ensure!(ts.iter().all(|t| t.0 == z), "the tiles are of one zoom");
    let xy: Vec<(u32, u32)> = ts.iter().map(|t| (t.1, t.2)).collect();
    let pool = pipeline::terrain_base::Pool::read(&root, &m, pipeline::terrain_base::tiles_box(z, &xy))?;
    let url = pipeline::terrain_base::terrarium_template(&root, &m)?;
    let raw = pipeline::terrain_pack::RawTiles::with_store(
        std::path::Path::new(raw),
        &root.join("sources/aws-terrarium"),
        &url,
    );
    println!("start: {} MB", mb());
    let all = std::time::Instant::now();
    let mut peak = 0;
    pool.with_tiles(&raw, |tiles| -> anyhow::Result<()> {
        for &t in &xy {
            let t0 = std::time::Instant::now();
            tiles.prefetch_counted(z, &[t], 1, &AtomicU64::new(0))?;
            let data = tiles.get(z, t.0, t.1)?.0.is_some();
            peak = peak.max(mb());
            println!("{z}/{}/{}: data {data}, {:.1} s, footprint {} MB", t.0, t.1, t0.elapsed().as_secs_f64(), mb());
        }
        Ok(())
    })??;
    println!("all: {:.1} s, peak seen {} MB, end {} MB", all.elapsed().as_secs_f64(), peak, mb());
    Ok(())
}
