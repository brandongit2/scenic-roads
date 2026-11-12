//! The hollow deep targets on the build's real records (read only): those settled unrun
//! (`TerrainTargets::deep_hollow`, task #842) and those with outputs an earlier run left
//! (`TerrainTargets::deep_hollow_named`), with what the plan gives each of their two steps now and
//! once their keys move (pipeline::agent::build::hollow_drops, task #1087: a drop, never a run),
//! and what its z6 tile's terrain-hi and slope-hi then wait for. Under a footprint guard.
//!
//! usage: hollow_drops <root> <pack-idx copy> <max MB> <out.json>
//!
//! `<pack-idx copy>`: a folder the terrain packs' indexes are read into and kept (never an
//! agent's own), as the checklist example's.

use agentcore::pool::records::Keys;
use pipeline::agent::build::{deep_outputs_named, hollow_drops, is_none_key, terrain_work, KeyCtx};
use pipeline::agent::tiles::TerrainTiles;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root, idx, max, out] = args.as_slice() else {
        anyhow::bail!("usage: hollow_drops <root> <pack-idx copy> <max MB> <out.json>")
    };
    jobrt::sys::footprint_guard("hollow_drops", max.parse()?);
    let nas = store::nasfs::NasRoot::from_arg(root);
    let m = buildstore::records::read_manifest(&nas)?;
    let date = inputs::osmpass::latest_pass_by(&nas, false).ok_or_else(|| anyhow::anyhow!("no complete pass"))?;
    let keys = Keys::load(&nas);
    let a = inputs::inputs::regions::read::source(&nas, &m)?;
    let cov = inputs::inputs::regions::read::accepted_coverage(&nas, &a, None)?;
    let reach = roads::reach::Reaches::load(&nas, &m, &date).ok().flatten();
    let mut tiles = TerrainTiles::new(Some(PathBuf::from(idx)));
    tiles.load(&nas, &m);
    let inputs = pipeline::agent::theplan::the_plan().key_inputs(&nas, &m);
    let x = KeyCtx::new(&cov, &date, &m, &inputs, reach.as_ref(), &tiles);
    let tt = pipeline::agent::build::terrain_slope_targets_made(&x);
    // (Each step's key for the hollow ones with outputs named, under the records `done`: "run",
    // "drop" or "current".)
    let said = |done: &Keys| -> Vec<serde_json::Value> {
        let tw = terrain_work(&tt, &x, done);
        let now = hollow_drops(&tt, done);
        tt.deep_hollow_named
            .iter()
            .map(|t| {
                let kind = |step: &str, k: Option<&String>, offered: &[(String, String)]| match offered
                    .iter()
                    .find(|o| o.0 == *t)
                {
                    Some((_, k)) if is_none_key(step, t, k) => "drop",
                    Some(_) => "run",
                    None => match k {
                        Some(k) if is_none_key(step, t, k) => "current as a drop",
                        _ => "current",
                    },
                };
                let td = now.terrain_deep.iter().find(|p| p.0 == *t).map(|p| &p.1);
                let sd = now.slope_deep.iter().find(|p| p.0 == *t).and_then(|p| p.1.as_ref());
                let u = codecs::legacy::TileId::parse(t).expect("a z10 tile");
                let k = terrain::terrain_deep::PACK_Z - 6;
                let z6 = format!("6/{}/{}", u.x >> k, u.y >> k);
                serde_json::json!({
                    "terrain-hi offered": tw.terrain.iter().any(|p| p.0 == z6),
                    "slope-hi offered": tw.slope.iter().any(|p| p.0 == z6),
                    "target": t,
                    "terrain-deep": kind("terrain-deep", td, &tw.terrain_deep),
                    "slope-deep": kind("slope-deep", sd, &tw.slope_deep),
                    "terrain-deep left": tw.terrain_deep_left.contains(t),
                    "slope-deep left": tw.slope_deep_left.contains(t),
                    "z6": z6,
                })
            })
            .collect()
    };
    let named: Vec<serde_json::Value> = tt
        .deep_hollow_named
        .iter()
        .map(|t| {
            let u = codecs::legacy::TileId::parse(t).expect("a z10 tile");
            let outputs: Vec<&String> = [
                terrain::terrain_deep::mid_logical(u.x, u.y),
                terrain::slope_pack::deep_logical(u.x, u.y),
                terrain::slope_pack::deep_mid_logical(u.x, u.y),
            ]
            .into_iter()
            .chain(terrain::terrain_deep::named_under(&m, terrain::terrain_deep::PACK_Z, u.x, u.y))
            .filter_map(|l| m.get_key_value(&l).map(|(l, _)| l))
            .collect();
            assert!(deep_outputs_named(&m, (u.x, u.y)));
            serde_json::json!({ "target": t, "outputs": outputs })
        })
        .collect();
    // (Their keys moved, as a version bump moves them: every record of theirs another key.)
    let mut moved = keys.clone();
    for t in &tt.deep_hollow_named {
        for r in [&mut moved.terrain_deep, &mut moved.slope_deep] {
            if let Some(k) = r.get_mut(t) {
                k.push_str(" moved");
            }
        }
    }
    let (now, then) = (said(&keys), said(&moved));
    let count = |v: &[serde_json::Value], step: &str, kind: &str| v.iter().filter(|e| e[step] == kind).count();
    eprintln!(
        "terrain-deep targets: {}; hollow, never run: {}; hollow with outputs named: {} (now: {} runs, {} drops; their keys moved: terrain-deep {} runs, {} drops, slope-deep {} runs, {} drops)",
        tt.terrain_deep.len(),
        tt.deep_hollow.len(),
        tt.deep_hollow_named.len(),
        count(&now, "terrain-deep", "run") + count(&now, "slope-deep", "run"),
        count(&now, "terrain-deep", "drop") + count(&now, "slope-deep", "drop"),
        count(&then, "terrain-deep", "run"),
        count(&then, "terrain-deep", "drop"),
        count(&then, "slope-deep", "run"),
        count(&then, "slope-deep", "drop"),
    );
    std::fs::write(
        out,
        serde_json::to_vec_pretty(&serde_json::json!({
            "hollow": tt.deep_hollow, "named": named, "now": now, "moved": then,
        }))?,
    )?;
    Ok(())
}
