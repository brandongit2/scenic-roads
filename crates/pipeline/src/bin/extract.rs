//! Extract car-accessible public roads (and car ferries) from OSM PBF extracts.
//!
//! usage: extract <out_dir> <spacing_m> <file.osm.pbf>...
//!
//! Pass 1 collects matching ways; pass 2 resolves node coordinates and private gates.
//! Output geometry is densified so consecutive vertices are at most `spacing_m` apart,
//! which lets the DEM stage sample every raster cell a road crosses.

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
    ref_: String,
    surface: String,
    refs: Vec<i64>,
}

#[derive(Default)]
struct Pass1 {
    ways: Vec<RawWay>,
    /// (member way id, route name) of designated scenic routes.
    scenic: Vec<(i64, String)>,
    /// Points of interest mapped as areas: (kind, name, node refs).
    poi_ways: Vec<(&'static str, String, Vec<i64>)>,
}

impl Pass1 {
    fn merge(mut a: Pass1, mut b: Pass1) -> Pass1 {
        if a.ways.len() < b.ways.len() {
            std::mem::swap(&mut a, &mut b);
        }
        a.ways.append(&mut b.ways);
        a.scenic.append(&mut b.scenic);
        a.poi_ways.append(&mut b.poi_ways);
        a
    }
}

struct Poi {
    kind: &'static str,
    lon: i32,
    lat: i32,
    name: String,
    ele: Option<f32>,
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

fn poi_kind(t: &Tags) -> Option<&'static str> {
    if t.is("highway", "rest_area") {
        Some("rest_area")
    } else if t.is("tourism", "picnic_site") {
        Some("picnic_site")
    } else if t.is("highway", "trailhead") {
        Some("trailhead")
    } else if t.is("tourism", "viewpoint") {
        Some("viewpoint")
    } else if t.is("natural", "peak") {
        Some("peak")
    } else if t.is("waterway", "waterfall") {
        Some("waterfall")
    } else if t.is("man_made", "lighthouse") {
        Some("lighthouse")
    } else {
        None
    }
}

fn parse_ele(v: Option<&str>) -> Option<f32> {
    let v = v?.trim();
    let num: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    let n: f32 = num.parse().ok()?;
    Some(if v.contains("ft") || v.contains('\'') { n * 0.3048 } else { n })
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
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: extract <out_dir> <spacing_m> <file.osm.pbf>...");
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
    let mut poi_ways: Vec<(&'static str, String, Vec<i64>)> = Vec::new();
    for p in &inputs {
        let name = p.file_name().unwrap().to_string_lossy().replace(".osm.pbf", "");
        let r = open_reader(p, &format!("ways · {name}"))?;
        let mut got = r.par_map_reduce(
            |el| {
                let mut out = Pass1::default();
                match el {
                    Element::Way(w) => {
                        let t = Tags(w.tags().collect());
                        if let Some(kind) = poi_kind(&t) {
                            out.poi_ways.push((kind, t.get("name").unwrap_or("").to_string(), w.refs().collect()));
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
                                ref_: t.get("ref").unwrap_or("").replace('\n', " "),
                                surface: t.get("surface").unwrap_or("").replace('\n', " "),
                                refs: w.refs().collect(),
                            });
                        }
                    }
                    Element::Relation(r) => {
                        let t = Tags(r.tags().collect());
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
    }
    ways.par_sort_unstable_by_key(|w| w.id);
    ways.dedup_by_key(|w| w.id);
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

    let mut needed: Vec<i64> = ways.iter().flat_map(|w| w.refs.iter().copied()).collect();
    needed.extend(poi_ways.iter().flat_map(|p| p.2.iter().copied()));
    needed.par_sort_unstable();
    needed.dedup();
    eprintln!("        {} unique nodes needed", needed.len());

    // ---- Pass 2: node coordinates + private gates --------------------------------
    let coords: Vec<AtomicU64> = (0..needed.len()).map(|_| AtomicU64::new(u64::MAX)).collect();
    let gates: Vec<AtomicU8> = (0..needed.len()).map(|_| AtomicU8::new(0)).collect();
    let (lo, hi) = (needed[0], needed[needed.len() - 1]);
    let record = |id: i64, lat: i32, lon: i32, tags: &mut dyn Iterator<Item = (&str, &str)>| -> Vec<Poi> {
        let t = Tags(tags.collect());
        let mut pois = Vec::new();
        if !t.0.is_empty() {
            if let Some(kind) = poi_kind(&t) {
                pois.push(Poi {
                    kind,
                    lon,
                    lat,
                    name: t.get("name").unwrap_or("").to_string(),
                    ele: parse_ele(t.get("ele")),
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
    for (kind, name, refs) in &poi_ways {
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
        pois.push(Poi { kind, lon: (sx / n) as i32, lat: (sy / n) as i32, name: name.clone(), ele: None });
    }

    // ---- Assemble, drop gated minor roads, densify --------------------------------
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
    for b in &built {
        let w = &ways[b.w];
        let start = vtotal;
        let mut emit = |p: [i32; 2], wv: &mut BufWriter<File>| -> Result<()> {
            wv.write_all(bytemuck::cast_slice(&p))?;
            vtotal += 1;
            Ok(())
        };
        emit(b.pts[0], &mut wv)?;
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
            _pad: [0; 3],
            route: intern(&w.route),
            _pad2: 0,
        });
        if w.flags & flag::COVERED != 0 {
            let m = b.pts[b.pts.len() / 2];
            pois.push(Poi { kind: "covered_bridge", lon: m[0], lat: m[1], name: w.name.clone(), ele: None });
        }
    }
    wv.flush()?;

    let mut ww = BufWriter::new(File::create(roadcore::tmp(&out, "ways.bin"))?);
    ww.write_all(WAYS_MAGIC)?;
    ww.write_all(&(recs.len() as u64).to_le_bytes())?;
    ww.write_all(bytemuck::cast_slice(&recs))?;
    ww.flush()?;
    std::fs::write(roadcore::tmp(&out, "strings.txt"), strings.join("\n"))?;
    // Points of interest for the map and the scenic metrics.
    let features: Vec<serde_json::Value> = pois
        .iter()
        .map(|p| {
            serde_json::json!({
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [p.lon as f64 * E7, p.lat as f64 * E7] },
                "properties": { "kind": p.kind, "name": p.name, "ele": p.ele.map(|e| e.round()) },
            })
        })
        .collect();
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for p in &pois {
        *counts.entry(p.kind).or_default() += 1;
    }
    eprintln!("POIs: {counts:?}");
    std::fs::write(roadcore::tmp(&out, "pois.json"), serde_json::to_vec(&serde_json::json!({ "type": "FeatureCollection", "features": features }))?)?;
    roadcore::commit(&out, &["verts.bin", "ways.bin", "strings.txt", "pois.json"])?;

    eprintln!("\nwrote {} ways, {} vertices ({} original nodes), {} strings", recs.len(), vtotal, orig_total, strings.len());
    let mut total = 0.0;
    for (c, l) in len_by_class.iter().enumerate() {
        eprintln!("  {:>14}: {:>10.0} km", class::NAMES[c], l / 1000.0);
        total += l;
    }
    eprintln!("  {:>14}: {:>10.0} km   ({:.0?})", "total", total / 1000.0, t0.elapsed());
    Ok(())
}
