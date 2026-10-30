//! Facts and Wikipedia articles for the landmark candidates' Wikidata items, per pass epoch (the
//! `items` job, docs/phase5.md "items"; their pageviews are the job's next part, crate::pageviews).
//!
//! The items: "facts" those of tags that are one QID (a multi-QID tag gets no facts), "views"
//! every candidate's first QID; scenic-build's `items` step gathers them.
//!
//! Per epoch (the pass's date) everything is asked again; within one, only items not seen yet (a
//! run started by new coverage doesn't move anyone else's fame). Caches in the job's cache folder,
//! appended a chunk at a time (a run stopped midway keeps what it fetched):
//!   facts-<epoch>.jsonl      QID → its facts (`facts_of`: sitelinks, descriptions, heights …)
//!   wp-<epoch>.jsonl         QID → its Wikipedia articles (crate::heritagewd::wikipedias)
//!   fetched-<epoch>.json     the first and last days anything was fetched for the epoch
//! They are kept on the NAS too (sources/items/<epoch>/answers.tar.zst, with kept-<epoch>.json
//! here saying which of its archives they last matched: crate::answers), so a run on another Mac,
//! or here after the cache is lost, asks only about items not seen yet. To ask Wikidata again,
//! delete facts-<epoch>.jsonl (or wp-) here: the deletion goes to the NAS with the next run, which
//! asks about every item again. Older epochs' files go at the end of a run.
//!
//! Output: `<out>/facts.json` (QID → record, the items Wikidata knows). Every failed query fails
//! the run (no silently skipped batch). The files' bytes are as Python's `json` writes them
//! (crate::pyjson).

use crate::agent::jobs::report;
use crate::heritagewd::{wikipedias_told, write_atomic};
use crate::interest::py_round;
use crate::pyjson::{dumps_spaced, loads, Py};
use crate::timings::{phase, Class};
use crate::wikiask::{val, Asker};
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;

/// Items fetched between cache writes.
const CHUNK: usize = 5000;
/// Items per Wikidata query.
const WD_BATCH: usize = 250;

/// The numeric facts: their names and properties.
const Q_PROPS: [(&str, &str); 6] = [
    ("height", "P2048"),
    ("elevation", "P2044"),
    ("prominence", "P2660"),
    ("isolation", "P2659"),
    ("discharge", "P2225"),
    ("focal", "P2923"),
];

