//! Heritage sites and designated areas for the units (docs/phase5.md "Heritage and area flags").
//! The `heritage-sites` job runs today's heritage.py on the registers' snapshot and the pass's
//! protected areas over the cover (the z12 tiles within 20 km of the coverage), then slices what
//! the units read per z6 tile, so a unit's key names only the slices near it: the sites' positions
//! (`work/heritage/<d>/pos/6-x-y`), and the flagged area polygons
//! (`work/heritage/<d>/areas/6-x-y`, those whose bounding box meets the tile, each line keyed by
//! its content so an unchanged polygon keeps its place).

use det::Det;
use crate::coverage::Coverage;
use crate::out::Out;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What heritage.py counts as covered: the z12 tiles within 20 km of the coverage.
pub const COVER_Z: u8 = 12;
pub const COVER_KM: f64 = 20.0;

/// The cover's tiles, sorted by (x, y).
pub fn cover_tiles(cov: &Coverage) -> Vec<(u32, u32)> {
    let s = 1u32 << (COVER_Z - 6);
    let parents: Vec<(u32, u32)> = crate::agent::build::coverage_tiles(cov).into_values().flatten().collect();
    let mut out: Vec<(u32, u32)> = parents
        .par_iter()
        .flat_map_iter(|&(x6, y6)| (x6 * s..(x6 + 1) * s).flat_map(move |x| (y6 * s..(y6 + 1) * s).map(move |y| (x, y))).filter(|&(x, y)| crate::terrain_pack::near_coverage(cov, COVER_Z, x, y, COVER_KM)))
        .collect();
    out.sort_unstable();
    out
}

/// The tiles as uint32 (x, y) pairs, little-endian (heritage.py's `--tiles`).
pub fn tiles_bytes(tiles: &[(u32, u32)]) -> Vec<u8> {
    tiles.iter().flat_map(|&(x, y)| x.to_le_bytes().into_iter().chain(y.to_le_bytes())).collect()
}

/// The tiles as one MultiPolygon feature in degrees (osmium extract's `-p`): each row's runs of
/// tiles, a run stacked with the same run in the rows below into one rectangle.
pub fn tiles_geojson(z: u8, tiles: &[(u32, u32)]) -> serde_json::Value {
    let mut rows: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(x, y) in tiles {
        rows.entry(y).or_default().push(x);
    }
    // Rectangles [x0, y0, x1, y1] in tiles, inclusive; `open` by run, the row they last reached.
    let mut done: Vec<[u32; 4]> = Vec::new();
    let mut open: BTreeMap<(u32, u32), [u32; 4]> = BTreeMap::new();
    for (y, mut xs) in rows {
        xs.sort_unstable();
        xs.dedup();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for x in xs {
            match runs.last_mut() {
                Some(r) if r.1 + 1 == x => r.1 = x,
                _ => runs.push((x, x)),
            }
        }
        let mut next: BTreeMap<(u32, u32), [u32; 4]> = BTreeMap::new();
        for r in runs {
            let rect = match open.remove(&r) {
                Some(mut o) if o[3] + 1 == y => {
                    o[3] = y;
                    o
                }
                Some(o) => {
                    done.push(o);
                    [r.0, y, r.1, y]
                }
                None => [r.0, y, r.1, y],
            };
            next.insert(r, rect);
        }
        done.extend(open.into_values());
        open = next;
    }
    done.extend(open.into_values());
    done.sort_unstable();
    let n = (1u64 << z) as f64;
    let lon = |t: u32| t as f64 / n * 360.0 - 180.0;
    let lat = |t: u32| (std::f64::consts::PI * (1.0 - 2.0 * t as f64 / n)).dsinh().datan().to_degrees();
    let polys: Vec<serde_json::Value> = done
        .iter()
        .map(|r| {
            let (w, e, s, nn) = (lon(r[0]), lon(r[2] + 1), lat(r[3] + 1), lat(r[1]));
            serde_json::json!([[[w, s], [e, s], [e, nn], [w, nn], [w, s]]])
        })
        .collect();
    serde_json::json!({"type": "Feature", "properties": {}, "geometry": {"type": "MultiPolygon", "coordinates": polys}})
}

/// The z6 tile holding a point (degrees), as tiles are cut (half-open).
fn z6_of(lon: f64, lat: f64) -> (u32, u32) {
    crate::stage::tiles_in(6, [lon, lat, lon, lat])[0]
}

