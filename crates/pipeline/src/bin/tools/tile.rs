//! A unit folder's sampled elevations post-processed (`roads::elev`): final.u16 and grade.u8,
//! which the scenic steps read. Its command line: agentcore::cli::progs::Tile.

use anyhow::Result;
use clap::Parser;
use roadcore::elev;
use roadcore::{Array, Ways};

pub fn main(args: Vec<String>) -> Result<()> {
    let dir = agentcore::cli::progs::Tile::parse_from(args).unit_dir;
    let t0 = std::time::Instant::now();
    let wv = Ways::open(&dir)?;
    let raw = Array::<f32>::open(&dir.join("elev.f32"))?;
    eprintln!("{} ways, {} vertices", wv.ways().len(), wv.verts().len());
    let mut net = roads::elev::Net::build(wv.ways(), wv.verts());
    net.raise(&roads::elev::raised(&dir, wv.ways(), wv.verts())?);
    let p = roads::elev::process(&net, raw.get());
    let du: Vec<u16> = p.elev.iter().map(|&e| elev::to_u16((e * 10.0).round() as i32)).collect();
    std::fs::write(roadcore::tmp(&dir, "final.u16"), bytemuck::cast_slice(&du))?;
    std::fs::write(roadcore::tmp(&dir, "grade.u8"), &p.grade)?;
    roadcore::commit(&dir, &["final.u16", "grade.u8"])?;
    // (A folder made before final.u16: its readers would take the new file anyway.)
    std::fs::remove_file(dir.join("final.i16")).ok();
    eprintln!("elevations processed ({:.0?})", t0.elapsed());
    Ok(())
}
