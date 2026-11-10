//! The timetables' to-do list and report (docs/inputs.md §6.7.3).
//!
//! OSM decides what's drawn; timetables only add counts. Two things the owner can't see otherwise:
//! - **The to-do:** OSM's rail and ferry lines in the coverage that no timetable covers, by
//!   priority, as `inputs/timetables/gtfs/todo/lines.jsonl` beside a brief (`todo/README.md`). The
//!   owner answers with static GTFS zips in the inputs folder (inputs::inputs::gtfs).
//! - **The report:** timetable routes that match nothing on the map, as `work/timetables/report`,
//!   which the lead shows on the build page and in `scenic status`. Never drawn, never held.
//!
//! The pieces (their paths: inputs::inputs::gtfs's `RAIL_LINES`, `FERRY_LINES`) come from the jobs that match timetables to the map: `rail` (`work/rail/lines`:
//! [`rail_lines`] and [`rail_unmatched`], from railfreq's misses) and `ferries`
//! (`work/ferries/lines`, landmarks::ferries' `timetables.json`). The lead-only `timetables-todo` job
//! ([`run`]) merges them.

use anyhow::{Context, Result};
use inputs::inputs::gtfs::{FERRY_LINES, RAIL_LINES};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::BufRead;

/// The report (the status's view of it is the inputs' gtfs::View).
pub use inputs::inputs::gtfs::REPORT;
/// The to-do list's file in the inputs folder's `todo/`.
pub const TODO_FILE: &str = "lines.jsonl";
/// The job's version: in its key.
pub const VERSION: u32 = 1;
/// The brief, `todo/README.md`.
pub const BRIEF: &str = include_str!("../../../tools/timetables/todo.md");
/// A rail line counts as covered when at least this share of its length has trains.
pub const COVERED: f64 = 0.5;
/// A line that isn't a passenger service (by its tags) counts this much of its length.
pub const NOT_PASSENGER: f64 = 0.1;

/// The route values of the rail set's service relations (not `railway`: the track, not a service).
const SERVICES: [&str; 6] = ["train", "subway", "tram", "light_rail", "monorail", "funicular"];

/// A relation of an OPL file: id, tags, way members.
#[derive(Debug, Clone)]
pub struct Rel {
    pub id: u64,
    pub tags: BTreeMap<String, String>,
    pub ways: Vec<u64>,
}

/// The rail service relations of an OPL file (`osmium cat -t relation -f opl`).
pub fn read_rels(r: impl BufRead) -> Result<Vec<Rel>> {
    use buildstore::outlines::opl::unescape;
    let mut out = Vec::new();
    for l in r.lines() {
        let l = l?;
        let mut f = l.split(' ');
        let Some(id) = f.next().and_then(|h| h.strip_prefix('r')).and_then(|s| s.parse().ok()) else { continue };
        let field = |c: char| l.split(' ').skip(1).find_map(|x| x.strip_prefix(c)).unwrap_or("");
        let tags: BTreeMap<String, String> = field('T')
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (unescape(k), unescape(v)))
            .collect();
        if !tags.get("route").is_some_and(|r| SERVICES.contains(&r.as_str())) {
            continue;
        }
        let ways = field('M')
            .split(',')
            .filter_map(|m| m.split_once('@').map_or(m, |(r, _)| r).strip_prefix('w')?.parse().ok())
            .collect();
        out.push(Rel { id, tags, ways });
    }
    out.sort_by_key(|r| r.id);
    Ok(out)
}

/// Whether a line's tags make it a passenger service.
fn passenger(t: &BTreeMap<String, String>) -> bool {
    let g = |k: &str| t.get(k).map(String::as_str);
    g("passenger") != Some("no") && g("service") != Some("freight")
}

