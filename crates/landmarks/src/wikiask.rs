//! Asking Wikidata's query service and Wikipedia's API, and Wikimedia's dumps, for the heritage
//! chain's and the items job's Rust steps (crate::heritagewd, crate::items, crate::pageviews): the
//! build's User-Agent (buildstore::fetch::USER_AGENT), every request refused offline first
//! (buildstore::fetch::online), through the job's proxy (`HTTPS_PROXY`, which takes each from the pool's
//! budget by its rate class: jobrt::budget). What it asks is declared APIs and data downloads
//! (buildstore::fetch::Kind's `api` and `data`), never pages, so no robots.txt is read. Answers are read
//! as Python's `json` reads them (pyport::pyjson), for caches whose bytes stay the Python's.
//!
//! What a service answers is sorted three ways besides an answer (`sort`):
//!
//! - **A wait**: a 429 or a `Retry-After`, the service asking for time. Told to the job's proxy
//!   (jobrt::budget::seen: every process and Mac held that long), waited out here as long
//!   (jobrt::budget::hold_s), and asked again: a wait is no failure. A service still refusing once
//!   the waits for one request add up past the longest hold the pool honours
//!   (jobrt::budget::HOLD_MAX_S) fails the run, what it saved kept.
//! - **Too costly**: the query ran past the service's time limit. QLever says so with a 429 and its
//!   error object, whose `exception` says it timed out (its server answers every cancelled query
//!   429, with no `Retry-After`); the Wikidata Query Service with a 500 naming its
//!   `TimeoutException`. Anything else is a wait, so a rate limit is never asked in halves. The
//!   same query would only run out again: it isn't asked again, nor told as a hold (no one asked
//!   too often, and the proxy has charged its time to the budget). A batch is asked again in halves
//!   (`Asker::sparql_batches`) down to one item; an item too costly alone is left out with a log
//!   line, asked again by the next run. More queries too costly in a row than one item, or two
//!   side by side, would give (the batch's halvings and two) is a service answering nothing, which
//!   fails the run rather than asking every half. A query that is no batch fails (`TooCostly`).
//! - **A failure**: no answer, or another error: asked again after a hold's default wait
//!   (jobrt::budget::HOLD_DEFAULT_S) times the failures so far, `FAILURES` in a row failing the run.
//!
//! A statement whose value is unknown (`somevalue`) is answered as a blank node, or the
//! `.well-known/genid/` IRI Wikidata's services give one; `val` reads either as no value (a
//! `novalue` statement has none to read). A value meant to be a number that isn't one is left out
//! with a log line (`number`, `int`): one statement never fails a run.

use anyhow::{bail, Result};
use pyport::pyjson::{loads, Py};
use std::time::Duration;

/// The prefixes every query is sent with.
pub const PREFIXES: &str =
    "PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> \
PREFIX schema: <http://schema.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
PREFIX wikibase: <http://wikiba.se/ontology#> ";

/// The IRIs Wikidata's query services give an unknown value (a blank node, skolemised).
const GENID: &str = "http://www.wikidata.org/.well-known/genid/";

/// Failures in a row (no answer, or an error that isn't a wait) that fail the run.
const FAILURES: u64 = 5;

/// A binding's value, `b[k]["value"]`: None when missing, empty, or an unknown value (a blank
/// node, or its genid IRI).
pub fn val<'a>(b: &'a Py, k: &str) -> Option<&'a str> {
    let v = b.get(k)?;
    if v.at("type").as_str() == Some("bnode") {
        return None;
    }
    v.at("value").as_str().filter(|s| !s.is_empty() && !s.starts_with(GENID))
}

/// Whether a binding holds an unknown value (`val`'s None for a value there).
pub fn unknown(v: &Py) -> bool {
    v.at("type").as_str() == Some("bnode") || v.at("value").as_str().is_some_and(|s| s.starts_with(GENID))
}