/// The facts query for `chunk`.
fn facts_query(chunk: &[String]) -> String {
    let values = chunk.iter().map(|q| format!("wd:{q}")).collect::<Vec<_>>().join(" ");
    let num = Q_PROPS
        .iter()
        .map(|(n, p)| {
            format!("OPTIONAL {{ ?item p:{p} ?st_{n} . ?st_{n} psn:{p} ?nv_{n} . ?nv_{n} wikibase:quantityAmount ?v_{n}0 }}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let maxes = Q_PROPS.iter().map(|(n, _)| format!("(MAX(?v_{n}0) AS ?v_{n})")).collect::<Vec<_>>().join(" ");
    format!(
        r#"PREFIX psn: <http://www.wikidata.org/prop/statement/value-normalized/> PREFIX p: <http://www.wikidata.org/prop/>
        SELECT ?item (SAMPLE(?sl0) AS ?sl) (SAMPLE(?den) AS ?d_en) (SAMPLE(?dloc) AS ?d_loc) (SAMPLE(?wen) AS ?w_en) (SAMPLE(?inc0) AS ?inc)
          {maxes}
          (GROUP_CONCAT(DISTINCT ?waterL; SEPARATOR="|") AS ?water) (GROUP_CONCAT(DISTINCT ?rangeL; SEPARATOR="|") AS ?range) WHERE {{
          VALUES ?item {{ {values} }}
          OPTIONAL {{ ?item wikibase:sitelinks ?sl0 }}
          OPTIONAL {{ ?item schema:description ?den FILTER(LANG(?den) = "en") }}
          OPTIONAL {{ ?item schema:description ?dloc FILTER(LANG(?dloc) IN ("fr", "es", "pt", "ca")) }}
          OPTIONAL {{ ?a schema:about ?item ; schema:isPartOf <https://en.wikipedia.org/> ; schema:name ?wen }}
          OPTIONAL {{ ?item wdt:P571 ?inc0 }}
          {num}
          OPTIONAL {{ ?item wdt:P206 ?w . ?w rdfs:label ?waterL FILTER(LANG(?waterL) = "en") }}
          OPTIONAL {{ ?item wdt:P4552 ?r . ?r rdfs:label ?rangeL FILTER(LANG(?rangeL) = "en") }}
        }} GROUP BY ?item"#
    )
}

/// One binding's record: sitelinks, short descriptions (English, else French, Spanish, Portuguese
/// or Catalan), English article, water body (P206), mountain range (P4552), inception, and the
/// numeric facts to 2 decimals.
fn record(b: &Py) -> Result<Py> {
    let sl = match val(b, "sl") {
        Some(s) => crate::pytext::strip(s).parse::<i128>().with_context(|| format!("sitelinks {s:?}"))?,
        None => 0,
    };
    let mut rec = vec![("sl".to_string(), Py::Int(sl.to_string()))];
    for k in ["d_en", "d_loc", "w_en", "water", "range"] {
        if let Some(v) = val(b, k) {
            rec.push((k.into(), Py::Str(v.into())));
        }
    }
    if let Some(v) = val(b, "inc") {
        rec.push(("inception".into(), Py::Str(v.chars().take(10).collect())));
    }
    for (n, _) in Q_PROPS {
        if let Some(v) = val(b, &format!("v_{n}")) {
            let x = crate::pytext::float(v).with_context(|| format!("{n} {v:?} not a number"))?;
            rec.push((n.into(), Py::Float(py_round(x, 2))));
        }
    }
    Ok(Py::Dict(rec))
}

/// Facts of these items from the Wikidata endpoint, WD_BATCH at a time (`progress` told how many
/// are done after each batch).
fn facts_of(
    ask: &Asker,
    endpoint: &str,
    qids: &[String],
    progress: &mut dyn FnMut(usize),
) -> Result<HashMap<String, Py>> {
    let mut out = HashMap::new();
    for (k, chunk) in qids.chunks(WD_BATCH).enumerate() {
        for b in ask.sparql(endpoint, &facts_query(chunk))? {
            let item = val(&b, "item").context("a binding without its item")?;
            let qid = item.rsplit_once('/').context("an item not a URL")?.1;
            out.insert(qid.to_string(), record(&b)?);
        }
        let done = (k * WD_BATCH + chunk.len()).min(qids.len());
        eprintln!("  wikidata {done}/{}", qids.len());
        progress(done);
    }
    Ok(out)
}

/// A JSON-lines cache's records by "qid", in the file's order (a repeated QID keeps its first
/// place and its last record); a last line cut short (a run stopped mid-write) is dropped from
/// the file.
fn read_jsonl(p: &Path) -> Result<Vec<(String, Py)>> {
    let b = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
    };
    let whole = b.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
    if whole != b.len() {
        std::fs::OpenOptions::new().write(true).open(p)?.set_len(whole as u64)?;
    }
    let text = std::str::from_utf8(&b[..whole]).with_context(|| format!("{} not UTF-8", p.display()))?;
    let mut out: Vec<(String, Py)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for line in text.lines() {
        let r = loads(line).with_context(|| format!("a line of {}", p.display()))?;
        let q = r.at("qid").py_str();
        match at.get(&q) {
            Some(&i) => out[i].1 = r,
            None => {
                at.insert(q.clone(), out.len());
                out.push((q, r));
            }
        }
    }
    Ok(out)
}

/// Appends `rows` to a JSON-lines cache, synced.
fn append_jsonl(p: &Path, rows: &[Py]) -> Result<()> {
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(p)?;
    let text: String = rows.iter().map(|r| dumps_spaced(r) + "\n").collect();
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    Ok(())
}

/// `{"qid": q, **r}`.
fn with_qid(q: &str, r: &Py) -> Py {
    let mut d = vec![("qid".to_string(), Py::Str(q.into()))];
    if let Py::Dict(e) = r {
        d.extend(e.iter().filter(|(k, _)| k != "qid").cloned());
    }
    Py::Dict(d)
}

/// A record without its "qid".
fn without_qid(r: &Py) -> Py {
    match r {
        Py::Dict(d) => Py::Dict(d.iter().filter(|(k, _)| k != "qid").cloned().collect()),
        other => other.clone(),
    }
}

/// Asks Wikidata (`wikidata`, the declared endpoint) for the facts of `facts_q` and the articles of
/// `views_q` it hasn't for epoch `epoch` in `cache`, and writes `<out>/facts.json`; `part(i)` marks
/// the job's part `i` beginning (1 the facts, 2 the articles).
pub fn run(
    wikidata: &str,
    facts_q: &BTreeSet<String>,
    views_q: &BTreeSet<String>,
    epoch: &str,
    cache: &Path,
    out: &Path,
    part: &dyn Fn(usize),
) -> Result<()> {
    std::fs::create_dir_all(cache)?;
    std::fs::create_dir_all(out)?;
    let ask = Asker::new();
    let p = phase("reading the caches", Class::Disk);
    let dpath = cache.join(format!("fetched-{epoch}.json"));
    let mut fetched = match std::fs::read_to_string(&dpath) {
        Ok(t) => loads(&t).context("parse fetched.json")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Py::Dict(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut note_fetch = || -> Result<()> {
        let today = crate::rail::day_of(std::time::SystemTime::now());
        if fetched.get("first").is_none() {
            fetched.set("first", Py::Str(today.clone()));
        }
        fetched.set("last", Py::Str(today));
        write_atomic(&dpath, &dumps_spaced(&fetched))
    };
    let fpath = cache.join(format!("facts-{epoch}.jsonl"));
    let mut facts: HashMap<String, Py> = read_jsonl(&fpath)?.into_iter().map(|(q, r)| (q, without_qid(&r))).collect();
    let todo: Vec<String> = facts_q.iter().filter(|q| !facts.contains_key(*q)).cloned().collect();
    drop(p);

    // Facts: this epoch's, only the new items fetched.
    part(1);
    eprintln!("facts: {} items, {} to fetch", facts_q.len(), todo.len());
    let what = "items' facts fetched from Wikidata";
    for (c, chunk) in todo.chunks(CHUNK).enumerate() {
        let k = c * CHUNK;
        // (The job's progress; within a chunk, said again after each of its batches.)
        report(k as u64, todo.len() as u64, what);
        let p = phase("fetching facts from Wikidata", Class::Net);
        let got = facts_of(&ask, wikidata, chunk, &mut |n| report((k + n) as u64, todo.len() as u64, what))?;
        drop(p);
        // Items Wikidata doesn't know (merged, deleted) are remembered as such, not asked again.
        let _p = phase("appending to the caches", Class::Disk);
        let missing = Py::Dict(vec![("sl".into(), Py::Int("0".into())), ("missing".into(), Py::Bool(true))]);
        let rows: Vec<Py> = chunk.iter().map(|q| with_qid(q, got.get(q).unwrap_or(&missing))).collect();
        append_jsonl(&fpath, &rows)?;
        for (q, r) in chunk.iter().zip(&rows) {
            facts.insert(q.clone(), without_qid(r));
        }
        note_fetch()?;
    }

    // Each item's Wikipedia articles: this epoch's.
    let wpath = cache.join(format!("wp-{epoch}.jsonl"));
    let p = phase("reading the caches", Class::Disk);
    let wp: BTreeSet<String> = read_jsonl(&wpath)?.into_iter().map(|(q, _)| q).collect();
    drop(p);
    let need: Vec<String> = views_q.iter().filter(|q| !wp.contains(*q)).cloned().collect();
    part(2);
    eprintln!("articles: {} items, {} to look up", views_q.len(), need.len());
    let what = "items' Wikipedia articles looked up";
    for (c, chunk) in need.chunks(CHUNK).enumerate() {
        let k = c * CHUNK;
        report(k as u64, need.len() as u64, what);
        let got = wikipedias_told(&ask, wikidata, chunk, &mut |n| report((k + n) as u64, need.len() as u64, what))?;
        let _p = phase("appending to the caches", Class::Disk);
        append_jsonl(&wpath, &got.iter().map(|(q, r)| with_qid(q, r)).collect::<Vec<_>>())?;
        note_fetch()?;
    }

    let p = phase("writing the outputs", Class::Disk);
    let known: Vec<(String, Py)> = facts_q
        .iter()
        .map(|q| (q.clone(), facts[q].clone()))
        .filter(|(_, r)| !r.get("missing").is_some_and(Py::truthy))
        .collect();
    let file = out.join("facts.json");
    write_atomic(&file, &dumps_spaced(&Py::Dict(known)))?;
    drop(p);
    // Older epochs' caches go.
    let _p = phase("deleting the old caches", Class::Disk);
    for e in std::fs::read_dir(cache)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let stem = name.split('.').next().unwrap_or("");
        let old = stem
            .split_once('-')
            .is_some_and(|(kind, ep)| ["facts", "wp", "fetched", "kept"].contains(&kind) && ep < epoch);
        if old && e.file_type()?.is_file() {
            std::fs::remove_file(e.path())?;
        }
    }
    eprintln!("items: {} facts, articles for {} items", facts_q.len(), views_q.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(kv: &[(&str, &str)]) -> Py {
        Py::Dict(
            kv.iter().map(|(k, v)| (k.to_string(), Py::Dict(vec![("value".into(), Py::Str(v.to_string()))]))).collect(),
        )
    }

    #[test]
    fn records_as_python() {
        let b = binding(&[
            ("item", "http://www.wikidata.org/entity/Q1"),
            ("sl", "12"),
            ("range", "Alps"),
            ("d_en", "mountain"),
            ("d_loc", ""),
            ("inc", "1850-01-01T00:00:00Z"),
            ("v_focal", "12.345"),
            ("v_height", "8848.86"),
            ("v_prominence", "1e3"),
        ]);
        assert_eq!(
            dumps_spaced(&record(&b).unwrap()),
            r#"{"sl": 12, "d_en": "mountain", "range": "Alps", "inception": "1850-01-01", "height": 8848.86, "prominence": 1000.0, "focal": 12.35}"#
        );
        // (No sitelinks: 0.)
        assert_eq!(dumps_spaced(&record(&binding(&[])).unwrap()), r#"{"sl": 0}"#);
        assert!(record(&binding(&[("v_height", "tall")])).is_err());
    }

    #[test]
    fn a_cut_short_line_dropped() {
        let d = std::env::temp_dir().join(format!("items-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("facts-x.jsonl");
        std::fs::write(
            &p,
            "{\"qid\": \"Q1\", \"sl\": 1}\n{\"qid\": \"Q2\", \"sl\": 2}\n{\"qid\": \"Q1\", \"sl\": 3}\n{\"qid\": \"Q",
        )
        .unwrap();
        let r = read_jsonl(&p).unwrap();
        assert_eq!(
            r.iter().map(|(q, v)| format!("{q}{}", dumps_spaced(v))).collect::<Vec<_>>(),
            [r#"Q1{"qid": "Q1", "sl": 3}"#, r#"Q2{"qid": "Q2", "sl": 2}"#]
        );
        assert!(std::fs::read_to_string(&p).unwrap().ends_with("3}\n"));
        append_jsonl(&p, &[with_qid("Q3", &Py::Dict(vec![("sl".into(), Py::Int("0".into()))]))]).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().ends_with("3}\n{\"qid\": \"Q3\", \"sl\": 0}\n"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
