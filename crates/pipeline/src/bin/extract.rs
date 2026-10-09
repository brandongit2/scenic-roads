//! Extract car-accessible public roads (and car ferries) and passenger rail lines from OSM PBF
//! extracts.
//!
//! Rail: tracks (railway=rail, light_rail, subway, tram, narrow_gauge, funicular, monorail,
//! preserved; no yards or sidings) used by a passenger route relation (route=train, tram, subway,
//! light_rail, monorail, funicular), grouped by service: trams, metro / rapid transit, commuter &
//! regional, intercity (sleepers included), heritage & mountain (tourist lines, rack railways,
//! funiculars). Tracks without a route relation are kept when their type says what they are
//! (tram, subway, funicular, preserved or tourist lines).
//!
//! usage: extract <out_dir> <spacing_m> [--rail-rels-only] <file.osm.pbf>...
//!
//! Pass 1 collects matching ways; pass 2 resolves node coordinates and private gates.
//! Besides the ways: rail-rels.bin, each rail track's primary service (the route relation that
//! names it; the app opens it on openstreetmap.org), sorted little-endian (i64 OSM way id, i64
//! relation id). `--rail-rels-only` writes just that (a build made before it existed), after
//! pass 1.
//! Output geometry is densified so consecutive vertices are at most `spacing_m` apart,
//! which lets the DEM stage sample every raster cell a road crosses. Outside North America and
//! Japan the elevation sources are 20–30 m, so there the spacing is at least `COARSE_SPACING_M`.

use det::Det;
use anyhow::{Context, Result};
use osmpbf::{Element, ElementReader};
use pipeline::{bytes_bar, morton, ProgressRead};
use rayon::prelude::*;
use roadcore::{class, dist_m, flag, WayRec, E7, WAYS_MAGIC};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::Relaxed};

struct RawWay {
    id: i64,
    route: String,
    class: u8,
    flags: u8,
    lanes: u8,
    maxspeed: u16,
    name: String,
    /// OSM's English name (`name:en`), roads only.
    name_en: String,
    ref_: String,
    surface: String,
    refs: Vec<i64>,
    /// Rail: service groups (bits, see `WayRec::rail`), line colour; `rail_fallback` = group
    /// when no route relation uses the track (u8::MAX: dropped then).
    rail: u8,
    colour: u32,
    rail_fallback: u8,
}

/// A passenger rail service (route relation) using a track.
struct RailUse {
    way: i64,
    /// The route relation.
    rel: i64,
    group: u8,
    colour: u32,
    /// Short label: the ref, else the name.
    label: String,
    name: String,
    ref_: String,
}

#[derive(Default)]
struct Pass1 {
    ways: Vec<RawWay>,
    /// (member way id, route name) of designated scenic routes.
    scenic: Vec<(i64, String)>,
    /// Points of interest mapped as areas: (kind, name, node refs, way id, kept tags).
    poi_ways: Vec<(&'static str, String, Vec<i64>, i64, Vec<(String, String)>)>,
    /// Hiking routes: (name, member way ids, relation id).
    hikes: Vec<(String, Vec<i64>, i64)>,
    rail_uses: Vec<RailUse>,
    /// Covered bridges' kept tags, by way id.
    bridge_tags: Vec<(i64, Vec<(String, String)>)>,
}

impl Pass1 {
    fn merge(mut a: Pass1, mut b: Pass1) -> Pass1 {
        if a.ways.len() < b.ways.len() {
            std::mem::swap(&mut a, &mut b);
        }
        a.ways.append(&mut b.ways);
        a.scenic.append(&mut b.scenic);
        a.poi_ways.append(&mut b.poi_ways);
        a.hikes.append(&mut b.hikes);
        a.bridge_tags.append(&mut b.bridge_tags);
        a.rail_uses.append(&mut b.rail_uses);
        a
    }
}

struct Poi {
    kind: &'static str,
    lon: i32,
    lat: i32,
    name: String,
    ele: Option<f32>,
    /// The OSM object it is ("n123", "w5"; none for a hiking route's end).
    osm: Option<String>,
    /// A reference for one that isn't an OSM object: a hiking route's end ("trail:<relation>:<node>").
    key: Option<String>,
    /// The tags its details show (KEEP_COMMON and its kind's own).
    tags: Vec<(String, String)>,
    /// A way's own nodes (points of interest mapped as ways, covered bridges): for the candidates'
    /// coverage clip.
    nodes: Vec<[i32; 2]>,
    /// A covered bridge's length (line_m).
    length_m: Option<u32>,
}

impl Poi {
    /// Its place in the one order points are kept in (docs/phase5.md "Candidates"): nodes, ways,
    /// then hiking-route ends, by id, then kind (a way can be a point of interest and a covered
    /// bridge). The readers' threads deliver points in no particular order.
    fn order(&self) -> (u8, i64, i64, &'static str) {
        let num = |s: &str| s.parse::<i64>().unwrap_or(0);
        match (&self.osm, &self.key) {
            (Some(o), _) if o.starts_with('n') => (0, num(&o[1..]), 0, self.kind),
            (Some(o), _) => (1, num(&o[1..]), 0, self.kind),
            (None, Some(k)) => {
                let mut it = k.trim_start_matches("trail:").split(':');
                (2, num(it.next().unwrap_or("")), num(it.next().unwrap_or("")), self.kind)
            }
            (None, None) => (3, 0, 0, self.kind),
        }
    }
}

/// Tags every stop & sight keeps for its details, and each kind's own (marksjob::detail_keys; plus
/// the Japanese romanisations, the English name where there's no name:en).
const KEEP_COMMON: &[&str] = &["name", "ele", "description", "website", "wikipedia", "wikidata", "operator", "access", "fee", "opening_hours", "start_date", "heritage", "alt_name", "name:en", "name:ja-Latn", "name:ja_rm"];

/// A line's length, planar (111.32 km per degree of longitude at the segment's mean latitude,
/// 110.57 of latitude), in metres rounded half-even.
fn line_m(nodes: &[[i32; 2]]) -> u32 {
    let mut km = 0f64;
    for w in nodes.windows(2) {
        let (x0, y0, x1, y1) = (w[0][0] as f64 * E7, w[0][1] as f64 * E7, w[1][0] as f64 * E7, w[1][1] as f64 * E7);
        let kx = 111.32 * ((y0 + y1) / 2.0).to_radians().dcos();
        km += ((x1 - x0) * kx).dhypot((y1 - y0) * 110.57);
    }
    (km * 1000.0).round_ties_even() as u32
}

/// How many grid cells of `cell_e7` (E7 degrees) east and west a search of `r_m` metres around a
/// point at `lat_e7` must look (a degree of longitude shrinks with the latitude; north and south,
/// one cell, as the cells are wider than the searches).
fn lon_cells(lat_e7: i32, r_m: f64, cell_e7: i64) -> i64 {
    let lat = (lat_e7 as f64 * E7).abs() + 0.01;
    let r_deg = r_m / (111_000.0 * lat.min(85.0).to_radians().dcos());
    ((r_deg / (cell_e7 as f64 * E7)).ceil() as i64).max(1)
}

fn keep_of(kind: &str) -> &'static [&'static str] {
    match kind {
        "peak" => &["prominence", "isolation", "munro", "corbett", "graham", "donald", "marilyn", "hewitt", "wainwright", "nuttall",
            "communication:amateur_radio:sota", "summit:cross", "summit:register", "volcano:status", "volcano:type", "natural", "tourism"],
        "waterfall" => &["height", "width", "intermittent", "seasonal", "waterway", "natural"],
        "lighthouse" => &["height", "seamark:light:character", "seamark:light:colour", "seamark:light:period", "seamark:light:range",
            "seamark:light:height", "seamark:light:sequence", "seamark:light:reference", "seamark:name", "building:colour", "tower:type",
            "historic", "heritage:operator", "seamark:light:1:character", "seamark:light:1:colour", "seamark:light:1:period",
            "seamark:light:1:range", "seamark:light:1:height", "man_made"],
        "viewpoint" => &["direction", "tower:type", "height", "man_made", "tourism"],
        "picnic_site" => &["toilets", "drinking_water", "shelter", "bench", "picnic_table", "fireplace", "bbq", "covered", "capacity", "tourism", "leisure"],
        "rest_area" => &["toilets", "drinking_water", "shelter", "picnic_table", "bench", "fuel", "restaurant", "shop", "wheelchair", "capacity", "highway"],
        "trailhead" | "trail_parking" => &["toilets", "drinking_water", "parking", "capacity", "route_ref", "hiking", "shelter", "highway", "amenity", "trailhead"],
        "covered_bridge" => &["bridge:structure", "bridge:name", "material", "historic", "bridge:ref", "layer", "bridge", "covered", "length"],
        _ => &[],
    }
}

