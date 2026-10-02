//! Assemble outlines from an outline set and look up points:
//!   cargo run --release -p pipeline --example outlines -- <set.osm.pbf> <out.sect> [lon,lat …]
use pipeline::outlines::{assemble, Outlines};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (set, out) = (PathBuf::from(&a[1]), PathBuf::from(&a[2]));
    let t = std::time::Instant::now();
    let s = assemble(&set, &std::env::temp_dir().join("outlines-work"), &out)?;
    println!("{} outlines, {} points, {} simplified, by level {:?} ({:.1?})", s.outlines, s.points, s.simplified_points, s.by_level, t.elapsed());
    let o = Outlines::open(&out)?;
    for p in &a[3..] {
        let v: Vec<f64> = p.split(',').map(|x| x.parse().unwrap()).collect();
        let q = [(v[0] * 1e7) as i32, (v[1] * 1e7) as i32];
        let c = o.containing(q);
        println!("{p}: {}", c.iter().map(|r| format!("{} {} (level {}, {}, {:.0} km²)", r.id, o.string(r.name), r.level, o.string(r.iso), r.area_km2)).collect::<Vec<_>>().join(" ⊂ "));
        println!("  ISO {:?}", o.iso_at(q));
    }
    Ok(())
}
