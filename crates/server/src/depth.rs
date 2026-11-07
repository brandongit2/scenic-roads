//! The depth layers' tiles (docs/steps.md §3; docs/formats.md, Depth): `depth` (Terrarium RGB,
//! metres below the water's surface) and `depth-class` (a byte a pixel: class, dries, source), as
//! stored, or, where the layer stores none (depth stops at each source's own zoom), made from the
//! nearest stored ancestor as the build scales a level up (terrain::depth::fall), encoded as the
//! stored ones are (lossless WebP). Outside the layer's zooms: 404; where it has nothing (no
//! stored ancestor): 204, as every layer's missing tile (crate::cache).
//!
//! And the terrain with the floor (`terrain_floor`): the water's surface less the depth on the water
//! the depth lies on, for the map's 3D floor; and the water's surface itself (`surface_over`),
//! where the map draws the water over that floor, and the contours' ground. The surface is sea
//! level on the world ocean (a pixel half sea or more in the water tiles, crate::water::sea_shares:
//! the depth's own rule for water, and not on a sea closed off from the ocean,
//! inputs::water::Enclosed), elsewhere the terrain's heights (a lake's, a river's or an enclosed
//! sea's its source's: the Caspian's −28 m in GLO-30): the terrain keeps the source's heights at
//! sea, its bathymetry and artefacts included, so it is no surface there. The floor is that surface
//! less the depth, which is below it (an enclosed sea's below its own: terrain::depth::surface),
//! so never above it. No second terrain is stored.

use crate::cache::Lru;
use crate::terrain::{made, respond, Got};
use crate::S;
use ::terrain::depth::fall::{self, Grid, Kind, Stored};
use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

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

/// Tile z/x/y of a depth layer as the map is given it (stored or made), decoded: None outside it, or
/// where the downloads don't reach away from the NAS (Data::reaches: never made from a coarse
/// ancestor that does).
pub fn served(s: &crate::AppState, layer: &'static str, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Grid>> {
    let kind = Kind::of_layer(layer).ok_or_else(|| anyhow::anyhow!("{layer} isn't a depth layer"))?;
    if !s.data.catalogue().layers.contains_key(layer) || !s.data.reaches(z, x, y) {
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
    tile(s, ::terrain::depth::LAYER, z, x, y, q, headers).await
}

pub async fn class_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    tile(s, ::terrain::depth::CLASS_LAYER, z, x, y, q, headers).await
}

/// The water's surface over terrain heights `e` (a tile's): sea level where `sea` (each pixel's
/// share of sea) is half or more (`at_sea`) and the sea is the world ocean's (`enclosed`, each
/// pixel's: an enclosed sea's; empty, none), the terrain's heights elsewhere. Not blended: a shore
/// pixel's mean of sea level and the source's bathymetry there would stand below sea level, a ring
/// of contours round every coast.
pub fn surface(e: &mut [f32], sea: &[f32], enclosed: &[bool]) {
    for (i, (v, &s)) in e.iter_mut().zip(sea).enumerate() {
        if on_ocean(s, enclosed, i) {
            *v = 0.0;
        }
    }
}

/// Whether pixel `i`, with share `s` of sea, is the world ocean's (`surface`).
fn on_ocean(s: f32, enclosed: &[bool], i: usize) -> bool {
    at_sea(s) && !enclosed.get(i).copied().unwrap_or(false)
}

/// Whether a pixel with share `s` of sea is the sea's (half or more: as the depth's water,
/// terrain::depth).
fn at_sea(s: f32) -> bool {
    s >= 0.5
}

/// The floor under the water: the depth subtracted where the depth's class says water, from the
/// water's surface its depth is below (`surface`): sea level on the world ocean (`sea`, each
/// pixel's share and whether it's an enclosed sea's), else terrain heights `e` (a lake's, a
/// river's or an enclosed sea's surface: the depth measures the Caspian's below its −28 m,
/// terrain::depth::surface; None: the terrain's heights throughout), so never above it; the land
/// as it is.
pub fn floor(e: &mut [f32], sea: Option<&crate::water::Sea>, depth: &[f32], class: &[u8]) {
    for i in 0..e.len().min(depth.len()).min(class.len()) {
        if class[i] & 7 != ::terrain::depth::C_NONE && depth[i] < ::terrain::depth::NODATA_MIN {
            let (share, enclosed) = sea.map_or((0.0, &[][..]), |s| (s.share[i], &s.enclosed[..]));
            let top = if on_ocean(share, enclosed, i) { 0.0 } else { e[i] };
            e[i] = top + depth[i].min(0.0);
        }
    }
}

