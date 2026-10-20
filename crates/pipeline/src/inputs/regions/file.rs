//! The region file (docs/inputs.md §6.1.3), `pins.toml` (§6.1.3, §6.1.5) and their parts, parsed:
//! every fault a finding of its own kind (`Fault`), nothing repaired.

use serde::Deserialize;
use std::collections::BTreeMap;

/// A fault of a file, by the check it's of (`rg-<check>`), in plain words.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub check: &'static str,
    pub message: String,
}

fn fault(check: &'static str, message: impl Into<String>) -> Fault {
    Fault { check, message: message.into() }
}

/// A part of `outline`, `minus`, `intersect` or `water` (§6.1.3).
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// An OSM relation's area.
    Osm(u64),
    /// A closed OSM way's area, from the regions' extract.
    OsmWay(u64),
    /// A 200-nautical-mile zone of Marine Regions' EEZ v12 by its MRGID, with the internal waters
    /// Internal Waters v4 draws for it (crate::inputs::regions::data).
    Eez(u64),
    /// A territorial sea (12 nmi) of Marine Regions' Territorial Seas v4 by its MRGID.
    Ts12(u64),
    /// A lake of Natural Earth's 1:10m lakes by its `ne_id`.
    NeLake(u64),
    /// A box of longitude and latitude: west, south, east, north (E7), and as written.
    Box([i64; 4], String),
    /// The whole Earth.
    Earth,
    /// Another region's land: its `outline` as resolved (for a region with `water`, cut back to the
    /// coastline's land), without its water.
    Land(String),
    /// Every region in a folder (its path in the folder tree), the region naming it left out.
    Group(String),
    /// What no other region covers: only in `water`, and in one region of the folder.
    Rest,
    /// Union, difference (the first less the others) and intersection of parts.
    Union(Vec<Part>),
    Minus(Vec<Part>),
    Intersect(Vec<Part>),
    /// The points of `of` nearer the parts `to` than the parts `than`, decided on a grid of `step_m`
    /// metres (`by` as written).
    Nearest {
        of: Box<Part>,
        to: Vec<Part>,
        than: Vec<Part>,
        step_m: f64,
        by: String,
    },
    /// A Polygon or MultiPolygon in a GeoJSON file of the folder, by its file name.
    Shape(String),
    /// Another region's area, as stored.
    Region(String),
    /// Everything within `m` metres (`by` as written) of a part or a line.
    Buffer {
        of: Box<Target>,
        m: f64,
        by: String,
    },
    /// The part of a shape on one side of a line.
    Cut {
        of: Box<Part>,
        line: String,
        left: bool,
    },
}

/// What a buffer is of.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Part(Part),
    Line(String),
}

impl std::fmt::Display for Part {
    /// The part as text: `osm:<id>`, `shape:<name>`, `region:<id>`, `{buffer <of> <by>}`, `{cut <of>
    /// line:<name> <keep>}`: a stored shape's `from`, and the outline entries a catalog records.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Part::Osm(id) => write!(f, "osm:{id}"),
            Part::Shape(n) => write!(f, "shape:{n}"),
            Part::Region(id) => write!(f, "region:{id}"),
            Part::OsmWay(id) => write!(f, "osm-way:{id}"),
            Part::Eez(id) => write!(f, "eez:{id}"),
            Part::Ts12(id) => write!(f, "ts12:{id}"),
            Part::NeLake(id) => write!(f, "ne-lake:{id}"),
            Part::Box(_, t) => write!(f, "box:{t}"),
            Part::Earth => write!(f, "earth"),
            Part::Land(id) => write!(f, "land:{id}"),
            Part::Group(g) => write!(f, "group:{g}"),
            Part::Rest => write!(f, "rest"),
            Part::Union(ps) => write!(f, "{{union {}}}", list_text(ps)),
            Part::Minus(ps) => write!(f, "{{minus {}}}", list_text(ps)),
            Part::Intersect(ps) => write!(f, "{{intersect {}}}", list_text(ps)),
            Part::Nearest { of, to, than, by, .. } => {
                write!(f, "{{nearest {of} to {} than {} step {by}}}", list_text(to), list_text(than))
            }
            Part::Buffer { of, by, .. } => match &**of {
                Target::Part(p) => write!(f, "{{buffer {p} {by}}}"),
                Target::Line(l) => write!(f, "{{buffer line:{l} {by}}}"),
            },
            Part::Cut { of, line, left } => {
                write!(f, "{{cut {of} line:{line} {}}}", if *left { "left" } else { "right" })
            }
        }
    }
}

