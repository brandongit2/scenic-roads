//! The heritage overlays' lean files (layer-*.json in the heritage chain's `data/build`), from
//! which the marks and overlays jobs make what the map loads by view: only the properties the map
//! draws, filters and labels with, in draw order, with polygons simplified. The heritage step
//! (`scenic-build heritage`) writes them as the chain's last part.
//!
//!   heritage         points sorted by fame (fa, least known first), so the best known are drawn
//!                    on top without a sort key (which would cost a draw call per dot). A World
//!                    Heritage Site in several components (whs-sites.json, from crate::whsshapes) is
//!                    one dot at its centre (with the lead component's record and np, the number
//!                    of components), and its components parts (pt 1, cn the component's name),
//!                    drawn small close in. Sites keep what the dots, labels, filters and hover
//!                    title use (`KEEP`); the rest of their properties (dates, authority, source,
//!                    links …) go to props-heritage.jsonl by record, which the marks job joins
//!                    into the sites' popup records.
//!   whs-shapes       the World Heritage outlines with i, their site's record (its dot's).
//!   heritage-areas   simplified to ~1 m (topology preserved: crate::tpsimplify); coordinates to
//!                    6 decimals.
//!   indigenous, special   coordinates to 6 decimals.
//!
//! Named features carry `en`, their own English where it truly differs (`english_at`: a heritage
//! site's UNESCO name or English Wikipedia title, an area's `name_en`); any other English is the
//! map's server's, from the translations' live tables, as it serves them.
//!
//! A point's name shows from mz (interest.py). A name repeated nearby waits longer
//! (crate::interest::same_names). Only the labels wait: the dots, counts and lists are the same.
//!
//! Also layer-summary.json: per polygon overlay, the feature count and each feature's area (km²),
//! for the Layers panel's counts under the area filters, without loading the polygons.
//!
//! Every file is byte for byte what the chain's Python wrote before (crate::pyjson).

use crate::interest::{py_round, same_names};
use crate::pyjson::{dumps, loads, Py};
use crate::pytext::{circled, differs, word_char};
use crate::timings::{phase, Class};
use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use unicode_normalization::char::is_combining_mark;

/// Heritage properties kept on the map (style, filters, labels, hover title, Sights list).
const KEEP: [&str; 18] = [
    "i",
    "name",
    "en",
    "designation",
    "t",
    "level",
    "fa",
    "ia",
    "mz",
    "pv",
    "sl",
    "dy",
    "by",
    "wp",
    "approx",
    "np",
    "pt",
    "cn",
];

/// The heritage areas' simplification tolerance (degrees, ~1 m).
const SIMPLIFY_DEG: f64 = 1e-5;

/// Writes the layers into `b` (the chain's `data/build`) from its files there, the sites' English
/// Wikipedia titles from `items` (crate::heritagewd's matches, `wd/items.jsonl`, when there).
pub fn write_all(b: &Path, items: &Path) -> Result<()> {
    let _p = phase("the map's layers", Class::Mixed);
    let record = heritage(b, items)?;
    if b.join("whs-shapes.json").exists() {
        whs_outlines(b, &record)?;
    }
    let mut summary = Py::Dict(Vec::new());
    for (key, src, tol) in [
        ("heritageAreas", "heritage-areas", SIMPLIFY_DEG),
        ("indigenous", "indigenous", 0.0),
        ("special", "special", 0.0),
    ] {
        summary.set(key, polygons(b, src, tol)?);
    }
    let _p = phase("the layers written", Class::Disk);
    write(b, "layer-summary.json", &summary)
}

fn read(b: &Path, name: &str) -> Result<Py> {
    let _p = phase("the layers read", Class::Disk);
    let text = std::fs::read_to_string(b.join(name)).with_context(|| format!("read {name}"))?;
    loads(&text).with_context(|| format!("parse {name}"))
}

/// Through a temporary file, as the chain's files are.
fn write(b: &Path, name: &str, v: &Py) -> Result<()> {
    write_text(b, name, &dumps(v))
}

