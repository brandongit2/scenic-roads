//! What this Mac's services share about the network: who may connect (this Mac, its LAN, the
//! tailnet, also through a proxy on this Mac), which requests are this Mac's own and where the
//! others came from, what a request must name to be one of theirs (never a web page elsewhere's),
//! the addresses others reach it by, whether `tailscale serve` proxies a port over HTTPS, and the
//! tokens a service keeps (made once, readable by its owner alone). The build Mac's coordinator
//! (crate::coord) and the map's server (crates/server, docs/plan.md §4) use them.

use anyhow::{Context, Result};
use axum::http::{header, HeaderMap};
use std::net::IpAddr;
use std::path::Path;
use std::time::{Duration, Instant};

/// A random token: 128 bits, hex.
pub fn random() -> Result<String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).context("read /dev/urandom")?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// The token kept at `p`: made once and kept (0600), so what holds it keeps it across restarts.
pub fn kept_token(p: &Path) -> Result<String> {
    if let Ok(t) = std::fs::read_to_string(p) {
        if t.trim().len() == 32 {
            return Ok(t.trim().to_string());
        }
    }
    let t = random()?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    crate::whole::write(p, t.as_bytes())?;
    if let Ok(f) = std::fs::File::open(p) {
        store::sys::set_mode(&f, 0o600).ok();
    }
    Ok(t)
}

/// Whether `ip` may connect: this Mac, its LAN, the tailnet (100.64.0.0/10, fd7a:115c:a1e0::/48).
pub fn allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])),
        IpAddr::V6(v) => match v.to_ipv4_mapped() {
            Some(v4) => allowed(IpAddr::V4(v4)),
            None => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
        },
    }
}

/// Whether `ip` is this Mac's own (loopback).
pub fn loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback() || v.to_ipv4_mapped().is_some_and(|v| v.is_loopback()),
    }
}

/// The headers a proxy on this Mac adds to a request it hands over (`tailscale serve` sets
/// X-Forwarded-For, and Tailscale-User-Login for a tailnet user's).
const PROXIED: [&str; 6] = ["x-forwarded-for", "x-forwarded-host", "x-forwarded-proto", "x-real-ip", "forwarded", "tailscale-user-login"];

/// Whether a request is this Mac's own: from loopback, and not handed over by a proxy on this Mac
/// (any header a proxy adds).
pub fn own(peer: IpAddr, h: &HeaderMap) -> bool {
    loopback(peer) && !PROXIED.iter().any(|k| h.contains_key(*k))
}

/// Where a request came from: its address; handed over by a proxy on this Mac, the address the
/// proxy took it from (X-Forwarded-For's last entry, the one the proxy added: a device may send
/// some of its own before it), else "a proxy on this Mac" (none, or not an address).
pub fn source(peer: IpAddr, h: &HeaderMap) -> String {
    if !loopback(peer) || own(peer, h) {
        return peer.to_string();
    }
    let proxy_said = h.get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).next_back().and_then(|a| a.trim().parse::<IpAddr>().ok());
    proxy_said.map_or_else(|| "a proxy on this Mac".to_string(), |ip| ip.to_string())
}

/// Whether a request may be answered at all: from this Mac, its LAN or the tailnet; handed over by
/// a proxy on this Mac (`tailscale serve`), from those alone, never the internet's (Tailscale
/// Funnel's, or an address that isn't one of those).
pub fn reached(peer: IpAddr, h: &HeaderMap) -> bool {
    if !allowed(peer) {
        return false;
    }
    if !loopback(peer) || own(peer, h) {
        return true;
    }
    !h.contains_key("tailscale-funnel-request") && source(peer, h).parse().is_ok_and(allowed)
}

/// Whether a host (a `Host` header's, an `Origin`'s; a port after it is ignored) names this Mac: an
/// IP address, localhost or a name under it, or a name only a tailnet or a local network resolves
/// (one label, or under .local, .home, .lan, .internal, .ts.net). Never a public name, which a web
/// page elsewhere could point at this Mac (DNS rebinding).
pub fn ours(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    let name = match h.strip_prefix('[') {
        Some(v6) => return v6.split(']').next().is_some_and(|a| a.parse::<std::net::Ipv6Addr>().is_ok()),
        None => h.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(h.as_str(), |(n, _)| n),
    };
    let name = name.trim_end_matches('.');
    !name.is_empty()
        && (name.parse::<IpAddr>().is_ok()
            || name == "localhost"
            || !name.contains('.')
            || [".localhost", ".local", ".home", ".lan", ".internal", ".ts.net"].iter().any(|s| name.ends_with(s)))
}

