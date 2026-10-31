//! The to-do lists (docs/names.md), written after each build by the `names-todo` job:
//!
//! - **`inputs/translations/todo/<language>.jsonl`:** the names in the coverage that something has with no
//!   English of its own and no line in any language spoken where it is, one entry per name, kind
//!   and first candidate language, sorted by priority; with `README.md`, the translators' brief,
//!   and `check.py`, its checker.
//! - **`inputs/descriptions/todo/`:** the landmarks (`landmarks.jsonl`) and parks and protected areas
//!   (`areas.jsonl`) with an English Wikipedia article or a register entry and no description,
//!   sorted by fame; with `README.md`, the writers' brief.
//!
//! What it reads, all from the newest catalogue:
//! - **Labels** (places, states, water, parks): the labels layer's zoom-12 tiles in the units' z6
//!   tiles, each label inside the coverage the catalogue records once (its feature id). Own English:
//!   `en`, else `kana` romanised; OSM's languages: `l`.
//! - **Roads, rail lines and ferries:** each named way placed once, at the centre of its box, from
//!   the hidata's ways-here index (its owner and index there), its owner unit's base pack (ways,
//!   names) and roads' own English (`global/roaden/<u>`) read once a record names it; by pieces of
//!   16 hidata files, each's ways placed kept (task #897, `resume_log::Pieces` in the job's
//!   scratch, a safe point): a resumed run replays them and reads only the rest. Roads carry no
//!   OSM language tags yet.
//! - **Landmarks:** the markdata's points (all eight kinds), their `en`, fame, and popup records
//!   (Wikidata item, Wikipedia article, register entry, written description).
//! - **Parks:** the ovdata's park records.
//! - The languages spoken where: the catalogue's outlines (`names::spoken`), kept in the scratch.
//! - The translations' and the descriptions' accepted versions (docs/inputs.md §6.3, §6.4), as the
//!   agent names them (their indexes' content names, in the job's key): never the inputs folders.

use anyhow::{Context, Result};
use names::{Kind, Lang, Names, Spoken};
use roadcore::{class, WayRec};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use store::range::PlainFile;
use store::sect::SectReader;

/// The job's version: in its key, so a change to what it writes runs it again.
pub const VERSION: u32 = 1;

/// The translators' brief, `translations/todo/README.md`.
pub const TRANSLATORS: &str = include_str!("../../../tools/names/translations-todo.md");
/// Its checker, `translations/todo/check.py`.
pub const CHECK: &str = crate::inputs::lint::CHECK;
/// The writers' brief, `descriptions/todo/README.md`.
pub const WRITERS: &str = include_str!("../../../tools/names/descriptions-todo.md");

/// A named thing, as the lists see it.
struct Thing<'a> {
    name: &'a str,
    kind: Kind,
    /// It has English of its own.
    own: bool,
    osm: Vec<Lang>,
    lon: f64,
    lat: f64,
    /// Its OSM object (n123, w123, r123), or what else names it.
    id: String,
    priority: f64,
}

/// One entry of a list, as it's gathered.
#[derive(Default)]
struct Entry {
    langs: Vec<Lang>,
    things: u64,
    best: f64,
    example: (String, f64, f64),
}

/// What the lists came to.
#[derive(Debug, Default)]
pub struct Report {
    /// Entries per language.
    pub langs: BTreeMap<String, usize>,
    /// Things read, by source.
    pub read: BTreeMap<&'static str, u64>,
    /// Things with English of their own; with a line; in English (or no letters); nowhere a
    /// language is known.
    pub own: u64,
    pub lined: u64,
    /// Settlements in Latin script, which keep their own name.
    pub settlements: u64,
    pub english: u64,
    pub nowhere: u64,
    /// Descriptions to write: landmarks, areas.
    pub landmarks: usize,
    pub areas: usize,
}

/// The lists being gathered.
struct Lists<'a> {
    names: &'a Names,
    spoken: &'a Spoken,
    entries: HashMap<(String, Kind, Lang), Entry>,
    report: Report,
}

const CJK_LANGS: [&str; 6] = ["ja", "zh", "yue", "nan", "hak", "ko"];

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0xac00..=0xd7af | 0x20000..=0x2ffff)
}

fn is_kana(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff)
}

/// Words that mark a Latin-script name as in another language than English: generic words and
/// articles of the coverage's languages (tools/names/check.py's, and the roads' own).
const OTHER_WORDS: &[&str] = &[
    // French
    "lac", "lacs", "rivière", "riviere", "fleuve", "ruisseau", "étang", "etang", "baie", "anse", "cap", "pointe", "île",
    "ile", "îles", "iles", "mont", "monts", "montagne", "col", "pic", "forêt", "foret", "parc", "réserve", "chute",
    "chutes", "vallée", "vallee", "plage", "marais", "château", "chateau", "église", "eglise", "chapelle", "pont",
    "gare", "musée", "musee", "moulin", "rue", "chemin", "rang", "côte", "cote", "allée", "allee", "impasse", "de",
    "du", "des", "la", "le", "les", "aux", "et", "saint", "sainte",
    // Spanish, Portuguese, Catalan, Galician
    "río", "rio", "lago", "laguna", "embalse", "sierra", "monte", "isla", "playa", "parque", "pico", "puerto",
    "castillo", "iglesia", "puente", "calle", "carrera", "camino", "avenida", "plaza", "del", "el", "los", "las", "y",
    "lagoa", "serra", "ilha", "praia", "castelo", "igreja", "rua", "estrada", "praça", "da", "do", "dos", "das",
    "carrer", "riu", "camí", "plaça", "dels", "rúa", "san", "santa", "santo", "são",
    // Welsh, Irish, Scottish Gaelic
    "afon", "llyn", "mynydd", "coed", "eglwys", "ffordd", "heol", "stryd", "lôn", "nant", "pen", "bryn", "cwm",
    "sliabh", "abhainn", "inis", "oileán", "bóthar", "sráid", "baile", "cnoc", "rathad", "sràid", "beinn", "gleann",
    "allt", "eilean",
];

/// Words that mark a Latin-script name as English where English isn't spoken: its articles and
/// joining words, and the generic words of the names English-language registers and maps give
/// ("Temples, Gardens and Archaeological Sites", "a Historic International Settlement").
const ENGLISH_WORDS: &[&str] = &[
    "the",
    "of",
    "and",
    "in",
    "on",
    "at",
    "lake",
    "river",
    "mount",
    "mountain",
    "mountains",
    "island",
    "islands",
    "bay",
    "park",
    "national",
    "historic",
    "historical",
    "site",
    "sites",
    "temple",
    "temples",
    "shrine",
    "garden",
    "gardens",
    "castle",
    "church",
    "cathedral",
    "museum",
    "bridge",
    "station",
    "line",
    "street",
    "road",
    "avenue",
    "tower",
    "house",
    "hall",
    "palace",
    "ruins",
    "settlement",
    "monument",
    "memorial",
    "district",
    "town",
    "city",
    "village",
    "world",
    "heritage",
    "international",
    "archaeological",
    "falls",
    "beach",
    "harbour",
    "harbor",
    "port",
    "trail",
    "valley",
    "forest",
    "reserve",
    "observatory",
    "school",
    "university",
    "airport",
    "market",
    "square",
    "centre",
    "center",
    "building",
    "estate",
    "farm",
    "mill",
    "lighthouse",
    "viewpoint",
    "peak",
    "hill",
    "point",
    "cape",
    "north",
    "south",
    "east",
    "west",
    "old",
    "new",
    "upper",
    "lower",
    "great",
    "little",
];

