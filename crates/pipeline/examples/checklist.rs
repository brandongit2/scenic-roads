//! The build's checklist (pipeline::agent::build::checklist, folded by stage: what the agent's status
//! carries, and the build page and `scenic status` show) worked out from a NAS project folder,
//! read only, and written as JSON: a look at the page against real data without an agent.
//!
//! usage: cargo run -p pipeline --example checklist -- <root> <pack-idx copy> <out.json>
//!
//! `<pack-idx copy>`: a copy of an agent's `pack-idx` folder (the terrain packs' indexes are read
//! from and kept in it; never an agent's own).

use agentcore::pool::records::Keys;
use pipeline::agent::build::{checklist, fold, KeyCtx};
use pipeline::agent::tiles::TerrainTiles;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root, idx, out] = args.as_slice() else {
        anyhow::bail!("usage: checklist <root> <pack-idx copy> <out.json>")
    };
    let root = PathBuf::from(root);
    let nr = store::nasfs::NasRoot::from_arg(&root);
    let m = buildstore::records::read_manifest(&nr)?;
    let date = inputs::osmpass::latest_pass_by(&nr, false).ok_or_else(|| anyhow::anyhow!("no complete pass"))?;
    let keys = Keys::load(&nr);
    let a = inputs::inputs::regions::read::source(&nr, &m)?;
    let cov = inputs::inputs::regions::read::accepted_coverage(&nr, &a, None)?;
    let reach = roads::reach::Reaches::load(&nr, &m, &date).ok().flatten();
    let mut tiles = TerrainTiles::new(Some(PathBuf::from(idx)));
    tiles.load(&nr, &m);
    let inputs = pipeline::agent::theplan::the_plan().key_inputs(&nr, &m);

    let x = KeyCtx::new(&cov, &date, &m, &inputs, reach.as_ref(), &tiles);
    let (items, next) = checklist(&x, &keys, root.join("inputs/hold-catalog").exists(), &[]);
    let lines = fold(&items, &next, &BTreeMap::new());
    std::fs::write(out, serde_json::to_vec_pretty(&lines)?)?;
    eprintln!("{} lines, {} items, written to {out}", lines.len(), items.len());
    Ok(())
}