/// Whether a page's origin is one of this Mac's own (localhost, a name under it, a loopback
/// address): the app's pages here, which spread their downloads over roads.localhost and the like.
pub fn local_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")) else { return false };
    let h = rest.to_ascii_lowercase();
    let name = match h.strip_prefix('[') {
        Some(v6) => return v6.split(']').next().is_some_and(|a| a.parse::<std::net::Ipv6Addr>().is_ok_and(|ip| ip.is_loopback())),
        None => h.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(h.as_str(), |(n, _)| n),
    };
    name == "localhost" || name.ends_with(".localhost") || name.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether a request's page, if it says one (`Origin`), is the service's own: this Mac's, or the
/// address the request itself names (the service's page on a device), as it came or as a proxy on
/// this Mac says it came (`X-Forwarded-Host`: a page can't set that without a preflight, which is
/// answered for this Mac's own pages alone).
pub fn from_the_page(h: &HeaderMap) -> bool {
    let Some(o) = h.get(header::ORIGIN) else { return true };
    let Ok(o) = o.to_str() else { return false };
    let named = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).is_some_and(|host| o.split_once("://").is_some_and(|(_, a)| a.eq_ignore_ascii_case(host)));
    local_origin(o) || named("host") || named("x-forwarded-host")
}

/// Whether `a` is `b`, compared in a time that says nothing of where they differ (a key's check).
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// This Mac's addresses for others: its Tailscale address (100.64.0.0/10), then its LAN name.
pub fn urls(port: u16) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(o) = std::process::Command::new("/sbin/ifconfig").output() {
        for w in String::from_utf8_lossy(&o.stdout).split_whitespace().collect::<Vec<_>>().windows(2) {
            let ["inet", a] = w else { continue };
            let Ok(ip) = a.parse::<std::net::Ipv4Addr>() else { continue };
            let url = format!("http://{ip}:{port}");
            if ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]) && !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out.push(format!("http://{}.local:{port}", crate::agent::cond::host_name()));
    out
}

