//! The unit steps' rules that depend on where a road is (docs/plan.md §5, By location), each with
//! the areas it applies to and a version. A unit's key names the versions of the rules whose areas
//! meet its reach, so a changed rule reruns only the units it applies to: change the rule, bump its
//! version here (the code of each rule points back to this table).
//!
//! Areas are the boxes the rules' code tests (w, s, e, n, degrees). A rule that applies everywhere
//! its more particular neighbours don't (FABDEM, the European road numbers) is listed with the
//! world: changing it reruns every unit, as it may change any of them.
//!
//! The data sources' credits are here too, another rule by location: each with the areas whose data
//! comes from it, the boxes its source's code tests. A catalog lists the credits whose areas meet
//! what it serves (`catalog_credits`), and the map shows the catalog's (© Credits). They enter no
//! key: a credit changes what the map says, not its data. The inputs' credits come from their own
//! descriptions instead (crate::inputs::credits: the heritage registers' among them).

use crate::coverage::DrawnRegion;
use crate::hipack::{grow, meets};

/// One rule: its name, version and areas (none: the whole world).
pub struct Rule {
    pub name: &'static str,
    pub version: u32,
    pub areas: &'static [[f64; 4]],
}

const WORLD: &[[f64; 4]] = &[];
const NORTH_AMERICA: &[[f64; 4]] = &[[-180.0, -90.0, -40.0, 90.0]];
const JAPAN: &[[f64; 4]] = &[[122.5, 20.0, 154.0, 46.5]];
const TAIWAN: &[[f64; 4]] = &[[118.0, 21.5, 122.5, 26.6]];
const HONG_KONG: &[[f64; 4]] = &[[113.8, 22.1, 114.5, 22.6]];
const SINGAPORE: &[[f64; 4]] = &[[103.5, 1.1, 104.2, 1.5]];
const ANDORRA: &[[f64; 4]] = &[[1.40, 42.42, 1.79, 42.66]];

pub const RULES: &[Rule] = &[
    // pipeline::dem: the DEM sources and their order.
    Rule { name: "dem-north-america", version: 1, areas: NORTH_AMERICA },
    Rule { name: "dem-japan", version: 1, areas: JAPAN },
    // (Taiwan takes FABDEM: it has no national DEM of its own here.)
    Rule { name: "dem-taiwan", version: 1, areas: TAIWAN },
    Rule { name: "dem-fabdem", version: 1, areas: WORLD },
    // extract: densification (8 m in North America and Japan, 15 m elsewhere).
    Rule { name: "spacing", version: 1, areas: WORLD },
    // extract::network_code: road numbers by area.
    Rule { name: "networks-north-america", version: 1, areas: NORTH_AMERICA },
    Rule { name: "networks-hong-kong", version: 1, areas: HONG_KONG },
    Rule { name: "networks-singapore", version: 1, areas: SINGAPORE },
    Rule { name: "networks-japan", version: 1, areas: JAPAN },
    Rule { name: "networks-taiwan", version: 1, areas: TAIWAN },
    Rule { name: "networks-andorra", version: 1, areas: ANDORRA },
    // France's and Monaco's, Spain's, Portugal's, Ireland's and Britain's numbers, and E-roads: tested
    // by their form and loose boxes, after the areas above.
    Rule { name: "networks-europe", version: 1, areas: WORLD },
];

/// The current version of the rule `name` (0 when there's no such rule).
pub fn version(name: &str) -> u32 {
    RULES.iter().find(|r| r.name == name).map_or(0, |r| r.version)
}

/// The DEM rules, in the order the units' kept samples record their versions (`unit`'s DEM cache).
pub const DEM_RULES: [&str; 4] = ["dem-north-america", "dem-japan", "dem-taiwan", "dem-fabdem"];

