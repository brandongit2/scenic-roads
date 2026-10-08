//! The water's tiles (pipeline::water; docs/formats.md "Water"): each pixel's share of sea and of
//! inland water, `/tiles/water/{z}/{x}/{y}`, 512 px.
//!
//! - z0–9 are the water layer's packs (a tile not stored is one value throughout: its stored
//!   ancestor's over it); deeper, or a stored zoom whose pack can't be read (offline, let go by the
//!   mirror), drawn here from the basemap's z14 tiles under or over it, as the build draws them.
//! - `?c=<sea>,<lake>` (hex colours): a PNG of the water in those colours, alpha its share (the
//!   map's raster layer); `?raw=1`: the shares themselves, red the sea's and green the inland
//!   water's (the coastal shading measures the shore from them, coast.worker.ts).
//! - Drawn tiles are kept (`KEPT` of them, most recently used, about 0.5 MB each), so the map's
//!   raster and the shading's asking for the same tile draw it once; at most `DRAWING` are drawn at
//!   once.

use crate::tiles::{etag_match, not_modified, versioned};
use crate::S;
use anyhow::Result;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use pipeline::water::{self as wt, Cov};
use std::sync::{Arc, Mutex, OnceLock};

/// Drawn tiles kept (by z, x, y and what they're made from).
const KEPT: usize = 192;
/// Tiles drawn at once (each drawn on the shared threads; more wait).
const DRAWING: usize = 4;

/// Shares as bytes, two a pixel (sea, inland): a drawn tile kept.
type Shares = Arc<Vec<u8>>;

#[derive(Default)]
struct Kept {
    tick: u64,
    map: std::collections::HashMap<(u8, u32, u32, u64), (u64, Shares)>,
}

fn kept() -> &'static Mutex<Kept> {
    static K: OnceLock<Mutex<Kept>> = OnceLock::new();
    K.get_or_init(Default::default)
}

fn blocks() -> &'static wt::Blocks {
    static B: OnceLock<wt::Blocks> = OnceLock::new();
    B.get_or_init(Default::default)
}

fn drawing() -> &'static tokio::sync::Semaphore {
    static D: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    D.get_or_init(|| tokio::sync::Semaphore::new(DRAWING))
}

fn shares(c: &Cov) -> Vec<u8> {
    c.sea.iter().zip(&c.inland).flat_map(|(&s, &i)| [wt::byte(s), wt::byte(i)]).collect()
}

/// What the tiles are made from: the water layer's packs and the basemap's archives.
fn made_from(s: &crate::AppState, archives: &[String]) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(s.data.layer_version(wt::LAYER).as_bytes());
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

/// The z14 tile x/y from each basemap archive that has it, keyed by where its bytes are (tiles
/// with the same bytes, the open sea's, share a key).
fn z14(s: &S, archives: &[String], x: u32, y: u32) -> Result<Vec<wt::Stored>> {
    let mut out = Vec::new();
    for (k, pm) in s.basemap.opened(&s.data, archives)?.iter().enumerate() {
        if let Some((off, len)) = pm.locate(wt::BASE_Z, x, y)? {
            let bytes = pm.source().read_at(off, len as usize)?;
            out.push(wt::Stored { key: off ^ ((k as u64) << 56), bytes: Arc::new(bytes) });
        }
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

/// Tile z/x/y's shares: stored, or drawn from the basemap.
fn draw(s: &S, archives: &[String], z: u8, x: u32, y: u32) -> Result<Vec<u8>> {
    let has_layer = s.data.catalog().layers.contains_key(wt::LAYER);
    if has_layer && z <= wt::STORED_MAXZ {
        let from_packs = (|| -> Result<Option<Vec<u8>>> {
            if let Some(c) = stored(s, z, x, y)? {
                return Ok(Some(shares(&c)));
            }
            Ok(wt::uniform_from_ancestor(z, x, y, &|a, ax, ay| stored(s, a, ax, ay))?.map(|(se, i)| [se, i].repeat(wt::SIZE * wt::SIZE)))
        })();
        match from_packs {
            Ok(Some(b)) => return Ok(b),
            // (A stored zoom the basemap can stand in for at a bearable cost: z9's 1,024 tiles.)
            Ok(None) | Err(_) if z >= wt::STORED_MAXZ => {}
            Ok(None) => anyhow::bail!("water {z}/{x}/{y}: no stored tile nor ancestor"),
            Err(e) => return Err(e),
        }
    }
    if z < wt::STORED_MAXZ {
        anyhow::bail!("water {z}/{x}/{y}: not stored, and too coarse to draw here");
    }
    let get = |cx: u32, cy: u32| z14(s, archives, cx, cy);
    Ok(shares(&wt::cover(z, x, y, &get, blocks())?))
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

fn hex(c: &str) -> Option<[f32; 3]> {
    let c = c.trim_start_matches('#');
    (c.len() == 6).then(|| [0, 2, 4].map(|i| u8::from_str_radix(&c[i..i + 2], 16).map(f32::from).unwrap_or(0.0)))
}

/// The tile as the map's raster (water in `sea`/`lake`, alpha its share; the sea's and the inland
/// water's shares summed, overlaps at most whole) or raw (red the sea, green the inland water).
fn encode(sh: &[u8], colours: Option<([f32; 3], [f32; 3])>) -> Result<Vec<u8>> {
    let n = wt::SIZE * wt::SIZE;
    match colours {
        None => {
            let mut px = Vec::with_capacity(n * 3);
            for p in sh.chunks_exact(2) {
                px.extend_from_slice(&[p[0], p[1], 0]);
            }
            png(&px, wt::SIZE, png::ColorType::Rgb)
        }
        Some((sea, lake)) => {
            let mut px = Vec::with_capacity(n * 4);
            for p in sh.chunks_exact(2) {
                let (s, i) = (f32::from(p[0]), f32::from(p[1]));
                let t = s + i;
                let rgb = if t > 0.0 { [0, 1, 2].map(|k| ((sea[k] * s + lake[k] * i) / t).round() as u8) } else { [0, 0, 0] };
                px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], t.min(255.0) as u8]);
            }
            png(&px, wt::SIZE, png::ColorType::Rgba)
        }
    }
}