/// `port`'s address over HTTPS, when `tailscale serve` proxies it ("https://<this Mac's tailnet
/// name>/<path>").
pub fn served_https(port: u16) -> Option<String> {
    let cli = ["/opt/homebrew/bin/tailscale", "/usr/local/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"].into_iter().find(|p| Path::new(p).exists())?;
    // (Bounded: a stuck daemon mustn't hold up whoever asks.)
    let mut child = std::process::Command::new(cli).args(["serve", "status", "--json"]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let t = Instant::now();
    while child.try_wait().ok()?.is_none() {
        if t.elapsed() > Duration::from_secs(3) {
            child.kill().ok();
            child.wait().ok();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut child.stdout.take()?, &mut out).ok()?;
    https_in(&serde_json::from_slice(&out).ok()?, port)
}

/// `served_https` from what `tailscale serve status --json` says.
pub fn https_in(v: &serde_json::Value, port: u16) -> Option<String> {
    for (host, site) in v["Web"].as_object()? {
        // (At the root alone: the pages ask for /api/…, /work/…, not under a path a proxy adds.)
        let Some(h) = site["Handlers"].get("/") else { continue };
        let proxy = h["Proxy"].as_str().unwrap_or("");
        if proxy.ends_with(&format!(":{port}")) || proxy.ends_with(&format!(":{port}/")) {
            let host = host.strip_suffix(":443").unwrap_or(host);
            return Some(format!("https://{host}/"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_may_connect() {
        for ok in ["127.0.0.1", "::1", "100.70.85.80", "192.168.1.20", "10.0.0.3", "fd7a:115c:a1e0::1", "::ffff:100.100.1.1"] {
            assert!(allowed(ok.parse().unwrap()), "{ok}");
        }
        for no in ["8.8.8.8", "100.128.0.1", "2001:db8::1"] {
            assert!(!allowed(no.parse().unwrap()), "{no}");
        }
        assert!(loopback("::ffff:127.0.0.1".parse().unwrap()) && !loopback("100.70.85.80".parse().unwrap()));
    }

    #[test]
    fn this_macs_own_and_another_devices_and_where_they_came_from() {
        use axum::http::HeaderValue;
        let mut h = HeaderMap::new();
        assert!(own("127.0.0.1".parse().unwrap(), &h) && own("::1".parse().unwrap(), &h));
        assert!(!own("100.70.85.80".parse().unwrap(), &h));
        assert_eq!(source("100.70.85.80".parse().unwrap(), &h), "100.70.85.80");
        // Handed over by tailscale serve: a device's, from the address the proxy added last.
        h.insert("x-forwarded-for", HeaderValue::from_static("10.9.9.9, 100.101.1.2"));
        assert!(!own("127.0.0.1".parse().unwrap(), &h));
        assert_eq!(source("127.0.0.1".parse().unwrap(), &h), "100.101.1.2");
        // (A device's own X-Forwarded-For, sent straight here, says nothing.)
        assert_eq!(source("192.168.1.20".parse().unwrap(), &h), "192.168.1.20");
        // By any proxy saying it is one.
        let mut x = HeaderMap::new();
        x.insert("tailscale-user-login", HeaderValue::from_static("someone@example.com"));
        assert!(!own("127.0.0.1".parse().unwrap(), &x));
        assert_eq!(source("127.0.0.1".parse().unwrap(), &x), "a proxy on this Mac");
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
        // (The proxy's own entry, strictly: one that isn't an address is no address.)
        let mut p = HeaderMap::new();
        p.insert("x-forwarded-for", HeaderValue::from_static("10.9.9.9, 100.64.0.9:5555"));
        assert_eq!(source("127.0.0.1".parse().unwrap(), &p), "a proxy on this Mac");
        // Reached: from here, the LAN, the tailnet, and through the proxy from those alone.
        let none = HeaderMap::new();
        assert!(reached("127.0.0.1".parse().unwrap(), &none) && reached("192.168.1.20".parse().unwrap(), &none) && reached("100.70.85.80".parse().unwrap(), &none));
        assert!(!reached("8.8.8.8".parse().unwrap(), &none));
        assert!(reached("127.0.0.1".parse().unwrap(), &h));
        let mut far = HeaderMap::new();
        far.insert("x-forwarded-for", HeaderValue::from_static("8.8.8.8"));
        assert!(!reached("127.0.0.1".parse().unwrap(), &far));
        let mut funnel = h.clone();
        funnel.insert("tailscale-funnel-request", HeaderValue::from_static("?1"));
        assert!(!reached("127.0.0.1".parse().unwrap(), &funnel));
        assert!(!reached("127.0.0.1".parse().unwrap(), &p));
    }

    #[test]
    fn a_page_elsewhere_is_never_this_macs() {
        use axum::http::HeaderValue;
        for h in ["127.0.0.1:8080", "localhost:8080", "roads.localhost:8080", "[::1]:8080", "100.70.85.80:18085", "192.168.1.20:8080", "mac.tail1.ts.net", "mac.tail1.ts.net:8443", "Brandons-MacBook-Pro.local:8080", "macbookpro:8080", "fe80::1"] {
            assert!(ours(h), "{h}");
        }
        for h in ["evil.example:8080", "evil.example", "127.0.0.1.nip.io:8080", "localhost.evil.example", "", "[nonsense]:80"] {
            assert!(!ours(h), "{h}");
        }
        for o in ["http://localhost:8080", "http://roads.localhost:8080", "http://127.0.0.1:5173", "http://[::1]:8080"] {
            assert!(local_origin(o), "{o}");
        }
        for o in ["https://evil.example", "http://192.168.1.20:8080", "null", "http://localhost.evil.example", "http://127.0.0.1.nip.io"] {
            assert!(!local_origin(o), "{o}");
        }
        let with = |pairs: &[(&str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(header::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
            }
            h
        };
        // No page; this Mac's page; the device's page at the address it asks; a page elsewhere.
        assert!(from_the_page(&with(&[("host", "100.70.85.80:8080")])));
        assert!(from_the_page(&with(&[("host", "roads.localhost:8080"), ("origin", "http://127.0.0.1:8080")])));
        assert!(from_the_page(&with(&[("host", "mac.tail1.ts.net:8443"), ("origin", "https://mac.tail1.ts.net:8443")])));
        assert!(!from_the_page(&with(&[("host", "127.0.0.1:8080"), ("origin", "https://evil.example")])));
        assert!(!from_the_page(&with(&[("host", "127.0.0.1:8080"), ("origin", "null")])));
        assert!(!from_the_page(&with(&[("host", "100.70.85.80:8080"), ("origin", "http://100.70.85.81:8080")])));
        // Through tailscale serve, the address the device asked for in X-Forwarded-Host.
        assert!(from_the_page(&with(&[("host", "127.0.0.1:8080"), ("x-forwarded-host", "mac.tail1.ts.net:8443"), ("origin", "https://mac.tail1.ts.net:8443")])));
    }

    #[test]
    fn a_kept_token_is_the_same_next_time() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t/token");
        let a = kept_token(&p).unwrap();
        assert_eq!((a.len(), kept_token(&p).unwrap()), (32, a.clone()));
    }
}
