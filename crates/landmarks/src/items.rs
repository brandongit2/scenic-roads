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
//! here saying which of its archives they last matched: agentcore::answers), so a run on another Mac,
//! or here after the cache is lost, asks only about items not seen yet. To ask Wikidata again,
//! delete facts-<epoch>.jsonl (or wp-) here: the deletion goes to the NAS with the next run, which
//! asks about every item again. Older epochs' files go at the end of a run.
//!
//! A batch's query asks each fact in a UNION branch holding the batch's items, so its cost is the
//! items' (crate::wikiask::fact_union); each fact is picked the same way whatever order the rows
//! come in (`record`).
//!
//! Output: `<out>/facts.json` (QID → record, every item asked: `{"sl": 0}` for one Wikidata has
//! nothing of or doesn't know; a cached record's facts `record` doesn't make, an older query's,
//! left out). A failed query fails the
//! run; one too costly for the service is asked in halves, and an item too costly alone is left
//! out with a log line (not cached, so asked again by the next run: crate::wikiask). An unknown
//! value, or one that isn't a number, is no value. The files' bytes are as Python's `json` writes
//! them (pyport::pyjson).

use crate::heritagewd::{wikipedias_told, write_atomic};
use crate::interest::py_round;
use crate::wikiask::{by_item, earliest, fact_union, labels, largest, least, most, val, Asker};
use anyhow::{Context, Result};
use jobrt::jobs::report;
use jobrt::timings::phase;
use pyport::pyjson::{dumps_spaced, loads, Py};
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;

/// Items fetched between cache writes.
const CHUNK: usize = 5000;
/// Items per Wikidata query.
const WD_BATCH: usize = 250;

/// The numeric facts: their names and properties.
const Q_PROPS: [(&str, &str); 5] =
    [("height", "P2048"), ("prominence", "P2660"), ("isolation", "P2659"), ("discharge", "P2225"), ("focal", "P2923")];