/// A binding's value `k` as a number (Python's `float`): None when it has none (`val`), or, with a
/// log line naming `what`, when it isn't a finite number.
fn number(b: &Py, k: &str, what: &str) -> Option<f64> {
    let v = val(b, k)?;
    let x = pyport::pytext::float(v).filter(|x| x.is_finite());
    if x.is_none() {
        eprintln!("  {what}: {k} {v:?} not a number, left out");
    }
    x
}

/// A binding's value `k` as an integer: None as `number`'s.
pub fn int(b: &Py, k: &str, what: &str) -> Option<i128> {
    let v = val(b, k)?;
    let x = pyport::pytext::strip(v).parse::<i128>().ok();
    if x.is_none() {
        eprintln!("  {what}: {k} {v:?} not an integer, left out");
    }
    x
}

/// A query of facts (task #1144): each fact's values as rows (`select`, `?k` the fact's name, `?v`
/// its value), a UNION of one branch a fact, each starting with `items` (the batch's VALUES and
/// what binds `?item` from them) before the fact's pattern (`?v` bound from `?item`). QLever works
/// an OPTIONAL or a UNION's branch out on its own before joining it, and applies a language filter
/// only after its scan: a fact's OPTIONAL naming no item read every description or label in
/// Wikidata, 9-45 s whatever the batch's size (more when the service was busy), past its time
/// limit even for one item. A branch holding the items is joined with them first, so its cost is
/// theirs. Language filters are written `STR(LANG(…))`: QLever's planner turns `LANG(?x) = "en"`
/// into a scan of every English literal. The rows' order is the service's: what is made of them
/// mustn't depend on it.
pub(crate) fn fact_union<K: std::fmt::Display, P: std::fmt::Display>(
    select: &str,
    items: &str,
    facts: impl IntoIterator<Item = (K, P)>,
) -> String {
    let branches: Vec<String> =
        facts.into_iter().map(|(k, pattern)| format!("{{ {items} {pattern} BIND(\"{k}\" AS ?k) }}")).collect();
    format!(
        "PREFIX psn: <http://www.wikidata.org/prop/statement/value-normalized/> PREFIX p: <http://www.wikidata.org/prop/>\n\
         SELECT {select} WHERE {{\n  {}\n}}",
        branches.join("\n  UNION ")
    )
}

/// A `fact_union` answer's rows by their item's QID.
pub(crate) fn by_item(rows: &[Py]) -> Result<std::collections::HashMap<&str, Vec<&Py>>> {
    let mut by: std::collections::HashMap<&str, Vec<&Py>> = Default::default();
    for b in rows {
        let Some(item) = val(b, "item") else { bail!("a binding without its item") };
        by.entry(item.rsplit('/').next().unwrap_or(item)).or_default().push(b);
    }
    Ok(by)
}

/// The rows of fact `k` (a `fact_union` answer's `?k`).
fn fact<'a>(rows: &'a [&'a Py], k: &'a str) -> impl Iterator<Item = &'a Py> + 'a {
    rows.iter().filter(move |b| val(b, "k") == Some(k)).copied()
}

/// Whether a row's value `?v` is a literal (not an item, nor an unknown value's IRI).
fn literal(b: &&Py) -> bool {
    b.at("v").at("type").as_str().is_some_and(|t| t.contains("literal"))
}

/// The labels of fact `k`'s rows, distinct and sorted, `|` between them (None for none).
pub(crate) fn labels(rows: &[&Py], k: &str) -> Option<String> {
    let l: std::collections::BTreeSet<&str> = fact(rows, k).filter_map(|b| val(b, "v")).collect();
    (!l.is_empty()).then(|| l.into_iter().collect::<Vec<_>>().join("|"))
}

/// A date's order (Wikidata's `xsd:dateTime`, a year before Christ signed): its year, then the rest.
fn date_key(d: &str) -> (i64, &str) {
    let (neg, rest) = d.strip_prefix('-').map_or((false, d), |r| (true, r));
    let n = rest.bytes().take_while(u8::is_ascii_digit).count();
    let y: i64 = rest[..n].parse().unwrap_or(0);
    (if neg { -y } else { y }, &rest[n..])
}

