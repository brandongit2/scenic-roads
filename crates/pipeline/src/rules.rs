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
//! comes from it, the boxes its source's code tests or, for a heritage register, its territory. A
//! catalog lists the credits whose areas meet what it serves (`catalog_credits`), and the map shows
//! the catalog's (© Credits). They enter no key: a credit changes what the map says, not its data.

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
    // dem/sample.py: the DEM sources and their order.
    Rule { name: "dem-north-america", version: 1, areas: NORTH_AMERICA },
    Rule { name: "dem-japan", version: 1, areas: JAPAN },
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

/// The DEM rules a sample depends on, by its source (dem/sample.py's `SRC_*`) and place (E7): the
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
        8 => vec![2],
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

/// Whether the box w, s, e, n (E7) meets Taiwan, where the MOI DTM (`inputs/moi-dtm/`, put there by
/// hand) is the DEM when it's there: its files' digest enters those units' keys.
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

// The heritage registers' territories (dem/heritage.py, heritage_eu.py, crhp.py): a register's
// sites lie there, and a run keeps those near the coverage. Generous, since a site missing its
// credit would break the register's terms, where one credited too many costs a line.

/// Mérimée and the sites patrimoniaux remarquables cover the overseas departments too.
const FRANCE: &[[f64; 4]] = &[
    [-5.2, 42.3, 8.3, 51.2],    // metropolitan France
    [8.5, 41.3, 9.6, 43.1],     // Corsica
    [-56.5, 46.7, -56.1, 47.2], // Saint-Pierre-et-Miquelon
    [-63.2, 14.3, -60.8, 18.2], // Guadeloupe, Martinique, Saint-Martin, Saint-Barthélemy
    [-54.7, 2.1, -51.5, 5.9],   // French Guiana
    [55.2, -21.4, 55.9, -20.8], // Réunion
    [44.9, -13.1, 45.4, -12.6], // Mayotte
];
const ENGLAND: &[[f64; 4]] = &[[-6.5, 49.8, 1.8, 55.85]];
const SCOTLAND: &[[f64; 4]] = &[[-9.0, 54.6, -0.7, 60.9]];
const WALES: &[[f64; 4]] = &[[-5.7, 51.3, -2.6, 53.5]];
const NORTHERN_IRELAND: &[[f64; 4]] = &[[-8.2, 54.0, -5.4, 55.4]];
/// The Bailiwick: Guernsey, Alderney, Sark and Herm.
const GUERNSEY: &[[f64; 4]] = &[[-2.7, 49.4, -2.1, 49.75]];
const IRELAND: &[[f64; 4]] = &[[-10.7, 51.3, -5.9, 55.5]];
/// The regions whose registers have locations, all on the mainland.
const SPAIN: &[[f64; 4]] = &[[-9.4, 35.9, 3.4, 43.8]];
/// The atlas covers the mainland only.
const PORTUGAL: &[[f64; 4]] = &[[-9.6, 36.9, -6.1, 42.2]];
const CANADA: &[[f64; 4]] = &[[-141.1, 41.6, -52.5, 83.2]];
const QUEBEC: &[[f64; 4]] = &[[-79.8, 44.9, -57.1, 62.6]];
const ONTARIO: &[[f64; 4]] = &[[-95.2, 41.6, -74.3, 56.9]];
const NOVA_SCOTIA: &[[f64; 4]] = &[[-66.4, 43.3, -59.6, 47.1]];
/// dem/crhp.py's provinces and territories (its `PROVINCES` boxes): New Brunswick, Prince Edward
/// Island, Newfoundland and Labrador, the western provinces and the territories.
const CRHP_PROVINCES: &[[f64; 4]] = &[
    [-69.1, 44.5, -63.7, 48.1],
    [-64.5, 45.9, -61.9, 47.1],
    [-67.9, 46.5, -52.5, 60.5],
    [-102.1, 48.9, -88.9, 60.1],
    [-110.0, 49.0, -101.3, 60.0],
    [-120.0, 49.0, -110.0, 60.0],
    [-139.1, 48.2, -114.0, 60.0],
    [-141.0, 60.0, -123.8, 69.7],
    [-136.5, 60.0, -101.9, 78.8],
    [-120.7, 51.6, -61.0, 83.2],
];
/// The states dem/heritage.py asks the National Register for (`STATES`): New York and New England.
const NRHP_STATES: &[[f64; 4]] = &[[-79.8, 40.4, -66.9, 47.5]];
/// The countries dem/railfeeds.py takes feeds from (`COUNTRIES`): Canada and the US; France with
/// Corsica, and Monaco; Spain, Portugal, Andorra and Gibraltar, with the Balearics and the Canaries;
/// Britain, Ireland, Man and the Channel Islands; Japan, Taiwan, Hong Kong and Singapore.
const RAIL_FEEDS: &[[f64; 4]] = &[
    NORTH_AMERICA[0],
    FRANCE[0],
    FRANCE[1],
    [-9.6, 35.9, 4.4, 43.8],
    [-18.2, 27.6, -13.4, 29.5],
    [-10.7, 49.1, 1.8, 60.9],
    JAPAN[0],
    TAIWAN[0],
    HONG_KONG[0],
    SINGAPORE[0],
];

