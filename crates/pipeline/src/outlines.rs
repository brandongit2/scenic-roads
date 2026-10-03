//! Administrative and ISO 3166 outlines (docs/plan.md §5 and §6, OSM pass step 8): what regions are
//! made of (`osm:<relation>`), the areas modules apply by (ISO 3166-1 and -2), and the Regions
//! panel's "areas containing this place".
//!
//! Assembled from the pass's outline set with `osmium export`, which builds each relation's
//! (multi)polygon. Kept: administrative areas of levels 2–8, and anything with an ISO 3166 code.
//! Stored as a sectioned file (`global/outlines`, RDSECT01):
//!
//! | section | records |
//! |---|---|
//! | `recs` | [`OutlineRec`] (64 B), sorted by relation id |
//! | `rings` | [`Ring`] (16 B): each outline's rings, outer and inner (even–odd) |
//! | `points` | `[i32; 2]` lon, lat (E7) |
//! | `srings`, `spoints` | the same, simplified for drawing (Douglas–Peucker; 1 km for countries, 250 m for levels 3–4, 60 m finer) |
//! | `strings` | names, `\n`-separated; index 0 is "" |

use anyhow::{ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

pub const FORMAT: &str = "outlines-1";

/// One outline.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct OutlineRec {
    /// The OSM relation id.
    pub id: u64,
    /// West, south, east, north (E7).
    pub bbox: [i32; 4],
    pub area_km2: f32,
    /// Strings: `name`, `name:en`, and the ISO 3166 code (`ISO3166-2`, else `ISO3166-1`).
    pub name: u32,
    pub name_en: u32,
    pub iso: u32,
    /// Its rings in `rings` (and simplified, in `srings`).
    pub rings: u32,
    pub nrings: u32,
    pub srings: u32,
    pub nsrings: u32,
    /// `admin_level` (0: none).
    pub level: u8,
    /// [`flag`]s.
    pub flags: u8,
    pub _pad: [u8; 2],
    /// Strings: the ISO 3166-1 code of the country it lies in ("" for a country, or none found).
    pub country: u32,
}

pub mod flag {
    /// Has `ISO3166-1` (a country or territory).
    pub const ISO1: u8 = 1;
    /// Has `ISO3166-2` (a subdivision).
    pub const ISO2: u8 = 2;
}

/// A ring: `count` points of `points` from `start`. A polygon is an outer ring followed by its
/// inner ones (holes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct Ring {
    pub start: u64,
    pub count: u32,
    /// 0: outer (starts a polygon), 1: inner.
    pub kind: u32,
}

const _: () = assert!(std::mem::size_of::<OutlineRec>() == 64);
const _: () = assert!(std::mem::size_of::<Ring>() == 16);

/// An outline as read from the export, before storing.
struct Parsed {
    id: u64,
    level: u8,
    flags: u8,
    name: String,
    name_en: String,
    iso: String,
    /// Polygons: rings (the first outer, the rest inner), E7.
    polys: Vec<Vec<Vec<[i32; 2]>>>,
}

fn level_of(p: &serde_json::Map<String, Value>) -> u8 {
    p.get("admin_level").and_then(|v| v.as_str()).and_then(|s| s.trim().parse::<u8>().ok()).unwrap_or(0)
}

