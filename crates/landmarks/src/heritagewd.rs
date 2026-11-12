//! Wikidata facts for the heritage sites, matched by their register ID, else through OpenStreetMap:
//! the heritage chain's first part, in its stand-in root (the heritage step).
//!
//! Each site in data/build/heritage.json whose register declares a Wikidata property
//! (`wikidata_property`, docs/inputs.md §6.5, as heritage-sources.json lists them: Mérimée's P380,
//! the NHLE's P1216 …) is looked up by its reference there (`ref`; a World Heritage site by its
//! site's id, `site`), and a site whose record links its Wikidata item (a wikidata.org link) by
//! that item, in batches: descriptions (en, fr, es, pt, ca), Wikipedia article titles in those
//! languages, the number of sitelinks (a notability measure), inception, and the labels of its type
//! (P31), architectural style (P149) and architect (P84). A batch's query asks each fact in a UNION
//! branch holding the batch's IDs, so its cost is theirs (crate::wikiask::fact_union), each fact
//! picked the same way whatever order the rows come in (`grouped`). English Wikipedia's short
//! descriptions are then fetched for the English titles. Queries go to the declared `wikidata` source
//! (docs/inputs.md §6.9: QLever's endpoint), the short descriptions to the declared `wikipedia`
//! source, both through crate::wikiask.
//!
//! Sites with no item, or none with an article in those languages, are then linked through
//! OpenStreetMap: a feature tagged with a Wikidata item (data/heritage/osm/named.geojsonseq, the
//! heritage step's export of the pass's named places) at the site (its outline within 40 m) that
//! carries the same register ID (in one of the OSM tags a register declares, `osm_ref_tags`) or
//! clearly the same name (three quarters of the two names' words shared, NFKD-folded).
//!
//! Every item's Wikipedia articles in any language are counted (`wpn`); an item with none in the
//! languages above links its article in another language (`OTHER`).
//!
//! Writes data/heritage/wd/items.jsonl (one line per matched site: its index in heritage.json and
//! the facts) and the pass's answers it adds to: ids.jsonl (per register ID: a row per item, its
//! facts as `grouped` puts them together, [] for no item), wp.jsonl (per item: its articles),
//! enwiki-shortdesc.json (per title; "" for none), so a rerun only asks what is new. The files' bytes are as dem/heritagewd.py wrote them
//! (pyport::pyjson's `dumps_spaced`).

use crate::wikiask::{earliest, fact, fact_union, int, labels, least, urlencode, val, Asker};
use anyhow::{bail, Context, Result};
use det::Det;
use jobrt::timings::phase;
use pyport::pyjson::{dumps_spaced, loads, Py};
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::Path;
use unicode_normalization::char::canonical_combining_class;
use unicode_normalization::UnicodeNormalization;

const LANGS: [&str; 5] = ["en", "fr", "es", "pt", "ca"];
const BATCH: usize = 300;
/// Languages to link when an item has no article in `LANGS`, most relevant to the map's regions first.
const OTHER: [&str; 18] =
    ["cy", "ga", "gd", "gl", "eu", "ast", "an", "oc", "br", "co", "zh", "zh-yue", "de", "it", "nl", "ru", "pl", "ja"];
const STOP: &[&str] = &[
    "the", "of", "and", "de", "du", "des", "la", "le", "les", "l", "d", "et", "y", "del", "dels", "da", "do", "das",
    "dos", "e", "a", "an", "en", "el", "els", "i", "o", "at", "in", "on", "au", "aux", "sur", "near", "former",
    "ancien", "ancienne",
];
const NAME_KEYS: [&str; 10] = [
    "name",
    "name:en",
    "name:fr",
    "name:es",
    "name:pt",
    "name:ca",
    "official_name",
    "alt_name",
    "old_name",
    "loc_name",
];

/// Where the chain's queries go: the declared sources' URLs.
pub struct Endpoints {
    pub wikidata: String,
    pub wikipedia: String,
}

/// A dict that keeps its keys' first places, as Python's.
pub(crate) struct Ordered<V> {
    pub(crate) keys: Vec<String>,
    pub(crate) vals: Vec<V>,
    at: HashMap<String, usize>,
}

impl<V> Ordered<V> {
    pub(crate) fn new() -> Self {
        Ordered { keys: vec![], vals: vec![], at: HashMap::new() }
    }
    pub(crate) fn get(&self, k: &str) -> Option<&V> {
        self.at.get(k).map(|&i| &self.vals[i])
    }
    pub(crate) fn has(&self, k: &str) -> bool {
        self.at.contains_key(k)
    }
    pub(crate) fn set(&mut self, k: &str, v: V) {
        match self.at.get(k) {
            Some(&i) => self.vals[i] = v,
            None => {
                self.at.insert(k.to_string(), self.keys.len());
                self.keys.push(k.to_string());
                self.vals.push(v);
            }
        }
    }
    fn entry(&mut self, k: &str, v: impl FnOnce() -> V) -> &mut V {
        if !self.has(k) {
            self.set(k, v());
        }
        let i = self.at[k];
        &mut self.vals[i]
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.keys.iter().zip(&self.vals)
    }
}

/// A key of `seen`: a property (or "QID") and an ID.
fn sk(prop: &str, id: &str) -> String {
    format!("{prop}\u{0}{id}")
}

fn dict(kv: Vec<(&str, Py)>) -> Py {
    Py::Dict(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// Writes through a temporary file and a rename: a run stopped midway leaves the old file. A
/// symbolic link is written through, to the file it names (the stand-in root links the pass's
/// answers in: a rename onto the link would replace the link, and what was written would never
/// reach them).
pub(crate) fn write_atomic(p: &Path, text: &str) -> Result<()> {
    let p = &match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::canonicalize(p).with_context(|| format!("follow the link {}", p.display()))?
        }
        _ => p.to_path_buf(),
    };
    let tmp = p.with_file_name(format!("{}.tmp", p.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, p)?;
    Ok(())
}

/// The first Wikidata item a site's link names (`wikidata.org/wiki/Q<digits>`).
fn qid_link(url: &str) -> Option<String> {
    const AT: &str = "wikidata.org/wiki/Q";
    let mut from = 0;
    while let Some(k) = url[from..].find(AT) {
        let s = from + k + AT.len();
        let n = url[s..].bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 {
            return Some(format!("Q{}", &url[s..s + n]));
        }
        from = s;
    }
    None
}

/// A name's words: lower-cased, possessives dropped, NFKD without combining marks, ASCII letters
/// and digits, common abbreviations spelled out, stop words dropped.
fn tokens(s: &str) -> HashSet<String> {
    let s = s.to_lowercase().replace('’', "'").replace("'s ", " ").replace("'s", "");
    let s: String = s.nfkd().filter(|c| canonical_combining_class(*c) == 0).collect();
    let mut out = HashSet::new();
    for t in s.split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit())).filter(|t| !t.is_empty()) {
        let t = match t {
            "st" => "saint",
            "ste" => "sainte",
            "sta" => "santa",
            "sto" => "santo",
            "mt" => "mount",
            "ch" => "church",
            t => t,
        };
        if !STOP.contains(&t) {
            out.insert(t.to_string());
        }
    }
    out
}

