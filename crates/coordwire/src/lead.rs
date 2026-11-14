//! How a Mac finds the lead's coordinator (docs/coordination.md §3.1, task #1119); nothing on the
//! NAS says where it is.
//!
//! - Each Mac's agent keeps its lead's contact in its folder (`SAVED`, `<agent home>/lead.json`):
//!   the lead's agent its own coordinator's, a member's as each beat's answer says it (`leading`,
//!   with the lead's addresses and page). Every other process on the Mac reads that file (the CLI,
//!   the map server, the app's updater and `scenic releases`, the dev tools): no NAS needed, so an
//!   update reaches a Mac whose share is away.
//! - With none saved, or one naming a member the declaration (`state/build/lead-member.json`, the
//!   owner's) no longer names, the declared lead's host is reached by its addresses (`addresses`:
//!   its tailnet address as Tailscale lists that host, then its LAN name), with the pool's token.
//!   A saved contact of the declared lead has those addresses after its own (its Mac may have moved
//!   networks since it was saved).

use crate::Contact;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The coordinator's port.
pub const PORT: u16 = 8090;

/// The lead's contact as a Mac's agent keeps it, in its folder.
pub const SAVED: &str = "lead.json";

/// The pool's token as an agent keeps it, under its folder (agentcore::agent::poolside's `seed`).
pub const TOKEN_KEPT: &str = "coord/workers-token";
/// The pool's token on the NAS (read once at each agent's start, and here by a process with no
/// agent's copy).
pub const TOKEN: &str = "state/coord/token";

/// A lead as a Mac knows it: its member and host, and how to reach it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Known {
    pub member: String,
    pub host: String,
    #[serde(flatten)]
    pub contact: Contact,
}

/// The app's folder on this Mac (`~/Library/Application Support/scenic`).
pub fn app_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/scenic")
}

/// This Mac's agent's folder, where it keeps its lead (`SAVED`).
pub fn agent_home() -> PathBuf {
    app_dir().join("agent")
}

/// This Mac's agent's folder for a process that isn't the agent (the CLI, the map server, the
/// updater): `agent_home`; None in a unit test, which never reads the Mac's own lead.
pub fn this_macs_agent() -> Option<PathBuf> {
    (!buildstore::out::testing()).then(agent_home)
}

