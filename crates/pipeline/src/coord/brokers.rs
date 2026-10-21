//! Tasks brokered by every member (docs/pool.md §8, §9; §12 phase 4's tenth batch): what a
//! coordinator knows of the pool's brokers, told by its agent from the members' heartbeats
//! (crate::agent::brokers), and answers a page with: where work that fits it waits (`/work/where`),
//! and which pages may call it from elsewhere (CORS: the members' own addresses, `origins`).
//! None of it while the switch is off (`Shared::brokers` None: the coordinator as it was).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The tasks of a kind waiting at a broker: how many, and the least memory one takes (MB), so a
/// page that spares less knows none fits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    pub n: u32,
    pub mb: u64,
}

/// A member's broker, as its heartbeat says it (crate::agent::pool::Heartbeat's `broker`): where a
/// page reaches it, and the tasks its jobs offer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Broker {
    /// Its coordinator's addresses over HTTP (Tailscale's, then the LAN's).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// Its address over HTTPS, when its owner has `tailscale serve` proxy it ("https://<name>/").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub https: Option<String>,
    /// The tasks waiting for a worker, by kind.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub waiting: BTreeMap<String, Waiting>,
    /// The tasks a worker holds now.
    #[serde(skip_serializing_if = "is_zero")]
    pub out: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A member of the pool, as a coordinator is told it: its id and host, whether it leads, and its
/// broker (none: its heartbeat says none, an app before the brokers or the switch off there).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub member: String,
    pub host: String,
    pub lead: bool,
    pub broker: Option<Broker>,
}

/// What a coordinator knows of the brokers (`Coordinator::set_brokers`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Brokers {
    /// The members in touch, this Mac's own among them.
    pub peers: Vec<Peer>,
    /// The origins whose pages may call this coordinator (`origins`).
    pub origins: BTreeSet<String>,
}

impl Brokers {
    /// From the members in touch, `own` this coordinator's own addresses (its `urls` and https).
    pub fn new(peers: Vec<Peer>, own: &Broker) -> Brokers {
        let mut origins = origins_of(own);
        for p in &peers {
            if let Some(b) = &p.broker {
                origins.extend(origins_of(b));
            }
        }
        Brokers { peers, origins }
    }
}

/// A URL's origin ("scheme://host[:port]", lowercased), when it's one of HTTP or HTTPS.
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{}", host.to_ascii_lowercase()))
}

/// Whether an origin is a tailnet address's (100.64.0.0/10, or a name under .ts.net).
fn tailnet(o: &str) -> bool {
    let host = o.split_once("://").map_or(o, |(_, h)| h);
    let name = host.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(n, _)| n);
    name.ends_with(".ts.net")
        || name
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

/// The origins of a broker's pages: each of its addresses, but its LAN names (`.local`) when it has
/// a tailnet address (a page calls it there; a LAN name is any device's on the network to claim).
pub fn origins_of(b: &Broker) -> BTreeSet<String> {
    let all: BTreeSet<String> = b.urls.iter().chain(b.https.iter()).filter_map(|u| origin(u)).collect();
    if all.iter().any(|o| tailnet(o)) {
        all.into_iter().filter(|o| !o.ends_with(".local") && !o.contains(".local:")).collect()
    } else {
        all
    }
}

/// A broker's base address for a page: over HTTPS when the page is (a secure page can't call a
/// plain one), else its first over HTTP; None when it has none such.
pub fn base(b: &Broker, https: bool) -> Option<String> {
    if https {
        b.https.as_deref().and_then(origin)
    } else {
        b.urls.first().and_then(|u| origin(u)).or_else(|| b.https.as_deref().and_then(origin))
    }
}

/// How many tasks wait at `b` that a worker doing `can`, sparing `mem_mb`, could take: of its
/// kinds, those whose least fits.
pub fn fitting(b: &Broker, can: &[String], mem_mb: u64) -> u32 {
    b.waiting.iter().filter(|(k, w)| can.contains(k) && w.mb <= mem_mb).map(|(_, w)| w.n).sum()
}

/// What a page asks of `/work/where`: what it does, and what it spares; whether it's a secure
/// page (only HTTPS addresses then).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ask {
    pub can: Vec<String>,
    pub mem_mb: u64,
    pub https: bool,
}

/// The answer (`/work/where`): the brokers to ask, most waiting first (by host after); the lead's
/// address (its page: the build at a glance, pausing, the lead's asks); and every member's (a page
/// whose own member goes away reloads from the next that answers): addresses and counts alone, no
/// member's id or host (a page needs none: it names a member by its address).
pub fn answer(b: &Brokers, a: &Ask) -> serde_json::Value {
    let mut asks: Vec<(u32, &Peer, String)> = b
        .peers
        .iter()
        .filter_map(|p| {
            let br = p.broker.as_ref()?;
            let n = fitting(br, &a.can, a.mem_mb);
            Some((n, p, base(br, a.https)?)).filter(|_| n > 0)
        })
        .collect();
    asks.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.host.cmp(&y.1.host)));
    let brokers: Vec<serde_json::Value> =
        asks.iter().map(|(n, _, base)| serde_json::json!({ "base": base, "waiting": n })).collect();
    let members: Vec<String> = b.peers.iter().filter_map(|p| base(p.broker.as_ref()?, a.https)).collect();
    let lead = b.peers.iter().find(|p| p.lead).and_then(|p| base(p.broker.as_ref()?, a.https));
    serde_json::json!({ "brokers": brokers, "members": members, "lead": lead })
}

