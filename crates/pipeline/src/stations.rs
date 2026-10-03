//! Rail stops for the map (today's dem/stations.py, docs/phase5.md "Stations"): every stop of a
//! passenger route relation, with the stop spacing of the lines calling there (how far apart their
//! stops are on average), which sets when a stop's dot appears and how big it is.
//!
//! A route's stops are its members with a stop role (stop, stop_entry_only, stop_exit_only), else
//! its platforms, in order; its spacing is the distance between consecutive stops, averaged (at
//! most 200 km). The members of one stop (stop positions, platforms, the station) and the same stop
//! on several routes are merged by name within 800 m (unnamed ones within 100 m), routes in id
//! order; a stop takes the largest spacing of the lines calling there and that line's service
//! group. Read from the OSM pass's `rail` set, worldwide.

use anyhow::{Context, Result};
use osmpbf::{Element, ElementReader};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const TRAM: u8 = 0;
pub const METRO: u8 = 1;
pub const COMMUTER: u8 = 2;
pub const INTERCITY: u8 = 3;
pub const HERITAGE: u8 = 4;

const INTERCITY_WORDS: &[&str] = &[
    "tgv", "inoui", "ouigo", "intercités", "intercites", "eurostar", "thalys", "lyria", "ave ", "alvia", "euromed",
    "iryo", "avlo", "talgo", "alfa pendular", "intercidades", "amtrak", "via rail", "acela", "lner", "avanti",
    "crosscountry", "cross country", "sleeper", "night riviera", "nightjet", "intercity", "inter city", "inter-city",
    "enterprise", "adirondack", "maple leaf", "vermonter", "ethan allen", "downeaster", "lake shore", "the canadian",
    "the ocean", "northeast regional", "hull chelsea", "transcantábrico", "costa verde express", "al andalus",
    "shinkansen", "新幹線", "特急", "limited express", "高鐵", "自強", "thsr",
];

/// A route relation's service group (extract.rs rail_route; stations.py group).
pub fn group(t: &HashMap<String, String>) -> Option<u8> {
    let get = |k: &str| t.get(k).map(String::as_str);
    let tourist = matches!(get("service"), Some("tourism" | "heritage"))
        || get("tourism") == Some("yes")
        || get("historic") == Some("yes")
        || get("railway:preserved") == Some("yes")
        || get("heritage:railway") == Some("yes");
    match get("route")? {
        "tram" => Some(if tourist { HERITAGE } else { TRAM }),
        "subway" | "light_rail" | "monorail" => Some(METRO),
        "funicular" => Some(HERITAGE),
        "train" => {
            if tourist {
                return Some(HERITAGE);
            }
            match get("service") {
                Some("long_distance" | "high_speed" | "night" | "international" | "car_shuttle") => Some(INTERCITY),
                Some(_) => Some(COMMUTER),
                None => {
                    let text = ["name", "brand", "network", "operator"].iter().map(|k| get(k).unwrap_or("")).collect::<Vec<_>>().join(" ").to_lowercase();
                    Some(if INTERCITY_WORDS.iter().any(|w| text.contains(w)) { INTERCITY } else { COMMUTER })
                }
            }
        }
        _ => None,
    }
}

/// Great-circle distance (m), as stations.py's.
pub fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.1.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((b.0 - a.0).to_radians() / 2.0).sin().powi(2);
    12_742_000.0 * h.sqrt().min(1.0).asin()
}

