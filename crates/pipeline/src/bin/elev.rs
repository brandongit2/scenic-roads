//! Elevations at every road vertex from the finished terrain staged in the unit's folder
//! (`pipeline::road_terrain`: `terrain.tiles`, `fine.tiles`, `codes.json`): its outputs elev.f32,
//! src.u8 and dem-stats.json. Its command line: pipeline::cli::progs::Elev. It reads nothing else:
//! no NAS, no network.

use anyhow::{Context, Result};
use clap::Parser;
use pipeline::cli::progs::Elev;

fn main() -> Result<()> {
    let Elev { build_dir: dir } = Elev::parse();
    pipeline::timings::job("elev", || -> Result<()> {
        let t = std::time::Instant::now();
        let va = roadcore::Array::<[i32; 2]>::open(&dir.join("verts.bin")).context("verts.bin")?;
        let verts = va.get();
        let (elev, src) = pipeline::road_terrain::heights(&dir, verts)?;
        for (name, bytes) in [("elev.f32", bytemuck::cast_slice::<f32, u8>(&elev)), ("src.u8", &src[..])] {
            let tmp = dir.join(format!("{name}.tmp"));
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, dir.join(name))?;
        }
        let missing = elev.iter().filter(|e| e.is_nan()).count();
        let stats = serde_json::json!({
            "vertices": verts.len(),
            "missing": missing,
            "seconds": (t.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        });
        std::fs::write(dir.join("dem-stats.json"), serde_json::to_string_pretty(&stats)?)?;
        println!("{stats}");
        Ok(())
    })
}
