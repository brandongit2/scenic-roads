//! Wikidata facts for the heritage sites, matched by their register ID, else through OpenStreetMap:
//! the heritage chain's first part, in its stand-in root (the heritage step).
//!
//! Each site in data/build/heritage.json whose register declares a Wikidata property
//! (`wikidata_property`, docs/inputs.md §6.5, as heritage-sources.json lists them: Mérimée's P380,
//! the NHLE's P1216 …) is looked up by its reference there (`ref`; a World Heritage site by its
//! site's id, `site`), and a site whose record links its Wikidata item (a wikidata.org link) by
//! that item, in batches: descriptions (en, fr, es, pt, ca), Wikipedia article titles in those
//! languages, the number of sitelinks (a notability measure), inception, and the labels of its type
//! (P31), architectural style (P149) and architect (P84). English Wikipedia's short descriptions
//! are then fetched for the English titles. Queries go to the declared `wikidata` source
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
//! the facts) and the pass's answers it adds to: ids.jsonl (per register ID: its result rows, []
//! for no item), wp.jsonl (per item: its articles), enwiki-shortdesc.json (per title; "" for none),
//! so a rerun only asks what is new. The files' bytes are as dem/heritagewd.py wrote them
//! (crate::pyjson's `dumps_spaced`).

use crate::pyjson::{dumps_spaced, loads, Py};
use crate::timings::{phase, Class};
use crate::wikiask::{urlencode, val, Asker};
use anyhow::{bail, Context, Result};
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

