//! The hollow test (inputs::inputs::elevation::hollow_of) run again on the accepted elevation
//! version's GeoTIFF datasets, their files read where they lie (read only), against what's
//! accepted: per dataset its time, requests and bytes; then each deep packs' tile of their presence
//! whose plan verdict (terrain::terrain_zoom::hollow) the run moves, and the tiles whose coarse
//! read found data only near their edge (`Hollow::edge`), each with what the records hold of it
//! (a deep pack named: a run found data; only its mid: a run found none; nothing: never run).
//! Task #1089.
//!
//! usage: [HOLLOW_IDS=<id>,...] [HOLLOW_PROBE=1] [HOLLOW_TRUTH=1] hollow_edges <root> <out.json> [x/y,...]
//!
//! With tiles (`x/y` at the deep packs' zoom), each dataset's presence is cut to them; with
//! `HOLLOW_IDS`, only those datasets are read again; with `HOLLOW_PROBE`, each tile's data is
//! probed (`probe`); with `HOLLOW_TRUTH`, each tile read again finer (`Hollow::edge`) has its data
//! at ~4 m inside it counted (`probe`), beside its verdict.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use store::range::RangeRead;
use store::IoError;

/// The length of (`x`, `y`).
fn len(x: f64, y: f64) -> f64 {
    (x * x + y * y).sqrt()
}

/// A file's reads, counted.
struct Counted {
    inner: Arc<dyn RangeRead>,
    reads: Arc<(AtomicU64, AtomicU64)>,
}

impl RangeRead for Counted {
    fn len(&self) -> Result<u64, IoError> {
        self.inner.len()
    }
    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        self.reads.0.fetch_add(1, Relaxed);
        self.reads.1.fetch_add(len as u64, Relaxed);
        self.inner.read_at(off, len)
    }
    fn in_memory(&self, off: u64, len: usize) -> bool {
        self.inner.in_memory(off, len)
    }
}

