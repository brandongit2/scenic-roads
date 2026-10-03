//! `convert-legacy-marks` (docs/phase5.md "Today's regions", steps 1–2): today's stops and sights
//! (`global/legacy/layer-pois-<kind>`, with `details-poi`, `peaks` and `layer-summits`) and heritage
//! sites (`layer-heritage`, with `details-heritage` and `props-heritage`) as markdata per z6 tile and
//! thinned tiles per kind, so the server answers the In view statistics and the app loads points by
//! view. Fame, isolation and `mz` are kept as they are; a kind's file order is its rank (the app's
//! tie-break).

use crate::marks::{self, flag, Cell, IdSource, KeepPt, MarkPt, MarkTile, Row, SummitRec, KINDS};
use crate::out::Out;
use anyhow::{Context, Result};
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// The heritage dots the last marks wrote: site record (`i`) → [id, lon, lat] (the overlays job's).
pub const HERITAGE_DOTS: &str = "work/marks/heritage-dots";

/// The stops & sights kinds (each its own file; heritage is one more).
pub const POI_KINDS: [&str; 7] = ["viewpoint", "peak", "waterfall", "lighthouse", "covered_bridge", "rest", "trailhead"];

/// A legacy file's bytes: this Mac's mirror copy when there is one, else the NAS's.
pub(crate) fn legacy_bytes(out: &Out, stem: &str) -> Result<Vec<u8>> {
    src_bytes(out, LEGACY, stem)
}

/// Today's heritage files' folder (`global/legacy`), the converted build's.
pub const LEGACY: &str = "global/legacy";

/// Where a pass's heritage comes from: the heritage job's outputs (`work/heritage/<date>`) when
/// it has made them, else today's.
pub fn heritage_source(out: &Out, date: &str) -> String {
    let job = format!("work/heritage/{date}");
    let has = |stem: &str| out.get(&format!("{job}/{stem}")).is_some();
    if has("layer-heritage") && has("details-heritage") && has("props-heritage") {
        job
    } else {
        LEGACY.to_string()
    }
}

/// A heritage or legacy file's bytes (`<src>/<stem>`): this Mac's mirror copy when there is one,
/// else the NAS's.
pub(crate) fn src_bytes(out: &Out, src: &str, stem: &str) -> Result<Vec<u8>> {
    let logical = format!("{src}/{stem}");
    let content = out.get(&logical).with_context(|| format!("{logical} isn't in the build manifest"))?;
    let mirror = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/scenic/mirror").join(content));
    let path = mirror.filter(|p| p.exists()).unwrap_or_else(|| out.path(content));
    std::fs::read(&path).with_context(|| format!("read {}", path.display()))
}

/// A point for the marks: its kind (an index of KINDS), place, record, the filters' values, lean
/// properties, popup record, and what its id is made from (an OSM id used when no other point has
/// it, else the reference). Today's converted points and the `marks` job's alike.
pub struct Point {
    pub kind: usize,
    pub lon: f64,
    pub lat: f64,
    pub pt: MarkPt,
    pub fvals: Vec<f64>,
    pub props: Map<String, Value>,
    pub info: Option<String>,
    pub osm: Option<u64>,
    pub reference: String,
}

type Pt = Point;

/// The POI details as the server merges them (details-poi, with peaks.json's record as `peak`), by
/// the layers' `i`.
fn poi_details(out: &Out) -> Result<HashMap<u64, (Option<String>, String)>> {
    let mut peaks: HashMap<u64, Value> = HashMap::new();
    if let Value::Array(a) = serde_json::from_slice(&legacy_bytes(out, "peaks")?)? {
        for mut p in a {
            if let Some(i) = p.get("i").and_then(Value::as_u64) {
                p.as_object_mut().map(|o| o.remove("i"));
                peaks.insert(i, p);
            }
        }
    }
    let mut by: HashMap<u64, (Option<String>, String)> = HashMap::new();
    let text = legacy_bytes(out, "details-poi")?;
    for line in text.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(mut v) = serde_json::from_slice::<Value>(line) else { continue };
        let Some(i) = v.get("i").and_then(Value::as_u64) else { continue };
        let osm = v.get("osm").and_then(Value::as_str).map(str::to_string);
        if let Some(p) = peaks.remove(&i) {
            v["peak"] = p;
        }
        by.insert(i, (osm, v.to_string()));
    }
    for (i, p) in peaks {
        by.entry(i).or_insert_with(|| (None, serde_json::json!({ "i": i, "peak": p }).to_string()));
    }
    Ok(by)
}

