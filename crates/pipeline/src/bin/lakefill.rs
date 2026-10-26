//! Canada's lakes missing from OSM, map sheet by map sheet (docs/steps.md §4, "Lakes filled
//! from Canada's own data"): the `lakefill` step's parts (pipeline::lakefill::run) one at a time,
//! and checks of a fill. Scratch folders only: the step puts the fill in the build.
//!
//! `lakefill survey <basemap.pmtiles> <outlines.sect> <out.csv>`: every NTS 1:50,000 sheet across
//! the Canadian regions, with the basemap's z12 water in it (`run::survey`).
//!
//! `lakefill gaps <sheets.csv> <out.csv>`: the survey's gap sheets (`run::gaps`).
//!
//! `lakefill fill --basemap <pmtiles> --sheets <gap-sheets.csv> --source <dir|zip>… --declaration <toml> [--band w,e]
//! --out <dir>`: the fill of those sheets from the lake-fill source's shapefiles (`run::fill`, as
//! `run::FillOpts::declared`, from the lake-fill source's declaration, producers/sources/canvec-lakes.toml;
//! `--no-complete-halves`, `--tol <m>`, `--confirm <share>` change its
//! rules), written to `<dir>` content-named with `report.json`.
//!
//! `lakefill merge --out <dir> <run dir>…`: bands' fills as one (`run::merge`).
//!
//! `lakefill all --basemap <pmtiles> --outlines <sect> --source <dir> --declaration <toml> --work <dir>`: all of it, as
//! the step runs it (`run::all`).
//!
//! `lakefill catalog-add --root <test root> --fill <lakefill.<hash>.sect>`: a test root's newest
//! catalog again, one newer, with the fill (copied into the root's `work/`) in its files and
//! `global` as a publish would put it, for a test server to draw. Never the NAS's.
//!
//! `lakefill stats --fill <f> --sheets <gap-sheets.csv>`: the fill's lakes and their area (km²) by
//! region (the survey's, by the sheet of each lake's first point), as JSON.
//!
//! `lakefill seamcheck --fill <f> --basemap <pmtiles> [--max n]`: where the fill's lakes were cut
//! at a sheet's edge (their edges along a sheet line) and OSM's water lies across it, the two
//! drawn at 0.5 m in a strip 30 m each side: the area both cover (doubled), and the width of the
//! band neither does between them (a sliver), per metre of edge, as JSON.
//!
//! `lakefill view …`: a box of the basemap's water with the sheets' edges, for checking by eye.

use anyhow::{Context, Result};
use clap::Parser;
use pipeline::cli::progs::{Declaration, Lakefill, LakefillCmd};
use pipeline::lakefill::{nts, run};
use pipeline::watercov::Raster;
use std::path::Path;

fn main() -> Result<()> {
    match Lakefill::parse().cmd {
        LakefillCmd::Survey { basemap, outlines, out } => {
            let rows = run::survey(&basemap, &outlines, &|d, n| eprintln!("lakefill: {d}/{n} z5 tiles"))?;
            std::fs::write(out, run::sheets_csv(&rows))?;
            Ok(())
        }
        LakefillCmd::Gaps { sheets, out } => {
            let g = run::gaps(&run::read_sheets_csv(&std::fs::read_to_string(sheets)?)?);
            eprintln!("lakefill: {} gap sheets, {:.0} km²", g.len(), g.iter().map(|v| v.area_km2).sum::<f64>());
            std::fs::write(out, run::gaps_csv(&g))?;
            Ok(())
        }
        LakefillCmd::All { basemap, outlines, source, declaration: d, work } => {
            let f = run::all(
                &basemap,
                &outlines,
                &source,
                &declaration(&d)?,
                &work,
                &|k| eprintln!("lakefill: part {k}"),
                &|_, _| {},
            )?;
            eprintln!("lakefill: {}", f.display());
            Ok(())
        }
        LakefillCmd::View { basemap, bbox, z, out } => {
            let b: Vec<f64> = bbox.split(',').map(str::parse).collect::<Result<_, _>>()?;
            anyhow::ensure!(b.len() == 4, "the box is w,s,e,n, not {bbox}");
            view(&basemap, [b[0], b[1], b[2], b[3]], z, &out)
        }
        LakefillCmd::Fill { basemap, sheets, source, declaration: d, band, no_complete_halves, tol, confirm, out } => {
            let gaps = run::read_gaps_csv(&std::fs::read_to_string(sheets)?)?;
            let mut o = run::FillOpts::declared(&basemap, &source[0], &declaration(&d)?)?;
            o.sources = source;
            if let Some(t) = tol {
                o.rules.tol_m = t;
            }
            o.rules.complete_halves = !no_complete_halves;
            if confirm.is_some() {
                o.confirm = confirm;
            }
            let band: Option<(f64, f64)> = band
                .map(|b| -> Result<(f64, f64)> {
                    let (w, e) = b.split_once(',').context("--band w,e")?;
                    Ok((w.parse()?, e.parse()?))
                })
                .transpose()?;
            run::fill(&o, &gaps, band, &out)?;
            Ok(())
        }
        LakefillCmd::CatalogAdd { root, fill } => catalog_add(&root, &fill),
        LakefillCmd::Stats { fill, sheets } => stats(&fill, &sheets),
        LakefillCmd::Seamcheck { fill, basemap, max } => seamcheck(&fill, &basemap, max),
        LakefillCmd::Merge { out, runs } => {
            run::merge(&runs, &out)?;
            Ok(())
        }
    }
}

