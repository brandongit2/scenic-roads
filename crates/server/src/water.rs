//! The water's tiles (pipeline::water; docs/formats.md "Water"): each pixel's share of sea and of
//! inland water, `/tiles/water/{z}/{x}/{y}`, 512 px.
//!
//! - z0–9 are the water layer's packs (a tile not stored is one value throughout: its stored
//!   ancestor's over it); deeper, or a stored zoom whose pack can't be read (offline, let go by the
//!   mirror), drawn here from the basemap's z14 tiles under or over it, as the build draws them.
//! - `?c=<sea>,<lake>[,<land>]` (hex colours): a PNG of the water in those colours, alpha its share
//!   (the map's raster layer); with the land's colour, the alpha that mixes water and land as light
//!   mixes (linear light: `alpha_for`), since the map blends the stored values; `?raw=1`: the shares themselves, red the sea's and green the inland
//!   water's (the coastal shading measures the shore from them, coast.worker.ts).
//! - Drawn tiles are kept (`KEPT` of them, most recently used, about 0.5 MB each), so the map's
//!   raster and the shading's asking for the same tile draw it once. At most `DRAWING` are drawn at
//!   once, on `THREADS` threads of their own (not the shared pool the queries use); a draw the map
//!   stopped waiting for still finishes and is kept. Stored tiles don't wait for draws.

use crate::cache::{self, etag_match, not_modified, Lru};
use crate::S;
use anyhow::Result;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use det::Det;
use pipeline::water::{self as wt, Cov};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

/// Tiles drawn at once (more wait), and the threads they're drawn on.
const DRAWING: usize = 4;
const THREADS: usize = 4;

/// Shares as bytes, two a pixel (sea, inland): a drawn tile kept.
type Shares = Arc<Vec<u8>>;

/// Drawn tiles kept, by (z, x, y, what they're made from): one view's (cache::VIEW_TILES), the least
/// recently used let go.
static KEPT_TILES: LazyLock<Mutex<Lru<(u8, u32, u32, u64), Shares>>> =
    LazyLock::new(|| Mutex::new(Lru::new(cache::VIEW_TILES * (wt::SIZE * wt::SIZE * 2) as u64)));

fn keep(key: (u8, u32, u32, u64), sh: Shares) {
    let n = sh.len() as u64;
    KEPT_TILES.lock().unwrap().put(key, sh, n);
}

fn blocks() -> &'static wt::Blocks {
    static B: OnceLock<wt::Blocks> = OnceLock::new();
    B.get_or_init(Default::default)
}

fn drawing() -> Arc<tokio::sync::Semaphore> {
    static D: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    D.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(DRAWING))).clone()
}

fn pool() -> &'static rayon::ThreadPool {
    static P: OnceLock<rayon::ThreadPool> = OnceLock::new();
    P.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(THREADS)
            .thread_name(|i| format!("water-{i}"))
            .build()
            .expect("the water's threads")
    })
}

/// What the tiles are made from: how they're drawn (`wt::VERSION`), the water layer's packs, the
/// lakes' fill, the enclosed seas (the surface's sea level: crate::depth) and the basemap's archives.
fn made_from(s: &crate::AppState, archives: &[String]) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(&wt::VERSION.to_le_bytes());
    h.update(s.data.layer_version(wt::LAYER).as_bytes());
    // (The lakes filled from Canada's own data, when the catalogue has them.)
    if let Some(c) = fill_content(s) {
        h.update(b"lakefill ");
        h.update(c.as_bytes());
        h.update(b"\n");
    }
    // (The seas closed off from the world ocean: where the surface is sea level, crate::depth.)
    if let Some(c) = s.data.content(wt::ENCLOSED) {
        h.update(b"enclosed ");
        h.update(c.as_bytes());
        h.update(b"\n");
    }
    for a in archives {
        h.update(a.as_bytes());
        h.update(b"\n");
    }
    u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap())
}

/// The URLs' version: changes with what the tiles are made from (main.rs meta's `versions`).
pub fn version(s: &crate::AppState) -> String {
    format!("{:012x}", made_from(s, &s.data.basemap_names()) >> 16)
}

/// The catalogue's lakes filled from Canada's own data (pipeline::lakefill): its logical and content
/// names.
fn fill_names(s: &crate::AppState) -> Option<(String, String)> {
    let l = s.data.catalogue().global.get("lakefill")?.clone();
    let c = s.data.content(&l)?;
    Some((l, c))
}