/// The DEM rules a sample depends on, by its source (pipeline::dem's sources) and place (E7): the
/// rule that chose its source, and for FABDEM (the fallback) also the rule of the area whose own
/// DEMs it stood in for. Unknown sources depend on every rule. Indexes into `DEM_RULES`.
pub fn dem_rules_of(src: u8, lon: i32, lat: i32) -> Vec<usize> {
    let meets = |a: &[[f64; 4]]| {
        let (x, y) = (lon as f64 * 1e-7, lat as f64 * 1e-7);
        a.iter().any(|b| b[0] <= x && x <= b[2] && b[1] <= y && y <= b[3])
    };
    match src {
        1..=3 => vec![0],
        5..=7 => vec![1],
        4 => {
            let mut v = vec![3];
            if meets(NORTH_AMERICA) {
                v.push(0);
            }
            if meets(JAPAN) {
                v.push(1);
            }
            if meets(TAIWAN) {
                v.push(2);
            }
            v
        }
        _ => vec![0, 1, 2, 3],
    }
}

/// Whether the box w, s, e, n (E7) meets Taiwan (the units' keys of the scheme before P8 had its DEM
/// files' digest there: agent::rekey::v1).
pub fn meets_taiwan(b: [i32; 4]) -> bool {
    let a = TAIWAN[0];
    let (w, s, e, n) = (b[0] as f64 * 1e-7, b[1] as f64 * 1e-7, b[2] as f64 * 1e-7, b[3] as f64 * 1e-7);
    a[0] <= e && a[2] >= w && a[1] <= n && a[3] >= s
}

/// The versions of the rules whose areas meet the box w, s, e, n (E7), as one string for a key.
pub fn versions_meeting(b: [i32; 4]) -> String {
    let (w, s, e, n) = (b[0] as f64 * 1e-7, b[1] as f64 * 1e-7, b[2] as f64 * 1e-7, b[3] as f64 * 1e-7);
    RULES
        .iter()
        .filter(|r| r.areas.is_empty() || r.areas.iter().any(|a| a[0] <= e && a[2] >= w && a[1] <= n && a[3] >= s))
        .map(|r| format!("{}={}", r.name, r.version))
        .collect::<Vec<_>>()
        .join(",")
}

// ---- credits ----------------------------------------------------------------------------------

/// A data source's credit as the map shows it: what came from it, the source as its terms ask to be
/// named, the terms, and the areas whose data comes from it (none: anywhere). Catalogs carry them as
/// written here (docs/formats.md, Catalog `credits`), `areas` left out for anywhere.
#[derive(Debug, serde::Serialize)]
pub struct Credit {
    pub what: &'static str,
    pub source: &'static str,
    pub terms: &'static str,
    #[serde(skip_serializing_if = "anywhere")]
    pub areas: &'static [[f64; 4]],
}

fn anywhere(areas: &&'static [[f64; 4]]) -> bool {
    areas.is_empty()
}

/// dem/leaftype.py: the EEA's dominant leaf type within this box (NALCMS west of 40° W, as
/// `NORTH_AMERICA`).
const EEA_LEAF: &[[f64; 4]] = &[[-40.0, 20.0, 40.0, 75.0]];

const CANADA: &[[f64; 4]] = &[[-141.1, 41.6, -52.5, 83.2]];

