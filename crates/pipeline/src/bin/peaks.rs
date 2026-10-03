//! Peak prominence and isolation from the terrain tiles, for every peak in pois.json.
//!
//! usage: peaks <build_dir>
//!
//! Summit: the highest DEM pixel within 150 m of the OSM point. A pixel claimed by several peaks
//! (needles next to a main summit) goes to the one tagged highest; the others start from their own
//! point. The DEM (~30 m at z12) cuts sharp summits down (Snowdon: 1,040 m for 1,085 m, below the
//! broad Carneddau), so the summit height is the tagged `ele` where it is plausible (from 30 m
//! under to 200 m over the DEM), else the DEM's; every summit pixel is raised to its peak's height
//! for all floods and searches, so a neighbouring summit counts at its real height too (Garnedd
//! Ugain, 740 m from Snowdon, would otherwise see nothing higher nearby).
//!
//! Prominence: a priority flood from the summit, always expanding the highest unvisited pixel,
//! until it reaches ground higher than the summit; the lowest pixel it had to pass is the key col,
//! and prominence = summit − col. Once the flood is down to sea level the answer is the summit
//! height whatever lies beyond. It runs at z12 (terrain.tiles, finer where stored near roads)
//! for up to 600k pixels, then, for the bigger peaks, again at z8 over the whole region (preloaded;
//! ~430 m pixels, which can miss narrow cols and summits, so those values are rougher). Missing
//! tiles are walls: a flood that ends without higher ground or sea gives a lower bound.
//!
//! The terrain tiles have single-pixel spikes and pits (a 2,781 m pixel among ~700 m ones in a z8
//! tile over the Laurentides), which would be false summits and cols: a pixel more than
//! max(150 m, 1.2 × the pixel size) above or below all eight neighbours is clamped to them.
//!
//! Isolation: the distance to the nearest ground higher than the summit, searching tiles nearest
//! first (z12 within 25 km, then z8 over the region); none found = a lower bound (region edge).
//!
//! Output: peaks.json, [{i (index in pois.json), e (DEM summit m), p (prominence m), pl (lower
//! bound), c ([lon, lat] of the col), ce (col m), iso (km), il (lower bound), hi ([lon, lat] of the
//! nearest higher ground)}].

use anyhow::Result;
use pipeline::peaks::{self, ArchiveTiles, Peak};
use roadcore::archive::Archive;
use std::path::PathBuf;

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: peaks <build_dir>"))?);
    let t0 = std::time::Instant::now();
    let pois: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("pois.json"))?)?;
    let mut list: Vec<Peak> = Vec::new();
    for (i, f) in pois["features"].as_array().unwrap().iter().enumerate() {
        if f["properties"]["kind"] == "peak" {
            let c = &f["geometry"]["coordinates"];
            let ele = f["properties"]["ele"].as_f64().map(|v| v as f32).unwrap_or(f32::NAN);
            list.push(Peak { i, lon: c[0].as_f64().unwrap(), lat: c[1].as_f64().unwrap(), ele });
        }
    }
    // PEAKS_BBOX=minlon,minlat,maxlon,maxlat limits the run (for testing; writes peaks-test.json).
    let bbox: Option<Vec<f64>> = std::env::var("PEAKS_BBOX").ok().map(|s| s.split(',').filter_map(|v| v.parse().ok()).collect());
    if let Some(b) = &bbox {
        list.retain(|p| p.lon >= b[0] && p.lat >= b[1] && p.lon <= b[2] && p.lat <= b[3]);
    }
    eprintln!("{} peaks", list.len());
    let arc = Archive::open(&dir.join("terrain.tiles"))?;
    let z8 = peaks::coarse_tiles(&arc);
    eprintln!("coarse grid: {} z{} tiles", z8.len(), peaks::COARSE_Z);
    let outs = peaks::run(&list, &ArchiveTiles(&arc), &z8)?;
    let name = if bbox.is_some() { "peaks-test.json" } else { "peaks.json" };
    std::fs::write(roadcore::tmp(&dir, name), serde_json::to_vec(&outs.iter().map(peaks::Out::json).collect::<Vec<_>>())?)?;
    roadcore::commit(&dir, &[name])?;
    let p100 = outs.iter().filter(|o| o.p >= 100.0).count();
    let p300 = outs.iter().filter(|o| o.p >= 300.0).count();
    eprintln!("peaks: {} done, {p100} with ≥ 100 m prominence, {p300} with ≥ 300 m ({:.0?})", outs.len(), t0.elapsed());
    Ok(())
}
