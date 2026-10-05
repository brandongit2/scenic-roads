//! The map on other devices: an iPhone, an iPad (docs/plan.md §4, Devices). The server listens
//! beyond this Mac, answering only this Mac, its LAN and the tailnet (pipeline::net::allowed), and a
//! request that isn't this Mac's own needs the map's key: from another address, or handed over by
//! a proxy on it (`tailscale serve` forwards a device's requests from loopback, X-Forwarded-For
//! set). The key comes in the map's address (`#k=…`, never sent to a server); the page gives it once
//! to POST /api/auth, which keeps it in an HttpOnly cookie (`scenic_k`), and every request carries
//! that. Without it only the app itself is served (its page, scripts, styles, manifest, service
//! worker and icons): none of the map's data, which is for its owner alone (licences).
//!
//! The address to open on a device (`<home>/map-page`, the status menu's Copy the Map's Address,
//! `scenic status`): over HTTPS where `tailscale serve` proxies the server, else its tailnet address.
//!
//! And a web page elsewhere, open in a browser on this Mac or a device, is never the map's own: a
//! request must name the map (its `Host`: an address, localhost, a tailnet or local network name,
//! never a public one, which a page could point at this Mac: DNS rebinding), and one from a page
//! (`Origin`) must come from the map's own page or this Mac's. So no site can read the map's data
//! or change its regions through a browser that can reach it.

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

/// The cookie the key is kept in.
const COOKIE: &str = "scenic_k";

/// The key and where the address for devices is written.
pub struct Remote {
    pub key: String,
    page: PathBuf,
}

impl Remote {
    /// The map's key (`<home>/remote-key`: made once, kept, readable by this Mac's user alone).
    pub fn new(home: &Path) -> anyhow::Result<Remote> {
        Ok(Remote { key: pipeline::net::kept_token(&home.join("remote-key"))?, page: home.join("map-page") })
    }

    /// The address to open on a device, written to `<home>/map-page` (0600) when it changes: the
    /// HTTPS one where `tailscale serve` proxies `port`, else the tailnet's (or the LAN name's).
    pub fn write_page(&self, port: u16) {
        let base = pipeline::net::served_https(port).or_else(|| pipeline::net::urls(port).first().map(|u| format!("{u}/")));
        let Some(base) = base else { return };
        let text = format!("{base}#k={}\n", self.key);
        if std::fs::read_to_string(&self.page).ok().as_deref() != Some(text.as_str()) && pipeline::whole::write(&self.page, text.as_bytes()).is_ok() {
            if let Ok(f) = std::fs::File::open(&self.page) {
                store::sys::set_mode(&f, 0o600).ok();
            }
        }
    }
}

/// Whether a request is this Mac's own: from loopback, and not handed over by a proxy on this Mac
/// (any header a proxy adds).
fn own(peer: IpAddr, h: &HeaderMap) -> bool {
    pipeline::net::loopback(peer) && !["x-forwarded-for", "x-forwarded-host", "x-forwarded-proto", "x-real-ip", "forwarded", "tailscale-user-login"].iter().any(|k| h.contains_key(*k))
}

/// Whether a host (a `Host` header's, an `Origin`'s; a port after it is ignored) names this map: an
/// IP address, localhost or a name under it, or a name only a tailnet or a local network resolves
/// (one label, or under .local, .home, .lan, .internal, .ts.net). Never a public name.
fn ours(host: &str) -> bool {
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

/// Whether a request's page, if it says one (`Origin`), is the map's: this Mac's own, or the
/// address the request itself names (the map's page on a device), as it came or as a proxy on this
/// Mac says it came (`X-Forwarded-Host`: a page can't set that without a preflight, which CORS
/// answers for this Mac's own pages alone).
fn from_the_map(h: &HeaderMap) -> bool {
    let Some(o) = h.get(header::ORIGIN) else { return true };
    let Ok(o) = o.to_str() else { return false };
    let named = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).is_some_and(|host| o.split_once("://").is_some_and(|(_, a)| a.eq_ignore_ascii_case(host)));
    local_origin(o) || named("host") || named("x-forwarded-host")
}

/// What any device the server answers may have: the app itself, not the map's data.
fn public(path: &str) -> bool {
    matches!(path, "/" | "/index.html" | "/manifest.webmanifest" | "/sw.js" | "/api/ping" | "/api/auth") || path.starts_with("/assets/") || path.starts_with("/icons/")
}

/// Whether the request carries the key (its cookie, or `Authorization: Bearer`).
fn carries(h: &HeaderMap, key: &str) -> bool {
    let same = |v: &str| v.len() == key.len() && v.bytes().zip(key.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
    let bearer = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).is_some_and(same);
    bearer || h.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).filter_map(|c| c.trim().strip_prefix(&format!("{COOKIE}="))).any(same)
}