/// A point's kept tags, sorted by key.
fn kept_tags(t: &Tags, kind: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = KEEP_COMMON
        .iter()
        .chain(keep_of(kind))
        .filter_map(|k| t.get(k).filter(|v| !v.is_empty()).map(|v| (k.to_string(), v.replace('\n', " "))))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Designated scenic routes: byway/scenic/tourist networks, scenic trails, Quebec's routes
/// touristiques ("Route du Fleuve", "Chemin du Roy" …), All-American Roads.
fn scenic_route(t: &Tags) -> Option<String> {
    if !t.is("route", "road") {
        return None;
    }
    let net = t.get("network").unwrap_or("").to_lowercase();
    let name_raw = t.get("name").unwrap_or("");
    let name = name_raw.to_lowercase();
    let numbered = name.starts_with("route ") && name[6..].starts_with(|c: char| c.is_ascii_digit());
    let hit = net.contains("scenic")
        || net.contains("byway")
        || net.contains("nsb")
        || net.contains(":ab:")
        || net.contains("tourist")
        || net.contains("touristique")
        || net == "us:glst"
        || net == "ca:ns:s"
        || name.contains("scenic")
        || name.contains("byway")
        || name.contains("all-american road")
        || name.contains("touristique")
        || name.ends_with(" trail")
        || name.contains("chemin du roy")
        || name.contains("chemins d'eau")
        || (!numbered && (name.starts_with("route du ") || name.starts_with("route de la ") || name.starts_with("route des ")));
    hit.then(|| if name_raw.is_empty() { t.get("network").unwrap_or("Scenic route").to_string() } else { name_raw.to_string() })
}

/// Car parks for a trail: named for one ("… Trail Parking", "Stationnement du sentier …",
/// "Sentiers de randonnée") or tagged for hiking. Whole words only ("trailer" parking isn't).
fn trail_parking(t: &Tags) -> bool {
    if !t.is("amenity", "parking") {
        return false;
    }
    if t.is("hiking", "yes") || t.is("trailhead", "yes") {
        return true;
    }
    let name = t.get("name").unwrap_or("").to_lowercase();
    name.split(|c: char| !c.is_alphanumeric())
        .any(|w| matches!(w, "trail" | "trails" | "trailhead" | "sentier" | "sentiers" | "randonnée" | "randonnee" | "hiking"))
}

/// A point's kind. `candidates`: as the landmark candidates have them (docs/phase5.md "pois"): a
/// viewpoint that is also a volcano is a peak (the unit's own
/// points keep it a viewpoint, for the road flags).
fn poi_kind(t: &Tags, candidates: bool) -> Option<&'static str> {
    if t.is("highway", "rest_area") {
        Some("rest_area")
    } else if t.is("tourism", "picnic_site") {
        Some("picnic_site")
    } else if t.is("highway", "trailhead") {
        Some("trailhead")
    } else if trail_parking(t) {
        Some("trail_parking") // a trailhead; see the de-duplication below
    } else if t.is("natural", "peak") {
        // Before viewpoints: summits are often tagged as both (Mont Blanc).
        Some("peak")
    } else if candidates && t.is("natural", "volcano") && t.is("tourism", "viewpoint") {
        Some("peak")
    } else if t.is("tourism", "viewpoint") {
        Some("viewpoint")
    } else if t.is("waterway", "waterfall") {
        Some("waterfall")
    } else if t.is("man_made", "lighthouse") {
        Some("lighthouse")
    } else {
        None
    }
}


struct Tags<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Tags<'a> {
    fn get(&self, k: &str) -> Option<&'a str> {
        self.0.iter().find(|(kk, _)| *kk == k).map(|(_, v)| *v)
    }
    fn is(&self, k: &str, v: &str) -> bool {
        self.get(k) == Some(v)
    }
}

/// Access values that make a road not publicly drivable.
fn denied_value(v: &str) -> bool {
    v.split(';').all(|p| {
        matches!(
            p.trim(),
            "no" | "private" | "customers" | "delivery" | "agricultural" | "forestry" | "permit"
                | "military" | "emergency" | "residents" | "psv" | "bus" | "taxi" | "official"
        )
    })
}

/// Most specific access key wins: motorcar > motor_vehicle > vehicle > access.
fn car_denied(t: &Tags) -> bool {
    for k in ["motorcar", "motor_vehicle", "vehicle", "access"] {
        if let Some(v) = t.get(k) {
            return denied_value(v);
        }
    }
    false
}

const UNPAVED: &[&str] = &[
    "unpaved", "gravel", "fine_gravel", "dirt", "earth", "ground", "sand", "grass", "compacted",
    "pebblestone", "mud", "rock", "woodchips", "clay", "grass_paver", "shells", "salt", "snow", "ice",
];

/// A passenger-capable track: (group when no route relation uses it, or u8::MAX), flags.
fn rail_track(t: &Tags) -> Option<(u8, u8)> {
    let kind = t.get("railway")?;
    if !matches!(kind, "rail" | "light_rail" | "subway" | "tram" | "narrow_gauge" | "funicular" | "monorail" | "preserved") {
        return None;
    }
    if matches!(t.get("service"), Some("yard" | "siding" | "spur" | "crossover")) || t.is("area", "yes")
        || matches!(t.get("usage"), Some("industrial" | "military" | "freight" | "test"))
    {
        return None;
    }
    let tourist = kind == "preserved" || t.is("railway:preserved", "yes") || t.is("usage", "tourism") || t.is("tourism", "yes");
    let rack = t.get("railway:rack").or(t.get("rack")).is_some_and(|v| v != "no");
    let fallback = if tourist || rack || kind == "funicular" {
        class::HERITAGE
    } else {
        match kind {
            "tram" => class::TRAM,
            "subway" | "monorail" | "light_rail" => class::METRO,
            _ => u8::MAX,
        }
    };
    let mut f = 0u8;
    if t.get("bridge").is_some_and(|v| v != "no") {
        f |= flag::BRIDGE;
    }
    if t.get("tunnel").is_some_and(|v| v != "no" && v != "culvert") {
        f |= flag::TUNNEL;
    }
    Some((fallback, f))
}

