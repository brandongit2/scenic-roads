//! Loads a translations folder and reports how long it took and what it holds, then times
//! `attach` over label tiles.
//!
//! usage: cargo run --release -p names --example load [<translations folder>] [<labels.tiles>]

use names::mvt::{self, LABELS};
use names::{Kind, Names};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output();
    out.ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| s.trim().parse::<f64>().ok()).map_or(f64::NAN, |kb| kb / 1024.0)
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "todo" {
            continue;
        }
        if p.is_dir() {
            files(&p, out);
        } else if name.ends_with(".jsonl") {
            out.push(p);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).map_or("/Volumes/personal/projects/scenic-roads/translations", String::as_str));
    let labels = args.get(2).map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/build/labels.tiles"));

    let before = rss_mb();
    let t0 = Instant::now();
    let mut names = Names::load(&dir)?;
    let load = t0.elapsed();
    let after = rss_mb();
    println!("load: {} names in {load:.2?} (heap {:.0} MB; process RSS {before:.0} → {after:.0} MB)", names.entries(), names.heap_bytes() as f64 / 1048576.0);
    for a in names.areas() {
        println!("  {:>3}: {:>2} files, {:>9} names, {:>7} lines not translated yet, version {:016x}", a.code, a.files, a.entries, a.ignored, a.version);
    }
    // Names that are both a place and a road, read each way.
    for (area, name) in [("fr", "Château"), ("fr", "Pont Vieux"), ("ib", "Castillo"), ("ib", "Iglesia"), ("jp", "中山道"), ("tw", "中山橋")] {
        let show = |k| names.translation(k, area, name).map(|t| format!("{} / {}", t.main, t.sub.unwrap_or("-")));
        println!("  {area} {name}: as a place {:?}, as a road {:?}", show(Kind::Place), show(Kind::Road));
    }
    for w in names.take_warnings() {
        println!("  warning: {w}");
    }

    // The same files read without parsing (the NAS, or the client's cache after the load above).
    let mut paths = Vec::new();
    files(&dir, &mut paths);
    let t0 = Instant::now();
    let mut bytes = 0;
    for p in &paths {
        bytes += std::fs::read(p)?.len();
    }
    let read = t0.elapsed();
    println!("read alone: {:.0} MB in {read:.2?} ({:.0} MB/s)", bytes as f64 / 1e6, bytes as f64 / 1e6 / read.as_secs_f64());
    let t0 = Instant::now();
    let again = Names::load(&dir)?;
    println!("load again: {} names in {:.2?}", again.entries(), t0.elapsed());
    drop(again);

    // Refresh with nothing changed: a listing, no reads.
    let t0 = Instant::now();
    let changed = names.refresh()?;
    println!("refresh, unchanged: {changed} in {:.2?}", t0.elapsed());

    // attach over label tiles, all zooms.
    if labels.exists() {
        let a = roadcore::archive::Archive::open(&labels)?;
        let entries = a.entries();
        let step = (entries.len() / 2000).max(1);
        let (mut n, mut changed, mut raw_bytes, mut features) = (0, 0, 0, 0);
        let mut slowest = (std::time::Duration::ZERO, String::new());
        let t0 = Instant::now();
        for e in entries.iter().step_by(step) {
            let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
            let Some(gz) = a.get(z, x, y) else { continue };
            let t = Instant::now();
            let raw = mvt::gunzip_if_gzip(gz)?;
            let out = mvt::attach(&raw, u32::from(z), x, y, &names, &[LABELS])?;
            if let Some(out) = &out {
                mvt::gzip(out)?;
                changed += 1;
            }
            let took = t.elapsed();
            if took > slowest.0 {
                slowest = (took, format!("{z}/{x}/{y}"));
            }
            raw_bytes += raw.len();
            features += mvt::Tile::decode(&raw)?.layers.iter().map(|l| l.features.len()).sum::<usize>();
            n += 1;
        }
        let total = t0.elapsed();
        println!(
            "attach (gunzip, attach, gzip) over {n} label tiles ({changed} changed, {features} features, {:.0} MB raw): {total:.2?} with decoding for the count, slowest {:.1?} ({})",
            raw_bytes as f64 / 1e6,
            slowest.0,
            slowest.1
        );
    }
    Ok(())
}