fn write_text(b: &Path, name: &str, text: &str) -> Result<()> {
    let tmp = b.join(format!("{name}.tmp"));
    std::fs::write(&tmp, text).with_context(|| format!("write {name}"))?;
    std::fs::rename(&tmp, b.join(name))?;
    Ok(())
}

fn features(fc: &mut Py) -> Result<&mut Vec<Py>> {
    match fc.get_mut("features") {
        Some(Py::List(a)) => Ok(a),
        _ => bail!("not a FeatureCollection"),
    }
}

/// A feature's properties (a dict).
fn props(f: &Py) -> &Py {
    f.at("properties")
}

fn props_mut(f: &mut Py) -> Result<&mut Py> {
    f.get_mut("properties").filter(|p| matches!(p, Py::Dict(_))).context("a feature without properties")
}

/// `d.get(k)`, with None as none.
fn val<'a>(p: &'a Py, k: &str) -> Option<&'a Py> {
    p.get(k).filter(|v| **v != Py::Null)
}

fn coords(f: &Py) -> Option<&Py> {
    f.get("geometry")?.get("coordinates")
}

/// A point's position.
fn position(f: &Py) -> Result<[f64; 2]> {
    let c = coords(f).and_then(Py::list).context("a point without coordinates")?;
    match (c.first().and_then(Py::number), c.get(1).and_then(Py::number)) {
        (Some(x), Some(y)) => Ok([x, y]),
        _ => bail!("a point without coordinates"),
    }
}

/// Coordinates to 6 decimals (~0.1 m); integers as they are.
fn rounded(g: &Py) -> Py {
    match g {
        Py::List(a) if matches!(a.first(), Some(Py::Int(_) | Py::Float(_))) => Py::List(
            a.iter()
                .map(|v| match v {
                    Py::Float(x) => Py::Float(py_round(*x, 6)),
                    v => v.clone(),
                })
                .collect(),
        ),
        Py::List(a) => Py::List(a.iter().map(rounded).collect()),
        g => g.clone(),
    }
}

fn round_geometry(f: &mut Py) -> Result<()> {
    let g = f.get_mut("geometry").context("no geometry")?;
    if let Some(c) = g.get("coordinates") {
        let r = rounded(c);
        g.set("coordinates", r);
    }
    Ok(())
}

/// The World Heritage id of a heritage.json site (its register's `site`), else none.
pub(crate) fn whs_id(p: &Py) -> Option<String> {
    let level_one = p.at("level").number() == Some(1.0);
    (level_one && p.at("site").truthy()).then(|| p.at("site").py_str())
}

/// Per feature, for the World Heritage Sites in several components (those with a `dot` in
/// whs-sites.json): (true, id) for the component the site's dot takes its record from (the first at
/// the site's `lead` position), (false, id) for the others.
fn roles(feats: &[Py], sites: &Py) -> Result<HashMap<usize, (bool, String)>> {
    let mut out = HashMap::new();
    let mut led: HashSet<String> = HashSet::new();
    for (i, f) in feats.iter().enumerate() {
        let Some(sid) = whs_id(props(f)) else { continue };
        let Some(s) = sites.get(&sid).filter(|s| s.get("dot").is_some()) else { continue };
        let at: Vec<f64> = coords(f)
            .and_then(Py::list)
            .context("a site without coordinates")?
            .iter()
            .take(2)
            .map(|c| c.number().map_or(f64::NAN, |x| py_round(x, 6)))
            .collect();
        let lead_at: Option<Vec<f64>> =
            s.at("lead").list().map(|l| l.iter().map(|c| c.number().unwrap_or(f64::NAN)).collect());
        let lead = !led.contains(&sid) && lead_at.as_ref() == Some(&at);
        if lead {
            led.insert(sid.clone());
        }
        out.insert(i, (lead, sid));
    }
    Ok(out)
}