/// Whether a name reads as English: Latin letters without accents, and more English words than
/// the coverage's other languages' (`ENGLISH_WORDS`, `OTHER_WORDS`), or, where English is spoken
/// (`english_spoken`), none of the others'. A name in CJK script with a Latin part (Hong Kong's "文武廟 Man Mo Temple Compound") reads
/// as English when its Latin part does: it carries its English.
fn reads_english(name: &str, english_spoken: bool) -> bool {
    let latin: String = if name.chars().any(is_cjk) {
        let l: String = name.chars().filter(|c| !is_cjk(*c)).collect();
        if l.chars().filter(|c| c.is_ascii_alphabetic()).count() < 4 {
            return false;
        }
        l
    } else {
        name.to_owned()
    };
    if latin.chars().any(|c| c.is_alphabetic() && !c.is_ascii()) {
        return false;
    }
    let words: Vec<String> =
        latin.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    let other = words.iter().filter(|w| OTHER_WORDS.contains(&w.as_str())).count();
    let english = words.iter().filter(|w| ENGLISH_WORDS.contains(&w.as_str())).count();
    // More English words than another language's ("The Architectural Work of Le Corbusier…");
    // else, where English is spoken, none of another language's.
    english > other || (english_spoken && other == 0)
}

impl Lists<'_> {
    /// Adds a thing: on a list when it lacks English.
    fn add(&mut self, t: Thing) {
        if t.own {
            self.report.own += 1;
            return;
        }
        // Nothing to translate in a name without letters ("66-91").
        if !t.name.chars().any(char::is_alphabetic) {
            self.report.english += 1;
            return;
        }
        let here = self.spoken.langs_at(t.lon, t.lat);
        if self.names.translation(t.kind, t.name, &names::spoken::lookup_order(&t.osm, here)).is_some() {
            self.report.lined += 1;
            return;
        }
        let en = Lang::parse("en").expect("en");
        // Candidates: OSM's languages where it gives some, else those spoken here; narrowed by the
        // name's script.
        let mut cands = if t.osm.is_empty() { here.to_vec() } else { names::spoken::lookup_order(&t.osm, &[]) };
        let cjk_lang = |l: &Lang| CJK_LANGS.contains(&l.as_str());
        if t.name.chars().any(is_cjk) {
            let mut c: Vec<Lang> = cands.iter().copied().filter(cjk_lang).collect();
            if t.name.chars().any(is_kana) {
                c.retain(|l| l.as_str() == "ja");
            }
            if !c.is_empty() {
                cands = c;
            }
        } else {
            let c: Vec<Lang> = cands.iter().copied().filter(|l| !cjk_lang(l) && l.as_str() != "ta").collect();
            if !c.is_empty() {
                cands = c;
            }
        }
        if cands.is_empty() {
            self.report.nowhere += 1;
            return;
        }
        // A settlement in Latin script keeps its own name (a well-known English one would be its
        // own English): nothing to ask.
        if t.kind == Kind::Settlement
            && !t.name.chars().any(is_cjk)
            && t.name
                .chars()
                .all(|c| !c.is_alphabetic() || c.is_ascii() || c.to_lowercase().all(|l| (l as u32) < 0x250))
        {
            self.report.settlements += 1;
            return;
        }
        // Where English is spoken, a name without another language's signs is English; elsewhere,
        // one with more English words than another language's.
        if reads_english(t.name, here.contains(&en) || cands.contains(&en)) {
            self.report.english += 1;
            return;
        }
        if cands.contains(&en) && cands.len() > 1 {
            cands.retain(|l| *l != en);
        }
        let e = self.entries.entry((t.name.to_owned(), t.kind, cands[0])).or_default();
        for l in cands {
            if !e.langs.contains(&l) {
                e.langs.push(l);
            }
        }
        e.things += 1;
        if t.priority > e.best || e.example.0.is_empty() {
            e.best = t.priority;
            e.example = (t.id, t.lon, t.lat);
        }
    }
}

/// A catalogue file's path under the root.
fn path_of(
    root: &store::nasfs::NasRoot,
    cat: &store::catalogue::Catalogue,
    logical: &str,
) -> Option<store::nasfs::NasPath> {
    cat.file(logical).map(|f| root.join(&f.file))
}

fn sect<'a>(path: impl Into<store::nasfs::Loc<'a>>) -> Result<SectReader<PlainFile>> {
    let path = path.into();
    SectReader::open(path.open_plain().with_context(|| format!("open {}", path.display()))?)
        .with_context(|| format!("read {}", path.display()))
}

/// The spoken languages the catalogue was made with: its `global/spoken` (the `spoken` job's, from
/// the languages input's accepted version).
pub fn spoken_for(root: &store::nasfs::NasRoot, cat: &store::catalogue::Catalogue) -> Result<Spoken> {
    let f =
        cat.global.get("spoken").and_then(|l| cat.file(l)).context(
            "the catalogue has no global/spoken (the spoken job makes it from the languages' accepted version)",
        )?;
    let b = root.join(&f.file).read().with_context(|| format!("read {}", f.file))?;
    Spoken::from_bytes(&b).with_context(|| format!("read {}", f.file))
}

/// What `run` reads of the map, as the map log's watches (docs/maplog.md §6.8; store::map::marks):
/// the units and the regions it records, the languages spoken, the labels' hi packs, the base packs,
/// the units' own-road lists, the hi data, the landmark points and the area overlays, each section's
/// keys and their files (`f/` and the logical names those sections give, as the steps name them).
pub const INPUTS: [&str; 17] = [
    "u/",
    "m/units",
    "r/",
    "m/regions",
    "s/global/spoken",
    "f/global/spoken",
    "l/labels/hi/",
    "f/layers/labels/hi/",
    "s/base/",
    "f/base/",
    "f/global/roaden/",
    "s/hidata/",
    "f/hidata/",
    "s/markdata/",
    "f/markdata/",
    "s/ovdata/",
    "f/ovdata/",
];

