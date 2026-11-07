//! Tasks brokered by every member (docs/pool.md §8, §9; §12 phase 4's tenth batch): what a
//! coordinator knows of the pool's brokers, told by its agent from the members' heartbeats
//! (crate::agent::brokers): which pages may call it from elsewhere (CORS: the members' own
//! addresses, `origins`; a member's page reads another's live charts and settings straight). None
//! of it while the switch is off (`Shared::brokers` None: the coordinator as it was).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A member's broker, as its heartbeat says it (crate::agent::pool::Heartbeat's `broker`): where a
/// page reaches it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Broker {
    /// Its coordinator's addresses over HTTP (Tailscale's, then the LAN's).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// Its address over HTTPS, when its owner has `tailscale serve` proxy it ("https://<name>/").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub https: Option<String>,
    /// Its LAN addresses (crate::net::lan_urls): where a device on the LAN may have opened its page
    /// by address. Origins of its pages alone, never contacted (any device's on the network to
    /// claim).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lan: Vec<String>,
    /// The other origins its page is served from: a proxy on it under a public name
    /// (crate::net::names; "https://map.brandontsang.net" on the M6), whose pages may call the
    /// members too. Never a broker's address: the proxy serves the page under a path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<String>,
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
pub fn tailnet(o: &str) -> bool {
    let host = o.split_once("://").map_or(o, |(_, h)| h);
    let name = host.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(n, _)| n);
    name.ends_with(".ts.net")
        || name
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