/// heritage.json's sites' positions (E7) by the z6 tile holding them, each list sorted, no repeats.
pub fn slice_sites(heritage_json: &[u8]) -> Result<BTreeMap<(u32, u32), Vec<[i32; 2]>>> {
    #[derive(serde::Deserialize)]
    struct Fc {
        features: Vec<F>,
    }
    #[derive(serde::Deserialize)]
    struct F {
        geometry: G,
    }
    #[derive(serde::Deserialize)]
    struct G {
        #[serde(rename = "type")]
        kind: String,
        coordinates: serde_json::Value,
    }
    let fc: Fc = serde_json::from_slice(heritage_json).context("heritage.json")?;
    let mut out: BTreeMap<(u32, u32), Vec<[i32; 2]>> = BTreeMap::new();
    for f in fc.features {
        // Points only, as the flags step reads them.
        let (Some(lon), Some(lat)) = (f.geometry.coordinates[0].as_f64(), f.geometry.coordinates[1].as_f64()) else { continue };
        if f.geometry.kind != "Point" {
            continue;
        }
        let p = [(lon * 1e7).round() as i32, (lat * 1e7).round() as i32];
        out.entry(z6_of(lon, lat)).or_default().push(p);
    }
    for v in out.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    Ok(out)
}

/// A GeoJSON geometry's bounding box (w, s, e, n), from every position in it.
fn bbox(coords: &serde_json::Value, b: &mut [f64; 4]) {
    match coords {
        serde_json::Value::Array(a) if a.len() >= 2 && a[0].is_number() => {
            let (x, y) = (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0));
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        serde_json::Value::Array(a) => a.iter().for_each(|c| bbox(c, b)),
        _ => {}
    }
}

/// The boxes of a geometry's parts (w, s, e, n): each polygon's, and one across the antimeridian as
/// its eastern and western halves.
fn part_boxes(g: &serde_json::Value) -> Vec<[f64; 4]> {
    let parts: Vec<&serde_json::Value> = match g["type"].as_str() {
        Some("MultiPolygon") => g["coordinates"].as_array().map(|a| a.iter().collect()).unwrap_or_default(),
        _ => vec![&g["coordinates"]],
    };
    let mut out = Vec::new();
    for part in parts {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        bbox(part, &mut b);
        if b[0] > b[2] {
            continue;
        }
        if b[2] - b[0] <= 180.0 {
            out.push(b);
            continue;
        }
        let mut pts = Vec::new();
        points(part, &mut pts);
        for east in [true, false] {
            let mut h = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for &(x, y) in pts.iter().filter(|p| (p.0 >= 0.0) == east) {
                h = [h[0].min(x), h[1].min(y), h[2].max(x), h[3].max(y)];
            }
            if h[0] <= h[2] {
                out.push(h);
            }
        }
    }
    out
}

/// Every position of a GeoJSON geometry.
fn points(coords: &serde_json::Value, out: &mut Vec<(f64, f64)>) {
    match coords {
        serde_json::Value::Array(a) if a.len() >= 2 && a[0].is_number() => out.push((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0))),
        serde_json::Value::Array(a) => a.iter().for_each(|c| points(c, out)),
        _ => {}
    }
}

/// area-shapes.geojsonseq's polygons by the z6 tiles their bounding boxes meet: each slice's lines
/// sorted by their content's hash (an unchanged polygon keeps its place when others change; the
/// rasterising is a union, so order doesn't matter). One across the antimeridian (the Aleutians'
/// refuges: parts each side) by its parts' boxes: its whole box is the world's width, which put it
/// in every tile of its latitudes (2026-10-05: Canada's and Britain's slices, and so their units'
/// keys, changed for Alaska's areas).
pub fn slice_areas(area_shapes: &str) -> Result<BTreeMap<(u32, u32), Vec<(String, String)>>> {
    let mut out: BTreeMap<(u32, u32), Vec<(String, String)>> = BTreeMap::new();
    for (i, line) in area_shapes.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).with_context(|| format!("area-shapes line {}", i + 1))?;
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        bbox(&v["geometry"]["coordinates"], &mut b);
        if b[0] > b[2] {
            continue;
        }
        let h = store::naming::hash16(line.as_bytes());
        let boxes = if b[2] - b[0] > 180.0 { part_boxes(&v["geometry"]) } else { vec![b] };
        let tiles: std::collections::BTreeSet<(u32, u32)> = boxes.into_iter().flat_map(|b| crate::stage::tiles_in(6, b)).collect();
        for t in tiles {
            out.entry(t).or_default().push((h.clone(), line.to_string()));
        }
    }
    for v in out.values_mut() {
        v.sort();
        v.dedup();
    }
    Ok(out)
}