/// The facts query for `chunk`: each fact's values as rows (`?item ?k ?v`), a UNION of one branch a
/// fact holding the items (crate::wikiask::fact_union), put together by `record`.
fn facts_query(chunk: &[String]) -> String {
    let values = chunk.iter().map(|q| format!("wd:{q}")).collect::<Vec<_>>().join(" ");
    let en = |p: &str| format!(r#"?item wdt:{p} ?o . ?o rdfs:label ?v FILTER(STR(LANG(?v)) = "en")"#);
    let mut facts: Vec<(&str, String)> = vec![
        ("sl", "?item wikibase:sitelinks ?v".into()),
        ("d_en", r#"?item schema:description ?v FILTER(STR(LANG(?v)) = "en")"#.into()),
        ("w_en", "?a schema:about ?item ; schema:isPartOf <https://en.wikipedia.org/> ; schema:name ?v".into()),
        ("water", en("P206")),
        ("range", en("P4552")),
        ("inc", "?item wdt:P571 ?v".into()),
    ];
    facts.extend(
        Q_PROPS
            .iter()
            .map(|(n, p)| (*n, format!("?item p:{p} ?st . ?st psn:{p} ?nv . ?nv wikibase:quantityAmount ?v"))),
    );
    fact_union("?item ?k ?v", &format!("VALUES ?item {{ {values} }}"), facts)
}

/// An item's record from its rows (`?k ?v`, `qid`'s), each fact picked the same way whatever
/// order they come in: sitelinks (0 for none), English short description and article (the least,
/// where the service has two), water bodies (P206) and mountain ranges (P4552) by their English
/// labels (distinct and sorted), the earliest inception, and the numeric facts' largest to 2
/// decimals. (What reads them: web/src/details.ts, crate::marksjob, crate::interest.)
fn record(rows: &[&Py], qid: &str) -> Py {
    let sl = most(rows, "sl", qid).map_or(0, |(n, _)| n);
    let mut rec = vec![("sl".to_string(), Py::Int(sl.to_string()))];
    for k in ["d_en", "w_en"] {
        if let Some(v) = least(rows, k).and_then(|b| val(b, "v")) {
            rec.push((k.into(), Py::Str(v.into())));
        }
    }
    for k in ["water", "range"] {
        if let Some(v) = labels(rows, k) {
            rec.push((k.into(), Py::Str(v)));
        }
    }
    if let Some(v) = earliest(rows, "inc").and_then(|b| val(b, "v")) {
        rec.push(("inception".into(), Py::Str(v.chars().take(10).collect())));
    }
    for (n, _) in Q_PROPS {
        if let Some(x) = largest(rows, n, qid) {
            rec.push((n.into(), Py::Float(py_round(x, 2))));
        }
    }
    Py::Dict(rec)
}

/// A cached record with `record`'s facts alone (not its "qid"): one an older query made holds
/// others (`d_loc`, another language's description, and `elevation`), which nothing reads.
fn kept(r: &Py) -> Py {
    let made = |k: &str| {
        ["sl", "d_en", "w_en", "water", "range", "inception"].contains(&k) || Q_PROPS.iter().any(|(n, _)| *n == k)
    };
    match r {
        Py::Dict(d) => Py::Dict(d.iter().filter(|(k, _)| made(k)).cloned().collect()),
        other => other.clone(),
    }
}

/// Facts of these items from the Wikidata endpoint, WD_BATCH a query (`progress` told how many
/// are done after each batch): each item asked and its record (`{"sl": 0}` for one Wikidata has
/// nothing of, or doesn't know: merged, deleted), in `qids`' order (an item too costly a query
/// alone is left out: crate::wikiask::Asker::sparql_batches).
fn facts_of(
    ask: &Asker,
    endpoint: &str,
    qids: &[String],
    progress: &mut dyn FnMut(usize),
) -> Result<Vec<(String, Py)>> {
    let mut out = vec![];
    ask.sparql_batches(endpoint, qids, WD_BATCH, &facts_query, &mut |chunk, rows| {
        let by = by_item(&rows)?;
        for q in chunk {
            out.push((q.clone(), record(by.get(q.as_str()).map_or(&[][..], |r| r), q)));
        }
        eprintln!("  wikidata {}/{}", out.len(), qids.len());
        progress(out.len());
        Ok(())
    })?;
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
    let p = phase("reading the caches");
    let dpath = cache.join(format!("fetched-{epoch}.json"));
    let mut fetched = match std::fs::read_to_string(&dpath) {
        Ok(t) => loads(&t).context("parse fetched.json")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Py::Dict(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut note_fetch = || -> Result<()> {
        let today = jobrt::timings::day_of(std::time::SystemTime::now());
        if fetched.get("first").is_none() {
            fetched.set("first", Py::Str(today.clone()));
        }
        fetched.set("last", Py::Str(today));
        write_atomic(&dpath, &dumps_spaced(&fetched))
    };
    let fpath = cache.join(format!("facts-{epoch}.jsonl"));
    let mut facts: HashMap<String, Py> = read_jsonl(&fpath)?.into_iter().map(|(q, r)| (q, kept(&r))).collect();
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
        let p = phase("fetching facts from Wikidata");
        let got = facts_of(&ask, wikidata, chunk, &mut |n| report((k + n) as u64, todo.len() as u64, what))?;
        drop(p);
        let _p = phase("appending to the caches");
        append_jsonl(&fpath, &got.iter().map(|(q, r)| with_qid(q, r)).collect::<Vec<_>>())?;
        facts.extend(got);
        note_fetch()?;
    }

    // Each item's Wikipedia articles: this epoch's.
    let wpath = cache.join(format!("wp-{epoch}.jsonl"));
    let p = phase("reading the caches");
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
        let _p = phase("appending to the caches");
        append_jsonl(&wpath, &got.iter().map(|(q, r)| with_qid(q, r)).collect::<Vec<_>>())?;
        note_fetch()?;
    }

    let p = phase("writing the outputs");
    // (An item left out, too costly a query alone, has no facts this run.)
    let known: Vec<(String, Py)> = facts_q.iter().filter_map(|q| Some((q.clone(), facts.get(q)?.clone()))).collect();
    let file = out.join("facts.json");
    write_atomic(&file, &dumps_spaced(&Py::Dict(known)))?;
    drop(p);
    // Older epochs' caches go.
    let _p = phase("deleting the old caches");
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

    /// A row of the facts query's answer: `q`'s fact `k`, its value `v` (of `ty`).
    fn row(q: &str, k: &str, ty: &str, v: &str) -> String {
        format!(
            r#"{{"item": {{"type": "uri", "value": "http://www.wikidata.org/entity/{q}"}}, "k": {{"type": "literal", "value": "{k}"}}, "v": {{"type": "{ty}", "value": "{v}"}}}}"#
        )
    }

    /// Task #1151: each fact picked the same way whatever order the service's rows come in (the
    /// old query's SAMPLE and GROUP_CONCAT took the service's order); an item with no rows
    /// (Wikidata has nothing of it) answered `{"sl": 0}`, as the old query's empty row was.
    #[test]
    fn records_from_the_rows() {
        use crate::wikiask::tests::{bindings, noting, replay};
        let genid = "http://www.wikidata.org/.well-known/genid/9e0e785fb54b8e85182f3338680c3feb";
        let rows = [
            row("Q1", "sl", "literal", "12"),
            row("Q1", "range", "literal", "Alps"),
            row("Q1", "range", "literal", "Bernese Alps"),
            row("Q1", "range", "literal", "Alps"),
            row("Q1", "d_en", "literal", "mountain in Switzerland"),
            row("Q1", "d_en", "literal", "mountain"),
            row("Q1", "inc", "literal", "1850-01-01T00:00:00Z"),
            row("Q1", "inc", "literal", "-0300-01-01T00:00:00Z"),
            row("Q1", "focal", "literal", "12.345"),
            row("Q1", "height", "literal", "8848.86"),
            row("Q1", "height", "literal", "8000"),
            row("Q1", "prominence", "literal", "1e3"),
            row("Q3", "height", "literal", "tall"),
            row("Q3", "focal", "literal", "2"),
            row("Q3", "inc", "uri", genid),
            row("Q3", "discharge", "uri", genid),
        ];
        let want = [
            r#"Q1 {"sl": 12, "d_en": "mountain", "range": "Alps|Bernese Alps", "inception": "-0300-01-0", "height": 8848.86, "prominence": 1000.0, "focal": 12.35}"#,
            // (No sitelinks: 0. A value that isn't a number, or is unknown, is none.)
            r#"Q2 {"sl": 0}"#,
            r#"Q3 {"sl": 0, "focal": 2.0}"#,
        ];
        let qids: Vec<String> = ["Q1", "Q2", "Q3"].map(String::from).into();
        for order in [rows.to_vec(), rows.iter().rev().cloned().collect()] {
            let (url, _) = replay(vec![(200, None, bindings(&order.join(",")))]);
            let (ask, _) = noting();
            let got = facts_of(&ask, &url, &qids, &mut |_| {}).unwrap();
            assert_eq!(got.iter().map(|(q, r)| format!("{q} {}", dumps_spaced(r))).collect::<Vec<_>>(), want);
        }
        let q = facts_query(&qids);
        assert_eq!(q.matches("VALUES ?item { wd:Q1 wd:Q2 wd:Q3 }").count(), 6 + Q_PROPS.len());
        assert!(!q.contains("OPTIONAL") && !q.contains(" LANG("), "{q}");
        // An older query's record: the facts nothing reads left out of facts.json.
        let old =
            loads(r#"{"qid": "Q1", "sl": 1, "d_en": "x", "d_loc": "y", "elevation": 3.0, "height": 2.0}"#).unwrap();
        assert_eq!(dumps_spaced(&kept(&old)), r#"{"sl": 1, "d_en": "x", "height": 2.0}"#);
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