/// The CORS headers' values (docs/pool.md §8): the methods and headers a member's page may send.
pub const ALLOW_METHODS: &str = "GET, POST, PUT";
pub const ALLOW_HEADERS: &str = "Authorization, X-Worker, Range, Content-Type";
/// How long a browser may keep a preflight's answer (s).
pub const MAX_AGE_S: u32 = 600;

#[cfg(test)]
mod tests {
    use super::*;

    fn broker(urls: &[&str], https: Option<&str>, waiting: &[(&str, u32, u64)]) -> Broker {
        Broker {
            urls: urls.iter().map(|u| u.to_string()).collect(),
            https: https.map(str::to_string),
            waiting: waiting.iter().map(|(k, n, mb)| (k.to_string(), Waiting { n: *n, mb: *mb })).collect(),
            out: 0,
        }
    }

    #[test]
    fn origins_are_the_members_addresses() {
        assert_eq!(origin("http://100.70.85.80:8090"), Some("http://100.70.85.80:8090".into()));
        assert_eq!(origin("https://Mac.tail1.ts.net/work/"), Some("https://mac.tail1.ts.net".into()));
        assert_eq!(origin("ftp://x"), None);
        assert_eq!(origin("https:///x"), None);
        let own = broker(&["http://100.70.85.80:8090", "http://m4.local:8090"], Some("https://m4.tail1.ts.net/"), &[]);
        let m1 = Peer {
            member: "m-1".into(),
            host: "m1".into(),
            lead: false,
            broker: Some(broker(&["http://100.93.150.29:8090"], None, &[])),
        };
        let old = Peer { member: "m-2".into(), host: "old".into(), lead: false, broker: None };
        let b = Brokers::new(vec![m1, old], &own);
        // (The LAN's name left out: a tailnet address is there.)
        let want: BTreeSet<String> =
            ["http://100.70.85.80:8090", "https://m4.tail1.ts.net", "http://100.93.150.29:8090"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        assert_eq!(b.origins, want);
        // With no tailnet address, the LAN's name is the page's.
        assert_eq!(
            origins_of(&broker(&["http://m4.local:8090"], None, &[])),
            ["http://m4.local:8090".to_string()].into()
        );
    }

    #[test]
    fn where_answers_the_brokers_with_work_that_fits_most_waiting_first() {
        let can: Vec<String> = vec!["tail".into(), "bldtile".into()];
        let peers = vec![
            Peer {
                member: "m-a".into(),
                host: "a".into(),
                lead: true,
                broker: Some(broker(&["http://100.1.1.1:8090"], Some("https://a.ts.net/"), &[("tail", 2, 800)])),
            },
            Peer {
                member: "m-b".into(),
                host: "b".into(),
                lead: false,
                broker: Some(broker(&["http://100.1.1.2:8090"], None, &[("tail", 1, 600), ("bldtile", 4, 900)])),
            },
            // (Too large for the page, and a kind it doesn't do.)
            Peer {
                member: "m-c".into(),
                host: "c".into(),
                lead: false,
                broker: Some(broker(
                    &["http://100.1.1.3:8090"],
                    Some("https://c.ts.net/"),
                    &[("tail", 9, 5000), ("terrainsub", 3, 100)],
                )),
            },
            Peer { member: "m-d".into(), host: "d".into(), lead: false, broker: None },
        ];
        let b = Brokers::new(peers, &Broker::default());
        let v = answer(&b, &Ask { can: can.clone(), mem_mb: 1000, https: false });
        let order: Vec<&str> = v["brokers"].as_array().unwrap().iter().map(|x| x["base"].as_str().unwrap()).collect();
        assert_eq!(order, ["http://100.1.1.2:8090", "http://100.1.1.1:8090"]);
        assert!(!v.to_string().contains("m-a") && !v.to_string().contains("\"host\""), "no member's id or host: {v}");
        assert_eq!(v["brokers"][0]["waiting"], 5);
        assert_eq!(v["brokers"][0]["base"], "http://100.1.1.2:8090");
        assert_eq!(v["lead"], "http://100.1.1.1:8090");
        assert_eq!(v["members"].as_array().unwrap().len(), 3, "every member with a broker");
        // A secure page: HTTPS addresses alone (b has none: left out).
        let v = answer(&b, &Ask { can, mem_mb: 1000, https: true });
        let order: Vec<&str> = v["brokers"].as_array().unwrap().iter().map(|x| x["base"].as_str().unwrap()).collect();
        assert_eq!(order, ["https://a.ts.net"]);
        assert_eq!(v["lead"], "https://a.ts.net");
    }
}