/// The terrain tile z/x/y with the floor (`floor`), encoded as the terrain's are
/// (crate::terrain::encode), and whether it may be kept (not from the water's stand-in shares:
/// crate::water::Sea::exact); None outside the terrain.
pub fn terrain_floor(s: &crate::S, z: u8, x: u32, y: u32) -> anyhow::Result<Option<(Vec<u8>, bool)>> {
    let Some(mut e) = crate::terrain::terrain_f32(s, z, x, y)? else { return Ok(None) };
    let mut exact = true;
    if let (Some(Grid::F(d)), Some(Grid::C(c))) =
        (served(s, ::terrain::depth::LAYER, z, x, y)?, served(s, ::terrain::depth::CLASS_LAYER, z, x, y)?)
    {
        let sea = crate::water::sea_shares(s, z, x, y)?;
        exact = sea.as_ref().is_none_or(|s| s.exact);
        floor(&mut e, sea.as_ref(), &d, &c);
    }
    Ok(crate::terrain::encode(e).ok().map(|b| (b, exact)))
}

/// Heights `e` (the terrain tile z/x/y's) as the water's surface (`surface`) by the water tiles'
/// shares there, and whether those are exact (not the stand-in: crate::water::Sea::exact); None
/// without the water layer or past its zooms (the terrain is its own surface there).
pub fn surface_over(s: &crate::S, z: u8, x: u32, y: u32, e: &mut [f32]) -> anyhow::Result<bool> {
    let Some(sea) = crate::water::sea_shares(s, z, x, y)? else { return Ok(true) };
    surface(e, &sea.share, &sea.enclosed);
    Ok(sea.exact)
}

/// What the water's surface over a terrain tile is (`surface_of`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Surface {
    /// The terrain tile itself: no water layer, past its zooms, or no pixel on the world ocean.
    Terrain,
    /// Sea level over some pixel, by the water tiles' own shares.
    Sea,
    /// Unknown: the shares are their nearest stored ancestor's stand-in (the basemap unreadable),
    /// so made from them and never kept (asked for again, made from the water tiles then).
    StandIn,
}

impl Surface {
    /// By a tile's share of sea (crate::water::sea_shares; None: no water there).
    fn of(sea: Option<&crate::water::Sea>) -> Surface {
        match sea {
            None => Surface::Terrain,
            Some(sea) if !sea.exact => Surface::StandIn,
            Some(sea) if sea.share.iter().enumerate().any(|(i, &s)| on_ocean(s, &sea.enclosed, i)) => Surface::Sea,
            Some(_) => Surface::Terrain,
        }
    }
}

/// Whether tiles have a pixel at sea, by the water's key (crate::water::shares_key: what the water
/// tiles are made from, so a new water version asks again): a tile's surface asked for again, the
/// map's mesh's and tint's alike, doesn't draw its water again. 2 bytes an entry and its key.
static AT_SEA: LazyLock<Mutex<Lru<(u8, u32, u32, u64), Surface>>> = LazyLock::new(|| Mutex::new(Lru::entries(1 << 16)));

