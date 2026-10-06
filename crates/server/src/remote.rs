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
use std::net::SocketAddr;
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

// (Which requests are this Mac's own, what one must name, and the key's check: pipeline::net, the
// build Mac's coordinator's too.)
pub use pipeline::net::local_origin;
use pipeline::net::{from_the_page, ours, own, same};

/// What any device the server answers may have: the app itself, not the map's data.
fn public(path: &str) -> bool {
    matches!(path, "/" | "/index.html" | "/manifest.webmanifest" | "/sw.js" | "/api/ping" | "/api/auth") || path.starts_with("/assets/") || path.starts_with("/icons/")
}

/// Whether the request carries the key (its cookie, or `Authorization: Bearer`).
fn carries(h: &HeaderMap, key: &str) -> bool {
    let bearer = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).is_some_and(|v| same(v, key));
    bearer || h.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).filter_map(|c| c.trim().strip_prefix(&format!("{COOKIE}="))).any(|v| same(v, key))
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
    if !from_the_page(req.headers()) {
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
}