/// Fact `k`'s earliest literal date's row.
pub(crate) fn earliest<'a>(rows: &'a [&'a Py], k: &'a str) -> Option<&'a Py> {
    fact(rows, k).filter(literal).filter(|b| val(b, "v").is_some()).min_by(|a, b| {
        let (x, y) = (val(a, "v").unwrap_or(""), val(b, "v").unwrap_or(""));
        date_key(x).cmp(&date_key(y)).then(x.cmp(y))
    })
}

/// Fact `k`'s row with the least value.
pub(crate) fn least<'a>(rows: &'a [&'a Py], k: &'a str) -> Option<&'a Py> {
    fact(rows, k).filter(|b| val(b, "v").is_some()).min_by_key(|b| val(b, "v"))
}

/// Fact `k`'s largest integer and its row (a value that isn't one left out, logged naming `what`).
pub(crate) fn most<'a>(rows: &'a [&'a Py], k: &'a str, what: &str) -> Option<(i128, &'a Py)> {
    fact(rows, k).filter_map(|b| Some((int(b, "v", what)?, b))).max_by_key(|(n, _)| *n)
}

/// Fact `k`'s largest literal number (a value that isn't one left out, logged naming `what`).
pub(crate) fn largest(rows: &[&Py], k: &str, what: &str) -> Option<f64> {
    fact(rows, k)
        .filter(literal)
        .filter_map(|b| number(b, "v", what))
        .fold(None, |m, x| Some(m.map_or(x, |m: f64| m.max(x))))
}

/// What a service said to a request (see the module's docs).
#[derive(Debug, PartialEq)]
enum Said {
    Answer(String),
    /// Wait this long (s), then ask again.
    Wait(u64, String),
    TooCostly(String),
    Failed(String),
}

/// A request's outcome sorted: its status (0: no answer), its body (or why there was none), its
/// `Retry-After`.
fn sort(status: u16, body: String, retry: Option<&str>) -> Said {
    let retry = retry.filter(|r| !r.trim().is_empty());
    if status == 200 {
        return Said::Answer(body);
    }
    let said = format!("HTTP {status}: {}", body.chars().take(300).collect::<String>());
    let timed_out = |e: &str| {
        let e = e.to_ascii_lowercase();
        ["timed out", "timeout", "time limit"].iter().any(|w| e.contains(w))
    };
    let cancelled = status == 429
        && retry.is_none()
        && loads(&body).is_ok_and(|v| v.get("exception").and_then(Py::as_str).is_some_and(timed_out));
    if cancelled || (status == 500 && body.contains("TimeoutException")) {
        return Said::TooCostly(said);
    }
    if status == 429 || retry.is_some() {
        return Said::Wait(jobrt::budget::hold_s(retry), said);
    }
    Said::Failed(said)
}

/// Queries too costly in a row (`Asker::sparql_batches`): how many, and the most a batch's costly
/// keys give.
struct Costly {
    in_a_row: usize,
    most: usize,
}

/// A query that ran past the query service's time limit: what it said.
#[derive(Debug)]
pub struct TooCostly(pub String);

impl std::fmt::Display for TooCostly {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "the query ran past the query service's time limit ({})", self.0)
    }
}

impl std::error::Error for TooCostly {}

/// What asks: one agent (its connections kept).
pub struct Asker {
    agent: ureq::Agent,
    /// How it waits (the tests': noted, no time passing).
    wait: Box<dyn Fn(Duration) + Send + Sync>,
}

impl Default for Asker {
    fn default() -> Self {
        Self::new()
    }
}