/// The heritage sites with each World Heritage Site in several components as one dot at its
/// centre, and its components as parts; and each World Heritage id's record (its dot's: the lead
/// component's, else its only one's).
fn merged_sites(b: &Path, feats: Vec<Py>) -> Result<(Vec<Py>, HashMap<String, Py>)> {
    let sites = match b.join("whs-sites.json").exists() {
        true => read(b, "whs-sites.json")?,
        false => Py::Dict(Vec::new()),
    };
    let role = roles(&feats, &sites)?;
    let mut record: HashMap<String, Py> = HashMap::new();
    for (i, f) in feats.iter().enumerate() {
        let p = props(f);
        if let (Some(sid), Some(rec)) = (whs_id(p), p.get("i")) {
            if !record.contains_key(&sid) || role.get(&i).is_some_and(|r| r.0) {
                record.insert(sid, rec.clone());
            }
        }
    }
    let mut out = Vec::with_capacity(feats.len() + role.len());
    let n_lead = role.values().filter(|r| r.0).count();
    for (i, f) in feats.into_iter().enumerate() {
        let Some((lead, sid)) = role.get(&i) else {
            out.push(f);
            continue;
        };
        let Py::Dict(p) = props(&f).clone() else { bail!("a feature without properties") };
        let site = sites.get(sid).context("a site's record")?;
        if *lead {
            let mut q = Py::Dict(p.iter().filter(|(k, _)| k != "component").cloned().collect());
            q.set("np", site.at("n").clone());
            let geometry = Py::Dict(vec![
                ("type".into(), Py::Str("Point".into())),
                ("coordinates".into(), site.at("dot").clone()),
            ]);
            out.push(feature(geometry, q));
        }
        let mut part =
            Py::Dict(p.iter().filter(|(k, _)| !matches!(k.as_str(), "ia" | "mz" | "component")).cloned().collect());
        part.set("pt", Py::Int("1".into()));
        let pd = Py::Dict(p);
        let cn = if pd.at("component").truthy() { pd.at("component") } else { pd.at("name") };
        part.set("cn", cn.clone());
        out.push(feature(f.at("geometry").clone(), part));
    }
    eprintln!(
        "layers: heritage: {n_lead} World Heritage Sites in several components as one dot, {} components drawn apart",
        role.len()
    );
    Ok((out, record))
}

fn feature(geometry: Py, properties: Py) -> Py {
    Py::Dict(vec![
        ("type".into(), Py::Str("Feature".into())),
        ("geometry".into(), geometry),
        ("properties".into(), properties),
    ])
}