/// The rail lines in the coverage: the service relations grouped as the ferries' are (a line is
/// its direction variants: one network or operator's `ref`, else the same two ends, else the
/// relation alone; the first relation's id names it), each with its length over the ways in the
/// coverage (`len`: OSM way id → metres) and the length of those with trains (`served`).
pub fn rail_lines(rels: &[Rel], len: &HashMap<u64, f64>, served: &HashSet<u64>) -> Vec<Value> {
    let mut groups: BTreeMap<(String, String, String), Vec<&Rel>> = BTreeMap::new();
    for r in rels {
        let g = |k: &str| r.tags.get(k).cloned().unwrap_or_default();
        let who = if g("network").is_empty() { g("operator") } else { g("network") }.to_lowercase();
        let key = if !g("ref").is_empty() {
            ("ref".into(), who, g("ref").to_lowercase())
        } else if !g("from").is_empty() && !g("to").is_empty() {
            let mut e = [g("from").to_lowercase(), g("to").to_lowercase()];
            e.sort();
            ("ends".into(), who, e.join("\u{1f}"))
        } else {
            ("r".into(), String::new(), format!("{:020}", r.id))
        };
        groups.entry(key).or_default().push(r);
    }
    let mut out = Vec::new();
    for rs in groups.values() {
        let ways: BTreeSet<u64> = rs.iter().flat_map(|r| r.ways.iter().copied()).collect();
        let km: f64 = ways.iter().filter_map(|w| len.get(w)).sum::<f64>() / 1000.0;
        if km <= 0.0 {
            continue;
        }
        let with: f64 = ways.iter().filter(|w| served.contains(w)).filter_map(|w| len.get(w)).sum::<f64>() / 1000.0;
        let first = rs[0];
        let tag = |k: &str| rs.iter().find_map(|r| r.tags.get(k)).cloned().unwrap_or_default();
        let operator = {
            let o = tag("operator");
            if o.is_empty() {
                tag("network")
            } else {
                o
            }
        };
        out.push(json!({
            "osm": format!("r{}", first.id), "kind": "rail", "name": tag("name"), "ref": tag("ref"),
            "operator": operator,
            "from": tag("from"), "to": tag("to"), "km": round1(km), "served_km": round1(with),
            "passenger": rs.iter().all(|r| passenger(&r.tags)),
        }));
    }
    out
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// A stop pair's position as the pairs hold it (four f32s' bits).
type Key = [u32; 4];

/// The positions of a pairs file's records (rail::PAIR bytes each).
pub fn pair_keys(pairs: &[u8]) -> HashSet<Key> {
    inputs::rail::Pair::all(pairs).map(|r| [r.a[0], r.a[1], r.b[0], r.b[1]].map(f32::to_bits)).collect()
}

/// railfreq's `rail-miss.bin`.
pub fn miss_keys(miss: &[u8]) -> HashSet<Key> {
    miss.chunks_exact(16).map(|r| [0, 4, 8, 12].map(|i| u32::from_le_bytes(r[i..i + 4].try_into().unwrap()))).collect()
}

/// The rail routes that match nothing on the map, from landmarks::gtfs::railcount's routes (`routes`): a route
/// with stop pairs in the coverage (`kept`: the pairs railfreq was given), none of which found a
/// track (`missed`), "a line OSM lacks"; a static route whose `osm` pin names neither a relation
/// (`rels`) nor a way (`ways`) of the pass's rail in the coverage. Each as landmarks::ferries writes its
/// own: `{mode, source, static, route, name, agency, osm, why}`.
pub fn rail_unmatched(
    routes: &Value,
    kept: &HashSet<Key>,
    missed: &HashSet<Key>,
    rels: &HashSet<u64>,
    ways: &HashSet<u64>,
) -> Vec<Value> {
    let key = |p: &Value| -> Option<Key> {
        let a = p.as_array()?;
        let f = |i: usize| a.get(i).and_then(Value::as_f64).map(|v| (v as f32).to_bits());
        Some([f(0)?, f(1)?, f(2)?, f(3)?])
    };
    let pairs: Vec<Option<Key>> = routes["pairs"].as_array().map(|a| a.iter().map(key).collect()).unwrap_or_default();
    let mut out = Vec::new();
    for r in routes["routes"].as_array().into_iter().flatten() {
        let mine: Vec<Key> = r["pairs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| pairs.get(i.as_u64()? as usize).copied().flatten())
            .filter(|k| kept.contains(k))
            .collect();
        let pin = r["osm"].as_str().unwrap_or("");
        let pinned_to_nothing = !pin.is_empty()
            && match (pin.as_bytes()[0], pin[1..].parse::<u64>()) {
                (b'r', Ok(id)) => !rels.contains(&id),
                (b'w', Ok(id)) => !ways.contains(&id),
                _ => true,
            };
        let why = if pinned_to_nothing && !mine.is_empty() {
            "its osm pin names no rail line of this pass"
        } else if !mine.is_empty() && mine.iter().all(|k| missed.contains(k)) {
            "its stops match no OpenStreetMap track"
        } else {
            continue;
        };
        let mut v = r.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("pairs");
            o.insert("why".into(), why.into());
        }
        out.push(v);
    }
    out
}

/// A line's priority: its length, a tenth of it for a line that isn't a passenger service.
fn priority(l: &Value) -> f64 {
    let km = l["km"].as_f64().unwrap_or(0.0);
    round1(km * if l["passenger"].as_bool().unwrap_or(true) { 1.0 } else { NOT_PASSENGER })
}

/// What [`run`] came to.
#[derive(Debug, Default)]
pub struct Report {
    pub rail: usize,
    pub ferry: usize,
    pub km: f64,
    pub unmatched: usize,
}

/// The to-do list's lines (by priority, then id) and the report, from the two pieces: rail lines
/// with less than [`COVERED`] of their length served; ferry lines without a timetable with an end
/// in the coverage (`inside`, E7 points).
pub fn merge(rail: &Value, ferries: &Value, inside: &dyn Fn([i32; 2]) -> bool) -> (Vec<Value>, Value, Report) {
    let mut lines: Vec<Value> = Vec::new();
    for l in rail["lines"].as_array().into_iter().flatten() {
        let (km, with) = (l["km"].as_f64().unwrap_or(0.0), l["served_km"].as_f64().unwrap_or(0.0));
        if km > 0.0 && with < COVERED * km {
            lines.push(l.clone());
        }
    }
    for l in ferries["lines"].as_array().into_iter().flatten() {
        let e7 = |p: &Value| -> Option<[i32; 2]> {
            Some([(p.get(0)?.as_f64()? * 1e7).round() as i32, (p.get(1)?.as_f64()? * 1e7).round() as i32])
        };
        if l["ends"].as_array().into_iter().flatten().filter_map(e7).any(inside) {
            lines.push(l.clone());
        }
    }
    let fields = ["osm", "kind", "name", "ref", "operator", "from", "to", "km"];
    let mut todo: Vec<Value> = lines
        .iter()
        .map(|l| {
            let mut o = serde_json::Map::new();
            for f in fields {
                o.insert(f.into(), l.get(f).cloned().unwrap_or(Value::from("")));
            }
            o.insert("priority".into(), priority(l).into());
            Value::Object(o)
        })
        .collect();
    todo.sort_by(|a, b| {
        b["priority"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&a["priority"].as_f64().unwrap_or(0.0))
            .then_with(|| a["kind"].as_str().cmp(&b["kind"].as_str()))
            .then_with(|| a["osm"].as_str().cmp(&b["osm"].as_str()))
    });
    let mut unmatched: Vec<Value> =
        rail["unmatched"].as_array().into_iter().chain(ferries["unmatched"].as_array()).flatten().cloned().collect();
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    unmatched.sort_by_key(|v| (s(v, "mode"), s(v, "source"), s(v, "route")));
    let r = Report {
        rail: todo.iter().filter(|l| l["kind"] == "rail").count(),
        ferry: todo.iter().filter(|l| l["kind"] == "ferry").count(),
        km: round1(todo.iter().filter_map(|l| l["km"].as_f64()).sum()),
        unmatched: unmatched.len(),
    };
    let report = json!({"to_do": todo.len(), "to_do_km": r.km, "by_kind": {"rail": r.rail, "ferry": r.ferry}, "file": format!("{}/{TODO_FILE}", inputs::inputs::todo_rel(inputs::inputs::gtfs::UNIT)), "unmatched": unmatched});
    (todo, report, r)
}

/// Writes a file whole (store::whole::write: a held old file renamed aside).
fn put(path: &store::nasfs::NasPath, bytes: &[u8]) -> Result<()> {
    path.write_whole(bytes)
}

/// The job: the two pieces read from `out`'s records, the to-do list and its brief written whole
/// into the static timetables' inputs folder's `todo/` (under `todo_root`: the project folder, or a
/// scratch one; `todo/` is never read as an input, inputs::inputs::list), the report put as
/// [`REPORT`].
pub fn run(
    out: &mut buildstore::out::Out,
    cov: &buildstore::coverage::Coverage,
    todo_root: &store::nasfs::NasRoot,
) -> Result<Report> {
    let read = |l: &str| -> Result<Value> {
        let c = out.get(l).with_context(|| format!("no {l} yet"))?;
        Ok(serde_json::from_slice(&out.path(c).read().with_context(|| format!("read {c}"))?)?)
    };
    use jobrt::timings::phase;
    let p = phase("reading the rail and ferry lines");
    let (rail, ferries) = (read(RAIL_LINES)?, read(FERRY_LINES)?);
    drop(p);
    let p = phase("matching the lines");
    let (todo, report, r) = merge(&rail, &ferries, &|p| cov.contains(p));
    drop(p);
    let _p = phase("writing the to-do list and the report");
    let dir = inputs::inputs::todo_dir(todo_root, inputs::inputs::gtfs::UNIT);
    dir.create_dir_all()?;
    let text: String = todo.iter().map(|l| l.to_string() + "\n").collect();
    put(&dir.join(TODO_FILE), text.as_bytes())?;
    put(&dir.join("README.md"), BRIEF.as_bytes())?;
    out.put_bytes(REPORT, "json", &serde_json::to_vec(&report)?)?;
    out.save()?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPL: &str = "r1 v1 Ttype=route,route=train,ref=S1,network=ZVV,name=S1%20%Zug Mw10@,w11@\n\
r2 v1 Ttype=route,route=train,ref=S1,network=ZVV,name=S1 back Mw11@,w12@\n\
r3 v1 Ttype=route,route=railway,name=Track Mw10@\n\
r4 v1 Ttype=route,route=train,service=freight,from=A,to=B Mw20@\n\
r5 v1 Ttype=route,route=tram,name=Far Mw99@\n";

    #[test]
    fn rail_lines_grouped_and_measured() {
        let rels = read_rels(OPL.as_bytes()).unwrap();
        assert_eq!(rels.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2, 4, 5], "not the track's relation");
        assert_eq!(rels[0].tags["name"], "S1 Zug");
        let len: HashMap<u64, f64> = [(10, 1000.0), (11, 2000.0), (12, 3000.0), (20, 5000.0)].into();
        let served: HashSet<u64> = [12].into();
        let l = rail_lines(&rels, &len, &served);
        assert_eq!(l.len(), 2, "both directions one line; a line with no way in the coverage left out");
        let s1 = l.iter().find(|x| x["osm"] == "r1").unwrap();
        assert_eq!(
            (s1["km"].as_f64(), s1["served_km"].as_f64(), s1["operator"].as_str()),
            (Some(6.0), Some(3.0), Some("ZVV"))
        );
        let f = l.iter().find(|x| x["osm"] == "r4").unwrap();
        assert_eq!(f["passenger"], false);
    }

    fn k(a: f32, b: f32, c: f32, d: f32) -> Key {
        [a, b, c, d].map(f32::to_bits)
    }

    #[test]
    fn rail_routes_matching_nothing() {
        let routes = json!({
            "pairs": [[1.0, 2.0, 1.5, 2.5], [1.5, 2.5, 2.0, 3.0], [9.0, 9.0, 9.5, 9.5]],
            "routes": [
                {"mode": "rail", "source": "f", "static": false, "route": "ok", "name": "A", "agency": "", "osm": "", "pairs": [0, 1]},
                {"mode": "rail", "source": "f", "static": false, "route": "gone", "name": "B", "agency": "", "osm": "", "pairs": [1]},
                {"mode": "rail", "source": "f", "static": false, "route": "away", "name": "C", "agency": "", "osm": "", "pairs": [2]},
                {"mode": "rail", "source": "s.zip", "static": true, "route": "p", "name": "D", "agency": "", "osm": "r77", "pairs": [0]},
                {"mode": "rail", "source": "s.zip", "static": true, "route": "q", "name": "E", "agency": "", "osm": "w5", "pairs": [0]}
            ]
        });
        let kept: HashSet<Key> = [k(1.0, 2.0, 1.5, 2.5), k(1.5, 2.5, 2.0, 3.0)].into();
        let missed: HashSet<Key> = [k(1.5, 2.5, 2.0, 3.0)].into();
        let u = rail_unmatched(&routes, &kept, &missed, &[1].into(), &[5].into());
        let got: Vec<(&str, &str)> =
            u.iter().map(|v| (v["route"].as_str().unwrap(), v["why"].as_str().unwrap())).collect();
        assert_eq!(
            got,
            vec![
                ("gone", "its stops match no OpenStreetMap track"),
                ("p", "its osm pin names no rail line of this pass")
            ],
            "a route beyond the coverage isn't reported; a route with one pair matched isn't"
        );
        assert!(u[0].get("pairs").is_none());
    }

    #[test]
    fn the_list_and_the_report() {
        let rail = json!({"lines": [
            {"osm": "r1", "kind": "rail", "name": "Long", "km": 100.0, "served_km": 10.0, "passenger": true},
            {"osm": "r2", "kind": "rail", "name": "Served", "km": 100.0, "served_km": 60.0, "passenger": true},
            {"osm": "r3", "kind": "rail", "name": "Freight", "km": 300.0, "served_km": 0.0, "passenger": false}
        ], "unmatched": [{"mode": "rail", "source": "f", "route": "x", "why": "w"}]});
        let ferries = json!({"lines": [
            {"osm": "w9", "kind": "ferry", "name": "In", "km": 40.0, "passenger": true, "ends": [[0.5, 0.5], [3.0, 3.0]]},
            {"osm": "w8", "kind": "ferry", "name": "Out", "km": 400.0, "passenger": true, "ends": [[5.0, 5.0], [6.0, 6.0]]}
        ], "unmatched": [{"mode": "ferry", "source": "a", "route": "y", "why": "w"}]});
        let inside = |p: [i32; 2]| p[0] < 10_000_000 && p[1] < 10_000_000;
        let (todo, report, r) = merge(&rail, &ferries, &inside);
        let ids: Vec<&str> = todo.iter().map(|l| l["osm"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["r1", "w9", "r3"], "by priority: a freight line's tenth");
        assert_eq!(
            todo[0].as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>(),
            ["osm", "kind", "name", "ref", "operator", "from", "to", "km", "priority"].map(String::from).into()
        );
        assert_eq!((r.rail, r.ferry, r.unmatched), (2, 1, 2));
        let v = inputs::inputs::gtfs::View::of(&report);
        assert_eq!((v.to_do, v.unmatched_total, v.unmatched[0].mode.as_str()), (3, 2, "ferry"));
        assert!(v.lines()[0].contains("3 lines with no timetable"));
    }
}
