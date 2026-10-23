//! The tree cover layers' tiles (`pipeline::trees`): the `--z3` run, its command line,
//! outputs and progress lines; a z6 tile's run and a z3 tile's assembly from them, as the build makes
//! the layers; and the same work a block at a time, for workers. Its command line:
//! pipeline::cli::progs::Trees, one mode a run:
//!
//!   trees --z3 x,y --coverage cov.json --sources sources.json --chm dir --chm-store dir --leaf dir --out dir
//!         [--workers n]
//!       z3 tile x,y: its canopy squares made ready in `chm` (from the NAS's `chm-store`), its
//!       leaf-type squares in `leaf` (made where missing: pipeline::leaftype),
//!       out/trees-{cover,height,leaf}.tiles holding its zoom 4–12 tiles;
//!       `sources.json`: where the declared canopy and leaf-type sources are
//!       (pipeline::trees::RunSources, written by the trees job)
//!   trees --z6 x,y (the same options)
//!       z6 tile x,y alone: out/trees-*.tiles holding its zoom 9–12 tiles, and out/trees-mid.sect
//!       its mid (its blocks' zoom-8 tiles and values)
//!   trees --assemble-lo --out dir [--workers n] <mid>…
//!       a z3 tile's zoom 8–4 from its z6 tiles' mids: out/trees-*.tiles
//!   trees --block 8/x/y --coverage cov.json --chm dir|url --leaf dir|url --out dir [--record dir]
//!       one zoom-8 block: out/trees-*.tiles holding its zoom 8–12 tiles and out/trees-tops.bin
//!       its zoom-8 values; its squares read from a folder, or URLs (a prefix: through the
//!       fetch layer); `--record`: the bytes it read kept in that mirror folder
//!   trees --blocks 8/x/y,… --coverage cov.json --chm dir|url --leaf dir|url --out dir
//!         [--squares top,left;…] [--record dir] [--workers n]
//!       blocks of one row made together, each strip of the canopy read once for them all
//!       (a row's task, pipeline::trees::task): out/8-x-y/ for each, as --block's out;
//!       `--squares`: the canopy squares the job found there (one not found fails the run,
//!       never a block without its trees)
//!   trees --assemble --out dir <block out dir>…
//!       the blocks' archives and zoom 7–4 from their values: the z3 tile's archives
//!
//!   SCENIC_FETCH_*  mirror folders, recording, no network (`pipeline::fetch`)

use anyhow::{bail, Context, Result};
use clap::Parser;
use pipeline::cli::progs::Trees;
use pipeline::fetch::Fetcher;
use pipeline::trees::{self, mask::Shapes, Inputs, Source};
use std::path::PathBuf;

fn main() -> Result<()> {
    let a = Trees::parse();
    // (Its phases, for the job's phase that runs it: pipeline::timings.)
    pipeline::timings::job("trees", || run(&a))
}

/// An option the mode needs.
fn need<'a, T>(v: &'a Option<T>, k: &str) -> Result<&'a T> {
    v.as_ref().with_context(|| format!("--{k} is needed (trees --help)"))
}

fn run(a: &Trees) -> Result<()> {
    // (Threads as many as said, else rayon's; in WebAssembly, none can be made: this one.)
    let mut pool = rayon::ThreadPoolBuilder::new();
    if let Some(w) = a.workers {
        pool = pool.num_threads(w.max(1));
    }
    let pool = pool.build().ok();
    let within = |f: &(dyn Fn() -> Result<()> + Sync)| pipeline::dem::sample::within(pool.as_ref(), f);
    let out = a.out.clone();
    if a.assemble || a.assemble_lo {
        if a.inputs.is_empty() {
            bail!("nothing to assemble");
        }
        return within(&|| {
            let said = |done, total| pipeline::agent::jobs::report(done, total, "zoom 7–4 tiles");
            let n = if a.assemble_lo {
                trees::assemble_lo(&a.inputs, &out, &said)?
            } else {
                trees::assemble(&a.inputs, &out, &said)?
            };
            for (l, n) in trees::LAYERS.iter().zip(n) {
                eprintln!("trees-{l}.tiles: {n} tiles");
            }
            Ok(())
        });
    }
    if let Some(p) = a.inputs.first() {
        bail!("unexpected {} (folders are --assemble's and --assemble-lo's)", p.display());
    }
    for (z, k, t) in [(3u8, "z3", &a.z3), (6, "z6", &a.z6)] {
        let Some(t) = t else { continue };
        let (x, y) = t.split_once(',').with_context(|| format!("--{k} x,y, not {t}"))?;
        let r = trees::Run {
            tile: (z, x.parse().with_context(|| format!("--{k}"))?, y.parse().with_context(|| format!("--{k}"))?),
            coverage: need(&a.coverage, "coverage")?.clone(),
            chm: PathBuf::from(need(&a.chm, "chm")?),
            chm_store: need(&a.chm_store, "chm-store")?.clone(),
            sources: need(&a.sources, "sources")?.clone(),
            leaf: PathBuf::from(need(&a.leaf, "leaf")?),
            out: out.clone(),
        };
        return within(&|| (if z == 3 { trees::z3(&r) } else { trees::z6(&r) }).map(|_| ()));
    }
    let cov = need(&a.coverage, "coverage")?;
    let shapes = Shapes::parse(&std::fs::read_to_string(cov).with_context(|| cov.display().to_string())?)?;
    let fetch = Fetcher::from_env();
    let there = a.squares.as_deref().map(trees::task::parse_squares).transpose().context("--squares top,left;…")?;
    let inp = Inputs {
        chm: Source::parse(need(&a.chm, "chm")?),
        leaf: Source::parse(need(&a.leaf, "leaf")?),
        fetch: &fetch,
        record: a.record.clone(),
        there,
    };
    let t0 = std::time::Instant::now();
    if let Some(list) = &a.blocks {
        let list = trees::task::parse_blocks(list).with_context(|| format!("--blocks 8/x/y,…, not {list}"))?;
        return within(&|| {
            let kept = trees::blocks_files(&shapes, &inp, &list, &out)?;
            let kept = if inp.record.is_some() {
                format!(", {:.1} MB of squares kept", kept as f64 / 1e6)
            } else {
                String::new()
            };
            eprintln!("trees: {} blocks in {:.1} s{kept}", list.len(), t0.elapsed().as_secs_f64());
            Ok(())
        });
    }
    let b = need(&a.block, "block")?;
    let u = pipeline::legacy::TileId::parse(b)
        .filter(|u| u.z == trees::ZBLOCK)
        .with_context(|| format!("--block 8/x/y, not {b}"))?;
    within(&|| {
        let r = trees::block_files(&shapes, &inp, u.x, u.y, &out)?;
        let n = |l: u8| r.tiles.iter().filter(|t| t.layer == l).count();
        let kept = if inp.record.is_some() {
            format!(", {:.1} MB of squares kept", r.kept as f64 / 1e6)
        } else {
            String::new()
        };
        eprintln!(
            "trees {}: {} cover, {} height, {} leaf-type tiles in {:.1} s{kept}",
            u.slash(),
            n(0),
            n(1),
            n(2),
            t0.elapsed().as_secs_f64()
        );
        Ok(())
    })
}