/// layer-heritage.json and props-heritage.jsonl; returns each World Heritage id's record.
fn heritage(b: &Path, items: &Path) -> Result<HashMap<String, Py>> {
    let mut fc = read(b, "heritage.json")?;
    let p = phase("the points ordered and repeated names spaced", Class::Compute);
    let feats = std::mem::take(features(&mut fc)?);
    let (mut feats, record) = merged_sites(b, feats)?;
    let fame = |f: &Py| {
        let p = props(f);
        match val(p, "fa").and_then(Py::number) {
            Some(fa) => fa,
            None => 5.0 - Some(p.at("level")).filter(|l| l.truthy()).and_then(Py::number).unwrap_or(5.0),
        }
    };
    feats.sort_by(|a, b| fame(a).partial_cmp(&fame(b)).unwrap_or(std::cmp::Ordering::Equal));
    let waits = space_names(&mut feats)?;
    drop(p);
    let p = phase("the points' properties made", Class::Compute);
    let wiki = wiki_titles(items)?;
    let mut rest_of: Vec<(Py, Vec<(String, Py)>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut n_en = 0;
    for f in feats.iter_mut() {
        let p = props_mut(f)?;
        let name = p.at("name").as_str().map(str::to_string);
        let wiki_title = match p.at("i") {
            Py::Int(i) => wiki.get(i),
            _ => None,
        };
        let own = site_english(name.as_deref().unwrap_or(""), p.at("name_en").as_str(), wiki_title.map(String::as_str));
        if let Some(en) = english_at(name.as_deref(), own.as_deref()) {
            p.set("en", Py::Str(en));
            n_en += 1;
        }
        let d = p.dict_mut().context("properties not a dict")?;
        let rest: Vec<(String, Py)> = d.iter().filter(|(k, _)| !KEEP.contains(&k.as_str())).cloned().collect();
        if let Some((_, i)) = d.iter().find(|(k, _)| k == "i").filter(|_| !rest.is_empty()) {
            if seen.insert(i.key()) {
                rest_of.push((i.clone(), rest));
            }
        }
        d.retain(|(k, _)| KEEP.contains(&k.as_str()));
        round_geometry(f)?;
    }
    drop(p);
    let n = feats.len();
    *features(&mut fc)? = feats;
    let _p = phase("the layers written", Class::Disk);
    write(b, "layer-heritage.json", &fc)?;
    rest_of.sort_by(|a, b| a.0.number().partial_cmp(&b.0.number()).unwrap_or(std::cmp::Ordering::Equal));
    let mut text = String::new();
    for (i, rest) in rest_of {
        let mut r = vec![("i".into(), i)];
        r.extend(rest);
        text.push_str(&dumps(&Py::Dict(r)));
        text.push('\n');
    }
    write_text(b, "props-heritage.jsonl", &text)?;
    eprintln!(
        "layers: heritage: {n} points ({n_en} with English; {waits} names wait for a better-known one of the same name)"
    );
    Ok(record)
}

/// Repeated names wait (crate::interest::same_names, the sites all one kind); how many wait.
fn space_names(feats: &mut [Py]) -> Result<usize> {
    let n = feats.len();
    let (mut lon, mut lat, mut fa, mut ia, mut mz) =
        (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![f64::NAN; n]);
    let mut names = Vec::with_capacity(n);
    for (i, f) in feats.iter().enumerate() {
        let p = props(f);
        let num = |k: &str| Some(p.at(k)).filter(|v| v.truthy()).and_then(Py::number).unwrap_or(0.0);
        fa[i] = num("fa");
        ia[i] = num("ia");
        if let Some(m) = val(p, "mz") {
            mz[i] = m.number().context("mz not a number")?;
        }
        names.push(p.at("name").as_str().unwrap_or("").to_string());
        // (Only a grouped point's position is read: a named one with mz.)
        if !names[i].is_empty() && !mz[i].is_nan() {
            [lon[i], lat[i]] = position(f)?;
        }
    }
    let before = mz.clone();
    let order: Vec<usize> = (0..n).collect();
    let waits = same_names(&order, &|i| ("heritage".into(), names[i].clone()), &lon, &lat, &fa, &ia, &mut mz);
    for (i, f) in feats.iter_mut().enumerate() {
        if mz[i] != before[i] && !mz[i].is_nan() {
            props_mut(f)?.set("mz", Py::Float(mz[i]));
        }
    }
    Ok(waits)
}

/// Heritage record (its `i`'s digits) → its English Wikipedia article's title (the last line naming it).
fn wiki_titles(items: &Path) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    if !items.exists() {
        return Ok(out);
    }
    let text = std::fs::read_to_string(items).with_context(|| format!("read {}", items.display()))?;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let it = loads(line)?;
        if let (Py::Str(t), Py::Int(i)) = (it.at("wiki").at("en"), it.at("i")) {
            if !t.is_empty() {
                out.insert(i.clone(), t.clone());
            }
        }
    }
    Ok(out)
}

/// layer-whs-shapes.json: the World Heritage outlines with i, their site's record (its dot's), so
/// an outline opens the site like its dot.
fn whs_outlines(b: &Path, record: &HashMap<String, Py>) -> Result<()> {
    let mut fc = read(b, "whs-shapes.json")?;
    let p = phase("the points' properties made", Class::Compute);
    let feats = features(&mut fc)?;
    let n = feats.len();
    for f in feats.iter_mut() {
        let p = props_mut(f)?;
        let id = p.get("id").context("an outline without its site's id")?.py_str();
        if let Some(i) = record.get(&id) {
            p.set("i", i.clone());
        }
    }
    drop(p);
    let _p = phase("the layers written", Class::Disk);
    write(b, "layer-whs-shapes.json", &fc)?;
    eprintln!("layers: whs-shapes: {n} outlines");
    Ok(())
}

