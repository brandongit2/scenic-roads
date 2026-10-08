//! The shoreline check's reference (tools/coastcheck/README.md): the water at full detail, and its
//! exact coverage served as raster tiles for the map to draw from the same camera as the app.
//!
//!   coastcheck build --set <water set .osm.pbf> --water-polygons <zip> --out <store folder>
//!                    [--index sparse_mem_array|sparse_file_array,<file>]
//!   coastcheck serve --store <store folder> [--port 18095]
//!   coastcheck tile --store <store folder> <z> <x> <y> [--size 512] > tile.png
//!
//! `build` reads every water area the basemap draws from the pass's `water` set (osmium export, its
//! areas assembled) and every ring of the sea's water polygons into a store (pipeline::watercov).
//! `serve` answers `/cov/{z}/{x}/{y}?s=<size>&k=<all|sea|inland>` (size 1–2048, 512 by default)
//! with a grey PNG, each pixel 255 × (1 − its water coverage): white land, black water, exact
//! anti-aliasing between; with `&raw=1`, the shares as the server's water tiles give them raw (red
//! the sea's, green the inland water's), for the coastal shading's reference.

use anyhow::{bail, Context, Result};
use pipeline::watercov::{self as wc, Geom, GeomWriter};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") => build(&args),
        Some("serve") => serve(&args),
        Some("stats") => stats(&args),
        Some("water-packs") => {
            // water-packs --tiles <dir> --catalog <n.json.zst> --root <dir>: water-build's tiles as
            // the water layer's packs (pipeline::layers' scopes) in a test root, and a catalog one
            // newer than --catalog with them (written into <root>/catalog). Never the NAS's.
            let tiles = PathBuf::from(opt(&args, "--tiles").context("--tiles")?);
            let root = PathBuf::from(opt(&args, "--root").context("--root")?);
            let mut cat = store::catalog::read(&PathBuf::from(opt(&args, "--catalog").context("--catalog")?))?;
            let mut all: Vec<(u8, u32, u32, PathBuf)> = Vec::new();
            for z in 0..=pipeline::water::STORED_MAXZ {
                let zd = tiles.join(z.to_string());
                let Ok(xs) = std::fs::read_dir(&zd) else { continue };
                for xe in xs {
                    let xe = xe?;
                    let x: u32 = xe.file_name().to_string_lossy().parse()?;
                    for ye in std::fs::read_dir(xe.path())? {
                        let ye = ye?;
                        let y: u32 = ye.file_name().to_string_lossy().trim_end_matches(".png").parse()?;
                        all.push((z, x, y, ye.path()));
                    }
                }
            }
            all.sort_by_key(|t| (t.0, t.1, t.2));
            let mut groups: std::collections::BTreeMap<(&str, u8, u32, u32), Vec<(u8, u32, u32, PathBuf)>> = Default::default();
            for t in all {
                let (scope, pz, px, py) = pipeline::layers::pack_of(t.0, t.1, t.2);
                groups.entry((scope, pz, px, py)).or_default().push(t);
            }
            let mut layer = store::catalog::Layer { encoding: "water-png".into(), minzoom: 0, maxzoom: pipeline::water::STORED_MAXZ, ..Default::default() };
            for ((scope, pz, px, py), ts) in groups {
                let logical = format!("layers/water/{scope}/{pz}-{px}-{py}");
                let tmp = root.join(format!("{logical}.tmp"));
                std::fs::create_dir_all(tmp.parent().unwrap())?;
                let mut w = store::pack::PackWriter::create(&tmp, serde_json::json!({"layer": "water", "scope": scope, "root": format!("{pz}/{px}/{py}"), "encoding": "water-png"}), false)?;
                for (z, x, y, p) in ts {
                    let b = std::fs::read(p)?;
                    w.add(z, x, y, &b, b.len() as u32)?;
                }
                w.finish()?;
                let content = store::naming::content_name(&logical, &store::naming::hash16_file(&tmp)?, "pack");
                std::fs::rename(&tmp, root.join(&content))?;
                cat.files.insert(logical.clone(), store::catalog::FileRef { file: content.clone(), size: std::fs::metadata(root.join(&content))?.len(), ..Default::default() });
                match scope {
                    "root" => layer.root = Some(logical),
                    "lo" => {
                        layer.lo.insert(format!("{pz}/{px}/{py}"), logical);
                    }
                    _ => {
                        layer.hi.insert(format!("{pz}/{px}/{py}"), logical);
                    }
                }
            }
            cat.layers.insert("water".into(), layer);
            cat.n += 1;
            let p = store::catalog::write_copy(&root.join("catalog"), &cat)?;
            eprintln!("coastcheck: {}", p.display());
            Ok(())
        }
        Some("water-build") => {
            // water-build --basemap <pmtiles> [--only x/y,…] --out <dir>: pipeline::water's build
            // as the step runs it, its tiles written as files (<dir>/z/x/y.png), for a regional run
            // measured without the NAS.
            use pipeline::water as wt;
            let t = std::time::Instant::now();
            let pm = store::pmtiles::PmTiles::open(Box::new(store::range::PlainFile::open(&PathBuf::from(opt(&args, "--basemap").context("--basemap")?))?))?;
            let z14 = wt::Z14::read(&pm)?;
            eprintln!("coastcheck: z14 directory read in {:.0} s", t.elapsed().as_secs_f64());
            let only: Option<Vec<(u32, u32)>> = opt(&args, "--only").map(|o| o.split(',').filter_map(|t| t.split_once('/').and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))).collect());
            let (tiles, made) = wt::build(&pm, &z14, only.as_deref(), &|d, n| eprintln!("coastcheck: {d}/{n} z5 tiles"))?;
            let dir = PathBuf::from(opt(&args, "--out").context("--out")?);
            for (z, x, y, png) in &tiles {
                let f = dir.join(format!("{z}/{x}/{y}.png"));
                std::fs::create_dir_all(f.parent().unwrap())?;
                std::fs::write(f, png)?;
            }
            println!("{}", serde_json::to_string(&serde_json::json!({"made": made, "secs": t.elapsed().as_secs_f64()}))?);
            Ok(())
        }
        Some("mark-sea") => {
            // mark-sea --store <dir> --water-polygons <zip>: a store made before it recorded which
            // rings are the sea's (they're the first: as many as the zip gives).
            let dir = PathBuf::from(opt(&args, "--store").context("--store")?);
            let tmp = dir.join("sea-count.tmp");
            let mut w = GeomWriter::create(&tmp)?;
            let mut unzip = std::process::Command::new("/usr/bin/unzip").arg("-p").arg(opt(&args, "--water-polygons").context("--water-polygons")?).arg("water-polygons-split-3857/water_polygons.shp").stdout(std::process::Stdio::piped()).spawn()?;
            wc::read_water_polygons(std::io::BufReader::with_capacity(4 << 20, unzip.stdout.take().context("unzip")?), &mut w)?;
            anyhow::ensure!(unzip.wait()?.success(), "unzip failed");
            let (n, _) = w.finish()?;
            std::fs::remove_dir_all(&tmp)?;
            let mut meta: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("meta.json"))?)?;
            meta["sea_rings"] = n.into();
            std::fs::write(dir.join("meta.json"), serde_json::to_vec(&meta)?)?;
            eprintln!("coastcheck: {n} sea rings");
            Ok(())
        }
        Some("tile") => {
            let g = Geom::open(&PathBuf::from(opt(&args, "--store").context("--store")?))?;
            let n: Vec<u32> = args.iter().skip(1).filter_map(|a| a.parse().ok()).take(3).collect();
            let [z, x, y] = n[..] else { bail!("tile <z> <x> <y>") };
            let size = opt(&args, "--size").map_or(Ok(512), |s| s.parse())?;
            let t = std::time::Instant::now();
            let png = cov_png(&g, z as u8, x, y, size, wc::Which::All)?;
            eprintln!("{} bytes in {:.0} ms", png.len(), t.elapsed().as_secs_f64() * 1e3);
            std::io::Write::write_all(&mut std::io::stdout(), &png)?;
            Ok(())
        }
        _ => bail!("coastcheck build|serve|tile (see its doc comment)"),
    }
}

