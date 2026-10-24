//! The internet as this Mac's jobs use it (docs/pool.md §10, Live; task #516): counted by the
//! agent's proxy, which every job's connection out goes through, so nothing is told by the jobs
//! but their retries and 429s (`/retry`, `/hold`). Per host: the bytes in (to the job) and out, the
//! requests asked (a tunnel is one: what's inside is the client's TLS), the connections opened and
//! those open now, the retries and the holds told. And the last `EVENTS` transfers, each as it ended.
//!
//! And each job's bytes in and out (`Job`), by the tag its agent gave it in its proxy's address
//! (`http://<tag>:x@127.0.0.1:<port>`, which its clients send as `Proxy-Authorization`; a client that
//! sends none is counted to its host alone): counted afresh as each job begins (`job_began`).
//!
//! A transfer's bytes are added as each chunk is relayed (one atomic add a read), so a long
//! download's rate shows as it goes, not as it ends.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The transfers kept (the newest).
pub const EVENTS: usize = 200;

/// One host's counts since the proxy started.
#[derive(Default, Debug)]
pub struct Host {
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    pub requests: AtomicU64,
    pub conns: AtomicU64,
    /// The connections out open now (the proxy's `Open`).
    pub open: AtomicU64,
    pub retries: AtomicU64,
    pub holds: AtomicU64,
}

/// A job's bytes since it began (`Net::job_began`).
#[derive(Default, Debug)]
pub struct Job {
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
}

/// The credential a job's clients send for tag `tag` (`Proxy-Authorization: Basic <it>`): base64 of
/// `<tag>:x`.
pub fn credential(tag: &str) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = format!("{tag}:x").into_bytes();
    let mut out = String::new();
    for c in b.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            out.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// How a transfer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Relayed until both sides ended.
    Ok,
    /// Broken off (a side's error or its idle limit).
    Broken,
    /// The host couldn't be reached (answered 502).
    Unreached,
    /// No connection out to be had (answered 503).
    Refused,
}

/// A transfer, as it ended.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Event {
    /// When it ended (unix ms).
    pub t: u64,
    pub host: String,
    #[serde(rename = "in")]
    pub bytes_in: u64,
    #[serde(rename = "out")]
    pub bytes_out: u64,
    pub ms: u64,
    pub outcome: Outcome,
}

/// The counts (module doc).
#[derive(Default, Debug)]
pub struct Net {
    hosts: Mutex<BTreeMap<String, Arc<Host>>>,
    events: Mutex<VecDeque<Event>>,
    /// The jobs' bytes, by their clients' credential (`credential`).
    jobs: Mutex<BTreeMap<String, Arc<Job>>>,
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Net {
    /// `host`'s counts (made with its first use, by its name in lower case; a lock taken once a connection, not a chunk).
    pub fn host(&self, host: &str) -> Arc<Host> {
        self.hosts.lock().unwrap_or_else(|e| e.into_inner()).entry(host.to_ascii_lowercase()).or_default().clone()
    }

    /// The job tagged `tag` begins: its bytes counted from none (connections of the tag's job
    /// before still count to theirs).
    pub fn job_began(&self, tag: &str) {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(credential(tag), Arc::default());
    }

    /// The bytes in and out of the job tagged `tag` since it began; None: none began so.
    pub fn job(&self, tag: &str) -> Option<(u64, u64)> {
        let j = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        j.get(&credential(tag)).map(|c| (c.bytes_in.load(Ordering::Relaxed), c.bytes_out.load(Ordering::Relaxed)))
    }

    /// The job whose clients send `cred` (a `Proxy-Authorization`'s Basic credential); None: no
    /// job began with it.
    pub fn job_of(&self, cred: &str) -> Option<Arc<Job>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(cred).cloned()
    }

    /// A transfer of `host` that took `ms` ended as `outcome` (its bytes are already counted).
    pub fn ended(&self, host: &str, bytes: (u64, u64), ms: u64, outcome: Outcome) {
        let mut e = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if e.len() == EVENTS {
            e.pop_front();
        }
        e.push_back(Event {
            t: unix_ms(),
            host: host.to_ascii_lowercase(),
            bytes_in: bytes.0,
            bytes_out: bytes.1,
            ms,
            outcome,
        });
    }

    /// The bytes in and out of every host together, and the requests asked (the live store's rates).
    pub fn totals(&self) -> (u64, u64, u64) {
        let h = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        h.values().fold((0, 0, 0), |(i, o, r), c| (i + l(&c.bytes_in), o + l(&c.bytes_out), r + l(&c.requests)))
    }