/// What the water's surface over terrain tile z/x/y is.
pub fn surface_of(s: &crate::S, z: u8, x: u32, y: u32) -> anyhow::Result<Surface> {
    let Some(key) = crate::water::shares_key(s, z, x, y) else { return Ok(Surface::Terrain) };
    if let Some(k) = AT_SEA.lock().unwrap().get(&key) {
        return Ok(k);
    }
    let k = Surface::of(crate::water::sea_shares(s, z, x, y)?.as_ref());
    if k != Surface::StandIn {
        AT_SEA.lock().unwrap().put(key, k, 0);
    }
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_is_the_surface_less_the_depth_on_the_water_alone() {
        // A lake at 120 m, 8 m deep; the land beside it (class none) keeps its height even where
        // the depth's band reaches it.
        let mut e = vec![120.0, 120.0, 135.0, 140.0];
        let d = vec![-8.0, ::terrain::depth::NODATA, 0.5, -3.0];
        let c = vec![
            ::terrain::depth::C_NATIVE | (9 << 4),
            ::terrain::depth::C_NATIVE,
            ::terrain::depth::C_NONE,
            ::terrain::depth::C_NONE,
        ];
        floor(&mut e, Some(&sea_of(vec![0.0; 4], Vec::new())), &d, &c);
        assert_eq!(e, vec![112.0, 120.0, 135.0, 140.0]);
    }

    /// At sea the terrain keeps the source's heights (its bathymetry, −35 m here; a shore's −2 m),
    /// and the floor stands on sea level: 0 m less the depth, whatever the terrain there, a pixel
    /// half sea as the sea's; the depth over the sea as it was. The surface is sea level at sea
    /// (half a pixel or more), the land's own height ashore, a polder's −6 m too.
    #[test]
    fn the_seas_floor_and_surface_stand_on_sea_level() {
        let c = ::terrain::depth::C_NATIVE | (3 << 4);
        let n = ::terrain::depth::C_NONE;
        let nd = ::terrain::depth::NODATA;
        let (d, cl) = (vec![-40.0, -12.0, -1.0, nd, nd], vec![c, c, c, n, n]);
        let sea = [1.0, 1.0, 0.5, 0.25, 0.0];
        let mut e = vec![-35.0, -2.0, 10.0, -20.0, -6.0];
        floor(&mut e, Some(&sea_of(sea.to_vec(), Vec::new())), &d, &cl);
        assert_eq!(e, vec![-40.0, -12.0, -1.0, -20.0, -6.0]);
        let mut e = vec![-35.0, -2.0, 10.0, -20.0, -6.0];
        surface(&mut e, &sea, &[]);
        assert_eq!(e, vec![0.0, 0.0, 0.0, -20.0, -6.0]);
    }

    /// A sea closed off from the world ocean (the Caspian, OSM's coastline round it) is no sea
    /// level: its surface is the terrain's (GLO-30's −28 m), the world ocean's beside it 0 m. Its
    /// floor is that surface less the depth, which the depth measures below it (GEBCO's −31 m a
    /// depth of −3 m, its −20 m on the shelf 0 m: a floor at −28 m).
    #[test]
    fn an_enclosed_sea_s_surface_is_its_source_s() {
        let mut e = vec![-28.0, -28.0, -28.0, -1.5, 12.0];
        let (sea, enclosed) = ([1.0, 1.0, 1.0, 1.0, 0.0], [true, true, true, false, false]);
        surface(&mut e, &sea, &enclosed);
        assert_eq!(e, vec![-28.0, -28.0, -28.0, 0.0, 12.0]);
        let c = ::terrain::depth::C_NATIVE | (3 << 4);
        let mut e = vec![-28.0, -28.0, -28.0, -1.5, 12.0];
        let (d, cl) = ([-3.0, -872.0, 0.0, -6.0, ::terrain::depth::NODATA], [c, c, c, c, ::terrain::depth::C_NONE]);
        floor(&mut e, Some(&sea_of(sea.to_vec(), enclosed.to_vec())), &d, &cl);
        assert_eq!(e, vec![-31.0, -900.0, -28.0, -6.0, 12.0]);
    }

    fn sea_of(share: Vec<f32>, enclosed: Vec<bool>) -> crate::water::Sea {
        crate::water::Sea { share, enclosed, exact: true }
    }

    /// The surface is made only over the world ocean: a tile without a pixel at sea (a shore's 0.4 of one,
    /// a polder's or a lake's none), past the water's zooms or without its layer is the terrain
    /// itself, served as stored; shares that are an ancestor's stand-in are neither (made from,
    /// never kept nor taken as no sea: a tile served as stored is cached for good).
    #[test]
    fn the_surface_is_made_only_over_the_sea() {
        use crate::water::Sea;
        let of = |share: Vec<f32>, exact| Surface::of(Some(&Sea { share, enclosed: Vec::new(), exact }));
        assert_eq!(Surface::of(None), Surface::Terrain);
        assert_eq!(of(vec![0.0, 0.4, 0.0], true), Surface::Terrain);
        assert_eq!(of(vec![0.0, 0.5, 0.0], true), Surface::Sea);
        assert_eq!(of(vec![0.0, 0.0, 0.0], false), Surface::StandIn);
        assert_eq!(of(vec![1.0, 1.0, 1.0], false), Surface::StandIn);
        // An enclosed sea's tile (the Caspian's open water) is the terrain's, the ocean's beside it not.
        let sea = |enclosed: Vec<bool>| Surface::of(Some(&Sea { share: vec![1.0, 1.0], enclosed, exact: true }));
        assert_eq!(sea(vec![true, true]), Surface::Terrain);
        assert_eq!(sea(vec![true, false]), Surface::Sea);
    }
}
