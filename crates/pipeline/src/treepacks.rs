//! The tree cover layers (docs/plan.md §6): tree cover, canopy height and leaf type, zoom 4–12, per
//! z3 tile the coverage meets, clipped to it (`dem/trees.py --z3`: Meta's canopy squares, kept on the
//! NAS, `sources/canopy/`, each downloaded once, and copied into the agent's cache the units read
//! too; the leaf-type squares on the NAS, `sources/trees/leaf/`, made whole by `dem/leaftype.py`
//! where missing), packed as the layers `trees-cover`, `trees-height` and `trees-leaf` (a lo pack per
//! z3 tile, hi packs per z6 tile). A z3 tile's run makes all of its packs: those it no longer has
//! (the coverage there shrank) leave the manifest, and a z3 tile the coverage has left loses them
//! all.

use crate::coverage::Coverage;
use crate::legacy::Unit;
use crate::out::Out;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Bumped when how the layers are made changes (every z3 tile is made again).
pub const TREES_V: u32 = 2;
pub const LAYERS: [&str; 3] = ["trees-cover", "trees-height", "trees-leaf"];

/// The leaf-type squares on the NAS (`lat<top>_lon<left>.tif`).
pub fn leaf_dir(root: &Path) -> PathBuf {
    root.join("sources/trees/leaf")
}

/// The z3 tiles the coverage meets, each with its key: the step's version and the coverage there
/// (Meta's canopy and the leaf-type sources are fixed datasets); and the z3 tiles with tree packs
/// (`m`, the build manifest) the coverage no longer meets, whose run drops them.
pub fn targets(cov: &Coverage, m: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for x in 0..8 {
        for y in 0..8 {
            let b = crate::hipack::tile_bounds(3, x, y);
            let t = format!("3/{x}/{y}");
            let had = || LAYERS.iter().any(|l| m.contains_key(&format!("layers/{l}/lo/3-{x}-{y}")) || m.range(format!("layers/{l}/hi/6-")..).take_while(|(k, _)| k.starts_with(&format!("layers/{l}/hi/6-"))).any(|(k, _)| Unit::parse(&k[format!("layers/{l}/hi/").len()..]).is_some_and(|u| (u.x >> 3, u.y >> 3) == (x, y))));
            if cov.meets_rect(b) {
                out.push((t.clone(), store::naming::hash16(format!("trees {TREES_V}|{t}|{}", cov.fingerprint(b)).as_bytes())));
            } else if had() {
                out.push((t.clone(), store::naming::hash16(format!("trees {TREES_V}|{t}|none").as_bytes())));
            }
        }
    }
    out
}

/// The coverage's shapes inside z3 tile `q`, for trees.py: each shape's rings whose box meets the
/// tile, in degrees (inside by even–odd, as the shape has them; a ring that doesn't meet the tile
/// can't change which of its points are inside).
fn coverage_json(cov: &Coverage, q: Unit) -> serde_json::Value {
    let b = crate::hipack::tile_bounds(q.z, q.x, q.y);
    let deg = |v: i32| v as f64 * 1e-7;
    let shapes: Vec<Vec<Vec<[f64; 2]>>> = cov
        .shapes
        .iter()
        .map(|s| {
            s.rings
                .iter()
                .filter(|r| {
                    let (mut w, mut so, mut e, mut n) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                    for p in r.iter() {
                        (w, so, e, n) = (w.min(p[0]), so.min(p[1]), e.max(p[0]), n.max(p[1]));
                    }
                    w <= b[2] && e >= b[0] && so <= b[3] && n >= b[1]
                })
                .map(|r| r.iter().map(|p| [deg(p[0]), deg(p[1])]).collect())
                .collect()
        })
        .filter(|rings: &Vec<Vec<[f64; 2]>>| !rings.is_empty())
        .collect();
    serde_json::json!({ "shapes": shapes })
}