fn fill_content(s: &crate::AppState) -> Option<String> {
    fill_names(s).map(|(_, c)| c)
}

/// The catalogue's fill, opened once (kept while the catalogue names it): memory-mapped from this
/// Mac's mirror (793 MB, 2026-10-15), never read into memory, but read whole from the NAS when the
/// mirror lacks it. One request opens it while the others wait for it (the lock is held across
/// the opening). One that can't be read fails the tile's request, which is tried again on the
/// next: a tile is never drawn without the fill under a version that names it.
fn fill(s: &crate::AppState) -> Result<Option<Arc<pipeline::lakefill::Fill>>> {
    static KEPT: OnceLock<Mutex<Option<(String, Arc<pipeline::lakefill::Fill>)>>> = OnceLock::new();
    let Some((logical, content)) = fill_names(s) else { return Ok(None) };
    let mut kept = KEPT.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((c, f)) = kept.as_ref() {
        if *c == content {
            return Ok(Some(f.clone()));
        }
    }
    let Some((c, whole)) = s.data.whole(&logical)? else { return Ok(None) };
    let f = Arc::new(pipeline::lakefill::Fill::from_source(Box::new(whole), &c)?);
    *kept = Some((c, f.clone()));
    Ok(Some(f))
}

/// The z14 tile x/y from each basemap archive that has it, keyed by its archive (by content name)
/// (or piece) and where its bytes are in it: tiles with the same bytes, the open sea's, share a
/// key, and another basemap's never do.
fn z14(s: &S, archives: &[String], x: u32, y: u32) -> Result<Vec<wt::Stored>> {
    let mut out = Vec::new();
    for a in archives {
        // (From the piece this Mac downloaded when it has it: keyed by the piece then.)
        let (name, pm) = s.basemap.source(&s.data, a, wt::BASE_Z, x, y)?;
        if let Some((off, len)) = pm.locate(wt::BASE_Z, x, y)? {
            let bytes = pm.source().read_at(off, len as usize)?;
            let key = xxhash_rust::xxh3::xxh3_64_with_seed(name.as_bytes(), off);
            out.push(wt::Stored { key, bytes: Arc::new(bytes) });
        }
    }
    // The fill's lakes there, as a tile of their own beside the basemap's (only in its reach:
    // elsewhere it has none, and away from the NAS a region there hasn't downloaded it).
    if !pipeline::lakefill::reaches(wt::BASE_Z, x, y) {
        return Ok(out);
    }
    if let Some(f) = fill(s)? {
        out.extend(<pipeline::lakefill::Fill as wt::Extra>::z14(&f, x, y));
    }
    Ok(out)
}

/// A stored tile, if its pack has it (None: not stored, or no water layer).
fn stored(s: &S, z: u8, x: u32, y: u32) -> Result<Option<Cov>> {
    match s.data.tile(wt::LAYER, z, x, y)? {
        Some((b, _)) => Ok(Some(Cov::from_png(b.bytes())?)),
        None => Ok(None),
    }
}

/// Tile z/x/y's shares if they're stored (Ok(None): to be drawn).
fn from_store(s: &S, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
    let has_layer = s.data.catalogue().layers.contains_key(wt::LAYER);
    if has_layer && z <= wt::STORED_MAXZ {
        let from_packs = (|| -> Result<Option<Vec<u8>>> {
            if let Some(c) = stored(s, z, x, y)? {
                return Ok(Some(c.to_bytes()));
            }
            Ok(wt::uniform_from_ancestor(z, x, y, &|a, ax, ay| stored(s, a, ax, ay))?
                .map(|(se, i)| [se, i].repeat(wt::SIZE * wt::SIZE)))
        })();
        match from_packs {
            Ok(Some(b)) => return Ok(Some(b)),
            // (A stored zoom the basemap can stand in for at a bearable cost: z9's 1,024 tiles.)
            Ok(None) | Err(_) if z >= wt::STORED_MAXZ => {}
            Ok(None) => anyhow::bail!("water {z}/{x}/{y}: no stored tile nor ancestor"),
            Err(e) => return Err(e),
        }
    }
    if z < wt::STORED_MAXZ {
        anyhow::bail!("water {z}/{x}/{y}: not stored, and too coarse to draw here");
    }
    Ok(None)
}