impl Asker {
    pub fn new() -> Asker {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(180)))
            .timeout_connect(Some(Duration::from_secs(30)))
            .user_agent(buildstore::fetch::USER_AGENT)
            .http_status_as_error(false)
            .proxy(ureq::Proxy::try_from_env())
            // (No redirect followed: MediaWiki resolves redirects in its answer.)
            .max_redirects(0)
            .build()
            .into();
        Asker { agent, wait: Box::new(std::thread::sleep) }
    }

    /// The size `url`'s server says it has (a HEAD's Content-Length): 0 when it doesn't say, or
    /// can't be asked (offline, no answer).
    pub fn content_length(&self, url: &str) -> u64 {
        if buildstore::fetch::online(url).is_err() {
            return 0;
        }
        let _w = store::waits::net();
        let r = self.agent.head(url).config().timeout_global(Some(Duration::from_secs(30))).build().call();
        r.ok()
            .filter(|r| r.status() == 200)
            .and_then(|r| r.headers().get("content-length")?.to_str().ok()?.trim().parse().ok())
            .unwrap_or(0)
    }

    /// `request` made until its answer reads (`read`; None: failed): waits waited out (told to the
    /// proxy for `url`'s host), failures asked again (see the module's docs), a query too costly
    /// an error (`TooCostly`).
    fn until_answered<T>(
        &self,
        url: &str,
        request: impl Fn() -> Result<(u16, String, Option<String>)>,
        read: impl Fn(&str) -> Option<T>,
    ) -> Result<T> {
        let (mut failures, mut waited) = (0, 0);
        loop {
            let (status, body, retry) = {
                let _w = store::waits::net();
                request().unwrap_or_else(|e| (0, format!("{e:#}"), None))
            };
            let why = match sort(status, body, retry.as_deref()) {
                Said::Answer(b) => match read(&b) {
                    Some(t) => return Ok(t),
                    None => format!("an answer that doesn't read: {}", b.chars().take(300).collect::<String>()),
                },
                Said::Wait(s, said) => {
                    jobrt::budget::seen(url, status, retry.as_deref());
                    waited += s;
                    if waited > jobrt::budget::HOLD_MAX_S {
                        bail!(
                            "{url}: still asking to wait after {} s of waits, past the longest hold the pool honours ({said})",
                            waited - s
                        );
                    }
                    eprintln!("  asked to wait {s} s ({})", said.chars().take(120).collect::<String>());
                    failures = 0;
                    (self.wait)(Duration::from_secs(s));
                    continue;
                }
                Said::TooCostly(said) => return Err(TooCostly(said).into()),
                Said::Failed(said) => said,
            };
            failures += 1;
            if failures == FAILURES {
                bail!("{url}: failed {FAILURES} times in a row ({why})");
            }
            (self.wait)(Duration::from_secs(jobrt::budget::HOLD_DEFAULT_S * failures));
        }
    }

    /// A SPARQL query (`PREFIXES` before it) POSTed to `endpoint`: its result's bindings.
    pub fn sparql(&self, endpoint: &str, q: &str) -> Result<Vec<Py>> {
        Ok(self.sparql_answer(endpoint, q)?.1)
    }

    /// `sparql`'s answer with its body as it came (a cache of it keeps the service's bytes).
    pub fn sparql_answer(&self, endpoint: &str, q: &str) -> Result<(String, Vec<Py>)> {
        buildstore::fetch::online(endpoint)?;
        let body = format!("{PREFIXES}{q}");
        self.until_answered(
            endpoint,
            || {
                let r = self
                    .agent
                    .post(endpoint)
                    .header("Accept", "application/sparql-results+json")
                    .send_form([("query", body.as_str())]);
                answer(r)
            },
            |text| match loads(text).ok()?.at("results").get("bindings")? {
                Py::List(b) => Some((text.to_string(), b.clone())),
                _ => None,
            },
        )
    }

    /// `keys` asked of `endpoint` `batch` at a time (`query` making a batch's query), each batch's
    /// keys and bindings given to `answered` as it comes. A batch too costly for the service is
    /// asked again in halves, down to one key; a key too costly alone is left out (not given to
    /// `answered`) with a log line. Too costly in a row past what one costly key, or two side by
    /// side, would give: the service answering nothing, an error (`TooCostly`).
    pub fn sparql_batches(
        &self,
        endpoint: &str,
        keys: &[String],
        batch: usize,
        query: &dyn Fn(&[String]) -> String,
        answered: &mut dyn FnMut(&[String], Vec<Py>) -> Result<()>,
    ) -> Result<()> {
        let mut costly = Costly { in_a_row: 0, most: 0 };
        for chunk in keys.chunks(batch.max(1)) {
            // (One costly key: the whole and each half holding it down to the key alone, its
            // halvings and one; a costly key beside it, one more.)
            costly.most = (usize::BITS - (chunk.len() - 1).leading_zeros()) as usize + 2;
            self.halves(endpoint, chunk, query, answered, &mut costly)?;
        }
        Ok(())
    }

    fn halves(
        &self,
        endpoint: &str,
        keys: &[String],
        query: &dyn Fn(&[String]) -> String,
        answered: &mut dyn FnMut(&[String], Vec<Py>) -> Result<()>,
        costly: &mut Costly,
    ) -> Result<()> {
        match self.sparql(endpoint, &query(keys)) {
            Ok(rows) => {
                costly.in_a_row = 0;
                answered(keys, rows)
            }
            Err(e) => match e.downcast::<TooCostly>() {
                Ok(t) if costly.in_a_row >= costly.most => Err(TooCostly(format!(
                    "{} queries in a row, more than costly items give: the service answers nothing; the last {}",
                    costly.most + 1,
                    t.0
                ))
                .into()),
                Ok(_) if keys.len() > 1 => {
                    costly.in_a_row += 1;
                    eprintln!("  a query of {} items ran past the service's time limit: asked in halves", keys.len());
                    let (a, b) = keys.split_at(keys.len() / 2);
                    self.halves(endpoint, a, query, answered, costly)?;
                    self.halves(endpoint, b, query, answered, costly)
                }
                Ok(t) => {
                    costly.in_a_row += 1;
                    eprintln!("  {}: left out, {t}", keys[0]);
                    Ok(())
                }
                Err(e) => Err(e),
            },
        }
    }

    /// A JSON answer to a GET of `url` (see the module's docs for what is asked again).
    pub fn get_json(&self, url: &str) -> Result<Py> {
        buildstore::fetch::online(url)?;
        self.until_answered(
            url,
            || answer(self.agent.get(url).config().timeout_global(Some(Duration::from_secs(60))).build().call()),
            |t| loads(t).ok(),
        )
    }
}

