//! The regions' `shape:` files (docs/inputs.md §6.1.3, §5.3): GeoJSON (RFC 7946, longitude first,
//! WGS 84) holding a Polygon or a MultiPolygon, as a geometry, a Feature or a FeatureCollection (the
//! union of its features). Read exactly, rounded to E7; invalid geometry (a ring unclosed, of fewer
//! than four points, crossing itself) is an error naming where, never repaired. And `.poly` files
//! (Osmosis's format, Geofabrik's) converted to it (`from_poly`), the conversion proved by reading
//! the GeoJSON back (`scenic regions convert-poly`).

use super::geom::{self, Pt, Ring};
use anyhow::{bail, Context, Result};
use serde_json::Value;

/// A shape file's polygons: each its outer ring and holes, unclosed, as the file has them (E7).
pub type Polys = Vec<Vec<Ring>>;

fn e7(v: &Value) -> Option<i64> {
    let f = v.as_f64()?;
    f.is_finite().then(|| (f * 1e7).round() as i64)
}

fn ring_of(v: &Value, at: &str) -> Result<Ring> {
    let pts = v.as_array().with_context(|| format!("{at}: a ring is a list of positions"))?;
    let mut r: Ring = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        let a = p.as_array().filter(|a| a.len() >= 2).with_context(|| format!("{at}, position {i}: [longitude, latitude]"))?;
        let (Some(x), Some(y)) = (e7(&a[0]), e7(&a[1])) else { bail!("{at}, position {i}: not numbers") };
        if !(-1_800_000_000..=1_800_000_000).contains(&x) || !(-900_000_000..=900_000_000).contains(&y) {
            bail!("{at}, position {i}: outside longitude −180…180, latitude −90…90 (longitude first)");
        }
        r.push([x, y]);
    }
    if r.len() < 4 {
        bail!("{at}: a ring has at least four positions (the last the first again)");
    }
    if r.first() != r.last() {
        bail!("{at}: the ring isn't closed (its last position isn't its first)");
    }
    r.pop();
    // (Checked unwrapped: a ring drawn across ±180° is read the short way round.)
    if let Some((i, j)) = geom::self_crossing(&geom::unwrap(&r), true) {
        bail!("{at}: the ring crosses itself (edges {i} and {j})");
    }
    Ok(r)
}

fn polygon_of(v: &Value, at: &str) -> Result<Vec<Ring>> {
    let rings = v.as_array().with_context(|| format!("{at}: a Polygon's coordinates are a list of rings"))?;
    if rings.is_empty() {
        bail!("{at}: a Polygon with no ring");
    }
    rings.iter().enumerate().map(|(k, r)| ring_of(r, &format!("{at}, ring {k}"))).collect()
}

fn geometry_of(g: &Value, at: &str, out: &mut Polys) -> Result<()> {
    let kind = g.get("type").and_then(Value::as_str).with_context(|| format!("{at}: no geometry type"))?;
    let coords = g.get("coordinates");
    match kind {
        "Polygon" => out.push(polygon_of(coords.with_context(|| format!("{at}: no coordinates"))?, at)?),
        "MultiPolygon" => {
            let ps = coords.and_then(Value::as_array).with_context(|| format!("{at}: a MultiPolygon's coordinates are a list of polygons"))?;
            for (k, p) in ps.iter().enumerate() {
                out.push(polygon_of(p, &format!("{at}, polygon {k}"))?);
            }
        }
        other => bail!("{at}: a {other}, not a Polygon or MultiPolygon"),
    }
    Ok(())
}

/// A shape file's polygons, read and checked: an error says what's wrong and where.
pub fn read(bytes: &[u8]) -> Result<Polys> {
    let v: Value = serde_json::from_slice(bytes).context("not JSON")?;
    let mut out = Vec::new();
    match v.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            let fs = v.get("features").and_then(Value::as_array).context("a FeatureCollection without features")?;
            for (i, f) in fs.iter().enumerate() {
                let g = f.get("geometry").filter(|g| !g.is_null()).with_context(|| format!("feature {i}: no geometry"))?;
                geometry_of(g, &format!("feature {i}"), &mut out)?;
            }
        }
        Some("Feature") => geometry_of(v.get("geometry").filter(|g| !g.is_null()).context("a Feature without geometry")?, "the feature", &mut out)?,
        Some(_) => geometry_of(&v, "the geometry", &mut out)?,
        None => bail!("not GeoJSON (no type)"),
    }
    if out.is_empty() {
        bail!("no polygon in it");
    }
    Ok(out)
}

/// Its rings, every polygon's, in order: what a plain `shape:` part is stored as (even–odd), as a
/// `.poly`'s rings were.
pub fn flat(p: &Polys) -> Vec<Ring> {
    p.iter().flatten().cloned().collect()
}

/// An E7 value as decimal degrees, exactly (seven decimals, trailing zeros dropped).
pub fn degrees(v: i64) -> String {
    let (sign, a) = if v < 0 { ("-", -(v as i128)) } else { ("", v as i128) };
    let (int, frac) = (a / 10_000_000, a % 10_000_000);
    let f = format!("{frac:07}");
    let f = f.trim_end_matches('0');
    if f.is_empty() { format!("{sign}{int}") } else { format!("{sign}{int}.{f}") }
}

