//! What task #929's terrain rule re-keys and rebuilds on the build's real records (read only): the
//! terrain and slope targets by step, the tiles each rule makes and how (sampled, averaged, relief),
//! the deep targets before and after, and each declared dataset's finest zoom by latitude band. The
//! rule before it is restated here (its latitude cap and its "finer than the base" sets) only to
//! count against.
//!
//! usage: cargo run -p pipeline --example terrain_impact -- <root>

use pipeline::terrain_pack::near_coverage;
use pipeline::terrain_zoom::{finest, Set};
use std::collections::BTreeMap;

/// The latitude cap before (z12 to 67°, z11 to 79°, z10 beyond).
fn cap_before(lat: f64) -> u8 {
    match lat.abs() {
        l if l <= 67.0 => 12,
        l if l <= 79.0 => 11,
        _ => 10,
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root] = args.as_slice() else { anyhow::bail!("usage: terrain_impact <root>") };
    let root = store::nasfs::NasRoot::from_arg(root);
    let m = pipeline::out::read_manifest(&root)?;
    let src = pipeline::inputs::regions::read::source(&root, &m)?;
    let cov = pipeline::inputs::regions::read::accepted_coverage(&root, &src, None)?;
    let declared = pipeline::terrain_zoom::Declared::read(&root, &m)?.expect("an accepted elevation version");
    let sets: &[Set] = &declared.sets;

    // (The rule before: datasets finer than the worldwide ones (rank above 0) by a tenth, each to the
    // finest zoom whose pixels are no finer than half its resolution, a pyramid to its own.)
    let ds = pipeline::dem::declared::accepted(&declared.elevation)?;
    let base = ds
        .iter()
        .filter(|d| d.declaration.rank.unwrap_or(0) > 0)
        .map(|d| d.measured.resolution)
        .fold(f64::INFINITY, f64::min);
    let old_zoom = |d: &pipeline::dem::declared::Accepted, lat: f64| -> u8 {
        if let pipeline::dem::declared::Data::Tiles { zoom, .. } = &d.declaration.data {
            return (*zoom).min(16) as u8;
        }
        let mut z = 0u8;
        while z < 16 && pipeline::dem::generic::tile_resolution(z as u32 + 1, lat) >= d.measured.resolution / 2.0 - 1e-9
        {
            z += 1;
        }
        z
    };
    let fine: Vec<(&pipeline::dem::declared::Accepted, &Set)> = ds
        .iter()
        .filter(|d| d.measured.resolution < base * 0.9)
        .filter_map(|d| sets.iter().find(|s| s.id == d.id).map(|s| (d, s)))
        .collect();
    // Each dataset's finest zoom by band, before and after.
    println!("## Each dataset's finest zoom by latitude: after | before (a fine one's own; the others, the cap)");
    let bands = |f: &dyn Fn(f64) -> u8| -> String {
        let mut by: BTreeMap<u8, (f64, f64)> = BTreeMap::new();
        for l in (0..=85).map(|l| l as f64) {
            let e = by.entry(f(l)).or_insert((l, l));
            e.0 = e.0.min(l);
            e.1 = e.1.max(l);
        }
        by.iter().rev().map(|(z, (a, b))| format!("z{z} {a:.0}–{b:.0}°")).collect::<Vec<_>>().join(", ")
    };
    for s in sets {
        let d = ds.iter().find(|d| d.id == s.id).expect("its declaration");
        let was_fine = d.measured.resolution < base * 0.9;
        println!(
            "{:<22} cell {:?}: {} | {}",
            s.id,
            s.detail.cell_at(45.0),
            bands(&|l| s.detail.zoom_at(l)),
            if was_fine { bands(&|l| old_zoom(d, l)) } else { bands(&cap_before) }
        );
    }
    println!("cap before: z12 0–67°, z11 67–79°, z10 79–85°");
    println!("polar finest after: z{} (before z9)", pipeline::polar::finest(sets, roadcore::polar::Pole::South));

    // The z6 tiles near the coverage and their levels.
    let z6: Vec<(u32, u32)> = pipeline::agent::build::coverage_tiles(&cov).into_values().flatten().collect();
    let z3: std::collections::BTreeSet<(u32, u32)> = z6.iter().map(|t| (t.0 >> 3, t.1 >> 3)).collect();
    let (mut before, mut after, mut deep_avg, mut deep_before, mut deep_after, mut makes) =
        (BTreeMap::<u8, usize>::new(), BTreeMap::<u8, usize>::new(), 0usize, 0usize, 0usize, 0usize);

    for &t in &z6 {
        for z in 9..=12u8 {
            let s = 1u32 << (z - 6);
            for x in t.0 * s..(t.0 + 1) * s {
                for y in t.1 * s..(t.1 + 1) * s {
                    if !near_coverage(&cov, z, x, y) {
                        continue;
                    }
                    if z <= cap_before(pipeline::terrain_pack::tile_lat(z, y)) {
                        *before.entry(z).or_default() += 1;
                    }
                    if z <= finest(sets, z, x, y) {
                        *after.entry(z).or_default() += 1;
                        if z == 12 && finest(sets, 12, x, y) >= 13 {
                            deep_avg += 1;
                        }
                    }
                }
            }
        }
        makes += pipeline::terrain_pack::makes_hi(&cov, sets, t) as usize;
        let k = 4;
        for x in t.0 << k..(t.0 + 1) << k {
            for y in t.1 << k..(t.1 + 1) << k {
                let b = roadcore::geo::tile_box(10, x, y);
                let lat = b[1].abs().max(b[3].abs());
                if near_coverage(&cov, 10, x, y) && fine.iter().any(|(d, s)| s.cover.meets(b) && old_zoom(d, lat) >= 13)
                {
                    deep_before += 1;
                }
            }
        }
        deep_after += pipeline::terrain_deep::targets_in(sets, &cov, t).len();
    }
    println!("\n## Targets (every key bumps: TERRAIN_V 8, TERRAIN_DEEP_V 2, TERRAIN_LO_V 3, SLOPE_V 3, polar 3/2/3)");
    println!("terrain-hi and slope-hi z6 tiles: {} ({} making hi tiles)", z6.len(), makes);
    println!("terrain-lo and slope-lo z3 tiles: {}", z3.len());
    println!(
        "terrain-deep and slope-deep z10 tiles: {deep_before} before (fine sets past the cap), {deep_after} after"
    );
    println!("\n## Hi tiles near the coverage by zoom (before → after)");
    for z in 9..=12u8 {
        println!("z{z}: {} → {}", before.get(&z).unwrap_or(&0), after.get(&z).unwrap_or(&0));
    }
    println!("z12 tiles averaged from deep tiles (B): {deep_avg}");
    // The lo and root tiles: their quarters outside every region now the relief, never sampled.
    let (mut lo_tiles, mut lo_relief) = (0usize, 0usize);
    for &q in &z3 {
        for z in 3..=8u8 {
            let s = 1u32 << (z - 3);
            for x in q.0 * s..(q.0 + 1) * s {
                for y in q.1 * s..(q.1 + 1) * s {
                    lo_tiles += 1;
                    if (0..4u32).any(|k| !near_coverage(&cov, z + 1, 2 * x + (k & 1), 2 * y + (k >> 1))) {
                        lo_relief += 1;
                    }
                }
            }
        }
    }
    let root_relief = (0..3u8)
        .flat_map(|z| (0..1u32 << z).flat_map(move |x| (0..1u32 << z).map(move |y| (z, x, y))))
        .filter(|&(z, x, y)| (0..4u32).any(|k| !near_coverage(&cov, z + 1, 2 * x + (k & 1), 2 * y + (k >> 1))))
        .count();
    println!("\n## Lo and root");
    println!(
        "lo tiles: {lo_tiles}, {lo_relief} with a quarter outside every region (the relief's, was the pool sampled)"
    );
    println!("root tiles: 21, {root_relief} with a quarter the relief's; z3 tiles outside the lo packs no longer sampled: {}", 64 - z3.len());
    // Road spacing by band.
    println!("\n## Road spacing (dem::spacing): the worldwide row's zoom by latitude, before → after");
    let f = pipeline::dem::spacing::Fine::of_accepted(&ds);
    let mut last = None;
    for y in (-60..85).step_by(1) {
        let b = cap_before((y as f64).abs().max((y as f64 + 1.0).abs()));
        let a = f.row_zoom(y);
        if last != Some((b, a)) {
            println!(
                "from {y}°: z{b} → z{a} ({:.1} m → {:.1} m)",
                pipeline::dem::spacing::spacing_at(b, y as f64),
                pipeline::dem::spacing::spacing_at(a, y as f64)
            );
            last = Some((b, a));
        }
    }
    println!("cells with a finer zoom: {}", f.cells.len());
    Ok(())
}
