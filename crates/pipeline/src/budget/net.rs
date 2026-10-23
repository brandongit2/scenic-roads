//! The internet as this Mac's jobs use it (docs/pool.md §10, Live; task #516): counted by the
//! agent's proxy, which every job's connection out goes through, so nothing is told by the jobs
//! but their retries and 429s (`/retry`, `/hold`). Per host: the bytes in (to the job) and out, the
//! requests asked (a tunnel is one: what's inside is the client's TLS), the connections opened,
//! the retries and the holds told. And the last `EVENTS` transfers, each as it ended.
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
    pub retries: AtomicU64,
    pub holds: AtomicU64,
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
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Net {
    /// `host`'s counts (made with its first use, by its name in lower case; a lock taken once a connection, not a chunk).
    pub fn host(&self, host: &str) -> Arc<Host> {
        self.hosts.lock().unwrap_or_else(|e| e.into_inner()).entry(host.to_ascii_lowercase()).or_default().clone()
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

    /// The bytes in and out of every host together (the live store's rates).
    pub fn totals(&self) -> (u64, u64) {
        let h = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        h.values()
            .fold((0, 0), |(i, o), c| (i + c.bytes_in.load(Ordering::Relaxed), o + c.bytes_out.load(Ordering::Relaxed)))
    }

    /// Each host's `field` summed by `pick` of its name (the status's counts by class).
    pub fn sum(&self, pick: impl Fn(&str) -> bool, field: impl Fn(&Host) -> &AtomicU64) -> u64 {
        let h = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        h.iter().filter(|(n, _)| pick(n)).map(|(_, c)| field(c).load(Ordering::Relaxed)).sum()
    }

    /// `/work/live`'s `net`: `{hosts: {host: {in, out, requests, conns, retries, holds}}, events:
    /// [{t, host, in, out, ms, outcome}, …]}`, the events oldest first.
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
                        "conns": l(&c.conns), "retries": l(&c.retries), "holds": l(&c.holds),
                    }),
                )
            })
            .collect();
        let events: Vec<Event> = self.events.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect();
        serde_json::json!({ "hosts": hosts, "events": events })
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
    fn a_hosts_counts_are_one_whatever_its_case() {
        let n = Net::default();
        n.host("A.example").bytes_in.fetch_add(3, Ordering::Relaxed);
        n.host("a.example").bytes_out.fetch_add(4, Ordering::Relaxed);
        assert_eq!(n.totals(), (3, 4));
        assert_eq!(n.view()["hosts"]["a.example"]["out"], 4);
    }
}
