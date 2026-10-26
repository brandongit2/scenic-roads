//! How densely `extract` puts points along a way (docs/inputs.md §6.8, Densification): as far
//! apart as the finished terrain's pixels where the way starts (the roads take their heights from
//! it: crate::road_terrain), never closer than `FLOOR_M`. The terrain's finest zoom at a point is
//! the finest a dataset finer than the base takes it to there (crate::terrain_deep: its `zoom_for`
//! of the dataset's full resolution, at most `MAX_Z`), else the base's (z12, coarser at high
//! latitudes: crate::terrain_pack::max_zoom_at); its pixel there is the spacing: 20–38 m where
//! GLO-30 alone is, ~7 m over USGS 3DEP's 10 m, `FLOOR_M` over the lidar. The datasets'
//! resolutions and coverages are those the input check measured (`facts.measured`); no source or
//! area is named here.
//!
//! A cell is the point's degrees floored (west, south corner), half-open: a point on a cell's west
//! or south edge is in that cell alone. A unit's key names, of the cells holding its ways' first
//! nodes (`crate::reach::Reach::cells`), the finer zoom each has (`Fine::key`): a dataset added or
//! measured again rebuilds only the units whose spacing it changes.

use super::declared::{Accepted, Cover};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The closest two points of a way are ever put (m): the owner's floor, however fine the terrain.
pub const FLOOR_M: f64 = 4.0;

/// The file a unit's folder holds for `extract` (a `Fine`, as JSON).
pub const FILE: &str = "spacing.json";

/// How far apart `extract` puts a way's points, by its first node (E7).
pub trait Spacing {
    fn at(&self, p: [i32; 2]) -> f64;
}

/// The same spacing everywhere (`extract` given a number: the steps that don't sample elevations).
pub struct Uniform(pub f64);

impl Spacing for Uniform {
    fn at(&self, _: [i32; 2]) -> f64 {
        self.0
    }
}

/// The 1° cell (west, south corner, degrees) of a point (E7), exactly.
pub fn cell(p: [i32; 2]) -> [i32; 2] {
    [p[0].div_euclid(10_000_000), p[1].div_euclid(10_000_000)]
}

/// The terrain's pixel (m) at latitude `lat` at zoom `z`, never under `FLOOR_M`: the spacing there.
pub fn spacing_at(z: u8, lat: f64) -> f64 {
    super::generic::tile_resolution(z as u32, lat).max(FLOOR_M)
}

/// Where the terrain goes past its base: each 1° cell a dataset finer than the base has data in,
/// with the finest zoom one takes the terrain to there (at the cell's poleward edge, as
/// crate::terrain_deep reckons a tile's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fine {
    pub cells: Vec<([i32; 2], u8)>,
}

impl Fine {
    /// From the declared datasets with their coverages (agent::tiles::Elevation).
    pub fn of(ds: &[(Accepted, Cover)]) -> Fine {
        let accepted: Vec<Accepted> = ds.iter().map(|(d, _)| d.clone()).collect();
        let base = crate::terrain_deep::base_resolution(&accepted);
        let mut cells: BTreeMap<[i32; 2], u8> = BTreeMap::new();
        for (d, c) in ds {
            let full = crate::terrain_deep::full_resolution(d);
            if !(full < base * crate::terrain_deep::FINER) || c.world {
                continue;
            }
            let pyramid = match &d.declaration.data {
                super::declared::Data::Tiles { zoom, .. } => Some((*zoom).min(crate::terrain_deep::MAX_Z as u32) as u8),
                super::declared::Data::Geotiff { .. } => None,
            };
            for &(x, y) in &c.cells {
                let poleward = (y as f64).abs().max((y as f64 + 1.0).abs());
                let z = pyramid.unwrap_or_else(|| crate::terrain_deep::zoom_for(full, poleward));
                if z > crate::terrain_pack::max_zoom_at(poleward) {
                    let e = cells.entry([x, y]).or_insert(z);
                    *e = (*e).max(z);
                }
            }
        }
        Fine { cells: cells.into_iter().collect() }
    }