/// When a tile can't be drawn (the basemap unreadable: away from the NAS without it in the mirror),
/// its nearest stored ancestor's pixels over it, scaled up (bilinear): the water as the coarser zoom
/// has it, rather than none (a failed tile is asked for again and again, and fails the coastal
/// shading's tiles around it).
fn fallback(s: &S, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
    for a in (0..=z.min(wt::STORED_MAXZ)).rev() {
        let d = z - a;
        let Some(c) = stored(s, a, x >> d, y >> d)? else { continue };
        let span = wt::SIZE as f64 / f64::from(1u32 << d);
        let (ox, oy) = (f64::from(x - ((x >> d) << d)) * span, f64::from(y - ((y >> d) << d)) * span);
        let n = wt::SIZE;
        let at = |v: &[f32], px: f64, py: f64| {
            let (fx, fy) = ((px - 0.5).clamp(0.0, (n - 1) as f64), (py - 0.5).clamp(0.0, (n - 1) as f64));
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
            let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
            let row = |yy: usize| v[yy * n + x0] * (1.0 - tx) + v[yy * n + x1] * tx;
            row(y0) * (1.0 - ty) + row(y1) * ty
        };
        let mut out = Vec::with_capacity(n * n * 2);
        for py in 0..n {
            for px in 0..n {
                let (sx, sy) = (ox + (px as f64 + 0.5) * span / n as f64, oy + (py as f64 + 0.5) * span / n as f64);
                out.push(wt::byte(at(&c.sea, sx, sy)));
                out.push(wt::byte(at(&c.inland, sx, sy)));
            }
        }
        return Ok(Some(out));
    }
    Ok(None)
}

/// Tile z/x/y's shares drawn from the basemap, on the water's own threads.
fn draw(s: &S, archives: &[String], z: u8, x: u32, y: u32) -> Result<Vec<u8>> {
    let get = |cx: u32, cy: u32| z14(s, archives, cx, cy);
    Ok(pool().install(|| wt::cover(z, x, y, &get, blocks()))?.to_bytes())
}

/// The seas closed off from the world ocean (pipeline::water::Enclosed), as the catalogue has them,
/// read once a content; None without them (a water layer made before them: all its sea the
/// ocean's).
fn enclosed(s: &crate::AppState) -> Result<Option<Arc<wt::Enclosed>>> {
    static KEPT: OnceLock<Mutex<Option<(String, Arc<wt::Enclosed>)>>> = OnceLock::new();
    let Some(content) = s.data.content(wt::ENCLOSED) else { return Ok(None) };
    let mut kept = KEPT.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((c, e)) = kept.as_ref() {
        if *c == content {
            return Ok(Some(e.clone()));
        }
    }
    let Some(b) = s.data.global(wt::ENCLOSED)? else { return Ok(None) };
    let e = Arc::new(wt::Enclosed::from_bytes(&b)?);
    *kept = Some((content, e.clone()));
    Ok(Some(e))
}

/// Which of tile z/x/y's 256 × 256 pixels (their middles) lie on an enclosed sea's pixels; empty
/// when none does.
pub fn enclosed_in(e: &wt::Enclosed, z: u8, x: u32, y: u32) -> Vec<bool> {
    let g = f64::from((wt::SIZE as u32) << e.z);
    let n = f64::from(1u32 << z);
    let at = |t: u32, p: f64| ((f64::from(t) + p) / n * g) as u32;
    let (x0, y0, x1, y1) = (at(x, 0.0), at(y, 0.0), at(x, 1.0).max(at(x, 0.0) + 1), at(y, 1.0).max(at(y, 0.0) + 1));
    if !e.meets(x0, y0, x1, y1) {
        return Vec::new();
    }
    (0..256 * 256)
        .map(|p| {
            let (px, py) = ((p % 256) as f64 + 0.5, (p / 256) as f64 + 0.5);
            e.has(at(x, px / 256.0), at(y, py / 256.0))
        })
        .collect()
}

/// A terrain tile's share of sea (`sea_shares`).
pub struct Sea {
    /// Each of the terrain's 256 × 256 pixels' share, 0–1.
    pub share: Vec<f32>,
    /// Whether each pixel's sea is a sea closed off from the world ocean (`enclosed_in`; empty:
    /// none in the tile), whose surface isn't sea level.
    pub enclosed: Vec<bool>,
    /// As the water tiles have it (kept, stored or drawn), not their nearest stored ancestor's
    /// stand-in (the basemap unreadable): a tile served from it can be cached for good.
    pub exact: bool,
}

