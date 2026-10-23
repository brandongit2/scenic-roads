//! A unit folder's sampled elevations post-processed (`pipeline::elev`): final.u16 and grade.u8,
//! which the scenic steps read. Its command line: pipeline::cli::progs::Tile.

use anyhow::Result;
use clap::Parser;
use roadcore::elev;
use roadcore::{Array, Ways};

fn main() -> Result<()> {
    let dir = pipeline::cli::progs::Tile::parse().unit_dir;
    let t0 = std::time::Instant::now();
    let wv = Ways::open(&dir)?;
    let raw = Array::<f32>::open(&dir.join("elev.f32"))?;
    eprintln!("{} ways, {} vertices", wv.ways().len(), wv.verts().len());
    let mut net = pipeline::elev::Net::build(wv.ways(), wv.verts());
    net.raise(&pipeline::elev::raised(&dir, wv.ways(), wv.verts())?);
    let p = pipeline::elev::process(&net, raw.get());
    let du: Vec<u16> = p.elev.iter().map(|&e| elev::to_u16((e * 10.0).round() as i32)).collect();
    std::fs::write(roadcore::tmp(&dir, "final.u16"), bytemuck::cast_slice(&du))?;
    std::fs::write(roadcore::tmp(&dir, "grade.u8"), &p.grade)?;
    roadcore::commit(&dir, &["final.u16", "grade.u8"])?;
    // (A folder made before final.u16: its readers would take the new file anyway.)
    std::fs::remove_file(dir.join("final.i16")).ok();
    eprintln!("elevations processed ({:.0?})", t0.elapsed());
    Ok(())
}