/// The outline in one exported feature, when it's one we keep.
fn parse(line: &str) -> Option<Parsed> {
    let line = line.trim_matches(|c: char| c == '\u{1e}' || c.is_whitespace());
    if line.is_empty() {
        return None;
    }
    let f: Value = serde_json::from_str(line).ok()?;
    let p = f.get("properties")?.as_object()?;
    if p.get("@type")?.as_str()? != "relation" {
        return None;
    }
    let level = level_of(p);
    let iso1 = p.get("ISO3166-1").or_else(|| p.get("ISO3166-1:alpha2")).and_then(|v| v.as_str()).unwrap_or("");
    let iso2 = p.get("ISO3166-2").and_then(|v| v.as_str()).unwrap_or("");
    let admin = p.get("boundary").and_then(|v| v.as_str()) == Some("administrative") && (2..=8).contains(&level);
    if !admin && iso1.is_empty() && iso2.is_empty() {
        return None;
    }
    let g = f.get("geometry")?;
    let e7 = |v: &Value| -> Option<[i32; 2]> { Some([(v.get(0)?.as_f64()? * 1e7).round() as i32, (v.get(1)?.as_f64()? * 1e7).round() as i32]) };
    let ring = |r: &Value| -> Option<Vec<[i32; 2]>> { r.as_array()?.iter().map(e7).collect() };
    let poly = |p: &Value| -> Option<Vec<Vec<[i32; 2]>>> { p.as_array()?.iter().map(ring).collect() };
    let polys: Vec<Vec<Vec<[i32; 2]>>> = match g.get("type")?.as_str()? {
        "Polygon" => vec![poly(g.get("coordinates")?)?],
        "MultiPolygon" => g.get("coordinates")?.as_array()?.iter().map(poly).collect::<Option<_>>()?,
        _ => return None,
    };
    let mut flags = 0;
    if !iso1.is_empty() {
        flags |= flag::ISO1;
    }
    if !iso2.is_empty() {
        flags |= flag::ISO2;
    }
    Some(Parsed {
        id: p.get("@id")?.as_u64()?,
        level,
        flags,
        name: p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        name_en: p.get("name:en").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        iso: if iso2.is_empty() { iso1 } else { iso2 }.to_string(),
        polys,
    })
}

/// Signed area (m²) of a ring, on a local equirectangular projection.
fn ring_area_m2(r: &[[i32; 2]]) -> f64 {
    if r.len() < 3 {
        return 0.0;
    }
    let lat0 = r.iter().map(|p| p[1] as f64).sum::<f64>() / r.len() as f64 * 1e-7;
    let kx = 111_320.0 * lat0.to_radians().cos() * 1e-7;
    let ky = 110_574.0 * 1e-7;
    let mut s = 0.0;
    for i in 0..r.len() {
        let (a, b) = (r[i], r[(i + 1) % r.len()]);
        s += (a[0] as f64 * kx) * (b[1] as f64 * ky) - (b[0] as f64 * kx) * (a[1] as f64 * ky);
    }
    s / 2.0
}

/// Douglas–Peucker with a tolerance in metres (on a local projection); keeps a closed ring's
/// first and last points and at least four.
pub fn simplify(r: &[[i32; 2]], tol_m: f64) -> Vec<[i32; 2]> {
    if r.len() <= 4 {
        return r.to_vec();
    }
    let lat0 = r[0][1] as f64 * 1e-7;
    let kx = 111_320.0 * lat0.to_radians().cos() * 1e-7;
    let ky = 110_574.0 * 1e-7;
    let xy: Vec<(f64, f64)> = r.iter().map(|p| (p[0] as f64 * kx, p[1] as f64 * ky)).collect();
    let mut keep = vec![false; r.len()];
    keep[0] = true;
    keep[r.len() - 1] = true;
    // A closed ring's chord from its first point to itself is a point: split at the farthest point.
    let far = (1..r.len() - 1).max_by(|&a, &b| {
        let d = |i: usize| (xy[i].0 - xy[0].0).powi(2) + (xy[i].1 - xy[0].1).powi(2);
        d(a).total_cmp(&d(b))
    });
    let mut stack: Vec<(usize, usize)> = match far {
        Some(f) => {
            keep[f] = true;
            vec![(0, f), (f, r.len() - 1)]
        }
        None => vec![(0, r.len() - 1)],
    };
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay) = xy[a];
        let (bx, by) = xy[b];
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let mut best = (0.0f64, 0usize);
        for (i, &(px, py)) in xy.iter().enumerate().take(b).skip(a + 1) {
            let d2 = if len2 == 0.0 {
                (px - ax).powi(2) + (py - ay).powi(2)
            } else {
                let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
                (px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)
            };
            if d2 > best.0 {
                best = (d2, i);
            }
        }
        if best.0 > tol_m * tol_m {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    let out: Vec<[i32; 2]> = r.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
    if out.len() < 4 {
        r.to_vec()
    } else {
        out
    }
}