/// With `HOLLOW_PROBE` set: deep packs' tile `t`'s points (a 512 × 512 grid) read at each of a
/// dataset's files' levels from ~300 m to ~40 m. `probe-truth`: round the cells with data at the
/// coarsest, the ~4 m pixels with data inside the tile, and how many of the first 200 lie inside
/// the dataset's cover cut by its regions (`inside`, what a run reads). `probe-near`: each level's
/// pixels with data, and for eight, how far the nearest data at ~4 m lies and how far the tile's
/// edge (an overview's reach). `probe`: each level's points with data, inside the cut cover, and
/// their heights' range.
fn probe(
    id: &str,
    crs: u16,
    files: &[(String, Option<[f64; 4]>)],
    t: [u32; 2],
    open: &dyn Fn(&str) -> Result<buildstore::geotiff::Tiff, String>,
    inside: &dyn Fn(f64, f64) -> bool,
    verbose: bool,
) -> anyhow::Result<usize> {
    let pz = terrain::terrain_deep::PACK_Z;
    let b = roadcore::geo::tile_box(pz, t[0], t[1]);
    let atlas = inputs::dem::proj::Projection::of(crs);
    let native = |lon: f64, lat: f64| atlas.as_ref().map_or((lon, lat), |a| a.project(lon, lat));
    // (Metres a unit of the files', roughly: a geographic one's degree.)
    let unit = if atlas.is_some() { 1.0 } else { 111_320.0 };
    let n = 512;
    let pts: Vec<(f64, f64, f64, f64)> = (0..n)
        .flat_map(|j| (0..n).map(move |q| (q, j)))
        .map(|(q, j)| {
            let lon = b[0] + (q as f64 + 0.5) / n as f64 * (b[2] - b[0]);
            let lat = b[1] + (j as f64 + 0.5) / n as f64 * (b[3] - b[1]);
            let (x, y) = native(lon, lat);
            (lon, lat, x, y)
        })
        .collect();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &pts {
        (x0, y0, x1, y1) = (x0.min(p.2), y0.min(p.3), x1.max(p.2), y1.max(p.3));
    }
    let mut total = 0;
    for (w, fp) in files {
        let Some(fb) = fp else { continue };
        if !(fb[0] < x1 && x0 < fb[2] && fb[1] < y1 && y0 < fb[3]) {
            continue;
        }
        let tif = open(w).map_err(anyhow::Error::msg)?.with_cache(512 << 20);
        let nodata = tif.nodata();
        let ok =
            |v: f32| v.is_finite() && v >= inputs::dem::sample::NODATA_BELOW && nodata.is_none_or(|nd| v as f64 != nd);
        // (The points with data at the coarsest level read: each one's 256 m square at the full
        // resolution, its valid pixels counted.)
        let mut coarse_pts: Vec<(f64, f64)> = Vec::new();
        for level in (0..tif.images().len()).rev() {
            let gt = tif.transform(level)?;
            if (gt[1].abs() * unit) > 400.0 {
                continue;
            }
            let img = tif.level(level)?.clone();
            let c = |x: f64| ((x - gt[0]) / gt[1]).floor();
            let r = |y: f64| ((y - gt[3]) / gt[5]).floor();
            let (ca, cb) = (c(x0).max(0.0) as u32, (c(x1) + 1.0).min(img.width as f64) as u32);
            let (ra, rb) = (r(y1).max(0.0) as u32, (r(y0) + 1.0).min(img.height as f64) as u32);
            if ca >= cb || ra >= rb {
                break;
            }
            let px = tif.read_window(level, ca, ra, cb - ca, rb - ra)?;
            let ww = (cb - ca) as i64;
            coarse_pts = pts
                .iter()
                .filter(|p| {
                    let (cc, rr) = (c(p.2) as i64 - ca as i64, r(p.3) as i64 - ra as i64);
                    (0..ww).contains(&cc) && (0..(rb - ra) as i64).contains(&rr) && ok(px[(rr * ww + cc) as usize])
                })
                .map(|p| (p.2, p.3))
                .collect();
            break;
        }
        // (Each level's points with data: how far the nearest data at ~4 m lies, searched to 3 km,
        // and how far inside the tile they are.)
        let fine = (0..tif.images().len())
            .filter(|&l| tif.transform(l).is_ok_and(|g| g[1].abs() * unit <= 4.0))
            .max()
            .unwrap_or(0);
        let gf = tif.transform(fine)?;
        let imgf = tif.level(fine)?.clone();
        let ring: Vec<(f64, f64)> = (0..4)
            .flat_map(|side| {
                (0..64).map(move |k| {
                    let f = k as f64 / 64.0;
                    match side {
                        0 => (b[0] + f * (b[2] - b[0]), b[1]),
                        1 => (b[2], b[1] + f * (b[3] - b[1])),
                        2 => (b[2] - f * (b[2] - b[0]), b[3]),
                        _ => (b[0], b[3] - f * (b[3] - b[1])),
                    }
                })
            })
            .map(|(lon, lat)| native(lon, lat))
            .collect();
        let to_edge = |x: f64, y: f64| ring.iter().map(|p| len(p.0 - x, p.1 - y)).fold(f64::MAX, f64::min);
        let in_tile = |x: f64, y: f64| {
            let mut inside = false;
            for (i, &(xi, yi)) in ring.iter().enumerate() {
                let (xj, yj) = ring[(i + ring.len() - 1) % ring.len()];
                if (yi > y) != (yj > y) && x < xi + (y - yi) / (yj - yi) * (xj - xi) {
                    inside = !inside;
                }
            }
            inside
        };
        // (The ~4 m pixels with data round the coarsest level's: how many lie inside the tile, and
        // of those how many inside its cut cover, as a run reads it. Each 256 m cell's window, 600 m
        // wider each way.)
        let mut seen = BTreeSet::new();
        let (mut held, mut covered, mut windows) = (0usize, 0usize, 0);
        let mut seen_px = BTreeSet::new();
        for &(x, y) in &coarse_pts {
            if !seen.insert(((x * unit / 256.0).floor() as i64, (y * unit / 256.0).floor() as i64)) {
                continue;
            }
            windows += 1;
            let n = (1456.0 / (gf[1].abs() * unit)) as i64;
            let fc = ((x - gf[0]) / gf[1]).floor() as i64 - n / 2;
            let fr = ((y - gf[3]) / gf[5]).floor() as i64 - n / 2;
            let (fc0, fr0) = (fc.clamp(0, imgf.width as i64 - 1), fr.clamp(0, imgf.height as i64 - 1));
            let (w, h) = ((fc + n).min(imgf.width as i64) - fc0, (fr + n).min(imgf.height as i64) - fr0);
            let v = tif.read_window(fine, fc0 as u32, fr0 as u32, w as u32, h as u32)?;
            for j in 0..h {
                for i in 0..w {
                    if ok(v[(j * w + i) as usize]) && seen_px.insert((fc0 + i, fr0 + j)) {
                        let (fx, fy) =
                            (gf[0] + ((fc0 + i) as f64 + 0.5) * gf[1], gf[3] + ((fr0 + j) as f64 + 0.5) * gf[5]);
                        if in_tile(fx, fy) {
                            held += 1;
                            if held > 200 {
                                continue;
                            }
                            // (Its lon, lat: the nearest of the grid's points; the first 200.)
                            let p = pts
                                .iter()
                                .min_by(|a, b| len(a.2 - fx, a.3 - fy).total_cmp(&len(b.2 - fx, b.3 - fy)))
                                .unwrap();
                            covered += inside(p.0, p.1) as usize;
                        }
                    }
                }
            }
        }
        total += held;
        if !verbose {
            continue;
        }
        eprintln!(
            "probe-truth {pz}/{}/{} {id} {w}: round its {windows} cells with data at ~256 m, {held} pixels at {:.0} m with data inside the tile, of the first 200 {covered} inside its cut cover",
            t[0],
            t[1],
            (gf[1].abs() * unit)
        );
        for level in 0..tif.images().len() {
            let gt = tif.transform(level)?;
            if (gt[1].abs() * unit) > 400.0 || (gt[1].abs() * unit) < 30.0 {
                continue;
            }
            let img = tif.level(level)?.clone();
            let c = |x: f64| ((x - gt[0]) / gt[1]).floor();
            let r = |y: f64| ((y - gt[3]) / gt[5]).floor();
            let (ca, cb) = (c(x0).max(0.0) as u32, (c(x1) + 1.0).min(img.width as f64) as u32);
            let (ra, rb) = (r(y1).max(0.0) as u32, (r(y0) + 1.0).min(img.height as f64) as u32);
            if ca >= cb || ra >= rb {
                continue;
            }
            let px = tif.read_window(level, ca, ra, cb - ca, rb - ra)?;
            let ww = (cb - ca) as i64;
            let mut cells = BTreeSet::new();
            for p in &pts {
                let (cc, rr) = (c(p.2) as i64 - ca as i64, r(p.3) as i64 - ra as i64);
                if (0..ww).contains(&cc) && (0..(rb - ra) as i64).contains(&rr) && ok(px[(rr * ww + cc) as usize]) {
                    cells.insert((cc, rr));
                }
            }
            let mut dists = Vec::new();
            for &(cc, rr) in cells.iter().step_by((cells.len() / 8).max(1)).take(8) {
                let (x, y) = (
                    gt[0] + (ca as i64 + cc) as f64 * gt[1] + gt[1] / 2.0,
                    gt[3] + (ra as i64 + rr) as f64 * gt[5] + gt[5] / 2.0,
                );
                let n = (6000.0 / (gf[1].abs() * unit)) as i64;
                let fc = ((x - gf[0]) / gf[1]).floor() as i64 - n / 2;
                let fr = ((y - gf[3]) / gf[5]).floor() as i64 - n / 2;
                let (fc0, fr0) = (fc.clamp(0, imgf.width as i64 - 1), fr.clamp(0, imgf.height as i64 - 1));
                let (w, h) = ((fc + n).min(imgf.width as i64) - fc0, (fr + n).min(imgf.height as i64) - fr0);
                let v = tif.read_window(fine, fc0 as u32, fr0 as u32, w as u32, h as u32)?;
                let mut best = f64::MAX;
                for j in 0..h {
                    for i in 0..w {
                        if ok(v[(j * w + i) as usize]) {
                            let (fx, fy) = (gf[0] + (fc0 + i) as f64 * gf[1], gf[3] + (fr0 + j) as f64 * gf[5]);
                            best = best.min(len(fx - x, fy - y));
                        }
                    }
                }
                dists.push(format!("{:.0}/{:.0}", best.min(9999.0), to_edge(x, y)));
            }
            eprintln!(
                "probe-near {pz}/{}/{} {id} level {level} ({:.0} m): {} pixels with data; nearest data at {:.0} m / from the edge (m): {}",
                t[0],
                t[1],
                (gt[1].abs() * unit),
                cells.len(),
                (gf[1].abs() * unit),
                dists.join(" ")
            );
        }
        for level in 0..tif.images().len() {
            let gt = tif.transform(level)?;
            if (gt[1].abs() * unit) > 400.0 || (gt[1].abs() * unit) < 30.0 {
                continue;
            }
            let img = tif.level(level)?.clone();
            let c = |x: f64| ((x - gt[0]) / gt[1]).floor();
            let r = |y: f64| ((y - gt[3]) / gt[5]).floor();
            let (ca, cb) = (c(x0).max(0.0) as u32, (c(x1) + 1.0).min(img.width as f64) as u32);
            let (ra, rb) = (r(y1).max(0.0) as u32, (r(y0) + 1.0).min(img.height as f64) as u32);
            if ca >= cb || ra >= rb {
                continue;
            }
            let px = tif.read_window(level, ca, ra, cb - ca, rb - ra)?;
            let ww = (cb - ca) as i64;
            let (mut valid, mut held, mut lo, mut hi) = (0, 0, f32::MAX, f32::MIN);
            for p in &pts {
                let (cc, rr) = (c(p.2) as i64 - ca as i64, r(p.3) as i64 - ra as i64);
                if !(0..ww).contains(&cc) || !(0..(rb - ra) as i64).contains(&rr) {
                    continue;
                }
                let v = px[(rr * ww + cc) as usize];
                if v.is_finite() && v >= inputs::dem::sample::NODATA_BELOW && nodata.is_none_or(|nd| v as f64 != nd) {
                    valid += 1;
                    (lo, hi) = (lo.min(v), hi.max(v));
                    held += inside(p.0, p.1) as usize;
                }
            }
            eprintln!(
                "probe {pz}/{}/{} {id} {w} level {level} ({:.0} m): {valid} of {} points hold data, {held} inside its cut cover, heights {lo}..{hi}",
                t[0],
                t[1],
                (gt[1].abs() * unit),
                pts.len()
            );
        }
    }
    Ok(total)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (root, out, only) = match args.as_slice() {
        [_, r, o] => (r, o, None),
        [_, r, o, t] => (r, o, Some(t)),
        _ => anyhow::bail!("usage: hollow_edges <root> <out.json> [x/y,...]"),
    };
    let only: Option<BTreeSet<[u32; 2]>> = only
        .map(|t| {
            t.split(',')
                .map(|s| {
                    let (x, y) = s.split_once('/').ok_or_else(|| anyhow::anyhow!("{s}: not x/y"))?;
                    Ok([x.parse()?, y.parse()?])
                })
                .collect::<anyhow::Result<_>>()
        })
        .transpose()?;
    let root = store::nasfs::NasRoot::from_arg(root);
    let m = buildstore::records::read_manifest(&root)?;
    let declared = terrain::terrain_zoom::Declared::read(&root, &m)?.expect("an accepted elevation version");
    let a = &declared.elevation;
    let mut ds = inputs::dem::declared::accepted(a)?;
    let fetch = buildstore::fetch::Fetcher::from_env();
    let mut per: Vec<serde_json::Value> = Vec::new();
    let mut edges: BTreeMap<[u32; 2], Vec<String>> = BTreeMap::new();
    let mut truth: Vec<serde_json::Value> = Vec::new();
    for d in ds.iter_mut() {
        let inputs::dem::declared::Data::Geotiff { index: Some(ix), .. } = &d.declaration.data else { continue };
        let (Some(crs), Some(presence)) = (d.measured.crs, d.measured.presence.clone()) else { continue };
        if std::env::var("HOLLOW_IDS").is_ok_and(|ids| !ids.split(',').any(|i| i == d.id)) {
            continue;
        }
        let ix = inputs::dem::declared::parse_index(&a.read(ix)?).map_err(anyhow::Error::msg)?;
        let files: Vec<(String, Option<[f64; 4]>)> = ix
            .files
            .iter()
            .map(|(w, b)| match w {
                inputs::dem::declared::Where::Url(u) => Ok((u.clone(), Some(*b))),
                inputs::dem::declared::Where::File(f) => anyhow::bail!("{}: a file of its own ({f})", d.id),
            })
            .collect::<anyhow::Result<_>>()?;
        let presence: Vec<[u32; 2]> =
            presence.into_iter().filter(|t| only.as_ref().is_none_or(|o| o.contains(t))).collect();
        if presence.is_empty() {
            continue;
        }
        let reads = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        let open = |w: &str| -> Result<buildstore::geotiff::Tiff, String> {
            let src = inputs::dem::generic::open_url(&fetch, w)
                .map_err(|e| format!("{w}: {e:#}"))?
                .ok_or_else(|| format!("{w}: none"))?;
            buildstore::geotiff::Tiff::open(Arc::new(Counted { inner: src, reads: reads.clone() }))
                .map_err(|e| format!("{w}: {e:#}"))
        };
        let t0 = std::time::Instant::now();
        let h = inputs::inputs::elevation::hollow_of(crs, &files, &presence, &open).map_err(anyhow::Error::msg)?;
        let secs = t0.elapsed().as_secs_f64();
        if std::env::var_os("HOLLOW_PROBE").is_some() {
            let set = declared.sets.iter().find(|s| s.id == d.id).expect("its set");
            for t in &presence {
                probe(&d.id, crs, &files, *t, &open, &|lon, lat| set.cover.contains(lon, lat), true)?;
            }
        }
        // (With `HOLLOW_TRUTH`: each tile read again finer, its data at ~4 m inside it counted, and
        // its verdict now.)
        if std::env::var_os("HOLLOW_TRUTH").is_some() {
            let mut by: BTreeMap<String, usize> = BTreeMap::new();
            for t in &h.edge {
                let n = probe(&d.id, crs, &files, *t, &open, &|_, _| true, false)?;
                let k = format!("{}: data at ~4 m inside {}, hollow now {}", d.id, n > 0, h.tiles.contains(t));
                *by.entry(k).or_default() += 1;
                truth.push(serde_json::json!({ "id": d.id, "tile": format!("10/{}/{}", t[0], t[1]), "pixels": n, "hollow": h.tiles.contains(t) }));
            }
            eprintln!("truth {}", serde_json::to_string(&by)?);
        }
        let was: BTreeSet<[u32; 2]> = d.measured.hollow.iter().flatten().copied().collect();
        let now: BTreeSet<[u32; 2]> = h.tiles.iter().copied().collect();
        let cut: BTreeSet<[u32; 2]> = presence.iter().copied().collect();
        let line = serde_json::json!({
            "id": d.id, "presence": presence.len(), "seconds": secs,
            "requests": reads.0.load(Relaxed), "mb": reads.1.load(Relaxed) as f64 / 1e6,
            "hollow accepted": was.intersection(&cut).count(), "hollow now": now.len(),
            "newly hollow": now.difference(&was).count(), "no longer hollow": was.intersection(&cut).filter(|t| !now.contains(*t)).count(),
            "edge": h.edge.len(),
        });
        eprintln!("{line}");
        per.push(line);
        for t in &h.edge {
            edges.entry(*t).or_default().push(d.id.clone());
        }
        // (Its presence cut: the rest of its tiles as accepted.)
        let mut hollow: BTreeSet<[u32; 2]> = was.difference(&cut).copied().collect();
        hollow.extend(now);
        d.measured.hollow = Some(hollow.into_iter().collect());
    }
    let sets = terrain::terrain_zoom::sets(&ds, &|n| Ok(a.root().join(n).read()?))?;
    let pz = terrain::terrain_deep::PACK_Z;
    let held = |t: [u32; 2]| {
        if !terrain::terrain_deep::named_under(&m, pz, t[0], t[1]).is_empty() {
            "deep pack"
        } else if m.contains_key(&terrain::terrain_deep::mid_logical(t[0], t[1])) {
            "mid only"
        } else {
            "never run"
        }
    };
    let tiles: BTreeSet<[u32; 2]> = ds
        .iter()
        .flat_map(|d| d.measured.presence.iter().flatten().copied())
        .filter(|t| only.as_ref().is_none_or(|o| o.contains(t)))
        .collect();
    let mut moved = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for &t in &tiles {
        let (before, after) = (
            terrain::terrain_zoom::hollow(&declared.sets, t[0], t[1]),
            terrain::terrain_zoom::hollow(&sets, t[0], t[1]),
        );
        let e = edges.get(&t);
        if before != after || e.is_some() {
            let h = held(t);
            *counts.entry(format!("hollow {before} -> {after}, edge {}, {h}", e.is_some())).or_default() += 1;
            moved.push(serde_json::json!({
                "tile": format!("{pz}/{}/{}", t[0], t[1]), "before": before, "after": after, "edge": e, "records": h,
            }));
        }
    }
    eprintln!("{}", serde_json::to_string_pretty(&counts)?);
    std::fs::write(
        out,
        serde_json::to_vec_pretty(
            &serde_json::json!({ "datasets": per, "counts": counts, "tiles": moved, "truth": truth }),
        )?,
    )?;
    Ok(())
}