/// Tile z/x/y's key among the water tiles kept (by what they're made from: it changes with the
/// URLs' water version); None where the water has no tile (no water layer, past its zooms).
pub fn shares_key(s: &S, z: u8, x: u32, y: u32) -> Option<(u8, u32, u32, u64)> {
    if !s.data.catalogue().layers.contains_key(wt::LAYER) || z > wt::MAXZ {
        return None;
    }
    Some((z, x, y, made_from(s, &s.data.basemap_names())))
}

/// Tile z/x/y's share of sea at the terrain's 256 px (0–1, each the mean of the water tile's 2 × 2
/// pixels over it), as the map's water tiles have it (kept, stored, drawn from the basemap, else
/// the nearest stored ancestor's), and which pixels are on a sea closed off from the world ocean:
/// the depth's sea, its floor's sea level, and the world ocean's, the water's surface's (crate::depth).
/// None without the water layer, or past its zooms.
pub fn sea_shares(s: &S, z: u8, x: u32, y: u32) -> Result<Option<Sea>> {
    let Some(key) = shares_key(s, z, x, y) else { return Ok(None) };
    let archives = s.data.basemap_names();
    let hit = KEPT_TILES.lock().unwrap().get(&key);
    let (sh, exact) = match hit {
        Some(sh) => (sh, true),
        None => match from_store(s, z, x, y).and_then(|b| b.map_or_else(|| draw(s, &archives, z, x, y), Ok)) {
            Ok(b) => {
                let sh = Arc::new(b);
                keep(key, sh.clone());
                (sh, true)
            }
            Err(e) => match fallback(s, z, x, y)? {
                Some(b) => (Arc::new(b), false),
                None => return Err(e),
            },
        },
    };
    let n = wt::SIZE;
    let at = |px: usize, py: usize| f32::from(sh[(py * n + px) * 2]) / 255.0;
    let share = (0..256 * 256)
        .map(|p| {
            let (i, j) = (2 * (p % 256), 2 * (p / 256));
            (at(i, j) + at(i + 1, j) + at(i, j + 1) + at(i + 1, j + 1)) * 0.25
        })
        .collect();
    let enclosed = enclosed(s)?.map(|e| enclosed_in(&e, z, x, y)).unwrap_or_default();
    Ok(Some(Sea { share, enclosed, exact }))
}

fn png(px: &[u8], w: usize, color: png::ColorType) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w as u32, w as u32);
    e.set_color(color);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::Balanced);
    let mut wr = e.write_header()?;
    wr.write_image_data(px)?;
    wr.finish()?;
    Ok(out)
}

/// A colour `rrggbb` (hex), or None.
fn hex(c: &str) -> Option<[f32; 3]> {
    let c = c.trim_start_matches('#');
    if c.len() != 6 || !c.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([0, 2, 4].map(|i| f32::from(u8::from_str_radix(&c[i..i + 2], 16).unwrap_or(0))))
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).dpowf(2.4)
    }
}

fn to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.dpowf(1.0 / 2.4) - 0.055
    }
}

/// A colour's luminance (linear light), from 0–255 sRGB.
fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * to_linear(c[0] / 255.0) + 0.7152 * to_linear(c[1] / 255.0) + 0.0722 * to_linear(c[2] / 255.0)
}

/// The alpha (0–255) that shows `share` (0–1) of water `w` over land `l` as the light of the two
/// mixed would look. The map blends the stored (sRGB) values, `l + alpha (w − l)`, which in light
/// gives the darker of the two more than its share: half a pixel of dark water on light land looks
/// three quarters water. Here the blend's lightness (its luminance, sRGB-encoded, which the stored
/// blend follows closely) is the mixed light's.
pub fn alpha_for(share: f32, w: [f32; 3], l: [f32; 3]) -> f32 {
    let (yw, yl) = (luminance(w), luminance(l));
    let (ew, el) = (to_srgb(yw), to_srgb(yl));
    if (ew - el).abs() < 1e-3 {
        return share * 255.0;
    }
    let target = to_srgb(share * yw + (1.0 - share) * yl);
    ((target - el) / (ew - el)).clamp(0.0, 1.0) * 255.0
}