/// The agent's key of what `run` reads (crate::agent::build::names_todo_work): the `seq` of the
/// last batch that touched any of `INPUTS`, by the log's `marks`; None before the log has any. A
/// batch that changed none of them (most: a depth or terrain tile alone) leaves the lists as they
/// are, so they aren't made again. A file the map's `NAMING` sections name whose `f/` key no prefix
/// of `INPUTS` covers (a step naming its outputs otherwise) makes the key the map's own `seq`: the
/// lists made again at each batch, said, rather than a change of that file missed.
pub fn inputs_seq(marks: &BTreeMap<String, store::map::Mark>, map: Option<&store::map::Map>) -> Option<String> {
    if let Some((m, f)) = map.and_then(|m| Some((m, unwatched(m)?))) {
        eprintln!("names-todo: {f} isn't under the inputs' watches (namestodo::INPUTS): the lists made at every batch");
        return Some(format!("map {}", m.seq));
    }
    INPUTS.iter().filter_map(|w| marks.get(*w)).map(|m| m.seq).max().map(|s| format!("map {s}"))
}

/// The sections of `INPUTS` whose values name files (logical names, each at `f/<name>`).
const NAMING: [&str; 6] = ["s/global/spoken", "l/labels/hi/", "s/base/", "s/hidata/", "s/markdata/", "s/ovdata/"];

/// The first `f/` key a section of `NAMING` names that no prefix of `INPUTS` covers.
fn unwatched(map: &store::map::Map) -> Option<String> {
    NAMING.iter().find_map(|sec| {
        map.entries()
            .range::<str, _>((std::ops::Bound::Included(*sec), std::ops::Bound::Unbounded))
            .take_while(|(k, _)| k.starts_with(sec))
            .filter_map(|(_, v)| v.as_str())
            .map(|v| format!("f/{v}"))
            .find(|f| !INPUTS.iter().any(|w| f.starts_with(w)))
    })
}

/// What `run` reads of catalogue `cat`, by content (`INPUTS`, as `run` reads them): the key of its
/// roads' pieces (crate::resume_log), so a run taken up again goes on only on the same inputs.
pub fn inputs_key(cat: &store::catalogue::Catalogue) -> String {
    let mut parts: Vec<String> = vec![format!("names-todo inputs {VERSION}")];
    parts.extend(cat.units.iter().cloned());
    parts.push(cat.coverage.get("regions").map(|r| r.to_string()).unwrap_or_default());
    let mut logical: Vec<&str> = Vec::new();
    logical.extend(cat.global.get("spoken").map(String::as_str));
    if let Some(l) = cat.layers.get("labels") {
        logical.extend(l.hi.values().map(String::as_str));
    }
    logical.extend(cat.base.values().map(String::as_str));
    logical.extend(cat.files.keys().filter(|k| k.starts_with("global/roaden/")).map(String::as_str));
    logical.extend(cat.hidata.values().map(String::as_str));
    logical.extend(cat.markdata.values().map(String::as_str));
    logical.extend(cat.ovdata.values().map(String::as_str));
    for l in logical {
        parts.push(format!("{l} {}", cat.content(l).unwrap_or("")));
    }
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    crate::agent::build::h(&refs)
}

/// The units' z6 tiles.
fn unit_tiles(cat: &store::catalogue::Catalogue) -> HashSet<(u32, u32)> {
    cat.units
        .iter()
        .filter_map(|u| {
            let mut p = u.split('/');
            (p.next()? == "6").then_some(())?;
            Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
        })
        .collect()
}

fn tile_of(key: &str) -> Option<(u32, u32)> {
    let mut p = key.split('/');
    (p.next()? == "6").then_some(())?;
    Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
}

/// The coverage the catalogue records, as shapes for point tests (none: a catalogue that records none).
fn coverage(cat: &store::catalogue::Catalogue) -> Vec<crate::coverage::Shape> {
    let regions: Vec<crate::coverage::DrawnRegion> =
        cat.coverage.get("regions").and_then(|r| serde_json::from_value(r.clone()).ok()).unwrap_or_default();
    let e7 = |p: &[f64; 2]| [(p[0] * 1e7).round() as i32, (p[1] * 1e7).round() as i32];
    regions
        .iter()
        .flat_map(|r| r.shapes.iter().map(move |(entry, polys)| (format!("{}: {entry}", r.id), polys)))
        .filter(|(_, polys)| !polys.is_empty())
        .map(|(source, polys)| {
            crate::coverage::Shape::new(
                source,
                polys.iter().flatten().map(|ring| ring.iter().map(e7).collect()).collect(),
                0.0,
            )
        })
        .collect()
}

/// Labels: the zoom-12 tiles of the labels layer's hi packs in the units' tiles, the labels inside
/// the coverage (`cov`; empty: all).
fn labels(
    root: &store::nasfs::NasRoot,
    cat: &store::catalogue::Catalogue,
    units: &HashSet<(u32, u32)>,
    cov: &[crate::coverage::Shape],
    lists: &mut Lists,
    progress: &dyn Fn(&str, u64, u64),
) -> Result<()> {
    let Some(layer) = cat.layers.get("labels") else { return Ok(()) };
    let packs: Vec<&String> =
        layer.hi.iter().filter(|(k, _)| tile_of(k).is_some_and(|t| units.contains(&t))).map(|(_, v)| v).collect();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    for (i, logical) in packs.iter().enumerate() {
        progress("label packs read", i as u64, packs.len() as u64);
        let Some(path) = path_of(root, cat, logical) else { continue };
        let src = path.open_plain().with_context(|| format!("open {}", path.display()))?;
        let idx = store::pack::PackIndex::read_from(&src)?;
        for e in idx.entries.iter().filter(|e| e.zxy().0 == 12) {
            let (z, x, y) = e.zxy();
            let blob = idx.read_blob(&src, e)?;
            let raw = names::mvt::gunzip_if_gzip(&blob)?;
            let tile = names::mvt::Tile::decode(&raw)?;
            for l in tile.layers.iter().filter(|l| l.name == "l" && l.extent > 0) {
                for f in &l.features {
                    let tags: Vec<(&str, &str)> = f
                        .tags
                        .chunks_exact(2)
                        .filter_map(|p| {
                            Some((l.keys.get(p[0] as usize)?.as_str(), l.values.get(p[1] as usize)?.as_str()?))
                        })
                        .collect();
                    let num = |k: &str| {
                        f.tags.chunks_exact(2).find(|p| l.keys.get(p[0] as usize).is_some_and(|x| x == k)).and_then(
                            |p| match l.values.get(p[1] as usize)? {
                                names::mvt::Value::Double(v) => Some(*v),
                                names::mvt::Value::Float(v) => Some(f64::from(*v)),
                                names::mvt::Value::Int(v) | names::mvt::Value::Sint(v) => Some(*v as f64),
                                names::mvt::Value::Uint(v) => Some(*v as f64),
                                _ => None,
                            },
                        )
                    };
                    let get =
                        |k: &str| tags.iter().find(|(key, _)| *key == k).map(|(_, v)| *v).filter(|v| !v.is_empty());
                    let (Some(name), Some((px, py))) = (get("n"), f.first_point()) else { continue };
                    let h = {
                        use std::hash::{Hash, Hasher};
                        let mut s = std::hash::DefaultHasher::new();
                        name.hash(&mut s);
                        s.finish()
                    };
                    if !seen.insert((f.id.unwrap_or(u64::MAX), h)) {
                        continue;
                    }
                    let (lon, lat) =
                        names::mvt::tile_to_lonlat(u32::from(z), x, y, l.extent, f64::from(px), f64::from(py));
                    let p = [(lon * 1e7).round() as i32, (lat * 1e7).round() as i32];
                    if !cov.is_empty() && !cov.iter().any(|s| s.inside(p)) {
                        continue;
                    }
                    *lists.report.read.entry("labels").or_default() += 1;
                    let (k, c) = (get("k").unwrap_or_default(), get("c").unwrap_or_default());
                    let kind = if k == "place" { Kind::of_place(c) } else { Kind::Other };
                    let own = names::own::own_english(&tags, &["en"]).is_some();
                    let s = num("s").unwrap_or(0.0);
                    let priority = match k {
                        "place" => s,
                        "state" => 50.0 + s,
                        _ => 30.0 + s,
                    };
                    let id = get("o").map_or_else(|| format!("label {}", f.id.unwrap_or(0)), str::to_owned);
                    lists.add(Thing {
                        name,
                        kind,
                        own,
                        osm: names::own::osm_langs(name, &tags),
                        lon,
                        lat,
                        id,
                        priority,
                    });
                }
            }
        }
    }
    Ok(())
}