/// Words shared by two names, of all their words (0 unless a word of 4+ letters is shared). Both
/// ways, so a part of a place ("Magdalen College, Kitchen") doesn't take the whole's item.
fn same_name(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let common = a.intersection(b).count();
    if common == 0 || !a.intersection(b).any(|t| t.len() >= 4 && !t.bytes().all(|c| c.is_ascii_digit())) {
        return 0.0;
    }
    common as f64 / a.union(b).count() as f64
}

fn point(site: &Py) -> Result<(f64, f64)> {
    let c = site.at("geometry").at("coordinates").list().context("a site without a point")?;
    let n = |v: &Py| v.number().context("a site's coordinate not a number");
    Ok((n(&c[0])?, n(&c[1])?))
}

/// The ~1 km grid cell (0.01°) a site's point is in: the one `osm_link` looks in.
fn site_cell(site: &Py) -> Result<(i64, i64)> {
    let (lon, lat) = point(site)?;
    Ok(((lon * 100.0) as i64, (lat * 100.0) as i64))
}

/// An OSM feature with a Wikidata item: its item, box, names' words and register refs.
struct Feat {
    qid: String,
    bbox: [f64; 4],
    names: Vec<HashSet<String>>,
    refs: HashSet<String>,
}

/// Python's `str.strip()`: its whitespace (the separators \x1c–\x1f too).
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// OSM features with a Wikidata item in `named` (geojsonseq): the grid of ~1 km cells → feature
/// indices, and the features. Only the cells of the sites to link (`cells`, as `site_cell` gives
/// them) are entered, and only the features in one of them kept (a large protected area's box
/// covers millions of cells). A cell lists its features in the file's order.
fn osm_index(
    named: &Path,
    cells: &HashSet<(i64, i64)>,
    osm_ref: &[(String, Vec<String>)],
) -> Result<(HashMap<(i64, i64), Vec<usize>>, Vec<Feat>)> {
    let mut by_x: std::collections::BTreeMap<i64, Vec<i64>> = Default::default();
    for &(cx, cy) in cells {
        by_x.entry(cx).or_default().push(cy);
    }
    for ys in by_x.values_mut() {
        ys.sort();
    }
    let ref_keys: Vec<&str> = osm_ref.iter().flat_map(|(_, ks)| ks.iter().map(String::as_str)).collect();
    let mut feats = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let f = std::fs::File::open(named).with_context(|| format!("open {}", named.display()))?;
    let mut r = std::io::BufReader::with_capacity(1 << 20, f);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if r.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        if !buf.windows(10).any(|w| w == b"\"wikidata\"") {
            continue;
        }
        let line = std::str::from_utf8(&buf).context("named.geojsonseq: not UTF-8")?;
        let g: serde_json::Value = serde_json::from_str(py_strip(line).trim_start_matches('\x1e'))
            .context("named.geojsonseq: a line not JSON")?;
        let p = &g["properties"];
        let s = |k: &str| p.get(k).and_then(|v| v.as_str());
        let qid = py_strip(s("wikidata").unwrap_or("").split(';').next().unwrap_or(""));
        if !(qid.len() > 1 && qid.starts_with('Q') && qid[1..].bytes().all(|c| c.is_ascii_digit())) {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        fn walk(c: &serde_json::Value, f: &mut dyn FnMut(f64, f64)) {
            if let Some(a) = c.as_array() {
                if let Some(x) = a.first().and_then(|v| v.as_f64()) {
                    f(x, a[1].as_f64().unwrap_or(f64::NAN));
                } else {
                    for v in a {
                        walk(v, f);
                    }
                }
            }
        }
        walk(&g["geometry"]["coordinates"], &mut |x, y| {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        });
        let (gx0, gx1) = ((x0 * 100.0) as i64 - 1, (x1 * 100.0) as i64 + 1);
        let (gy0, gy1) = ((y0 * 100.0) as i64 - 1, (y1 * 100.0) as i64 + 1);
        // The sites' cells within its box (inclusive).
        let mut hit = Vec::new();
        for (&cx, col) in by_x.range(gx0..=gx1) {
            let a = col.partition_point(|&y| y < gy0);
            let b = col.partition_point(|&y| y <= gy1);
            hit.extend(col[a..b].iter().map(|&cy| (cx, cy)));
        }
        if hit.is_empty() {
            continue;
        }
        let mut names = Vec::new();
        for k in NAME_KEYS {
            if let Some(v) = s(k).filter(|v| !v.is_empty()) {
                names.extend(v.split(';').map(tokens).filter(|t| !t.is_empty()));
            }
        }
        let refs = ref_keys
            .iter()
            .filter_map(|k| s(k).filter(|v| !v.is_empty()))
            .map(|v| py_strip(v).to_uppercase())
            .collect();
        let n = feats.len();
        feats.push(Feat { qid: qid.to_string(), bbox: [x0, y0, x1, y1], names, refs });
        for c in hit {
            grid.entry(c).or_default().push(n);
        }
    }
    Ok((grid, feats))
}

/// The Wikidata item of the OSM feature at a site: same register ID, else clearly the same name.
fn osm_link(
    site: &Py,
    rid: Option<&str>,
    grid: &HashMap<(i64, i64), Vec<usize>>,
    feats: &[Feat],
) -> Result<Option<String>> {
    let (lon, lat) = point(site)?;
    let (pad_x, pad_y) = (40.0 / (roadcore::geo::M_PER_DEG * lat.to_radians().dcos()), 40.0 / roadcore::geo::M_PER_DEG);
    let p = site.at("properties");
    let names: Vec<HashSet<String>> = [p.at("name"), p.at("name_en")]
        .iter()
        .map(|v| tokens(v.as_str().unwrap_or("")))
        .filter(|t| !t.is_empty())
        .collect();
    let rid = rid.map(str::to_uppercase);
    let mut best: Option<(&str, (bool, f64, f64))> = None;
    for &n in grid.get(&site_cell(site)?).map(Vec::as_slice).unwrap_or(&[]) {
        let f = &feats[n];
        let [x0, y0, x1, y1] = f.bbox;
        if !(x0 - pad_x <= lon && lon <= x1 + pad_x && y0 - pad_y <= lat && lat <= y1 + pad_y) {
            continue;
        }
        let idm = rid.as_ref().is_some_and(|r| f.refs.contains(r));
        let sim = names.iter().flat_map(|a| f.names.iter().map(move |b| same_name(a, b))).fold(0.0, f64::max);
        if !idm && sim < 0.75 {
            continue;
        }
        let k = (idm, sim, -((x1 - x0) * (y1 - y0)));
        if best.as_ref().is_none_or(|(_, b)| k.partial_cmp(b) == Some(std::cmp::Ordering::Greater)) {
            best = Some((&f.qid, k));
        }
    }
    Ok(best.map(|(q, _)| q.to_string()))
}