/// The heritage records as the server merges them (details-heritage, with props-heritage's record as
/// `props`), by the layer's `i`.
fn heritage_details(out: &Out, src: &str) -> Result<HashMap<u64, Value>> {
    let mut by: HashMap<u64, Value> = HashMap::new();
    let text = src_bytes(out, src, "details-heritage")?;
    for line in text.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        if let Some(i) = v.get("i").and_then(Value::as_u64) {
            by.insert(i, v);
        }
    }
    let text = src_bytes(out, src, "props-heritage")?;
    for line in text.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(mut p) = serde_json::from_slice::<Value>(line) else { continue };
        let Some(i) = p.get("i").and_then(Value::as_u64) else { continue };
        p.as_object_mut().map(|o| o.remove("i"));
        by.entry(i).or_insert_with(|| serde_json::json!({ "i": i }))["props"] = p;
    }
    Ok(by)
}

/// The UNESCO site id in a World Heritage List URL.
fn whc_site(url: &str) -> Option<&str> {
    let id = url.split("whc.unesco.org/en/list/").nth(1)?.trim_end_matches('/');
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then_some(id)
}

/// Today's heritage sites (World Heritage components among them).
fn heritage_points(out: &Out, src: &str, pts: &mut Vec<Pt>) -> Result<()> {
    let details = heritage_details(out, src)?;
    let k = marks::kind_index("heritage").unwrap();
    let fields = marks::fields("heritage");
    let fc: Value = serde_json::from_slice(&src_bytes(out, src, "layer-heritage")?)?;
    let feats = fc["features"].as_array().context("layer-heritage: no features")?;
    for (rank, f) in feats.iter().enumerate() {
        let c = &f["geometry"]["coordinates"];
        let (Some(lon), Some(lat)) = (c[0].as_f64(), c[1].as_f64()) else { continue };
        let mut props = f["properties"].as_object().cloned().unwrap_or_default();
        let i = props.remove("i").and_then(|v| v.as_u64());
        let info = i.and_then(|i| details.get(&i));
        let fnum = |key: &str| props.get(key).and_then(Value::as_f64);
        let level = fnum("level");
        let tier = marks::heritage_tier(props.get("t").and_then(Value::as_str), level);
        let tier_i = marks::tier_index(tier).with_context(|| format!("layer-heritage #{rank}: unknown tier {tier:?}"))?;
        // dotData's class: level class (World Heritage, national top grade, the rest) + 3 × group.
        let l = level.filter(|v| *v != 0.0 && !v.is_nan()).unwrap_or(5.0);
        let group = ["w", "n", "p", "m"].iter().position(|g| tier.starts_with(g)).unwrap_or(3) as u8;
        let class = (if l == 1.0 { 0 } else if l == 2.0 { 1 } else { 2 }) + 3 * group;
        let named = props.get("name").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
        // JavaScript's truthiness, as the app's index reads `pt` (an empty string isn't a part).
        let component = props.get("pt").is_some_and(|v| match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
            Value::String(s) => !s.is_empty(),
            _ => true,
        });
        let pt = MarkPt {
            lon: marks::e7(lon),
            lat: marks::e7(lat),
            fa: fnum("fa").unwrap_or(0.0) as f32,
            ia: fnum("ia").map_or(marks::IA_UNKNOWN, |v| v as f32),
            mz: fnum("mz").map_or(f32::NAN, |v| v as f32),
            rank: rank as u32,
            kz: marks::KZ_NONE,
            class,
            tier: tier_i,
            flags: if named { flag::NAMED } else { 0 } | if component { flag::COMPONENT } else { 0 },
        };
        let fvals = fields.iter().map(|p| marks::num(&props, p)).collect();
        // References: a World Heritage Site's dot by its UNESCO id; a Canadian federal designation by
        // its DFHD id; else the legacy reference (tier, place, name, record URL).
        let url = info.and_then(|v| v["props"]["url"].as_str()).unwrap_or("");
        let name = props.get(if component { "cn" } else { "name" }).and_then(Value::as_str).unwrap_or("");
        let reference = match (whc_site(url), component, info.and_then(|v| v["props"]["dfhd_id"].as_str().map(str::to_string).or_else(|| v["props"]["dfhd_id"].as_u64().map(|n| n.to_string())))) {
            (Some(site), false, _) if tier.starts_with('w') => format!("whc:{site}"),
            (_, _, Some(d)) => format!("reg:dfhd:{d}"),
            _ => format!("legacy:heritage|{tier}|{},{}|{name}|{url}", pt.lon, pt.lat),
        };
        pts.push(Pt { kind: k, lon, lat, pt, fvals, props, info: info.map(Value::to_string), osm: None, reference });
    }
    Ok(())
}