/// Names that mark a long-distance service when a train route has no `service` tag.
const INTERCITY_WORDS: &[&str] = &[
    "tgv", "inoui", "ouigo", "intercités", "intercites", "eurostar", "thalys", "lyria", "ave ", "alvia", "euromed",
    "iryo", "avlo", "talgo", "alfa pendular", "intercidades", "amtrak", "via rail", "acela", "lner", "avanti",
    "crosscountry", "cross country", "sleeper", "night riviera", "nightjet", "intercity", "inter city", "inter-city",
    "enterprise", "adirondack", "maple leaf", "vermonter", "ethan allen", "downeaster", "lake shore", "the canadian",
    "the ocean", "northeast regional", "hull chelsea", "transcantábrico", "costa verde express", "al andalus",
];

/// Service group of a passenger route relation.
fn rail_route(t: &Tags) -> Option<u8> {
    let route = t.get("route")?;
    let tourist = matches!(t.get("service"), Some("tourism" | "heritage")) || t.is("tourism", "yes") || t.is("historic", "yes")
        || t.is("railway:preserved", "yes") || t.is("heritage:railway", "yes");
    Some(match route {
        "tram" => if tourist { class::HERITAGE } else { class::TRAM },
        "subway" | "light_rail" | "monorail" => class::METRO,
        "funicular" => class::HERITAGE,
        "train" => {
            if tourist {
                class::HERITAGE
            } else {
                match t.get("service") {
                    Some("long_distance" | "high_speed" | "night" | "international" | "car_shuttle") => class::INTERCITY,
                    Some(_) => class::COMMUTER,
                    None => {
                        let text = ["name", "brand", "network", "operator"].iter().filter_map(|k| t.get(k)).collect::<Vec<_>>().join(" ").to_lowercase();
                        if INTERCITY_WORDS.iter().any(|w| text.contains(w)) { class::INTERCITY } else { class::COMMUTER }
                    }
                }
            }
        }
        _ => return None,
    })
}

/// OSM `colour` (#rgb, #rrggbb or a CSS name) as 0x01RRGGBB; 0 = none.
fn parse_colour(v: Option<&str>) -> u32 {
    let Some(v) = v else { return 0 };
    let v = v.trim().to_lowercase();
    let hex = v.strip_prefix('#').unwrap_or(&v);
    let rgb = if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        u32::from_str_radix(hex, 16).ok()
    } else if hex.len() == 3 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let n = u32::from_str_radix(hex, 16).ok().unwrap_or(0);
        Some(((n >> 8 & 15) * 0x110000) | ((n >> 4 & 15) * 0x1100) | ((n & 15) * 0x11))
    } else {
        match v.as_str() {
            "red" => Some(0xff0000), "blue" => Some(0x0000ff), "green" => Some(0x008000), "yellow" => Some(0xffff00),
            "orange" => Some(0xffa500), "purple" => Some(0x800080), "brown" => Some(0xa52a2a), "black" => Some(0x000000),
            "white" => Some(0xffffff), "grey" | "gray" => Some(0x808080), "pink" => Some(0xffc0cb), "cyan" | "aqua" => Some(0x00ffff),
            "magenta" | "fuchsia" => Some(0xff00ff), "navy" => Some(0x000080), "maroon" => Some(0x800000), "olive" => Some(0x808000),
            "teal" => Some(0x008080), "lime" => Some(0x00ff00), "silver" => Some(0xc0c0c0), "gold" => Some(0xffd700),
            "violet" => Some(0xee82ee), "darkgreen" => Some(0x006400), "darkblue" => Some(0x00008b), "lightblue" => Some(0xadd8e6),
            _ => None,
        }
    };
    rgb.map_or(0, |c| c | 1 << 24)
}