/// The colours a tile is drawn in: the sea's, the lakes', and the land's under them if known.
type Colours = ([f32; 3], [f32; 3], Option<[f32; 3]>);

/// The tile as the map's raster (water in `sea`/`lake`, alpha its share; the sea's and the inland
/// water's shares summed, overlaps at most whole; over a known land, the share as light mixes it)
/// or raw (red the sea, green the inland water).
fn encode(sh: &[u8], colours: Option<Colours>) -> Result<Vec<u8>> {
    let n = wt::SIZE * wt::SIZE;
    match colours {
        None => {
            let mut px = Vec::with_capacity(n * 3);
            for p in sh.chunks_exact(2) {
                px.extend_from_slice(&[p[0], p[1], 0]);
            }
            png(&px, wt::SIZE, png::ColorType::Rgb)
        }
        Some((sea, lake, land)) => {
            // The alpha for each share, sea and lake (a pixel of both: mixed by their shares).
            let lut = |w: [f32; 3]| -> Vec<u8> {
                (0..=255).map(|t| land.map_or(t as u8, |l| alpha_for(t as f32 / 255.0, w, l).round() as u8)).collect()
            };
            let (sea_a, lake_a) = (lut(sea), lut(lake));
            let mut px = Vec::with_capacity(n * 4);
            for p in sh.chunks_exact(2) {
                let (s, i) = (f32::from(p[0]), f32::from(p[1]));
                let t = s + i;
                // (Where there's none, the sea's colour: a pixel sampled between it and water then
                // blends no black in.)
                let rgb = if t > 0.0 {
                    [0, 1, 2].map(|k| ((sea[k] * s + lake[k] * i) / t).round() as u8)
                } else {
                    sea.map(|v| v as u8)
                };
                let k = t.min(255.0) as usize;
                let a =
                    if t > 0.0 { ((f32::from(sea_a[k]) * s + f32::from(lake_a[k]) * i) / t).round() as u8 } else { 0 };
                px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
            }
            png(&px, wt::SIZE, png::ColorType::Rgba)
        }
    }
}