/// Every point of today's files.
fn load_points(out: &Out) -> Result<Vec<Pt>> {
    let details = poi_details(out)?;
    let mut pts: Vec<Pt> = Vec::new();
    for kind in POI_KINDS {
        let k = marks::kind_index(kind).unwrap();
        let fc: Value = serde_json::from_slice(&legacy_bytes(out, &format!("layer-pois-{kind}"))?)?;
        let feats = fc["features"].as_array().with_context(|| format!("layer-pois-{kind}: no features"))?;
        let fields = marks::fields(kind);
        for (rank, f) in feats.iter().enumerate() {
            let c = &f["geometry"]["coordinates"];
            let (Some(lon), Some(lat)) = (c[0].as_f64(), c[1].as_f64()) else { continue };
            let mut props = f["properties"].as_object().cloned().unwrap_or_default();
            let i = props.remove("i").and_then(|v| v.as_u64());
            let (osm, info) = match i.and_then(|i| details.get(&i)) {
                Some((osm, info)) => (osm.as_deref().and_then(marks::osm_id), Some(info.clone())),
                None => (None, None),
            };
            let named = props.get("name").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
            let picnic = props.get("kind").and_then(Value::as_str) == Some("picnic_site");
            let fnum = |key: &str| props.get(key).and_then(Value::as_f64);
            let pt = MarkPt {
                lon: marks::e7(lon),
                lat: marks::e7(lat),
                fa: fnum("fa").unwrap_or(0.0) as f32,
                ia: fnum("ia").map_or(marks::IA_UNKNOWN, |v| v as f32),
                mz: fnum("mz").map_or(f32::NAN, |v| v as f32),
                rank: rank as u32,
                kz: marks::KZ_NONE,
                class: 0,
                tier: 0,
                flags: if named { flag::NAMED } else { 0 } | if picnic { flag::PICNIC } else { 0 },
            };
            let fvals = fields.iter().map(|p| marks::num(&props, p)).collect();
            let name = props.get("name").and_then(Value::as_str).unwrap_or("");
            let sub = props.get("kind").and_then(Value::as_str).unwrap_or(kind);
            let reference = format!("legacy:poi|{sub}|{},{}|{name}", pt.lon, pt.lat);
            pts.push(Pt { kind: k, lon, lat, pt, fvals, props, info, osm, reference });
        }
    }
    heritage_points(out, LEGACY, &mut pts)?;
    Ok(pts)
}

/// Today's heritage sites as points (the `marks` job's until the `heritage` job makes them).
pub fn heritage_marks(out: &Out, src: &str) -> Result<Vec<Point>> {
    let mut pts = Vec::new();
    heritage_points(out, src, &mut pts)?;
    Ok(pts)
}

pub struct Converted {
    pub points: usize,
    pub tiles: usize,
    pub thinned: usize,
}

/// Today's points with the ids `write` gives them (the same inputs, the same assignment): for the
/// overlays' World Heritage outlines, which carry their site dot's id.
pub fn points_with_ids(out: &Out) -> Result<(Vec<Point>, Vec<u64>)> {
    let pts = load_points(out)?;
    let ids = marks::assign_ids(&id_sources(&pts))?;
    Ok((pts, ids))
}

fn id_sources(pts: &[Point]) -> Vec<IdSource> {
    pts.iter().map(|p| IdSource { osm: p.osm, reference: p.reference.clone(), canon: format!("{}{}", Value::Object(p.props.clone()), p.info.as_deref().unwrap_or("")) }).collect()
}

/// Converts today's points: see [`write`].
pub fn convert(out: &mut Out) -> Result<Converted> {
    let pts = load_points(out)?;
    let summits = summits(out)?;
    write(out, pts, summits)
}