/// Route network by signage from the ref, road class and location (rough country boxes where
/// ref formats collide: "A1" is a UK A road, a Portuguese autoestrada or a Jersey road).
/// (Changing a rule here: bump its version in crates/pipeline/src/rules.rs, "networks-…", so the units
/// it applies to rerun.)
fn network_code(lon: f64, lat: f64, ref_: &str, c: u8) -> u8 {
    use roadcore::network as n;
    let first = ref_.split(';').next().unwrap_or("").trim();
    let up = first.to_uppercase();
    let pre: String = up.chars().take_while(|ch| ch.is_ascii_alphabetic()).collect();
    let rest = &up[pre.len()..];
    let sep = rest.chars().next();
    let num = rest.trim_start_matches([' ', '-']).chars().next().is_some_and(|ch| ch.is_ascii_digit());
    if lon < -40.0 {
        // North America.
        if first.is_empty() {
            return n::NONE;
        }
        if (pre == "I" || up.starts_with("I-")) && num {
            return n::US_INTERSTATE;
        }
        if pre == "US" && num {
            return n::US_HIGHWAY;
        }
        if matches!(pre.as_str(), "NY" | "VT" | "NH" | "ME" | "MA" | "CT" | "RI" | "PA" | "NJ" | "SR") && num {
            return n::US_STATE;
        }
        if pre == "CR" || pre == "CO" {
            return n::US_COUNTY;
        }
        if pre == "A" || c == class::MOTORWAY {
            return n::CA_AUTOROUTE;
        }
        return if c >= class::PRIMARY { n::CA_PROVINCIAL } else { n::CA_REGIONAL };
    }
    if (113.8..114.5).contains(&lon) && (22.1..22.6).contains(&lat) {
        return if first.chars().next().is_some_and(|ch| ch.is_ascii_digit()) { n::HK_ROUTE } else { n::NONE };
    }
    // Singapore: expressways by their initials (PIE, AYE, CTE…).
    if (103.5..104.2).contains(&lon) && (1.1..1.5).contains(&lat) {
        return if c == class::MOTORWAY && !first.is_empty() { n::SG_EXPRESSWAY } else { n::NONE };
    }
    let lead_digit = first.chars().next().is_some_and(|ch| ch.is_ascii_digit());
    // Japan: expressways (E-numbers, or any motorway), national routes (trunk roads with a number),
    // prefectural roads (other numbered roads).
    if (122.5..154.0).contains(&lon) && (20.0..46.5).contains(&lat) {
        return if c == class::MOTORWAY || (pre == "E" && num) {
            n::JP_EXPRESSWAY
        } else if !lead_digit {
            n::NONE
        } else if c == class::TRUNK {
            n::JP_NATIONAL
        } else {
            n::JP_PREFECTURAL
        };
    }
    // Taiwan: national freeways (motorways, 1–10), provincial highways (1–88), county roads
    // (101–205); a heavenly-stem suffix (甲乙丙…) marks a branch. Township roads (a county
    // character, 南74) count as unnumbered.
    if (118.0..122.5).contains(&lon) && (21.5..26.6).contains(&lat) {
        let k: u32 = first.chars().take_while(|ch| ch.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
        return match k {
            0 => n::NONE,
            _ if c == class::MOTORWAY && k <= 10 => n::TW_FREEWAY,
            1..=99 => n::TW_PROVINCIAL,
            _ => n::TW_COUNTY,
        };
    }
    if first.is_empty() || !num {
        return n::NONE;
    }
    if pre == "E" {
        return n::E_ROAD;
    }
    // Andorra: CG-1 … CG-6, CS-xxx.
    if (1.40..1.79).contains(&lon) && (42.42..42.66).contains(&lat) && matches!(pre.as_str(), "CG" | "CS") {
        return if pre == "CG" { n::AD_GENERAL } else { n::AD_SECUNDARIA };
    }
    // France and Monaco write "A 7", "N 7", "D 1075", "M 6007".
    if sep == Some(' ') {
        match pre.as_str() {
            "A" => return n::FR_AUTOROUTE,
            "N" | "RN" => return n::FR_NATIONALE,
            "D" | "RD" => return n::FR_DEPARTEMENTALE,
            "M" => return n::FR_METROPOLE,
            _ => {}
        }
    }
    // Spain writes "A-7", "AP-7", "N-340", "C-31", "CV-500", "M-30" …
    if sep == Some('-') && lat < 44.0 && lon < 4.5 && !(lon < -6.2 && lat < 42.2 && pre.len() <= 2 && matches!(pre.as_str(), "A" | "IP" | "IC" | "N" | "EN" | "R" | "ER" | "M" | "EM")) {
        return match pre.as_str() {
            "A" | "AP" | "AG" | "AC" | "AS" | "AV" | "EX" if c >= class::TRUNK => n::ES_AUTOVIA,
            "A" | "AP" => n::ES_AUTOVIA,
            "N" => n::ES_NACIONAL,
            _ if c >= class::SECONDARY => n::ES_AUTONOMICA,
            _ => n::ES_LOCAL,
        };
    }
    // Portugal (mainland box): A, IP, IC, N/EN, R/ER, M/EM.
    if lon < -6.1 && lat < 42.2 && lat > 36.8 {
        return match pre.as_str() {
            "A" => n::PT_AUTOESTRADA,
            "IP" => n::PT_IP,
            "IC" => n::PT_IC,
            "N" | "EN" => n::PT_NACIONAL,
            _ => n::PT_REGIONAL,
        };
    }
    // Ireland (the Republic): M, N (national primary ≤ 33, secondary above), R.
    let ireland = lon < -5.9 && (51.3..55.5).contains(&lat) && !(lon > -8.2 && lat > 54.0 && !matches!(pre.as_str(), "N" | "R"));
    if ireland && matches!(pre.as_str(), "M" | "N" | "R") {
        let k: u32 = rest.trim_start_matches([' ', '-']).chars().take_while(|ch| ch.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
        return match pre.as_str() {
            "M" => n::IE_MOTORWAY,
            "N" if k <= 33 => n::IE_NATIONAL_PRIMARY,
            "N" => n::IE_NATIONAL_SECONDARY,
            _ => n::IE_REGIONAL,
        };
    }
    // Great Britain, Northern Ireland, Isle of Man, Channel Islands: M, A (primary = trunk), B.
    if lat > 49.1 {
        return match pre.as_str() {
            "M" => n::GB_MOTORWAY,
            "A" if c >= class::TRUNK => n::GB_A_PRIMARY,
            "A" => n::GB_A,
            "B" => n::GB_B,
            _ => n::NONE,
        };
    }
    n::NONE
}

fn classify(t: &Tags) -> Option<(u8, u8)> {
    if t.is("route", "ferry") {
        let carries_cars = ["motorcar", "motor_vehicle", "vehicle", "hgv"]
            .iter()
            .any(|k| matches!(t.get(k), Some("yes" | "designated" | "permissive")))
            || t.get("ferry").is_some();
        if !carries_cars || car_denied(t) {
            return None;
        }
        return Some((class::FERRY, 0));
    }
    let hw = t.get("highway")?;
    let (c, link) = match hw {
        "motorway" => (class::MOTORWAY, false),
        "motorway_link" => (class::MOTORWAY, true),
        "trunk" => (class::TRUNK, false),
        "trunk_link" => (class::TRUNK, true),
        "primary" => (class::PRIMARY, false),
        "primary_link" => (class::PRIMARY, true),
        "secondary" => (class::SECONDARY, false),
        "secondary_link" => (class::SECONDARY, true),
        "tertiary" => (class::TERTIARY, false),
        "tertiary_link" => (class::TERTIARY, true),
        "unclassified" | "road" => (class::UNCLASSIFIED, false),
        "residential" => (class::RESIDENTIAL, false),
        "living_street" => (class::LIVING_STREET, false),
        "service" => (class::SERVICE, false),
        _ => return None,
    };
    if c == class::SERVICE
        && matches!(
            t.get("service"),
            Some("parking_aisle" | "driveway" | "drive-through" | "emergency_access" | "parking" | "private")
        )
    {
        return None;
    }
    if t.is("area", "yes")
        || car_denied(t)
        || t.is("winter_road", "yes")
        || t.is("ice_road", "yes")
        || t.is("seasonal", "winter")
        || t.is("4wd_only", "yes")
        || t.is("smoothness", "impassable")
    {
        return None;
    }
    let mut f = 0u8;
    if link {
        f |= flag::LINK;
    }
    if t.get("bridge").is_some_and(|v| v != "no") {
        f |= flag::BRIDGE;
    }
    if t.get("tunnel").is_some_and(|v| v != "no" && v != "culvert") {
        f |= flag::TUNNEL;
    }
    if t.get("surface").is_some_and(|s| UNPAVED.contains(&s)) {
        f |= flag::UNPAVED;
    }
    if matches!(t.get("oneway"), Some("yes" | "1" | "true" | "-1" | "reversible"))
        || matches!(t.get("junction"), Some("roundabout" | "circular"))
        || (c == class::MOTORWAY && !t.is("oneway", "no"))
    {
        f |= flag::ONEWAY;
    }
    if t.is("toll", "yes") {
        f |= flag::TOLL;
    }
    if t.is("scenic", "yes") {
        f |= flag::SCENIC;
    }
    if f & flag::BRIDGE != 0 && (t.is("covered", "yes") || t.is("bridge", "covered")) {
        f |= flag::COVERED;
    }
    Some((c, f))
}

fn parse_maxspeed(v: Option<&str>) -> u16 {
    let Some(v) = v else { return 0 };
    let v = v.trim();
    let num: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    let Ok(n) = num.parse::<f64>() else { return 0 };
    let kmh = if v.contains("mph") { n * 1.609_344 } else { n };
    kmh.round().min(300.0) as u16
}

fn open_reader(path: &Path, label: &str) -> Result<ElementReader<ProgressRead<std::io::BufReader<File>>>> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let len = f.metadata()?.len();
    let pb = bytes_bar(len, label.to_string());
    Ok(ElementReader::new(ProgressRead { inner: std::io::BufReader::with_capacity(1 << 20, f), pb }))
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().collect();
    let rail_rels_only = args.iter().any(|a| a == "--rail-rels-only");
    args.retain(|a| a != "--rail-rels-only");
    // The landmark candidates' points (poi_kind), for the pois job.
    let candidates = args.iter().any(|a| a == "--candidates");
    args.retain(|a| a != "--candidates");
    // The pass's hiking-route ends (pipeline::trailends), instead of working them out from the input.
    let trailends: Option<PathBuf> = args.iter().position(|a| a == "--trailends").map(|i| {
        let p = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
        args.drain(i..(i + 2).min(args.len()));
        p
    });
    if args.len() < 4 {
        eprintln!("usage: extract <out_dir> <spacing_m> [--rail-rels-only] [--candidates] [--trailends <file>] <file.osm.pbf>...");
        std::process::exit(2);
    }
    let out = PathBuf::from(&args[1]);
    let spacing: f64 = args[2].parse()?;
    let inputs: Vec<PathBuf> = args[3..].iter().map(PathBuf::from).collect();
    std::fs::create_dir_all(&out)?;
    let t0 = std::time::Instant::now();

    // ---- Pass 1: ways ------------------------------------------------------------
    let mut ways: Vec<RawWay> = Vec::new();
    let mut scenic: Vec<(i64, String)> = Vec::new();
    let mut poi_ways: Vec<(&'static str, String, Vec<i64>, i64, Vec<(String, String)>)> = Vec::new();
    let mut hikes: Vec<(String, Vec<i64>, i64)> = Vec::new();
    let mut bridge_tags: Vec<(i64, Vec<(String, String)>)> = Vec::new();
    let mut rail_uses: Vec<RailUse> = Vec::new();
    for p in &inputs {
        let name = p.file_name().unwrap().to_string_lossy().replace(".osm.pbf", "");
        let r = open_reader(p, &format!("ways · {name}"))?;
        let mut got = r.par_map_reduce(
            |el| {
                let mut out = Pass1::default();
                match el {
                    Element::Way(w) => {
                        let t = Tags(w.tags().collect());
                        if let Some(kind) = poi_kind(&t, candidates) {
                            out.poi_ways.push((kind, t.get("name").unwrap_or("").to_string(), w.refs().collect(), w.id(), kept_tags(&t, kind)));
                        }
                        if t.is("covered", "yes") || t.is("bridge", "covered") {
                            out.bridge_tags.push((w.id(), kept_tags(&t, "covered_bridge")));
                        }
                        if let Some((c, f)) = classify(&t) {
                            out.ways.push(RawWay {
                                id: w.id(),
                                route: if f & flag::SCENIC != 0 { t.get("name").unwrap_or("Scenic road").to_string() } else { String::new() },
                                class: c,
                                flags: f,
                                lanes: t.get("lanes").and_then(|v| v.parse().ok()).unwrap_or(0),
                                maxspeed: parse_maxspeed(t.get("maxspeed")),
                                name: t.get("name").unwrap_or("").replace('\n', " "),
                                name_en: t.get("name:en").unwrap_or("").trim().replace('\n', " "),
                                ref_: t.get("ref").unwrap_or("").replace('\n', " "),
                                surface: t.get("surface").unwrap_or("").replace('\n', " "),
                                refs: w.refs().collect(),
                                rail: 0,
                                colour: 0,
                                rail_fallback: u8::MAX,
                            });
                        } else if let Some((fallback, f)) = rail_track(&t) {
                            out.ways.push(RawWay {
                                id: w.id(),
                                route: String::new(),
                                class: class::TRAM, // set from the services below
                                flags: f,
                                lanes: 0,
                                maxspeed: parse_maxspeed(t.get("maxspeed")),
                                name: t.get("name").unwrap_or("").replace('\n', " "),
                                name_en: String::new(),
                                ref_: String::new(),
                                surface: String::new(),
                                refs: w.refs().collect(),
                                rail: 0,
                                colour: parse_colour(t.get("colour")),
                                rail_fallback: fallback,
                            });
                        }
                    }
                    Element::Relation(r) => {
                        let t = Tags(r.tags().collect());
                        if matches!(t.get("route"), Some("hiking" | "foot")) {
                            let name = t.get("name").or(t.get("ref")).unwrap_or("").replace('\n', " ");
                            let ids: Vec<i64> = r.members().filter(|m| m.member_type == osmpbf::RelMemberType::Way).map(|m| m.member_id).collect();
                            if !ids.is_empty() {
                                out.hikes.push((name, ids, r.id()));
                            }
                        }
                        if let Some(group) = rail_route(&t) {
                            let (name, ref_) = (t.get("name").unwrap_or("").replace('\n', " "), t.get("ref").unwrap_or("").replace('\n', " "));
                            let label = if ref_.is_empty() { name.clone() } else { ref_.clone() };
                            let colour = parse_colour(t.get("colour"));
                            for m in r.members() {
                                if m.member_type == osmpbf::RelMemberType::Way {
                                    let role = m.role().unwrap_or("");
                                    if role.is_empty() || matches!(role, "forward" | "backward" | "main" | "route") {
                                        out.rail_uses.push(RailUse { way: m.member_id, rel: r.id(), group, colour, label: label.clone(), name: name.clone(), ref_: ref_.clone() });
                                    }
                                }
                            }
                        }
                        if let Some(route) = scenic_route(&t) {
                            for m in r.members() {
                                if m.member_type == osmpbf::RelMemberType::Way {
                                    out.scenic.push((m.member_id, route.replace('\n', " ")));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                out
            },
            Pass1::default,
            Pass1::merge,
        )?;
        eprintln!("  {name}: {} ways, {} scenic-route members", got.ways.len(), got.scenic.len());
        ways.append(&mut got.ways);
        scenic.append(&mut got.scenic);
        poi_ways.append(&mut got.poi_ways);
        hikes.append(&mut got.hikes);
        rail_uses.append(&mut got.rail_uses);
        bridge_tags.append(&mut got.bridge_tags);
    }
    bridge_tags.par_sort_unstable_by_key(|b| b.0);
    bridge_tags.dedup_by_key(|b| b.0);
    ways.par_sort_unstable_by_key(|w| w.id);
    ways.dedup_by_key(|w| w.id);
    // Rail: the services using each track. The primary group (drawing class, colour, name) is
    // the most important one: heritage > intercity > commuter > metro > tram.
    let mut rail_rels: Vec<(i64, i64)> = Vec::new();
    {
        let rank = |g: u8| match g {
            class::HERITAGE => 0,
            class::INTERCITY => 1,
            class::COMMUTER => 2,
            class::METRO => 3,
            _ => 4,
        };
        rail_uses.par_sort_unstable_by(|a, b| a.way.cmp(&b.way).then(rank(a.group).cmp(&rank(b.group))).then(a.label.cmp(&b.label)).then(a.rel.cmp(&b.rel)));
        rail_uses.dedup_by(|a, b| a.way == b.way && a.group == b.group && a.label == b.label);
        let (mut kept, mut dropped, mut via_routes) = (0usize, 0usize, 0usize);
        for w in ways.iter_mut() {
            if w.class != class::TRAM {
                continue; // roads (rail tracks are all provisionally TRAM here)
            }
            let lo = rail_uses.partition_point(|u| u.way < w.id);
            let hi = rail_uses.partition_point(|u| u.way <= w.id);
            let uses = &rail_uses[lo..hi];
            if uses.is_empty() {
                if w.rail_fallback == u8::MAX {
                    w.class = u8::MAX; // freight-only or unknown: dropped below
                    dropped += 1;
                    continue;
                }
                w.class = w.rail_fallback;
                w.rail = 1 << (w.class - class::TRAM);
                kept += 1;
                continue;
            }
            via_routes += 1;
            kept += 1;
            for u in uses {
                w.rail |= 1 << (u.group - class::TRAM);
            }
            let p = &uses[0];
            rail_rels.push((w.id, p.rel));
            w.class = p.group;
            if p.colour != 0 {
                w.colour = p.colour;
            }
            // Name: the primary service's name (the track's own name is usually an infrastructure
            // line name); refs: every service's ref, so a hovered line follows its own service.
            if !p.name.is_empty() {
                w.name = p.name.clone();
            }
            let mut refs: Vec<&str> = uses.iter().map(|u| u.ref_.as_str()).filter(|r| !r.is_empty()).collect();
            refs.dedup();
            w.ref_ = refs.join(";");
            let mut labels: Vec<&str> = uses.iter().map(|u| u.label.as_str()).filter(|l| !l.is_empty()).collect();
            labels.dedup();
            let more = labels.len().saturating_sub(4);
            w.route = labels[..labels.len().min(4)].join(" · ") + &if more > 0 { format!(" +{more}") } else { String::new() };
        }
        ways.retain(|w| w.class != u8::MAX);
        eprintln!("rail: {kept} passenger tracks ({via_routes} on route relations), {dropped} other tracks dropped");
    }
    // (in way id order, as the ways are)
    let rail_rels_bytes: Vec<u8> = rail_rels.iter().flat_map(|(w, r)| w.to_le_bytes().into_iter().chain(r.to_le_bytes())).collect();
    std::fs::write(roadcore::tmp(&out, "rail-rels.bin"), &rail_rels_bytes)?;
    if rail_rels_only {
        roadcore::commit(&out, &["rail-rels.bin"])?;
        eprintln!("wrote rail-rels.bin: {} tracks ({:.0?})", rail_rels.len(), t0.elapsed());
        return Ok(());
    }
    // Flag members of scenic routes (shortest route name wins when a way is in several).
    scenic.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.len().cmp(&b.1.len())));
    scenic.dedup_by_key(|x| x.0);
    let mut n_scenic = 0;
    for (id, route) in &scenic {
        if let Ok(i) = ways.binary_search_by_key(id, |w| w.id) {
            ways[i].flags |= flag::SCENIC;
            ways[i].route = route.clone();
            n_scenic += 1;
        }
    }
    eprintln!("pass 1: {} unique ways, {} on scenic routes ({:.0?})", ways.len(), n_scenic, t0.elapsed());

    // ---- Pass 1b: end nodes of hiking-route member ways (relations come after ways in a file) ----
    // (Not when the pass's ends are given: those are the whole routes', and carry their positions.)
    let given_ends: Option<Vec<pipeline::trailends::End>> = trailends.as_deref().map(pipeline::trailends::read).transpose()?;
    if given_ends.is_some() {
        hikes.clear();
    }
    let mut hike_ids: Vec<i64> = hikes.iter().flat_map(|h| h.1.iter().copied()).collect();
    hike_ids.par_sort_unstable();
    hike_ids.dedup();
    let mut hike_ends: Vec<(i64, i64, i64)> = Vec::new(); // (way id, first node, last node)
    if !hike_ids.is_empty() {
        for p in &inputs {
            let name = p.file_name().unwrap().to_string_lossy().replace(".osm.pbf", "");
            let r = open_reader(p, &format!("hiking routes · {name}"))?;
            let mut got = r.par_map_reduce(
                |el| match el {
                    Element::Way(w) if hike_ids.binary_search(&w.id()).is_ok() => {
                        let refs: Vec<i64> = w.refs().collect();
                        match (refs.first(), refs.last()) {
                            (Some(&a), Some(&b)) => vec![(w.id(), a, b)],
                            _ => Vec::new(),
                        }
                    }
                    _ => Vec::new(),
                },
                Vec::new,
                |mut a, mut b| {
                    a.append(&mut b);
                    a
                },
            )?;
            hike_ends.append(&mut got);
        }
        hike_ends.par_sort_unstable();
        hike_ends.dedup_by_key(|e| e.0);
    }
    // A route's ends: way ends used once (the route's own start and finish, where it meets a road
    // or car park). Only simple linear routes (exactly two such ends): branches and loops don't say
    // which end is the way in.
    let mut route_ends: Vec<(String, i64, i64)> = Vec::new();
    for (name, ids, rel) in &hikes {
        let mut deg: HashMap<i64, u32> = HashMap::new();
        for id in ids {
            if let Ok(i) = hike_ends.binary_search_by_key(id, |e| e.0) {
                *deg.entry(hike_ends[i].1).or_default() += 1;
                *deg.entry(hike_ends[i].2).or_default() += 1;
            }
        }
        let ends: Vec<i64> = deg.iter().filter(|(_, &d)| d == 1).map(|(&n, _)| n).collect();
        if ends.len() == 2 {
            route_ends.extend(ends.into_iter().map(|n| (name.clone(), n, *rel)));
        }
    }
    // The pass's ends, and where they are.
    let mut end_pos: HashMap<i64, (i32, i32)> = HashMap::new();
    if let Some(es) = &given_ends {
        for e in es {
            route_ends.push((e.name.clone(), e.node, e.rel));
            end_pos.insert(e.node, (e.lon, e.lat));
        }
        eprintln!("        {} hiking-route ends given", route_ends.len());
    } else {
        eprintln!("        {} hiking routes, {} route ends", hikes.len(), route_ends.len());
    }

    let mut needed: Vec<i64> = ways.iter().flat_map(|w| w.refs.iter().copied()).collect();
    needed.extend(poi_ways.iter().flat_map(|p| p.2.iter().copied()));
    if given_ends.is_none() {
        needed.extend(route_ends.iter().map(|e| e.1));
    }
    needed.par_sort_unstable();
    needed.dedup();
    eprintln!("        {} unique nodes needed", needed.len());

    // ---- Pass 2: node coordinates + private gates --------------------------------
    let coords: Vec<AtomicU64> = (0..needed.len()).map(|_| AtomicU64::new(u64::MAX)).collect();
    let gates: Vec<AtomicU8> = (0..needed.len()).map(|_| AtomicU8::new(0)).collect();
    // An empty range (lo > hi) when nothing is needed: a piece of open sea.
    let (lo, hi) = (needed.first().copied().unwrap_or(1), needed.last().copied().unwrap_or(0));
    let record = |id: i64, lat: i32, lon: i32, tags: &mut dyn Iterator<Item = (&str, &str)>| -> Vec<Poi> {
        let t = Tags(tags.collect());
        let mut pois = Vec::new();
        if !t.0.is_empty() {
            if let Some(kind) = poi_kind(&t, candidates) {
                pois.push(Poi {
                    kind,
                    lon,
                    lat,
                    name: t.get("name").unwrap_or("").to_string(),
                    ele: pipeline::candidates::parse_ele(t.get("ele")),
                    osm: Some(format!("n{id}")),
                    key: None,
                    tags: kept_tags(&t, kind),
                    nodes: Vec::new(),
                    length_m: None,
                });
            }
        }
        if id < lo || id > hi {
            return pois;
        }
        if let Ok(i) = needed.binary_search(&id) {
            coords[i].store(((lat as u32 as u64) << 32) | lon as u32 as u64, Relaxed);
            if !t.0.is_empty()
                && matches!(
                    t.get("barrier"),
                    Some("gate" | "lift_gate" | "swing_gate" | "sliding_gate" | "chain" | "hampshire_gate")
                )
                && (t.is("locked", "yes") || car_denied(&t))
            {
                gates[i].store(1, Relaxed);
            }
        }
        pois
    };
    let mut pois: Vec<Poi> = Vec::new();
    for p in &inputs {
        let name = p.file_name().unwrap().to_string_lossy().replace(".osm.pbf", "");
        let r = open_reader(p, &format!("nodes · {name}"))?;
        let mut got = r.par_map_reduce(
            |el| match el {
                Element::DenseNode(n) => record(n.id(), n.decimicro_lat(), n.decimicro_lon(), &mut n.tags()),
                Element::Node(n) => record(n.id(), n.decimicro_lat(), n.decimicro_lon(), &mut n.tags()),
                _ => Vec::new(),
            },
            Vec::new,
            |mut a, mut b| {
                a.append(&mut b);
                a
            },
        )?;
        pois.append(&mut got);
    }
    eprintln!("pass 2 done ({:.0?})", t0.elapsed());
    for (kind, name, refs, id, tags) in &poi_ways {
        let pts: Vec<(i64, i64)> = refs
            .iter()
            .filter_map(|&r| {
                let i = needed.binary_search(&r).ok()?;
                let v = coords[i].load(Relaxed);
                (v != u64::MAX).then(|| ((v as u32 as i32) as i64, ((v >> 32) as u32 as i32) as i64))
            })
            .collect();
        if pts.is_empty() {
            continue;
        }
        let n = pts.len() as i64;
        let (sx, sy) = pts.iter().fold((0i64, 0i64), |a, p| (a.0 + p.0, a.1 + p.1));
        let nodes: Vec<[i32; 2]> = pts.iter().map(|p| [p.0 as i32, p.1 as i32]).collect();
        pois.push(Poi { kind, lon: (sx / n) as i32, lat: (sy / n) as i32, name: name.clone(), ele: None, osm: Some(format!("w{id}")), key: None, tags: tags.clone(), nodes, length_m: None });
    }

    // Hiking-route ends within 300 m of a drivable road are trailheads (named after the route).
    {
        let cell = |lon: i32, lat: i32| ((lon as i64).div_euclid(30_000), (lat as i64).div_euclid(30_000)); // ≈ 0.003°
        let mut cand: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let mut pts: Vec<(i32, i32)> = Vec::new();
        for (k, (_, n, _)) in route_ends.iter().enumerate() {
            let p = match end_pos.get(n) {
                Some(&p) => p,
                None => {
                    let v = needed.binary_search(n).ok().map(|i| coords[i].load(Relaxed)).unwrap_or(u64::MAX);
                    if v == u64::MAX { (i32::MIN, 0) } else { (v as u32 as i32, (v >> 32) as u32 as i32) }
                }
            };
            pts.push(p);
            if p.0 != i32::MIN {
                cand.entry(cell(p.0, p.1)).or_default().push(k);
            }
        }
        let mut near = vec![false; route_ends.len()];
        for w in ways.iter().filter(|w| !class::is_rail(w.class)) {
            for r in &w.refs {
                let Ok(i) = needed.binary_search(r) else { continue };
                let v = coords[i].load(Relaxed);
                if v == u64::MAX {
                    continue;
                }
                let (lon, lat) = (v as u32 as i32, (v >> 32) as u32 as i32);
                let (cx, cy) = cell(lon, lat);
                let kx = lon_cells(lat, 300.0, 30_000);
                for dx in -kx..=kx {
                    for dy in -1..=1 {
                        for &k in cand.get(&(cx + dx, cy + dy)).map(|v| v.as_slice()).unwrap_or(&[]) {
                            if !near[k] && dist_m(lon as f64 * E7, lat as f64 * E7, pts[k].0 as f64 * E7, pts[k].1 as f64 * E7) < 300.0 {
                                near[k] = true;
                            }
                        }
                    }
                }
            }
        }
        let mut n = 0;
        for (k, (name, node, rel)) in route_ends.iter().enumerate() {
            if near[k] {
                let tags = if name.is_empty() { Vec::new() } else { vec![("name".to_string(), name.clone())] };
                pois.push(Poi { kind: "trail_route", lon: pts[k].0, lat: pts[k].1, name: name.clone(), ele: None, osm: None, key: Some(format!("trail:{rel}:{node}")), tags, nodes: Vec::new(), length_m: None });
                n += 1;
            }
        }
        eprintln!("        {n} hiking-route ends near roads");
    }
    // One trailhead per spot: mapped trailheads first, then trail car parks, then route ends (each
    // rank in the points' one order); any within 150 m of one already kept is dropped (lending it
    // its name if it has none).
    pois.sort_by(|a, b| a.order().cmp(&b.order()));
    {
        let rank = |k: &str| match k {
            "trailhead" => 0,
            "trail_parking" => 1,
            "trail_route" => 2,
            _ => 3,
        };
        let mut order: Vec<usize> = (0..pois.len()).filter(|&i| rank(pois[i].kind) < 3).collect();
        order.sort_by_key(|&i| rank(pois[i].kind));
        let mut drop = vec![false; pois.len()];
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let cell = |p: &Poi| ((p.lon as i64).div_euclid(20_000), (p.lat as i64).div_euclid(20_000)); // ≈ 0.002°
        for i in order {
            let (cx, cy) = cell(&pois[i]);
            let near = |j: usize| dist_m(pois[i].lon as f64 * E7, pois[i].lat as f64 * E7, pois[j].lon as f64 * E7, pois[j].lat as f64 * E7) < 150.0;
            let kx = lon_cells(pois[i].lat, 150.0, 20_000);
            let dup = (-kx..=kx)
                .flat_map(|dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)))
                .flat_map(|c| grid.get(&c).cloned().unwrap_or_default())
                .find(|&j| near(j));
            match dup {
                Some(j) => {
                    if pois[j].name.is_empty() && !pois[i].name.is_empty() {
                        pois[j].name = pois[i].name.clone();
                    }
                    drop[i] = true;
                }
                None => grid.entry((cx, cy)).or_default().push(i),
            }
        }
        let mut i = 0;
        pois.retain(|_| {
            i += 1;
            !drop[i - 1]
        });
        for p in &mut pois {
            if rank(p.kind) < 3 {
                p.kind = "trailhead";
            }
        }
    }

    // ---- Assemble, drop gated minor roads, densify --------------------------------
    // North America (west of 40° W) and Japan (GSI) have 1–10 m lidar and DEMs; elsewhere the DEMs
    // are 20–30 m (Taiwan's MOI DTM, FABDEM). (Versioned as "spacing" in pipeline::rules; the
    // units' reach densifies long ways the same way, pipeline::reach::LongWay::touches.)
    const COARSE_SPACING_M: f64 = 15.0;
    let spacing_at = |p: [i32; 2]| {
        let (lon, lat) = (p[0] as f64 * E7, p[1] as f64 * E7);
        let japan = (122.5..154.0).contains(&lon) && (20.0..46.5).contains(&lat);
        if lon < -40.0 || japan { spacing } else { spacing.max(COARSE_SPACING_M) }
    };
    let lookup = |id: i64| -> Option<(usize, i32, i32)> {
        let i = needed.binary_search(&id).ok()?;
        let v = coords[i].load(Relaxed);
        (v != u64::MAX).then(|| (i, (v >> 32) as u32 as i32, v as u32 as i32))
    };
    let mut gated = 0usize;
    let mut missing = 0usize;
    struct Built {
        w: usize,
        pts: Vec<[i32; 2]>,
        key: u64,
    }
    // Err(true) = dropped for a private gate, Err(false) = unresolved geometry.
    let results: Vec<Result<Built, bool>> = ways
        .par_iter()
        .enumerate()
        .map(|(wi, w)| {
            let mut pts: Vec<[i32; 2]> = Vec::with_capacity(w.refs.len());
            let n = w.refs.len();
            let mut has_gate = false;
            for (k, &r) in w.refs.iter().enumerate() {
                if let Some((i, lat, lon)) = lookup(r) {
                    if k > 0 && k + 1 < n && gates[i].load(Relaxed) != 0 {
                        has_gate = true;
                    }
                    pts.push([lon, lat]);
                }
            }
            if has_gate && w.class <= class::UNCLASSIFIED {
                return Err(true);
            }
            pts.dedup();
            if pts.len() < 2 {
                return Err(false);
            }
            let (mx, my) = roadcore::merc(pts[0][0] as f64 * E7, pts[0][1] as f64 * E7);
            let key = morton((mx * 4294967295.0) as u32, (my * 4294967295.0) as u32);
            Ok(Built { w: wi, pts, key })
        })
        .collect();
    for r in &results {
        match r {
            Err(true) => gated += 1,
            Err(false) => missing += 1,
            Ok(_) => {}
        }
    }
    let mut built: Vec<Built> = results.into_iter().filter_map(Result::ok).collect();
    // (key, way id): deterministic order, so per-vertex files stay aligned across runs.
    built.par_sort_unstable_by_key(|b| (b.key, ways[b.w].id));
    eprintln!("assembled {} ways (dropped {gated} gated, {missing} unresolved)", built.len());

    // String table: index 0 = "".
    let mut strings: Vec<String> = vec![String::new()];
    let mut sidx: HashMap<String, u32> = HashMap::new();
    sidx.insert(String::new(), 0);
    let mut intern = |s: &str| -> u32 {
        if let Some(&i) = sidx.get(s) {
            return i;
        }
        let i = strings.len() as u32;
        strings.push(s.to_owned());
        sidx.insert(s.to_owned(), i);
        i
    };

    let mut wv = BufWriter::with_capacity(16 << 20, File::create(roadcore::tmp(&out, "verts.bin"))?);
    let mut recs: Vec<WayRec> = Vec::with_capacity(built.len());
    let mut vtotal: u64 = 0;
    let mut orig_total: u64 = 0;
    let mut len_by_class = [0f64; class::COUNT];
    // Roads' own English, by way id, where it isn't their name (name-en.json).
    let mut name_en: std::collections::BTreeMap<String, String> = Default::default();
    for b in &built {
        let w = &ways[b.w];
        if !w.name_en.is_empty() && w.name_en != w.name {
            name_en.insert(w.id.to_string(), w.name_en.clone());
        }
        let start = vtotal;
        let mut emit = |p: [i32; 2], wv: &mut BufWriter<File>| -> Result<()> {
            wv.write_all(bytemuck::cast_slice(&p))?;
            vtotal += 1;
            Ok(())
        };
        emit(b.pts[0], &mut wv)?;
        let spacing = spacing_at(b.pts[0]);
        for s in b.pts.windows(2) {
            let (a, c) = (s[0], s[1]);
            let (ax, ay, cx, cy) = (a[0] as f64 * E7, a[1] as f64 * E7, c[0] as f64 * E7, c[1] as f64 * E7);
            let d = dist_m(ax, ay, cx, cy);
            len_by_class[w.class as usize] += d;
            if w.class != class::FERRY && d > spacing {
                let k = (d / spacing).ceil() as i64;
                for j in 1..k {
                    let t = j as f64 / k as f64;
                    let p = [
                        (a[0] as f64 + (c[0] - a[0]) as f64 * t).round() as i32,
                        (a[1] as f64 + (c[1] - a[1]) as f64 * t).round() as i32,
                    ];
                    emit(p, &mut wv)?;
                }
            }
            emit(c, &mut wv)?;
        }
        orig_total += b.pts.len() as u64;
        recs.push(WayRec {
            id: w.id,
            vstart: start,
            vcount: (vtotal - start) as u32,
            name: intern(&w.name),
            ref_: intern(&w.ref_),
            surface: intern(&w.surface),
            maxspeed: w.maxspeed,
            class: w.class,
            flags: w.flags,
            lanes: w.lanes,
            network: if class::is_rail(w.class) { 0 } else { network_code(b.pts[0][0] as f64 * E7, b.pts[0][1] as f64 * E7, &w.ref_, w.class) },
            rail: w.rail,
            _pad: 0,
            route: intern(&w.route),
            colour: w.colour,
        });
        if w.flags & flag::COVERED != 0 {
            let m = b.pts[b.pts.len() / 2];
            let tags = bridge_tags.binary_search_by_key(&w.id, |b| b.0).map(|i| bridge_tags[i].1.clone()).unwrap_or_default();
            // Its own nodes (the points are densified): what its length is measured on, for a line
            // (not a closed way) of some length.
            let nodes: Vec<[i32; 2]> = w.refs.iter().filter_map(|&r| lookup(r).map(|(_, lat, lon)| [lon, lat])).collect();
            let line = w.refs.first() != w.refs.last() && nodes.windows(2).any(|p| p[0] != p[1]);
            let length_m = line.then(|| line_m(&nodes));
            pois.push(Poi { kind: "covered_bridge", lon: m[0], lat: m[1], name: w.name.clone(), ele: None, osm: Some(format!("w{}", w.id)), key: None, tags, nodes, length_m });
        }
    }
    wv.flush()?;

    let mut ww = BufWriter::new(File::create(roadcore::tmp(&out, "ways.bin"))?);
    ww.write_all(WAYS_MAGIC)?;
    ww.write_all(&(recs.len() as u64).to_le_bytes())?;
    ww.write_all(bytemuck::cast_slice(&recs))?;
    ww.flush()?;
    std::fs::write(roadcore::tmp(&out, "strings.txt"), strings.join("\n"))?;
    std::fs::write(roadcore::tmp(&out, "name-en.json"), serde_json::to_vec(&name_en)?)?;
    // Points of interest for the map and the scenic metrics, in their one order.
    pois.sort_by(|a, b| a.order().cmp(&b.order()));
    let features: Vec<serde_json::Value> = pois
        .iter()
        .map(|p| {
            serde_json::json!({
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [p.lon as f64 * E7, p.lat as f64 * E7] },
                "properties": {
                    "kind": p.kind, "name": p.name, "ele": p.ele.map(|e| e.round()), "osm": p.osm, "key": p.key,
                    "tags": p.tags.iter().map(|(k, v)| (k.clone(), serde_json::Value::from(v.as_str()))).collect::<serde_json::Map<_, _>>(),
                    "nodes": (!p.nodes.is_empty()).then_some(&p.nodes), "length_m": p.length_m,
                },
            })
        })
        .collect();
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for p in &pois {
        *counts.entry(p.kind).or_default() += 1;
    }
    eprintln!("POIs: {counts:?}");
    std::fs::write(roadcore::tmp(&out, "pois.json"), serde_json::to_vec(&serde_json::json!({ "type": "FeatureCollection", "features": features }))?)?;
    roadcore::commit(&out, &["verts.bin", "ways.bin", "strings.txt", "name-en.json", "pois.json", "rail-rels.bin"])?;

    eprintln!("\nwrote {} ways, {} vertices ({} original nodes), {} strings", recs.len(), vtotal, orig_total, strings.len());
    let mut total = 0.0;
    for (c, l) in len_by_class.iter().enumerate() {
        eprintln!("  {:>14}: {:>10.0} km", class::NAMES[c], l / 1000.0);
        total += l;
    }
    eprintln!("  {:>14}: {:>10.0} km   ({:.0?})", "total", total / 1000.0, t0.elapsed());
    Ok(())
}
