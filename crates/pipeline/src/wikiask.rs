//! Asking Wikidata's query service and Wikipedia's API, for the heritage chain's Rust steps
//! (crate::heritagewd): the build's User-Agent (crate::fetch::USER_AGENT), every request refused
//! offline first (crate::fetch::online), through the job's proxy (`HTTPS_PROXY`, which takes each
//! from the pool's budget by its rate class: crate::budget), a 429 or a `Retry-After` told to it
//! (crate::budget::seen), and a host's robots.txt read before its first request: a path it
//! disallows to us is never asked (an unreadable robots.txt, or none, allows everything). Answers
//! are read as Python's `json` reads them (crate::pyjson), for caches whose bytes stay the Python's.

use crate::pyjson::{loads, Py};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::sync::Mutex;
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

/// What asks: one agent (its connections kept), each host's robots.txt rules once read.
pub struct Asker {
    agent: ureq::Agent,
    robots: Mutex<HashMap<String, Robots>>,
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
            .build()
            .into();
        Asker { agent, robots: Mutex::new(HashMap::new()) }
    }

    /// Whether `url` may be asked: offline refused (an error), a path its host's robots.txt
    /// disallows to us false.
    pub fn may(&self, url: &str) -> Result<bool> {
        crate::fetch::online(url)?;
        let (origin, path) = split_url(url).with_context(|| format!("not an http(s) URL: {url}"))?;
        let mut m = self.robots.lock().unwrap();
        if !m.contains_key(&origin) {
            let r = self.get_text(&format!("{origin}/robots.txt"), 60).ok().filter(|(s, _, _)| *s == 200);
            m.insert(origin.clone(), r.map(|(_, t, _)| Robots::parse(&t, "scenic-roads")).unwrap_or_default());
        }
        Ok(m[&origin].allows(&path))
    }

    /// GET: the status, the body, and a `Retry-After` if any.
    fn get_text(&self, url: &str, timeout_s: u64) -> Result<(u16, String, Option<String>)> {
        crate::fetch::online(url)?;
        let r = self.agent.get(url).config().timeout_global(Some(Duration::from_secs(timeout_s))).build().call();
        answer(r)
    }

    /// A SPARQL query (`PREFIXES` before it) POSTed to `endpoint`: its result's bindings. Errors
    /// (no answer, not a result) are tried again with growing pauses, longer when rate-limited
    /// (429), ten times in all.
    pub fn sparql(&self, endpoint: &str, q: &str) -> Result<Vec<Py>> {
        anyhow::ensure!(self.may(endpoint)?, "{endpoint}: its robots.txt disallows us");
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
                    return Ok(b);
                }
            }
            crate::budget::seen(endpoint, status, retry.as_deref());
            last = format!("HTTP {status}: {}", text.chars().take(300).collect::<String>());
            std::thread::sleep(Duration::from_secs(if status == 429 { 45 } else { 10 } * attempt));
        }
        bail!("the query service failed ten times ({last})")
    }

    /// A JSON answer to a GET of `url` (None when its host's robots.txt disallows it): an HTTP
    /// error or what isn't JSON tried again, five times with growing pauses, then an error.
    pub fn get_json(&self, url: &str) -> Result<Option<Py>> {
        if !self.may(url)? {
            return Ok(None);
        }
        let mut last = String::new();
        for attempt in 1..=5u64 {
            match self.get_text(url, 60) {
                Ok((200, t, _)) => match loads(&t) {
                    Ok(v) => return Ok(Some(v)),
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

/// `scheme://host[:port]` and the path with its query (`/` when none).
fn split_url(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let cut = rest.find(['/', '?']).unwrap_or(rest.len());
    let path = &rest[cut..];
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    Some((format!("{scheme}://{}", &rest[..cut]), path))
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

/// A robots.txt's rules for one agent: its own group's (by its product token), else `*`'s.
#[derive(Clone, Debug, Default)]
struct Robots {
    /// (allow, pattern).
    rules: Vec<(bool, String)>,
}

impl Robots {
    fn parse(text: &str, token: &str) -> Robots {
        let token = token.to_ascii_lowercase();
        // Groups: their agents, their rules.
        let mut groups: Vec<(Vec<String>, Vec<(bool, String)>)> = Vec::new();
        let mut in_agents = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once(':') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match k.as_str() {
                "user-agent" => {
                    if !in_agents {
                        groups.push((vec![], vec![]));
                        in_agents = true;
                    }
                    groups.last_mut().unwrap().0.push(v.to_ascii_lowercase());
                }
                "allow" | "disallow" => {
                    in_agents = false;
                    if let Some(g) = groups.last_mut() {
                        if !v.is_empty() {
                            g.1.push((k == "allow", v));
                        }
                    }
                }
                _ => in_agents = false,
            }
        }
        let pick = |want: &dyn Fn(&str) -> bool| -> Option<Vec<(bool, String)>> {
            let rules: Vec<_> =
                groups.iter().filter(|g| g.0.iter().any(|a| want(a))).flat_map(|g| g.1.clone()).collect();
            groups.iter().any(|g| g.0.iter().any(|a| want(a))).then_some(rules)
        };
        let rules = pick(&|a| a != "*" && token.contains(a)).or_else(|| pick(&|a| a == "*")).unwrap_or_default();
        Robots { rules }
    }

    /// Whether `path` may be asked: the longest matching rule decides, an allow winning a tie.
    fn allows(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (allow, pat) in &self.rules {
            if matches(pat, path) {
                let n = pat.len();
                if best.is_none_or(|(bn, ba)| n > bn || (n == bn && *allow && !ba)) {
                    best = Some((n, *allow));
                }
            }
        }
        best.is_none_or(|(_, a)| a)
    }
}

/// A robots.txt pattern against a path: a prefix, `*` any run, `$` its end.
fn matches(pat: &str, path: &str) -> bool {
    let (pat, end) = match pat.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pat, false),
    };
    let parts: Vec<&str> = pat.split('*').collect();
    let mut at = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            at = part.len();
        } else if i == parts.len() - 1 && end {
            return path.len() >= at + part.len() && path.ends_with(part);
        } else {
            match path[at..].find(part) {
                Some(k) => at += k + part.len(),
                None => return false,
            }
        }
    }
    !end || at == path.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_rules() {
        let r = Robots::parse(
            "User-agent: Googlebot\nDisallow: /\n\nUser-agent: *\nAllow: /w/load.php?\nDisallow: /w/\nDisallow: /x$\n",
            "scenic-roads",
        );
        assert!(r.allows("/wiki/A"));
        assert!(!r.allows("/w/api.php?action=query"));
        assert!(r.allows("/w/load.php?x=1"));
        assert!(!r.allows("/x"));
        assert!(r.allows("/xy"));
        let own =
            Robots::parse("User-agent: *\nDisallow: /\n\nUser-agent: scenic-roads\nDisallow: /p*q\n", "scenic-roads");
        assert!(own.allows("/a"));
        assert!(!own.allows("/p1q"));
        assert!(Robots::parse("", "scenic-roads").allows("/"));
    }

    #[test]
    fn urls() {
        assert_eq!(split_url("https://a.org/w/api.php?x=1"), Some(("https://a.org".into(), "/w/api.php?x=1".into())));
        assert_eq!(split_url("https://a.org"), Some(("https://a.org".into(), "/".into())));
        assert_eq!(urlencode(&[("t", "A b|é")]), "t=A+b%7C%C3%A9");
    }
}