/// layer-<src>.json: the areas with their own English, simplified within `tol` when it isn't 0;
/// returns their summary (`n`, and `a`, each one's area).
fn polygons(b: &Path, src: &str, tol: f64) -> Result<Py> {
    let mut fc = read(b, &format!("{src}.json"))?;
    let p = phase("the polygons simplified", Class::Compute);
    let feats = features(&mut fc)?;
    for f in feats.iter_mut() {
        let p = props_mut(f)?;
        if let Some(en) = english_at(p.at("name").as_str(), p.at("name_en").as_str()) {
            p.set("en", Py::Str(en));
        }
        let g = f.get("geometry").context("an area without geometry")?;
        let mut kind = g.at("type").clone();
        let mut c = g.at("coordinates").clone();
        if tol != 0.0 {
            if let Some((k, simplified)) = crate::tpsimplify::simplify_geojson(&kind, &c, tol)? {
                kind = Py::Str(k.into());
                c = simplified;
            }
        }
        f.set("geometry", Py::Dict(vec![("type".into(), kind), ("coordinates".into(), rounded(&c))]));
    }
    drop(p);
    let n = feats.len();
    let areas: Vec<Py> = feats.iter().map(|f| props(f).at("a").clone()).collect();
    let _p = phase("the layers written", Class::Disk);
    write(b, &format!("layer-{src}.json"), &fc)?;
    eprintln!("layers: {src}: {n} areas");
    Ok(Py::Dict(vec![("n".into(), Py::Int(n.to_string())), ("a".into(), Py::List(areas))]))
}

/// A name's English for the layers' `en`: its own, when it truly differs; else none (the map's
/// server gives it its translation where it is).
fn english_at(name: Option<&str>, own: Option<&str>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty())?;
    differs(name, own).then(|| own.unwrap().trim().to_string())
}

/// Python's `str.isalpha` (the L categories).
fn alpha(c: char) -> bool {
    c.is_alphabetic() && !is_combining_mark(c) && !c.is_numeric() && !circled(c)
}

/// Letters whose Unicode name says LATIN (Python's unicodedata, Unicode 16).
const LATIN: [(u32, u32); 34] = [
    (0x41, 0x5A),
    (0x61, 0x7A),
    (0xC0, 0xD6),
    (0xD8, 0xF6),
    (0xF8, 0x2AF),
    (0x1D00, 0x1D25),
    (0x1D62, 0x1D65),
    (0x1D6B, 0x1D77),
    (0x1D79, 0x1D9A),
    (0x1E00, 0x1EFF),
    (0x2071, 0x2071),
    (0x207F, 0x207F),
    (0x2090, 0x209C),
    (0x2184, 0x2184),
    (0x2C2E, 0x2C2E),
    (0x2C5E, 0x2C5E),
    (0x2C60, 0x2C7C),
    (0x2C7E, 0x2C7F),
    (0xA722, 0xA76F),
    (0xA771, 0xA787),
    (0xA78B, 0xA7CD),
    (0xA7D0, 0xA7D1),
    (0xA7D3, 0xA7D3),
    (0xA7D5, 0xA7DC),
    (0xA7F5, 0xA7F7),
    (0xA7FA, 0xA7FF),
    (0xAB30, 0xAB5A),
    (0xAB60, 0xAB64),
    (0xAB66, 0xAB68),
    (0xFB00, 0xFB06),
    (0xFF21, 0xFF3A),
    (0xFF41, 0xFF5A),
    (0x1DF00, 0x1DF1E),
    (0x1DF25, 0x1DF2A),
];

/// Whether at least as many of a name's letters are Latin as not.
fn latin(s: &str) -> bool {
    let (mut lat, mut other) = (0, 0);
    for c in s.chars().filter(|c| alpha(*c)) {
        if LATIN.iter().any(|(a, b)| (*a..=*b).contains(&(c as u32))) {
            lat += 1;
        } else {
            other += 1;
        }
    }
    lat >= other
}