fn build(args: &[String]) -> Result<()> {
    let set = PathBuf::from(opt(args, "--set").context("--set")?);
    let zip = PathBuf::from(opt(args, "--water-polygons").context("--water-polygons")?);
    let out = PathBuf::from(opt(args, "--out").context("--out")?);
    let index = opt(args, "--index").unwrap_or_else(|| "sparse_mem_array".into());
    let mut w = GeomWriter::create(&out)?;
    let t = std::time::Instant::now();
    // The sea's water polygons.
    let mut unzip = std::process::Command::new("/usr/bin/unzip").arg("-p").arg(&zip).arg("water-polygons-split-3857/water_polygons.shp").stdout(std::process::Stdio::piped()).spawn().context("run unzip")?;
    let shp = unzip.stdout.take().context("unzip's output")?;
    let polygons = wc::read_water_polygons(std::io::BufReader::with_capacity(4 << 20, shp), &mut w);
    anyhow::ensure!(unzip.wait()?.success(), "unzip failed");
    eprintln!("coastcheck: {} water polygons in {:.0} s", polygons?, t.elapsed().as_secs_f64());
    w.end_sea();
    // The inland water.
    let cfg = out.join("export.json");
    std::fs::write(&cfg, serde_json::to_vec(&serde_json::json!({
        "attributes": {"type": false, "id": false},
        "area_tags": wc::AREA_TAGS,
        "include_tags": ["natural", "water", "waterway", "landuse", "tunnel", "covered"],
    }))?)?;
    let mut child = pipeline::osmpass::osmium()
        .args(["export", "-f", "geojsonseq", "--geometry-types=polygon", "--overwrite", "-o", "-"])
        .arg("-c")
        .arg(&cfg)
        .arg(format!("--index-type={index}"))
        .arg(&set)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("run osmium export")?;
    let stdout = child.stdout.take().context("osmium's output")?;
    let n = wc::read_export(std::io::BufReader::with_capacity(4 << 20, stdout), &mut w, &|n| {
        if n % 1_000_000 < 16384 {
            eprintln!("coastcheck: {n} areas read ({:.0} s)", t.elapsed().as_secs_f64());
        }
    });
    anyhow::ensure!(child.wait()?.success(), "osmium export failed");
    std::fs::remove_file(&cfg).ok();
    eprintln!("coastcheck: {} areas in {:.0} s", n?, t.elapsed().as_secs_f64());
    let skipped = w.skipped;
    let (rings, points) = w.finish()?;
    eprintln!("coastcheck: {rings} rings, {points} points ({skipped} left out) in {:.0} s", t.elapsed().as_secs_f64());
    Ok(())
}