/// The logical names of a pass's slices.
pub fn pos_logical(date: &str, x: u32, y: u32) -> String {
    format!("work/heritage/{date}/pos/6-{x}-{y}")
}
pub fn areas_logical(date: &str, x: u32, y: u32) -> String {
    format!("work/heritage/{date}/areas/6-{x}-{y}")
}
/// The heritage-sites job's whole outputs (heritage.json → `base/heritage`, …).
pub fn base_logical(date: &str, stem: &str) -> String {
    format!("work/heritage/{date}/base/{stem}")
}

/// Uploads the slices, removing the pass's slices this run didn't write (a tile that lost its
/// sites or areas). How many of each went up.
pub fn put_slices(out: &mut Out, date: &str, sites: &BTreeMap<(u32, u32), Vec<[i32; 2]>>, areas: &BTreeMap<(u32, u32), Vec<(String, String)>>) -> Result<(usize, usize)> {
    let mut wrote: BTreeSet<String> = BTreeSet::new();
    for (&(x, y), pts) in sites {
        let l = pos_logical(date, x, y);
        out.put_bytes(&l, "json", &serde_json::to_vec(pts)?)?;
        wrote.insert(l);
    }
    for (&(x, y), lines) in areas {
        let l = areas_logical(date, x, y);
        let body: String = lines.iter().map(|(_, l)| format!("{l}\n")).collect();
        out.put_bytes(&l, "jsonl", body.as_bytes())?;
        wrote.insert(l);
    }
    for prefix in [format!("work/heritage/{date}/pos/"), format!("work/heritage/{date}/areas/")] {
        let stale: Vec<String> = out.manifest.range(prefix.clone()..).take_while(|(l, _)| l.starts_with(&prefix)).map(|(l, _)| l.clone()).filter(|l| !wrote.contains(l)).collect();
        for l in stale {
            out.remove(&l);
        }
    }
    Ok((sites.len(), areas.len()))
}