/// The origins of a broker's pages: every address its page is served at (docs/roles.md §3.3, the
/// owner's rule: the page works from any device), so a member allows the lead's page however it was
/// opened. Its addresses (the tailnet's, its LAN name: a member's port sends a LAN device there,
/// crate::coord::memberpage::location), its HTTPS page (`tailscale serve`), the tailnet's names for
/// it over HTTP (MagicDNS: the HTTPS page's name, full and short, at each address's port), its LAN
/// addresses (`lan`: a device on the LAN typing one), and its `pages` (a proxy's public names).
pub fn origins_of(b: &Broker) -> BTreeSet<String> {
    let mut out: BTreeSet<String> =
        b.urls.iter().chain(b.lan.iter()).chain(b.https.iter()).filter_map(|u| origin(u)).collect();
    let ports: BTreeSet<String> = out
        .iter()
        .filter(|o| o.starts_with("http://"))
        .filter_map(|o| {
            o.rsplit_once(':').map(|(_, p)| p.to_string()).filter(|p| p.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    let ts = b.https.as_deref().and_then(origin).and_then(|o| o.strip_prefix("https://").map(str::to_string));
    if let Some(name) = ts.filter(|n| n.ends_with(".ts.net")) {
        let short = name.split('.').next().unwrap_or(&name).to_string();
        for p in &ports {
            out.insert(format!("http://{name}:{p}"));
            out.insert(format!("http://{short}:{p}"));
        }
    }
    out.extend(b.pages.iter().filter_map(|u| page_origin(u)));
    out
}

/// Whether `ip` is one of a member's addresses (`origins`, `origins_of`'s): a request from that Mac
/// itself, so a page of its own loopback (localhost: the lead's page opened on the lead) is the
/// member's page, as it is the lead's own there (crate::net::from_the_page).
pub fn member_ip(origins: &BTreeSet<String>, ip: std::net::IpAddr) -> bool {
    let ip = match ip {
        std::net::IpAddr::V6(v) => v.to_ipv4_mapped().map_or(std::net::IpAddr::V6(v), std::net::IpAddr::V4),
        v4 => v4,
    };
    let at = format!("http://{ip}:");
    !ip.is_loopback() && origins.iter().any(|o| o.starts_with(&at))
}

/// A proxy's page origin (`Broker::pages`) when it is "https://<host>" alone, lowercased: no port,
/// path, query or user; anything else none.
fn page_origin(u: &str) -> Option<String> {
    let host = u.strip_prefix("https://").or_else(|| u.strip_prefix("HTTPS://"))?;
    let host = host.strip_suffix('/').unwrap_or(host);
    (!host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
        .then(|| format!("https://{}", host.to_ascii_lowercase()))
}

/// The CORS headers' values (docs/pool.md §8): the methods and headers a member's page may send.
pub const ALLOW_METHODS: &str = "GET, POST";
pub const ALLOW_HEADERS: &str = "Content-Type";
/// How long a browser may keep a preflight's answer (s).
pub const MAX_AGE_S: u32 = 600;

#[cfg(test)]
mod tests {
    use super::*;

    fn broker(urls: &[&str], https: Option<&str>) -> Broker {
        Broker {
            urls: urls.iter().map(|u| u.to_string()).collect(),
            https: https.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn origins_are_every_address_a_members_page_is_served_at() {
        assert_eq!(origin("http://100.70.85.80:8090"), Some("http://100.70.85.80:8090".into()));
        assert_eq!(origin("https://Mac.tail1.ts.net/work/"), Some("https://mac.tail1.ts.net".into()));
        assert_eq!(origin("ftp://x"), None);
        assert_eq!(origin("https:///x"), None);
        let own = Broker {
            lan: vec!["http://192.168.1.20:8090".into()],
            ..broker(&["http://100.70.85.80:8090", "http://m4.local:8090"], Some("https://m4.tail1.ts.net/"))
        };
        let m1 = Peer {
            member: "m-1".into(),
            host: "m1".into(),
            lead: false,
            broker: Some(broker(&["http://100.93.150.29:8090"], None)),
        };
        let old = Peer { member: "m-2".into(), host: "old".into(), lead: false, broker: None };
        let b = Brokers::new(vec![m1, old], &own);
        // Every address a page is served at: the tailnet's, the LAN's name (where a member sends a
        // LAN device), its LAN address, the HTTPS page, and MagicDNS's names for it over HTTP.
        let want: BTreeSet<String> = [
            "http://100.70.85.80:8090",
            "http://m4.local:8090",
            "http://192.168.1.20:8090",
            "https://m4.tail1.ts.net",
            "http://m4.tail1.ts.net:8090",
            "http://m4:8090",
            "http://100.93.150.29:8090",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(b.origins, want);
        assert_eq!(origins_of(&broker(&["http://m4.local:8090"], None)), ["http://m4.local:8090".to_string()].into());
        // A foreign page: none of them.
        for o in [
            "https://evil.example",
            "http://m4.local:8091",
            "http://evil.local:8090",
            "http://100.70.85.81:8090",
            "http://192.168.1.21:8090",
            "http://evil:8090",
            "http://localhost:8090", // pool port: not bound
        ] {
            assert!(!b.origins.contains(o), "{o}");
        }
        // A request from a member's Mac itself (its tailnet or LAN address): its loopback page is
        // the member's; from any other address, or a loopback one (this Mac's own), not.
        for (ip, member) in [
            ("100.93.150.29", true),
            ("192.168.1.20", true),
            ("::ffff:100.93.150.29", true),
            ("100.93.150.30", false),
            ("192.168.1.2", false),
            ("127.0.0.1", false),
        ] {
            assert_eq!(member_ip(&b.origins, ip.parse().unwrap()), member, "{ip}");
        }
        // An HTTPS name not the tailnet's: no HTTP names made from it.
        let other = broker(&["http://100.1.1.1:8090"], Some("https://x.example.net/"));
        assert_eq!(origins_of(&other).len(), 2);
        // A proxy's public name for its page: an origin of its pages, never a broker's address.
        let named =
            Broker { pages: vec!["https://Map.Example.net".into()], ..broker(&["http://100.1.1.1:8090"], None) };
        assert!(origins_of(&named).contains("https://map.example.net"));
        // Only "https://<host>": no other scheme, port, path, query or user.
        for bad in [
            "http://map.example.net",
            "https://map.example.net:8443",
            "https://map.example.net/build",
            "https://u@map.example.net",
            "https://map.example.net?x",
            "https://",
        ] {
            let b = Broker { pages: vec![bad.into()], ..Default::default() };
            assert!(origins_of(&b).is_empty(), "{bad}");
        }
    }
}