    /// From the declared datasets (`declared::accepted`).
    pub fn of_accepted(ds: &[Accepted]) -> Fine {
        let with: Vec<(Accepted, Cover)> =
            ds.iter().map(|d| (d.clone(), Cover::of(d.measured.cover.as_deref()))).collect();
        Fine::of(&with)
    }

    /// The terrain's finer zoom in 1° cell `c`, where it has one.
    pub fn zoom(&self, c: [i32; 2]) -> Option<u8> {
        self.cells.binary_search_by(|e| e.0.cmp(&c)).ok().map(|i| self.cells[i].1)
    }

    /// What decides the spacing of ways whose first nodes are in `cells`, for a key: the finer
    /// zoom each of them has.
    pub fn key(&self, cells: &[[i32; 2]]) -> String {
        let fine: Vec<String> =
            cells.iter().filter_map(|&c| self.zoom(c).map(|z| format!("{},{} {z}", c[0], c[1]))).collect();
        format!("spacing {} floor {FLOOR_M}", store::naming::hash16(fine.join(" ").as_bytes()))
    }

    /// From the declared datasets' accepted version the records name (`m`): an error before the
    /// build reads them (their switch, docs/inputs.md §6.8).
    pub fn of_records(root: &store::nasfs::NasRoot, m: &std::collections::BTreeMap<String, String>) -> Result<Fine> {
        use crate::inputs::elevation;
        anyhow::ensure!(
            elevation::switched(m),
            "the records don't say the build reads the declared elevation datasets yet (docs/inputs.md §6.8)"
        );
        let a = crate::inputs::open(root, m, elevation::UNIT)?
            .context("the records name no accepted version of the declared elevation datasets")?;
        Ok(Fine::of_accepted(&super::declared::accepted(&a)?))
    }

