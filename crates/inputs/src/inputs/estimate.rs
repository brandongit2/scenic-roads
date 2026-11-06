//! A `lake-estimate` depth source (docs/inputs.md §6.10): its declarations' format and its
//! method, from its `params`, with its grids' zooms. The lake-depth step makes its grids
//! (crate::depth::lakes, which re-exports these); the input check validates its params
//! (`sources`). (Task #916 step 6d: from crate::depth::lakes.)

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

/// The declarations' format: a depth source made by the lake-depth step.
pub const FORMAT: &str = "lake-estimate";

/// How a source is estimated (its declaration's `params.method`).
#[derive(Clone, Debug, PartialEq)]
pub enum Method {
    /// GLOBathy's maximum depths on HydroLAKES' outlines, from the source's folder.
    Globathy,
    /// The basemap's small lakes from the land's slope, leaving out the lakes under the HydroLAKES
    /// polygons of source `outlines`.
    LandSlope { outlines: String },
}

impl Method {
    /// The method a `lake-estimate` declaration's params name.
    pub fn of(params: &BTreeMap<String, serde_json::Value>) -> Result<Method> {
        match params.get("method").and_then(|v| v.as_str()) {
            Some("globathy") => Ok(Method::Globathy),
            Some("land-slope") => Ok(Method::LandSlope {
                outlines: params
                    .get("outlines")
                    .and_then(|v| v.as_str())
                    .context("`params.outlines`: the source whose HydroLAKES outlines it leaves out")?
                    .to_string(),
            }),
            m => bail!("`params.method` is {m:?}: globathy or land-slope"),
        }
    }

    /// Its grids' zoom and the zoom of the tiles they're cut along (one grid a tile).
    pub fn zooms(&self) -> (u8, u8) {
        match self {
            Method::Globathy => (GLOBATHY_Z, 8),
            Method::LandSlope { .. } => (SMALL_Z, 10),
        }
    }
}

/// GLOBathy's grids' zoom (~40 m at 50°N: an estimate, held coarser than its 1″ rasters).
pub const GLOBATHY_Z: u8 = 11;
/// The small lakes' (the basemap's own finest).
pub const SMALL_Z: u8 = 14;