/// Makes z3 tile `q`'s tree layers and uploads them, dropping its packs it no longer makes (all of
/// them when the coverage has left it).
pub fn build(out: &mut Out, cov: &Coverage, q: Unit, dem: &Path, chm: &Path, scratch: &Path, workers: usize) -> Result<()> {
    let dir = scratch.join(format!("trees-{}", q.dash()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let cj = coverage_json(cov, q);
    if cj["shapes"].as_array().is_none_or(|s| s.is_empty()) {
        drop_packs(out, q, &crate::layers::LayerOut::default())?;
        std::fs::remove_dir_all(&dir).ok();
        return Ok(());
    }
    std::fs::write(dir.join("coverage.json"), serde_json::to_vec(&cj)?)?;
    let st = std::process::Command::new("uv")
        .current_dir(dem)
        .args(["run", "python", "trees.py", "--z3", &format!("{},{}", q.x, q.y), "--coverage"])
        .arg(dir.join("coverage.json"))
        .arg("--chm")
        .arg(chm)
        .arg("--chm-store")
        .arg(out.root().join("sources/canopy"))
        .arg("--leaf")
        .arg(leaf_dir(out.root()))
        .arg("--out")
        .arg(&dir)
        .args(["--workers", &workers.to_string()])
        .status()
        .context("run trees.py")?;
    anyhow::ensure!(st.success(), "trees.py for {}: {st}", q.slash());
    for layer in LAYERS {
        let arc = roadcore::archive::Archive::open(&dir.join(format!("{layer}.tiles")))?;
        let made = crate::layers::split_archive(out, &arc, layer, "terrarium-webp", false, 12)?;
        let gone = drop_layer_packs(out, q, layer, &made)?;
        eprintln!("trees {}: {layer}: {} lo and {} hi packs, {gone} dropped", q.slash(), made.lo.len(), made.hi.len());
    }
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// Drops z3 tile `q`'s packs of every tree layer that `made` doesn't have.
fn drop_packs(out: &mut Out, q: Unit, made: &crate::layers::LayerOut) -> Result<()> {
    for layer in LAYERS {
        let n = drop_layer_packs(out, q, layer, made)?;
        eprintln!("trees {}: {layer}: the coverage has left; {n} packs dropped", q.slash());
    }
    Ok(())
}

/// Drops z3 tile `q`'s packs of `layer` (its lo pack, its z6 tiles' hi packs) that `made` doesn't
/// have; how many.
fn drop_layer_packs(out: &mut Out, q: Unit, layer: &str, made: &crate::layers::LayerOut) -> Result<usize> {
    let mut gone = Vec::new();
    let lo = format!("layers/{layer}/lo/{}", q.dash());
    if !made.lo.contains_key(&q.slash()) && out.get(&lo).is_some() {
        gone.push(lo);
    }
    for x in q.x * 8..(q.x + 1) * 8 {
        for y in q.y * 8..(q.y + 1) * 8 {
            let hi = format!("layers/{layer}/hi/6-{x}-{y}");
            if !made.hi.contains_key(&format!("6/{x}/{y}")) && out.get(&hi).is_some() {
                gone.push(hi);
            }
        }
    }
    for l in &gone {
        out.remove(l);
    }
    out.save()?;
    Ok(gone.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;

    fn cov(outline: &str) -> Coverage {
        let d = tempfile::tempdir().unwrap();
        Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec![outline.into()] }], None, d.path()).unwrap()
    }

    #[test]
    fn the_z3_tiles_the_coverage_meets() {
        // Reykjavik's 20 km circle: in z3 tile 3/3/2 only.
        let c = cov("place:-21.9,64.13,20");
        let m = std::collections::BTreeMap::new();
        let t = targets(&c, &m);
        assert_eq!(t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2"]);
        // The coverage there is the key: a bigger circle, another key.
        assert_ne!(targets(&cov("place:-21.9,64.13,25"), &m)[0].1, t[0].1);
        assert_eq!(targets(&cov("place:-21.9,64.13,20"), &m)[0].1, t[0].1);
        // Tree packs where the coverage no longer is (a hi pack of z3 tile 3/4/2): its run drops them.
        let m: std::collections::BTreeMap<String, String> = [("layers/trees-cover/hi/6-33-23".to_string(), "x".to_string())].into();
        let t = targets(&c, &m);
        assert_eq!(t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2", "3/4/2"]);
    }

    #[test]
    fn a_z3_tiles_shapes_for_trees_py() {
        let c = cov("place:-21.9,64.13,20");
        let j = coverage_json(&c, Unit { z: 3, x: 3, y: 2 });
        let rings = j["shapes"][0].as_array().unwrap();
        assert_eq!(rings.len(), 1);
        let p = &rings[0][0];
        assert!((p[0].as_f64().unwrap() + 21.9).abs() < 1.0 && (p[1].as_f64().unwrap() - 64.13).abs() < 1.0);
        // Another z3 tile: no shape there.
        assert!(coverage_json(&c, Unit { z: 3, x: 0, y: 0 })["shapes"].as_array().unwrap().is_empty());
    }
}