/// A road's priority by its class (rail lines 36).
fn road_priority(c: u8) -> f64 {
    match c {
        class::MOTORWAY => 45.0,
        class::TRUNK => 42.0,
        class::PRIMARY => 38.0,
        class::SECONDARY => 34.0,
        class::TERTIARY => 30.0,
        c if class::is_rail(c) => 36.0,
        class::FERRY => 32.0,
        _ => 20.0,
    }
}

/// One unit's ways, by index: their names (into `strings`), classes, ids, own English, and whether
/// placed yet.
struct UnitWays {
    ways: Vec<WayRec>,
    strings: Vec<String>,
    own: Vec<bool>,
    placed: Vec<bool>,
}

/// A named way placed by a ways-here record (its first): what a piece of the roads keeps of it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Placed {
    owner: u64,
    index: u32,
    name: String,
    class: u8,
    own: bool,
    lon: f64,
    lat: f64,
    way: i64,
}

/// How many hidata files' ways-here indexes a piece of the roads is (task #897): a few seconds.
const HERE_PIECE: usize = 16;

/// Roads and rail lines: each unit's named ways, placed by their boxes in the hidata's ways-here
/// index (each way once: its first tile's record). By pieces of `HERE_PIECE` hidata files, each's
/// ways placed kept (crate::resume_log::Pieces, a safe point): a resumed run replays those its last
/// kept and reads only the rest, a unit's ways read once a record of the rest names it.
fn roads(
    root: &store::nasfs::NasRoot,
    cat: &store::catalogue::Catalogue,
    lists: &mut Lists,
    pieces: &mut crate::resume_log::Pieces,
    progress: &dyn Fn(&str, u64, u64),
) -> Result<()> {
    let by_key: HashMap<u64, (&String, &String)> = cat
        .base
        .iter()
        .filter_map(|(unit, logical)| Some((crate::legacy::TileId::parse(unit)?.key(), (unit, logical))))
        .collect();
    let unit = |key: u64| -> Result<Option<UnitWays>> {
        let Some((unit, logical)) = by_key.get(&key) else { return Ok(None) };
        let Some(path) = path_of(root, cat, logical) else { return Ok(None) };
        let s = sect(&path)?;
        let ways: Vec<WayRec> = s.read_pod("ways")?;
        let strings: Vec<String> =
            String::from_utf8_lossy(&s.read("strings")?).split('\n').map(str::to_owned).collect();
        let roaden: HashMap<String, String> = path_of(root, cat, &format!("global/roaden/{}", unit.replace('/', "-")))
            .and_then(|p| p.read().ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let own: Vec<bool> = ways.iter().map(|w| roaden.contains_key(&w.id.to_string())).collect();
        let placed = vec![false; ways.len()];
        Ok(Some(UnitWays { ways, strings, own, placed }))
    };
    let hidata: Vec<&String> = cat.hidata.values().collect();
    let here = |i: usize| -> Result<Vec<roadcore::packs::Here>> {
        let Some(path) = path_of(root, cat, hidata[i]) else { return Ok(Vec::new()) };
        let s = sect(&path)?;
        if s.section("here").is_none() {
            return Ok(Vec::new());
        }
        Ok(s.read_pod::<roadcore::packs::Here>("here")?)
    };
    roads_in(pieces, hidata.len(), &here, &unit, lists, progress)
}

/// `roads`' pieces, of `n` hidata files' ways-here records (`here`), the units' ways read by `unit`.
fn roads_in(
    pieces: &mut crate::resume_log::Pieces,
    n: usize,
    here: &dyn Fn(usize) -> Result<Vec<roadcore::packs::Here>>,
    unit: &dyn Fn(u64) -> Result<Option<UnitWays>>,
    lists: &mut Lists,
    progress: &dyn Fn(&str, u64, u64),
) -> Result<()> {
    let mut units: HashMap<u64, Option<UnitWays>> = HashMap::new();
    // (Ways placed by kept pieces, in units not read yet: marked as each is read.)
    let mut pending: HashMap<u64, Vec<u32>> = HashMap::new();
    for c in 0..n.div_ceil(HERE_PIECE) {
        let range = c * HERE_PIECE..((c + 1) * HERE_PIECE).min(n);
        progress("ways-here indexes read", range.start as u64, n as u64);
        let p = pieces.piece((1, c as u32, 0), || {
            let mut placed: Vec<Placed> = Vec::new();
            for i in range.clone() {
                for h in here(i)? {
                    if h.extra & roadcore::packs::here_extra::UNNAMED != 0 {
                        continue;
                    }
                    if !units.contains_key(&h.owner) {
                        let mut u = unit(h.owner)?;
                        if let (Some(u), Some(ks)) = (u.as_mut(), pending.remove(&h.owner)) {
                            for k in ks {
                                if let Some(p) = u.placed.get_mut(k as usize) {
                                    *p = true;
                                }
                            }
                        }
                        units.insert(h.owner, u);
                    }
                    let Some(Some(u)) = units.get_mut(&h.owner) else { continue };
                    let k = h.index as usize;
                    let (Some(w), Some(p)) = (u.ways.get(k), u.placed.get_mut(k)) else { continue };
                    if *p {
                        continue;
                    }
                    *p = true;
                    let name = u.strings.get(w.name as usize).map(String::as_str).unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    placed.push(Placed {
                        owner: h.owner,
                        index: h.index,
                        name: name.to_string(),
                        class: w.class,
                        own: u.own[k],
                        lon: (h.bbox[0] as f64 + h.bbox[2] as f64) * 0.5e-7,
                        lat: (h.bbox[1] as f64 + h.bbox[3] as f64) * 0.5e-7,
                        way: w.id,
                    });
                }
            }
            Ok(serde_json::to_vec(&placed)?)
        })?;
        for w in serde_json::from_slice::<Vec<Placed>>(&p).context("a kept piece of the roads")? {
            match units.get_mut(&w.owner) {
                Some(Some(u)) => {
                    if let Some(p) = u.placed.get_mut(w.index as usize) {
                        *p = true;
                    }
                }
                Some(None) => {}
                None => pending.entry(w.owner).or_default().push(w.index),
            }
            *lists.report.read.entry("ways").or_default() += 1;
            let kind = if class::is_rail(w.class) || w.class == class::FERRY { Kind::Other } else { Kind::Road };
            lists.add(Thing {
                name: &w.name,
                kind,
                own: w.own,
                osm: Vec::new(),
                lon: w.lon,
                lat: w.lat,
                id: format!("w{}", w.way),
                priority: road_priority(w.class),
            });
        }
    }
    Ok(())
}

/// A landmark or area whose description is wanted.
struct Wanted {
    fame: f64,
    line: Value,
}

/// The descriptions written: Wikidata items and OSM objects (the descriptions' accepted `.jsonl`
/// files, in their paths' order: later ones win, `drop` removes one). An error when a file can't be
/// read: a list made without it would ask for descriptions already written.
fn described(accepted: &crate::inputs::Accepted) -> Result<HashSet<String>> {
    let mut files: Vec<(&String, store::nasfs::NasPath)> =
        accepted.files().filter(|(p, _)| p.ends_with(".jsonl")).collect();
    // (By the paths' components, as the map's server reads them.)
    files.sort_by(|a, b| Path::new(a.0).cmp(Path::new(b.0)));
    let mut set = HashSet::new();
    for (_, f) in files {
        let text = f.read_to_string().with_context(|| format!("read {}", f.display()))?;
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(key) = v.get("qid").or_else(|| v.get("id")).and_then(Value::as_str) else { continue };
            if v.get("drop").and_then(Value::as_bool) == Some(true) {
                set.remove(key);
            } else if v.get("long").and_then(Value::as_str).is_some_and(|l| !l.trim().is_empty()) {
                set.insert(key.to_owned());
            }
        }
    }
    Ok(set)
}

