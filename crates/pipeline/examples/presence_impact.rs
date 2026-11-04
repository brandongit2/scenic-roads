//! What measuring where the fine datasets really have data (`Measured::presence`, task #929) removes
//! from the terrain's band past z12, on the build's real records (read only): each GeoTIFF dataset's
//! presence worked out as the input check does (pipeline::inputs::elevation::presence_of, its files
//! from its index in `inputs/elevation/`), then the terrain-deep targets and their tiles with and
//! without it. Under a footprint guard.
//!
//! usage: presence_impact <root> <max MB>

use pipeline::dem::declared::{Data, Where};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root, max] = args.as_slice() else { anyhow::bail!("usage: presence_impact <root> <max MB>") };
    jobrt::sys::footprint_guard("presence_impact", max.parse()?);
    let nas = store::nasfs::NasRoot::from_arg(root);
    let m = pipeline::out::read_manifest(&nas)?;
    let src = pipeline::inputs::regions::read::source(&nas, &m)?;
    let cov = pipeline::inputs::regions::read::accepted_coverage(&nas, &src, None)?;
    let declared = pipeline::terrain_zoom::Declared::read(&nas, &m)?.expect("an accepted elevation version");
    let mut ds = pipeline::dem::declared::accepted(&declared.elevation)?;
    let fetch = pipeline::fetch::Fetcher::from_env();
    let fetched = nas.join("sources/fetched");
    for d in ds.iter_mut() {
        let Data::Geotiff { index: Some(i), store, .. } = &d.declaration.data else { continue };
        let Some(cover) = d.measured.cover.clone() else { continue };
        let t0 = std::time::Instant::now();
        let ix = pipeline::dem::declared::parse_index(&nas.join(&format!("inputs/elevation/{i}")).read()?)
            .map_err(anyhow::Error::msg)?;
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
        let open = |w: &str| -> Result<pipeline::geotiff::Tiff, String> {
            let src: std::sync::Arc<dyn store::range::RangeRead> = if store {
                pipeline::dem::generic::stored(&id, &fetched.join(&id), w, None)
                    .map_err(|e| format!("{e:#}"))?
                    .ok_or_else(|| format!("{w}: not in the store"))?
                    .read
            } else {
                pipeline::dem::generic::open_url(&fetch, w)
                    .map_err(|e| format!("{w}: {e:#}"))?
                    .ok_or_else(|| format!("{w}: none"))?
            };
            pipeline::geotiff::Tiff::open(src).map_err(|e| format!("{w}: {e:#}"))
        };
        let crs = d.measured.crs.unwrap_or(4326);
        let p = pipeline::inputs::elevation::presence_of(crs, &files, &cover, &open).map_err(anyhow::Error::msg)?;
        println!(
            "{}: {} files, presence {} z10 tiles ({:.0} s)",
            d.id,
            files.len(),
            p.len(),
            t0.elapsed().as_secs_f64()
        );
        d.measured.presence = Some(p);
    }
    let read = |n: &str| Ok(declared.elevation.root().join(n).read()?);
    let with = pipeline::terrain_zoom::sets(&ds, &read)?;
    let z6: Vec<(u32, u32)> = pipeline::agent::build::coverage_tiles(&cov).into_values().flatten().collect();
    let count = |sets: &[pipeline::terrain_zoom::Set]| -> (usize, usize) {
        let (mut targets, mut tiles) = (0, 0);
        for &t in &z6 {
            for d in pipeline::terrain_deep::targets_in(sets, &cov, t) {
                targets += 1;
                tiles += pipeline::terrain_deep::levels(&cov, sets, d).iter().map(|l| l.1.len()).sum::<usize>();
            }
        }
        (targets, tiles)
    };
    let (bt, bn) = count(&declared.sets);
    let (at, an) = count(&with);
    println!("terrain-deep targets: {bt} by the cells, {at} by presence; their tiles {bn} → {an}");
    Ok(())
}