/// A `.poly` file's rings (Osmosis's polygon format): each section a ring, `!`-prefixed sections
/// holes, E7. (As `coverage::read_poly` read them for the `poly:` and `geofabrik:` parts.)
pub fn read_poly(text: &str) -> Result<Vec<Vec<[i32; 2]>>> {
    crate::coverage::read_poly(text)
}

/// A `.poly`'s rings as a GeoJSON Feature (`properties`: where it came from): each ring a polygon of
/// its own (even–odd, as the `.poly` is read), closed, in degrees written exactly from E7, so
/// reading it back gives the same rings.
pub fn from_poly(rings: &[Vec<[i32; 2]>], properties: &serde_json::Map<String, Value>) -> String {
    let mut polys = Vec::new();
    for r in rings {
        let mut pts: Vec<String> = r.iter().map(|p| format!("[{},{}]", degrees(p[0] as i64), degrees(p[1] as i64))).collect();
        if r.first() != r.last() {
            pts.push(pts[0].clone());
        }
        polys.push(format!("[[{}]]", pts.join(",")));
    }
    format!("{{\"type\":\"Feature\",\"properties\":{},\"geometry\":{{\"type\":\"MultiPolygon\",\"coordinates\":[{}]}}}}\n", Value::Object(properties.clone()), polys.join(","))
}

/// A `.poly`'s rings as a plain `shape:` part stores them: unclosed (a ring's closing point, when
/// the `.poly` repeats it, dropped), E7.
pub fn poly_rings_unclosed(rings: &[Vec<[i32; 2]>]) -> Vec<Ring> {
    rings.iter().map(|r| {
        let mut v: Ring = r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect();
        if v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        v
    }).collect()
}

/// The E7 rings as `i32` (every shape file's point is within ±180°, ±90°).
pub fn to_i32(r: &[Pt]) -> Vec<[i32; 2]> {
    r.iter().map(|p| [p[0] as i32, p[1] as i32]).collect()
}

/// What converting one `.poly` found (`convert_poly`).
#[derive(Debug)]
pub struct Converted {
    pub from: std::path::PathBuf,
    pub to: std::path::PathBuf,
    pub rings: usize,
    pub points: usize,
    /// The z6 tiles it meets, each with the same fingerprint as the `.poly`'s shape.
    pub tiles: usize,
    /// Whether the file was written now (false: one was there already, with the same rings).
    pub written: bool,
}

