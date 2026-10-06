//! The tree cover layers' tiles (`pipeline::trees`): dem/trees.py's `--z3` run, its command line,
//! outputs and progress lines; and the same work a block at a time, for workers.
//!
//! usage: trees --z3 x,y --coverage cov.json --chm dir --chm-store dir --leaf dir --out dir
//!              [--workers n] [--dem dir]
//!            z3 tile x,y: its canopy squares made ready in `chm` (from the NAS's `chm-store`), its
//!            leaf-type squares in `leaf` (made by `dem`'s leaftype.py where missing: the current
//!            folder unless said), out/trees-{cover,height,leaf}.tiles holding its zoom 4–12 tiles
//!        trees --block 8/x/y --coverage cov.json --chm dir|url --leaf dir|url --out dir [--record dir]
//!            one zoom-8 block: out/trees-*.tiles holding its zoom 8–12 tiles and out/trees-tops.bin
//!            its zoom-8 values; its squares read from a folder, or URLs (a prefix: through the
//!            fetch layer); `--record`: the bytes it read kept in that mirror folder
//!        trees --assemble --out dir <block out dir>…
//!            the blocks' archives and zoom 7–4 from their values: the z3 tile's archives
//!
//!   SCENIC_FETCH_*  mirror folders, recording, no network (`pipeline::fetch`)

use anyhow::{bail, Context, Result};
use pipeline::fetch::Fetcher;
use pipeline::trees::{self, mask::Shapes, Inputs, Source};
use std::collections::BTreeMap;
use std::path::PathBuf;

const USAGE: &str = "usage: trees --z3 x,y --coverage cov.json --chm dir --chm-store dir --leaf dir --out dir [--workers n] [--dem dir]
       trees --block 8/x/y --coverage cov.json --chm dir|url --leaf dir|url --out dir [--record dir]
       trees --assemble --out dir <block out dir>…";

fn main() -> Result<()> {
    let mut opts: BTreeMap<String, String> = BTreeMap::new();
    let mut assemble = false;
    let mut rest: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--assemble" {
            assemble = true;
        } else if let Some(f) = a.strip_prefix("--") {
            let (k, v) = match f.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (f.to_string(), args.next().with_context(|| format!("--{f} needs a value\n{USAGE}"))?),
            };
            if !["z3", "block", "coverage", "chm", "chm-store", "leaf", "out", "workers", "dem", "record"].contains(&k.as_str()) {
                bail!("unknown option --{k}\n{USAGE}");
            }
            opts.insert(k, v);
        } else {
            rest.push(PathBuf::from(a));
        }
    }
    let get = |k: &str| opts.get(k).with_context(|| format!("--{k} is needed\n{USAGE}"));
    // (Threads as many as said, else rayon's; in WebAssembly, none can be made: this one.)
    let mut pool = rayon::ThreadPoolBuilder::new();
    if let Some(w) = opts.get("workers") {
        pool = pool.num_threads(w.parse::<usize>().context("--workers")?.max(1));
    }
    let pool = pool.build().ok();
    let within = |f: &(dyn Fn() -> Result<()> + Sync)| pipeline::dem::sample::within(pool.as_ref(), f);
    let out = PathBuf::from(get("out")?);
    if assemble {
        if rest.is_empty() {
            bail!("no blocks to assemble\n{USAGE}");
        }
        return within(&|| {
            let n = trees::assemble(&rest, &out, &|done, total| pipeline::agent::jobs::report(done, total, "zoom 7–4 tiles"))?;
            for (l, n) in trees::LAYERS.iter().zip(n) {
                eprintln!("trees-{l}.tiles: {n} tiles");
            }
            Ok(())
        });
    }
    if !rest.is_empty() {
        bail!("unexpected {}\n{USAGE}", rest[0].display());
    }
    if let Some(q) = opts.get("z3") {
        let (x, y) = q.split_once(',').with_context(|| format!("--z3 x,y, not {q}"))?;
        let a = trees::Z3 {
            qx: x.parse().context("--z3")?,
            qy: y.parse().context("--z3")?,
            coverage: PathBuf::from(get("coverage")?),
            chm: PathBuf::from(get("chm")?),
            chm_store: PathBuf::from(get("chm-store")?),
            leaf: PathBuf::from(get("leaf")?),
            out,
            dem: opts.get("dem").map_or_else(|| PathBuf::from("."), PathBuf::from),
        };
        return within(&|| trees::z3(&a).map(|_| ()));
    }
    let b = get("block")?;
    let u = pipeline::legacy::Unit::parse(b).filter(|u| u.z == trees::ZBLOCK).with_context(|| format!("--block 8/x/y, not {b}"))?;
    let cov = PathBuf::from(get("coverage")?);
    let shapes = Shapes::parse(&std::fs::read_to_string(&cov).with_context(|| cov.display().to_string())?)?;
    let fetch = Fetcher::from_env();
    let inp = Inputs { chm: Source::parse(get("chm")?), leaf: Source::parse(get("leaf")?), fetch: &fetch, record: opts.get("record").map(PathBuf::from), there: None };
    let t0 = std::time::Instant::now();
    within(&|| {
        let r = trees::block_files(&shapes, &inp, u.x, u.y, &out)?;
        let n = |l: u8| r.tiles.iter().filter(|t| t.layer == l).count();
        let kept = if inp.record.is_some() { format!(", {:.1} MB of squares kept", r.kept as f64 / 1e6) } else { String::new() };
        eprintln!("trees {}: {} cover, {} height, {} leaf-type tiles in {:.1} s{kept}", u.slash(), n(0), n(1), n(2), t0.elapsed().as_secs_f64());
        Ok(())
    })
}