/// Its query: the facts of the items with `prop` = each of `ids` (or the items themselves,
/// "QID"), each fact's values as rows (`?id ?item ?k ?v`; a "QID" query's without `?id`), a UNION
/// of one branch a fact holding the IDs (crate::wikiask::fact_union), put together by `grouped`.
/// A `prop` query's "item" branch names each ID's items, whatever facts they have.
fn query(prop: &str, ids: &[String]) -> String {
    let q = prop == "QID";
    let values =
        ids.iter().map(|i| if q { format!("wd:{i}") } else { format!("\"{i}\"") }).collect::<Vec<_>>().join(" ");
    let items = if q {
        format!("VALUES ?item {{ {values} }}")
    } else {
        format!("VALUES ?id {{ {values} }} ?item wdt:{prop} ?id .")
    };
    let en = |p: &str| format!(r#"?item wdt:{p} ?o . ?o rdfs:label ?v FILTER(STR(LANG(?v)) = "en")"#);
    let mut facts: Vec<(String, String)> =
        vec![("sl".into(), "?item wikibase:sitelinks ?v".into()), ("inc".into(), "?item wdt:P571 ?v".into())];
    for l in LANGS {
        facts.push((format!("d_{l}"), format!(r#"?item schema:description ?v FILTER(STR(LANG(?v)) = "{l}")"#)));
        facts.push((
            format!("w_{l}"),
            format!("?art schema:about ?item ; schema:isPartOf <https://{l}.wikipedia.org/> ; schema:name ?v"),
        ));
    }
    facts.extend([("inst", "P31"), ("style", "P149"), ("arch", "P84")].map(|(k, p)| (k.into(), en(p))));
    if !q {
        facts.push(("item".into(), String::new()));
    }
    fact_union(if q { "?item ?k ?v" } else { "?id ?item ?k ?v" }, &items, facts)
}

/// The rows of a batch of `query(prop, ids)`, one a site's item as `record` reads them, by ID: an
/// ID's items in QID order, each's facts picked the same way whatever order the service's rows
/// come in (sitelinks; the earliest inception; per language, the least description and article,
/// where the service has two; the type's, style's and architect's English labels, distinct and
/// sorted, `|` between them). Every ID of a "QID" batch has its row (its item's), whatever
/// facts it has; another property's an ID no item holds has none.
fn grouped(prop: &str, ids: &[String], rows: &[Py]) -> Result<HashMap<String, Vec<Py>>> {
    let q = prop == "QID";
    let mut by: HashMap<&str, std::collections::BTreeMap<(usize, &str), Vec<&Py>>> = HashMap::new();
    for b in rows {
        let item = val(b, "item").context("a row without its item")?;
        let qid = item.rsplit('/').next().unwrap_or(item);
        let id = if q { qid } else { val(b, "id").context("a row without its ID")? };
        by.entry(id).or_default().entry((qid.len(), qid)).or_default().push(b);
    }
    let lit = |s: &str| dict(vec![("type", Py::Str("literal".into())), ("value", Py::Str(s.into()))]);
    let row = |id: &str, qid: &str, rows: &[&Py]| {
        let item = dict(vec![
            ("type", Py::Str("uri".into())),
            ("value", Py::Str(format!("http://www.wikidata.org/entity/{qid}"))),
        ]);
        let mut r: Vec<(String, Py)> = vec![("id".into(), lit(id)), ("item".into(), item)];
        if let Some(b) = fact(rows, "sl").filter(|b| int(b, "v", qid).is_some()).max_by_key(|b| int(b, "v", qid)) {
            r.push(("sl".into(), b.at("v").clone()));
        }
        if let Some(b) = earliest(rows, "inc") {
            r.push(("inc".into(), b.at("v").clone()));
        }
        for l in LANGS {
            for k in [format!("d_{l}"), format!("w_{l}")] {
                if let Some(v) = least(rows, &k).map(|b| b.at("v").clone()) {
                    r.push((k, v));
                }
            }
        }
        for k in ["inst", "style", "arch"] {
            if let Some(v) = labels(rows, k) {
                r.push((k.into(), lit(&v)));
            }
        }
        Py::Dict(r)
    };
    let mut out = HashMap::new();
    for id in ids {
        let mut got: Vec<Py> =
            by.get(id.as_str()).into_iter().flatten().map(|((_, qid), rs)| row(id, qid, rs)).collect();
        if q && got.is_empty() {
            got.push(row(id, id, &[]));
        }
        out.insert(id.clone(), got);
    }
    Ok(out)
}

/// Looks up `todo`'s IDs, each property's (or "QID"'s) `BATCH` a query
/// (crate::wikiask::Asker::sparql_batches: a batch too costly for the service asked in halves, an
/// ID too costly alone left out, asked again by the next run): each batch's rows by its IDs ([]
/// for none) set in `seen` and appended to its cache `path` as they come, a safe point after each
/// (what came is in the pass's answers, which a resumed run doesn't ask again).
fn look_up(
    ask: &Asker,
    endpoint: &str,
    todo: &[(String, Vec<String>)],
    seen: &mut Ordered<Py>,
    path: &Path,
) -> Result<()> {
    let n: usize = todo.iter().map(|(_, k)| k.len()).sum();
    let (mut done, mut batches) = (0, 0);
    for (prop, keys) in todo {
        ask.sparql_batches(endpoint, keys, BATCH, &|ks| query(prop, ks), &mut |ks, rows| {
            let mut got = grouped(prop, ks, &rows)?;
            let mut lines = String::new();
            for k in ks {
                let r = dict(vec![
                    ("prop", Py::Str(prop.clone())),
                    ("id", Py::Str(k.clone())),
                    ("rows", Py::List(got.remove(k).unwrap_or_default())),
                ]);
                lines.push_str(&dumps_spaced(&r));
                lines.push('\n');
                seen.set(&sk(prop, k), r);
            }
            append(path, &lines)?;
            done += ks.len();
            if batches % 25 == 0 {
                eprintln!("  looked up {done}/{n}");
            }
            batches += 1;
            jobrt::control::piece_saved()
        })?;
    }
    Ok(())
}

/// `text` appended to `p`, after its last whole line (a line an append cut short dropped first, as
/// `read_lines` drops it).
fn append(p: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::FileExt;
    let mut f = std::fs::OpenOptions::new().read(true).append(true).create(true).open(p)?;
    let len = f.metadata()?.len();
    let mut last = [b'\n'];
    if len > 0 {
        f.read_exact_at(&mut last, len - 1)?;
    }
    if last[0] != b'\n' {
        let old = std::fs::read(p)?;
        f.set_len(old.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1) as u64)?;
    }
    f.write_all(text.as_bytes()).with_context(|| format!("append to {}", p.display()))
}

/// A site's facts from one of its result rows (a value that isn't one left out, logged).
fn record(b: &Py) -> Result<Vec<(String, Py)>> {
    let item = val(b, "item").context("a row without its item")?;
    let qid = item.rsplit('/').next().unwrap_or(item);
    let sl = Py::Int(int(b, "sl", qid).unwrap_or(0).to_string());
    let langs = |pre: &str| {
        Py::Dict(
            LANGS
                .iter()
                .filter_map(|l| val(b, &format!("{pre}_{l}")).map(|v| (l.to_string(), Py::Str(v.into()))))
                .collect(),
        )
    };
    let mut rec = vec![
        ("qid".to_string(), Py::Str(qid.into())),
        ("sl".to_string(), sl),
        ("desc".to_string(), langs("d")),
        ("wiki".to_string(), langs("w")),
    ];
    if let Some(inc) = val(b, "inc") {
        rec.push(("inception".into(), Py::Str(inc.chars().take(10).collect())));
    }
    for k in ["inst", "style", "arch"] {
        if let Some(v) = val(b, k) {
            rec.push((k.into(), Py::List(v.split('|').map(|s| Py::Str(s.into())).collect())));
        }
    }
    Ok(rec)
}

fn item(i: usize, prop: &str, id: &str, rec: &[(String, Py)]) -> Py {
    let mut d = vec![
        ("i".to_string(), Py::Int(i.to_string())),
        ("prop".to_string(), Py::Str(prop.into())),
        ("id".to_string(), Py::Str(id.into())),
    ];
    d.extend(rec.iter().cloned());
    Py::Dict(d)
}

/// Items a query of Wikipedia articles asks.
const WP_BATCH: usize = 1000;

/// Per item: the number of Wikipedia articles (any language) and their `lang|title` list (an item
/// too costly a query alone left out: crate::wikiask::Asker::sparql_batches).
pub(crate) fn wikipedias(ask: &Asker, endpoint: &str, qids: &[String]) -> Result<Vec<(String, Py)>> {
    wikipedias_told(ask, endpoint, qids, &mut |_| {})
}

/// `wikipedias`, `progress` told how many are done after each batch.
pub(crate) fn wikipedias_told(
    ask: &Asker,
    endpoint: &str,
    qids: &[String],
    progress: &mut dyn FnMut(usize),
) -> Result<Vec<(String, Py)>> {
    let _p = phase("looking up Wikipedia articles");
    let query = |chunk: &[String]| {
        format!(
            "SELECT ?item (COUNT(DISTINCT ?art) AS ?n) (GROUP_CONCAT(?lt; SEPARATOR=\"\\t\") AS ?arts) WHERE {{
          VALUES ?item {{ {} }}
          ?art schema:about ?item ; schema:isPartOf ?site ; schema:inLanguage ?lang ; schema:name ?nm .
          FILTER(STRENDS(STR(?site), \".wikipedia.org/\"))
          BIND(CONCAT(?lang, \"|\", ?nm) AS ?lt)
        }} GROUP BY ?item",
            chunk.iter().map(|x| format!("wd:{x}")).collect::<Vec<_>>().join(" ")
        )
    };
    let (mut out, mut batches) = (Vec::new(), 0);
    ask.sparql_batches(endpoint, qids, WP_BATCH, &query, &mut |chunk, rows| {
        let mut got: HashMap<String, Py> = HashMap::new();
        for b in rows {
            if let Some(it) = val(&b, "item") {
                got.insert(it.rsplit('/').next().unwrap_or(it).to_string(), b);
            }
        }
        for x in chunk {
            let r = match got.get(x) {
                Some(b) => dict(vec![
                    ("n", Py::Int(int(b, "n", x).unwrap_or(0).to_string())),
                    ("arts", Py::List(val(b, "arts").unwrap_or("").split('\t').map(|s| Py::Str(s.into())).collect())),
                ]),
                None => dict(vec![("n", Py::Int("0".into())), ("arts", Py::List(vec![]))]),
            };
            out.push((x.clone(), r));
        }
        if batches % 20 == 0 {
            eprintln!("  Wikipedia articles: {}/{}", out.len(), qids.len());
        }
        batches += 1;
        progress(out.len());
        Ok(())
    })?;
    Ok(out)
}

/// The item's article in another language: the first of `OTHER` it has, else its least language's.
fn other_article(arts: &Py) -> Option<(String, String)> {
    let mut have: Vec<(String, String)> = Vec::new();
    for a in arts.list().into_iter().flatten().filter_map(Py::as_str) {
        if let Some((l, t)) = a.split_once('|') {
            match have.iter_mut().find(|(k, _)| k == l) {
                Some(e) => e.1 = t.to_string(),
                None => have.push((l.to_string(), t.to_string())),
            }
        }
    }
    OTHER.iter().find_map(|l| have.iter().find(|(k, _)| k == l).cloned()).or_else(|| have.into_iter().min())
}

/// English Wikipedia's short descriptions, 50 titles a request: every title asked for is in the
/// answer ("" when it has none), and the titles they were normalized or redirected from. A batch
/// that can't be fetched fails the run.
fn shortdescs(ask: &Asker, api: &str, titles: &[String]) -> Result<Ordered<String>> {
    let _p = phase("fetching short descriptions from Wikipedia");
    let mut out = Ordered::new();
    for (k, chunk) in titles.chunks(50).enumerate() {
        let joined = chunk.join("|");
        let url = format!(
            "{api}?{}",
            urlencode(&[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("prop", "pageprops"),
                ("ppprop", "wikibase-shortdesc"),
                ("redirects", "1"),
                ("titles", &joined),
            ])
        );
        let d = ask.get_json(&url)?;
        let q = d.get("query");
        if d.get("error").is_some() || q.is_none() {
            bail!(
                "short descriptions: titles {}–{}: an answer without a query: {}",
                k * 50,
                k * 50 + chunk.len(),
                pyport::pyjson::dumps(&d).chars().take(200).collect::<String>()
            );
        }
        let q = q.unwrap();
        for t in chunk {
            out.set(t, String::new());
        }
        let mut back: HashMap<String, String> = HashMap::new();
        for n in q.at("normalized").list().into_iter().flatten().chain(q.at("redirects").list().into_iter().flatten()) {
            back.insert(n.at("to").py_str(), n.at("from").py_str());
        }
        for p in q.at("pages").list().into_iter().flatten() {
            let Some(sd) = p.at("pageprops").at("wikibase-shortdesc").as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let mut t = p.at("title").py_str();
            for _ in 0..100 {
                let Some(f) = back.get(&t) else { break };
                out.set(&t, sd.to_string());
                t = f.clone();
            }
            out.set(&t, sd.to_string());
        }
        if k % 20 == 0 {
            eprintln!("  short descriptions: {}/{}", k * 50 + chunk.len(), titles.len());
        }
    }
    Ok(out)
}

/// A JSON-lines cache: each line by its key (`key` of the line), first places kept, last values;
/// a last line without its newline (an append cut short: `look_up`'s) dropped.
pub(crate) fn read_lines(p: &Path, key: impl Fn(&Py) -> Result<String>) -> Result<Ordered<Py>> {
    let mut out = Ordered::new();
    if p.exists() {
        let text = std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
        for line in text.split_inclusive('\n').filter(|l| l.ends_with('\n')).map(str::trim_end) {
            if line.is_empty() {
                continue;
            }
            let r = loads(line).with_context(|| format!("a line of {}", p.display()))?;
            out.set(&key(&r)?, r);
        }
    }
    Ok(out)
}

fn str_at<'a>(r: &'a Py, k: &str) -> Result<&'a str> {
    r.at(k).as_str().with_context(|| format!("`{k}` not a string"))
}

/// Matches the sites in `root`'s data/build/heritage.json to Wikidata: writes
/// data/heritage/wd/items.jsonl and adds what it asked to the pass's answers there.
pub fn run(root: &Path, ends: &Endpoints) -> Result<()> {
    let b = root.join("data/build");
    let w = root.join("data/heritage/wd");
    let ask = Asker::new();
    let p = phase("reading the sites and their registers");
    std::fs::create_dir_all(&w)?;
    let fc = loads(&std::fs::read_to_string(b.join("heritage.json"))?).context("parse heritage.json")?;
    let feats = fc.at("features").list().context("heritage.json: no features")?;
    let srcs =
        loads(&std::fs::read_to_string(b.join("heritage-sources.json"))?).context("parse heritage-sources.json")?;
    let registers = srcs.at("registers");
    let osm_ref: Vec<(String, Vec<String>)> = match registers {
        Py::Dict(d) => d
            .iter()
            .filter(|(_, r)| r.at("osm_ref_tags").truthy())
            .map(|(k, r)| {
                let tags = r.at("osm_ref_tags").list().into_iter().flatten().filter_map(Py::as_str).map(str::to_string);
                (k.clone(), tags.collect())
            })
            .collect(),
        _ => vec![],
    };
    drop(p);
    let p = phase("matching the register IDs");
    let mut by_prop: Ordered<Ordered<Vec<usize>>> = Ordered::new();
    for (i, f) in feats.iter().enumerate() {
        let p = f.at("properties");
        let d = p.at("register").as_str().map_or(&Py::Null, |r| registers.at(r));
        let prop = d.at("wikidata_property");
        if prop.truthy() {
            let prop = prop.as_str().context("a wikidata_property not a string")?;
            let key = if d.at("class").as_str() == Some("world-heritage") { p.at("site") } else { p.at("ref") };
            if key.truthy() {
                let key = key.as_str().context("a site's register ID not a string")?;
                by_prop.entry(prop, Ordered::new).entry(key, Vec::new).push(i);
            }
            continue;
        }
        if let Some(q) = p.at("url").as_str().and_then(qid_link) {
            by_prop.entry("QID", Ordered::new).entry(&q, Vec::new).push(i);
        }
    }
    eprintln!("{}", by_prop.iter().map(|(p, v)| format!("{p}: {}", v.keys.len())).collect::<Vec<_>>().join(", "));
    drop(p);

    // Results per register ID (ids.jsonl: prop, id, result rows, [] = no item).
    let p = phase("reading the register ID cache");
    let id_cache = w.join("ids.jsonl");
    let mut seen = read_lines(&id_cache, |r| Ok(sk(str_at(r, "prop")?, str_at(r, "id")?)))?;
    drop(p);
    let todo: Vec<(String, Vec<String>)> = by_prop
        .iter()
        .map(|(prop, ids)| {
            let mut keys: Vec<String> = ids.keys.iter().filter(|k| !seen.has(&sk(prop, k))).cloned().collect();
            keys.sort();
            (prop.clone(), keys)
        })
        .collect();
    let n_ids: usize = by_prop.vals.iter().map(|v| v.keys.len()).sum();
    eprintln!("{n_ids} register IDs, {} to look up", todo.iter().map(|j| j.1.len()).sum::<usize>());
    let p = phase("looking up register IDs on Wikidata");
    look_up(&ask, &ends.wikidata, &todo, &mut seen, &id_cache)?;
    drop(p);
    let write_ids = |seen: &Ordered<Py>| -> Result<()> {
        let _p = phase("writing the register ID cache");
        let mut s = String::new();
        for (_, r) in seen.iter() {
            s.push_str(&dumps_spaced(r));
            s.push('\n');
        }
        write_atomic(&id_cache, &s)
    };
    write_ids(&seen)?;
    let rows_of = |seen: &Ordered<Py>, prop: &str, id: &str| -> Vec<Py> {
        seen.get(&sk(prop, id)).and_then(|r| r.at("rows").list().cloned()).unwrap_or_default()
    };

    let p = phase("making the records");
    let mut items: Vec<Py> = Vec::new();
    let mut rid_of: HashMap<usize, String> = HashMap::new();
    for (prop, ids) in by_prop.iter() {
        for (rid, sites) in ids.iter() {
            let recs = rows_of(&seen, prop, rid).iter().map(record).collect::<Result<Vec<_>>>()?;
            for &i in sites {
                rid_of.insert(i, rid.clone());
                items.extend(recs.iter().map(|r| item(i, prop, rid, r)));
            }
        }
    }
    drop(p);

    // Sites with no item, or none with an article in LANGS: the item of the OSM feature there.
    let linked: HashSet<usize> =
        items.iter().filter(|r| r.at("wiki").truthy()).filter_map(|r| r.at("i").number()).map(|n| n as usize).collect();
    let todo: Vec<usize> = (0..feats.len()).filter(|i| !linked.contains(i)).collect();
    eprintln!("OSM links: indexing features with a Wikidata item; {} sites to link", todo.len());
    let p = phase("indexing the OSM features with an item");
    let cells = todo.iter().map(|&i| site_cell(&feats[i])).collect::<Result<HashSet<_>>>()?;
    let (grid, ofeats) = osm_index(&root.join("data/heritage/osm/named.geojsonseq"), &cells, &osm_ref)?;
    drop(p);
    let p = phase("linking sites through OSM");
    let have_q: HashSet<(usize, String)> =
        items.iter().map(|r| (r.at("i").number().unwrap_or(-1.0) as usize, r.at("qid").py_str())).collect();
    let mut osm_sites: Ordered<Vec<usize>> = Ordered::new();
    for &i in &todo {
        let reg = feats[i].at("properties").at("register").as_str().unwrap_or("");
        let rid = rid_of.get(&i).map(String::as_str).filter(|_| osm_ref.iter().any(|(r, _)| r == reg));
        if let Some(q) = osm_link(&feats[i], rid, &grid, &ofeats)? {
            if !have_q.contains(&(i, q.clone())) {
                osm_sites.entry(&q, Vec::new).push(i);
            }
        }
    }
    let mut new_q: Vec<String> = osm_sites.keys.iter().filter(|q| !seen.has(&sk("QID", q))).cloned().collect();
    new_q.sort();
    drop(p);
    eprintln!(
        "OSM links: {} sites to {} items ({} to look up)",
        osm_sites.vals.iter().map(Vec::len).sum::<usize>(),
        osm_sites.keys.len(),
        new_q.len()
    );
    let p = phase("looking up OSM-linked items on Wikidata");
    look_up(&ask, &ends.wikidata, &[("QID".to_string(), new_q)], &mut seen, &id_cache)?;
    drop(p);
    write_ids(&seen)?;
    let p = phase("making the records");
    for (q, sites) in osm_sites.iter() {
        for b in rows_of(&seen, "QID", q) {
            let r = record(&b)?;
            items.extend(sites.iter().map(|&i| item(i, "OSM", q, &r)));
        }
    }
    drop(p);

    // Wikipedia articles in any language, per item (wp.jsonl).
    let p = phase("reading the article cache");
    let wp_cache = w.join("wp.jsonl");
    let mut wp = read_lines(&wp_cache, |r| Ok(r.at("qid").py_str()))?;
    drop(p);
    let mut need: Vec<String> =
        items.iter().map(|r| r.at("qid").py_str()).filter(|q| !wp.has(q)).collect::<HashSet<_>>().into_iter().collect();
    need.sort();
    eprintln!("Wikipedia articles: {} items to look up", need.len());
    for (q, r) in wikipedias(&ask, &ends.wikidata, &need)? {
        let mut d = vec![("qid".to_string(), Py::Str(q.clone()))];
        if let Py::Dict(e) = r {
            d.extend(e);
        }
        wp.set(&q, Py::Dict(d));
    }
    {
        let _p = phase("writing the article cache");
        let mut s = String::new();
        for (_, r) in wp.iter() {
            s.push_str(&dumps_spaced(r));
            s.push('\n');
        }
        write_atomic(&wp_cache, &s)?;
    }
    let p = phase("making the records");
    let empty = Py::Dict(vec![]);
    for r in items.iter_mut() {
        let w = wp.get(&r.at("qid").py_str()).unwrap_or(&empty);
        r.set("wpn", w.get("n").cloned().unwrap_or(Py::Int("0".into())));
        if !r.at("wiki").truthy() {
            if let Some((l, t)) = other_article(w.at("arts")) {
                r.get_mut("wiki").context("an item without `wiki`")?.set(&l, Py::Str(t));
            }
        }
    }
    items.sort_by_key(|r| r.at("i").number().unwrap_or(0.0) as usize);
    drop(p);
    let with: HashSet<usize> =
        items.iter().filter(|r| r.at("wiki").truthy()).filter_map(|r| r.at("i").number()).map(|n| n as usize).collect();
    eprintln!("{} of {} sites with a Wikipedia article", with.len(), feats.len());
    {
        let _p = phase("writing the items");
        let mut s = String::new();
        for r in &items {
            s.push_str(&dumps_spaced(r));
            s.push('\n');
        }
        write_atomic(&w.join("items.jsonl"), &s)?;
    }
    let p = phase("reading the short description cache");
    let mut titles: Vec<String> = items
        .iter()
        .filter_map(|r| r.at("wiki").get("en"))
        .map(Py::py_str)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    titles.sort();
    let sd_path = w.join("enwiki-shortdesc.json");
    let mut have = if sd_path.exists() {
        loads(&std::fs::read_to_string(&sd_path)?).context("parse enwiki-shortdesc.json")?
    } else {
        Py::Dict(vec![])
    };
    let Py::Dict(hd) = &mut have else { bail!("enwiki-shortdesc.json: not an object") };
    let mut at: HashMap<String, usize> = hd.iter().enumerate().map(|(k, (t, _))| (t.clone(), k)).collect();
    let todo: Vec<String> = titles.iter().filter(|t| !at.contains_key(*t)).cloned().collect();
    drop(p);
    eprintln!(
        "{} sites matched to Wikidata; {} English articles ({} short descriptions to fetch)",
        items.len(),
        titles.len(),
        todo.len()
    );
    // (Titles with no description are "": not asked again.)
    let got = shortdescs(&ask, &ends.wikipedia, &todo)?;
    for (t, sd) in got.iter() {
        match at.get(t) {
            Some(&k) => hd[k].1 = Py::Str(sd.clone()),
            None => {
                at.insert(t.clone(), hd.len());
                hd.push((t.clone(), Py::Str(sd.clone())));
            }
        }
    }
    {
        let _p = phase("writing the short description cache");
        write_atomic(&sd_path, &dumps_spaced(&have))?;
    }
    eprintln!("done: {} short descriptions", at.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(s: &[&str]) -> HashSet<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn name_tokens() {
        assert_eq!(tokens("St. Mary’s Church of the Île"), set(&["saint", "mary", "church", "ile"]));
        assert_eq!(tokens("Château de Ste-Anne"), set(&["chateau", "sainte", "anne"]));
        assert_eq!(tokens("ﬁeld Ⅻ"), set(&["field", "xii"]));
        assert!(tokens("the of").is_empty());
    }

    #[test]
    fn names_compared() {
        let a = set(&["magdalen", "college"]);
        assert_eq!(same_name(&a, &set(&["magdalen", "college", "kitchen"])), 2.0 / 3.0);
        assert_eq!(same_name(&set(&["1234", "x"]), &set(&["1234"])), 0.0);
        assert_eq!(same_name(&a, &set(&["oxford"])), 0.0);
    }

    #[test]
    fn links_and_other_articles() {
        assert_eq!(
            qid_link("https://www.wikidata.org/wiki/Qx https://www.wikidata.org/wiki/Q42#a"),
            Some("Q42".into())
        );
        assert_eq!(qid_link("https://example.org/"), None);
        let arts = loads(r#"["zz|Z", "de|D", "ga|G", "nolang"]"#).unwrap();
        assert_eq!(other_article(&arts), Some(("ga".into(), "G".into())));
        let arts = loads(r#"["zz|Z", "xa|X"]"#).unwrap();
        assert_eq!(other_article(&arts), Some(("xa".into(), "X".into())));
    }

    /// Task #1144: areadetails' park facts, linked into the stand-in root from the pass's answers,
    /// written by a rename onto the link replaced it: every batch went to the scratch root, and
    /// each run asked the same parks again. Written through the link, the answers get them.
    #[test]
    fn writes_go_through_a_link() {
        let d = tempfile::tempdir().unwrap();
        let (kept, link) = (d.path().join("areas-wikidata.json"), d.path().join("wikidata.json"));
        std::fs::write(&kept, "{}").unwrap();
        jobrt::sys::symlink(&kept, &link).unwrap();
        write_atomic(&link, "{\"Q1\": {}}").unwrap();
        write_atomic(&link, "{\"Q1\": {}, \"Q2\": {}}").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link kept");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "{\"Q1\": {}, \"Q2\": {}}");
        // (A plain file as before; nothing left beside either.)
        let plain = d.path().join("plain.json");
        write_atomic(&plain, "x").unwrap();
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "x");
        let mut names: Vec<String> =
            std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        names.sort();
        assert_eq!(names, ["areas-wikidata.json", "plain.json", "wikidata.json"]);
    }

    #[test]
    fn a_cut_short_append_dropped() {
        let p = std::env::temp_dir().join(format!("heritagewd-lines-{}.jsonl", std::process::id()));
        std::fs::write(&p, "{\"qid\": \"Q1\", \"n\": 1}\n").unwrap();
        append(&p, "{\"qid\": \"Q2\", \"n\": 2}\n{\"qid\": \"Q1\", \"n\": 3}\n{\"qid\": \"Q").unwrap();
        let lines = |p: &Path| {
            let r = read_lines(p, |r| Ok(r.at("qid").py_str())).unwrap();
            r.iter().map(|(k, v)| format!("{k} {}", dumps_spaced(v))).collect::<Vec<String>>()
        };
        assert_eq!(lines(&p), [r#"Q1 {"qid": "Q1", "n": 3}"#, r#"Q2 {"qid": "Q2", "n": 2}"#]);
        // (The next append goes after the last whole line.)
        append(&p, "{\"qid\": \"Q3\", \"n\": 0}\n").unwrap();
        assert_eq!(lines(&p)[2], r#"Q3 {"qid": "Q3", "n": 0}"#);
        assert!(std::fs::read_to_string(&p).unwrap().ends_with("3}\n{\"qid\": \"Q3\", \"n\": 0}\n"));
        std::fs::remove_file(&p).ok();
    }

    /// A row of a facts query's answer: ID `id`'s item `q`, its fact `k` (the value `v`, JSON; ""
    /// for none, the "item" branch's).
    fn fact_row(id: Option<&str>, q: &str, k: &str, v: &str) -> String {
        let id = id.map_or(String::new(), |i| format!(r#""id": {{"type": "literal", "value": "{i}"}}, "#));
        let v = if v.is_empty() { String::new() } else { format!(r#", "v": {v}"#) };
        format!(
            r#"{{{id}"item": {{"type": "uri", "value": "http://www.wikidata.org/entity/{q}"}}, "k": {{"type": "literal", "value": "{k}"}}{v}}}"#
        )
    }

    /// Task #1151: a batch's rows put together as `record` reads them, the same whatever order
    /// the service's rows come in (the old query's SAMPLE and GROUP_CONCAT took its order): an
    /// ID's items in QID order, each fact picked order-free. A "QID" batch's item with no facts
    /// still has its row (the old query's was all OPTIONALs); another property's ID no item holds
    /// has none.
    #[test]
    fn rows_grouped_the_same_way() {
        let lit = |s: &str| format!(r#"{{"type": "literal", "value": "{s}"}}"#);
        let lang = |s: &str, l: &str| format!(r#"{{"type": "literal", "value": "{s}", "xml:lang": "{l}"}}"#);
        let genid =
            r#"{"type": "uri", "value": "http://www.wikidata.org/.well-known/genid/9e0e785fb54b8e85182f3338680c3feb"}"#;
        let rows = [
            fact_row(Some("PA1"), "Q10", "item", ""),
            fact_row(Some("PA1"), "Q9", "sl", &lit("3")),
            fact_row(Some("PA1"), "Q9", "d_fr", &lang("église paroissiale", "fr")),
            fact_row(Some("PA1"), "Q9", "d_fr", &lang("église", "fr")),
            fact_row(Some("PA1"), "Q9", "w_en", &lang("St Paul's", "en")),
            fact_row(Some("PA1"), "Q9", "inc", &lit("1200-01-01T00:00:00Z")),
            fact_row(Some("PA1"), "Q9", "inc", genid),
            fact_row(Some("PA1"), "Q9", "inc", &lit("1150-01-01T00:00:00Z")),
            fact_row(Some("PA1"), "Q9", "inst", &lang("church", "en")),
            fact_row(Some("PA1"), "Q9", "inst", &lang("building", "en")),
            fact_row(Some("PA1"), "Q9", "inst", &lang("church", "en")),
            fact_row(Some("PA1"), "Q9", "item", ""),
        ];
        let ids: Vec<String> = ["PA1", "PA2"].map(String::from).into();
        let want = [
            r#"PA1 Q9 {"qid": "Q9", "sl": 3, "desc": {"fr": "église"}, "wiki": {"en": "St Paul's"}, "inception": "1150-01-01", "inst": ["building", "church"]}"#,
            r#"PA1 Q10 {"qid": "Q10", "sl": 0, "desc": {}, "wiki": {}}"#,
        ];
        let made = |rows: &[String]| {
            let parsed: Vec<Py> = rows.iter().map(|r| loads(r).unwrap()).collect();
            let mut by = grouped("P380", &ids, &parsed).unwrap();
            assert!(by.remove("PA2").unwrap().is_empty(), "an ID no item holds has no rows");
            let mut out = vec![];
            for b in by.remove("PA1").unwrap() {
                let item = val(&b, "item").unwrap().rsplit('/').next().unwrap().to_string();
                out.push(format!("{} {item} {}", val(&b, "id").unwrap(), dumps_spaced(&Py::Dict(record(&b).unwrap()))));
            }
            out
        };
        assert_eq!(made(&rows), want);
        assert_eq!(made(&rows.iter().rev().cloned().collect::<Vec<_>>()), want, "the same in any order");
        // A "QID" batch: every item its row.
        let rows = [fact_row(None, "Q1", "sl", &lit("2"))].map(|r| loads(&r).unwrap());
        let by = grouped("QID", &["Q1".into(), "Q404".into()], &rows).unwrap();
        let rec = |q: &str| dumps_spaced(&Py::Dict(record(&by[q][0]).unwrap()));
        assert_eq!(rec("Q1"), r#"{"qid": "Q1", "sl": 2, "desc": {}, "wiki": {}}"#);
        assert_eq!(rec("Q404"), r#"{"qid": "Q404", "sl": 0, "desc": {}, "wiki": {}}"#);
        assert_eq!(val(&by["Q404"][0], "id"), Some("Q404"));
        let q = query("P380", &ids);
        assert_eq!(
            q.matches(r#"VALUES ?id { "PA1" "PA2" } ?item wdt:P380 ?id ."#).count(),
            2 + 2 * LANGS.len() + 3 + 1
        );
        assert!(!q.contains("OPTIONAL") && !q.contains(" LANG("), "{q}");
        assert_eq!(query("QID", &["Q1".into()]).matches("VALUES ?item { wd:Q1 }").count(), 2 + 2 * LANGS.len() + 3);
    }

    /// A run on caches holding every answer asks nothing (the unit tests' process is offline) and
    /// writes what dem/heritagewd.py wrote for them.
    #[test]
    fn items_from_the_caches() {
        let t = std::env::temp_dir().join(format!("heritagewd-{}", std::process::id()));
        for d in ["data/build", "data/heritage/wd", "data/heritage/osm"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        let w = |p: &str, s: &str| std::fs::write(t.join(p), s).unwrap();
        w(
            "data/build/heritage.json",
            r#"{"features":[
{"geometry":{"type":"Point","coordinates":[2.35,48.85]},"properties":{"register":"fr","ref":"PA1","name":"Église Saint-Paul"}},
{"geometry":{"type":"Point","coordinates":[-0.1,51.5]},"properties":{"register":"x","ref":"9","name":"Old Mill","url":"https://www.wikidata.org/wiki/Q7"}},
{"geometry":{"type":"Point","coordinates":[-0.2,51.4]},"properties":{"register":"x","ref":"8","name":"St Anne's Chapel"}}]}"#,
        );
        w(
            "data/build/heritage-sources.json",
            r#"{"registers":{"fr":{"wikidata_property":"P380","osm_ref_tags":["ref:mhs"]},"x":{}}}"#,
        );
        w(
            "data/heritage/wd/ids.jsonl",
            "{\"prop\": \"P380\", \"id\": \"PA1\", \"rows\": [{\"id\": {\"type\": \"literal\", \"value\": \"PA1\"}, \"item\": {\"type\": \"uri\", \"value\": \"http://www.wikidata.org/entity/Q1\"}, \"sl\": {\"value\": \"3\"}, \"d_fr\": {\"value\": \"église\"}, \"inc\": {\"value\": \"1200-01-01T00:00:00Z\"}, \"inst\": {\"value\": \"church|building\"}}]}\n\
{\"prop\": \"QID\", \"id\": \"Q7\", \"rows\": []}\n\
{\"prop\": \"QID\", \"id\": \"Q9\", \"rows\": [{\"item\": {\"value\": \"http://www.wikidata.org/entity/Q9\"}, \"w_en\": {\"value\": \"St Anne's Chapel, London\"}}]}\n",
        );
        w("data/heritage/wd/wp.jsonl", "{\"qid\": \"Q1\", \"n\": 2, \"arts\": [\"de|Paulskirche\", \"cy|Eglwys\"]}\n{\"qid\": \"Q9\", \"n\": 1, \"arts\": [\"en|St Anne's Chapel, London\"]}\n");
        w("data/heritage/wd/enwiki-shortdesc.json", r#"{"St Anne's Chapel, London": "Chapel in London"}"#);
        w(
            "data/heritage/osm/named.geojsonseq",
            "\x1e{\"type\":\"Feature\",\"geometry\":{\"type\":\"Polygon\",\"coordinates\":[[[-0.2001,51.3999],[-0.1999,51.3999],[-0.1999,51.4001],[-0.2001,51.3999]]]},\"properties\":{\"name\":\"Saint Anne Chapel\",\"wikidata\":\"Q9;Q10\"}}\n\
\x1e{\"type\":\"Feature\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[5,5]},\"properties\":{\"name\":\"Far\",\"wikidata\":\"Q11\"}}\n",
        );
        let ends =
            Endpoints { wikidata: "https://query.invalid/".into(), wikipedia: "https://wiki.invalid/w/api.php".into() };
        run(&t, &ends).unwrap();
        let r = |p: &str| std::fs::read_to_string(t.join(p)).unwrap();
        assert_eq!(
            r("data/heritage/wd/items.jsonl"),
            "{\"i\": 0, \"prop\": \"P380\", \"id\": \"PA1\", \"qid\": \"Q1\", \"sl\": 3, \"desc\": {\"fr\": \"église\"}, \"wiki\": {\"cy\": \"Eglwys\"}, \"inception\": \"1200-01-01\", \"inst\": [\"church\", \"building\"], \"wpn\": 2}\n\
{\"i\": 2, \"prop\": \"OSM\", \"id\": \"Q9\", \"qid\": \"Q9\", \"sl\": 0, \"desc\": {}, \"wiki\": {\"en\": \"St Anne's Chapel, London\"}, \"wpn\": 1}\n"
        );
        assert_eq!(r("data/heritage/wd/enwiki-shortdesc.json"), r#"{"St Anne's Chapel, London": "Chapel in London"}"#);
        std::fs::remove_dir_all(&t).ok();
    }
}
