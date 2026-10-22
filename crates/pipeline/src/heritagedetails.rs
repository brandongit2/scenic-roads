//! Heritage site details, in the heritage chain's stand-in root (after crate::heritagewd): the short
//! description and Wikipedia facts from crate::heritagewd's answers, and the long (paraphrased)
//! descriptions.
//!
//!   data/build/details-heritage.jsonl  per site (by index in heritage.json, which gains `i`):
//!       Wikidata item, short description (English Wikipedia's, else Wikidata's in English, else
//!       in the local language), type, style, architect, inception, the Wikipedia article
//!       (English, else local) and, where written, a long description with its source article.
//!
//! Long descriptions: the ones written in the registers' snapshot (`data/heritage/desc/*.out.jsonl`).
//! Those written since are the NAS's descriptions/, which the server lays over the records
//! (crates/server/src/descriptions.rs). The files' bytes are as Python's `json` writes them
//! (crate::pyjson).

use crate::pyjson::{dumps, dumps_spaced, loads, Py};
use crate::timings::{phase, Class};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

const LOCAL: [&str; 4] = ["fr", "es", "pt", "ca"];

fn lines(p: &Path) -> Result<Vec<String>> {
    let s = std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
    Ok(s.lines().map(str::to_string).collect())
}

/// Per site index (its key), its Wikidata item: of several (sharing its register ID, or found
/// through OSM), one with a Wikipedia article, then the most-linked.
fn best_items(w: &Path) -> Result<HashMap<String, Py>> {
    let rank = |r: &Py| (r.at("wiki").truthy(), r.at("sl").number().unwrap_or(f64::NEG_INFINITY));
    let mut out: HashMap<String, Py> = HashMap::new();
    for line in lines(&w.join("items.jsonl"))? {
        let r = loads(&line).context("a line of items.jsonl")?;
        let k = r.get("i").context("an item without `i`")?.key();
        if out.get(&k).is_none_or(|cur| rank(&r) > rank(cur)) {
            out.insert(k, r);
        }
    }
    Ok(out)
}

/// The item's Wikipedia article: English, else the local languages', else its first.
fn article(r: &Py) -> Option<(String, Py)> {
    let Py::Dict(w) = r.at("wiki") else { return None };
    let find = |l: &str| w.iter().find(|(k, _)| k == l).map(|(k, v)| (k.clone(), v.clone()));
    find("en").or_else(|| LOCAL.iter().find_map(|l| find(l))).or_else(|| w.first().cloned())
}

/// Written descriptions by Wikidata item: the pilot's (batch-*, rewritten fix-*), overridden by
/// those in the current voice (v2-*), and those researched from other sources where the article
/// didn't do (research-*, with their sources as "src": [{"t": title, "u": url}]; "drop" where
/// nothing reliable was found, which removes an earlier one).
fn long_descriptions(d: &Path) -> Result<HashMap<String, Py>> {
    let mut groups: Vec<Vec<std::path::PathBuf>> = vec![vec![]; 4];
    if d.is_dir() {
        for e in std::fs::read_dir(d)?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = n.strip_suffix(".out.jsonl") else { continue };
            let g = ["batch-", "fix-", "v2-", "research-"].iter().position(|p| stem.starts_with(p));
            if let Some(g) = g {
                groups[g].push(e.path());
            }
        }
    }
    let mut out = HashMap::new();
    for mut g in groups {
        // (Python's sorted(glob): by the path's code points, as Rust's byte order.)
        g.sort();
        for p in g {
            for line in lines(&p)? {
                let Ok(r) = loads(&line) else { continue };
                if r.at("qid").truthy() && r.at("drop").truthy() {
                    out.remove(&r.at("qid").key());
                } else if r.at("qid").truthy() && r.at("long").truthy() {
                    out.insert(r.at("qid").key(), r);
                }
            }
        }
    }
    Ok(out)
}