/// A unit's heritage inputs for its box `b` (degrees), into its folder: `heritage.json` (the sites
/// inside the box, as the flags step reads them) and `area-shapes.geojsonseq` (the polygons of the
/// slices meeting the box, each once). The sites and polygons written.
pub fn unit_inputs(out: &Out, date: &str, b: [f64; 4], dir: &Path) -> Result<(usize, usize)> {
    let inside = |p: &[i32; 2]| {
        let (x, y) = (p[0] as f64 / 1e7, p[1] as f64 / 1e7);
        x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]
    };
    let mut sites: Vec<[i32; 2]> = Vec::new();
    let mut polys: BTreeMap<String, String> = BTreeMap::new();
    for (x, y) in crate::stage::tiles_in(6, b) {
        if let Some(c) = out.get(&pos_logical(date, x, y)) {
            let pts: Vec<[i32; 2]> = serde_json::from_slice(&std::fs::read(out.path(c))?).with_context(|| format!("{}", pos_logical(date, x, y)))?;
            sites.extend(pts.into_iter().filter(inside));
        }
        if let Some(c) = out.get(&areas_logical(date, x, y)) {
            for line in std::fs::read_to_string(out.path(c))?.lines().filter(|l| !l.is_empty()) {
                polys.entry(store::naming::hash16(line.as_bytes())).or_insert_with(|| line.to_string());
            }
        }
    }
    sites.sort_unstable();
    sites.dedup();
    let feats: Vec<String> = sites.iter().map(|p| format!(r#"{{"type":"Feature","geometry":{{"type":"Point","coordinates":[{},{}]}},"properties":{{}}}}"#, p[0] as f64 / 1e7, p[1] as f64 / 1e7)).collect();
    write(dir, "heritage.json", format!(r#"{{"type":"FeatureCollection","features":[{}]}}"#, feats.join(",")).as_bytes())?;
    let body: String = polys.values().map(|l| format!("{l}\n")).collect();
    write(dir, "area-shapes.geojsonseq", body.as_bytes())?;
    Ok((sites.len(), polys.len()))
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join(name))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_cover_exactly_the_tiles() {
        // An L of tiles and a lone one: rows 10–12, runs stacking where they repeat.
        let tiles = vec![(5, 10), (6, 10), (5, 11), (6, 11), (5, 12), (6, 12), (7, 12), (9, 12), (5, 13)];
        let g = tiles_geojson(12, &tiles);
        let polys = g["geometry"]["coordinates"].as_array().unwrap();
        // (5–6 × 10–11), then rows 12 and 13's own runs: 5–7 at 12, 9 at 12, 5 at 13.
        assert_eq!(polys.len(), 4);
        // Every tile's centre inside exactly one rectangle; none outside the tiles.
        let n = 4096.0;
        let lon = |t: f64| t / n * 360.0 - 180.0;
        let lat = |t: f64| (std::f64::consts::PI * (1.0 - 2.0 * t / n)).dsinh().datan().to_degrees();
        let count = |x: f64, y: f64| {
            polys
                .iter()
                .filter(|p| {
                    let r = p[0].as_array().unwrap();
                    let (w, s, e, nn) = (r[0][0].as_f64().unwrap(), r[0][1].as_f64().unwrap(), r[2][0].as_f64().unwrap(), r[2][1].as_f64().unwrap());
                    x > w && x < e && y > s && y < nn
                })
                .count()
        };
        for tx in 3..12u32 {
            for ty in 8..15u32 {
                let c = count(lon(tx as f64 + 0.5), lat(ty as f64 + 0.5));
                assert_eq!(c, tiles.contains(&(tx, ty)) as usize, "tile {tx}/{ty}");
            }
        }
    }

    #[test]
    fn slices_and_a_units_inputs() {
        let d = tempfile::tempdir().unwrap();
        let mut out = Out::open(d.path(), &d.path().join("scratch")).unwrap();
        let her = br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","geometry":{"type":"Point","coordinates":[-1.6,55.0]},"properties":{"name":"a"}},
            {"type":"Feature","geometry":{"type":"Point","coordinates":[-1.6,55.0]},"properties":{"name":"a again"}},
            {"type":"Feature","geometry":{"type":"Point","coordinates":[2.35,48.85]},"properties":{"name":"b"}}]}"#;
        let sites = slice_sites(her).unwrap();
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[&(31, 20)], vec![[-16_000_000, 550_000_000]]);
        // One polygon across the z6 border at 0°, one inside a tile.
        let shapes = concat!(
            r#"{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[-0.5,51.0],[0.5,51.0],[0.5,51.5],[-0.5,51.0]]]},"properties":{"bit":1}}"#,
            "\n",
            r#"{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[-1.7,54.9],[-1.5,54.9],[-1.5,55.1],[-1.7,54.9]]]},"properties":{"bit":4}}"#,
            "\n"
        );
        let areas = slice_areas(shapes).unwrap();
        assert_eq!(areas.keys().copied().collect::<Vec<_>>(), vec![(31, 20), (31, 21), (32, 21)]);
        assert_eq!(put_slices(&mut out, "d", &sites, &areas).unwrap(), (2, 3));
        // A stale slice from an earlier run goes.
        out.put_bytes("work/heritage/d/pos/6-1-1", "json", b"[]").unwrap();
        put_slices(&mut out, "d", &sites, &areas).unwrap();
        assert!(out.get("work/heritage/d/pos/6-1-1").is_none());
        // A unit near Newcastle: its box meets 6/31/20 only; the site and the small polygon.
        let u = d.path().join("unit");
        let (s, p) = unit_inputs(&out, "d", [-2.0, 54.5, -1.0, 55.5], &u).unwrap();
        assert_eq!((s, p), (1, 1));
        let h: serde_json::Value = serde_json::from_slice(&std::fs::read(u.join("heritage.json")).unwrap()).unwrap();
        assert_eq!(h["features"][0]["geometry"]["coordinates"][0], -1.6);
        // A box across 0°: the border polygon once, from both tiles' slices.
        let (_, p) = unit_inputs(&out, "d", [-1.0, 50.5, 1.0, 51.8], &u).unwrap();
        assert_eq!(p, 1);
    }

    #[test]
    fn a_shape_across_the_antimeridian_is_in_its_parts_tiles_alone() {
        // An Aleutian refuge: a part each side of 180°, at 52° N; and a park of two parts far apart.
        let shapes = concat!(
            r#"{"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[179.2,51.8],[179.6,51.8],[179.6,52.1],[179.2,51.8]]],[[[-179.6,51.8],[-179.2,51.8],[-179.2,52.1],[-179.6,51.8]]]]},"properties":{"bit":2}}"#,
            "
",
            r#"{"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[-100.5,52.0],[-100.2,52.0],[-100.2,52.2],[-100.5,52.0]]],[[[-90.5,52.0],[-90.2,52.0],[-90.2,52.2],[-90.5,52.0]]]]},"properties":{"bit":1}}"#,
            "
"
        );
        let areas = slice_areas(shapes).unwrap();
        let has = |t: (u32, u32), bit: &str| areas.get(&t).is_some_and(|v| v.iter().any(|(_, l)| l.contains(&format!("\"bit\":{bit}"))));
        // At 52° N, z6 row 21: the refuge in the tiles at each edge, not the ones between.
        assert!(has((63, 21), "2") && has((0, 21), "2"));
        assert!(!(1..63).any(|x| has((x, 21), "2")));
        // The park as before: every tile its box meets, the ones between its parts too.
        assert!((14..=15).all(|x| has((x, 21), "1")));
    }
}