pub async fn water_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    if z > wt::MAXZ || x >= 1 << z || y >= 1 << z {
        return StatusCode::NOT_FOUND.into_response();
    }
    let v = cache::versioned(q.as_deref());
    let query: std::collections::HashMap<String, String> = q.as_deref().map(|q| url_pairs(q)).unwrap_or_default();
    let colours = match query.get("c") {
        Some(c) => {
            let cs: Vec<Option<[f32; 3]>> = c.split(',').map(hex).collect();
            match cs[..] {
                [Some(a), Some(b)] => Some((a, b, None)),
                [Some(a), Some(b), Some(l)] => Some((a, b, Some(l))),
                _ => return StatusCode::BAD_REQUEST.into_response(),
            }
        }
        None if query.contains_key("raw") => None,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let archives = s.data.basemap_names();
    let from = made_from(&s, &archives);
    let etag = format!("\"w{from:016x}-{z}-{x}-{y}-{}\"", query.get("c").map_or("raw", String::as_str));
    if etag_match(&headers, &etag) {
        return not_modified(&etag, v);
    }
    let key = (z, x, y, from);
    let hit = KEPT_TILES.lock().unwrap().get(&key);
    let sh = match hit {
        Some(sh) => sh,
        None => {
            // Stored first (no waiting on draws), then drawn: the permit and the keeping go with the
            // draw, so one the map stopped waiting for still counts and is kept.
            let s2 = s.clone();
            let stored = match tokio::task::spawn_blocking(move || from_store(&s2, z, x, y)).await {
                Ok(r) => r,
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            let got = match stored {
                Ok(Some(b)) => {
                    let sh = Arc::new(b);
                    keep(key, sh.clone());
                    Ok(sh)
                }
                Ok(None) => {
                    let Ok(permit) = drawing().acquire_owned().await else {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    };
                    let s2 = s.clone();
                    match tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let sh = Arc::new(draw(&s2, &archives, z, x, y)?);
                        keep(key, sh.clone());
                        anyhow::Ok(sh)
                    })
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    }
                }
                Err(e) => Err(e),
            };
            match got {
                Ok(sh) => sh,
                Err(e) => {
                    eprintln!("water {z}/{x}/{y}: {e:#}");
                    // The coarser zoom's water instead, not kept, without an ETag, not stored, and never
                    // cached for good (cache::NotForGood): the tile is drawn properly once the basemap can be
                    // read again.
                    let s2 = s.clone();
                    let fb = tokio::task::spawn_blocking(move || fallback(&s2, z, x, y)).await;
                    let Ok(Ok(Some(b))) = fb else { return StatusCode::SERVICE_UNAVAILABLE.into_response() };
                    return match tokio::task::spawn_blocking(move || encode(&b, colours)).await {
                        Ok(Ok(body)) => {
                            let mut r = cache::respond(body, "image/png", false, None, false);
                            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                            r.extensions_mut().insert(cache::NotForGood);
                            r
                        }
                        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    };
                }
            }
        }
    };
    match tokio::task::spawn_blocking(move || encode(&sh, colours)).await {
        Ok(Ok(body)) => cache::respond(body, "image/png", false, Some(&etag), v),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A query's pairs (no percent-decoding needed: hex colours and flags).
fn url_pairs(q: &str) -> std::collections::HashMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (p.to_string(), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stored_tiles_and_their_uniform_children_coloured_or_raw() {
        // A catalogue whose water layer is one root pack holding z0: its left half sea, its bottom
        // right quarter a lake.
        let mut c = Cov::uniform(0, 0);
        for y in 0..wt::SIZE {
            for x in 0..wt::SIZE {
                if x < wt::SIZE / 2 {
                    c.sea[y * wt::SIZE + x] = 1.0;
                } else if y >= wt::SIZE / 2 {
                    c.inland[y * wt::SIZE + x] = 1.0;
                }
            }
        }
        let c = Cov::drawn(c.sea, c.inland);
        let png0 = c.png();
        let nas = tempfile::tempdir().unwrap();
        let content = "layers/water/root/0-0-0.0123456789abcdef.pack";
        let path = nas.path().join(content);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = store::pack::PackWriter::create(
            &path,
            serde_json::json!({"layer": "water", "scope": "root", "root": "0/0/0", "encoding": "water-png"}),
            false,
        )
        .unwrap();
        w.add(0, 0, 0, &png0, png0.len() as u32).unwrap();
        w.finish().unwrap();
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.files.insert(
            "layers/water/root/0-0-0".into(),
            store::catalogue::FileRef {
                file: content.into(),
                size: std::fs::metadata(&path).unwrap().len(),
                ..Default::default()
            },
        );
        cat.layers.insert(
            "water".into(),
            store::catalogue::Layer {
                encoding: "water-png".into(),
                minzoom: 0,
                maxzoom: 9,
                root: Some("layers/water/root/0-0-0".into()),
                ..Default::default()
            },
        );
        // A basemap the catalogue names but the NAS hasn't (away, not mirrored).
        cat.basemap = vec!["layers/basemap/world-gone".into()];
        cat.files.insert(
            "layers/basemap/world-gone".into(),
            store::catalogue::FileRef {
                file: "layers/basemap/world-gone.0123456789abcdef.pmtiles".into(),
                size: 1,
                ..Default::default()
            },
        );
        store::testkit::map_root(&nas.path(), &cat);
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let get = |z, x, y, q: &str| {
            water_tile(State(s.clone()), Path((z, x, y)), RawQuery(Some(q.to_string())), HeaderMap::new())
        };
        let pixels = |b: &[u8]| {
            let mut d = png::Decoder::new(std::io::Cursor::new(b.to_vec())).read_info().unwrap();
            let mut px = vec![0u8; d.output_buffer_size().unwrap()];
            let info = d.next_frame(&mut px).unwrap();
            (info.color_type, px)
        };
        // Raw: red the sea, green the lake.
        let r = get(0, 0, 0, "raw=1").await;
        assert_eq!(r.status(), StatusCode::OK);
        let (t, px) = pixels(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap());
        assert_eq!(t, png::ColorType::Rgb);
        let at = |x: usize, y: usize| &px[(y * wt::SIZE + x) * 3..(y * wt::SIZE + x) * 3 + 3];
        assert_eq!((at(10, 10), at(500, 10), at(500, 500)), (&[255, 0, 0][..], &[0, 0, 0][..], &[0, 255, 0][..]));
        // Coloured: z1's tiles aren't stored, each one value from z0.
        for ((x, y), want) in [((0, 0), [1, 2, 3, 255]), ((1, 0), [1, 2, 3, 0]), ((1, 1), [4, 5, 6, 255])] {
            let r = get(1, x, y, "c=010203,040506&v=1").await;
            assert_eq!(r.status(), StatusCode::OK, "1/{x}/{y}");
            let (t, px) = pixels(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap());
            assert_eq!(t, png::ColorType::Rgba);
            assert!(px.chunks_exact(4).all(|p| p == want), "1/{x}/{y}");
        }
        // Deeper, drawn from the basemap, which can't be read: z0's water over it instead (10/0/0 in
        // the sea, 10/1023/1023 in the lake), not to be cached.
        for ((x, y), want) in [((0, 0), [1, 2, 3, 255]), ((1023, 1023), [4, 5, 6, 255])] {
            let r = get(10, x, y, "c=010203,040506").await;
            assert_eq!(r.status(), StatusCode::OK, "10/{x}/{y}");
            assert_eq!(r.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
            let (_, px) = pixels(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap());
            assert!(px.chunks_exact(4).all(|p| p == want), "10/{x}/{y}");
        }
        // With the land's colour: whole pixels as before; four colours are too many.
        let r = get(1, 0, 0, "c=010203,040506,ffffff").await;
        let (_, px) = pixels(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap());
        assert!(px.chunks_exact(4).all(|p| p == [1, 2, 3, 255]));
        assert_eq!(get(0, 0, 0, "c=010203,040506,ffffff,000000").await.status(), StatusCode::BAD_REQUEST);
        // A colour is needed, or raw.
        assert_eq!(get(0, 0, 0, "").await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(get(0, 0, 0, "c=0x0102,040506").await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(get(0, 0, 0, "c=zz0203,040506").await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(crate::meta_json(&s)["water"], serde_json::json!(true));
    }

    #[test]
    fn the_alpha_mixes_water_and_land_as_light_mixes() {
        let (black, white, grey) = ([0.0; 3], [255.0; 3], [128.0; 3]);
        // Half a pixel of black water on white land: as light, a quarter of the way in sRGB
        // (255 × (1 − 0.735)), not halfway.
        assert!((alpha_for(0.5, black, white) - 67.5).abs() < 0.5, "{}", alpha_for(0.5, black, white));
        // White water on black land: the other way.
        assert!((alpha_for(0.5, white, black) - 187.5).abs() < 0.5);
        // Whole pixels, and colours alike, as they were.
        for (w, l) in [(black, white), (white, black), (grey, grey)] {
            assert_eq!(alpha_for(0.0, w, l), 0.0);
            assert!((alpha_for(1.0, w, l) - 255.0).abs() < 1e-3);
        }
        assert_eq!(alpha_for(0.3, grey, grey), 0.3 * 255.0);
        // The map's own dark colours: barely changed.
        let (sea, land) = ([12.0, 22.0, 34.0], [11.0, 14.0, 19.0]);
        assert!((alpha_for(0.5, sea, land) - 127.5).abs() < 15.0);
    }

    /// A terrain tile's pixels on an enclosed sea are those whose middles lie on its pixels, at any
    /// zoom; a tile it doesn't reach has none.
    #[test]
    fn a_terrain_tile_s_pixels_on_an_enclosed_sea() {
        // Pixels 300–301 of row 5 of z0's 512, and 20–29 of row 11.
        let e = wt::Enclosed { z: 0, runs: vec![[5, 300, 302], [11, 20, 30]], bodies: Vec::new() };
        let on = |m: Vec<bool>| (0..m.len()).filter(|&p| m[p]).collect::<Vec<_>>();
        // z1/0/0: a terrain pixel an enclosed pixel.
        assert_eq!(on(enclosed_in(&e, 1, 0, 0)), (11 * 256 + 20..11 * 256 + 30).collect::<Vec<_>>());
        // z0: two a side to a terrain pixel, its middle on the odd ones.
        let mut want = vec![2 * 256 + 150];
        want.extend(5 * 256 + 10..5 * 256 + 15);
        assert_eq!(on(enclosed_in(&e, 0, 0, 0)), want);
        // z2/2/0 (columns 256–383, rows 0–127): pixels 300–301 of row 5, each two a side.
        let want: Vec<usize> = [10, 11].iter().flat_map(|r| (88..92).map(move |c| r * 256 + c)).collect();
        assert_eq!(on(enclosed_in(&e, 2, 2, 0)), want);
        assert!(enclosed_in(&e, 1, 1, 1).is_empty());
    }
}