/// The basemap's water over a box at zoom `z` (its tiles' own, 256 px a tile), as a PNG: land
/// dark, inland water blue, sea grey, with the NTS 1:50,000 sheets' edges.
fn view(base: &Path, b: [f64; 4], z: u8, out: &Path) -> Result<()> {
    let pm = store::pmtiles::PmTiles::with_leaf_cache(Box::new(store::range::PlainFile::open(base)?), 4096)?;
    let n = f64::from(1u32 << z);
    let wx = |lon: f64| (lon + 180.0) / 360.0 * n;
    let wy = |lat: f64| {
        let r = lat.to_radians();
        (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n
    };
    let (x0, x1, y0, y1) = (wx(b[0]), wx(b[2]), wy(b[3]), wy(b[1]));
    const T: usize = 256;
    let (w, h) = (((x1 - x0) * T as f64).round() as usize, ((y1 - y0) * T as f64).round() as usize);
    let (mut sea, mut inland) = (Raster::new(w, h), Raster::new(w, h));
    for ty in y0.floor() as u32..=y1.floor() as u32 {
        for tx in x0.floor() as u32..=x1.floor() as u32 {
            let Some(raw) = pm.get(z, tx, ty)? else { continue };
            let tw = pipeline::water::tile_water(&raw)?;
            let k = T as f64 / tw.extent;
            let (ox, oy) = ((f64::from(tx) - x0) * T as f64, (f64::from(ty) - y0) * T as f64);
            for (is_sea, r) in &tw.rings {
                let ras = if *is_sea { &mut sea } else { &mut inland };
                let mut prev = r[r.len() - 1];
                for &p in r {
                    ras.edge([ox + prev[0] * k, oy + prev[1] * k], [ox + p[0] * k, oy + p[1] * k]);
                    prev = p;
                }
            }
        }
    }
    let (s, i) = (sea.coverage(), inland.coverage());
    let mut px = vec![0u8; w * h * 3];
    for k in 0..w * h {
        let (sv, iv) = (s[k].min(1.0), i[k].min(1.0 - s[k].min(1.0)));
        let l = 1.0 - sv - iv;
        let mix = |land: f32, sc: f32, ic: f32| (land * l + sc * sv + ic * iv) as u8;
        px[3 * k] = mix(34.0, 90.0, 70.0);
        px[3 * k + 1] = mix(38.0, 96.0, 150.0);
        px[3 * k + 2] = mix(44.0, 104.0, 230.0);
    }
    // The sheets' edges.
    let lat_of = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    for yy in 0..h {
        let lat = lat_of(y0 + (yy as f64 + 0.5) / T as f64);
        let lat_up = lat_of(y0 + (yy as f64 - 0.5) / T as f64);
        for xx in 0..w {
            let lon = (x0 + (xx as f64 + 0.5) / T as f64) / n * 360.0 - 180.0;
            let lon_l = (x0 + (xx as f64 - 0.5) / T as f64) / n * 360.0 - 180.0;
            let (k, kl, ku) = (nts::sheet_key(lon, lat), nts::sheet_key(lon_l, lat), nts::sheet_key(lon, lat_up));
            if xx > 0 && k != kl || yy > 0 && k != ku {
                px[3 * (yy * w + xx)..3 * (yy * w + xx) + 3].copy_from_slice(&[230, 200, 60]);
            }
        }
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(out)?), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()?.write_image_data(&px)?;
    Ok(())
}

/// The lake-fill source's declaration (`--declaration <file.toml>`: one as added to
/// inputs/sources/, producers/sources/canvec-lakes.toml), as the step reads it accepted.
fn declaration(d: &Declaration) -> Result<pipeline::inputs::sources::Source> {
    let p = &d.declaration;
    let d = pipeline::inputs::sources::parse(&std::fs::read(p).with_context(|| p.display().to_string())?)
        .map_err(|why| anyhow::anyhow!("{}: {}", p.display(), why.join("; ")))?;
    let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(pipeline::inputs::sources::Source { id, data: d.data, keyed: String::new(), index: Vec::new() })
}

fn catalog_add(root: &Path, fill: &Path) -> Result<()> {
    anyhow::ensure!(!root.starts_with("/Volumes"), "a test root, not the NAS's");
    let dir = root.join("catalog");
    let n = *store::catalog::list(&dir)?.first().context("no catalog in the root")?;
    let mut cat = store::catalog::read_n(&dir, n)?;
    let name = fill.file_name().context("a fill file")?.to_string_lossy().into_owned();
    let hash = name.trim_start_matches("lakefill.").trim_end_matches(".sect");
    let logical = format!("{}test", pipeline::lakefill::PREFIX);
    let content = store::naming::content_name(&logical, hash, "sect");
    std::fs::create_dir_all(root.join(&content).parent().unwrap())?;
    std::fs::copy(fill, root.join(&content))?;
    cat.files.insert(
        logical.clone(),
        store::catalog::FileRef {
            file: content.clone(),
            size: std::fs::metadata(fill)?.len(),
            fmt: 1,
            ..Default::default()
        },
    );
    cat.global.insert("lakefill".into(), logical);
    cat.n += 1;
    let p = store::catalog::write_copy(&dir, &cat)?;
    eprintln!("lakefill: {} with {content}", p.display());
    Ok(())
}

fn stats(path: &Path, sheets: &Path) -> Result<()> {
    use pipeline::lakefill as lf;
    let f = lf::Fill::open(path, "stats")?;
    let mut region: std::collections::HashMap<nts::Key, String> = std::collections::HashMap::new();
    let mut rd = std::io::BufRead::lines(std::io::BufReader::new(std::fs::File::open(sheets)?));
    let head: Vec<String> = rd.next().context("empty")??.split(',').map(str::to_string).collect();
    let col = |n: &str| head.iter().position(|h| h == n).context("a column");
    let (cw, cs, cr) = (col("west")?, col("south")?, col("region")?);
    for l in rd {
        let l = l?;
        let v: Vec<&str> = l.split(',').collect();
        region.insert(nts::sheet_key(v[cw].parse::<f64>()? + 1e-6, v[cs].parse::<f64>()? + 1e-6), v[cr].to_string());
    }
    let mut by: std::collections::BTreeMap<String, (u64, f64, u64)> = std::collections::BTreeMap::new();
    for i in 0..f.lakes().len() as u32 {
        let l = f.lake(i);
        let b = l.bbox();
        let (_, lat) = lf::lonlat([(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]);
        let m = lf::metres_per_unit(lat);
        let km2 = l.area() * m * m / 1e6;
        let (lon0, lat0) = lf::lonlat(l.rings[0][0]);
        let r = region.get(&nts::sheet_key(lon0, lat0)).cloned().unwrap_or_else(|| "beside a gap sheet".into());
        let e = by.entry(r).or_default();
        e.0 += 1;
        e.1 += km2;
        e.2 += u64::from(l.intermittent);
    }
    // The biggest, for checking by eye.
    let mut big: Vec<(f64, f64, f64)> = (0..f.lakes().len() as u32)
        .map(|i| {
            let l = f.lake(i);
            let b = l.bbox();
            let (lon, lat) = lf::lonlat([(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]);
            let m = lf::metres_per_unit(lat);
            (l.area() * m * m / 1e6, lon, lat)
        })
        .collect();
    big.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (km2, lon, lat) in big.iter().take(15) {
        eprintln!("lakefill: {km2:.1} km² at {lat:.4}, {lon:.4}");
    }
    let out: serde_json::Map<String, serde_json::Value> = by
        .into_iter()
        .map(|(r, (n, km2, i))| {
            (r, serde_json::json!({"lakes": n, "km2": (km2 * 10.0).round() / 10.0, "intermittent": i}))
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn seamcheck(fill: &Path, basemap: &Path, max: usize) -> Result<()> {
    use pipeline::lakefill as lf;
    use pipeline::lakefill::OsmWater;
    let f = lf::Fill::open(fill, "check")?;
    let pm = store::pmtiles::PmTiles::with_leaf_cache(Box::new(store::range::PlainFile::open(basemap)?), 4096)?;
    let osm = lf::BasemapOsm::new(pm);
    // The cut edges: a lake's edge along a sheet line (north–south on a sheet's longitude, or
    // east–west on a quarter degree), at least 20 m long.
    let on_lon = |x: f64, lat: f64| {
        let lon = x * 360.0 - 180.0;
        let w = nts::width((lat * 4.0).floor() as i32);
        let k = (lon / w).round();
        (lon - k * w).abs() < 1e-7
    };
    let on_lat = |y: f64| {
        let (_, lat) = lf::lonlat([0.0, y]);
        ((lat * 4.0).round() - lat * 4.0).abs() < 1e-7
    };
    let mut cuts: Vec<(u32, [f64; 2], [f64; 2])> = Vec::new();
    for i in 0..f.lakes().len() as u32 {
        let l = f.lake(i);
        for r in &l.rings {
            for k in 0..r.len() {
                let (p, q) = (r[k], r[(k + 1) % r.len()]);
                let (_, lat) = lf::lonlat(p);
                let m = lf::metres_per_unit(lat);
                let len = ((q[0] - p[0]).hypot(q[1] - p[1])) * m;
                if len < 20.0 {
                    continue;
                }
                if (p[0] == q[0] && on_lon(p[0], lat)) || (p[1] == q[1] && on_lat(p[1])) {
                    cuts.push((i, p, q));
                }
            }
        }
    }
    eprintln!("seamcheck: {} cut edges", cuts.len());
    cuts.truncate(max);
    let boxes: Vec<[f64; 4]> = cuts
        .iter()
        .map(|(_, p, q)| {
            let (_, lat) = lf::lonlat(*p);
            let d = 30.0 / lf::metres_per_unit(lat);
            [p[0].min(q[0]) - d, p[1].min(q[1]) - d, p[0].max(q[0]) + d, p[1].max(q[1]) + d]
        })
        .collect();
    osm.prepare(&boxes)?;
    let (mut edge_m, mut with_osm_m, mut doubled_m2, mut sliver_rows, mut rows_both) =
        (0.0f64, 0.0f64, 0.0f64, 0u64, 0u64);
    let mut slivers: Vec<f64> = Vec::new();
    for ((i, p, q), b) in cuts.iter().zip(&boxes) {
        let (_, lat) = lf::lonlat(*p);
        let m = lf::metres_per_unit(lat);
        let px = 0.5 / m;
        let (w, h) = (((b[2] - b[0]) / px).ceil() as usize, ((b[3] - b[1]) / px).ceil() as usize);
        if w * h > 4_000_000 {
            continue;
        }
        let draw = |rings: &[Vec<[f64; 2]>]| {
            let mut r = pipeline::watercov::Raster::new(w, h);
            for ring in rings {
                let mut prev = ring[ring.len() - 1];
                for &pt in ring {
                    r.edge([(prev[0] - b[0]) / px, (prev[1] - b[1]) / px], [(pt[0] - b[0]) / px, (pt[1] - b[1]) / px]);
                    prev = pt;
                }
            }
            r.coverage()
        };
        let lake = draw(&f.lake(*i).rings);
        let water = osm.rings_near(*b)?;
        let o = draw(&water);
        let len = (q[0] - p[0]).hypot(q[1] - p[1]) * m;
        edge_m += len;
        let both: f64 = lake.iter().zip(&o).map(|(a, c)| f64::from(a.min(*c))).sum::<f64>() * 0.25;
        // Across the edge, row by row (columns for an east–west edge): where the lake is water on
        // its side and OSM's on the other, the band between with neither.
        let vertical = p[0] == q[0];
        let (n_rows, n_cols) = if vertical { (h, w) } else { (w, h) };
        let at = |v: &[f32], row: usize, col: usize| if vertical { v[row * w + col] } else { v[col * w + row] };
        let mut any = false;
        for row in 0..n_rows {
            let lw: Vec<bool> = (0..n_cols).map(|c| at(&lake, row, c) > 0.5).collect();
            let ow: Vec<bool> = (0..n_cols).map(|c| at(&o, row, c) > 0.5).collect();
            let (Some(l0), Some(l1)) = (lw.iter().position(|&x| x), lw.iter().rposition(|&x| x)) else { continue };
            let (Some(o0), Some(o1)) = (ow.iter().position(|&x| x), ow.iter().rposition(|&x| x)) else { continue };
            any = true;
            rows_both += 1;
            // The band between the lake's side and OSM's (in px), or none where they meet.
            let gap = if l1 < o0 {
                (o0 - l1 - 1) as f64
            } else if o1 < l0 {
                (l0 - o1 - 1) as f64
            } else {
                0.0
            };
            if gap > 0.0 {
                sliver_rows += 1;
                slivers.push(gap * 0.5);
            }
        }
        if any {
            with_osm_m += len;
            doubled_m2 += both;
        }
    }
    slivers.sort_by(f64::total_cmp);
    let pct = |p: f64| slivers.get(((slivers.len() as f64 - 1.0) * p) as usize).copied().unwrap_or(0.0);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "cut_edges": cuts.len(), "edge_m": edge_m.round(), "edge_m_with_osm_across": with_osm_m.round(),
            "doubled_m2": doubled_m2.round(), "doubled_m2_per_m": if with_osm_m > 0.0 { doubled_m2 / with_osm_m } else { 0.0 },
            "rows_with_both": rows_both, "rows_with_a_sliver": sliver_rows,
            "sliver_m": {"p50": pct(0.5), "p90": pct(0.9), "max": pct(1.0)}
        }))?
    );
    Ok(())
}
