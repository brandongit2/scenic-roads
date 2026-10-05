//! What this Mac's services share about the network: who may connect (this Mac, its LAN, the
//! tailnet), the addresses others reach it by, whether `tailscale serve` proxies a port over HTTPS,
//! and the tokens a service keeps (made once, readable by its owner alone). The build Mac's
//! coordinator (crate::coord) and the map's server (crates/server: devices, docs/plan.md §4) use
//! them.

use anyhow::{Context, Result};
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
    fn a_kept_token_is_the_same_next_time() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t/token");
        let a = kept_token(&p).unwrap();
        assert_eq!((a.len(), kept_token(&p).unwrap()), (32, a.clone()));
    }
}
