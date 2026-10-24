//! The build's checklist (crate::agent::build::checklist, folded by stage: what the agent's status
//! carries, and the build page and `scenic status` show) worked out from a NAS project folder,
//! read only, and written as JSON: a look at the page against real data without an agent.
//!
//! usage: cargo run -p pipeline --example checklist -- <root> <pack-idx copy> <out.json>
//!
//! `<pack-idx copy>`: a copy of an agent's `pack-idx` folder (the terrain packs' indexes are read
//! from and kept in it; never an agent's own).

use pipeline::agent::build::{checklist, fold, Keys};
use pipeline::agent::rekey::{as_read, FileTimes};
use pipeline::agent::tiles::TerrainTiles;
use pipeline::coverage::Coverage;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root, idx, out] = args.as_slice() else {
        anyhow::bail!("usage: checklist <root> <pack-idx copy> <out.json>")
    };
    let root = PathBuf::from(root);
    let nr = store::nasfs::NasRoot::from_arg(&root);
    let m = pipeline::out::read_manifest(&nr)?;
    let date = pipeline::osmpass::latest_pass_by(&nr, false).ok_or_else(|| anyhow::anyhow!("no complete pass"))?;
    let mut keys = Keys::load(&nr);
    let a = pipeline::inputs::regions::read::source(&nr, &m)?;
    let cov = Coverage::accepted(&nr, &a, None)?;
    let reach = pipeline::reach::Reaches::load(&nr, &m, &date).ok().flatten();
    let mut tiles = TerrainTiles::new(Some(PathBuf::from(idx)));
    tiles.load(&nr, &m);
    let inputs = pipeline::agent::input_digests(&nr);
    let times = FileTimes::new(&nr);
    as_read(&mut keys, &cov, &m, &tiles, &times);
    let (items, next) = checklist(
        &cov,
        &date,
        &m,
        &keys,
        &inputs,
        root.join("inputs/hold-catalog").exists(),
        reach.as_ref(),
        &[],
        &tiles,
    );
    let lines = fold(&items, &next, &BTreeMap::new());
    std::fs::write(out, serde_json::to_vec_pretty(&lines)?)?;
    eprintln!("{} lines, {} items, written to {out}", lines.len(), items.len());
    Ok(())
}
