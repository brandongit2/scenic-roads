//! How densely `extract` puts points along a way, from the declared elevation datasets
//! (docs/inputs.md §6.8, Densification): `FINE_M` apart where a dataset finer than `COARSE_M` has
//! data in the 1° cell of the way's first node, `COARSE_M` elsewhere. The datasets' resolutions and
//! coverages are those the input check measured (`facts.measured`); no source or area is named here.
//!
//! A cell is the point's degrees floored (west, south corner), half-open: a point on a cell's west
//! or south edge is in that cell alone. A unit's key names, of the cells holding its ways' first
//! nodes (`crate::reach::Reach::cells`), those that are fine (`Fine::key`): a dataset added or
//! measured again rebuilds only the units whose spacing it changes.

use super::declared::{Accepted, Cover};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// The spacing (m) where a fine dataset covers the way.
pub const FINE_M: f64 = 8.0;
/// The spacing (m) elsewhere; a dataset is fine when its resolution is finer than this (it holds
/// detail points this far apart would skip).
pub const COARSE_M: f64 = 15.0;

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

/// Where a fine dataset has data: 1° cells, or the world.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fine {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub world: bool,
    pub cells: BTreeSet<[i32; 2]>,
}

impl Fine {
    /// From the roads' declared datasets with their coverages (agent::tiles::Elevation).
    pub fn of(ds: &[(Accepted, Cover)]) -> Fine {
        let mut f = Fine::default();
        for (_, c) in ds.iter().filter(|(d, _)| !d.declaration.terrain && d.measured.resolution < COARSE_M) {
            f.world |= c.world;
            f.cells.extend(c.cells.iter().map(|&(x, y)| [x, y]));
        }
        if f.world {
            f.cells.clear();
        }
        f
    }

    /// From the roads' declared datasets (`declared::accepted`).
    pub fn of_accepted(ds: &[Accepted]) -> Fine {
        let with: Vec<(Accepted, Cover)> =
            ds.iter().map(|d| (d.clone(), Cover::of(d.measured.cover.as_deref()))).collect();
        Fine::of(&with)
    }

    /// Whether 1° cell `c` is fine.
    pub fn fine(&self, c: [i32; 2]) -> bool {
        self.world || self.cells.contains(&c)
    }

    /// What decides the spacing of ways whose first nodes are in `cells`, for a key: those of them
    /// that are fine.
    pub fn key(&self, cells: &[[i32; 2]]) -> String {
        let fine: Vec<String> = cells.iter().filter(|&&c| self.fine(c)).map(|c| format!("{},{}", c[0], c[1])).collect();
        format!("spacing {}", store::naming::hash16(fine.join(" ").as_bytes()))
    }

    /// From the declared datasets' accepted version the records name (`m`): an error before the
    /// build reads them (their switch, docs/inputs.md §6.8).
    pub fn of_records(root: &Path, m: &std::collections::BTreeMap<String, String>) -> Result<Fine> {
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
        let cells: Vec<String> = self.cells.iter().map(|c| format!("{},{}", c[0], c[1])).collect();
        store::naming::hash16(format!("{}|{}", self.world, cells.join(" ")).as_bytes())
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
pub fn digest_read(root: &Path, m: &std::collections::BTreeMap<String, String>) -> Result<String> {
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
        if self.fine(cell(p)) {
            FINE_M
        } else {
            COARSE_M
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dem::declared::{Data, Declaration, Measured};

    fn ds(id: &str, resolution: f64, cover: Option<Vec<[f64; 4]>>, terrain: bool) -> (Accepted, Cover) {
        let d = Accepted {
            convert: None,
            id: id.into(),
            path: format!("{id}.toml"),
            declaration: Declaration {
                surface: "ground".into(),
                heights: "EGM2008".into(),
                rank: None,
                code: 1,
                terrain,
                regions: None,
                data: Data::Geotiff {
                    urls: vec!["https://example.org/a.tif".into()],
                    index: None,
                    files: vec![],
                    read_resolution: None,
                    store: false,
                },
            },
            measured: Measured { crs: Some(4326), resolution, full: None, cover: cover.clone(), uncut: None },
            keyed: "k".into(),
        };
        (d, Cover::of(cover.as_deref()))
    }

    #[test]
    fn fine_where_a_dataset_finer_than_the_coarse_spacing_has_data() {
        let f = Fine::of(&[
            ds("lidar", 8.0, Some(vec![[-124.0, 48.0, -122.0, 49.0]]), false),
            ds("ned", 10.3, Some(vec![[-100.0, 40.0, -99.0, 41.0]]), false),
            ds("world", 30.0, None, false),
            ds("terrain", 5.0, Some(vec![[10.0, 45.0, 11.0, 46.0]]), true),
        ]);
        assert!(!f.world);
        assert_eq!(f.cells.iter().copied().collect::<Vec<_>>(), vec![[-124, 48], [-123, 48], [-100, 40]]);
        let e7 = |x: f64, y: f64| [(x * 1e7).round() as i32, (y * 1e7).round() as i32];
        assert_eq!(f.at(e7(-123.5, 48.5)), FINE_M);
        assert_eq!(f.at(e7(-99.5, 40.5)), FINE_M);
        // (A cell's west and south edges are its own; its east edge the next cell's.)
        assert_eq!(f.at(e7(-122.0, 48.5)), COARSE_M);
        assert_eq!(f.at(e7(-124.0, 48.0)), FINE_M);
        assert_eq!(f.at(e7(10.5, 45.5)), COARSE_M);
        assert_eq!(f.at(e7(2.0, 48.0)), COARSE_M);
        assert_eq!(cell(e7(-0.5, -0.5)), [-1, -1]);
        // A fine dataset everywhere: fine everywhere.
        let w = Fine::of(&[ds("all", 5.0, None, false), ds("lidar", 8.0, Some(vec![[0.0, 0.0, 1.0, 1.0]]), false)]);
        assert!(w.world && w.cells.is_empty() && w.at(e7(50.0, 50.0)) == FINE_M);
    }

    #[test]
    fn the_key_names_only_the_cells_given_that_are_fine() {
        let a = Fine::of(&[ds("lidar", 8.0, Some(vec![[0.0, 0.0, 2.0, 1.0]]), false)]);
        let b = Fine::of(&[ds("lidar", 8.0, Some(vec![[0.0, 0.0, 1.0, 1.0], [5.0, 5.0, 6.0, 6.0]]), false)]);
        // (Cell (1, 0) differs: a unit with a way starting there keys apart, one without doesn't.)
        assert_eq!(a.key(&[[0, 0], [3, 3]]), b.key(&[[0, 0], [3, 3]]));
        assert_ne!(a.key(&[[0, 0], [1, 0]]), b.key(&[[0, 0], [1, 0]]));
        let d = tempfile::tempdir().unwrap();
        a.write(&d.path().join(FILE)).unwrap();
        assert_eq!(Fine::read(&d.path().join(FILE)).unwrap(), a);
    }
}