pub async fn water_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    if z > wt::MAXZ || x >= 1 << z || y >= 1 << z {
        return StatusCode::NOT_FOUND.into_response();
    }
    let v = versioned(q.as_deref());
    let query: std::collections::HashMap<String, String> = q.as_deref().map(|q| url_pairs(q)).unwrap_or_default();
    let colours = match query.get("c") {
        Some(c) => match c.split_once(',').and_then(|(a, b)| Some((hex(a)?, hex(b)?))) {
            Some(cs) => Some(cs),
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        None if query.contains_key("raw") => None,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let archives = s.data.basemap_names();
    for a in &archives {
        s.data.used(a);
    }
    let from = made_from(&s, &archives);
    let etag = format!("\"w{from:016x}-{z}-{x}-{y}-{}\"", query.get("c").map_or("raw", String::as_str));
    if etag_match(&headers, &etag) {
        return not_modified(&etag, v);
    }
    let key = (z, x, y, from);
    let hit = {
        let mut k = kept().lock().unwrap();
        k.tick += 1;
        let t = k.tick;
        k.map.get_mut(&key).map(|e| {
            e.0 = t;
            e.1.clone()
        })
    };
    let sh = match hit {
        Some(sh) => sh,
        None => {
            let _permit = drawing().acquire().await;
            let s2 = s.clone();
            let got = tokio::task::spawn_blocking(move || draw(&s2, &archives, z, x, y)).await;
            match got {
                Ok(Ok(b)) => {
                    let sh = Arc::new(b);
                    let mut k = kept().lock().unwrap();
                    k.tick += 1;
                    let t = k.tick;
                    k.map.insert(key, (t, sh.clone()));
                    if k.map.len() > KEPT {
                        if let Some(old) = k.map.iter().min_by_key(|e| e.1 .0).map(|e| *e.0) {
                            k.map.remove(&old);
                        }
                    }
                    sh
                }
                Ok(Err(e)) => {
                    eprintln!("water {z}/{x}/{y}: {e:#}");
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    };
    match tokio::task::spawn_blocking(move || encode(&sh, colours)).await {
        Ok(Ok(body)) => {
            let mut r = body.into_response();
            let h = r.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
            if let Ok(e) = HeaderValue::from_str(&etag) {
                h.insert(header::ETAG, e);
            }
            h.insert(header::CACHE_CONTROL, crate::cache::cache_control(v, "no-cache"));
            r
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A query's pairs (no percent-decoding needed: hex colours and flags).
fn url_pairs(q: &str) -> std::collections::HashMap<String, String> {
    q.split('&').filter(|p| !p.is_empty()).map(|p| match p.split_once('=') {
        Some((k, v)) => (k.to_string(), v.to_string()),
        None => (p.to_string(), String::new()),
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stored_tiles_and_their_uniform_children_coloured_or_raw() {
        // A catalog whose water layer is one root pack holding z0: its left half sea, its bottom
        // right quarter a lake.
        let mut c = Cov::uniform(0.0, 0.0);
        for y in 0..wt::SIZE {
            for x in 0..wt::SIZE {
                if x < wt::SIZE / 2 {
                    c.sea[y * wt::SIZE + x] = 1.0;
                } else if y >= wt::SIZE / 2 {
                    c.inland[y * wt::SIZE + x] = 1.0;
                }
            }
        }
        let png0 = c.png();
        let nas = tempfile::tempdir().unwrap();
        let content = "layers/water/root/0-0-0.0123456789abcdef.pack";
        let path = nas.path().join(content);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = store::pack::PackWriter::create(&path, serde_json::json!({"layer": "water", "scope": "root", "root": "0/0/0", "encoding": "water-png"}), false).unwrap();
        w.add(0, 0, 0, &png0, png0.len() as u32).unwrap();
        w.finish().unwrap();
        let mut cat = store::catalog::Catalog::new(1);
        cat.files.insert("layers/water/root/0-0-0".into(), store::catalog::FileRef { file: content.into(), size: std::fs::metadata(&path).unwrap().len(), ..Default::default() });
        cat.layers.insert("water".into(), store::catalog::Layer { encoding: "water-png".into(), minzoom: 0, maxzoom: 9, root: Some("layers/water/root/0-0-0".into()), ..Default::default() });
        store::catalog::write_copy(&nas.path().join("catalog"), &cat).unwrap();
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), nas.path());
        let get = |z, x, y, q: &str| water_tile(State(s.clone()), Path((z, x, y)), RawQuery(Some(q.to_string())), HeaderMap::new());
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
        for ((x, y), want) in [((0, 0), [1, 2, 3, 255]), ((1, 0), [0, 0, 0, 0]), ((1, 1), [4, 5, 6, 255])] {
            let r = get(1, x, y, "c=010203,040506&v=1").await;
            assert_eq!(r.status(), StatusCode::OK, "1/{x}/{y}");
            let (t, px) = pixels(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap());
            assert_eq!(t, png::ColorType::Rgba);
            assert!(px.chunks_exact(4).all(|p| p == want), "1/{x}/{y}");
        }
        // A colour is needed, or raw.
        assert_eq!(get(0, 0, 0, "").await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(crate::meta_json(&s)["water"], serde_json::json!(true));
    }
}
