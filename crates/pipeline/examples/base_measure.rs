//! A z6 tile's terrain base tiles (its z12 → z9, every one, no coverage) sampled from the elevation
//! pool (pipeline::terrain_base), timed level by level: a measure on real data, writing nothing but
//! the Terrarium tiles' local cache.
//!
//! usage: cargo run --release -p pipeline --example base_measure -- <plan.json> <z6 x/y> <stores dir>
//!        <raw cache dir> <aws-terrarium archive dir>
//!
//! `<plan.json>`: a `dem::declared::Plan` over the z6 tile (the pool's datasets, their files near it).

use pipeline::terrain_pack::{RawSource, RawTiles};
use std::path::Path;
use std::sync::atomic::AtomicU64;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, plan, t, store, raw, aws] = args.as_slice() else {
        anyhow::bail!("usage: base_measure <plan.json> <z6 x/y> <stores dir> <raw cache dir> <aws-terrarium dir>")
    };
    let plan: pipeline::dem::declared::Plan = serde_json::from_slice(&std::fs::read(plan)?)?;
    let t: Vec<u32> = t.split('/').map(|v| v.parse()).collect::<Result<_, _>>()?;
    let pool = pipeline::terrain_base::Pool::of(
        plan,
        store::nasfs::NasRoot::from_arg(store).path(),
        pipeline::terrain_base::tiles_box(6, &[(t[0], t[1])]),
    );
    let url = pool.terrarium.clone().ok_or_else(|| anyhow::anyhow!("no Terrarium pyramid in the plan"))?;
    let raw = RawTiles::with_store(Path::new(raw), &store::nasfs::NasRoot::from_arg(aws).path(), &url);
    let all = std::time::Instant::now();
    pool.with_tiles(&raw, None, |tiles| -> anyhow::Result<()> {
        for z in (9..=12u8).rev() {
            let s = 1u32 << (z - 6);
            let ts: Vec<(u32, u32)> = (t[0] * s..(t[0] + 1) * s)
                .flat_map(|x| (t[1] * s..(t[1] + 1) * s).map(move |y| (x, y)))
                .filter(|&(_, y)| z <= pipeline::terrain_pack::max_zoom_at(pipeline::terrain_pack::tile_lat(z, y)))
                .collect();
            let t0 = std::time::Instant::now();
            tiles.prefetch_counted(z, &ts, 1, &AtomicU64::new(0))?;
            let mut with = 0;
            for &(x, y) in &ts {
                with += tiles.get(z, x, y)?.0.is_some() as usize;
            }
            println!("z{z}: {} tiles ({with} with data) in {:.1} s", ts.len(), t0.elapsed().as_secs_f64());
        }
        Ok(())
    })??;
    println!("all: {:.1} s", all.elapsed().as_secs_f64());
    Ok(())
}