fn dict(kv: Vec<(&str, Py)>) -> Py {
    Py::Dict(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// `d.get(k, default)`: the key's value when there (None included).
fn get_or(d: &Py, k: &str, default: Py) -> Py {
    d.get(k).cloned().unwrap_or(default)
}

/// Writes details-heritage.jsonl in `root`'s data/build and stamps each site's index `i` onto
/// heritage.json.
pub fn run(root: &Path) -> Result<()> {
    let b = root.join("data/build");
    let w = root.join("data/heritage/wd");
    let d = root.join("data/heritage/desc");
    let p = phase("the items, descriptions and sites read", Class::Disk);
    let items = best_items(&w)?;
    let sdp = w.join("enwiki-shortdesc.json");
    let sd = if sdp.exists() {
        loads(&std::fs::read_to_string(&sdp)?).context("parse enwiki-shortdesc.json")?
    } else {
        Py::Dict(vec![])
    };
    let long = long_descriptions(&d)?;
    let mut ex = HashMap::new();
    if d.join("extracts.jsonl").exists() {
        for line in lines(&d.join("extracts.jsonl"))? {
            let r = loads(&line).context("a line of extracts.jsonl")?;
            ex.insert(r.at("qid").key(), r);
        }
    }
    let mut fc = loads(&std::fs::read_to_string(b.join("heritage.json"))?).context("parse heritage.json")?;
    drop(p);
    let p = phase("the site details made and written", Class::Compute);
    let empty = Py::Dict(vec![]);
    let (mut n, mut nl) = (0, 0);
    let mut out = String::new();
    let Py::Dict(top) = &mut fc else { anyhow::bail!("heritage.json: not a FeatureCollection") };
    let Some((_, Py::List(features))) = top.iter_mut().find(|(k, _)| k == "features") else {
        anyhow::bail!("heritage.json: no features")
    };
    let total = features.len();
    for (i, f) in features.iter_mut().enumerate() {
        let Py::Dict(fd) = f else { anyhow::bail!("heritage.json: a feature not an object") };
        let props = &mut fd.iter_mut().find(|(k, _)| k == "properties").context("a site without properties")?.1;
        props.set("i", Py::Int(i.to_string()));
        let Some(r) = items.get(&Py::Int(i.to_string()).key()) else { continue };
        let art = article(r);
        let wiki = r.at("wiki");
        let desc = r.at("desc");
        let mut short = match wiki.get("en") {
            Some(en) => sd.at(&en.py_str()).clone(),
            None => Py::Null,
        };
        if !short.truthy() {
            short = desc.at("en").clone();
        }
        if !short.truthy() {
            short = LOCAL.iter().find_map(|l| desc.get(l)).cloned().unwrap_or(Py::Null);
        }
        let mut rec =
            dict(vec![("i", Py::Int(i.to_string())), ("qid", r.at("qid").clone()), ("sl", r.at("sl").clone())]);
        if short.truthy() {
            rec.set("short", short);
        }
        for k in ["inception", "inst", "style", "arch"] {
            if let Some(v) = r.get(k) {
                rec.set(k, v.clone());
            }
        }
        if let Some((lang, title)) = &art {
            rec.set("wiki", dict(vec![("lang", Py::Str(lang.clone())), ("title", title.clone())]));
        }
        if let Some(lg) = long.get(&r.at("qid").key()) {
            let src = ex.get(&r.at("qid").key()).unwrap_or(&empty);
            rec.set("long", lg.at("long").clone());
            let long_src = if lg.at("src").truthy() {
                dict(vec![("refs", lg.at("src").clone())])
            } else {
                let (al, at) = match &art {
                    Some((l, t)) => (Py::Str(l.clone()), t.clone()),
                    None => (Py::Str("en".into()), Py::Str(String::new())),
                };
                dict(vec![("lang", get_or(src, "lang", al)), ("title", get_or(src, "title", at))])
            };
            rec.set("long_src", long_src);
            nl += 1;
        }
        out.push_str(&dumps_spaced(&rec));
        out.push('\n');
        n += 1;
    }
    std::fs::write(b.join("details-heritage.jsonl"), out)?;
    drop(p);
    let _p = phase("the sites written with their indexes", Class::Disk);
    let tmp = b.join("heritage.json.tmp");
    std::fs::write(&tmp, dumps(&fc))?;
    std::fs::rename(&tmp, b.join("heritage.json"))?;
    eprintln!("details-heritage.jsonl: {n} of {total} sites with Wikidata, {nl} with a long description");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_order() {
        let r = loads(r#"{"wiki":{"de":"D","pt":"P"}}"#).unwrap();
        assert_eq!(article(&r), Some(("pt".into(), Py::Str("P".into()))));
        let r = loads(r#"{"wiki":{"de":"D"}}"#).unwrap();
        assert_eq!(article(&r), Some(("de".into(), Py::Str("D".into()))));
        assert_eq!(article(&loads(r#"{"wiki":{}}"#).unwrap()), None);
    }

    #[test]
    fn details_as_python() {
        let t = std::env::temp_dir().join(format!("heritagedetails-{}", std::process::id()));
        for d in ["data/build", "data/heritage/wd", "data/heritage/desc"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        std::fs::write(
            t.join("data/build/heritage.json"),
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"n":"A"}},{"type":"Feature","properties":{}}]}"#,
        )
        .unwrap();
        std::fs::write(
            t.join("data/heritage/wd/items.jsonl"),
            "{\"i\":0,\"qid\":\"Q1\",\"sl\":3,\"wiki\":{},\"desc\":{\"fr\":\"église\"}}\n{\"i\":0,\"qid\":\"Q2\",\"sl\":1,\"wiki\":{\"en\":\"A\"},\"desc\":{},\"inst\":[\"x\"]}\n",
        )
        .unwrap();
        std::fs::write(t.join("data/heritage/wd/enwiki-shortdesc.json"), r#"{"A":"short A"}"#).unwrap();
        std::fs::write(t.join("data/heritage/desc/v2-1.out.jsonl"), "{\"qid\":\"Q2\",\"long\":\"L\"}\nnot json\n")
            .unwrap();
        run(&t).unwrap();
        assert_eq!(
            std::fs::read_to_string(t.join("data/build/details-heritage.jsonl")).unwrap(),
            "{\"i\": 0, \"qid\": \"Q2\", \"sl\": 1, \"short\": \"short A\", \"inst\": [\"x\"], \"wiki\": {\"lang\": \"en\", \"title\": \"A\"}, \"long\": \"L\", \"long_src\": {\"lang\": \"en\", \"title\": \"A\"}}\n"
        );
        assert!(std::fs::read_to_string(t.join("data/build/heritage.json")).unwrap().contains(r#"{"n":"A","i":0}"#));
        std::fs::remove_dir_all(&t).ok();
    }

    /// A researched description's sources (`src`) become `long_src: {"refs": …}`; a later `drop`
    /// removes an earlier description; an extract names the article a description came from.
    #[test]
    fn researched_sources_and_drops() {
        let t = std::env::temp_dir().join(format!("heritagedetails-refs-{}", std::process::id()));
        for d in ["data/build", "data/heritage/wd", "data/heritage/desc"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        let w = |p: &str, s: &str| std::fs::write(t.join(p), s).unwrap();
        w("data/build/heritage.json", r#"{"features":[{"properties":{}},{"properties":{}},{"properties":{}}]}"#);
        w(
            "data/heritage/wd/items.jsonl",
            "{\"i\":0,\"qid\":\"Q1\",\"sl\":1,\"wiki\":{},\"desc\":{}}\n\
             {\"i\":1,\"qid\":\"Q2\",\"sl\":1,\"wiki\":{},\"desc\":{}}\n\
             {\"i\":2,\"qid\":\"Q3\",\"sl\":1,\"wiki\":{\"fr\":\"T\"},\"desc\":{}}\n",
        );
        // (batch- before v2- before research-: research's drop removes Q2's.)
        w(
            "data/heritage/desc/batch-00.out.jsonl",
            "{\"qid\":\"Q2\",\"long\":\"old\"}\n{\"qid\":\"Q3\",\"long\":\"L3\"}\n",
        );
        w(
            "data/heritage/desc/research-00.out.jsonl",
            "{\"qid\":\"Q1\",\"long\":\"L1\",\"src\":[{\"t\":\"Site\",\"u\":\"https://e.org\"}]}\n{\"qid\":\"Q2\",\"drop\":true}\n",
        );
        w("data/heritage/desc/extracts.jsonl", "{\"qid\":\"Q3\",\"lang\":\"en\",\"title\":\"E\"}\n");
        run(&t).unwrap();
        assert_eq!(
            std::fs::read_to_string(t.join("data/build/details-heritage.jsonl")).unwrap(),
            "{\"i\": 0, \"qid\": \"Q1\", \"sl\": 1, \"long\": \"L1\", \"long_src\": {\"refs\": [{\"t\": \"Site\", \"u\": \"https://e.org\"}]}}\n\
             {\"i\": 1, \"qid\": \"Q2\", \"sl\": 1}\n\
             {\"i\": 2, \"qid\": \"Q3\", \"sl\": 1, \"wiki\": {\"lang\": \"fr\", \"title\": \"T\"}, \"long\": \"L3\", \"long_src\": {\"lang\": \"en\", \"title\": \"E\"}}\n"
        );
        std::fs::remove_dir_all(&t).ok();
    }
}