/// Every data source's credit, in the order the map lists them. The app's own (the colour ramps it
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
        what: "Road elevation, Japan",
        source: "Created by editing GSI Tiles (elevation tiles (Fundamental Geospatial Data Digital Elevation Model)): 地理院タイル（標高タイル（基盤地図情報数値標高モデル））を加工して作成, Geospatial Information Authority of Japan (maps.gsi.go.jp/development/ichiran.html)",
        terms: "GSI terms of use (Public Data License 1.0)",
        areas: JAPAN,
    },
    Credit {
        what: "Road elevation, Taiwan",
        source: "內政部 2025年版全臺灣20公尺網格數值地形模型DTM資料 (Ministry of the Interior, Taiwan, 20 m DTM, 2025 edition). The Open Data is made available to the public under the Open Government Data License, User can make use of it when complying to the condition and obligation of its terms. Open Government Data License: https://data.gov.tw/license",
        terms: "Open Government Data License 1.0",
        areas: TAIWAN,
    },
    Credit {
        what: "Road elevation, Europe, Hong Kong, Singapore (and Taiwan without the MOI DTM)",
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
        what: "UNESCO World Heritage",
        source: "UNESCO World Heritage Centre, World Heritage List (data.unesco.org)",
        terms: "© UNESCO World Heritage Centre, CC BY-SA 4.0",
        areas: WORLD,
    },
    Credit {
        what: "France heritage",
        source: "Ministère de la Culture, base Mérimée (POP); sites patrimoniaux remarquables via the Géoportail de l'Urbanisme",
        terms: "Licence Ouverte 2.0",
        areas: FRANCE,
    },
    Credit {
        what: "Andorra heritage",
        source: "Govern d'Andorra, Inventari general del patrimoni cultural (IDE Andorra)",
        terms: "Private, personal use only",
        areas: ANDORRA,
    },
    Credit {
        what: "Canadian federal designations",
        source: "Parks Canada Directory of Federal Heritage Designations",
        terms: "OGL–Canada",
        areas: CANADA,
    },
    Credit {
        what: "US designations",
        source: "NPS National Register of Historic Places",
        terms: "Public domain",
        areas: NRHP_STATES,
    },
    Credit {
        what: "Québec heritage",
        source: "MCC Répertoire du patrimoine culturel",
        terms: "CC BY 4.0",
        areas: QUEBEC,
    },
    Credit {
        what: "Ontario heritage",
        source: "Ontario Heritage Act Register (Ontario Heritage Trust)",
        terms: "Personal non-commercial use only",
        areas: ONTARIO,
    },
    Credit {
        what: "Nova Scotia heritage",
        source: "Registered Heritage Properties; Halifax (HRM) municipal heritage",
        terms: "NS Open Government Licence; HRM open data",
        areas: NOVA_SCOTIA,
    },
    Credit {
        what: "NB, PEI, NL, western & northern Canada heritage",
        source: "Canadian Register of Historic Places; Moncton open data",
        terms: "Non-commercial reproduction with credit",
        areas: CRHP_PROVINCES,
    },
    Credit {
        what: "Biosphere reserves, geoparks, dark-sky places",
        source: "UNESCO MAB & Global Geoparks, DarkSky International, RASC",
        terms: "Facts from the official registries",
        areas: WORLD,
    },
    Credit {
        what: "England heritage",
        source: "© Historic England, National Heritage List for England; contains Ordnance Survey data © Crown copyright and database right",
        terms: "OGL v3",
        areas: ENGLAND,
    },
    Credit {
        what: "Scotland heritage",
        source: "Contains Historic Environment Scotland and Ordnance Survey data © Historic Environment Scotland – Scottish Charity No. SC045925 © Crown copyright and database right",
        terms: "OGL v3",
        areas: SCOTLAND,
    },
    Credit {
        what: "Wales heritage",
        source: "Designated Historic Asset GIS Data, The Welsh Historic Environment Service (Cadw), via DataMapWales",
        terms: "OGL v3",
        areas: WALES,
    },
    Credit {
        what: "Northern Ireland heritage",
        source: "Department for Communities, Historic Environment Division",
        terms: "OGL v3",
        areas: NORTHERN_IRELAND,
    },
    Credit {
        what: "Guernsey heritage",
        source: "States of Guernsey Development & Planning Authority",
        terms: "gov.gg terms: research and private use",
        areas: GUERNSEY,
    },
    Credit {
        what: "Ireland heritage",
        source: "National Inventory of Architectural Heritage; National Monuments Service (Department of Housing, Local Government and Heritage)",
        terms: "CC BY 4.0",
        areas: IRELAND,
    },
    Credit {
        what: "Spain heritage",
        source: "Generalitat de Catalunya; Junta de Castilla y León; Gobierno de Aragón; Generalitat Valenciana; Xunta de Galicia; Gobierno de Navarra (IDENA); Junta de Extremadura; IAPH (Junta de Andalucía)",
        terms: "Per region: CC BY / CC BY-SA / free use with credit",
        areas: SPAIN,
    },
    Credit {
        what: "Portugal heritage",
        source: "Património Cultural, I.P., Atlas do Património Classificado e em Vias de Classificação",
        terms: "CC BY-NC 4.0",
        areas: PORTUGAL,
    },
    Credit {
        what: "Hong Kong heritage",
        source: "Antiquities and Monuments Office, via the Common Spatial Data Infrastructure (CSDI) Portal",
        terms: "DATA.GOV.HK terms",
        areas: HONG_KONG,
    },
    Credit {
        what: "Japan heritage",
        source: "出典：文化庁 国指定文化財等データベース（https://kunishitei.bunka.go.jp/）を加工して作成 (Agency for Cultural Affairs, Database of National Cultural Properties, edited); preservation districts: 国土数値情報（伝統的建造物群保存地区データ）(MLIT)",
        terms: "PDL 1.0 (CC BY 4.0 compatible); CC BY 4.0",
        areas: JAPAN,
    },
    Credit {
        what: "Taiwan heritage",
        source: "文化部文化資產局 2026 文化資產個案 (Bureau of Cultural Heritage, Ministry of Culture). The Open Data is made available to the public under the Open Government Data License, User can make use of it when complying to the condition and obligation of its terms. Open Government Data License: https://data.gov.tw/license",
        terms: "Open Government Data License 1.0",
        areas: TAIWAN,
    },
    Credit {
        what: "Singapore heritage",
        source: "Contains information from Monuments (NHB), Historic Sites (NHB) and Master Plan 2019 SDCP Conservation Area layer (URA) accessed on 2026-09-29 from data.gov.sg which is made available under the terms of the Singapore Open Data Licence version 1.0 https://data.gov.sg/open-data-licence",
        terms: "Singapore Open Data Licence 1.0",
        areas: SINGAPORE,
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
    Credit {
        what: "Rail service frequency",
        source: "Operators’ GTFS timetables (112 feeds via the Mobility Database catalogue and operators: SNCF, Renfe, IDFM, TfI, MTA, MBTA, GO, exo, VIA, Amtrak and others; Great Britain: the Rail Delivery Group timetable as GTFS by Catenary Transit; Singapore: Land Transport Authority, LTA DataMall, under the Singapore Open Data Licence 1.0); MTR frequencies from mtr.com.hk (exact for the Airport Express and High Speed Rail, whose timetables are published in full; other lines a lower bound, at least the service hours at the slowest published off-peak headway)",
        terms: "Each operator’s open-data terms",
        areas: RAIL_FEEDS,
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
];

/// How far past the coverage a catalog's data reaches: heritage sites and terrain are made within
/// 20 km of it.
const NEAR_COVERAGE_KM: f64 = 20.0;

/// The credits a catalog lists, in the table's order: those for anywhere, and those whose areas meet
/// what it serves. That's the coverage it records (`regions`) and 20 km around it, and the built
/// units' ways (`extents`, E7), which reach past the coverage (ways are kept whole) and stay served
/// while their region's removal waits for a rebuild.
pub fn catalog_credits(regions: &[DrawnRegion], extents: &[[i32; 4]]) -> Vec<&'static Credit> {
    let e7 = |d: f64| (d * 1e7).round() as i32;
    CREDITS
        .iter()
        .filter(|c| {
            c.areas.is_empty()
                || c.areas.iter().any(|a| {
                    let b = [e7(a[0]), e7(a[1]), e7(a[2]), e7(a[3])];
                    let near = grow(b, NEAR_COVERAGE_KM);
                    regions.iter().any(|r| r.meets_rect(near)) || extents.iter().any(|&x| meets(b, x))
                })
        })
        .collect()
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
        for w in ["Road elevation, Japan", "Japan heritage", "Rail service frequency", "UNESCO World Heritage"] {
            assert!(tokyo.contains(&w), "{w}");
        }
        assert!(tokyo[0].starts_with("Roads, water") && tokyo.contains(&"Road elevation, Europe, Hong Kong, Singapore (and Taiwan without the MOI DTM)"));
        for w in ["Road elevation, North America", "France heritage", "Taiwan heritage", "Forest leaf type, Europe"] {
            assert!(!tokyo.contains(&w), "{w}");
        }
        // Heritage sites reach 20 km past the coverage: Andorra's register for a region 11 km south
        // of Andorra, not for one 50 km south.
        assert!(whats(catalog_credits(&[region("urgell", 1.5, 42.25, 1.6, 42.32)], &[])).contains(&"Andorra heritage"));
        assert!(!whats(catalog_credits(&[region("solsona", 1.5, 41.9, 1.6, 41.97)], &[])).contains(&"Andorra heritage"));
        // A unit's ways in France, its region since removed: France's credits stay with its data.
        let both = whats(catalog_credits(&[region("tokyo", 139.5, 35.5, 140.0, 36.0)], &[e7(2.2, 48.7, 2.5, 48.9)]));
        assert!(both.contains(&"France heritage") && both.contains(&"Forest leaf type, Europe") && both.contains(&"Japan heritage"));
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
        assert!(has("Road elevation, Europe, Hong Kong, Singapore (and Taiwan without the MOI DTM)", "CC BY-NC-SA 4.0"));
        assert!(has("UNESCO World Heritage", "CC BY-SA 4.0"));
        assert!(has("Portugal heritage", "CC BY-NC 4.0"));
        assert!(has("Ontario heritage", "Personal non-commercial use only"));
        assert!(has("Andorra heritage", "Private, personal use only"));
        assert!(has("Guernsey heritage", "research and private use"));
        assert!(has("Taiwan heritage", "Open Government Data License 1.0"));
        assert!(has("Road elevation, Japan", "Public Data License 1.0"));
        assert!(has("England heritage", "OGL v3"));
        assert_eq!(CREDITS.len(), 42);
        // As catalogs carry them: areas only where a credit has some.
        let v = serde_json::to_value(&CREDITS[..2]).unwrap();
        assert!(v[0].get("areas").is_none() && v[1]["areas"][0][2] == -40.0);
    }
}