/// Writes points as the map reads them: ids (unique over every point: an OSM id only when no other
/// point has it), the thinned tiles' keep rule, markdata per z6 tile, thinned tiles per kind (packs
/// `layers/marks-<kind>/{root,lo}`), and `global/marks/summary`; with the named peaks for the
/// highest in view.
pub fn write(out: &mut Out, pts: Vec<Point>, summits: Vec<(SummitRec, String)>) -> Result<Converted> {
    let t0 = std::time::Instant::now();
    let ids = marks::assign_ids(&id_sources(&pts))?;
    let mut all: Vec<(u64, Pt)> = ids.into_iter().zip(pts).collect();
    eprintln!("marks: {} points with ids in {:.1?}", all.len(), t0.elapsed());
    // The World Heritage outlines' dots, for the overlays: each site's record (`i`) → its dot's id
    // and place.
    let kh = marks::kind_index("heritage").unwrap();
    let dots: BTreeMap<u64, (u64, f64, f64)> = all
        .iter()
        .filter(|(_, p)| p.kind == kh && p.pt.flags & flag::COMPONENT == 0)
        .filter_map(|(id, p)| Some((p.info.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok())?["i"].as_u64()?, (*id, p.lon, p.lat))))
        .collect();
    out.put_bytes(HERITAGE_DOTS, "json", &serde_json::to_vec(&dots)?)?;

    // The keep rule, per kind.
    let mut by_kind: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (j, (_, p)) in all.iter().enumerate() {
        by_kind.entry(p.kind).or_default().push(j);
    }
    for (&k, idx) in &by_kind {
        let sizes = marks::size_fields(KINDS[k]);
        let fields = marks::fields(KINDS[k]);
        let si: Vec<usize> = sizes.iter().map(|s| fields.iter().position(|f| f == s).unwrap()).collect();
        let kp: Vec<KeepPt> = idx.iter().map(|&j| { let p = &all[j].1; KeepPt { lon: p.lon, lat: p.lat, pt: &p.pt, sizes: si.iter().map(|&f| p.fvals[f]).collect() } }).collect();
        let kz = marks::keep_zooms(&kp);
        drop(kp);
        for (&j, z) in idx.iter().zip(kz) {
            all[j].1.pt.kz = z;
        }
        let kept: Vec<usize> = (0..=marks::THIN_MAX_Z).map(|z| idx.iter().filter(|&&j| all[j].1.pt.kz <= z).count()).collect();
        eprintln!("marks: {:<15} {:>7} points; kept at z0–5: {kept:?}", KINDS[k], idx.len());
    }

    // Markdata per z6 tile.
    let mut tiles: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (j, (_, p)) in all.iter().enumerate() {
        tiles.entry(marks::tile_at(p.lon, p.lat, 6)).or_default().push(j);
    }
    let mut summits_by: BTreeMap<(u32, u32), Vec<(SummitRec, String)>> = BTreeMap::new();
    for s in summits {
        summits_by.entry(marks::tile_at(marks::deg(s.0.lon), marks::deg(s.0.lat), 6)).or_default().push(s);
    }
    let mut keys: Vec<(u32, u32)> = tiles.keys().chain(summits_by.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    let written: Vec<((u32, u32), PathBuf)> = keys
        .par_iter()
        .map(|&t| -> Result<((u32, u32), PathBuf)> {
            let mut rows: Vec<Vec<Row>> = (0..KINDS.len()).map(|_| Vec::new()).collect();
            for &j in tiles.get(&t).map(Vec::as_slice).unwrap_or(&[]) {
                let (id, p) = &all[j];
                rows[p.kind].push(Row { id: *id, pt: p.pt, fvals: p.fvals.clone(), props: Value::Object(p.props.clone()).to_string().into_bytes(), info: p.info.clone().unwrap_or_default().into_bytes() });
            }
            for r in &mut rows {
                r.sort_by_key(|r| r.id);
            }
            let local = out.scratch_file(&format!("markdata/6-{}-{}.sect", t.0, t.1));
            marks::write_markdata(&local, t, &rows, summits_by.get(&t).map(Vec::as_slice).unwrap_or(&[]))?;
            Ok((t, local))
        })
        .collect::<Result<_>>()?;
    // What this write uploads: every markdata tile and marks pack it doesn't is stale and goes
    // from the manifest at the end (a tile or kind that lost its points, a region removed).
    let mut wrote: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (t, local) in &written {
        let l = format!("markdata/6-{}-{}", t.0, t.1);
        out.put_file(&l, "sect", local)?;
        wrote.insert(l);
    }
    eprintln!("marks: {} markdata tiles in {:.1?}", written.len(), t0.elapsed());

    // Thinned tiles per kind, z0–5: root (z0–2) and lo per z3 (z3–5).
    let mut thinned = 0;
    for (&k, idx) in &by_kind {
        let kind = KINDS[k];
        let kp: Vec<KeepPt> = idx.iter().map(|&j| { let p = &all[j].1; KeepPt { lon: p.lon, lat: p.lat, pt: &p.pt, sizes: Vec::new() } }).collect();
        let mut packs: BTreeMap<(&'static str, u8, u32, u32), Vec<(u8, u32, u32, Vec<u8>, u32)>> = BTreeMap::new();
        for z in 0..=marks::THIN_MAX_Z {
            let mut at: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
            for (q, p) in kp.iter().enumerate() {
                if p.pt.flags & flag::COMPONENT == 0 {
                    at.entry(marks::tile_at(p.lon, p.lat, z)).or_default().push(q);
                }
            }
            let made: Vec<((u32, u32), Vec<u8>, u32)> = at
                .par_iter()
                .map(|(&(x, y), qs)| -> Result<((u32, u32), Vec<u8>, u32)> {
                    let mut keep: Vec<usize> = qs.iter().copied().filter(|&q| kp[q].pt.kz <= z).collect();
                    keep.sort_by_key(|&q| kp[q].pt.rank);
                    let cells: Vec<Cell> = marks::speck_cells(&kp, qs, z, x, y);
                    let tile = MarkTile {
                        ids: keep.iter().map(|&q| all[idx[q]].0).collect(),
                        fvals: (0..marks::fields(kind).len()).map(|f| keep.iter().map(|&q| all[idx[q]].1.fvals[f]).collect()).collect(),
                        pts: keep.iter().map(|&q| *kp[q].pt).collect(),
                        cells,
                        props: keep.iter().map(|&q| Value::Object(all[idx[q]].1.props.clone()).to_string().into_bytes()).collect(),
                    };
                    let raw = tile.encode();
                    let gz = gzip(&raw)?;
                    Ok(((x, y), gz, raw.len() as u32))
                })
                .collect::<Result<_>>()?;
            for ((x, y), gz, raw) in made {
                packs.entry(crate::layers::pack_of(z, x, y)).or_default().push((z, x, y, gz, raw));
                thinned += 1;
            }
        }
        for ((scope, rz, rx, ry), mut tiles) in packs {
            tiles.sort_by_key(|t| roadcore::archive::tile_key(t.0, t.1, t.2));
            if let Some((l, _)) = crate::layers::write_pack(out, &format!("marks-{kind}"), "rdmt", true, scope, (rz, rx, ry), &mut tiles.into_iter())? {
                wrote.insert(l);
            }
        }
    }
    // Totals per kind (the `kind` property: rest is rest_area and picnic_site) and heritage tier, for
    // the Layers panel.
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    let mut tiers: BTreeMap<String, u64> = BTreeMap::new();
    for (_, p) in &all {
        if p.pt.flags & flag::COMPONENT == 0 {
            let k = p.props.get("kind").and_then(Value::as_str).unwrap_or(KINDS[p.kind]);
            *totals.entry(k.to_string()).or_default() += 1;
            if KINDS[p.kind] == "heritage" {
                *tiers.entry(marks::TIERS[p.pt.tier as usize].to_string()).or_default() += 1;
            }
        }
    }
    out.put_bytes("global/marks/summary", "json", &serde_json::to_vec(&serde_json::json!({ "fmt": 1, "kinds": totals, "tiers": tiers }))?)?;
    let stale: Vec<String> = out.manifest.keys().filter(|k| (k.starts_with("markdata/") || k.starts_with("layers/marks-")) && !wrote.contains(*k)).cloned().collect();
    for k in &stale {
        out.remove(k);
    }
    if !stale.is_empty() {
        eprintln!("marks: {} stale markdata tiles and packs dropped", stale.len());
    }
    out.save()?;
    Ok(Converted { points: all.len(), tiles: written.len(), thinned })
}

/// The named peaks with a height, in the summits list's order ([lon, lat, ele, name]).
fn summits(out: &Out) -> Result<Vec<(SummitRec, String)>> {
    let v: Value = serde_json::from_slice(&legacy_bytes(out, "layer-summits")?)?;
    let list = v["p"].as_array().context("layer-summits: no p")?;
    Ok(list
        .iter()
        .enumerate()
        .filter_map(|(rank, e)| {
            let (lon, lat, ele, name) = (e[0].as_f64()?, e[1].as_f64()?, e[2].as_f64()?, e[3].as_str()?);
            Some((SummitRec { rank: rank as u32, lon: marks::e7(lon), lat: marks::e7(lat), pad: 0, ele }, name.to_string()))
        })
        .collect())
}

fn gzip(b: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    e.write_all(b)?;
    Ok(e.finish()?)
}