fn answer(r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, String, Option<String>)> {
    let mut r = r?;
    let retry = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
    let text = r.body_mut().with_config().limit(1 << 30).read_to_string()?;
    Ok((r.status().as_u16(), text, retry))
}

/// `application/x-www-form-urlencoded`, as Python's `urllib.parse.urlencode`.
pub fn urlencode(kv: &[(&str, &str)]) -> String {
    let enc = |s: &str| {
        let mut o = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => o.push(b as char),
                b' ' => o.push('+'),
                _ => o.push_str(&format!("%{b:02X}")),
            }
        }
        o
    };
    kv.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    /// QLever's answer to the heritage step's park query on 7 Nov (its body as the job's log kept
    /// it, the query cut): a query past its time limit.
    pub(crate) const QLEVER_TIMED_OUT: &str = "{\n    \"exception\": \"Operation timed out. Last operation: Sort (internal order) on ?item\",\n    \"query\": \"PREFIX wd: <http://www.wikidata.org/entity/> SELECT ?item WHERE { }\",\n    \"status\": \"ERROR\"\n}";

    /// A recorded answer: status, `Retry-After`, body.
    pub(crate) type Reply = (u16, Option<&'static str>, String);

    /// A server on this Mac answering each request with the next of `replies` (the last again
    /// once they run out), noting the queries (the POSTs' `query`, or the GETs' paths): its URL and
    /// what it was asked.
    pub(crate) fn replay(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://{}/sparql", l.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(vec![]));
        let seen = asked.clone();
        std::thread::spawn(move || {
            let mut k = 0;
            loop {
                let mut s = match store::spawn::accept(&l) {
                    Ok((s, _)) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(_) => break,
                };
                s.set_nonblocking(false).unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split(' ').nth(1).unwrap_or("").to_string();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let form = String::from_utf8(body).unwrap();
                let q = form.strip_prefix("query=").map_or(path, |q| q.to_string());
                seen.lock().unwrap().push(q);
                let (status, retry, text) = replies[k.min(replies.len() - 1)].clone();
                k += 1;
                let retry = retry.map_or(String::new(), |r| format!("Retry-After: {r}\r\n"));
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{retry}Connection: close\r\n\r\n",
                    text.len()
                );
                s.write_all(head.as_bytes()).unwrap();
                s.write_all(text.as_bytes()).unwrap();
            }
        });
        (url, asked)
    }

    /// An asker whose waits are noted, not waited.
    pub(crate) fn noting() -> (Asker, Arc<Mutex<Vec<u64>>>) {
        let waits = Arc::new(Mutex::new(vec![]));
        let w = waits.clone();
        let mut a = Asker::new();
        a.wait = Box::new(move |d| w.lock().unwrap().push(d.as_secs()));
        (a, waits)
    }

    pub(crate) fn bindings(rows: &str) -> String {
        format!("{{\"head\": {{\"vars\": []}}, \"results\": {{\"bindings\": [{rows}]}}}}")
    }

    #[test]
    fn urls() {
        assert_eq!(urlencode(&[("t", "A b|é")]), "t=A+b%7C%C3%A9");
    }

    #[test]
    fn answers_sorted() {
        assert_eq!(sort(200, "x".into(), None), Said::Answer("x".into()));
        assert!(matches!(sort(429, QLEVER_TIMED_OUT.into(), None), Said::TooCostly(_)));
        let wdqs = "SPARQL-QUERY: ... java.util.concurrent.TimeoutException\n\tat ...";
        assert!(matches!(sort(500, wdqs.into(), None), Said::TooCostly(_)));
        // (A 429 asking for time: its Retry-After, else the hold's default; a date is no seconds.)
        assert!(matches!(sort(429, "Too many requests".into(), Some("7")), Said::Wait(7, _)));
        assert!(matches!(sort(429, QLEVER_TIMED_OUT.into(), Some("7")), Said::Wait(7, _)));
        // (A 429 whose error isn't a time-out is a wait, never halved.)
        let limited = r#"{"exception": "Too many requests from this address", "status": "ERROR"}"#;
        assert!(matches!(sort(429, limited.into(), None), Said::Wait(60, _)));
        let d = jobrt::budget::HOLD_DEFAULT_S;
        assert!(matches!(sort(429, "".into(), None), Said::Wait(s, _) if s == d));
        assert!(matches!(sort(429, "".into(), Some("Wed, 21 Oct 2026 07:28:00 GMT")), Said::Wait(s, _) if s == d));
        assert!(matches!(sort(503, "".into(), Some("2")), Said::Wait(2, _)));
        assert!(matches!(sort(503, "".into(), None), Said::Failed(_)));
        assert!(matches!(sort(0, "connection refused".into(), None), Said::Failed(_)));
    }

    #[test]
    fn unknown_values_are_no_values() {
        let b = loads(
            r#"{"v": {"type": "uri", "value": "http://www.wikidata.org/.well-known/genid/9e0e785fb54b8e85182f3338680c3feb"},
                "n": {"type": "bnode", "value": "b0"}, "x": {"type": "literal", "value": "12.5"},
                "s": {"type": "literal", "value": "lots"}, "i": {"value": " 3 "}}"#,
        )
        .unwrap();
        assert_eq!((val(&b, "v"), val(&b, "n"), val(&b, "x")), (None, None, Some("12.5")));
        assert!(unknown(b.at("v")) && unknown(b.at("n")) && !unknown(b.at("x")));
        assert_eq!(number(&b, "v", "t"), None);
        assert_eq!(number(&b, "x", "t"), Some(12.5));
        assert_eq!(number(&b, "s", "t"), None, "not a number: left out");
        assert_eq!(int(&b, "i", "t"), Some(3));
        assert_eq!(int(&b, "x", "t"), None);
    }

    /// Waits asked for are waited out and asked again, never counted as failures; Retry-After
    /// followed.
    #[test]
    fn waits_are_waited_out() {
        let ok = bindings(r#"{"item": {"type": "uri", "value": "http://www.wikidata.org/entity/Q1"}}"#);
        let mut replies: Vec<Reply> = (0..12).map(|_| (429, Some("3"), "slow down".to_string())).collect();
        replies.push((200, None, ok));
        let (url, asked) = replay(replies);
        let (a, waits) = noting();
        let rows = a.sparql(&url, "SELECT ?item WHERE { }").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(asked.lock().unwrap().len(), 13);
        assert_eq!(*waits.lock().unwrap(), vec![3; 12]);
    }

    /// A service still refusing past the longest hold the pool honours fails the run.
    #[test]
    fn endless_waits_end() {
        let (url, asked) = replay(vec![(429, Some("300"), "slow down".into())]);
        let (a, waits) = noting();
        let e = a.sparql(&url, "SELECT ?item WHERE { }").unwrap_err();
        assert!(format!("{e:#}").contains("still asking to wait after 900 s"), "{e:#}");
        assert_eq!(asked.lock().unwrap().len(), 4);
        assert_eq!(*waits.lock().unwrap(), vec![300; 3]);
    }

    /// Failures are asked again a few times, waiting longer each, then fail the run.
    #[test]
    fn failures_fail_in_the_end() {
        let (url, asked) = replay(vec![(502, None, "bad gateway".into())]);
        let (a, waits) = noting();
        let e = a.sparql(&url, "SELECT ?item WHERE { }").unwrap_err();
        assert!(format!("{e:#}").contains("failed 5 times in a row (HTTP 502: bad gateway)"), "{e:#}");
        assert_eq!(asked.lock().unwrap().len(), FAILURES as usize);
        let d = jobrt::budget::HOLD_DEFAULT_S;
        assert_eq!(*waits.lock().unwrap(), vec![d, 2 * d, 3 * d, 4 * d]);
    }

    /// A query past the time limit isn't asked again; a batch of them is asked in halves, and a
    /// key too costly alone left out.
    #[test]
    fn costly_batches_are_halved() {
        let (url, _) = replay(vec![(429, None, QLEVER_TIMED_OUT.into())]);
        let (a, waits) = noting();
        let e = a.sparql(&url, "SELECT ?item WHERE { }").unwrap_err();
        assert!(e.downcast_ref::<TooCostly>().is_some(), "{e:#}");
        // Four keys: the whole and its first half too costly, then Q1 and Q2 alone answered; Q3
        // and Q4 together answered.
        let row = |q: &str| bindings(&format!(r#"{{"item": {{"value": "http://www.wikidata.org/entity/{q}"}}}}"#));
        let t = || (429, None, QLEVER_TIMED_OUT.to_string());
        let (url, asked) = replay(vec![t(), t(), (200, None, row("Q1")), t(), (200, None, row("Q34"))]);
        let keys: Vec<String> = ["Q1", "Q2", "Q3", "Q4"].iter().map(|s| s.to_string()).collect();
        let mut got = vec![];
        a.sparql_batches(&url, &keys, 4, &|ks| ks.join(","), &mut |ks, rows| {
            got.push((ks.join(","), rows.len()));
            Ok(())
        })
        .unwrap();
        assert_eq!(got, vec![("Q1".to_string(), 1), ("Q3,Q4".to_string(), 1)], "Q2 left out");
        let asked: Vec<String> =
            asked.lock().unwrap().iter().map(|q| q.rsplit('+').next().unwrap().to_string()).collect();
        assert_eq!(asked, ["Q1%2CQ2%2CQ3%2CQ4", "Q1%2CQ2", "Q1", "Q2", "Q3%2CQ4"]);
        assert!(waits.lock().unwrap().is_empty(), "nothing waited: too costly isn't a wait");
        // A service answering nothing: eight keys, the whole and three halvings to a key, a key
        // beside it, then the run fails rather than asking all fifteen queries.
        let (url, asked) = replay(vec![t()]);
        let keys: Vec<String> = (1..=8).map(|i| format!("Q{i}")).collect();
        let e = a.sparql_batches(&url, &keys, 8, &|ks| ks.join(","), &mut |_, _| panic!("no answer")).unwrap_err();
        assert!(format!("{e:#}").contains("6 queries in a row"), "{e:#}");
        assert_eq!(asked.lock().unwrap().len(), 6);
    }
}