fn list_text(ps: &[Part]) -> String {
    format!("[{}]", ps.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "))
}

impl Part {
    /// Whether it's an operation (not a plain `osm:` or `shape:` part).
    pub fn is_plain(&self) -> bool {
        matches!(self, Part::Osm(_) | Part::Shape(_))
    }

    /// The parts directly inside it.
    pub fn children(&self) -> Vec<&Part> {
        match self {
            Part::Buffer { of, .. } => match &**of {
                Target::Part(p) => vec![p],
                Target::Line(_) => vec![],
            },
            Part::Cut { of, .. } => vec![of],
            Part::Union(ps) | Part::Minus(ps) | Part::Intersect(ps) => ps.iter().collect(),
            Part::Nearest { of, to, than, .. } => std::iter::once(&**of).chain(to).chain(than).collect(),
            _ => vec![],
        }
    }

    /// Whether it reads the regions' datasets (sea zones, lakes, the coastline's land).
    pub fn needs_data(&self) -> bool {
        matches!(self, Part::Eez(_) | Part::Ts12(_) | Part::NeLake(_))
    }

    /// Every part within it, itself first.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Part>) {
        out.push(self);
        for c in self.children() {
            c.walk(out);
        }
    }

    /// The lines it names.
    fn lines<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Part::Buffer { of, .. } => match &**of {
                Target::Part(p) => p.lines(out),
                Target::Line(l) => out.push(l),
            },
            Part::Cut { of, line, .. } => {
                out.push(line);
                of.lines(out);
            }
            p => {
                for c in p.children() {
                    c.lines(out);
                }
            }
        }
    }
}

/// A line (§6.1.3): drawn by hand, or OSM ways strung together.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Coords(Vec<[i64; 2]>),
    Ways(Vec<u64>),
}

/// A region file, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub id: String,
    pub name: String,
    pub outline: Vec<Part>,
    pub minus: Vec<Part>,
    pub intersect: Vec<Part>,
    /// Its water (§6.1.3, "Land and water"): when there, the outline is cut back to the coastline's
    /// land and this is added, less every region's land.
    pub water: Vec<Part>,
    /// Whether the file has a `water` key at all (`water = []`: land and lakes only, cut back to
    /// the coastline's land).
    pub water_key: bool,
    /// `hand_out = "<folder>"`: each stretch of its water that only one region of that folder
    /// borders goes to that region (`tiles::resolve`).
    pub hand_out: Option<String>,
    /// `hand_out_all = true`, with `hand_out`: every stretch goes, to the region of the folder it
    /// shares the longest edge with, or the nearest if it touches none (the list's rule for land
    /// inside a split place's border but outside all its regions); the region itself ends empty.
    pub hand_out_all: bool,
    /// `disabled = true`: checked and resolved like any other region (others may name it, the cover
    /// check counts it), but never built: left out of the coverage and every list the build plans
    /// or sizes from (crate::inputs::regions::read).
    pub disabled: bool,
    pub lines: BTreeMap<String, Line>,
    /// Its own `[pins]`, overriding `pins.toml`'s.
    pub pins: BTreeMap<String, String>,
}

impl Region {
    /// Whether it's stored as its parts unchanged (§6.1.4): only `osm:` and `shape:` parts in
    /// `outline`, no `minus` nor `intersect`.
    pub fn is_plain(&self) -> bool {
        self.minus.is_empty() && self.intersect.is_empty() && !self.water_key && self.outline.iter().all(Part::is_plain)
    }

    /// Whether it has land and water apart (a `water` key): the land-and-water rules apply to it.
    pub fn has_water(&self) -> bool {
        self.water_key
    }