/// An English word for what a site is: a Wikipedia title with one is English (not the site's other
/// name: "Hôtel du Tillet de la Bussière").
const ENGLISH: &str = "abbey castle church cathedral chapel house houses palace bridge tower towers museum priory manor fort fortress hall
garden gardens park lake river mount mountain island islands cave caves monastery convent basilica mill lighthouse
station square street market theatre theater hospital school college university library gate walls wall ruins site
temple shrine tomb tombs mound battlefield aqueduct viaduct canal harbour port citadel villa estate farm barracks
bastion arch column monument memorial statue fountain cross well baths amphitheatre hotel courthouse prison mine
quarry dam reservoir falls waterfall valley gorge beach beaches bay cape forest peak hill hills range district
building buildings cemetery pavilion residence mansion cottage lodge inn pier wharf dock dockyard railway line road
trail avenue centre center observatory stadium rock stone stones circle henge barrow hillfort camp keep gallery
town city village old";

/// Whether a title has one of `ENGLISH`'s words as a whole word (in any case).
fn has_english_word(s: &str) -> bool {
    s.split(|c: char| !word_char(c)).any(|w| {
        let w: String = w.chars().map(|c| if c == 'ſ' { 's' } else { c }).collect::<String>().to_lowercase();
        !w.is_empty() && ENGLISH.split_whitespace().any(|e| e == w)
    })
}

/// A heritage site's own English: UNESCO's (`own`), else its English Wikipedia article's title when
/// that's English (or the name isn't in Latin script), without a trailing "(…)".
fn site_english(name: &str, own: Option<&str>, wiki: Option<&str>) -> Option<String> {
    if differs(name, own) {
        return Some(own.unwrap().trim().to_string());
    }
    let wiki = wiki.filter(|w| !w.is_empty())?;
    let wiki = strip_paren(wiki).trim().to_string();
    (differs(name, Some(&wiki)) && (!latin(name) || has_english_word(&wiki))).then_some(wiki)
}

