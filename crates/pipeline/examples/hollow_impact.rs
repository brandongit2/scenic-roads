//! What the plan's hollow deep tiles (task #842: `Measured::hollow`, terrain::terrain_zoom::hollow)
//! skip, on the build's real records (read only): each GeoTIFF dataset's hollow tiles worked out
//! as the input check does (inputs::inputs::elevation::hollow_of, over its accepted presence, its
//! files from its index in the accepted version), then the terrain-deep targets now: those
//! waiting that the plan would no longer run, and, held against what ran, the targets done and
//! current that made no pack (the skip's right answers) or made one (data a skip would lose).
//! Under a footprint guard.
//!
//! usage: hollow_impact <root> <pack-idx copy> <max MB> <out.json>
//!
//! `<pack-idx copy>`: a folder the terrain packs' indexes are read into and kept (never an
//! agent's own), as the checklist example's.

use agentcore::pool::records::Keys;
use inputs::dem::declared::{Data, Where};
use pipeline::agent::build::KeyCtx;
use pipeline::agent::tiles::TerrainTiles;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root, idx, max, out] = args.as_slice() else {
        anyhow::bail!("usage: hollow_impact <root> <pack-idx copy> <max MB> <out.json>")
    };
    jobrt::sys::footprint_guard("hollow_impact", max.parse()?);
    let nas = store::nasfs::NasRoot::from_arg(root);
    let m = buildstore::records::read_manifest(&nas)?;
    let declared = terrain::terrain_zoom::Declared::read(&nas, &m)?.expect("an accepted elevation version");
    let mut ds = inputs::dem::declared::accepted(&declared.elevation)?;
    let fetch = buildstore::fetch::Fetcher::from_env();
    let fetched = nas.join("sources/fetched");
    let mut per_set = serde_json::Map::new();
    for d in ds.iter_mut() {
        let Data::Geotiff { index: Some(i), store, .. } = &d.declaration.data else { continue };
        let Some(presence) = d.measured.presence.clone() else { continue };
        let t0 = std::time::Instant::now();
        let ix = inputs::dem::declared::parse_index(&declared.elevation.read(i)?).map_err(anyhow::Error::msg)?;
        let files: Vec<(String, Option<[f64; 4]>)> = ix
            .files
            .into_iter()
            .map(|(w, b)| {
                (
                    match w {
                        Where::Url(u) => u,
                        Where::File(f) => format!("{}/{f}", d.id),
                    },
                    Some(b),
                )
            })
            .collect();
        let (id, store) = (d.id.clone(), *store);
        let opened = std::sync::atomic::AtomicUsize::new(0);
        let open = |w: &str| -> Result<buildstore::geotiff::Tiff, String> {
            opened.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let src: std::sync::Arc<dyn store::range::RangeRead> = if store {
                inputs::dem::generic::stored(&id, &fetched.join(&id), w, None)
                    .map_err(|e| format!("{e:#}"))?
                    .ok_or_else(|| format!("{w}: not in the store"))?
                    .read
            } else {
                inputs::dem::generic::open_url(&fetch, w)
                    .map_err(|e| format!("{w}: {e:#}"))?
                    .ok_or_else(|| format!("{w}: none"))?
            };
            buildstore::geotiff::Tiff::open(src).map_err(|e| format!("{w}: {e:#}"))
        };
        let crs = d.measured.crs.unwrap_or(4326);
        // (Kept beside the output, by dataset: a run again reads none of it.)
        let kept = std::path::Path::new(out).with_extension(format!("{}.hollow.json", d.id));
        let h: Vec<[u32; 2]> = match std::fs::read(&kept) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(_) => {
                let h =
                    inputs::inputs::elevation::hollow_of(crs, &files, &presence, &open).map_err(anyhow::Error::msg)?;
                std::fs::write(&kept, serde_json::to_vec(&h)?)?;
                h
            }
        };
        let secs = t0.elapsed().as_secs_f64();
        let n = opened.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!(
            "{}: {} files, {n} opened; presence {} z10 tiles, {} hollow ({secs:.0} s)",
            d.id,
            files.len(),
            presence.len(),
            h.len()
        );
        per_set.insert(
            d.id.clone(),
            serde_json::json!({ "files": files.len(), "opened": n, "presence": presence.len(), "hollow": h.len(), "secs": secs }),
        );
        d.measured.hollow = Some(h);
    }
    let read = |n: &str| Ok(declared.elevation.root().join(n).read()?);
    let with = terrain::terrain_zoom::sets(&ds, &read)?;

    // The targets now, which are done (their key recorded), which of those made a pack.
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
    let (mut waiting, mut empty, mut packed) = (vec![], vec![], vec![]);
    for (t, k) in &tt.terrain_deep {
        if keys.terrain_deep.get(t) != Some(k) {
            waiting.push(t.clone());
        } else if pipeline::agent::steps::output_named("terrain-deep", t, &m) == Some(false) {
            empty.push(t.clone());
        } else {
            packed.push(t.clone());
        }
    }
    let hollow = |t: &String| {
        let u = codecs::legacy::TileId::parse(t).expect("a z10 tile");
        terrain::terrain_zoom::hollow(&with, u.x, u.y)
    };
    let of = |ts: &[String]| ts.iter().filter(|t| hollow(t)).cloned().collect::<Vec<_>>();
    // (By the outlines alone: the plan's on the accepted sets now, none measured hollow yet.)
    let cut = |ts: &[String]| ts.iter().filter(|t| tt.deep_hollow.contains(*t)).count();
    let (hw, he, hp) = (of(&waiting), of(&empty), of(&packed));
    eprintln!(
        "terrain-deep targets: {}; waiting {} ({} hollow, {} by the outlines alone); done {} made no pack ({} hollow, {}), {} made one ({} hollow, {}: data a skip would lose)",
        tt.terrain_deep.len(),
        waiting.len(),
        hw.len(),
        cut(&waiting),
        empty.len(),
        he.len(),
        cut(&empty),
        packed.len(),
        hp.len(),
        cut(&packed)
    );
    std::fs::write(
        out,
        serde_json::to_vec_pretty(&serde_json::json!({
            "sets": per_set,
            "waiting": waiting, "waiting_hollow": hw,
            "empty": empty, "empty_hollow": he,
            "packed": packed, "packed_hollow": hp,
            "plan_hollow_now": tt.deep_hollow,
        }))?,
    )?;
    Ok(())
}