    /// The whole of it as one digest: what the steps that read a spacing file over tiles they
    /// don't divide by cells (the links, rail and candidates) name in their keys.
    pub fn digest(&self) -> String {
        let cells: Vec<String> = self.cells.iter().map(|(c, z)| format!("{},{} {z}", c[0], c[1])).collect();
        store::naming::hash16(format!("floor {FLOOR_M}|{}", cells.join(" ")).as_bytes())
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec(self)?).with_context(|| format!("write {}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Fine> {
        serde_json::from_slice(&std::fs::read(path).with_context(|| format!("read {}", path.display()))?)
            .with_context(|| format!("{}: not a spacing file", path.display()))
    }
}

/// The digests (`Fine::digest`) of the accepted elevation versions read, by their index's
/// content name (a version's never changes: read once a process).
static READ: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The planner's input (agent: `inputs`) naming the spacing the steps reading a spacing file
/// key on: `Fine::digest` of the accepted elevation version, "?" while it can't be read.
pub const INPUT: &str = "spacing";

/// `Fine::digest` of the accepted elevation version the records `m` name, read from its index
/// (`Fine::of_records`), cached by that version.
pub fn digest_read(root: &store::nasfs::NasRoot, m: &std::collections::BTreeMap<String, String>) -> Result<String> {
    let v = m
        .get(&crate::inputs::logical(crate::inputs::elevation::UNIT))
        .context("the records name no accepted version of the declared elevation datasets")?;
    if let Some(d) = READ.lock().ok().and_then(|r| r.get(v).cloned()) {
        return Ok(d);
    }
    let d = Fine::of_records(root, m)?.digest();
    if let Ok(mut r) = READ.lock() {
        r.insert(v.clone(), d.clone());
    }
    Ok(d)
}

/// The spacing's line for a key, from the planner's inputs (`INPUT`): None while it isn't known
/// (the step waits).
pub fn key_line(inputs: &std::collections::BTreeMap<String, String>) -> Option<String> {
    inputs.get(INPUT).filter(|d| d.as_str() != "?").map(|d| format!("spacing {d}"))
}

impl Spacing for Fine {
    fn at(&self, p: [i32; 2]) -> f64 {
        let lat = p[1] as f64 * 1e-7;
        let z = self.zoom(cell(p)).unwrap_or_else(|| crate::terrain_pack::max_zoom_at(lat));
        spacing_at(z, lat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dem::declared::{Data, Declaration, Measured};

    fn ds(id: &str, resolution: f64, rank: Option<i64>, cover: Option<Vec<[f64; 4]>>) -> (Accepted, Cover) {
        let d = Accepted {
            convert: None,
            id: id.into(),
            path: format!("{id}.toml"),
            declaration: Declaration {
                surface: "ground".into(),
                heights: "EGM2008".into(),
                rank,
                code: 1,
                regions: None,
                exclude: Vec::new(),
                data: Data::Geotiff {
                    urls: vec!["https://example.org/a.tif".into()],
                    index: None,
                    files: vec![],
                    store: false,
                },
            },
            measured: Measured { crs: Some(4326), resolution, cover: cover.clone(), uncut: None, outline: None },
            keyed: "k".into(),
        };
        (d, Cover::of(cover.as_deref()))
    }

    #[test]
    fn the_spacing_is_the_terrains_pixel_there_never_under_the_floor() {
        let f = Fine::of(&[
            ds("lidar", 2.0, None, Some(vec![[-124.0, 48.0, -122.0, 49.0]])),
            ds("ned", 10.3, None, Some(vec![[-100.0, 40.0, -99.0, 41.0]])),
            ds("glo30", 30.9, Some(1), None),
        ]);
        assert_eq!(f.cells, vec![([-124, 48], 16), ([-123, 48], 16), ([-100, 40], 14)]);
        let e7 = |x: f64, y: f64| [(x * 1e7).round() as i32, (y * 1e7).round() as i32];
        // (The lidar's z16 pixels are finer than the floor; 3DEP's z14 ~7 m; elsewhere z12's.)
        assert_eq!(f.at(e7(-123.5, 48.5)), FLOOR_M);
        assert!((f.at(e7(-99.5, 40.5)) - spacing_at(14, 40.5)).abs() < 1e-9 && f.at(e7(-99.5, 40.5)) > 7.0);
        let base = f.at(e7(10.5, 45.5));
        assert!((base - spacing_at(12, 45.5)).abs() < 1e-9 && base > 26.0 && base < 28.0, "{base}");
        // (A cell's west and south edges are its own; its east edge the next cell's.)
        assert!(f.at(e7(-122.0, 48.5)) > 20.0);
        assert_eq!(f.at(e7(-124.0, 48.0)), FLOOR_M);
        // At high latitudes the base's zoom is coarser: z11's pixel past 67°.
        assert!((f.at(e7(10.0, 70.5)) - spacing_at(11, 70.5)).abs() < 1e-9);
        assert_eq!(cell(e7(-0.5, -0.5)), [-1, -1]);
        // No dataset finer than the base: the base everywhere.
        assert!(Fine::of(&[ds("glo30", 30.9, Some(1), None)]).cells.is_empty());
    }

    #[test]
    fn the_key_names_only_the_cells_given_and_their_zooms() {
        let a = Fine::of(&[ds("lidar", 2.0, None, Some(vec![[0.0, 0.0, 2.0, 1.0]])), ds("glo30", 30.9, Some(1), None)]);
        let b = Fine::of(&[
            ds("lidar", 2.0, None, Some(vec![[0.0, 0.0, 1.0, 1.0], [5.0, 5.0, 6.0, 6.0]])),
            ds("glo30", 30.9, Some(1), None),
        ]);
        // (Cell (1, 0) differs: a unit with a way starting there keys apart, one without doesn't.)
        assert_eq!(a.key(&[[0, 0], [3, 3]]), b.key(&[[0, 0], [3, 3]]));
        assert_ne!(a.key(&[[0, 0], [1, 0]]), b.key(&[[0, 0], [1, 0]]));
        let d = tempfile::tempdir().unwrap();
        a.write(&d.path().join(FILE)).unwrap();
        assert_eq!(Fine::read(&d.path().join(FILE)).unwrap(), a);
    }
}
