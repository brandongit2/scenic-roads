//! The web app's copies of the build's constants (docs/consistency-report.md F33): the road
//! classes, the tile style and line flags (web/src/config.ts), the shading's steepness scale
//! (basemap.ts), the Earth and the Mercator (merc.ts) and the packs' zoom split (packs.ts) hold
//! roadcore's, store's and terrain's values. A class or flag added on one side fails here, not
//! as wrong colours on the map.
//!
//! scans: web/src/config.ts web/src/basemap.ts web/src/merc.ts web/src/packs.ts

mod common;

use roadcore::{class, geo, shade, tile};

fn read(path: &str) -> String {
    std::fs::read_to_string(common::root().join(path)).unwrap()
}

/// The text of `const <name> = …;` in `src`, the statement's first line to its `;`.
fn value_of<'a>(src: &'a str, name: &str) -> &'a str {
    let at = src
        .find(&format!("export const {name} = "))
        .or_else(|| src.find(&format!("const {name} = ")))
        .unwrap_or_else(|| panic!("no const {name}"));
    let rest = &src[at..];
    let rest = &rest[rest.find(" = ").unwrap() + 3..];
    rest[..rest.find(';').unwrap()].trim()
}

/// `name`'s number: a literal, `1 << n`, `1 / n` or `NAME + n` of another of the file's.
fn number(src: &str, name: &str) -> f64 {
    let v = value_of(src, name);
    let lit = |t: &str| -> f64 {
        let t = t.trim();
        t.parse::<f64>().unwrap_or_else(|_| number(src, t))
    };
    if let Some((a, b)) = v.split_once("<<") {
        return ((lit(a) as u64) << (lit(b) as u64)) as f64;
    }
    if let Some((a, b)) = v.split_once(" / ") {
        return lit(a) / lit(b);
    }
    if let Some((a, b)) = v.split_once(" + ") {
        return lit(a) + lit(b);
    }
    lit(v)
}

/// The quoted strings of array `name` (`[ 'a', 'b', … ]`).
fn strings(src: &str, name: &str) -> Vec<String> {
    let v = value_of(src, name);
    let body = &v[v.find('[').unwrap() + 1..v.rfind(']').unwrap()];
    body.split(',').map(|s| s.trim().trim_matches('\'').to_string()).filter(|s| !s.is_empty()).collect()
}

#[test]
fn config_ts_holds_roadcores_classes_styles_and_flags() {
    let c = read("web/src/config.ts");
    assert_eq!(strings(&c, "CLASS_KEYS"), class::NAMES);
    assert_eq!(number(&c, "NCLASS"), class::COUNT as f64);
    assert_eq!(number(&c, "NROAD"), class::NROAD as f64);
    assert_eq!(number(&c, "CAR_FERRY"), f64::from(class::FERRY));
    assert_eq!(number(&c, "MINOR_MAX_CLASS"), f64::from(class::UNCLASSIFIED));
    assert_eq!(number(&c, "RAIL0"), f64::from(class::TRAM));
    let groups = value_of(&c, "RAIL_GROUPS");
    let keys: Vec<&str> = groups.split("key: '").skip(1).map(|s| &s[..s.find('\'').unwrap()]).collect();
    assert_eq!(keys, class::NAMES[class::TRAM as usize..]);
    for (name, v) in [
        ("ST_UNPAVED", tile::style::UNPAVED),
        ("ST_BRIDGE", tile::style::BRIDGE),
        ("ST_TUNNEL", tile::style::TUNNEL),
        ("ST_LINK", tile::style::LINK),
        ("LF_UNNAMED", tile::lflag::UNNAMED),
        ("LF_ONEWAY", tile::lflag::ONEWAY),
        ("LF_TOLL", tile::lflag::TOLL),
        ("LF_RAIL_SHIFT", tile::lflag::RAIL_SHIFT),
    ] {
        assert_eq!(number(&c, name), f64::from(v), "{name}");
    }
    assert_eq!(number(&c, "DEEP_PACK_Z"), f64::from(terrain::terrain_deep::PACK_Z));
    assert_eq!(number(&c, "DEEP_FIRST_Z"), f64::from(terrain::terrain_deep::FIRST_Z));
}

#[test]
fn basemap_ts_holds_the_shadings_scale() {
    assert_eq!(number(&read("web/src/basemap.ts"), "SLOPE_MAX"), f64::from(shade::SLOPE_MAX));
}

#[test]
fn merc_ts_holds_roadcores_earth() {
    let m = read("web/src/merc.ts");
    assert_eq!(number(&m, "MERCATOR_LAT"), geo::MERCATOR_LAT);
    assert_eq!(number(&m, "EARTH_R"), geo::EARTH_R);
    assert_eq!(number(&m, "WGS84_A"), geo::WGS84_A);
    assert_eq!(number(&m, "WGS84_F"), 1.0 / geo::WGS84_RF);
    // (MapLibre's figure for the world's width, rounded to the mm: roadcore's MERC_C.)
    assert!((number(&m, "MERC_CIRC") - geo::MERC_C).abs() < 1e-3);
}

#[test]
fn packs_ts_holds_the_pack_split() {
    let p = read("web/src/packs.ts");
    assert_eq!(number(&p, "LO_Z"), f64::from(store::catalogue::LO_Z));
    assert_eq!(number(&p, "HI_Z"), f64::from(store::catalogue::HI_Z));
    assert_eq!(number(&p, "HI_FIRST_Z"), f64::from(store::catalogue::HI_FIRST_Z));
    assert_eq!(number(&p, "POLAR_BLOCK_Z"), f64::from(terrain::polar::BLOCK_Z));
}