/// The translations' accepted files as the tables read them: by inputs-folder path, the copy and its
/// content name.
pub fn translation_files(accepted: &crate::inputs::Accepted) -> BTreeMap<String, (PathBuf, String)> {
    accepted
        .files()
        .filter(|(p, _)| p.ends_with(".jsonl"))
        // (The names crate's tables read their files by path: tests/nas_access.rs.)
        .map(|(p, path)| (p.clone(), (path.unchecked_path(), accepted.index.files[p].file.clone())))
        .collect()
}

/// An English Wikipedia article's title, from a record's tags (`wikipedia` "en:…", Wikidata's
/// `w_en`, the popup's `wiki`).
fn enwiki(info: &Value) -> Option<String> {
    let s = |v: Option<&Value>| v.and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_owned);
    s(info.get("wikipedia"))
        .and_then(|w| w.strip_prefix("en:").map(str::to_owned))
        .or_else(|| s(info.pointer("/wd/w_en")))
        .or_else(|| {
            (info.pointer("/wiki/lang").and_then(Value::as_str) == Some("en"))
                .then(|| s(info.pointer("/wiki/title")))
                .flatten()
        })
}

/// The landmarks: names on the lists, and the descriptions wanted.
fn landmarks(
    root: &store::nasfs::NasRoot,
    cat: &store::catalogue::Catalogue,
    lists: &mut Lists,
    done: &HashSet<String>,
    want: &mut Vec<Wanted>,
    progress: &dyn Fn(&str, u64, u64),
) -> Result<()> {
    let n = cat.markdata.len() as u64;
    // Each file opened once (a bundle holds every tile, task #449), its tiles read in the
    // catalogue's order.
    let mut files: HashMap<&str, HashMap<(u32, u32), crate::marks::MarkdataFile>> = HashMap::new();
    for logical in cat.markdata.values() {
        if files.contains_key(logical.as_str()) {
            continue;
        }
        let Some(path) = path_of(root, cat, logical) else { continue };
        files.insert(logical, crate::marks::markdata_files(&path)?.into_iter().collect());
    }
    for (i, (tile, logical)) in cat.markdata.iter().enumerate() {
        progress("landmark tiles read", i as u64, n);
        let Some(t) = crate::legacy::TileId::parse(tile).map(|u| (u.x, u.y)) else { continue };
        let Some(s) = files.get(logical.as_str()).and_then(|f| f.get(&t)) else { continue };
        let pts: Vec<crate::marks::MarkPt> = s.read_pod("pts")?;
        let blocks = |name: &str, idx: &str| -> Result<Vec<Vec<u8>>> {
            let data = s.read(name)?;
            let idx: Vec<[u64; 2]> = s.read_pod(idx)?;
            idx.iter()
                .map(|[o, l]| {
                    Ok(zstd::bulk::decompress(
                        data.get(*o as usize..(*o + *l) as usize).context("a block past the end")?,
                        64 << 20,
                    )?)
                })
                .collect()
        };
        let (props, info) = (blocks("props", "props_idx")?, blocks("info", "info_idx")?);
        for (k, p) in pts.iter().enumerate() {
            let (b, j) = (k / crate::marks::BLOCK, k % crate::marks::BLOCK);
            let pr: Value = props
                .get(b)
                .and_then(|b| crate::marks::object(b, j).ok())
                .and_then(|o| serde_json::from_slice(o).ok())
                .unwrap_or(Value::Null);
            let inf: Value = info
                .get(b)
                .and_then(|b| crate::marks::object(b, j).ok())
                .and_then(|o| serde_json::from_slice(o).ok())
                .unwrap_or(Value::Null);
            let name = pr.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            *lists.report.read.entry("landmarks").or_default() += 1;
            let (lon, lat) = (f64::from(p.lon) * 1e-7, f64::from(p.lat) * 1e-7);
            let fame = f64::from(p.fa);
            let osm = inf.get("osm").or_else(|| inf.pointer("/props/osm")).and_then(Value::as_str).map(str::to_owned);
            let qid = inf.get("qid").or_else(|| inf.get("wikidata")).and_then(Value::as_str).map(str::to_owned);
            let en = pr.get("en").and_then(Value::as_str).filter(|e| !e.trim().is_empty());
            lists.add(Thing {
                name,
                kind: Kind::Other,
                own: en.is_some(),
                osm: Vec::new(),
                lon,
                lat,
                id: osm.clone().or_else(|| qid.clone()).unwrap_or_default(),
                priority: 30.0 + 8.0 * fame,
            });
            // A description, when it has an English article or a register entry and none yet.
            let register = inf.pointer("/props/url").and_then(Value::as_str).map(str::to_owned);
            let article = enwiki(&inf);
            if article.is_none() && register.is_none() {
                continue;
            }
            let has = inf.get("long").and_then(Value::as_str).is_some_and(|l| !l.trim().is_empty())
                || qid.as_ref().is_some_and(|q| done.contains(q))
                || osm.as_ref().is_some_and(|o| done.contains(o));
            if has {
                continue;
            }
            let other = inf.get("wiki").filter(|_| article.is_none()).cloned();
            want.push(Wanted {
                fame,
                line: json!({
                    "qid": qid, "id": osm, "name": name, "en": en, "kind": pr.get("kind"),
                    "designation": pr.get("designation").or_else(|| inf.pointer("/props/category")),
                    "at": [(lon * 1e5).round() / 1e5, (lat * 1e5).round() / 1e5],
                    "enwiki": article, "wiki": other, "register": register,
                    // (Its heritage register's id: the catalogue's credits name the register.)
                    "source": inf.pointer("/props/register").or_else(|| inf.pointer("/props/source")), "fame": (fame * 1000.0).round() / 1000.0,
                }),
            });
        }
    }
    Ok(())
}