/// Every data source's credit, in the order the map lists them, but the inputs' (their descriptions':
/// crate::inputs::credits; the heritage registers' since #138). The app's own (the colour ramps it
/// ships) are the app's: web/src/ui/strip.ts.
pub const CREDITS: &[Credit] = &[
    Credit {
        what: "Roads, water, boundaries, places, parks, points of interest, Indigenous land boundaries",
        source: "OpenStreetMap (Geofabrik extracts); basemap schema by OpenMapTiles",
        terms: "© OpenStreetMap contributors, ODbL",
        areas: WORLD,
    },
    Credit {
        what: "Road elevation, North America",
        source: "NRCan HRDEM lidar → USGS 3DEP 10 m → NRCan MRDEM 30 m",
        terms: "OGL–Canada / public domain",
        areas: NORTH_AMERICA,
    },
    Credit {
        what: "Lakes missing from OpenStreetMap in parts of Canada",
        source: "Natural Resources Canada, CanVec 1:50,000 Hydro features (waterbody_2), 2018",
        terms: "Contains information licensed under the Open Government Licence – Canada (open.canada.ca/en/open-government-licence-canada)",
        areas: CANADA,
    },
    Credit {
        what: "Road elevation, Japan",
        source: "Created by editing GSI Tiles (elevation tiles (Fundamental Geospatial Data Digital Elevation Model)): 地理院タイル（標高タイル（基盤地図情報数値標高モデル））を加工して作成, Geospatial Information Authority of Japan (maps.gsi.go.jp/development/ichiran.html)",
        terms: "GSI terms of use (Public Data License 1.0)",
        areas: JAPAN,
    },
    Credit {
        what: "Road elevation, Europe, Hong Kong, Singapore, Taiwan",
        source: "FABDEM v1-2 30 m (University of Bristol / Fathom; Hawker et al. 2022). FABDEM is produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all rights reserved.",
        terms: "CC BY-NC-SA 4.0 (non-commercial)",
        areas: WORLD,
    },
    Credit {
        what: "3D terrain, hill-shading, contours, slope",
        source: "Terrain Tiles (Terrarium) on AWS Open Data",
        terms: "Mapzen / various open sources",
        areas: WORLD,
    },
    Credit {
        what: "Tree canopy height & cover (scenic factors, tree cover layer)",
        source: "Meta & WRI global canopy height",
        terms: "CC BY 4.0",
        areas: WORLD,
    },
    Credit {
        what: "Forest leaf type, Europe",
        source: "© European Union, Copernicus Land Monitoring Service 2018, European Environment Agency (EEA): High Resolution Layer Dominant Leaf Type",
        terms: "Copernicus free and open data policy (attribution)",
        areas: EEA_LEAF,
    },
    Credit {
        what: "Forest leaf type, North America",
        source: "2020 Land Cover of North America (NALCMS): Commission for Environmental Cooperation; NRCan/CCRS, USGS, INEGI, CONAFOR",
        terms: "CEC terms of use (attribution)",
        areas: NORTH_AMERICA,
    },
    Credit {
        what: "Land cover",
        source: "ESA WorldCover 2021",
        terms: "CC BY 4.0",
        areas: WORLD,
    },
    Credit {
        what: "Local-language names (some UNESCO sites, biosphere reserves, geoparks, Québec federal sites)",
        source: "Wikidata",
        terms: "CC0",
        areas: WORLD,
    },
    Credit {
        what: "Passenger rail lines and services",
        source: "OpenStreetMap route relations and tracks",
        terms: "© OpenStreetMap contributors, ODbL",
        areas: WORLD,
    },
    Credit {
        what: "Stops & sights details",
        source: "OpenStreetMap tags (heights, lights, hill lists, facilities); Wikidata facts (heights, flow, prominence, isolation, inception, descriptions) via QLever (University of Freiburg)",
        terms: "ODbL; Wikidata CC0",
        areas: WORLD,
    },
    Credit {
        what: "Peak prominence & isolation",
        source: "Computed from the terrain tiles (key col by priority flood, nearest higher ground); tagged values (OSM, Wikidata) preferred",
        terms: "Derived",
        areas: WORLD,
    },
    Credit {
        what: "Heritage descriptions",
        source: "Wikidata items matched by register ID; English Wikipedia short descriptions; for the most notable sites, 2–3 sentence summaries of their Wikipedia articles written by Claude",
        terms: "Wikidata CC0; Wikipedia CC BY-SA 4.0",
        areas: WORLD,
    },
    Credit {
        what: "Park & area details",
        source: "Areas computed from the boundaries; OpenStreetMap protected-area tags; Wikidata (inception, visitors, operator)",
        terms: "ODbL; Wikidata CC0",
        areas: WORLD,
    },
    // (The rail feeds are those of the countries the coverage is in: crate::rail.)
    Credit {
        what: "Rail service frequency",
        source: "Operators’ GTFS timetables (the feeds that run rail where the map is, via the Mobility Database catalogue and from operators: SNCF, Renfe, IDFM, TfI, MTA, MBTA, GO, exo, VIA, Amtrak and others; Great Britain: the Rail Delivery Group timetable as GTFS by Catenary Transit; Singapore: Land Transport Authority, LTA DataMall, under the Singapore Open Data Licence 1.0); MTR frequencies from mtr.com.hk (exact for the Airport Express and High Speed Rail, whose timetables are published in full; other lines a lower bound, at least the service hours at the slowest published off-peak headway)",
        terms: "Each operator’s open-data terms",
        areas: WORLD,
    },
    Credit {
        what: "Ferry routes and terminals",
        source: "OpenStreetMap ferry routes and route relations",
        terms: "© OpenStreetMap contributors, ODbL",
        areas: WORLD,
    },
    Credit {
        what: "Ferry sailings",
        source: "Operators’ published GTFS timetables (listed in each line’s source: MBTA, NYC Ferry, NYC DOT, NY Waterway, STQ, Halifax Transit, BreizhGo, Bacs de Seine, Brittany Ferries, Transtejo Soflusa, Hong Kong Transport Department and others), operators’ timetable pages, and OSM interval tags",
        terms: "Each operator’s open-data terms",
        areas: WORLD,
    },
    Credit {
        what: "Roadside buildings",
        source: "Overture Maps Foundation buildings (OpenStreetMap, Microsoft and Google footprints)",
        terms: "ODbL · CDLA Permissive 2.0",
        areas: WORLD,
    },
    Credit {
        what: "3D buildings",
        source: "Overture Maps Foundation buildings, release 2026-09-23.1 (© OpenStreetMap contributors, Microsoft, Esri Community Maps, USGS, IGN España, Google and others; each building's sources in the release)",
        terms: "ODbL 1.0",
        areas: WORLD,
    },
    Credit {
        what: "Building heights where none are known",
        source: "GHSL GHS-BUILT-H R2023A (EC JRC), © European Union, 1995-2026",
        terms: "CC BY 4.0",
        areas: WORLD,
    },
];