/// `re.sub(r"\s*\([^)]*\)$", "", s)`: a last "(…)" (with no ")" inside) and the spaces before it.
fn strip_paren(s: &str) -> &str {
    let body = s.strip_suffix('\n').unwrap_or(s);
    let Some(inner) = body.strip_suffix(')') else { return s };
    // The leftmost "(" from which only non-")" run to the end.
    let after_close = inner.rfind(')').map_or(0, |k| k + 1);
    let Some(open) = inner[after_close..].find('(').map(|k| k + after_close) else { return s };
    let start = inner[..open].trim_end_matches(char::is_whitespace).len();
    &s[..start]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small chain's files, and what dem/layers.py wrote from them (its last version).
    const INPUTS: [(&str, &str); 6] = [
        (
            r#"heritage.json"#,
            r#"{"type":"FeatureCollection","features":[
{"type":"Feature","geometry":{"type":"Point","coordinates":[2.1234567,48.7654321]},"properties":{"name":"Château de Chambord","level":3,"i":0,"fa":1.5,"ia":2.0,"mz":6.5,"date":"1840","url":"u"}},
{"type":"Feature","geometry":{"type":"Point","coordinates":[2.13,48.77]},"properties":{"name":"Lookout","level":3,"i":1,"fa":0.5,"ia":0.1,"mz":9.25}},
{"type":"Feature","geometry":{"type":"Point","coordinates":[2.131,48.771]},"properties":{"name":"lookout ","level":3,"i":2,"fa":0.7,"ia":0.3,"mz":8.0}},
{"type":"Feature","geometry":{"type":"Point","coordinates":[10.5,45.25]},"properties":{"name":"Site A","name_en":"Old Town A","level":1,"site":"77","component":"North","i":3,"fa":3.0,"ia":20000.0,"mz":0.0,"src":"x"}},
{"type":"Feature","geometry":{"type":"Point","coordinates":[10.6,45.3]},"properties":{"name":"Site A","name_en":"Old Town A","level":1,"site":"77","component":"","i":4,"fa":2.0,"ia":5.0,"mz":4.0,"src":"y"}},
{"type":"Feature","geometry":{"type":"Point","coordinates":[-3,51]},"properties":{"name":"Tōdai-ji","level":2,"i":5,"fa":2.5,"ia":1e-05,"mz":12}}
]}"#,
        ),
        (r#"whs-sites.json"#, r#"{"77":{"q":["Q1"],"n":2,"dot":[10.55,45.275],"lead":[10.5,45.25]},"78":{"q":[]}}"#),
        (
            r#"whs-shapes.json"#,
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"LineString","coordinates":[[10.5,45.25],[10.6,45.3]]},"properties":{"id":77,"n":1,"c":0,"a":1.25}},{"type":"Feature","geometry":{"type":"LineString","coordinates":[[1,2],[3,4]]},"properties":{"id":99}}]}"#,
        ),
        (
            r#"heritage-areas.json"#,
            r#"{"type":"FeatureCollection","features":[
{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0,0],[0.5,0.000001],[1,0],[1,1],[0.5,1.000004],[0,1],[0,0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.5,0.600001],[0.4,0.6],[0.4,0.4]]]},"properties":{"name":"Parc","name_en":"Park","a":12.5}},
{"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[5,5],[5.000003,5.5],[5,6],[6,6],[6,5],[5,5]]]]},"properties":{"name":"Zone","a":null}},
{"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[7,7],[8,7],[8,8],[7,7]]],[[[9,9],[10,9],[10,10],[9.5,10.000002],[9,9]]]]},"properties":{}}
]}"#,
        ),
        (
            r#"indigenous.json"#,
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[1.12345678,2.1],[3,2],[3,4],[1.12345678,2.1]]]},"properties":{"name":"Nation","a":3}}]}"#,
        ),
        (r#"special.json"#, r#"{"type":"FeatureCollection","features":[]}"#),
    ];
    const ITEMS: &str = r#"{"i": 0, "wiki": {"en": "Château de Chambord (castle)"}}
{"i": 5, "wiki": {"en": "Tōdai-ji"}}
"#;
    const OUTPUTS: [(&str, &str); 6] = [
        (
            r#"layer-heritage.json"#,
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Point","coordinates":[2.13,48.77]},"properties":{"name":"Lookout","level":3,"i":1,"fa":0.5,"ia":0.1,"mz":11.62}},{"type":"Feature","geometry":{"type":"Point","coordinates":[2.131,48.771]},"properties":{"name":"lookout ","level":3,"i":2,"fa":0.7,"ia":0.3,"mz":8.0}},{"type":"Feature","geometry":{"type":"Point","coordinates":[2.123457,48.765432]},"properties":{"name":"Château de Chambord","level":3,"i":0,"fa":1.5,"ia":2.0,"mz":6.5}},{"type":"Feature","geometry":{"type":"Point","coordinates":[10.6,45.3]},"properties":{"name":"Site A","level":1,"i":4,"fa":2.0,"pt":1,"cn":"Site A","en":"Old Town A"}},{"type":"Feature","geometry":{"type":"Point","coordinates":[-3,51]},"properties":{"name":"Tōdai-ji","level":2,"i":5,"fa":2.5,"ia":1e-05,"mz":12}},{"type":"Feature","geometry":{"type":"Point","coordinates":[10.55,45.275]},"properties":{"name":"Site A","level":1,"i":3,"fa":3.0,"ia":20000.0,"mz":0.0,"np":2,"en":"Old Town A"}},{"type":"Feature","geometry":{"type":"Point","coordinates":[10.5,45.25]},"properties":{"name":"Site A","level":1,"i":3,"fa":3.0,"pt":1,"cn":"North","en":"Old Town A"}}]}"#,
        ),
        (
            r#"props-heritage.jsonl"#,
            r#"{"i":0,"date":"1840","url":"u"}
{"i":3,"name_en":"Old Town A","site":"77","src":"x"}
{"i":4,"name_en":"Old Town A","site":"77","src":"y"}
"#,
        ),
        (
            r#"layer-whs-shapes.json"#,
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"LineString","coordinates":[[10.5,45.25],[10.6,45.3]]},"properties":{"id":77,"n":1,"c":0,"a":1.25,"i":3}},{"type":"Feature","geometry":{"type":"LineString","coordinates":[[1,2],[3,4]]},"properties":{"id":99}}]}"#,
        ),
        (
            r#"layer-heritage-areas.json"#,
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0],[0.0,0.0]],[[0.4,0.4],[0.6,0.4],[0.6,0.6],[0.4,0.6],[0.4,0.4]]]},"properties":{"name":"Parc","name_en":"Park","a":12.5,"en":"Park"}},{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[5.0,5.0],[5.0,6.0],[6.0,6.0],[6.0,5.0],[5.0,5.0]]]},"properties":{"name":"Zone","a":null}},{"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[7.0,7.0],[8.0,7.0],[8.0,8.0],[7.0,7.0]]],[[[9.0,9.0],[10.0,9.0],[10.0,10.0],[9.5,10.000002],[9.0,9.0]]]]},"properties":{}}]}"#,
        ),
        (
            r#"layer-indigenous.json"#,
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Polygon","coordinates":[[[1.123457,2.1],[3,2],[3,4],[1.123457,2.1]]]},"properties":{"name":"Nation","a":3}}]}"#,
        ),
        (
            r#"layer-summary.json"#,
            r#"{"heritageAreas":{"n":3,"a":[12.5,null,null]},"indigenous":{"n":1,"a":[3]},"special":{"n":0,"a":[]}}"#,
        ),
    ];

    #[test]
    fn writes_what_the_python_wrote() {
        let d = tempfile::tempdir().unwrap();
        for (name, text) in INPUTS {
            std::fs::write(d.path().join(name), text).unwrap();
        }
        let items = d.path().join("items.jsonl");
        std::fs::write(&items, ITEMS).unwrap();
        write_all(d.path(), &items).unwrap();
        for (name, want) in OUTPUTS {
            assert_eq!(std::fs::read_to_string(d.path().join(name)).unwrap(), want, "{name}");
        }
        assert!(!d.path().join("layer-special.json.tmp").exists());
    }

    #[test]
    fn english_as_names_py() {
        assert!(!differs("Montréal", Some("Montreal")));
        assert!(!differs("Mont-Blanc", Some("Mont Blanc")));
        assert!(differs("Köln", Some("Cologne")));
        assert_eq!(english_at(Some("Köln"), Some(" Cologne ")), Some("Cologne".into()));
        assert_eq!(english_at(None, Some("Cologne")), None);
        assert_eq!(
            site_english("Hôtel du Tillet de la Bussière", None, Some("Hôtel du Tillet (Paris)")),
            None,
            "a Latin name's title without an English word"
        );
        assert_eq!(
            site_english("Château de Chambord", None, Some("Chambord Castle (France)")),
            Some("Chambord Castle".into())
        );
        assert_eq!(site_english("東大寺", None, Some("Tōdai-ji")), Some("Tōdai-ji".into()));
        assert_eq!(site_english("X", Some("Y"), Some("Z castle")), Some("Y".into()));
    }

    #[test]
    fn strips_a_last_parenthesis() {
        assert_eq!(strip_paren("Fort (a) (b)"), "Fort (a)");
        assert_eq!(strip_paren("Fort  (b)"), "Fort");
        assert_eq!(strip_paren("Fort (a)b"), "Fort (a)b");
        assert_eq!(strip_paren("(b)"), "");
        assert_eq!(strip_paren("Fort (a (b)"), "Fort");
    }

    #[test]
    fn latin_counts_letters() {
        assert!(latin("Abbey 12"));
        assert!(!latin("Москва"));
        assert!(latin("Aб"));
        assert!(!latin("Ab Где"));
    }
}