/// Every `.poly` in `from` (ours at its top, Geofabrik's in `geofabrik/`) converted to GeoJSON in
/// `into` (`<name>.geojson`, Geofabrik's in `into/geofabrik/`), each a Feature naming its `.poly`
/// (`named_as`: how the properties name `from`), written new (never over another file), and
/// proved: read back, the same E7 rings as the `.poly` (`crate::coverage::read_poly`), and a
/// coverage shape of them the same fingerprint on every z6 tile it meets. A file already there with
/// the same rings is left as it is; one with others is an error.
pub fn convert_poly(from: &std::path::Path, into: &std::path::Path, named_as: &str) -> Result<Vec<Converted>> {
    let mut out = Vec::new();
    for (sub, dst) in [("", into.to_path_buf()), ("geofabrik", into.join("geofabrik"))] {
        let dir = if sub.is_empty() { from.to_path_buf() } else { from.join(sub) };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".poly") && !n.starts_with('.')).collect();
        names.sort();
        for n in names {
            let p = dir.join(&n);
            let rings = read_poly(&std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?).with_context(|| format!("{}", p.display()))?;
            let mut props = serde_json::Map::new();
            let rel = if sub.is_empty() { n.clone() } else { format!("{sub}/{n}") };
            props.insert("converted_from".into(), format!("{named_as}/{rel}").into());
            props.insert("converted_by".into(), "scenic regions convert-poly (#135)".into());
            let text = from_poly(&rings, &props);
            let to = dst.join(format!("{}.geojson", n.trim_end_matches(".poly")));
            // The proof: read back, the same rings; the same fingerprint on every z6 tile.
            let back = flat(&read(text.as_bytes()).with_context(|| format!("{} read back", to.display()))?);
            anyhow::ensure!(back == poly_rings_unclosed(&rings), "{}: the GeoJSON read back doesn't have the .poly's rings", to.display());
            let a = crate::coverage::Shape::new(String::new(), rings.clone(), 0.0);
            let b = crate::coverage::Shape::new(String::new(), back.iter().map(|r| to_i32(r)).collect(), 0.0);
            let (ca, cb) = (crate::coverage::Coverage { shapes: vec![a] }, crate::coverage::Coverage { shapes: vec![b] });
            let mut tiles = 0;
            for x in 0..64u32 {
                for y in 0..64u32 {
                    let d = crate::bld::tile_box_deg(6, x, y);
                    let t = [(d[0] * 1e7).round() as i32, (d[1] * 1e7).round() as i32, (d[2] * 1e7).round() as i32, (d[3] * 1e7).round() as i32];
                    let f = ca.fingerprint(t);
                    anyhow::ensure!(f == cb.fingerprint(t), "{}: the fingerprint on 6/{x}/{y} differs from the .poly's", to.display());
                    tiles += !f.is_empty() as usize;
                }
            }
            std::fs::create_dir_all(&dst).with_context(|| format!("create {}", dst.display()))?;
            let written = match std::fs::File::options().write(true).create_new(true).open(&to) {
                Ok(mut f) => {
                    std::io::Write::write_all(&mut f, text.as_bytes()).with_context(|| format!("write {}", to.display()))?;
                    true
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let there = flat(&read(&std::fs::read(&to)?).with_context(|| format!("{} (there already)", to.display()))?);
                    anyhow::ensure!(there == back, "{} is there already, with other rings", to.display());
                    false
                }
                Err(e) => return Err(e).with_context(|| format!("create {}", to.display())),
            };
            out.push(Converted { from: p, to, rings: rings.len(), points: rings.iter().map(Vec::len).sum(), tiles, written });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geojson_is_read_exactly_and_checked() {
        let f = br#"{"type": "Feature", "properties": {"source": "drawn"},
            "geometry": {"type": "Polygon", "coordinates": [[[-8.2, 41.1], [-8.0, 41.1], [-8.0, 41.3], [-8.2, 41.1]]]}}"#;
        let p = read(f).unwrap();
        assert_eq!(p, vec![vec![vec![[-82_000_000, 411_000_000], [-80_000_000, 411_000_000], [-80_000_000, 413_000_000]]]]);
        let bad = |s: &str| read(s.as_bytes()).unwrap_err().to_string();
        assert!(bad(r#"{"type": "Point", "coordinates": [1, 2]}"#).contains("not a Polygon"));
        assert!(bad(r#"{"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 1]]]}"#).contains("isn't closed"));
        assert!(bad(r#"{"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [0, 0]]]}"#).contains("at least four"));
        assert!(bad(r#"{"type": "Polygon", "coordinates": [[[0, 0], [1, 1], [1, 0], [0, 1], [0, 0]]]}"#).contains("crosses itself"));
        assert!(bad("[1, 2").contains("not JSON"));
        // A collection's polygons, each of its features'.
        let c = br#"{"type": "FeatureCollection", "features": [
            {"type": "Feature", "geometry": {"type": "MultiPolygon", "coordinates": [[[[0, 0], [1, 0], [1, 1], [0, 0]]], [[[2, 0], [3, 0], [3, 1], [2, 0]]]]}},
            {"type": "Feature", "geometry": {"type": "Polygon", "coordinates": [[[5, 0], [6, 0], [6, 1], [5, 0]]]}}]}"#;
        assert_eq!(read(c).unwrap().len(), 3);
        // Across the antimeridian, drawn the short way: not crossing itself.
        assert!(read(br#"{"type": "Polygon", "coordinates": [[[179, -17], [-179, -17], [-179, -16], [179, -16], [179, -17]]]}"#).is_ok());
    }

    #[test]
    fn convert_poly_writes_and_proves_each() {
        let d = tempfile::tempdir().unwrap();
        let (from, into) = (d.path().join("outlines"), d.path().join("regions/shapes"));
        std::fs::create_dir_all(from.join("geofabrik")).unwrap();
        std::fs::write(from.join("x.poly"), "x\n1\n -8.1 41.1\n -8.0 41.1\n -8.0 41.3\nEND\nEND\n").unwrap();
        std::fs::write(from.join("geofabrik/europe-y.poly"), "y\n1\n 1 1\n 2 1\n 2 2\n 1 1\nEND\nEND\n").unwrap();
        let c = convert_poly(&from, &into, "inputs/outlines").unwrap();
        assert_eq!(c.len(), 2);
        assert!(c.iter().all(|x| x.written && x.tiles >= 1));
        assert!(into.join("x.geojson").exists() && into.join("geofabrik/europe-y.geojson").exists());
        // Again: there already, the same: left.
        assert!(convert_poly(&from, &into, "inputs/outlines").unwrap().iter().all(|x| !x.written));
    }

    #[test]
    fn a_poly_converted_reads_back_the_same() {
        let poly = "x\n1\n   -8.123456E+00   4.1123456E+01\n   -8.0  41.1\n   -8.0  41.3\n   -8.123456E+00   4.1123456E+01\nEND\n!2\n -8.05 41.15\n -8.04 41.15\n -8.04 41.16\nEND\nEND\n";
        let rings = read_poly(poly).unwrap();
        let mut props = serde_json::Map::new();
        props.insert("converted_from".into(), "inputs/outlines/x.poly".into());
        let g = from_poly(&rings, &props);
        let back = flat(&read(g.as_bytes()).unwrap());
        assert_eq!(back, poly_rings_unclosed(&rings));
        assert_eq!(degrees(-81_234_560), "-8.123456");
        assert_eq!(degrees(5), "0.0000005");
        assert_eq!(degrees(-1_800_000_000), "-180");
    }
}