    /// Each host's `field` summed by `pick` of its name (the status's counts by class).
    pub fn sum(&self, pick: impl Fn(&str) -> bool, field: impl Fn(&Host) -> &AtomicU64) -> u64 {
        let h = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        h.iter().filter(|(n, _)| pick(n)).map(|(_, c)| field(c).load(Ordering::Relaxed)).sum()
    }

    /// `/work/live`'s `net`: `{hosts: {host: {in, out, requests, conns, open, retries, holds, limit}},
    /// events: [{t, host, in, out, ms, outcome}, …]}`, the events oldest first; `limit`, the budget
    /// its connections are metered by (`limit_of`), null when none.
    pub fn view(&self) -> serde_json::Value {
        let hosts: serde_json::Map<String, serde_json::Value> = self
            .hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(n, c)| {
                let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
                (
                    n.clone(),
                    serde_json::json!({
                        "in": l(&c.bytes_in), "out": l(&c.bytes_out), "requests": l(&c.requests),
                        "conns": l(&c.conns), "open": l(&c.open), "retries": l(&c.retries), "holds": l(&c.holds),
                        "limit": limit_of(n),
                    }),
                )
            })
            .collect();
        let events: Vec<Event> = self.events.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
        serde_json::json!({ "hosts": hosts, "events": events })
    }
}

/// The budget `host`'s connections are metered by, in words, from its class's rate (the pool's
/// whole: crate::budget::Class::rate); None: not metered.
pub fn limit_of(host: &str) -> Option<String> {
    let c = super::class_of(host)?;
    let r = c.rate();
    let per_s = f64::from(r.tokens) * 1000.0 / r.window_ms as f64;
    let who = match c {
        super::Class::Wiki => "Wikimedia",
        super::Class::Query => "Wikidata queries",
    };
    let rate =
        if per_s >= 1.0 { format!("{} a second", fmt_n(per_s)) } else { format!("1 every {} s", fmt_n(1.0 / per_s)) };
    Some(if r.per_token_ms > 0 {
        format!("{who}: {rate}, {} s of query time each", fmt_n(r.per_token_ms as f64 / 1000.0))
    } else {
        format!("{who}: {rate}")
    })
}

fn fmt_n(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round())
    } else {
        format!("{v:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_newest_two_hundred() {
        let n = Net::default();
        for i in 0..(EVENTS as u64 + 50) {
            n.ended("a.example", (i, 0), 1, Outcome::Ok);
        }
        let v = n.view();
        let ev = v["events"].as_array().unwrap();
        assert_eq!(ev.len(), EVENTS);
        assert_eq!(ev[0]["in"], 50);
        assert_eq!(ev[EVENTS - 1]["in"], EVENTS as u64 + 49);
        assert_eq!(ev[0]["outcome"], "ok");
    }

    #[test]
    fn a_jobs_credential_is_its_tags_base64() {
        assert_eq!(credential("job1"), "am9iMTp4");
        assert_eq!(credential("job12"), "am9iMTI6eA==");
        assert_eq!(credential("job123"), "am9iMTIzOng=");
        let n = Net::default();
        assert!(n.job("job1").is_none() && n.job_of(&credential("job1")).is_none());
        n.job_began("job1");
        n.job_of(&credential("job1")).unwrap().bytes_in.fetch_add(5, Ordering::Relaxed);
        assert_eq!(n.job("job1"), Some((5, 0)));
        n.job_began("job1");
        assert_eq!(n.job("job1"), Some((0, 0)));
    }

    #[test]
    fn a_hosts_counts_are_one_whatever_its_case() {
        let n = Net::default();
        n.host("A.example").bytes_in.fetch_add(3, Ordering::Relaxed);
        n.host("a.example").bytes_out.fetch_add(4, Ordering::Relaxed);
        assert_eq!(n.totals(), (3, 4, 0));
        assert_eq!(n.view()["hosts"]["a.example"]["out"], 4);
        assert_eq!(n.view()["hosts"]["a.example"]["limit"], serde_json::Value::Null);
    }

    #[test]
    fn a_hosts_limit_is_its_budgets_rate() {
        assert_eq!(limit_of("www.wikidata.org").as_deref(), Some("Wikimedia: 4 a second"));
        assert_eq!(
            limit_of("query.wikidata.org").as_deref(),
            Some("Wikidata queries: 1 every 2 s, 2 s of query time each")
        );
        assert_eq!(limit_of("s3.amazonaws.com"), None);
    }
}