fn tolerance_m(level: u8) -> f64 {
    match level {
        0..=2 => 1000.0,
        3..=4 => 250.0,
        _ => 60.0,
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Summary {
    pub outlines: usize,
    pub points: usize,
    pub simplified_points: usize,
    pub by_level: std::collections::BTreeMap<u8, usize>,
}

/// Assembles the outlines of `set` (the pass's outline set, OSM PBF) into a sectioned file at `out`,
/// using `work` for osmium's export.
pub fn assemble(set: &Path, work: &Path, out: &Path) -> Result<Summary> {
    std::fs::create_dir_all(work)?;
    let geo = work.join("outlines.geojsonseq");
    // Node locations in a sparse index on disk (16 bytes a node): osmium's default can switch to a
    // dense array as big as the highest node id (about 100 GB for the planet's).
    let idx = work.join("outlines-nodes.idx");
    std::fs::remove_file(&idx).ok();
    let st = Command::new("osmium")
        .args(["export", "-f", "geojsonseq", "--geometry-types=polygon", "-a", "type,id", "--overwrite"])
        .arg(format!("--index-type=sparse_file_array,{}", idx.display()))
        .arg("-o")
        .arg(&geo)
        .arg(set)
        .status()
        .context("run osmium export")?;
    std::fs::remove_file(&idx).ok();
    ensure!(st.success(), "osmium export of {} failed: {st}", set.display());
    let r = assemble_geojsonseq(&geo, out);
    std::fs::remove_file(&geo).ok();
    r
}

/// The same from an existing export.
pub fn assemble_geojsonseq(geo: &Path, out: &Path) -> Result<Summary> {
    let f = std::fs::File::open(geo).with_context(|| format!("open {}", geo.display()))?;
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut batch: Vec<String> = Vec::with_capacity(4096);
    let mut lines = BufReader::with_capacity(1 << 20, f).lines();
    loop {
        batch.clear();
        for l in lines.by_ref().take(4096) {
            batch.push(l?);
        }
        if batch.is_empty() {
            break;
        }
        parsed.extend(batch.par_iter().filter_map(|l| parse(l)).collect::<Vec<_>>());
    }
    // One record per relation (osmium writes each once; a repeat would be a bug upstream).
    parsed.sort_by_key(|p| p.id);
    parsed.dedup_by_key(|p| p.id);

    let mut strings: Vec<String> = vec![String::new()];
    let mut index: HashMap<String, u32> = HashMap::from([(String::new(), 0)]);
    let mut intern = |s: &str| -> u32 {
        let s = s.replace('\n', " ");
        if let Some(&i) = index.get(&s) {
            return i;
        }
        strings.push(s.clone());
        let i = (strings.len() - 1) as u32;
        index.insert(s, i);
        i
    };
    // Simplify in parallel (rings in the same order as the polygons'), then lay out.
    let simplified: Vec<Vec<Vec<[i32; 2]>>> = parsed.par_iter().map(|p| p.polys.iter().flatten().map(|r| simplify(r, tolerance_m(p.level))).collect()).collect();
    // Which country each one lies in: the country outline holding a point of it.
    let countries: Vec<(usize, crate::coverage::Shape)> = parsed
        .par_iter()
        .enumerate()
        .filter(|(_, p)| p.flags & flag::ISO1 != 0)
        .map(|(i, p)| (i, crate::coverage::Shape::new(String::new(), p.polys.iter().flatten().cloned().collect(), 0.0)))
        .collect();
    let country_of: Vec<Option<usize>> = parsed
        .par_iter()
        .map(|p| {
            if p.flags & flag::ISO1 != 0 {
                return None;
            }
            // A point inside the area: its outer ring's centre, else its first point.
            let r = p.polys.first()?.first()?;
            let n = r.len().max(1) as i64;
            let c = [(r.iter().map(|q| q[0] as i64).sum::<i64>() / n) as i32, (r.iter().map(|q| q[1] as i64).sum::<i64>() / n) as i32];
            let inside = |pt: [i32; 2]| countries.iter().find(|(_, sh)| sh.contains(pt)).map(|(i, _)| *i);
            inside(c).or_else(|| inside(r[0]))
        })
        .collect();
    let mut recs = Vec::with_capacity(parsed.len());
    let (mut rings, mut points, mut srings, mut spoints): (Vec<Ring>, Vec<[i32; 2]>, Vec<Ring>, Vec<[i32; 2]>) = Default::default();
    let mut sum = Summary::default();
    for (pi, (p, simple)) in parsed.iter().zip(simplified).enumerate() {
        let mut bbox = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        let mut area = 0.0;
        let r0 = rings.len();
        let mut kinds: Vec<u32> = Vec::new();
        for poly in &p.polys {
            for (k, r) in poly.iter().enumerate() {
                let a = ring_area_m2(r).abs();
                area += if k == 0 { a } else { -a };
                for q in r {
                    bbox = [bbox[0].min(q[0]), bbox[1].min(q[1]), bbox[2].max(q[0]), bbox[3].max(q[1])];
                }
                let kind = (k > 0) as u32;
                kinds.push(kind);
                rings.push(Ring { start: points.len() as u64, count: r.len() as u32, kind });
                points.extend_from_slice(r);
            }
        }
        let s0 = srings.len();
        for (r, &kind) in simple.iter().zip(&kinds) {
            srings.push(Ring { start: spoints.len() as u64, count: r.len() as u32, kind });
            spoints.extend_from_slice(r);
        }
        let country = country_of[pi].map(|c| parsed[c].iso.clone()).unwrap_or_default();
        if rings.len() == r0 {
            continue;
        }
        *sum.by_level.entry(p.level).or_default() += 1;
        recs.push(OutlineRec {
            id: p.id,
            bbox,
            area_km2: (area.max(0.0) / 1e6) as f32,
            name: intern(&p.name),
            name_en: intern(&p.name_en),
            iso: intern(&p.iso),
            rings: r0 as u32,
            nrings: (rings.len() - r0) as u32,
            srings: s0 as u32,
            nsrings: (srings.len() - s0) as u32,
            level: p.level,
            flags: p.flags,
            _pad: [0; 2],
            country: intern(&country),
        });
    }
    sum.outlines = recs.len();
    sum.points = points.len();
    sum.simplified_points = spoints.len();
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let meta = serde_json::json!({ "fmt": FORMAT, "outlines": recs.len(), "points": points.len(), "spoints": spoints.len() });
    let mut w = store::sect::SectWriter::create(out, meta)?;
    w.add_pod("recs", &recs)?;
    w.add_pod("rings", &rings)?;
    w.add_pod("points", &points)?;
    w.add_pod("srings", &srings)?;
    w.add_pod("spoints", &spoints)?;
    w.add("strings", strings.join("\n").as_bytes())?;
    w.finish()?;
    Ok(sum)
}

/// Whether `p` lies inside the rings (even–odd: holes and multipolygons alike).
pub fn inside(rings: &[&[[i32; 2]]], p: [i32; 2]) -> bool {
    let (x, y) = (p[0] as f64, p[1] as f64);
    let mut c = false;
    for r in rings {
        let n = r.len();
        if n < 3 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = (r[i][0] as f64, r[i][1] as f64);
            let (xj, yj) = (r[j][0] as f64, r[j][1] as f64);
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
    }
    c
}

/// The outlines: records and names in memory, rings read from the file as asked, with plain
/// reads (the NAS's file is never mapped, nor read whole: its points are gigabytes for the
/// planet), as the server's index reads them.
pub struct Outlines {
    pub recs: Vec<OutlineRec>,
    strings: Vec<String>,
    file: store::sect::SectReader<store::range::PlainFile>,
}

impl Outlines {
    pub fn open(path: &Path) -> Result<Outlines> {
        let r = store::sect::SectReader::open(store::range::PlainFile::open(path)?)?;
        ensure!(r.meta().get("fmt").and_then(|v| v.as_str()) == Some(FORMAT), "{}: not {FORMAT}", path.display());
        Ok(Outlines { recs: r.read_pod("recs")?, strings: String::from_utf8(r.read("strings")?)?.split('\n').map(str::to_string).collect(), file: r })
    }

    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }

    pub fn by_id(&self, id: u64) -> Option<&OutlineRec> {
        self.recs.binary_search_by_key(&id, |r| r.id).ok().map(|k| &self.recs[k])
    }

    /// Rings `first..first + n` of `rings` with their points from `points`, and their kinds.
    fn read_rings(&self, rings: &str, points: &str, first: u32, n: u32) -> Result<Vec<(u32, Vec<[i32; 2]>)>> {
        let rs: Vec<Ring> = bytemuck::pod_collect_to_vec(&self.file.read_part(rings, first as u64 * std::mem::size_of::<Ring>() as u64, n as usize * std::mem::size_of::<Ring>())?);
        let (Some(a), Some(b)) = (rs.first(), rs.last()) else { return Ok(Vec::new()) };
        let (start, end) = (a.start as u64, b.start as u64 + b.count as u64);
        let pts: Vec<[i32; 2]> = bytemuck::pod_collect_to_vec(&self.file.read_part(points, start * 8, ((end - start) * 8) as usize)?);
        Ok(rs.iter().map(|r| (r.kind, pts[(r.start as u64 - start) as usize..(r.start as u64 - start) as usize + r.count as usize].to_vec())).collect())
    }

    pub fn rings(&self, o: &OutlineRec) -> Result<Vec<Vec<[i32; 2]>>> {
        Ok(self.read_rings("rings", "points", o.rings, o.nrings)?.into_iter().map(|r| r.1).collect())
    }

    /// The simplified rings grouped as polygons (an outer ring, then its holes).
    pub fn simple_polygons(&self, o: &OutlineRec) -> Result<Vec<Vec<Vec<[i32; 2]>>>> {
        let mut out: Vec<Vec<Vec<[i32; 2]>>> = Vec::new();
        for (kind, pts) in self.read_rings("srings", "spoints", o.srings, o.nsrings)? {
            match out.last_mut() {
                Some(p) if kind == 1 => p.push(pts),
                _ => out.push(vec![pts]),
            }
        }
        Ok(out)
    }

    pub fn simple_rings(&self, o: &OutlineRec) -> Result<Vec<Vec<[i32; 2]>>> {
        Ok(self.read_rings("srings", "spoints", o.srings, o.nsrings)?.into_iter().map(|r| r.1).collect())
    }

    /// Every outline containing `p`, smallest area first.
    pub fn containing(&self, p: [i32; 2]) -> Result<Vec<&OutlineRec>> {
        let mut v = Vec::new();
        for o in self.recs.iter().filter(|o| p[0] >= o.bbox[0] && p[0] <= o.bbox[2] && p[1] >= o.bbox[1] && p[1] <= o.bbox[3]) {
            let rings = self.rings(o)?;
            if inside(&rings.iter().map(Vec::as_slice).collect::<Vec<_>>(), p) {
                v.push(o);
            }
        }
        v.sort_by(|a, b| a.area_km2.total_cmp(&b.area_km2));
        Ok(v)
    }

    /// The ISO 3166-1 and 3166-2 codes at `p` ("" where none).
    pub fn iso_at(&self, p: [i32; 2]) -> Result<(String, String)> {
        let c = self.containing(p)?;
        let pick = |f: u8| c.iter().find(|o| o.flags & f != 0).map(|o| self.string(o.iso).to_string()).unwrap_or_default();
        Ok((pick(flag::ISO1), pick(flag::ISO2)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE_WITH_HOLE: &str = r#"{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]]},"properties":{"@type":"relation","@id":7,"boundary":"administrative","admin_level":"4","name":"Carré","name:en":"Square","ISO3166-2":"XX-SQ"}}"#;

    #[test]
    fn parse_keeps_admin_and_iso_only() {
        let p = parse(SQUARE_WITH_HOLE).unwrap();
        assert_eq!((p.id, p.level, p.iso.as_str(), p.flags), (7, 4, "XX-SQ", flag::ISO2));
        assert_eq!(p.polys[0].len(), 2);
        assert!(parse(&SQUARE_WITH_HOLE.replace("\"relation\"", "\"way\"")).is_none());
        assert!(parse(&SQUARE_WITH_HOLE.replace("\"admin_level\":\"4\"", "\"admin_level\":\"10\"").replace(",\"ISO3166-2\":\"XX-SQ\"", "")).is_none());
        // An ISO code keeps it whatever its level.
        assert!(parse(&SQUARE_WITH_HOLE.replace("\"admin_level\":\"4\"", "\"admin_level\":\"10\"")).is_some());
    }

    #[test]
    fn round_trip_and_lookup() {
        let d = tempfile::tempdir().unwrap();
        let geo = d.path().join("o.geojsonseq");
        let big = SQUARE_WITH_HOLE.replace("\"@id\":7", "\"@id\":3").replace("[[0,0],[1,0],[1,1],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]", "[[-1,-1],[2,-1],[2,2],[-1,2],[-1,-1]]").replace("\"admin_level\":\"4\"", "\"admin_level\":\"2\"").replace("\"ISO3166-2\":\"XX-SQ\"", "\"ISO3166-1\":\"XX\"");
        std::fs::write(&geo, format!("\u{1e}{SQUARE_WITH_HOLE}\n\u{1e}{big}\n")).unwrap();
        let out = d.path().join("outlines.sect");
        let s = assemble_geojsonseq(&geo, &out).unwrap();
        assert_eq!(s.outlines, 2);
        let o = Outlines::open(&out).unwrap();
        let e7 = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        assert_eq!(o.containing(e7(0.2, 0.2)).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), vec![7, 3]);
        // In the hole: only the country.
        assert_eq!(o.containing(e7(0.5, 0.5)).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), vec![3]);
        assert_eq!(o.iso_at(e7(0.2, 0.2)).unwrap(), ("XX".to_string(), "XX-SQ".to_string()));
        assert_eq!(o.string(o.by_id(7).unwrap().name_en), "Square");
        assert_eq!(o.simple_polygons(o.by_id(7).unwrap()).unwrap().iter().map(Vec::len).collect::<Vec<_>>(), vec![2], "the hole nests in its polygon");
        assert_eq!(o.string(o.by_id(7).unwrap().country), "XX", "inside the country");
        assert_eq!(o.string(o.by_id(3).unwrap().country), "", "a country has none");
        let a = o.by_id(7).unwrap().area_km2;
        // 1° × 1° at the equator less a 0.2° × 0.2° hole: about 12,300 − 490 km².
        assert!((a - 11_820.0).abs() < 200.0, "{a}");
    }

    #[test]
    fn simplify_keeps_shape() {
        let n = 1000;
        let ring: Vec<[i32; 2]> = (0..=n).map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            [(t.cos() * 1e6) as i32, (t.sin() * 1e6) as i32]
        }).collect();
        let s = simplify(&ring, 100.0);
        assert!(s.len() > 8 && s.len() < 200, "{}", s.len());
        assert_eq!(s.first(), ring.first());
        assert_eq!(s.last(), ring.last());
    }
}