/// Writes through a temporary file and a rename: a run stopped midway leaves the old file.
pub(crate) fn write_atomic(p: &Path, text: &str) -> Result<()> {
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
    let (pad_x, pad_y) = (40.0 / (111320.0 * lat.to_radians().cos()), 40.0 / 110540.0);
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

/// Its query: the facts of the items with `prop` = each of `ids` (or the items themselves, "QID").
fn query(prop: &str, ids: &[String]) -> String {
    let q = prop == "QID";
    let values =
        ids.iter().map(|i| if q { format!("wd:{i}") } else { format!("\"{i}\"") }).collect::<Vec<_>>().join(" ");
    let bind = if q {
        format!("VALUES ?item {{ {values} }} BIND(STRAFTER(STR(?item), 'entity/') AS ?id)")
    } else {
        format!("VALUES ?id {{ {values} }} ?item wdt:{prop} ?id .")
    };
    let opt = LANGS
        .iter()
        .map(|l| {
            format!(
                "OPTIONAL {{ ?item schema:description ?dd_{l} FILTER(LANG(?dd_{l}) = \"{l}\") }}\n\
                 OPTIONAL {{ ?art_{l} schema:about ?item ; schema:isPartOf <https://{l}.wikipedia.org/> ; schema:name ?ww_{l} }}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let samples = LANGS
        .iter()
        .map(|l| format!("(SAMPLE(?dd_{l}) AS ?d_{l}) (SAMPLE(?ww_{l}) AS ?w_{l})"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "SELECT ?id ?item (SAMPLE(?sl0) AS ?sl) (SAMPLE(?inc0) AS ?inc)
      {samples}
      (GROUP_CONCAT(DISTINCT ?instL; SEPARATOR=\"|\") AS ?inst)
      (GROUP_CONCAT(DISTINCT ?styleL; SEPARATOR=\"|\") AS ?style)
      (GROUP_CONCAT(DISTINCT ?archL; SEPARATOR=\"|\") AS ?arch) WHERE {{
      {bind}
      OPTIONAL {{ ?item wikibase:sitelinks ?sl0 }}
      OPTIONAL {{ ?item wdt:P571 ?inc0 }}
      {opt}
      OPTIONAL {{ ?item wdt:P31 ?i . ?i rdfs:label ?instL FILTER(LANG(?instL) = \"en\") }}
      OPTIONAL {{ ?item wdt:P149 ?s . ?s rdfs:label ?styleL FILTER(LANG(?styleL) = \"en\") }}
      OPTIONAL {{ ?item wdt:P84 ?a . ?a rdfs:label ?archL FILTER(LANG(?archL) = \"en\") }}
    }} GROUP BY ?id ?item"
    )
}

/// Looks the batches up, two at a time: each batch's rows by its IDs ([] for none), in `jobs`' order.
fn look_up(ask: &Asker, endpoint: &str, jobs: &[(String, Vec<String>)]) -> Result<Vec<Vec<(String, Py)>>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: std::sync::Mutex<Vec<Option<Result<Vec<(String, Py)>>>>> =
        std::sync::Mutex::new((0..jobs.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| loop {
                let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((prop, keys)) = jobs.get(n) else { break };
                let r = ask.sparql(endpoint, &query(prop, keys)).map(|rows| {
                    let mut got: HashMap<&str, Vec<Py>> = HashMap::new();
                    for b in &rows {
                        if let Some(id) = val(b, "id") {
                            got.entry(id).or_default().push(b.clone());
                        }
                    }
                    keys.iter().map(|k| (k.clone(), Py::List(got.remove(k.as_str()).unwrap_or_default()))).collect()
                });
                if n % 25 == 0 {
                    eprintln!("  batches {}/{}", n + 1, jobs.len());
                }
                let failed = r.is_err();
                out.lock().unwrap()[n] = Some(r);
                if failed {
                    next.store(usize::MAX / 2, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|r| r.unwrap_or_else(|| bail!("a batch not looked up"))).collect()
}

/// A site's facts from one of its result rows.
fn record(b: &Py) -> Result<Vec<(String, Py)>> {
    let item = val(b, "item").context("a row without its item")?;
    let qid = item.rsplit('/').next().unwrap_or(item);
    let sl = match val(b, "sl") {
        Some(s) => Py::Int(s.trim().parse::<i128>().with_context(|| format!("sitelinks {s:?}"))?.to_string()),
        None => Py::Int("0".into()),
    };
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

/// Per item: the number of Wikipedia articles (any language) and their `lang|title` list.
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
    let _p = phase("looking up Wikipedia articles", Class::Net);
    let mut out = Vec::new();
    for (k, chunk) in qids.chunks(1000).enumerate() {
        let q = format!(
            "SELECT ?item (COUNT(DISTINCT ?art) AS ?n) (GROUP_CONCAT(?lt; SEPARATOR=\"\\t\") AS ?arts) WHERE {{
          VALUES ?item {{ {} }}
          ?art schema:about ?item ; schema:isPartOf ?site ; schema:inLanguage ?lang ; schema:name ?nm .
          FILTER(STRENDS(STR(?site), \".wikipedia.org/\"))
          BIND(CONCAT(?lang, \"|\", ?nm) AS ?lt)
        }} GROUP BY ?item",
            chunk.iter().map(|x| format!("wd:{x}")).collect::<Vec<_>>().join(" ")
        );
        let mut got: HashMap<String, Py> = HashMap::new();
        for b in ask.sparql(endpoint, &q)? {
            if let Some(it) = val(&b, "item") {
                got.insert(it.rsplit('/').next().unwrap_or(it).to_string(), b);
            }
        }
        for x in chunk {
            let r = match got.get(x) {
                Some(b) => dict(vec![
                    ("n", Py::Int(val(b, "n").map_or(Ok(0), |s| s.trim().parse::<i128>())?.to_string())),
                    ("arts", Py::List(val(b, "arts").unwrap_or("").split('\t').map(|s| Py::Str(s.into())).collect())),
                ]),
                None => dict(vec![("n", Py::Int("0".into())), ("arts", Py::List(vec![]))]),
            };
            out.push((x.clone(), r));
        }
        if k % 20 == 0 {
            eprintln!("  Wikipedia articles: {}/{}", k * 1000 + chunk.len(), qids.len());
        }
        progress(k * 1000 + chunk.len());
    }
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
    let _p = phase("fetching short descriptions from Wikipedia", Class::Net);
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
                crate::pyjson::dumps(&d).chars().take(200).collect::<String>()
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

/// A JSON-lines cache: each line by its key (`key` of the line), first places kept, last values.
pub(crate) fn read_lines(p: &Path, key: impl Fn(&Py) -> Result<String>) -> Result<Ordered<Py>> {
    let mut out = Ordered::new();
    if p.exists() {
        let text = std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
        for line in text.lines() {
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
    let p = phase("reading the sites and their registers", Class::Disk);
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
    let p = phase("matching the register IDs", Class::Compute);
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
    let p = phase("reading the register ID cache", Class::Disk);
    let id_cache = w.join("ids.jsonl");
    let mut seen = read_lines(&id_cache, |r| Ok(sk(str_at(r, "prop")?, str_at(r, "id")?)))?;
    drop(p);
    let mut jobs = Vec::new();
    for (prop, ids) in by_prop.iter() {
        let mut keys: Vec<String> = ids.keys.iter().filter(|k| !seen.has(&sk(prop, k))).cloned().collect();
        keys.sort();
        for c in keys.chunks(BATCH) {
            jobs.push((prop.clone(), c.to_vec()));
        }
    }
    let n_ids: usize = by_prop.vals.iter().map(|v| v.keys.len()).sum();
    eprintln!("{n_ids} register IDs, {} to look up", jobs.iter().map(|j| j.1.len()).sum::<usize>());
    let p = phase("looking up register IDs on Wikidata", Class::Net);
    let save = |seen: &mut Ordered<Py>, jobs: &[(String, Vec<String>)], res: Vec<Vec<(String, Py)>>| {
        for ((prop, _), rows) in jobs.iter().zip(res) {
            for (k, rows) in rows {
                seen.set(
                    &sk(prop, &k),
                    dict(vec![("prop", Py::Str(prop.clone())), ("id", Py::Str(k)), ("rows", rows)]),
                );
            }
        }
    };
    let res = look_up(&ask, &ends.wikidata, &jobs)?;
    save(&mut seen, &jobs, res);
    drop(p);
    let write_ids = |seen: &Ordered<Py>| -> Result<()> {
        let _p = phase("writing the register ID cache", Class::Disk);
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

    let p = phase("making the records", Class::Compute);
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
    let p = phase("indexing the OSM features with an item", Class::Compute);
    let cells = todo.iter().map(|&i| site_cell(&feats[i])).collect::<Result<HashSet<_>>>()?;
    let (grid, ofeats) = osm_index(&root.join("data/heritage/osm/named.geojsonseq"), &cells, &osm_ref)?;
    drop(p);
    let p = phase("linking sites through OSM", Class::Compute);
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
    let p = phase("looking up OSM-linked items on Wikidata", Class::Net);
    let jobs: Vec<(String, Vec<String>)> = new_q.chunks(BATCH).map(|c| ("QID".to_string(), c.to_vec())).collect();
    let res = look_up(&ask, &ends.wikidata, &jobs)?;
    save(&mut seen, &jobs, res);
    drop(p);
    write_ids(&seen)?;
    let p = phase("making the records", Class::Compute);
    for (q, sites) in osm_sites.iter() {
        for b in rows_of(&seen, "QID", q) {
            let r = record(&b)?;
            items.extend(sites.iter().map(|&i| item(i, "OSM", q, &r)));
        }
    }
    drop(p);

    // Wikipedia articles in any language, per item (wp.jsonl).
    let p = phase("reading the article cache", Class::Disk);
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
        let _p = phase("writing the article cache", Class::Disk);
        let mut s = String::new();
        for (_, r) in wp.iter() {
            s.push_str(&dumps_spaced(r));
            s.push('\n');
        }
        write_atomic(&wp_cache, &s)?;
    }
    let p = phase("making the records", Class::Compute);
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
        let _p = phase("writing the items", Class::Disk);
        let mut s = String::new();
        for r in &items {
            s.push_str(&dumps_spaced(r));
            s.push('\n');
        }
        write_atomic(&w.join("items.jsonl"), &s)?;
    }
    let p = phase("reading the short description cache", Class::Disk);
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
        let _p = phase("writing the short description cache", Class::Disk);
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
