//! The to-do lists (docs/plan.md §7), written after each build by the `names-todo` job:
//!
//! - **`translations/todo/<language>.jsonl`:** the names in the coverage that something has with no
//!   English of its own and no line in any language spoken where it is, one entry per name, kind
//!   and first candidate language, sorted by priority; with `README.md`, the translators' brief,
//!   and `check.py`, its checker.
//! - **`descriptions/todo/`:** the landmarks (`landmarks.jsonl`) and parks and protected areas
//!   (`areas.jsonl`) with an English Wikipedia article or a register entry and no description,
//!   sorted by fame; with `README.md`, the writers' brief.
//!
//! What it reads, all from the newest catalog:
//! - **Labels** (places, states, water, parks): the labels layer's zoom-12 tiles in the coverage's
//!   units' z6 tiles, each label once (its feature id). Own English: `en`, else `kana` romanised;
//!   OSM's languages: `l`.
//! - **Roads and rail lines:** each unit's base pack (ways, names) and its roads' own English
//!   (`global/roaden/<u>`); where each is, the centre of its box from the hidata's ways-here
//!   index. Roads carry no OSM language tags yet.
//! - **Landmarks:** the markdata's points (all eight kinds), their `en`, fame, and popup records
//!   (Wikidata item, Wikipedia article, register entry, written description).
//! - **Parks:** the ovdata's park records.
//! - The languages spoken where: the catalog's outlines (`names::spoken`), kept in the scratch.
//! - The translations folder and the descriptions folder (not their `todo/`).

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
pub const CHECK: &str = include_str!("../../../tools/names/check.py");
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
    /// Things with English of their own; with a line; in English; nowhere a language is known.
    pub own: u64,
    pub lined: u64,
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
    "lac", "lacs", "rivière", "riviere", "fleuve", "ruisseau", "étang", "etang", "baie", "anse", "cap", "pointe", "île", "ile", "îles", "iles", "mont", "monts", "montagne", "col", "pic", "forêt", "foret", "parc", "réserve", "chute", "chutes", "vallée", "vallee", "plage", "marais", "château", "chateau", "église", "eglise", "chapelle", "pont", "gare", "musée", "musee", "moulin", "rue", "chemin", "rang", "côte", "cote", "allée", "allee", "impasse", "boulevard", "de", "du", "des", "la", "le", "les", "aux", "et", "saint", "sainte",
    // Spanish, Portuguese, Catalan, Galician
    "río", "rio", "lago", "laguna", "embalse", "sierra", "monte", "isla", "playa", "parque", "pico", "puerto", "castillo", "iglesia", "puente", "calle", "carrera", "camino", "avenida", "plaza", "del", "el", "los", "las", "y", "lagoa", "serra", "ilha", "praia", "castelo", "igreja", "rua", "estrada", "praça", "da", "do", "dos", "das", "carrer", "riu", "camí", "plaça", "dels", "rúa", "san", "santa", "santo", "são",
    // Welsh, Irish, Scottish Gaelic
    "afon", "llyn", "mynydd", "coed", "eglwys", "ffordd", "heol", "stryd", "lôn", "nant", "pen", "bryn", "cwm", "sliabh", "abhainn", "inis", "oileán", "bóthar", "sráid", "baile", "cnoc", "rathad", "sràid", "beinn", "gleann", "allt", "eilean",
];

/// Whether a name reads as English: Latin letters without accents, and none of the coverage's
/// other languages' words.
fn reads_english(name: &str) -> bool {
    if name.chars().any(|c| c.is_alphabetic() && !c.is_ascii()) {
        return false;
    }
    !name.split(|c: char| !c.is_alphanumeric()).any(|w| OTHER_WORDS.contains(&w.to_lowercase().as_str()))
}

