//! The depth layers' tiles (docs/steps.md §3; docs/formats.md, Depth): `depth` (Terrarium RGB,
//! metres below the water's surface) and `depth-class` (a byte a pixel: class, dries, source), as
//! stored, or, where the layer stores none (depth stops at each source's own zoom), made from the
//! nearest stored ancestor as the build scales a level up (pipeline::depth::fall), encoded as the
//! stored ones are (lossless WebP). Outside the layer's zooms: 404; where it has nothing (no
//! stored ancestor): 204, as every layer's missing tile (crate::cache).
//!
//! And the terrain with the floor (`terrain_floor`): the terrain's heights minus the depth on the
//! water the depth lies on (the terrain there its water's surface: the sea's 0 m, a lake's or a
//! river's its source's heights), for the map's 3D
//! floor; no second terrain is stored.

use crate::cache::Lru;
use crate::terrain::{made, respond, Got};
use crate::S;
use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use pipeline::depth::fall::{self, Grid, Kind, Stored};

use std::sync::{Arc, LazyLock, Mutex};

/// Decoded stored tiles, by layer, tile and catalogue generation (an ancestor serves many children),
/// a view's tiles of each of the two layers (cache::VIEW_TILES; an ancestor or none, at most
/// `fall::TILE`² f32s, ~260 KB, so ~97 MB at most). (Tiles made here are kept with the terrain's,
/// crate::terrain::MADE.)
type Decoded = Lru<(&'static str, u8, u32, u32, u64), Stored>;
static DECODED: LazyLock<Mutex<Decoded>> =
    LazyLock::new(|| Mutex::new(Lru::entries(2 * crate::cache::VIEW_TILES as usize)));

fn gen(s: &crate::AppState) -> u64 {
    s.data.generation.load(std::sync::atomic::Ordering::Relaxed)
}

/// A decoded tile's bytes in memory.
fn weight(d: &Stored) -> u64 {
    match d.as_deref() {
        Some(Grid::F(v)) => std::mem::size_of_val(v.as_slice()) as u64,
        Some(Grid::C(v)) => v.len() as u64,
        None => 0,
    }
}

/// Layer `layer`'s stored tile z/x/y, decoded; None where it has none.
fn decoded(s: &crate::AppState, layer: &'static str, kind: Kind, z: u8, x: u32, y: u32) -> anyhow::Result<Stored> {
    let key = (layer, z, x, y, gen(s));
    if let Some(d) = DECODED.lock().unwrap().get(&key) {
        return Ok(d);
    }
    let d = match s.data.tile(layer, z, x, y)? {
        Some((b, _)) => Some(Arc::new(fall::decode(kind, b.bytes())?)),
        None => None,
    };
    DECODED.lock().unwrap().put(key, d.clone(), weight(&d));
    Ok(d)
}

/// Tile z/x/y of a depth layer as the map is given it (stored or made), decoded: None outside it.
pub fn served(s: &crate::AppState, layer: &'static str, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Grid>> {
    let kind = Kind::of_layer(layer).ok_or_else(|| anyhow::anyhow!("{layer} isn't a depth layer"))?;
    if !s.data.catalogue().layers.contains_key(layer) {
        return Ok(None);
    }
    Ok(fall::served(&|z, x, y| decoded(s, layer, kind, z, x, y), kind, z, x, y)?.map(|(g, _)| g))
}

async fn tile(s: S, layer: &'static str, z: u8, x: u32, y: u32, q: Option<String>, headers: HeaderMap) -> Response {
    let v = crate::cache::versioned(q.as_deref());
    if !fall::in_layer(z, x, y) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let got = tokio::task::spawn_blocking(move || {
        if !s.data.catalogue().layers.contains_key(layer) {
            return None;
        }
        if let Some(g) = crate::terrain::stored(&s, layer, z, x, y, &headers, "") {
            return Some(g);
        }
        made(layer, gen(&s), z, x, y, || served(&s, layer, z, x, y)?.map(|t| fall::encode(&t)).transpose())
    })
    .await
    .unwrap_or(Some(Got::Failed));
    respond(got, v)
}

pub async fn depth_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    tile(s, pipeline::depth::LAYER, z, x, y, q, headers).await
}

pub async fn class_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    tile(s, pipeline::depth::CLASS_LAYER, z, x, y, q, headers).await
}

/// The floor under the water: terrain heights `e` (a tile's) with the depth subtracted where the
/// depth's class says water (its depth below the water's surface, the terrain's heights there).
pub fn floor(e: &mut [f32], depth: &[f32], class: &[u8]) {
    for i in 0..e.len().min(depth.len()).min(class.len()) {
        if class[i] & 7 != pipeline::depth::C_NONE && depth[i] < pipeline::depth::NODATA_MIN {
            e[i] += depth[i].min(0.0);
        }
    }
}

/// The terrain tile z/x/y with the floor (`floor`), encoded as the terrain's are
/// (crate::terrain::encode); None outside the terrain.
pub fn terrain_floor(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let Some(mut e) = crate::terrain::terrain_f32(s, z, x, y)? else { return Ok(None) };
    if let (Some(Grid::F(d)), Some(Grid::C(c))) =
        (served(s, pipeline::depth::LAYER, z, x, y)?, served(s, pipeline::depth::CLASS_LAYER, z, x, y)?)
    {
        floor(&mut e, &d, &c);
    }
    Ok(crate::terrain::encode(e).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_is_the_surface_less_the_depth_on_the_water_alone() {
        // A lake at 120 m, 8 m deep; the land beside it (class none) keeps its height even where
        // the depth's band reaches it.
        let mut e = vec![120.0, 120.0, 135.0, 140.0];
        let d = vec![-8.0, pipeline::depth::NODATA, 0.5, -3.0];
        let c = vec![
            pipeline::depth::C_NATIVE | (9 << 4),
            pipeline::depth::C_NATIVE,
            pipeline::depth::C_NONE,
            pipeline::depth::C_NONE,
        ];
        floor(&mut e, &d, &c);
        assert_eq!(e, vec![112.0, 120.0, 135.0, 140.0]);
    }
}
