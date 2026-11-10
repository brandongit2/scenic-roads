//! Terrain-root's and an assembly's time and peak footprint by the rule (task #929), on the build's
//! real records, writing nothing but the Terrarium tiles' local cache: the root from the lo packs'
//! z3 tiles and the relief (terrain::terrain_pack::make_root), then z3 tile `q`'s z8 → z3 with no
//! z6 tile's mid (its quarters outside every region the relief's, an assembly's most reading), under
//! a footprint guard. Its count of the assembly's tiles is not an assembly's: a quarter inside a
//! region is its children's and, with no mid given, void (a tile of only those is none), and the
//! relief has no tile over the deep sea (heights under crate::dem::sample::NODATA_BELOW are none,
//! as the lo packs before it had none there). 3/2/2, 4 Nov: 511 of 1,365, against 1,204 in its lo
//! pack (749 of its tiles wholly in regions; 161 of those wholly outside them absent there too).
//!
//! usage: cargo run -p pipeline --example lo_root_measure -- <root> <raw cache dir> <max MB> <3/x/y>

fn mb() -> u64 {
    jobrt::sys::footprint().unwrap_or(0) / 1_000_000
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, root, raw, max, q] = args.as_slice() else {
        anyhow::bail!("usage: lo_root_measure <root> <raw cache dir> <max MB> <3/x/y>")
    };
    jobrt::sys::footprint_guard("lo_root_measure", max.parse()?);
    let q: Vec<u32> = q.split('/').map(|v| v.parse()).collect::<Result<_, _>>()?;
    anyhow::ensure!(q.len() == 3 && q[0] == 3, "a z3 tile");
    let q = (q[1], q[2]);
    let nas = store::nasfs::NasRoot::from_arg(root);
    let scratch = std::path::Path::new(raw).join("scratch");
    let out = buildstore::out::Out::unbound(&nas, &scratch)?;
    let m = out.manifest.clone();
    let src = inputs::inputs::regions::read::source(&nas, &m)?;
    let cov = inputs::inputs::regions::read::accepted_coverage(&nas, &src, None)?;
    let sets = terrain::terrain_zoom::Declared::read(&nas, &m)?.map(|d| d.sets).unwrap_or_default();
    let url = terrain::terrain_base::terrarium_template(&nas, &m)?;
    let rt = terrain::terrain_pack::RawTiles::with_store(
        std::path::Path::new(raw),
        &nas.join("sources/aws-terrarium"),
        &url,
    );
    println!("start: {} MB", mb());
    for (what, b) in
        [("root", terrain::terrain_base::tiles_box(0, &[(0, 0)])), ("lo", terrain::terrain_base::tiles_box(3, &[q]))]
    {
        let pool = terrain::terrain_base::Pool::read(&nas, &m, b)?;
        let relief = pool.relief();
        let none = |_: u32, _: u32| false;
        let fill = terrain::terrain_pack::fill_of(&cov, &sets, &none);
        let native = |z: u8, x: u32, y: u32| terrain::terrain_pack::native_of(&fill, z, x, y);
        let s = terrain::terrain_pack::Sources { native: Some(&native), sets: &sets };
        let t0 = std::time::Instant::now();
        let n = pool.with_tiles(&rt, |tiles| {
            relief.with_tiles(&rt, |rel| -> anyhow::Result<usize> {
                let reaching = |z: u8, x: u32, y: u32| terrain::terrain_zoom::reaching(&sets, z, x, y);
                let fills = terrain::terrain_pack::Fills::new(tiles, rel, &fill, &reaching);
                Ok(match what {
                    "root" => terrain::terrain_pack::make_root(&out, &fills, &s)?.len(),
                    _ => terrain::terrain_pack::make_lo_from(&fills, &s, q, &[])?.len(),
                })
            })
        })???;
        println!("{what}: {n} tiles in {:.1} s, footprint {} MB", t0.elapsed().as_secs_f64(), mb());
    }
    Ok(())
}
