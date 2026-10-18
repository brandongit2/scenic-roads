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

/// A part of `outline`, `minus` or `intersect` (§6.1.3).
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// An OSM relation's area.
    Osm(u64),
    /// A Polygon or MultiPolygon in a GeoJSON file of the folder, by its file name.
    Shape(String),
    /// Another region's area, as stored.
    Region(String),
    /// Everything within `m` metres (`by` as written) of a part or a line.
    Buffer { of: Box<Target>, m: f64, by: String },
    /// The part of a shape on one side of a line.
    Cut { of: Box<Part>, line: String, left: bool },
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

impl Part {
    /// Whether it's an operation (not a plain `osm:` or `shape:` part).
    pub fn is_plain(&self) -> bool {
        matches!(self, Part::Osm(_) | Part::Shape(_))
    }

    /// Every part within it, itself first.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Part>) {
        out.push(self);
        match self {
            Part::Buffer { of, .. } => {
                if let Target::Part(p) = &**of {
                    p.walk(out);
                }
            }
            Part::Cut { of, .. } => of.walk(out),
            _ => {}
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
            _ => {}
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
    pub lines: BTreeMap<String, Line>,
    /// Its own `[pins]`, overriding `pins.toml`'s.
    pub pins: BTreeMap<String, String>,
}

impl Region {
    /// Whether it's stored as its parts unchanged (§6.1.4): only `osm:` and `shape:` parts in
    /// `outline`, no `minus` nor `intersect`.
    pub fn is_plain(&self) -> bool {
        self.minus.is_empty() && self.intersect.is_empty() && self.outline.iter().all(Part::is_plain)
    }

    /// Every part, nested ones included.
    pub fn parts(&self) -> Vec<&Part> {
        let mut out = Vec::new();
        for p in self.outline.iter().chain(&self.minus).chain(&self.intersect) {
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

    /// The ways its lines name (sorted, once each).
    pub fn ways(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .lines
            .values()
            .filter_map(|l| if let Line::Ways(w) = l { Some(w.clone()) } else { None })
            .flatten()
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
        for p in self.outline.iter().chain(&self.minus).chain(&self.intersect) {
            p.lines(&mut v);
        }
        v.sort();
        v.dedup();
        v
    }

    /// Its outline entries as text, as a catalog records them and `outline_digest` hashes them: the
    /// same strings as a recipe's for `osm:` parts.
    pub fn outline_text(&self) -> Vec<String> {
        self.outline.iter().map(|p| p.to_string()).collect()
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

/// A distance: a number and `m`, `km` or `nmi`, positive, at most 200 km; in metres.
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
        return Err(format!("{s:?}: a distance is more than nothing and at most 200 km"));
    }
    Ok(m)
}

fn parse_part(v: &toml::Value) -> Result<Part, Fault> {
    match v {
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
                "geofabrik" | "poly" | "place" => Err(fault("part", format!("{s:?}: {kind}: is no longer a kind of part (convert the shape to GeoJSON and name it shape:<name>.geojson: docs/inputs.md §6.1.3)"))),
                _ => Err(fault("part", format!("{s:?}: unknown kind {kind:?} (osm:, shape:, region:)"))),
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
            } else {
                Err(fault(
                    "part",
                    format!(
                        "a table part is {{ buffer = …, by = … }} or {{ cut = …, by = …, keep = … }}, not {{ {} }}",
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
    outline: Vec<toml::Value>,
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
    if raw.outline.is_empty() {
        faults.push(fault("id", "no outline"));
    }
    let mut list = |vs: &[toml::Value]| -> Vec<Part> {
        vs.iter().filter_map(|v| parse_part(v).map_err(|f| faults.push(f)).ok()).collect()
    };
    let outline = list(&raw.outline);
    let minus = list(&raw.minus);
    let intersect = list(&raw.intersect);
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
    let r = Region { id: raw.id, name: raw.name, outline, minus, intersect, lines, pins };
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
            check("a.toml", "id = \"a\"\nname = \"A\"\noutline = [{ buffer = \"osm:1\", by = \"300 km\" }]\n"),
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