/// A tile's coverage as a grey PNG: 255 × (1 − coverage).
fn cov_png(g: &Geom, z: u8, x: u32, y: u32, size: usize, which: wc::Which) -> Result<Vec<u8>> {
    if !(1..=2048).contains(&size) || z > 24 || x >= 1 << z || y >= 1 << z {
        bail!("no such tile");
    }
    let c = wc::tile_coverage_of(g, z, x, y, size, which);
    let px: Vec<u8> = c.iter().map(|&v| (255.0 * (1.0 - v)).round() as u8).collect();
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, size as u32, size as u32);
        e.set_color(png::ColorType::Grayscale);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::Fast);
        let mut w = e.write_header()?;
        w.write_image_data(&px)?;
    }
    Ok(out)
}

/// A tile's shares as the server's `/tiles/water/…?raw=1` has them: red the sea's, green the
/// inland water's (the coastal shading measures the shore from them).
fn raw_png(g: &Geom, z: u8, x: u32, y: u32, size: usize) -> Result<Vec<u8>> {
    if !(1..=2048).contains(&size) || z > 24 || x >= 1 << z || y >= 1 << z {
        bail!("no such tile");
    }
    let sea = wc::tile_coverage_of(g, z, x, y, size, wc::Which::Sea);
    let inland = wc::tile_coverage_of(g, z, x, y, size, wc::Which::Inland);
    let px: Vec<u8> = sea.iter().zip(&inland).flat_map(|(&s, &i)| [(255.0 * s).round() as u8, (255.0 * i).round() as u8, 0]).collect();
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, size as u32, size as u32);
        e.set_color(png::ColorType::Rgb);
        e.set_compression(png::Compression::Fast);
        let mut w = e.write_header()?;
        w.write_image_data(&px)?;
    }
    Ok(out)
}

