//! The map on other devices: an iPhone, an iPad (docs/plan.md §4, Devices). The server listens
//! beyond this Mac and answers this Mac, its LAN and the tailnet alone, also through a proxy on
//! this Mac (`tailscale serve`), from those alone (pipeline::net::reached).
//!
//! The address to open on a device (`<home>/map-page`, the status menu's Copy the Map's Address,
//! `scenic status`): over HTTPS where `tailscale serve` proxies the server, else its tailnet address.
//!
//! And a web page elsewhere, open in a browser on this Mac or a device, is never the map's own: a
//! request must name the map (its `Host`: an address, localhost, a tailnet or local network name,
//! never a public one, which a page could point at this Mac: DNS rebinding), and one from a page
//! (`Origin`) must come from the map's own page or this Mac's. So no site can read the map's data
//! or change its regions through a browser that can reach it.

use axum::extract::{ConnectInfo, Request};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Where the address for devices is written.
pub struct Remote {
    page: PathBuf,
}

impl Remote {
    pub fn new(home: &Path) -> Remote {
        Remote { page: home.join("map-page") }
    }

    /// The address to open on a device, written to `<home>/map-page` (0600) when it changes: the
    /// HTTPS one where `tailscale serve` proxies `port`, else the tailnet's (or the LAN name's).
    pub fn write_page(&self, port: u16) {
        let base = pipeline::net::served_https(port).or_else(|| pipeline::net::urls(port).first().map(|u| format!("{u}/")));
        let Some(base) = base else { return };
        let text = format!("{base}\n");
        if std::fs::read_to_string(&self.page).ok().as_deref() != Some(text.as_str()) && pipeline::whole::write(&self.page, text.as_bytes()).is_ok() {
            if let Ok(f) = std::fs::File::open(&self.page) {
                store::sys::set_mode(&f, 0o600).ok();
            }
        }
    }
}

// (Who may be answered and what a request must name: pipeline::net, the build Mac's coordinator's
// too.)
pub use pipeline::net::local_origin;
use pipeline::net::{from_the_page, ours, reached};

/// Every request, first: refused from anywhere but this Mac, its LAN and the tailnet, naming
/// another address, or from a page elsewhere.
pub async fn gate(ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    if !reached(peer.ip(), req.headers()) {
        return (StatusCode::FORBIDDEN, "not from here").into_response();
    }
    // (No Host at all: not a browser.)
    if req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).is_some_and(|h| !ours(h)) {
        return (StatusCode::FORBIDDEN, "not this map's address").into_response();
    }
    if !from_the_page(req.headers()) {
        return (StatusCode::FORBIDDEN, "not from the map's page").into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    /// What the gate answers a request for the map's data from `peer` with `headers`.
    async fn status(peer: &str, headers: &[(&str, &str)]) -> StatusCode {
        let app = axum::Router::new().route("/api/meta", axum::routing::get(|| async { "data" })).layer(axum::middleware::from_fn(gate));
        let mut req = Request::builder().uri("/api/meta");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut req = req.body(Body::empty()).unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(peer.parse().unwrap(), 50000)));
        app.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn the_lan_and_the_tailnet_get_the_data_without_a_key() {
        assert_eq!(status("127.0.0.1", &[("host", "127.0.0.1:8080")]).await, StatusCode::OK);
        assert_eq!(status("192.168.1.66", &[("host", "192.168.1.20:8080")]).await, StatusCode::OK);
        assert_eq!(status("100.101.1.2", &[("host", "mac.tail1.ts.net:8080"), ("origin", "http://mac.tail1.ts.net:8080")]).await, StatusCode::OK);
        // Through tailscale serve, from a tailnet device.
        assert_eq!(status("127.0.0.1", &[("host", "127.0.0.1:8080"), ("x-forwarded-host", "mac.tail1.ts.net"), ("x-forwarded-for", "100.101.1.2"), ("origin", "https://mac.tail1.ts.net")]).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_internet_another_address_and_a_page_elsewhere_are_refused() {
        assert_eq!(status("8.8.8.8", &[("host", "1.2.3.4:8080")]).await, StatusCode::FORBIDDEN);
        // Through the proxy from the internet: Tailscale Funnel, or an address that isn't ours.
        assert_eq!(status("127.0.0.1", &[("host", "127.0.0.1:8080"), ("x-forwarded-for", "100.101.1.2"), ("tailscale-funnel-request", "?1")]).await, StatusCode::FORBIDDEN);
        assert_eq!(status("127.0.0.1", &[("host", "127.0.0.1:8080"), ("x-forwarded-for", "8.8.8.8")]).await, StatusCode::FORBIDDEN);
        // A public name (DNS rebinding), and a page from another origin.
        assert_eq!(status("192.168.1.66", &[("host", "evil.example:8080")]).await, StatusCode::FORBIDDEN);
        assert_eq!(status("192.168.1.66", &[("host", "192.168.1.20:8080"), ("origin", "https://evil.example")]).await, StatusCode::FORBIDDEN);
        assert_eq!(status("127.0.0.1", &[("host", "127.0.0.1:8080"), ("origin", "null")]).await, StatusCode::FORBIDDEN);
    }

    #[test]
    fn the_address_for_devices_carries_no_key() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("map-page"), "http://100.70.85.80:8080/#k=0123456789abcdef0123456789abcdef\n").unwrap();
        Remote::new(d.path()).write_page(8080);
        let page = std::fs::read_to_string(d.path().join("map-page")).unwrap();
        // (No address at all on a Mac without a network: the old one stays, and isn't this test's.)
        if pipeline::net::urls(8080).first().is_some() || pipeline::net::served_https(8080).is_some() {
            assert!(!page.contains("#k="), "{page}");
        }
    }
}