/// Every request, first: refused from anywhere but this Mac, its LAN and the tailnet; another
/// device's needs the key, but for the app itself.
pub async fn gate(State(r): State<std::sync::Arc<Remote>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    if !pipeline::net::allowed(peer.ip()) {
        return (StatusCode::FORBIDDEN, "not from here").into_response();
    }
    // (No Host at all: not a browser.)
    if req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).is_some_and(|h| !ours(h)) {
        return (StatusCode::FORBIDDEN, "not this map's address").into_response();
    }
    if !from_the_map(req.headers()) {
        return (StatusCode::FORBIDDEN, "not from the map's page").into_response();
    }
    if own(peer.ip(), req.headers()) || public(req.uri().path()) || carries(req.headers(), &r.key) {
        return next.run(req).await;
    }
    (StatusCode::UNAUTHORIZED, [(header::CACHE_CONTROL, "no-store")], axum::Json(serde_json::json!({ "error": "the map's address with its key (#k=…) is needed once on this device" }))).into_response()
}

/// POST /api/auth `{"key": "…"}`: the key given once from the map's address, kept in its cookie.
pub async fn auth(State(r): State<std::sync::Arc<Remote>>, h: HeaderMap, body: Bytes) -> Response {
    let given = serde_json::from_slice::<serde_json::Value>(&body).ok().and_then(|v| v["key"].as_str().map(str::to_string)).unwrap_or_default();
    let mut ok = HeaderMap::new();
    ok.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {given}")).unwrap_or(HeaderValue::from_static("")));
    if given.is_empty() || !carries(&ok, &r.key) {
        return (StatusCode::UNAUTHORIZED, "not the map's key").into_response();
    }
    // (Secure where the page came over HTTPS: `tailscale serve` says so.)
    let https = h.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    let cookie = format!("{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=315360000{}", r.key, if https { "; Secure" } else { "" });
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie), (header::CACHE_CONTROL, "no-store".into())]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_macs_own_and_another_devices() {
        let mut h = HeaderMap::new();
        assert!(own("127.0.0.1".parse().unwrap(), &h) && own("::1".parse().unwrap(), &h));
        assert!(!own("100.70.85.80".parse().unwrap(), &h));
        // Handed over by tailscale serve: a device's.
        h.insert("x-forwarded-for", HeaderValue::from_static("100.101.1.2"));
        assert!(!own("127.0.0.1".parse().unwrap(), &h));
        // By any proxy saying it is one.
        let mut x = HeaderMap::new();
        x.insert("x-forwarded-host", HeaderValue::from_static("mac.tail1.ts.net:8443"));
        assert!(!own("127.0.0.1".parse().unwrap(), &x));
    }

    #[test]
    fn the_key_in_its_cookie_or_a_bearer() {
        let key = "0123456789abcdef0123456789abcdef";
        let mut h = HeaderMap::new();
        assert!(!carries(&h, key));
        h.insert(header::COOKIE, HeaderValue::from_str(&format!("other=1; {COOKIE}={key}")).unwrap());
        assert!(carries(&h, key));
        let mut w = HeaderMap::new();
        w.insert(header::COOKIE, HeaderValue::from_str(&format!("{COOKIE}=0123456789abcdef0123456789abcdee")).unwrap());
        assert!(!carries(&w, key));
        let mut b = HeaderMap::new();
        b.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {key}")).unwrap());
        assert!(carries(&b, key));
        assert!(public("/assets/index-abc.js") && public("/") && public("/sw.js") && !public("/api/meta") && !public("/tiles/roads/1/0/0") && !public("/fonts/x/0-255.pbf"));
    }

    #[test]
    fn a_page_elsewhere_is_never_the_maps() {
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
        assert!(from_the_map(&with(&[("host", "100.70.85.80:8080")])));
        assert!(from_the_map(&with(&[("host", "roads.localhost:8080"), ("origin", "http://127.0.0.1:8080")])));
        assert!(from_the_map(&with(&[("host", "mac.tail1.ts.net:8443"), ("origin", "https://mac.tail1.ts.net:8443")])));
        assert!(!from_the_map(&with(&[("host", "127.0.0.1:8080"), ("origin", "https://evil.example")])));
        assert!(!from_the_map(&with(&[("host", "127.0.0.1:8080"), ("origin", "null")])));
        assert!(!from_the_map(&with(&[("host", "100.70.85.80:8080"), ("origin", "http://100.70.85.81:8080")])));
        // Through tailscale serve, the address the device asked for in X-Forwarded-Host.
        assert!(from_the_map(&with(&[("host", "127.0.0.1:8080"), ("x-forwarded-host", "mac.tail1.ts.net:8443"), ("origin", "https://mac.tail1.ts.net:8443")])));
    }
}