fn serve(args: &[String]) -> Result<()> {
    use axum::extract::{Path as P, Query, State};
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;
    let g = Arc::new(Geom::open(&PathBuf::from(opt(args, "--store").context("--store")?))?);
    let port: u16 = opt(args, "--port").map_or(Ok(18095), |p| p.parse())?;
    eprintln!("coastcheck: {} rings; serving on 127.0.0.1:{port}", g.rings());
    async fn tile(State(g): State<Arc<Geom>>, P((z, x, y)): P<(u8, u32, String)>, Query(q): Query<std::collections::HashMap<String, String>>) -> axum::response::Response {
        let y: u32 = match y.trim_end_matches(".png").parse() {
            Ok(y) => y,
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        };
        let size = q.get("s").and_then(|s| s.parse().ok()).unwrap_or(512usize);
        let which = match q.get("k").map(String::as_str) {
            Some("sea") => wc::Which::Sea,
            Some("inland") => wc::Which::Inland,
            _ => wc::Which::All,
        };
        let raw = q.contains_key("raw");
        let r = tokio::task::spawn_blocking(move || if raw { raw_png(&g, z, x, y, size) } else { cov_png(&g, z, x, y, size, which) }).await;
        match r {
            Ok(Ok(png)) => ([(header::CONTENT_TYPE, "image/png"), (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"), (header::CACHE_CONTROL, "no-store")], png).into_response(),
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }
    let app = axum::Router::new().route("/cov/{z}/{x}/{y}", axum::routing::get(tile)).with_state(g);
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    rt.block_on(async {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        axum::serve(l, app).await?;
        anyhow::Ok(())
    })?;
    let _ = std::io::stdin().read(&mut [0u8; 1]);
    Ok(())
}

/// stats --store <dir> --z <zoom> [--sample n] [--size 512]: the tiles at a zoom some shore passes
/// through (every ring's edges walked), and a sample of them drawn and compressed: how big a
/// coverage layer would be.
fn stats(args: &[String]) -> Result<()> {
    use rayon::prelude::*;
    let g = Geom::open(&PathBuf::from(opt(args, "--store").context("--store")?))?;
    let z: u8 = opt(args, "--z").context("--z")?.parse()?;
    let sample: usize = opt(args, "--sample").map_or(Ok(200), |s| s.parse())?;
    let size: usize = opt(args, "--size").map_or(Ok(512), |s| s.parse())?;
    let n = 1usize << z;
    let shift = 32 - u32::from(z);
    let t = std::time::Instant::now();
    // Tiles each edge passes through (its endpoints' tiles and those between, by small steps).
    let marks: Vec<Vec<u64>> = (0..g.rings()).into_par_iter().chunks(1 << 16).map(|ids| {
        let mut out = Vec::new();
        for i in ids {
            let r = g.ring(i);
            let mut prev = r[r.len() - 1];
            for &p in r {
                let (x0, y0, x1, y1) = (prev[0] >> shift, prev[1] >> shift, p[0] >> shift, p[1] >> shift);
                if x0 == x1 && y0 == y1 {
                    out.push((y0 as u64) << 32 | x0 as u64);
                } else {
                    let steps = (x0.abs_diff(x1).max(y0.abs_diff(y1)) as u64 * 4 + 4) as f64;
                    for k in 0..=steps as u64 {
                        let f = k as f64 / steps;
                        let x = (f64::from(prev[0]) + (f64::from(p[0]) - f64::from(prev[0])) * f) as u64 >> shift;
                        let y = (f64::from(prev[1]) + (f64::from(p[1]) - f64::from(prev[1])) * f) as u64 >> shift;
                        out.push(y << 32 | x);
                    }
                }
                prev = p;
            }
            out.sort_unstable();
            out.dedup();
        }
        out
    }).collect();
    let mut all: Vec<u64> = marks.into_iter().flatten().collect();
    all.par_sort_unstable();
    all.dedup();
    eprintln!("z{z}: {} of {} tiles have shore ({:.0} s)", all.len(), n * n, t.elapsed().as_secs_f64());
    // A sample, evenly through them.
    let step = (all.len() / sample.max(1)).max(1);
    let picked: Vec<u64> = all.iter().step_by(step).copied().take(sample).collect();
    let sizes: Vec<(usize, usize, bool)> = picked.par_iter().map(|&k| {
        let (x, y) = ((k & 0xffff_ffff) as u32, (k >> 32) as u32);
        let sea = wc::tile_coverage_of(&g, z, x, y, size, wc::Which::Sea);
        let inland = wc::tile_coverage_of(&g, z, x, y, size, wc::Which::Inland);
        let all: Vec<u8> = sea.iter().zip(&inland).map(|(a, b)| (255.0 * (1.0 - (a + b).min(1.0))).round() as u8).collect();
        let uniform = all.iter().all(|&v| v == all[0]);
        let png1 = gray_png(&all, size);
        // Two channels: the sea's and the inland water's.
        let two: Vec<u8> = sea.iter().zip(&inland).flat_map(|(a, b)| [(255.0 * a).round() as u8, (255.0 * b).round() as u8]).collect();
        let png2 = {
            let mut out = Vec::new();
            let mut e = png::Encoder::new(&mut out, size as u32, size as u32);
            e.set_color(png::ColorType::GrayscaleAlpha);
            e.set_compression(png::Compression::High);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&two).unwrap();
            drop(w);
            out
        };
        (png1.len(), png2.len(), uniform)
    }).collect();
    let k = sizes.len().max(1) as f64;
    let (b1, b2) = (sizes.iter().map(|s| s.0).sum::<usize>() as f64 / k, sizes.iter().map(|s| s.1).sum::<usize>() as f64 / k);
    let uniform = sizes.iter().filter(|s| s.2).count();
    println!("{{\"z\": {z}, \"tiles\": {}, \"size\": {size}, \"sampled\": {}, \"uniform\": {uniform}, \"png1\": {:.0}, \"png2\": {:.0}, \"gb1\": {:.2}, \"gb2\": {:.2}, \"secs\": {:.0}}}", all.len(), sizes.len(), b1, b2, b1 * all.len() as f64 / 1e9, b2 * all.len() as f64 / 1e9, t.elapsed().as_secs_f64());
    Ok(())
}

fn gray_png(px: &[u8], size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, size as u32, size as u32);
    e.set_color(png::ColorType::Grayscale);
    e.set_compression(png::Compression::High);
    let mut w = e.write_header().unwrap();
    w.write_image_data(px).unwrap();
    drop(w);
    out
}