/// How far past the coverage a catalog's data reaches: heritage sites and terrain are made within
/// 20 km of it.
const NEAR_COVERAGE_KM: f64 = 20.0;

/// The credits a catalog lists, in the table's order: those for anywhere, and those whose areas meet
/// what it serves. That's the coverage it records (`regions`) and 20 km around it, and the built
/// units' ways (`extents`, E7), which reach past the coverage (ways are kept whole) and stay served
/// while their region's removal waits for a rebuild.
pub fn catalog_credits(regions: &[DrawnRegion], extents: &[[i32; 4]]) -> Vec<&'static Credit> {
    CREDITS.iter().filter(|c| listed(c.areas, regions, extents)).collect()
}

/// Whether a credit of areas `areas` (none: anywhere) is listed by a catalog serving `regions` and
/// the built units' ways `extents` (`catalog_credits`' rule; the inputs' descriptions' too,
/// crate::inputs::credits).
pub fn listed(areas: &[[f64; 4]], regions: &[DrawnRegion], extents: &[[i32; 4]]) -> bool {
    let e7 = |d: f64| (d * 1e7).round() as i32;
    areas.is_empty()
        || areas.iter().any(|a| {
            let b = [e7(a[0]), e7(a[1]), e7(a[2]), e7(a[3])];
            let near = grow(b, NEAR_COVERAGE_KM);
            regions.iter().any(|r| r.meets_rect(near)) || extents.iter().any(|&x| meets(b, x))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e7(w: f64, s: f64, e: f64, n: f64) -> [i32; 4] {
        [(w * 1e7) as i32, (s * 1e7) as i32, (e * 1e7) as i32, (n * 1e7) as i32]
    }

    #[test]
    fn rules_by_area() {
        let tokyo = versions_meeting(e7(139.0, 35.0, 140.0, 36.0));
        assert!(tokyo.contains("dem-japan=1") && tokyo.contains("networks-japan=1") && !tokyo.contains("north-america"));
        let quebec = versions_meeting(e7(-72.0, 46.0, -71.0, 47.0));
        assert!(quebec.contains("dem-north-america=1") && !quebec.contains("japan"));
        // The world's rules everywhere.
        assert!(tokyo.contains("dem-fabdem=1") && quebec.contains("spacing=1"));
    }

    /// A region of one drawn box, as a catalog records it.
    fn region(id: &str, w: f64, s: f64, e: f64, n: f64) -> DrawnRegion {
        let ring = vec![[w, s], [e, s], [e, n], [w, n], [w, s]];
        let entry = format!("poly:{id}.poly");
        DrawnRegion { id: id.into(), name: id.into(), outline: vec![entry.clone()], shapes: [(entry, vec![vec![ring]])].into() }
    }

    fn whats(cs: Vec<&Credit>) -> Vec<&'static str> {
        cs.iter().map(|c| c.what).collect()
    }

    #[test]
    fn credits_by_what_a_catalog_serves() {
        // Around Tokyo: Japan's sources and the world's, no one else's.
        let tokyo = whats(catalog_credits(&[region("tokyo", 139.5, 35.5, 140.0, 36.0)], &[]));
        for w in ["Road elevation, Japan", "Rail service frequency", "Land cover"] {
            assert!(tokyo.contains(&w), "{w}");
        }
        assert!(tokyo[0].starts_with("Roads, water") && tokyo.contains(&"Road elevation, Europe, Hong Kong, Singapore, Taiwan"));
        for w in ["Road elevation, North America", "Lakes missing from OpenStreetMap in parts of Canada", "Forest leaf type, Europe"] {
            assert!(!tokyo.contains(&w), "{w}");
        }
        // The data reaches 20 km past the coverage: Japan's box for a region about 10 km west of
        // it, not for one 70 km west.
        assert!(whats(catalog_credits(&[region("near", 122.3, 24.0, 122.4, 24.1)], &[])).contains(&"Road elevation, Japan"));
        assert!(!whats(catalog_credits(&[region("far", 121.7, 24.0, 121.8, 24.1)], &[])).contains(&"Road elevation, Japan"));
        // A unit's ways in France, its region since removed: Europe's credits stay with its data.
        let both = whats(catalog_credits(&[region("tokyo", 139.5, 35.5, 140.0, 36.0)], &[e7(2.2, 48.7, 2.5, 48.9)]));
        assert!(both.contains(&"Forest leaf type, Europe") && both.contains(&"Road elevation, Japan"));
        // Nothing served: the world's only.
        assert!(whats(catalog_credits(&[], &[])).iter().all(|w| CREDITS.iter().any(|c| c.what == *w && c.areas.is_empty())));
    }

    #[test]
    fn credit_areas_are_boxes() {
        for c in CREDITS {
            for a in c.areas {
                assert!(a[0] < a[2] && a[1] < a[3] && a[0] >= -180.0 && a[2] <= 180.0 && a[1] >= -90.0 && a[3] <= 90.0, "{}: {a:?}", c.what);
            }
        }
    }

    /// The notices the sources' terms ask for, or that limit the map's use, stay in the table.
    #[test]
    fn licence_notices_kept() {
        let has = |what: &str, terms: &str| CREDITS.iter().any(|c| c.what == what && c.terms.contains(terms));
        assert!(has("Roads, water, boundaries, places, parks, points of interest, Indigenous land boundaries", "© OpenStreetMap contributors, ODbL"));
        assert!(has("Road elevation, Europe, Hong Kong, Singapore, Taiwan", "CC BY-NC-SA 4.0"));
        assert!(has("Road elevation, Japan", "Public Data License 1.0"));
        assert!(has("3D buildings", "ODbL 1.0") && has("Building heights where none are known", "CC BY 4.0"));
        // (The 3D buildings' credit names the release they're built from.)
        assert!(CREDITS.iter().any(|c| c.what == "3D buildings" && c.source.contains(crate::buildtiles::RELEASE)));
        assert_eq!(CREDITS.len(), 22);
        // As catalogs carry them: areas only where a credit has some.
        let v = serde_json::to_value(&CREDITS[..2]).unwrap();
        assert!(v[0].get("areas").is_none() && v[1]["areas"][0][2] == -40.0);
    }
}