    /// Whether any of its parts reads the regions' datasets.
    pub fn needs_data(&self) -> bool {
        self.has_water() || self.parts().iter().any(|p| p.needs_data())
    }

    /// The folders it names (`group:`).
    pub fn groups(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .parts()
            .into_iter()
            .filter_map(|p| if let Part::Group(g) = p { Some(g.as_str()) } else { None })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The regions whose land it names (`land:`).
    pub fn lands(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .parts()
            .into_iter()
            .filter_map(|p| if let Part::Land(id) = p { Some(id.as_str()) } else { None })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Whether its water is the rest (`rest`).
    pub fn is_rest(&self) -> bool {
        self.water
            .iter()
            .flat_map(|p| {
                let mut v = Vec::new();
                p.walk(&mut v);
                v
            })
            .any(|p| matches!(p, Part::Rest))
    }

    /// Every part of its outline (`outline`, `minus`, `intersect`), or of its water, nested ones
    /// included.
    pub fn parts_of(&self, water: bool) -> Vec<&Part> {
        let mut out = Vec::new();
        let top: Vec<&Part> = if water {
            self.water.iter().collect()
        } else {
            self.outline.iter().chain(&self.minus).chain(&self.intersect).collect()
        };
        for p in top {
            p.walk(&mut out);
        }
        out
    }

    /// The regions its outline names, by `region:` and `land:`, and the folders (`group:`).
    pub fn outline_refs(&self) -> (Vec<&str>, Vec<&str>) {
        let mut ids = Vec::new();
        let mut groups = Vec::new();
        for p in self.parts_of(false) {
            match p {
                Part::Region(id) | Part::Land(id) => ids.push(id.as_str()),
                Part::Group(g) => groups.push(g.as_str()),
                _ => {}
            }
        }
        ids.sort();
        ids.dedup();
        groups.sort();
        groups.dedup();
        (ids, groups)
    }

    /// Every part, nested ones included.
    pub fn parts(&self) -> Vec<&Part> {
        let mut out = Vec::new();
        for p in self.outline.iter().chain(&self.minus).chain(&self.intersect).chain(&self.water) {
            p.walk(&mut out);
        }
        out
    }

    /// The relations it names (sorted, once each).
    pub fn relations(&self) -> Vec<u64> {
        let mut v: Vec<u64> =
            self.parts().into_iter().filter_map(|p| if let Part::Osm(id) = p { Some(*id) } else { None }).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The ways its lines and `osm-way:` parts name (sorted, once each).
    pub fn ways(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .lines
            .values()
            .filter_map(|l| if let Line::Ways(w) = l { Some(w.clone()) } else { None })
            .flatten()
            .chain(self.parts().into_iter().filter_map(|p| if let Part::OsmWay(w) = p { Some(*w) } else { None }))
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The regions it names (`region:`).
    pub fn regions(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .parts()
            .into_iter()
            .filter_map(|p| if let Part::Region(id) = p { Some(id.as_str()) } else { None })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The shape files it names (`shape:`).
    pub fn shapes(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .parts()
            .into_iter()
            .filter_map(|p| if let Part::Shape(n) = p { Some(n.as_str()) } else { None })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The lines its parts use, by name (sorted, once each).
    pub fn used_lines(&self) -> Vec<&str> {
        let mut v = Vec::new();
        for p in self.outline.iter().chain(&self.minus).chain(&self.intersect).chain(&self.water) {
            p.lines(&mut v);
        }
        v.sort();
        v.dedup();
        v
    }

    /// Its outline entries as text, as a catalog records them and `outline_digest` hashes them: the
    /// same strings as a recipe's for `osm:` parts.
    pub fn outline_text(&self) -> Vec<String> {
        self.outline.iter().map(|p| p.to_string()).chain(self.water.iter().map(|p| format!("water {p}"))).collect()
    }
}

/// A region's id: `[a-z0-9-]`, 1–64 characters, not starting with a dash (as recipes' ids).
pub fn valid_id(id: &str) -> bool {
    crate::agent::recipes::valid_id(id)
}

/// A shape file's name: `<name>.geojson`, no path.
pub fn valid_shape_name(n: &str) -> bool {
    n.strip_suffix(".geojson")
        .is_some_and(|s| !s.is_empty() && !s.starts_with(['.', '@', '#', '_']) && !s.contains(['/', '\\']))
}

/// A distance: a number and `m`, `km` or `nmi`, positive, at most 200 nmi; in metres.
pub fn parse_distance(s: &str) -> Result<f64, String> {
    let t = s.trim();
    let (num, unit) = t.split_at(
        t.find(|c: char| c.is_ascii_alphabetic())
            .ok_or_else(|| format!("{s:?}: a distance is a number and m, km or nmi"))?,
    );
    let v: f64 = num.trim().parse().map_err(|_| format!("{s:?}: a distance is a number and m, km or nmi"))?;
    let m = match unit.trim() {
        "m" => v,
        "km" => v * 1000.0,
        "nmi" => v * 1852.0,
        u => return Err(format!("{s:?}: {u:?} isn't m, km or nmi")),
    };
    if !(m > 0.0 && m <= super::geom::MAX_BUFFER_M) {
        return Err(format!("{s:?}: a distance is more than nothing and at most 200 nmi"));
    }
    Ok(m)
}

/// A nearest's grid step when none is given.
pub const DEFAULT_STEP: &str = "1 km";
const MIN_STEP_M: f64 = 100.0;
const MAX_STEP_M: f64 = 20_000.0;

/// A box, `<west>,<south>,<east>,<north>` in degrees: west under east (a box across ±180° is two),
/// south under north, within −180…180 and −90…90; E7.
pub fn parse_box(s: &str) -> Result<[i64; 4], String> {
    let v: Vec<f64> = s
        .split(',')
        .map(|x| x.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| "a box is box:<west>,<south>,<east>,<north> in degrees".to_string())?;
    if v.len() != 4 {
        return Err("a box is box:<west>,<south>,<east>,<north> in degrees".into());
    }
    if !(-180.0 <= v[0] && v[0] < v[2] && v[2] <= 180.0 && -90.0 <= v[1] && v[1] < v[3] && v[3] <= 90.0) {
        return Err("a box's west is under its east and its south under its north, within ±180°, ±90°".into());
    }
    Ok([0, 1, 2, 3].map(|i| (v[i] * 1e7).round() as i64))
}

fn parse_part(v: &toml::Value) -> Result<Part, Fault> {
    match v {
        toml::Value::String(s) if s == "earth" => Ok(Part::Earth),
        toml::Value::String(s) if s == "rest" => Ok(Part::Rest),
        toml::Value::String(s) => {
            let (kind, rest) = s.split_once(':').ok_or_else(|| {
                fault(
                    "part",
                    format!(
                        "{s:?}: a part is osm:<relation>, shape:<name>.geojson, region:<id>, or a table (buffer, cut)"
                    ),
                )
            })?;
            match kind {
                "osm" => rest.parse::<u64>().ok().filter(|id| *id > 0 && !rest.starts_with('0')).map(Part::Osm).ok_or_else(|| fault("part", format!("{s:?}: not a relation id"))),
                "shape" if valid_shape_name(rest) => Ok(Part::Shape(rest.into())),
                "shape" => Err(fault("part", format!("{s:?}: a shape is <name>.geojson, a file of the folder"))),
                "region" if valid_id(rest) => Ok(Part::Region(rest.into())),
                "region" => Err(fault("part", format!("{s:?}: not a region's id"))),
                "osm-way" | "eez" | "ts12" | "ne-lake" => {
                    let id = rest.parse::<u64>().ok().filter(|id| *id > 0 && !rest.starts_with('0')).ok_or_else(|| fault("part", format!("{s:?}: not an id")))?;
                    Ok(match kind {
                        "osm-way" => Part::OsmWay(id),
                        "eez" => Part::Eez(id),
                        "ts12" => Part::Ts12(id),
                        _ => Part::NeLake(id),
                    })
                }
                "box" => parse_box(rest).map(|b| Part::Box(b, rest.to_string())).map_err(|e| fault("part", format!("{s:?}: {e}"))),
                "land" if valid_id(rest) => Ok(Part::Land(rest.into())),
                "land" => Err(fault("part", format!("{s:?}: not a region's id"))),
                "group" if !rest.is_empty() && rest.split('/').all(|c| !c.is_empty() && c != "." && c != "..") => Ok(Part::Group(rest.trim_end_matches('/').into())),
                "group" => Err(fault("part", format!("{s:?}: a group is a folder's path, as group:europe/portugal"))),
                "geofabrik" | "poly" | "place" => Err(fault("part", format!("{s:?}: {kind}: is no longer a kind of part (convert the shape to GeoJSON and name it shape:<name>.geojson: docs/inputs.md §6.1.3)"))),
                _ => Err(fault("part", format!("{s:?}: unknown kind {kind:?} (osm:, osm-way:, shape:, region:, land:, group:, eez:, ts12:, ne-lake:, box:)"))),
            }
        }
        toml::Value::Table(t) => {
            let keys: Vec<&str> = t.keys().map(String::as_str).collect();
            if t.contains_key("buffer") {
                if let Some(k) = keys.iter().find(|k| !["buffer", "by"].contains(k)) {
                    return Err(fault("toml", format!("a buffer has no field {k:?} (buffer, by)")));
                }
                let by = t
                    .get("by")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| fault("part", "a buffer needs by = \"<distance>\""))?;
                let m = parse_distance(by).map_err(|e| fault("part", e))?;
                let of = match t.get("buffer") {
                    Some(toml::Value::String(s)) if s.starts_with("line:") => {
                        Target::Line(s["line:".len()..].to_string())
                    }
                    Some(v) => Target::Part(parse_part(v)?),
                    None => unreachable!(),
                };
                Ok(Part::Buffer { of: Box::new(of), m, by: by.trim().to_string() })
            } else if t.contains_key("cut") {
                if let Some(k) = keys.iter().find(|k| !["cut", "by", "keep"].contains(k)) {
                    return Err(fault("toml", format!("a cut has no field {k:?} (cut, by, keep)")));
                }
                let line = t
                    .get("by")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.strip_prefix("line:"))
                    .ok_or_else(|| fault("part", "a cut needs by = \"line:<name>\""))?;
                let left = match t.get("keep").and_then(|v| v.as_str()) {
                    Some("left") => true,
                    Some("right") => false,
                    _ => return Err(fault("part", "a cut needs keep = \"left\" or \"right\"")),
                };
                let of = parse_part(t.get("cut").unwrap())?;
                Ok(Part::Cut { of: Box::new(of), line: line.to_string(), left })
            } else if let Some(op) = ["union", "minus", "intersect"].iter().find(|k| t.contains_key(**k)) {
                if t.len() != 1 {
                    return Err(fault("toml", format!("{{ {op} = [...] }} has no other field")));
                }
                let Some(vs) = t[*op].as_array().filter(|a| !a.is_empty()) else {
                    return Err(fault("part", format!("{{ {op} = [...] }}: a non-empty list of parts")));
                };
                let ps = vs.iter().map(parse_part).collect::<Result<Vec<_>, _>>()?;
                Ok(match *op {
                    "union" => Part::Union(ps),
                    "minus" => Part::Minus(ps),
                    _ => Part::Intersect(ps),
                })
            } else if t.contains_key("nearest") {
                if let Some(k) = keys.iter().find(|k| !["nearest", "to", "than", "step"].contains(k)) {
                    return Err(fault("toml", format!("a nearest has no field {k:?} (nearest, to, than, step)")));
                }
                let list = |k: &str| -> Result<Vec<Part>, Fault> {
                    let Some(vs) = t.get(k).and_then(|v| v.as_array()).filter(|a| !a.is_empty()) else {
                        return Err(fault("part", format!("a nearest needs {k} = [<parts>]")));
                    };
                    vs.iter().map(parse_part).collect()
                };
                let by = t.get("step").and_then(|v| v.as_str()).unwrap_or(DEFAULT_STEP).trim().to_string();
                let step_m = parse_distance(&by).map_err(|e| fault("part", e))?;
                if !(MIN_STEP_M..=MAX_STEP_M).contains(&step_m) {
                    return Err(fault("part", format!("a nearest's step {by:?}: 100 m to 20 km")));
                }
                Ok(Part::Nearest {
                    of: Box::new(parse_part(&t["nearest"])?),
                    to: list("to")?,
                    than: list("than")?,
                    step_m,
                    by,
                })
            } else {
                Err(fault(
                    "part",
                    format!(
                        "a table part is {{ buffer = …, by = … }}, {{ cut = …, by = …, keep = … }}, {{ union | minus | intersect = [...] }} or {{ nearest = …, to = [...], than = [...] }}, not {{ {} }}",
                        keys.join(", ")
                    ),
                ))
            }
        }
        other => Err(fault("toml", format!("a part is a string or a table, not {}", other.type_str()))),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegion {
    id: String,
    name: String,
    #[serde(default)]
    outline: Vec<toml::Value>,
    water: Option<Vec<toml::Value>>,
    #[serde(default)]
    hand_out: Option<String>,
    #[serde(default)]
    hand_out_all: bool,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    minus: Vec<toml::Value>,
    #[serde(default)]
    intersect: Vec<toml::Value>,
    #[serde(default)]
    lines: BTreeMap<String, RawLine>,
    #[serde(default)]
    pins: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLine {
    coords: Option<Vec<[f64; 2]>>,
    ways: Option<Vec<u64>>,
}

/// Pins, checked: each value a non-empty string, `osm` (when there) a date.
fn pins_of(raw: BTreeMap<String, toml::Value>) -> Result<BTreeMap<String, String>, Fault> {
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        let Some(s) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(fault("pin", format!("pin {k:?}: a non-empty string")));
        };
        if k == "osm" && !crate::osmpass::is_date(s) {
            return Err(fault("pin", format!("pin osm = {s:?}: the OSM pass's date (YYYY-MM-DD)")));
        }
        out.insert(k, s.to_string());
    }
    Ok(out)
}

/// A folder's group (`_group.toml`): the name the map's Regions list shows it by, and its place
/// among its sibling folders (lower first; without one, after those with one, by name).
#[derive(Clone, Debug, PartialEq, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
}

/// `_group.toml`, parsed: a name that isn't empty, an integer order if any.
pub fn parse_group(text: &str) -> Result<Group, Fault> {
    let g: Group = toml::from_str(text).map_err(|e| fault("group", format!("_group.toml: {}", e.message())))?;
    if g.name.trim().is_empty() {
        return Err(fault("group", "_group.toml: name is empty"));
    }
    Ok(g)
}

/// `pins.toml`, parsed.
pub fn parse_pins(text: &str) -> Result<BTreeMap<String, String>, Fault> {
    let raw: BTreeMap<String, toml::Value> =
        toml::from_str(text).map_err(|e| fault("toml", format!("pins.toml: {}", e.message())))?;
    pins_of(raw)
}

/// A region file, parsed and checked as a file (§6.1.8: `rg-toml`, `rg-id`, `rg-part`, `rg-line`,
/// `rg-pin`): its name, `<id>.toml`, is its id's. Every fault, not only the first.
pub fn parse(file_name: &str, text: &str) -> Result<Region, Vec<Fault>> {
    let raw: RawRegion = toml::from_str(text).map_err(|e| vec![fault("toml", e.message().to_string())])?;
    let mut faults = Vec::new();
    if !valid_id(&raw.id) {
        faults.push(fault("id", format!("id {:?}: lower-case letters, digits and dashes, 1–64 of them", raw.id)));
    } else if file_name.strip_suffix(".toml") != Some(raw.id.as_str()) {
        faults.push(fault(
            "id",
            format!("the file is {file_name} but its id is {:?}: a region's file is <id>.toml", raw.id),
        ));
    }
    if raw.name.trim().is_empty() {
        faults.push(fault("id", "no name"));
    }
    if raw.outline.is_empty() && raw.water.as_ref().is_none_or(Vec::is_empty) {
        faults.push(fault("id", "no outline (nor water)"));
    }
    let mut list = |vs: &[toml::Value]| -> Vec<Part> {
        vs.iter().filter_map(|v| parse_part(v).map_err(|f| faults.push(f)).ok()).collect()
    };
    let outline = list(&raw.outline);
    let minus = list(&raw.minus);
    let intersect = list(&raw.intersect);
    let water_key = raw.water.is_some();
    let water = list(raw.water.as_deref().unwrap_or_default());
    let mut lines = BTreeMap::new();
    for (name, l) in raw.lines {
        if !valid_id(&name) {
            faults.push(fault(
                "line",
                format!("line {name:?}: a line's name is as an id (lower-case letters, digits, dashes)"),
            ));
            continue;
        }
        match (l.coords, l.ways) {
            (Some(c), None) => {
                let pts: Vec<[i64; 2]> =
                    c.iter().map(|p| [(p[0] * 1e7).round() as i64, (p[1] * 1e7).round() as i64]).collect();
                let mut distinct = pts.clone();
                distinct.dedup();
                if distinct.len() < 2 {
                    faults.push(fault("line", format!("line {name}: at least two distinct points")));
                } else if c.iter().any(|p| !(-180.0..=180.0).contains(&p[0]) || !(-90.0..=90.0).contains(&p[1])) {
                    faults.push(fault(
                        "line",
                        format!("line {name}: a point outside longitude −180…180, latitude −90…90 (longitude first)"),
                    ));
                } else {
                    lines.insert(name, Line::Coords(pts));
                }
            }
            (None, Some(w)) if !w.is_empty() => {
                lines.insert(name, Line::Ways(w));
            }
            (None, Some(_)) => faults.push(fault("line", format!("line {name}: ways = [] names no way"))),
            _ => faults.push(fault("line", format!("line {name}: either coords or ways, one of them"))),
        }
    }
    let pins = pins_of(raw.pins).map_err(|f| faults.push(f)).unwrap_or_default();
    if !faults.is_empty() {
        return Err(faults);
    }
    if raw.hand_out.is_some() && !water_key {
        faults.push(fault("part", "hand_out is for a region with water"));
    }
    if raw.hand_out_all && raw.hand_out.is_none() {
        faults.push(fault("part", "hand_out_all is for a region with hand_out"));
    }
    let r = Region {
        id: raw.id,
        name: raw.name,
        outline,
        minus,
        intersect,
        water,
        water_key,
        hand_out: raw.hand_out.map(|h| h.trim_end_matches('/').to_string()),
        hand_out_all: raw.hand_out_all,
        disabled: raw.disabled,
        lines,
        pins,
    };
    // `rest` only in water; `land:`/`group:` naming the region itself.
    for p in r.outline.iter().chain(&r.minus).chain(&r.intersect) {
        let mut v = Vec::new();
        p.walk(&mut v);
        if v.iter().any(|q| matches!(q, Part::Rest)) {
            return Err(vec![fault("part", "rest is only in water: what no other region covers")]);
        }
    }
    // (Its own land only in its water: the land is resolved first.)
    let mut own = Vec::new();
    for p in r.outline.iter().chain(&r.minus).chain(&r.intersect) {
        p.walk(&mut own);
    }
    let names_self = |p: &&Part| matches!(p, Part::Land(id) | Part::Region(id) if *id == r.id);
    if own.iter().any(names_self) || r.regions().contains(&r.id.as_str()) {
        return Err(vec![fault("ref", format!("{} names itself (only its water may name its own land:)", r.id))]);
    }
    // Lines named by parts that the file doesn't have.
    let missing: Vec<&str> = r.used_lines().into_iter().filter(|l| !r.lines.contains_key(*l)).collect();
    if !missing.is_empty() {
        return Err(vec![fault("ref", format!("line:{} names no line of the file", missing.join(", line:")))]);
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_examples_parse() {
        let r = parse("spain-mainland.toml", "id = \"spain-mainland\"\nname = \"Mainland Spain\"\noutline = [\"osm:1311341\"]\nminus = [\"osm:349000\", \"region:canaries\"]\n").unwrap();
        assert_eq!(r.minus, vec![Part::Osm(349000), Part::Region("canaries".into())]);
        assert!(!r.is_plain());
        let r = parse(
            "azores.toml",
            "id = \"azores\"\nname = \"Azores\"\noutline = [{ buffer = \"osm:1629145\", by = \"12 nmi\" }]\n",
        )
        .unwrap();
        assert_eq!(r.outline_text(), vec!["{buffer osm:1629145 12 nmi}"]);
        let r = parse("spain-north.toml", "id = \"spain-north\"\nname = \"Northern Spain\"\noutline = [{ cut = \"region:spain-mainland\", by = \"line:split\", keep = \"left\" }]\n[lines.split]\ncoords = [[-9.8, 42.0], [3.6, 41.6]]\n").unwrap();
        assert_eq!(r.lines["split"], Line::Coords(vec![[-98_000_000, 420_000_000], [36_000_000, 416_000_000]]));
        assert_eq!(r.outline_text(), vec!["{cut region:spain-mainland line:split left}"]);
        let r = parse("tagus-banks.toml", "id = \"tagus-banks\"\nname = \"Tagus banks\"\noutline = [{ buffer = \"line:river\", by = \"3 km\" }]\nintersect = [\"osm:295480\"]\n[lines.river]\nways = [24046327, 24046330, 155883810]\n").unwrap();
        assert_eq!(r.ways(), vec![24046327, 24046330, 155883810]);
        let r = parse("portugal.toml", "id = \"portugal\"\nname = \"Portugal\"\noutline = [\"osm:295480\"]\n").unwrap();
        assert!(r.is_plain());
        assert_eq!(r.outline_text(), vec!["osm:295480"]);
    }

    #[test]
    fn faults_are_named() {
        let check = |name: &str, text: &str| parse(name, text).unwrap_err().iter().map(|f| f.check).collect::<Vec<_>>();
        assert_eq!(check("a.toml", "id = \"a\"\nname = \"A\"\noutline = [\"osm:1\"]\nminsu = []\n"), vec!["toml"]);
        assert_eq!(check("b.toml", "id = \"a\"\nname = \"A\"\noutline = [\"osm:1\"]\n"), vec!["id"]);
        assert_eq!(
            check(
                "a.toml",
                "id = \"a\"\nname = \"A\"\noutline = [\"poly:x.poly\", \"place:1,2,3\", \"geofabrik:europe\"]\n"
            ),
            vec!["part", "part", "part"]
        );
        assert_eq!(
            check("a.toml", "id = \"a\"\nname = \"A\"\noutline = [{ buffer = \"osm:1\", by = \"400 km\" }]\n"),
            vec!["part"]
        );
        assert_eq!(
            check(
                "a.toml",
                "id = \"a\"\nname = \"A\"\noutline = [{ cut = \"osm:1\", by = \"line:x\", keep = \"up\" }]\n"
            ),
            vec!["part"]
        );
        assert_eq!(
            check(
                "a.toml",
                "id = \"a\"\nname = \"A\"\noutline = [{ cut = \"osm:1\", by = \"line:x\", keep = \"up\" }]\n"
            ),
            vec!["part"]
        );
        assert_eq!(
            check(
                "a.toml",
                "id = \"a\"\nname = \"A\"\noutline = [{ cut = \"osm:1\", by = \"line:x\", keep = \"left\" }]\n"
            ),
            vec!["ref"]
        );
        assert_eq!(
            check("a.toml", "id = \"a\"\nname = \"A\"\noutline = [\"osm:1\"]\n[lines.x]\ncoords = [[1, 2], [1, 2]]\n"),
            vec!["line"]
        );
        assert_eq!(
            check("a.toml", "id = \"a\"\nname = \"A\"\noutline = [\"osm:1\"]\n[pins]\nosm = \"yesterday\"\n"),
            vec!["pin"]
        );
        assert_eq!(check("a.toml", "id = \"a\"\nname = \"\"\noutline = []\n"), vec!["id", "id"]);
        assert_eq!(parse_distance("12 nmi"), Ok(22224.0));
        assert_eq!(parse_distance("500m"), Ok(500.0));
        assert!(parse_distance("-1 km").is_err() && parse_distance("12").is_err() && parse_distance("1 mi").is_err());
        assert_eq!(parse_pins("osm = \"2026-09-29\"\ngadm = \"4.1\"\n").unwrap()["gadm"], "4.1");
    }
}