/// A name for merging: lowercase, without words like "station" and punctuation.
pub fn norm(name: &str) -> String {
    const WORDS: &[&str] = &["station", "gare", "estación", "estação", "halt", "stop", "platform", "quai", "voie", "bahnhof", "駅"];
    let lower = name.to_lowercase();
    // Words as \b…\b in Python's re (Unicode word characters).
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let chars: Vec<char> = lower.chars().collect();
    let mut out = String::with_capacity(lower.len());
    let mut i = 0;
    while i < chars.len() {
        let at_start = i == 0 || !is_word(chars[i - 1]);
        let mut matched = 0;
        if at_start {
            for w in WORDS {
                let wc: Vec<char> = w.chars().collect();
                if chars[i..].starts_with(&wc) {
                    let end = i + wc.len();
                    if end == chars.len() || !is_word(chars[end]) {
                        matched = wc.len();
                        break;
                    }
                }
            }
        }
        if matched > 0 {
            out.push(' ');
            i += matched;
            continue;
        }
        let c = chars[i];
        out.push(if "()（）.,'’-".contains(c) { ' ' } else { c });
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A stop as the map shows it.
#[derive(Clone, Debug)]
pub struct Stop {
    pub lon: f64,
    pub lat: f64,
    pub name: String,
    /// OSM's English name of the object that named it, where it differs.
    pub en: Option<String>,
    pub group: u8,
    pub mask: u8,
    pub spacing: f64,
    /// The OSM objects merged into it (type, id), for its id: the lowest (docs/phase5.md "Ids").
    pub members: Vec<(u8, i64)>,
}

impl Stop {
    /// The zoom from which its spacing spans one pixel (512 px tiles).
    pub fn mz(&self) -> f64 {
        (40_075_016.7 * self.lat.to_radians().cos() / (512.0 * self.spacing)).log2()
    }
}

struct Obj {
    c: (f64, f64),
    name: String,
    en: Option<String>,
    station: bool,
}

struct Building {
    c: (f64, f64),
    station: bool,
    names: Vec<(String, u32, Option<String>)>,
    sp: f64,
    g: Option<u8>,
    m: u8,
    members: Vec<(u8, i64)>,
}

/// The stops of the routes in an OSM file (the pass's `rail` set): read in three passes (routes,
/// their member ways, the nodes needed), then merged in routes' id order.
pub fn stops(pbf: &Path) -> Result<Vec<Stop>> {
    // Pass 1: the passenger routes, by id.
    let mut routes: Vec<(i64, HashMap<String, String>, Vec<(u8, i64, String)>)> = ElementReader::from_path(pbf)
        .with_context(|| format!("open {}", pbf.display()))?
        .par_map_reduce(
            |el| match el {
                Element::Relation(r) => {
                    let t: HashMap<String, String> = r.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                    if !matches!(t.get("route").map(String::as_str), Some("train" | "subway" | "tram" | "light_rail" | "monorail" | "funicular")) {
                        return Vec::new();
                    }
                    let ms = r
                        .members()
                        .filter_map(|m| {
                            let ty = match m.member_type {
                                osmpbf::RelMemberType::Node => 0u8,
                                osmpbf::RelMemberType::Way => 1,
                                osmpbf::RelMemberType::Relation => 2,
                            };
                            Some((ty, m.member_id, m.role().ok()?.to_string()))
                        })
                        .collect();
                    vec![(r.id(), t, ms)]
                }
                _ => Vec::new(),
            },
            Vec::new,
            |mut a, mut b| {
                a.append(&mut b);
                a
            },
        )?;
    routes.sort_by_key(|r| r.0);
    let wanted = |role: &str| role.starts_with("stop") || role.starts_with("platform");
    let member_ways: HashSet<i64> = routes.iter().flat_map(|r| r.2.iter()).filter(|m| m.0 == 1 && wanted(&m.2)).map(|m| m.1).collect();
    let member_nodes: HashSet<i64> = routes.iter().flat_map(|r| r.2.iter()).filter(|m| m.0 == 0 && wanted(&m.2)).map(|m| m.1).collect();
    // A stop object: tagged public_transport=*, or railway=station, halt, stop, tram_stop, platform.
    let stop_tags = |t: &HashMap<&str, &str>| t.contains_key("public_transport") || matches!(t.get("railway"), Some(&("station" | "halt" | "stop" | "tram_stop" | "platform")));
    let obj_of = |t: &HashMap<&str, &str>, c: (f64, f64)| {
        let name = t.get("name").copied().unwrap_or("").to_string();
        let en = t.get("name:en").filter(|e| !e.is_empty() && **e != name).map(|e| e.to_string());
        let station = matches!(t.get("railway"), Some(&("station" | "halt"))) || t.get("public_transport") == Some(&"station");
        Obj { c, name, en, station }
    };
    // Pass 2: the member ways that are stop objects, and their nodes.
    let ways: Vec<(i64, Vec<i64>, HashMap<String, String>)> = ElementReader::from_path(pbf)?.par_map_reduce(
        |el| match el {
            Element::Way(w) if member_ways.contains(&w.id()) => {
                let t: HashMap<&str, &str> = w.tags().collect();
                if !stop_tags(&t) {
                    return Vec::new();
                }
                vec![(w.id(), w.refs().collect(), t.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())]
            }
            _ => Vec::new(),
        },
        Vec::new,
        |mut a, mut b| {
            a.append(&mut b);
            a
        },
    )?;
    let way_nodes: HashSet<i64> = ways.iter().flat_map(|w| w.1.iter().copied()).collect();
    // Pass 3: the nodes: stop objects among the members, and the stop ways' vertices.
    let nodes: Vec<(i64, (f64, f64), Option<HashMap<String, String>>)> = ElementReader::from_path(pbf)?.par_map_reduce(
        |el| {
            let (id, c, tags): (i64, (f64, f64), Vec<(&str, &str)>) = match el {
                Element::Node(n) => (n.id(), (n.lon(), n.lat()), n.tags().collect()),
                Element::DenseNode(n) => (n.id(), (n.lon(), n.lat()), n.tags().collect()),
                _ => return Vec::new(),
            };
            let member = member_nodes.contains(&id);
            if !member && !way_nodes.contains(&id) {
                return Vec::new();
            }
            let t: HashMap<&str, &str> = tags.into_iter().collect();
            let keep = member && stop_tags(&t);
            vec![(id, c, keep.then(|| t.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()))]
        },
        Vec::new,
        |mut a, mut b| {
            a.append(&mut b);
            a
        },
    )?;
    let coords: HashMap<i64, (f64, f64)> = nodes.iter().map(|n| (n.0, n.1)).collect();
    let mut objs: HashMap<(u8, i64), Obj> = HashMap::new();
    for (id, c, t) in &nodes {
        if let Some(t) = t {
            let t: HashMap<&str, &str> = t.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            objs.insert((0, *id), obj_of(&t, *c));
        }
    }
    for (id, refs, t) in &ways {
        // osmium export's geometry: a line's (or a closed way's ring's) coordinates; the mean of them.
        let cs: Vec<(f64, f64)> = refs.iter().filter_map(|r| coords.get(r).copied()).collect();
        if cs.is_empty() || cs.len() < refs.len() {
            continue;
        }
        let c = (cs.iter().map(|p| p.0).sum::<f64>() / cs.len() as f64, cs.iter().map(|p| p.1).sum::<f64>() / cs.len() as f64);
        let t: HashMap<&str, &str> = t.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        objs.insert((1, *id), obj_of(&t, c));
    }
    // Merge, routes in id order.
    let mut stops: Vec<Building> = Vec::new();
    let mut cells: HashMap<(String, bool, i64, i64), Vec<usize>> = HashMap::new();
    let mut stop_of = |key: (u8, i64), stops: &mut Vec<Building>| -> Option<usize> {
        let o = objs.get(&key)?;
        let (x, y) = o.c;
        let nm = norm(&o.name);
        let named = !nm.is_empty();
        let (r, k) = if named { (800.0, 0.02) } else { (100.0, 0.002) };
        let (cx, cy) = ((x / k) as i64, (y / k) as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ids) = cells.get(&(nm.clone(), named, cx + dx, cy + dy)) {
                    for &i in ids {
                        let s = &mut stops[i];
                        if dist(s.c, (x, y)) <= r {
                            if o.station && !s.station {
                                (s.c, s.station) = ((x, y), true);
                            }
                            match s.names.iter_mut().find(|n| n.0 == o.name) {
                                Some(n) => n.1 += 1,
                                None => s.names.push((o.name.clone(), 1, o.en.clone())),
                            }
                            s.members.push(key);
                            return Some(i);
                        }
                    }
                }
            }
        }
        stops.push(Building { c: (x, y), station: o.station, names: vec![(o.name.clone(), 1, o.en.clone())], sp: 0.0, g: None, m: 0, members: vec![key] });
        cells.entry((nm, named, cx, cy)).or_default().push(stops.len() - 1);
        Some(stops.len() - 1)
    };
    for (_, tags, members) in &routes {
        let Some(g) = group(tags) else { continue };
        let mut sm: Vec<(u8, i64)> = members.iter().filter(|m| m.0 != 2 && m.2.starts_with("stop")).map(|m| (m.0, m.1)).collect();
        if sm.len() < 2 {
            sm = members.iter().filter(|m| m.0 != 2 && m.2.starts_with("platform")).map(|m| (m.0, m.1)).collect();
        }
        let mut seq: Vec<usize> = Vec::new();
        for k in sm {
            if let Some(i) = stop_of(k, &mut stops) {
                if seq.last() != Some(&i) {
                    seq.push(i);
                }
            }
        }
        let distinct: HashSet<usize> = seq.iter().copied().collect();
        if distinct.len() < 2 {
            continue;
        }
        let length: f64 = seq.windows(2).map(|w| dist(stops[w[0]].c, stops[w[1]].c)).sum();
        let sp = (length / (seq.len() - 1) as f64).min(200_000.0);
        for i in distinct {
            let s = &mut stops[i];
            s.m |= 1 << g;
            if sp > s.sp {
                (s.sp, s.g) = (sp, Some(g));
            }
        }
    }
    let mut out: Vec<Stop> = stops
        .into_iter()
        .filter(|s| s.g.is_some() && s.sp >= 50.0)
        .map(|s| {
            // Its name: the most common (a named one first; the first of equals).
            let best = s.names.iter().fold(None::<&(String, u32, Option<String>)>, |b, n| match b {
                Some(b) if (!b.0.is_empty(), b.1) >= (!n.0.is_empty(), n.1) => Some(b),
                _ => Some(n),
            });
            let (name, _, en) = best.cloned().unwrap();
            let mut members = s.members.clone();
            members.sort();
            members.dedup();
            Stop { lon: s.c.0, lat: s.c.1, name, en, group: s.g.unwrap(), mask: s.m, spacing: s.sp, members }
        })
        .collect();
    // Widest spacing first (stable: equals as made).
    out.sort_by(|a, b| b.spacing.total_cmp(&a.spacing));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_for_merging() {
        assert_eq!(norm("London St Pancras International"), "london st pancras international");
        assert_eq!(norm("Gare de Lyon"), "de lyon");
        assert_eq!(norm("Bahnhof Zoo (S-Bahn)"), "zoo s bahn");
        assert_eq!(norm("Stopford"), "stopford");
        // (No word boundary between two CJK characters, as in Python's re.)
        assert_eq!(norm("東京駅"), "東京駅");
        assert_eq!(norm("駅"), "");
    }

    #[test]
    fn groups() {
        let t = |kv: &[(&str, &str)]| kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        assert_eq!(group(&t(&[("route", "train"), ("name", "TGV Paris - Lyon")])), Some(INTERCITY));
        assert_eq!(group(&t(&[("route", "train"), ("service", "regional")])), Some(COMMUTER));
        assert_eq!(group(&t(&[("route", "tram"), ("tourism", "yes")])), Some(HERITAGE));
        assert_eq!(group(&t(&[("route", "bus")])), None);
    }
}