impl Lists<'_> {
    /// Adds a thing: on a list when it lacks English.
    fn add(&mut self, t: Thing) {
        if t.own {
            self.report.own += 1;
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
        // Where English is spoken, a name without another language's signs is English.
        if cands.contains(&en) {
            if reads_english(t.name) {
                self.report.english += 1;
                return;
            }
            if cands.len() > 1 {
                cands.retain(|l| *l != en);
            }
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

/// A catalog file's path under the root.
fn path_of(root: &Path, cat: &store::catalog::Catalog, logical: &str) -> Option<PathBuf> {
    cat.file(logical).map(|f| root.join(&f.file))
}

fn sect(path: &Path) -> Result<SectReader<PlainFile>> {
    SectReader::open(PlainFile::open(path).with_context(|| format!("open {}", path.display()))?).with_context(|| format!("read {}", path.display()))
}

/// The spoken languages for the catalog's outlines, kept in `scratch` by the outlines' content
/// name (made in minutes from the NAS, read back in milliseconds).
pub fn spoken_for(root: &Path, cat: &store::catalog::Catalog, scratch: &Path) -> Result<Spoken> {
    let logical = cat.global.get("outlines").context("the catalog has no outlines")?;
    let f = cat.file(logical).context("the outlines' file")?;
    let kept = scratch.join(format!("spoken-{}.bin", f.file.replace('/', "_")));
    if let Some(s) = std::fs::read(&kept).ok().and_then(|b| Spoken::from_bytes(&b).ok()) {
        return Ok(s);
    }
    let s = crate::outlines::Outlines::open(&root.join(&f.file))?.spoken()?;
    std::fs::create_dir_all(scratch).ok();
    std::fs::write(&kept, s.to_bytes()).ok();
    Ok(s)
}

/// The units' z6 tiles.
fn unit_tiles(cat: &store::catalog::Catalog) -> HashSet<(u32, u32)> {
    cat.units.iter().filter_map(|u| {
        let mut p = u.split('/');
        (p.next()? == "6").then_some(())?;
        Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
    }).collect()
}

fn tile_of(key: &str) -> Option<(u32, u32)> {
    let mut p = key.split('/');
    (p.next()? == "6").then_some(())?;
    Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
}

/// Labels: the zoom-12 tiles of the labels layer's hi packs in the units' tiles.
fn labels(root: &Path, cat: &store::catalog::Catalog, units: &HashSet<(u32, u32)>, lists: &mut Lists, progress: &dyn Fn(&str, u64, u64)) -> Result<()> {
    let Some(layer) = cat.layers.get("labels") else { return Ok(()) };
    let packs: Vec<&String> = layer.hi.iter().filter(|(k, _)| tile_of(k).is_some_and(|t| units.contains(&t))).map(|(_, v)| v).collect();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    for (i, logical) in packs.iter().enumerate() {
        progress("label packs read", i as u64, packs.len() as u64);
        let Some(path) = path_of(root, cat, logical) else { continue };
        let src = PlainFile::open(&path).with_context(|| format!("open {}", path.display()))?;
        let idx = store::pack::PackIndex::read_from(&src)?;
        for e in idx.entries.iter().filter(|e| e.zxy().0 == 12) {
            let (z, x, y) = e.zxy();
            let blob = idx.read_blob(&src, e)?;
            let raw = names::mvt::gunzip_if_gzip(&blob)?;
            let tile = names::mvt::Tile::decode(&raw)?;
            for l in tile.layers.iter().filter(|l| l.name == "l" && l.extent > 0) {
                for f in &l.features {
                    let tags: Vec<(&str, &str)> = f.tags.chunks_exact(2).filter_map(|p| Some((l.keys.get(p[0] as usize)?.as_str(), l.values.get(p[1] as usize)?.as_str()?))).collect();
                    let num = |k: &str| f.tags.chunks_exact(2).find(|p| l.keys.get(p[0] as usize).is_some_and(|x| x == k)).and_then(|p| match l.values.get(p[1] as usize)? {
                        names::mvt::Value::Double(v) => Some(*v),
                        names::mvt::Value::Float(v) => Some(f64::from(*v)),
                        names::mvt::Value::Int(v) | names::mvt::Value::Sint(v) => Some(*v as f64),
                        names::mvt::Value::Uint(v) => Some(*v as f64),
                        _ => None,
                    });
                    let get = |k: &str| tags.iter().find(|(key, _)| *key == k).map(|(_, v)| *v).filter(|v| !v.is_empty());
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
                    *lists.report.read.entry("labels").or_default() += 1;
                    let (lon, lat) = names::mvt::tile_to_lonlat(u32::from(z), x, y, l.extent, f64::from(px), f64::from(py));
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
                    lists.add(Thing { name, kind, own, osm: names::own::osm_langs(name, &tags), lon, lat, id, priority });
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

/// Roads and rail lines: each unit's ways, placed by their boxes in the hidata.
fn roads(root: &Path, cat: &store::catalog::Catalog, lists: &mut Lists, progress: &dyn Fn(&str, u64, u64)) -> Result<()> {
    // Where each way is: its box's centre, from the ways-here index (a way drawn in several tiles
    // is in each: the first).
    let mut at: HashMap<u64, [i32; 2]> = HashMap::new();
    let n = cat.hidata.len() as u64;
    for (i, logical) in cat.hidata.values().enumerate() {
        progress("ways-here indexes read", i as u64, n);
        let Some(path) = path_of(root, cat, logical) else { continue };
        let s = sect(&path)?;
        if s.section("here").is_none() {
            continue;
        }
        for h in s.read_pod::<roadcore::packs::Here>("here")? {
            at.entry(h.id).or_insert([((h.bbox[0] as i64 + h.bbox[2] as i64) / 2) as i32, ((h.bbox[1] as i64 + h.bbox[3] as i64) / 2) as i32]);
        }
    }
    let n = cat.base.len() as u64;
    for (i, (unit, logical)) in cat.base.iter().enumerate() {
        progress("units' ways read", i as u64, n);
        let Some(path) = path_of(root, cat, logical) else { continue };
        let s = sect(&path)?;
        let ways: Vec<WayRec> = s.read_pod("ways")?;
        let strings = String::from_utf8_lossy(&s.read("strings")?).into_owned();
        let strings: Vec<&str> = strings.split('\n').collect();
        let own: HashMap<String, String> = path_of(root, cat, &format!("global/roaden/{}", unit.replace('/', "-")))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for w in &ways {
            let name = strings.get(w.name as usize).copied().unwrap_or("");
            if name.is_empty() {
                continue;
            }
            *lists.report.read.entry("ways").or_default() += 1;
            let Some(p) = at.get(&(w.id as u64)) else { continue };
            let kind = if class::is_rail(w.class) || w.class == class::FERRY { Kind::Other } else { Kind::Road };
            lists.add(Thing {
                name,
                kind,
                own: own.contains_key(&w.id.to_string()),
                osm: Vec::new(),
                lon: f64::from(p[0]) * 1e-7,
                lat: f64::from(p[1]) * 1e-7,
                id: format!("w{}", w.id),
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

/// The descriptions written: Wikidata items and OSM objects (`descriptions/**/*.jsonl`, not
/// `todo/`; later file names win, `drop` removes one).
fn described(dir: &Path) -> HashSet<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with('.') || n.starts_with('@') || n.starts_with('#') || n == "todo" {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if n.ends_with(".jsonl") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let mut set = HashSet::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
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
    set
}

/// An English Wikipedia article's title, from a record's tags (`wikipedia` "en:…", Wikidata's
/// `w_en`, the popup's `wiki`).
fn enwiki(info: &Value) -> Option<String> {
    let s = |v: Option<&Value>| v.and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_owned);
    s(info.get("wikipedia")).and_then(|w| w.strip_prefix("en:").map(str::to_owned))
        .or_else(|| s(info.pointer("/wd/w_en")))
        .or_else(|| (info.pointer("/wiki/lang").and_then(Value::as_str) == Some("en")).then(|| s(info.pointer("/wiki/title"))).flatten())
}

/// The landmarks: names on the lists, and the descriptions wanted.
fn landmarks(root: &Path, cat: &store::catalog::Catalog, lists: &mut Lists, done: &HashSet<String>, want: &mut Vec<Wanted>, progress: &dyn Fn(&str, u64, u64)) -> Result<()> {
    let n = cat.markdata.len() as u64;
    for (i, logical) in cat.markdata.values().enumerate() {
        progress("landmark tiles read", i as u64, n);
        let Some(path) = path_of(root, cat, logical) else { continue };
        let s = sect(&path)?;
        let pts: Vec<crate::marks::MarkPt> = s.read_pod("pts")?;
        let blocks = |name: &str, idx: &str| -> Result<Vec<Vec<u8>>> {
            let data = s.read(name)?;
            let idx: Vec<[u64; 2]> = s.read_pod(idx)?;
            idx.iter().map(|[o, l]| Ok(zstd::bulk::decompress(data.get(*o as usize..(*o + *l) as usize).context("a block past the end")?, 64 << 20)?)).collect()
        };
        let (props, info) = (blocks("props", "props_idx")?, blocks("info", "info_idx")?);
        for (k, p) in pts.iter().enumerate() {
            let (b, j) = (k / crate::marks::BLOCK, k % crate::marks::BLOCK);
            let pr: Value = props.get(b).and_then(|b| crate::marks::object(b, j).ok()).and_then(|o| serde_json::from_slice(o).ok()).unwrap_or(Value::Null);
            let inf: Value = info.get(b).and_then(|b| crate::marks::object(b, j).ok()).and_then(|o| serde_json::from_slice(o).ok()).unwrap_or(Value::Null);
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
                priority: 30.0 + 20.0 * fame.min(3.0),
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
                    "source": inf.pointer("/props/source"), "fame": (fame * 1000.0).round() / 1000.0,
                }),
            });
        }
    }
    Ok(())
}

/// Parks and protected areas with an English article (their records have no register entry), by
/// Wikidata's sitelinks where known, else area.
fn areas(root: &Path, cat: &store::catalog::Catalog, done: &HashSet<String>, want: &mut Vec<Wanted>) -> Result<()> {
    let mut seen = HashSet::new();
    for logical in cat.ovdata.values() {
        let Some(path) = path_of(root, cat, logical) else { continue };
        let s = sect(&path)?;
        if s.section("parks.recs").is_none() {
            continue;
        }
        let recs = s.read("parks.recs")?;
        for v in serde_json::Deserializer::from_slice(&recs).into_iter::<Value>().flatten() {
            let (Some(name), Some(osm)) = (v.get("name").and_then(Value::as_str), v.get("osm").and_then(Value::as_str)) else { continue };
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

/// Writes a file whole (a temporary name, then renamed).
fn put(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))
}

/// Makes the lists from the newest catalog under `root` into `out` (`translations/todo/` and
/// `descriptions/todo/` under it: the root itself for the job, a scratch folder to look first),
/// with the translations of `translations` (the root's `translations/` for the job).
pub fn run(root: &Path, out: &Path, translations: &Path, scratch: &Path, progress: &dyn Fn(&str, u64, u64)) -> Result<Report> {
    let cat = store::catalog::latest(&root.join("catalog"))?.context("no catalog")?;
    let spoken = spoken_for(root, &cat, scratch)?;
    let mut names = Names::load(translations)?;
    names.take_warnings();
    let units = unit_tiles(&cat);
    let mut lists = Lists { names: &names, spoken: &spoken, entries: HashMap::new(), report: Report::default() };
    labels(root, &cat, &units, &mut lists, progress)?;
    roads(root, &cat, &mut lists, progress)?;
    let done = described(&root.join("descriptions"));
    let mut want = Vec::new();
    landmarks(root, &cat, &mut lists, &done, &mut want, progress)?;
    let mut wanted_areas = Vec::new();
    areas(root, &cat, &done, &mut wanted_areas)?;
    let Lists { entries, mut report, .. } = lists;

    // The translations' lists.
    let mut by_lang: BTreeMap<Lang, Vec<(String, Kind, Entry)>> = BTreeMap::new();
    for ((n, k, l), e) in entries {
        by_lang.entry(l).or_default().push((n, k, e));
    }
    let dir = out.join("translations/todo");
    std::fs::create_dir_all(&dir)?;
    let mut written = HashSet::new();
    for (lang, mut es) in by_lang {
        let pri = |e: &Entry| e.best + (e.things as f64).log10();
        es.sort_by(|a, b| pri(&b.2).total_cmp(&pri(&a.2)).then(b.2.things.cmp(&a.2.things)).then_with(|| a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
        let mut text = String::new();
        for (n, k, e) in &es {
            let line = json!({
                "n": n, "kind": k.as_str(), "langs": e.langs.iter().map(Lang::as_str).collect::<Vec<_>>(),
                "things": e.things, "example": {"osm": e.example.0, "at": [(e.example.1 * 1e5).round() / 1e5, (e.example.2 * 1e5).round() / 1e5]},
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
    for e in std::fs::read_dir(&dir)?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.ends_with(".jsonl") && !written.contains(&n) {
            std::fs::remove_file(e.path()).ok();
        }
    }
    put(&dir.join("README.md"), TRANSLATORS.as_bytes())?;
    put(&dir.join("check.py"), CHECK.as_bytes())?;

    // The descriptions' lists.
    let ddir = out.join("descriptions/todo");
    std::fs::create_dir_all(&ddir)?;
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
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_or_not() {
        assert!(reads_english("Lake Louise"));
        assert!(reads_english("Main Street"));
        assert!(reads_english("O'Connell Bridge"));
        assert!(!reads_english("Rue Principale"));
        assert!(!reads_english("Lac des Sables"));
        assert!(!reads_english("Montréal"));
        assert!(!reads_english("Ffordd Caergybi"));
        assert!(!reads_english("中山"));
    }

    fn spoken() -> Spoken {
        let p = |x: f64, y: f64| [(x * 1e7) as i32, (y * 1e7) as i32];
        let rect = |code: &str, w: f64, s: f64, e: f64, n: f64| names::spoken::Area { code: code.into(), area_km2: (e - w) * (n - s), polygons: vec![vec![vec![p(w, s), p(e, s), p(e, n), p(w, n)]]] };
        Spoken::build([rect("JP", 128.0, 30.0, 146.0, 46.0), rect("FR", -5.0, 42.0, 8.0, 51.0), rect("CA", -80.0, 42.0, -60.0, 60.0), rect("CA-QC", -75.0, 45.0, -65.0, 55.0)])
    }

    #[test]
    fn what_goes_on_the_lists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("0-converted")).unwrap();
        std::fs::write(dir.path().join("0-converted/x.jsonl"), "{\"n\": \"Lac Bleu\", \"kind\": \"other\", \"langs\": \"fr\", \"sub\": \"Blue Lake\"}\n").unwrap();
        let f = std::fs::File::options().write(true).open(dir.path().join("0-converted/x.jsonl")).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60)).unwrap();
        let names = Names::load(dir.path()).unwrap();
        let sp = spoken();
        let mut l = Lists { names: &names, spoken: &sp, entries: HashMap::new(), report: Report::default() };
        let t = |name: &'static str, kind, own, osm: &[&str], lon, lat, pr| Thing { name, kind, own, osm: osm.iter().filter_map(|x| Lang::parse(x)).collect(), lon, lat, id: format!("n{}", name.len()), priority: pr };
        l.add(t("Lac Bleu", Kind::Other, false, &[], 2.0, 46.0, 30.0)); // a line
        l.add(t("Lac Noir", Kind::Other, true, &[], 2.0, 46.0, 30.0)); // its own English
        l.add(t("Lac Noir", Kind::Other, false, &[], 2.0, 46.0, 31.0));
        l.add(t("Lac Noir", Kind::Other, false, &[], -73.0, 46.0, 35.0)); // Quebec: French first
        l.add(t("Lake Louise", Kind::Other, false, &[], -76.0, 44.0, 30.0)); // English in Canada
        l.add(t("Rue Haute", Kind::Road, false, &[], -79.0, 44.0, 20.0)); // French signs in Ontario
        l.add(t("中山", Kind::Other, false, &[], 139.0, 35.0, 40.0));
        l.add(t("Kêr", Kind::Settlement, false, &["br"], 2.0, 46.0, 50.0)); // OSM's language
        l.add(t("Sea", Kind::Other, false, &[], -30.0, 40.0, 50.0)); // nowhere
        let r = &l.report;
        assert_eq!((r.own, r.lined, r.english, r.nowhere), (1, 1, 1, 1));
        let get = |n: &str, k: Kind, lang: &str| l.entries.get(&(n.to_owned(), k, Lang::parse(lang).unwrap()));
        let e = get("Lac Noir", Kind::Other, "fr").expect("Lac Noir");
        assert_eq!((e.things, e.best, e.langs.iter().map(Lang::as_str).collect::<Vec<_>>()), (2, 35.0, vec!["fr"]));
        assert_eq!(get("Rue Haute", Kind::Road, "fr").map(|e| e.langs.len()), Some(1));
        assert!(get("中山", Kind::Other, "ja").is_some());
        assert!(get("Kêr", Kind::Settlement, "br").is_some());
        assert_eq!(l.entries.len(), 4);
    }
}