/// Parks and protected areas with an English article (their records have no register entry), by
/// Wikidata's sitelinks where known, else area.
fn areas(
    root: &store::nasfs::NasRoot,
    cat: &store::catalogue::Catalogue,
    done: &HashSet<String>,
    want: &mut Vec<Wanted>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for logical in cat.ovdata.values() {
        let Some(path) = path_of(root, cat, logical) else { continue };
        let s = sect(&path)?;
        if s.section("parks.recs").is_none() {
            continue;
        }
        let recs = s.read("parks.recs")?;
        for v in serde_json::Deserializer::from_slice(&recs).into_iter::<Value>().flatten() {
            let (Some(name), Some(osm)) = (v.get("name").and_then(Value::as_str), v.get("osm").and_then(Value::as_str))
            else {
                continue;
            };
            if !seen.insert(osm.to_owned()) {
                continue;
            }
            let Some(article) = enwiki(&v) else { continue };
            let qid = v.get("wikidata").and_then(Value::as_str);
            if done.contains(osm) || qid.is_some_and(|q| done.contains(q)) {
                continue;
            }
            let km2 = v.get("area_km2").and_then(Value::as_f64).unwrap_or(0.0);
            let sl = v.pointer("/wd/sl").and_then(Value::as_f64);
            let fame = sl.unwrap_or_else(|| (km2 + 1.0).log10());
            let bbox = v.get("bbox").cloned().unwrap_or(Value::Null);
            want.push(Wanted {
                fame,
                line: json!({"qid": qid, "id": osm, "name": name, "kind": v.get("boundary").or_else(|| v.get("leisure")), "bbox": bbox, "area_km2": km2, "enwiki": article, "fame": (fame * 1000.0).round() / 1000.0}),
            });
        }
    }
    Ok(())
}

/// An entry's example: its OSM object (`osm`: n123, w123, r123), else its Wikidata item (`qid`), else
/// its label's row (`label`: labels made before they carried their OSM object), and where it is.
fn example((id, lon, lat): &(String, f64, f64)) -> Value {
    let at = json!([(lon * 1e5).round() / 1e5, (lat * 1e5).round() / 1e5]);
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if id.len() > 1 && matches!(&id[..1], "n" | "w" | "r") && digits(&id[1..]) {
        json!({"osm": id, "at": at})
    } else if id.len() > 1 && id.starts_with('Q') && digits(&id[1..]) {
        json!({"qid": id, "at": at})
    } else if let Some(row) = id.strip_prefix("label ") {
        json!({"label": row.parse::<u64>().unwrap_or(0), "at": at})
    } else {
        json!({"at": at})
    }
}

/// Writes a file whole (store::whole::write: a held old file renamed aside).
fn put(path: &store::nasfs::NasPath, bytes: &[u8]) -> Result<()> {
    path.write_whole(bytes)
}