/// The lead the agent of `home` keeps; None when it keeps none (or it doesn't read).
pub fn saved(home: &Path) -> Option<Known> {
    std::fs::read(home.join(SAVED)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Keeps `k` in `home` (whole, readable by this user alone: it holds the token), when it differs.
pub fn save(home: &Path, k: &Known) -> Result<()> {
    let p = home.join(SAVED);
    let b = serde_json::to_vec_pretty(k)?;
    if std::fs::read(&p).ok().as_deref() == Some(&b[..]) {
        return Ok(());
    }
    std::fs::create_dir_all(home).with_context(|| home.display().to_string())?;
    store::whole::write(&p, &b).with_context(|| p.display().to_string())?;
    if let Ok(f) = std::fs::File::open(&p) {
        store::sys::set_mode(&f, 0o600).ok();
    }
    Ok(())
}

/// The pool's token: the NAS's (the pool's, which every agent copies as it starts), else the
/// agent's copy in `home` (the share away).
pub fn token(home: Option<&Path>, root: Option<&store::nasfs::NasRoot>) -> Option<String> {
    let trim = |b: Vec<u8>| Some(String::from_utf8(b).ok()?.trim().to_string()).filter(|t| !t.is_empty());
    root.and_then(|r| r.join(TOKEN).read().ok())
        .and_then(trim)
        .or_else(|| home.and_then(|h| std::fs::read(h.join(TOKEN_KEPT)).ok()).and_then(trim))
}

/// The declared lead (`buildstore::records::lead`) and its host's addresses (`addresses`), with
/// `token`. None: no declaration. An error: the NAS can't be read now.
pub fn declared(root: &store::nasfs::NasRoot, token: &str) -> Result<Option<Known>> {
    let Some(d) = buildstore::records::lead::declared(root)? else { return Ok(None) };
    let urls = addresses(root, &d.host);
    Ok(Some(Known { member: d.member, host: d.host, contact: Contact { urls, token: token.to_string(), page: None } }))
}

/// How to reach the lead now: the one the agent of `home` keeps, while the declaration names it or
/// can't be read (no NAS: the saved one alone), the declared host's addresses after its own; else
/// the declared lead's. None: neither, or a root that answers declares no lead (whatever is kept
/// here).
pub fn find(home: Option<&Path>, root: Option<&store::nasfs::NasRoot>) -> Result<Option<Contact>> {
    let kept = home.and_then(saved);
    let Some(root) = root else { return Ok(kept.map(|k| k.contact)) };
    let Some(token) = token(home, Some(root)).or_else(|| kept.as_ref().map(|k| k.contact.token.clone())) else {
        return Ok(kept.map(|k| k.contact));
    };
    let decl = match declared(root, &token) {
        Ok(d) => d,
        Err(e) => return kept.map(|k| Some(k.contact)).ok_or(e),
    };
    Ok(match (kept, decl) {
        (Some(mut k), Some(d)) if k.member == d.member => {
            for u in d.contact.urls {
                if !k.contact.urls.contains(&u) {
                    k.contact.urls.push(u);
                }
            }
            Some(k.contact)
        }
        (_, Some(d)) => Some(d.contact),
        // (No lead declared on a root that answers (its `inputs/` there): none. A lead kept here is
        // another pool's then: a test's or a dev agent's job on a scratch root, or a command on a
        // local copy laid out as the project folder, reads this Mac's own agent's lead.json, the
        // real pool's. A share whose mount went stale reads as no file at all: the kept one.)
        (k, None) => k.filter(|_| !root.join("inputs").metadata().is_ok_and(|m| m.is_dir())).map(|k| k.contact),
    })
}

/// The lead's agent's status (agentcore::agent's `Status`, its members' among its `helpers`), asked
/// of its coordinator (`/pool/view`'s `status`): None when there's no lead to ask, or it doesn't
/// answer one.
pub fn status(home: Option<&Path>, root: Option<&store::nasfs::NasRoot>) -> Option<serde_json::Value> {
    let c = crate::client::Client::to_lead(home, root, &store::sys::host_name()).ok()??;
    let (code, v) = c.post_json("/pool/view", &serde_json::json!({})).ok()?;
    (code == 200).then(|| v["status"].clone()).filter(|s| s.is_object())
}

/// A host's addresses at `PORT`: its tailnet ones as `tailscale status` lists the peer (or this
/// Mac) of that name, then its LAN name. In a unit test (`buildstore::out::testing`), those a test's
/// lead announced for that host under `root` (`announce`).
pub fn addresses(root: &store::nasfs::NasRoot, host: &str) -> Vec<String> {
    if buildstore::out::testing() {
        return announced().lock().unwrap().get(&(key_of(root), host.to_string())).cloned().unwrap_or_default();
    }
    let mut out: Vec<String> = crate::net::tailscale_json(&["status", "--json"])
        .map(|v| tailnet_ips(&v, host))
        .unwrap_or_default()
        .into_iter()
        .map(|ip| format!("http://{ip}:{PORT}"))
        .collect();
    out.push(format!("http://{host}.local:{PORT}"));
    out
}

/// The tailnet IPv4 addresses `tailscale status --json` gives the device named `host` (its
/// computer name, `HostName`, or its DNS name's first label, by their letters and digits alone,
/// any case: "Mac mini de Brandon" is host `Mac-mini-de-Brandon`).
pub fn tailnet_ips(v: &serde_json::Value, host: &str) -> Vec<String> {
    let key = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let want = key(host);
    let peers = v["Peer"].as_object().into_iter().flat_map(|m| m.values());
    std::iter::once(&v["Self"])
        .chain(peers)
        .filter(|p| {
            let dns = p["DNSName"].as_str().unwrap_or("").split('.').next().unwrap_or("");
            !want.is_empty() && (key(p["HostName"].as_str().unwrap_or("")) == want || key(dns) == want)
        })
        .flat_map(|p| p["TailscaleIPs"].as_array().cloned().unwrap_or_default())
        .filter_map(|ip| ip.as_str().and_then(|s| s.parse::<std::net::Ipv4Addr>().ok()))
        .filter(|ip| ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        .map(|ip| ip.to_string())
        .collect()
}

type Announced = std::sync::Mutex<std::collections::HashMap<(String, String), Vec<String>>>;

/// A test pool's name in `announced`: its root, as it displays.
fn key_of(root: &store::nasfs::NasRoot) -> String {
    root.join(".").display().to_string()
}

fn announced() -> &'static Announced {
    static A: std::sync::OnceLock<Announced> = std::sync::OnceLock::new();
    A.get_or_init(Default::default)
}

/// In a unit test, a lead's coordinator at `urls` as host `host` of the pool at `root`, for
/// `addresses` (every Mac of a test is this process, on 127.0.0.1, its port its own); `None`: gone.
/// Nothing outside a test: a real host's addresses are its own.
pub fn announce(root: &store::nasfs::NasRoot, host: &str, urls: Option<Vec<String>>) {
    if !buildstore::out::testing() {
        return;
    }
    let key = (key_of(root), host.to_string());
    let mut a = announced().lock().unwrap();
    match urls {
        Some(u) => a.insert(key, u),
        None => a.remove(&key),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task #1119: a host is found in Tailscale's list by its computer name or DNS name, its
    /// tailnet IPv4 alone.
    #[test]
    fn a_host_is_found_in_tailscales_list() {
        let v = serde_json::json!({
            "Self": { "HostName": "MacBook Pro de Brandon", "DNSName": "macbook-pro-de-brandon.t.ts.net.", "TailscaleIPs": ["100.80.1.2", "fd7a::1"] },
            "Peer": {
                "a": { "HostName": "Mac mini de Brandon", "DNSName": "mac-mini-de-brandon.t.ts.net.", "TailscaleIPs": ["100.95.84.69", "fd7a::2"] },
                "b": { "HostName": "Brandon's MacBook Pro", "DNSName": "brandons-macbook-pro-cadstrom.t.ts.net.", "TailscaleIPs": ["100.70.85.80"] },
                "c": { "HostName": "localhost", "DNSName": "iphone.t.ts.net.", "TailscaleIPs": ["100.101.189.23"] }
            }
        });
        assert_eq!(tailnet_ips(&v, "Mac-mini-de-Brandon"), ["100.95.84.69"]);
        assert_eq!(tailnet_ips(&v, "Brandons-MacBook-Pro"), ["100.70.85.80"], "by its computer name");
        assert_eq!(tailnet_ips(&v, "MacBook-Pro-de-Brandon"), ["100.80.1.2"], "this Mac itself");
        assert!(tailnet_ips(&v, "Elsewhere").is_empty());
        assert!(tailnet_ips(&v, "").is_empty());
    }

    fn known(member: &str, urls: &[&str]) -> Known {
        Known {
            member: member.into(),
            host: format!("h-{member}"),
            contact: Contact { urls: urls.iter().map(|u| u.to_string()).collect(), token: "t".into(), page: None },
        }
    }

    fn declare(root: &Path, member: &str) {
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let d = serde_json::json!({ "member": member, "host": format!("h-{member}"), "since": 1 });
        std::fs::write(root.join(buildstore::records::lead::LEAD), d.to_string()).unwrap();
    }

    /// Task #1119: the saved lead first, the declared host's addresses after it; a saved lead the
    /// declaration no longer names gives way to the declared one; no NAS, the saved one alone; a
    /// root that declares none, none (review: a scratch root's job never reaches the real lead).
    #[test]
    fn the_lead_is_found_saved_then_declared() {
        let d = tempfile::tempdir().unwrap();
        let (home, nas) = (d.path().join("home"), d.path().join("nas"));
        std::fs::create_dir_all(&nas).unwrap();
        let root = store::nasfs::NasRoot::local_for_test(&nas);
        assert_eq!(find(Some(&home), Some(&root)).unwrap(), None, "nothing saved, none declared");
        declare(&nas, "m-1");
        assert_eq!(find(Some(&home), Some(&root)).unwrap(), None, "no token anywhere");
        std::fs::create_dir_all(nas.join("state/coord")).unwrap();
        std::fs::write(nas.join(TOKEN), "tok\n").unwrap();
        announce(&root, "h-m-1", Some(vec!["http://127.0.0.1:1".into()]));
        let c = find(Some(&home), Some(&root)).unwrap().unwrap();
        assert_eq!((c.urls, c.token), (vec!["http://127.0.0.1:1".to_string()], "tok".to_string()), "the declared");
        save(&home, &known("m-1", &["http://127.0.0.1:2"])).unwrap();
        assert_eq!(find(Some(&home), Some(&root)).unwrap().unwrap().urls, ["http://127.0.0.1:2", "http://127.0.0.1:1"]);
        assert_eq!(find(Some(&home), None).unwrap().unwrap().urls, ["http://127.0.0.1:2"], "no NAS: the saved alone");
        declare(&nas, "m-2");
        announce(&root, "h-m-2", Some(vec!["http://127.0.0.1:3".into()]));
        assert_eq!(find(Some(&home), Some(&root)).unwrap().unwrap().urls, ["http://127.0.0.1:3"], "the lead moved");
        assert_eq!(find(None, Some(&root)).unwrap().unwrap().token, "tok", "no agent here: the NAS's token");
        std::fs::remove_file(nas.join(buildstore::records::lead::LEAD)).unwrap();
        assert_eq!(
            find(Some(&home), Some(&root)).unwrap().unwrap().urls,
            ["http://127.0.0.1:2"],
            "a share gone stale: the kept"
        );
        std::fs::create_dir_all(nas.join("inputs")).unwrap();
        assert_eq!(
            find(Some(&home), Some(&root)).unwrap(),
            None,
            "a root that answers declaring no lead: not the one kept here"
        );
        announce(&root, "h-m-1", None);
        announce(&root, "h-m-2", None);
    }
}
