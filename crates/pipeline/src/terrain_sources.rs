//! The 3D terrain's sources as the owner declares them (docs/inputs.md §6.8): the elevation datasets
//! with `terrain = true` in `inputs/elevation/`. One is a tile pyramid in the Terrarium encoding, the
//! terrain's base (fetched raw, repaired and blended: crate::terrain_pack); at most one is a GeoTIFF
//! dataset by a tile index of 1° tiles, each with its fill mask, blended in north of 60°N
//! (crate::terrain_north). What's done with them (the repair, the water's flattening, the blend) is
//! the terrain's own code; where they are is the declarations'. The terrain's keys name them by their
//! `keyed` digests (`pin`).

use crate::dem::declared::{self, Accepted, Data, Where};
use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;

/// The terrain's declared sources, as its jobs read them.
#[derive(Clone, Debug)]
pub struct Declared {
    /// The base's URL template (`{z}`, `{x}`, `{y}`).
    pub base: String,
    /// The dataset blended in north of 60°N, when one is declared.
    pub north: Option<North>,
    /// What the terrain's keys name (`pin`).
    pub pin: String,
    /// The roads' datasets finer than the terrain's, which make it finer where they have data
    /// (crate::terrain_fine); None when none is.
    pub fine: Option<crate::terrain_fine::Fine>,
}

/// A GeoTIFF dataset of 1° tiles, by its tile index.
#[derive(Clone, Debug)]
pub struct North {
    /// The dataset's id: its files are kept in `sources/fetched/<id>/`.
    pub id: String,
    /// Each tile by its south-west corner (latitude, longitude, whole degrees): its file's URL and its
    /// fill mask's.
    pub cells: HashMap<(i32, i32), (String, Option<String>)>,
}

/// What the terrain's keys name of its declared sources `ds` (the terrain's, in their order): their
/// ids and `keyed` digests.
pub fn pin(ds: &[Accepted]) -> String {
    format!("terrain-sources {}", declared::tag(ds))
}

/// The terrain's sources in accepted version `a` (its index read for the northern dataset's tiles).
pub fn of(a: &crate::inputs::Accepted) -> Result<Declared> {
    let ds = declared::accepted_terrain(a)?;
    let mut base = None;
    let mut north = None;
    for d in &ds {
        match &d.declaration.data {
            Data::Tiles { url, encoding, .. } => {
                ensure!(
                    encoding == "terrarium",
                    "{}: the terrain's tile pyramid is read as Terrarium PNGs, not {encoding}",
                    d.path
                );
                ensure!(base.is_none(), "{}: the terrain has one tile pyramid declared already", d.path);
                base = Some(url.clone());
            }
            Data::Geotiff { index: Some(i), .. } => {
                ensure!(north.is_none(), "{}: the terrain has one GeoTIFF dataset declared already", d.path);
                let ix = declared::parse_index(&a.read(i)?).map_err(|e| anyhow::anyhow!("{i}: {e}"))?;
                ensure!(ix.crs == 4326, "{i}: the terrain's 1° tiles are indexed in EPSG:4326, not EPSG:{}", ix.crs);
                let mut cells = HashMap::new();
                for ((w, b), m) in ix.files.into_iter().zip(ix.masks) {
                    let Where::Url(u) = w else { bail!("{i}: the terrain's tiles are named by URL") };
                    cells.insert((b[1].round() as i32, b[0].round() as i32), (u, m));
                }
                north = Some(North { id: d.id.clone(), cells });
            }
            Data::Geotiff { .. } => bail!("{}: the terrain's GeoTIFF dataset names its 1° tiles by an index", d.path),
        }
    }
    Ok(Declared {
        base: base.context("no tile pyramid is declared for the terrain (`terrain = true`, docs/inputs.md §6.8)")?,
        north,
        pin: pin(&ds),
        fine: crate::terrain_fine::Fine::of(a, &ds)?,
    })
}

/// The terrain's sources in the accepted version `out`'s records name.
pub fn read(out: &crate::out::Out) -> Result<Declared> {
    let a = crate::inputs::open(out.root(), &out.manifest, crate::inputs::elevation::UNIT)?.context("no version of the elevation datasets is accepted: the terrain's sources are declared there (docs/inputs.md §6.8)")?;
    of(&a)
}

/// The base's template in tests (nothing fetched: a test goes offline, or has its tiles).
#[cfg(test)]
pub const TEST_BASE: &str = "https://tiles.test/{z}/{x}/{y}.png";

/// Tile `z/x/y`'s URL by template `url`.
pub fn tile_url(url: &str, z: u8, x: u32, y: u32) -> String {
    url.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terrains_sources_from_an_accepted_version() {
        let d = tempfile::tempdir().unwrap();
        let mut m = std::collections::BTreeMap::new();
        crate::inputs::elevation::accept_fixture(d.path(), &mut m);
        let a = crate::inputs::open(d.path(), &m, crate::inputs::elevation::UNIT).unwrap().unwrap();
        let t = of(&a).unwrap();
        assert!(t.base.contains("{z}") && t.base.ends_with("{y}.png"));
        assert_eq!(tile_url(&t.base, 5, 6, 7), t.base.replace("{z}/{x}/{y}", "5/6/7"));
        let n = t.north.unwrap();
        assert_eq!(n.id, "glo30");
        assert_eq!(n.cells.keys().copied().collect::<std::collections::BTreeSet<_>>(), [(64, -148), (65, -148)].into());
        assert!(
            n.cells[&(64, -148)].0.ends_with("N64_W148_DEM.tif")
                && n.cells[&(64, -148)].1.as_deref().unwrap().ends_with("N64_W148_FLM.tif")
        );
        // The pin: the terrain's declarations alone (the roads' left out), by id and `keyed`.
        let ds = declared::accepted_terrain(&a).unwrap();
        assert_eq!(ds.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["glo30", "terrarium"]);
        assert_eq!(t.pin, pin(&ds));
        assert!(declared::accepted(&a).unwrap().iter().all(|d| !d.declaration.terrain));
    }

    /// The declarations dropped into `inputs/elevation/` (kept in producers/elevation/terrain/) are
    /// of the drop box's shape, and declare the terrain's base and its northern dataset.
    #[test]
    fn the_staged_declarations_parse() {
        for (path, b) in [
            ("terrarium.toml", &include_bytes!("../../../producers/elevation/terrain/terrarium.toml")[..]),
            ("glo30.toml", &include_bytes!("../../../producers/elevation/terrain/glo30.toml")[..]),
        ] {
            let (desc, findings, _) = crate::inputs::credits::parse_description(path, b);
            assert!(desc.is_some() && findings.is_empty(), "{path}: {findings:?}");
            let c = crate::inputs::Checks::file(&crate::inputs::elevation::Elevation::default(), path, b);
            assert!(c.findings.is_empty() && c.keyed.is_some(), "{path}: {:?}", c.findings);
            let d = declared::parse(b).unwrap_or_else(|e| panic!("{path}: {e:?}"));
            assert!(d.terrain && d.code == 0, "{path}");
            match path {
                "terrarium.toml" => {
                    assert!(matches!(&d.data, Data::Tiles { encoding, zoom: 12, .. } if encoding == "terrarium"))
                }
                _ => assert!(matches!(&d.data, Data::Geotiff { index: Some(i), .. } if i == "glo30-tiles.geojson")),
            }
        }
    }
}
