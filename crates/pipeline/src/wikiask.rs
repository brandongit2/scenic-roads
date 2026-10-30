//! Asking Wikidata's query service and Wikipedia's API, and Wikimedia's dumps, for the heritage
//! chain's and the items job's Rust steps (crate::heritagewd, crate::items, crate::pageviews): the
//! build's User-Agent (crate::fetch::USER_AGENT), every request refused offline first
//! (crate::fetch::online), through the job's proxy (`HTTPS_PROXY`, which takes each from the pool's
//! budget by its rate class: crate::budget), a 429 or a `Retry-After` told to it
//! (crate::budget::seen) and backed off from. What it asks is declared APIs and data downloads
//! (crate::fetch::Kind's `api` and `data`), never pages, so no robots.txt is read. Answers are read
//! as Python's `json` reads them (crate::pyjson), for caches whose bytes stay the Python's.

use crate::pyjson::{loads, Py};
use anyhow::{bail, Result};
use std::time::Duration;

/// The prefixes every query is sent with.
pub const PREFIXES: &str =
    "PREFIX wd: <http://www.wikidata.org/entity/> PREFIX wdt: <http://www.wikidata.org/prop/direct/> \
PREFIX schema: <http://schema.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
PREFIX wikibase: <http://wikiba.se/ontology#> ";

/// A binding's value: `b[k]["value"]`, None when missing or empty.
pub fn val<'a>(b: &'a Py, k: &str) -> Option<&'a str> {
    b.get(k).and_then(|v| v.at("value").as_str()).filter(|s| !s.is_empty())
}

/// What asks: one agent (its connections kept).
pub struct Asker {
    agent: ureq::Agent,
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
            .user_agent(crate::fetch::USER_AGENT)
            .http_status_as_error(false)
            .proxy(ureq::Proxy::try_from_env())
            // (No redirect followed: MediaWiki resolves redirects in its answer.)
            .max_redirects(0)
            .build()
            .into();
        Asker { agent }
    }

    /// The size `url`'s server says it has (a HEAD's Content-Length): 0 when it doesn't say, or
    /// can't be asked (offline, no answer).
    pub fn content_length(&self, url: &str) -> u64 {
        if crate::fetch::online(url).is_err() {
            return 0;
        }
        let _w = store::waits::net();
        let r = self.agent.head(url).config().timeout_global(Some(Duration::from_secs(30))).build().call();
        r.ok()
            .filter(|r| r.status() == 200)
            .and_then(|r| r.headers().get("content-length")?.to_str().ok()?.trim().parse().ok())
            .unwrap_or(0)
    }

    /// GET: the status, the body, and a `Retry-After` if any.
    fn get_text(&self, url: &str, timeout_s: u64) -> Result<(u16, String, Option<String>)> {
        crate::fetch::online(url)?;
        let _w = store::waits::net();
        let r = self.agent.get(url).config().timeout_global(Some(Duration::from_secs(timeout_s))).build().call();
        answer(r)
    }

    /// A SPARQL query (`PREFIXES` before it) POSTed to `endpoint`: its result's bindings. Errors
    /// (no answer, not a result) are tried again with growing pauses, longer when rate-limited
    /// (429), ten times in all.
    pub fn sparql(&self, endpoint: &str, q: &str) -> Result<Vec<Py>> {
        Ok(self.sparql_answer(endpoint, q)?.1)
    }

    /// `sparql`'s answer with its body as it came (a cache of it keeps the service's bytes).
    pub fn sparql_answer(&self, endpoint: &str, q: &str) -> Result<(String, Vec<Py>)> {
        crate::fetch::online(endpoint)?;
        let body = format!("{PREFIXES}{q}");
        let mut last = String::new();
        for attempt in 1..=10u64 {
            let r = self
                .agent
                .post(endpoint)
                .header("Accept", "application/sparql-results+json")
                .send_form([("query", body.as_str())]);
            let (status, text, retry) = match answer(r) {
                Ok(a) => a,
                Err(e) => (0, format!("{e:#}"), None),
            };
            if status == 200 {
                if let Some(Py::List(b)) = loads(&text).ok().and_then(|v| v.at("results").get("bindings").cloned()) {
                    return Ok((text, b));
                }
            }
            crate::budget::seen(endpoint, status, retry.as_deref());
            last = format!("HTTP {status}: {}", text.chars().take(300).collect::<String>());
            std::thread::sleep(Duration::from_secs(if status == 429 { 45 } else { 10 } * attempt));
        }
        bail!("the query service failed ten times ({last})")
    }

    /// A JSON answer to a GET of `url`: an HTTP error or what isn't JSON tried again, five times
    /// with growing pauses, then an error.
    pub fn get_json(&self, url: &str) -> Result<Py> {
        let mut last = String::new();
        for attempt in 1..=5u64 {
            match self.get_text(url, 60) {
                Ok((200, t, _)) => match loads(&t) {
                    Ok(v) => return Ok(v),
                    Err(e) => last = format!("{e:#}"),
                },
                Ok((s, t, retry)) => {
                    crate::budget::seen(url, s, retry.as_deref());
                    last = format!("HTTP {s}: {}", t.chars().take(200).collect::<String>());
                }
                Err(e) => last = format!("{e:#}"),
            }
            std::thread::sleep(Duration::from_secs(5 * attempt));
        }
        bail!("{url}: failed five times ({last})")
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
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(urlencode(&[("t", "A b|é")]), "t=A+b%7C%C3%A9");
    }
}
