//! The 3D terrain's base as the elevation pool declares it (docs/inputs.md §6.8): of the datasets in
//! `inputs/elevation/`, the roads' and the terrain's alike, the worldwide ones its base is made from.
//! The last tile pyramid in the Terrarium encoding with no `regions` is its base (fetched raw:
//! crate::terrain_pack); the last GeoTIFF dataset with no `regions` by a tile index of 1° tiles in
//! EPSG:4326, each with its fill mask, is blended in north of 60°N (crate::terrain_north). What's
//! done with them (the water's flattening, the blend) is the terrain's own code; where they are is
//! the declarations'. The terrain's keys name them by their `keyed` digests (`pin`).

use crate::dem::declared::{self, Accepted, Data, Where};
use anyhow::{bail, Context, Result};
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
    /// The finest full resolution of the two (m): datasets finer go on past the base
    /// (crate::terrain_fine).
    pub resolution: f64,
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

/// What the terrain's keys name of the declarations it reads `ds`: their ids and `keyed` digests.
pub fn pin(ds: &[Accepted]) -> String {
    format!("terrain-sources {}", declared::tag(ds))
}

/// Whether `d` is a tile pyramid in the Terrarium encoding used everywhere: the base's kind.
fn is_base(d: &Accepted) -> bool {
    d.declaration.regions.is_none()
        && matches!(&d.declaration.data, Data::Tiles { encoding, .. } if encoding == "terrarium")
}

/// The terrain's sources in accepted version `a` (its index read for the northern dataset's tiles).
pub fn of(a: &crate::inputs::Accepted) -> Result<Declared> {
    let ds = declared::accepted(a)?;
    let base = ds.iter().rev().find(|d| is_base(d)).context(
        "no tile pyramid in the Terrarium encoding is declared everywhere: the terrain's base (docs/inputs.md §6.8)",
    )?;
    let Data::Tiles { url, .. } = &base.declaration.data else { unreachable!("the base is a tile pyramid") };
    let mut used = Vec::new();
    let mut north = None;
    for d in ds.iter().rev() {
        let Data::Geotiff { index: Some(i), .. } = &d.declaration.data else { continue };
        if d.declaration.regions.is_some() {
            continue;
        }
        let ix = declared::parse_index(&a.read(i)?).map_err(|e| anyhow::anyhow!("{i}: {e}"))?;
        if ix.crs != 4326 || ix.masks.is_empty() || ix.masks.iter().any(Option::is_none) {
            continue;
        }
        let mut cells = HashMap::new();
        for ((w, b), m) in ix.files.into_iter().zip(ix.masks) {
            let Where::Url(u) = w else { bail!("{i}: the terrain's 1° tiles are named by URL") };
            cells.insert((b[1].round() as i32, b[0].round() as i32), (u, m));
        }
        north = Some(North { id: d.id.clone(), cells });
        used.push(d.clone());
        break;
    }
    used.push(base.clone());
    let resolution = used.iter().map(crate::terrain_fine::full_resolution).fold(f64::INFINITY, f64::min);
    Ok(Declared { base: url.clone(), north, pin: pin(&used), resolution })
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
        // The pin: the two it reads, by id and `keyed` (the pool's others left out).
        let ds = declared::accepted(&a).unwrap();
        let used: Vec<Accepted> =
            ["glo30", "terrarium"].iter().map(|i| ds.iter().find(|d| d.id == *i).unwrap().clone()).collect();
        assert_eq!(t.pin, pin(&used));
    }

    /// The worldwide declarations dropped into `inputs/elevation/` (kept in producers/elevation/)
    /// are of the drop box's shape: GLO-30 by its index, kept in the store, then the Terrarium tiles.
    #[test]
    fn the_staged_declarations_parse() {
        for (path, b) in [
            ("terrarium.toml", &include_bytes!("../../../producers/elevation/terrarium.toml")[..]),
            ("glo30.toml", &include_bytes!("../../../producers/elevation/glo30.toml")[..]),
        ] {
            let (desc, findings, _) = crate::inputs::credits::parse_description(path, b);
            assert!(desc.is_some() && findings.is_empty(), "{path}: {findings:?}");
            let c = crate::inputs::Checks::file(&crate::inputs::elevation::Elevation::default(), path, b);
            assert!(c.findings.is_empty() && c.keyed.is_some(), "{path}: {:?}", c.findings);
            let d = declared::parse(b).unwrap_or_else(|e| panic!("{path}: {e:?}"));
            assert_eq!(d.heights, "EGM2008", "{path}");
            match path {
                "terrarium.toml" => assert!(
                    d.rank == Some(2)
                        && matches!(&d.data, Data::Tiles { encoding, zoom: 12, .. } if encoding == "terrarium")
                ),
                _ => assert!(
                    d.rank == Some(1)
                        && matches!(&d.data, Data::Geotiff { index: Some(i), store: true, .. } if i == "glo30-tiles.geojson")
                ),
            }
        }
    }
}