/// Makes the lists from the newest catalogue under `root` into `out` (the translations' and the
/// descriptions' `todo/` under it: the root itself for the job, a scratch folder to look first),
/// with the accepted versions of the translations and the descriptions the agent names.
pub fn run(
    root: &store::nasfs::NasRoot,
    out: &store::nasfs::NasRoot,
    translations: &crate::inputs::Accepted,
    descriptions: &crate::inputs::Accepted,
    scratch: &Path,
    progress: &dyn Fn(&str, u64, u64),
) -> Result<Report> {
    use crate::timings::phase;
    let p = phase("reading the catalog, the languages spoken and the translations");
    let cat = store::catalogue::latest(&root.join(store::catalogue::NAME))?.context("no catalogue")?;
    let spoken = spoken_for(root, &cat)?;
    let mut names = Names::from_files(&translation_files(translations));
    let failed = names.take_warnings();
    anyhow::ensure!(failed.is_empty(), "the translations' accepted files: {}", failed.join("; "));
    drop(p);
    let units = unit_tiles(&cat);
    // (The roads' pieces, task #897: what they keep is the catalogue's alone, so by what of it the
    // lists read.)
    let id = crate::resume_log::job_id("names-todo", &VERSION.to_string(), "roads", &[&inputs_key(&cat)]);
    let mut pieces = crate::resume_log::Pieces::open(scratch, (0, 0, 0), "names-todo", &id, crate::resume_log::on());
    let mut lists = Lists { names: &names, spoken: &spoken, entries: HashMap::new(), report: Report::default() };
    // (Each kind's names read from the map's files on the NAS and sorted into the lists as they
    // come: a phase a kind.)
    let p = phase("gathering the labels' names");
    labels(root, &cat, &units, &coverage(&cat), &mut lists, progress)?;
    drop(p);
    let p = phase("gathering the roads' names");
    roads(root, &cat, &mut lists, &mut pieces, progress)?;
    drop(p);
    let p = phase("gathering the landmarks' names");
    let done = described(descriptions)?;
    let mut want = Vec::new();
    landmarks(root, &cat, &mut lists, &done, &mut want, progress)?;
    drop(p);
    let p = phase("gathering the areas");
    let mut wanted_areas = Vec::new();
    areas(root, &cat, &done, &mut wanted_areas)?;
    drop(p);
    let Lists { entries, mut report, .. } = lists;
    let _p = phase("sorting and writing the lists");

    // The translations' lists.
    let mut by_lang: BTreeMap<Lang, Vec<(String, Kind, Entry)>> = BTreeMap::new();
    for ((n, k, l), e) in entries {
        by_lang.entry(l).or_default().push((n, k, e));
    }
    let dir = crate::inputs::todo_dir(out, crate::inputs::translations::UNIT);
    dir.create_dir_all()?;
    let mut written = HashSet::new();
    for (lang, mut es) in by_lang {
        let pri = |e: &Entry| e.best + (e.things as f64).log10();
        es.sort_by(|a, b| {
            pri(&b.2)
                .total_cmp(&pri(&a.2))
                .then(b.2.things.cmp(&a.2.things))
                .then_with(|| a.0.cmp(&b.0))
                .then(a.1.cmp(&b.1))
        });
        let mut text = String::new();
        for (n, k, e) in &es {
            let line = json!({
                "n": n, "kind": k.as_str(), "langs": e.langs.iter().map(Lang::as_str).collect::<Vec<_>>(),
                "things": e.things, "example": example(&e.example),
                "priority": (pri(e) * 100.0).round() / 100.0,
            });
            text.push_str(&line.to_string());
            text.push('\n');
        }
        let file = format!("{}.jsonl", lang.as_str());
        put(&dir.join(&file), text.as_bytes())?;
        written.insert(file);
        report.langs.insert(lang.as_str().to_owned(), es.len());
    }
    // Lists of languages with nothing left go.
    for e in dir.read_dir()? {
        if e.name.ends_with(".jsonl") && !written.contains(&e.name) {
            e.path.remove_file().ok();
        }
    }
    put(&dir.join("README.md"), TRANSLATORS.as_bytes())?;
    // (The checker, named without a quoted `.py`: publish.sh takes every quoted one in the pipeline
    // for a Python step of the app's dem/, and this one isn't.)
    put(&dir.join("check").with_extension("py"), CHECK.as_bytes())?;

    // The descriptions' lists.
    let ddir = crate::inputs::todo_dir(out, crate::inputs::descriptions::UNIT);
    ddir.create_dir_all()?;
    for (file, mut w) in [("landmarks.jsonl", want), ("areas.jsonl", wanted_areas)] {
        w.sort_by(|a, b| b.fame.total_cmp(&a.fame));
        let text: String = w.iter().map(|x| x.line.to_string() + "\n").collect();
        put(&ddir.join(file), text.as_bytes())?;
        if file == "landmarks.jsonl" {
            report.landmarks = w.len();
        } else {
            report.areas = w.len();
        }
    }
    put(&ddir.join("README.md"), WRITERS.as_bytes())?;
    pieces.finish();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/maplog.md §8, step 4: the lists' key moves with a batch touching any of their inputs'
    /// prefixes (a section's key, or a file of one), and with nothing else; across a reopened log.
    #[test]
    fn names_todo_reruns_exactly_when_its_prefixes_change() {
        use store::map::{log::Log, Map};
        let d = tempfile::tempdir().unwrap();
        let f = |file: &str| json!({"file": file, "size": 1, "fmt": 1});
        let mut e: store::map::Entries = [
            ("u/6/1/1".to_string(), json!(true)),
            ("s/base/6/1/1".into(), json!("base/6-1-1")),
            ("f/base/6-1-1".into(), f("base/6-1-1.a.sect")),
            ("l/terrain/deep/10/1/1".into(), json!("layers/terrain/deep/10-1-1")),
            ("f/layers/terrain/deep/10-1-1".into(), f("layers/terrain/deep/10-1-1.a.pack")),
            ("f/layers/labels/hi/6-1-1".into(), f("layers/labels/hi/6-1-1.a.pack")),
        ]
        .into();
        let mut log = Log::open_watching(d.path(), &INPUTS).unwrap();
        log.seed(Map::new(1, "t1".into(), e.clone())).unwrap();
        let key = |l: &Log| inputs_seq(l.marks(), Some(&l.map())).unwrap();
        assert_eq!(key(&log), "map 1");
        let mut seq = 1;
        let mut step = |log: &mut Log, e: &store::map::Entries| {
            seq += 1;
            let b = log.map().batch_to(e, seq, &format!("t{seq}"), "", "");
            log.append(&b).unwrap();
            key(log)
        };
        // A deep terrain pack built again: not an input.
        e.insert("f/layers/terrain/deep/10-1-1".into(), f("layers/terrain/deep/10-1-1.b.pack"));
        assert_eq!(step(&mut log, &e), "map 1");
        // A base pack's file: an input.
        e.insert("f/base/6-1-1".into(), f("base/6-1-1.b.sect"));
        assert_eq!(step(&mut log, &e), "map 3");
        // A labels' pack's file, then a unit dropped: each an input.
        e.insert("f/layers/labels/hi/6-1-1".into(), f("layers/labels/hi/6-1-1.b.pack"));
        assert_eq!(step(&mut log, &e), "map 4");
        e.remove("u/6/1/1");
        assert_eq!(step(&mut log, &e), "map 5");
        // A slope tile added: not.
        e.insert("l/slope/hi/6/1/1".into(), json!("layers/slope/hi/6-1-1"));
        assert_eq!(step(&mut log, &e), "map 5");
        // Reopened (from its segments, then from its snapshot): the same key.
        drop(log);
        let mut log = Log::open_watching(d.path(), &INPUTS).unwrap();
        assert_eq!(key(&log), "map 5");
        log.compact().unwrap();
        let mut log = Log::open_watching(d.path(), &INPUTS).unwrap();
        assert_eq!(key(&log), "map 5");
        // A base pack named outside the watched prefixes: made again at every batch, not missed.
        e.insert("s/base/6/1/2".into(), json!("elsewhere/6-1-2"));
        assert_eq!(step(&mut log, &e), "map 7");
        e.insert("l/slope/hi/6/1/2".into(), json!("layers/slope/hi/6-1-2"));
        assert_eq!(step(&mut log, &e), "map 8");
    }

    #[test]
    fn english_or_not() {
        for (name, spoken, english) in [
            ("Lake Louise", true, true),
            ("Main Street", true, true),
            ("O'Connell Bridge", true, true),
            ("Shiplett Boulevard", true, true),
            ("Rue Principale", true, false),
            ("Lac des Sables", true, false),
            ("Montréal", true, false),
            ("Ffordd Caergybi", true, false),
            ("中山", true, false),
            ("文武廟 Man Mo Temple Compound", true, true),
            ("文武廟 Man Mo", true, true),
            ("文武廟 Man Mo", false, false),
            ("The Architectural Work of Le Corbusier, an Outstanding Contribution to the Modern Movement", false, true),
            ("Church of Saint-Martin", false, true),
            ("Rue de la Paix", false, false),
            ("Hiraizumi – Temples, Gardens and Archaeological Sites Representing the Buddhist Pure Land", false, true),
            ("Kulangsu, a Historic International Settlement", false, true),
            ("Hiraizumi", false, false),
            ("Le Clivet", false, false),
            ("Stany", false, false),
        ] {
            assert_eq!(reads_english(name, spoken), english, "{name}");
        }
    }

    fn spoken() -> Spoken {
        let p = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        let rect = |code: &str, w: f64, s: f64, e: f64, n: f64| names::spoken::Area {
            code: code.into(),
            area_km2: (e - w) * (n - s),
            polygons: vec![vec![vec![p(w, s), p(e, s), p(e, n), p(w, n)]]],
        };
        let table = names::Table::parse("JP\tja\nFR\tfr\nCA\ten,fr\nCA-QC\tfr,en\n");
        Spoken::build(
            &table,
            [
                rect("JP", 128.0, 30.0, 146.0, 46.0),
                rect("FR", -5.0, 42.0, 8.0, 51.0),
                rect("CA", -80.0, 42.0, -60.0, 60.0),
                rect("CA-QC", -75.0, 45.0, -65.0, 55.0),
            ],
        )
    }

    /// Task #897: the roads paused after every piece kept, resumed each time, gather the same lists
    /// as a run never paused, each piece's ways-here records read once.
    #[test]
    fn roads_paused_at_every_piece_resume_to_the_same() {
        use crate::resume_log::{Pieces, STOP_AFTER};
        use roadcore::packs::Here;
        let names = Names::load(tempfile::tempdir().unwrap().path()).unwrap();
        let sp = spoken();
        // Two units' ways (two named, one not; a third unit missing), placed in France and Japan; ways-here records over
        // 40 files (three pieces), each way in several, some unnamed by the record.
        let way = |id: i64, name: u32, class: u8| WayRec { id, name, class, ..bytemuck::Zeroable::zeroed() };
        let unit = |key: u64| -> Result<Option<UnitWays>> {
            let strings: Vec<String> = match key {
                1 => vec!["".into(), "Rue Haute".into(), "Chemin Bas".into()],
                2 => vec!["".into(), "Kita-dori".into(), "大通り".into()],
                _ => return Ok(None),
            };
            let ways = vec![
                way(key as i64 * 10, 1, class::PRIMARY),
                way(key as i64 * 10 + 1, 2, 3),
                way(key as i64 * 10 + 2, 0, 2),
            ];
            Ok(Some(UnitWays { own: vec![false, key == 2, false], placed: vec![false; 3], ways, strings }))
        };
        let read = std::cell::Cell::new(0);
        let here = |i: usize| -> Result<Vec<Here>> {
            read.set(read.get() + 1);
            let (lon, lat) = if i % 2 == 0 { (2.0, 47.0) } else { (139.0, 36.0) };
            let e7 = |d: f64| (d * 1e7) as i32 + i as i32;
            Ok((0..3u32)
                .map(|k| Here {
                    owner: 1 + (i as u64 + k as u64) % 3,
                    index: (k + i as u32) % 4,
                    extra: if i % 7 == 0 { roadcore::packs::here_extra::UNNAMED } else { 0 },
                    bbox: [e7(lon), e7(lat), e7(lon), e7(lat)],
                    ..bytemuck::Zeroable::zeroed()
                })
                .collect())
        };
        let gather = |pieces: &mut Pieces| -> Result<String> {
            let mut l = Lists { names: &names, spoken: &sp, entries: HashMap::new(), report: Report::default() };
            roads_in(pieces, 40, &here, &unit, &mut l, &|_, _, _| {})?;
            let mut e: Vec<String> = l
                .entries
                .iter()
                .map(|(k, e)| format!("{k:?} {:?} {} {} {:?}", e.langs, e.things, e.best, e.example))
                .collect();
            e.sort();
            Ok(format!("{e:?} {:?}", l.report))
        };
        let fresh = gather(&mut Pieces::none()).unwrap();
        assert!(fresh.contains("Rue Haute") || fresh.contains("Chemin Bas"), "{fresh}");
        read.set(0);
        let d = tempfile::tempdir().unwrap();
        let mut stops = 0;
        let resumed = loop {
            STOP_AFTER.set(Some(1));
            let r = gather(&mut Pieces::open(d.path(), (0, 0, 0), "names-todo", "a", true));
            STOP_AFTER.set(None);
            match r {
                Err(e) => {
                    assert!(crate::control::is_paused(&e), "{e:#}");
                    stops += 1;
                }
                Ok(r) => break r,
            }
        };
        assert_eq!(stops, 3, "a stop after each piece");
        assert_eq!(resumed, fresh);
        assert_eq!(read.get(), 40, "each file read once");
    }

    #[test]
    fn what_goes_on_the_lists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("0-converted")).unwrap();
        std::fs::write(
            dir.path().join("0-converted/x.jsonl"),
            "{\"n\": \"Lac Bleu\", \"kind\": \"other\", \"langs\": \"fr\", \"sub\": \"Blue Lake\"}\n",
        )
        .unwrap();
        let f = std::fs::File::options().write(true).open(dir.path().join("0-converted/x.jsonl")).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60)).unwrap();
        let names = Names::load(dir.path()).unwrap();
        let sp = spoken();
        let mut l = Lists { names: &names, spoken: &sp, entries: HashMap::new(), report: Report::default() };
        let t = |name: &'static str, kind, own, osm: &[&str], lon, lat, pr| Thing {
            name,
            kind,
            own,
            osm: osm.iter().filter_map(|x| Lang::parse(x)).collect(),
            lon,
            lat,
            id: format!("n{}", name.len()),
            priority: pr,
        };
        l.add(t("Lac Bleu", Kind::Other, false, &[], 2.0, 46.0, 30.0)); // a line
        l.add(t("Lac Noir", Kind::Other, true, &[], 2.0, 46.0, 30.0)); // its own English
        l.add(t("Lac Noir", Kind::Other, false, &[], 2.0, 46.0, 31.0));
        l.add(t("Lac Noir", Kind::Other, false, &[], -73.0, 46.0, 35.0)); // Quebec: French first
        l.add(t("Lake Louise", Kind::Other, false, &[], -76.0, 44.0, 30.0)); // English in Canada
        l.add(t("Rue Haute", Kind::Road, false, &[], -79.0, 44.0, 20.0)); // French signs in Ontario
        l.add(t("中山", Kind::Other, false, &[], 139.0, 35.0, 40.0));
        l.add(t("Kêr Vraz", Kind::Other, false, &["br"], 2.0, 46.0, 50.0)); // OSM's language
        l.add(t("Le Moulin", Kind::Settlement, false, &[], 2.0, 46.0, 50.0)); // keeps its name
        l.add(t("Sea", Kind::Other, false, &[], -30.0, 40.0, 50.0)); // nowhere
        let r = &l.report;
        assert_eq!((r.own, r.lined, r.english, r.nowhere), (1, 1, 1, 1));
        let get = |n: &str, k: Kind, lang: &str| l.entries.get(&(n.to_owned(), k, Lang::parse(lang).unwrap()));
        let e = get("Lac Noir", Kind::Other, "fr").expect("Lac Noir");
        assert_eq!((e.things, e.best, e.langs.iter().map(Lang::as_str).collect::<Vec<_>>()), (2, 35.0, vec!["fr"]));
        assert_eq!(get("Rue Haute", Kind::Road, "fr").map(|e| e.langs.len()), Some(1));
        assert!(get("中山", Kind::Other, "ja").is_some());
        assert!(get("Kêr Vraz", Kind::Other, "br").is_some());
        assert_eq!(l.report.settlements, 1);
        assert_eq!(l.entries.len(), 4);
    }
}
