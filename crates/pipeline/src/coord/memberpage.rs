//! The build page on a member's port (docs/roles.md §3.3): the page is the lead's alone. A
//! member's broker answers the page and its data and asks, from a page of its own origin (or none:
//! a browser's address bar, the menu bar's web view), with a 307 to the lead's same path and query
//! at its address of the same network as the one asked (`location`),
//! while the lead answers its beats and says it serves its members' pages (`beats::PAGE`). Without
//! such a lead (none known, not answering, or an older app's) it answers `/work/` with a small page
//! of its own: this Mac's status, from its `helper.json`, and since when the lead isn't reached.
//!
//! Not redirected: `/work/live` and `/work/settings` asked by the lead's page from its own origin
//! (CORS, `Coordinator::set_brokers`), the jobs' calls (a key), and `sw.js` (an installed page's
//! worker updates from its own origin, then follows the redirects: web/work/sw.js).

use std::path::Path;

/// Where the build page a member's port is asked for is.
#[derive(Clone, Debug, PartialEq)]
pub enum PageAt {
    /// The lead's, at its coordinator's addresses (`urls`: its contact's, the tailnet's then its
    /// LAN name, crate::net::urls; `https`: its HTTPS page when `tailscale serve` proxies it,
    /// `Contact::page`).
    Lead { urls: Vec<String>, https: Option<String> },
    /// The lead not reached since `since` (unix s; None: not since this agent started), `lead` its
    /// name when known: this Mac's own page.
    Away { lead: Option<String>, since: Option<u64> },
}

impl Default for PageAt {
    fn default() -> PageAt {
        PageAt::Away { lead: None, since: None }
    }
}

/// What a member's port does with a keyless request for `path` from a page of its own origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    /// The page's (its files, its data, the owner's asks): the lead's to answer.
    Lead,
    /// The page's address itself: the lead's, or this Mac's own page.
    Page,
    /// Answered here as before (`/work/live`, `sw.js`, the jobs' calls).
    Here,
}

/// `path` (a GET when `get`) on a member's port, from a page of its own origin.
pub fn answer(path: &str, get: bool, page_file: impl Fn(&str) -> bool) -> Answer {
    if get {
        return match path {
            "/work" | "/work/" | "/work/index.html" => Answer::Page,
            "/work/sw.js" => Answer::Here,
            p if p.strip_prefix("/work/").is_some_and(&page_file) => Answer::Lead,
            _ => Answer::Here,
        };
    }
    match path {
        "/work/live" => Answer::Here,
        "/work/swarm" | "/work/history" | "/work/steps" | "/work/regions" | "/work/catalogs" | "/work/tocome"
        | "/work/built" | "/work/pause" | "/work/inputs" | "/work/act" | "/work/settings" | "/work/hold"
        | "/work/timings" | "/work/floors" | "/work/redo" => Answer::Lead,
        _ => Answer::Here,
    }
}

/// The lead's address for `path_and_query`, as reached the way this request reached this Mac
/// (`host`, its `Host`), so the device that asked can open it (any device, the owner's rule): asked
/// through `tailscale serve` (`proxied`), its HTTPS page when it has one; at a tailnet address
/// (100.64.0.0/10) or name (`.ts.net`), its tailnet address; at a LAN address or `.local` name
/// (a device on the LAN, perhaps without Tailscale), its LAN name; else (this Mac itself, the menu
/// bar's web view) its first.
pub fn location(lead: &PageAt, path_and_query: &str, proxied: bool, host: &str) -> Option<String> {
    let PageAt::Lead { urls, https } = lead else { return None };
    let name = host.rsplit_once(':').filter(|(h, p)| !h.ends_with(']') || p.is_empty()).map_or(host, |(h, _)| h);
    let name = name.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let ip = name.parse::<std::net::IpAddr>().ok();
    let tailnet = |u: &str| u.split("://").nth(1).and_then(|a| a.split(':').next()).is_some_and(is_tailnet);
    let lan = |u: &str| u.split("://").nth(1).and_then(|a| a.split(':').next()).is_some_and(|h| h.ends_with(".local"));
    let pick = |f: &dyn Fn(&str) -> bool| urls.iter().find(|u| f(u)).cloned();
    let base = match https.as_deref().filter(|_| proxied) {
        Some(h) => h.strip_suffix("work/").unwrap_or(h).to_string(),
        None if is_tailnet(&name) || name.ends_with(".ts.net") => pick(&tailnet).or_else(|| urls.first().cloned())?,
        None if name.ends_with(".local") || ip.is_some_and(|ip| !ip.is_loopback()) => {
            pick(&lan).or_else(|| urls.first().cloned())?
        }
        None => urls.first()?.clone(),
    };
    Some(format!("{}/{}", base.trim_end_matches('/'), path_and_query.trim_start_matches('/')))
}

/// Whether `host` is a tailnet address (100.64.0.0/10).
fn is_tailnet(host: &str) -> bool {
    host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

/// This Mac's own page (`home`: its agent's folder, its `helper.json`), the lead away.
pub fn page(home: Option<&Path>, lead: Option<&str>, since: Option<u64>, now: u64) -> String {
    let status: Option<crate::agent::Status> =
        home.and_then(|h| std::fs::read(h.join("helper.json")).ok()).and_then(|b| serde_json::from_slice(&b).ok());
    let lead = lead.map_or_else(|| "The lead".to_string(), |l| format!("The lead ({})", esc(l)));
    let since = match since {
        Some(t) => format!("since {}", ago(now, t)),
        None => "since this Mac's agent started".into(),
    };
    let mut body = format!("<p class=\"warn\">{lead} isn't reachable {since}: the build page is the lead's. This page is this Mac's own.</p>");
    match &status {
        None => body.push_str("<p>This Mac's agent hasn't written its status yet.</p>"),
        Some(s) => {
            body.push_str(&format!(
                "<h2>{} <span>app {} · status {}</span></h2>",
                esc(&s.host),
                esc(&s.app),
                ago(now, s.beat)
            ));
            let jobs = s.jobs();
            if jobs.is_empty() {
                body.push_str("<p>No job running.</p>");
            }
            for j in jobs {
                let how = j
                    .progress
                    .as_ref()
                    .filter(|p| p.total > 0.0)
                    .map(|p| format!(" · {:.0} of {:.0} {}", p.done, p.total, esc(&p.unit)))
                    .unwrap_or_default();
                let state =
                    j.paused.as_deref().or(j.pausing.as_deref()).map(|p| format!(" · {}", esc(p))).unwrap_or_default();
                body.push_str(&format!(
                    "<p><b>{}</b> {}, for {}{how}{state}</p>",
                    esc(&j.id),
                    esc(&j.what),
                    ago(now, j.started).trim_end_matches(" ago")
                ));
            }
            for w in &s.waiting {
                body.push_str(&format!("<p class=\"dim\">{}: {}</p>", esc(&w.what), esc(&w.why)));
            }
        }
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"color-scheme\" content=\"light dark\"><meta http-equiv=\"refresh\" content=\"10\"><title>Scenic build</title>\
<style>body{{margin:0;padding:12px 16px;background:#0e1013;color:#d7dce2;font:13px/1.45 -apple-system,BlinkMacSystemFont,system-ui,sans-serif}}\
@media (prefers-color-scheme: light){{body{{background:#f4f6f8;color:#1c232b}}.dim{{color:#5d6873}}.warn{{color:#9a6a12}}}}\
h1{{font-size:15px;margin:0 0 8px}}h2{{font-size:13px;margin:14px 0 6px}}h2 span,.dim{{font-weight:400;color:#8a939c}}.warn{{color:#e0b45a}}p{{margin:4px 0}}</style>\
</head><body id=\"away\"><h1>Scenic build</h1>{body}</body></html>"
    )
}

fn ago(now: u64, t: u64) -> String {
    let s = now.saturating_sub(t);
    match s {
        0..90 => format!("{s} s ago"),
        90..5400 => format!("{} min ago", (s + 30) / 60),
        _ => format!("{} h ago", (s + 1800) / 3600),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_and_its_asks_go_to_the_lead_the_jobs_and_live_stay() {
        let file = |p: &str| ["dash.js", "dash.css", "sw.js"].contains(&p);
        assert_eq!(answer("/work/", true, file), Answer::Page);
        assert_eq!(answer("/work", true, file), Answer::Page);
        assert_eq!(answer("/work/dash.js", true, file), Answer::Lead);
        assert_eq!(answer("/work/sw.js", true, file), Answer::Here);
        assert_eq!(answer("/work/in/1/a", true, file), Answer::Here);
        for p in ["/work/swarm", "/work/pause", "/work/settings", "/work/hold", "/work/act"] {
            assert_eq!(answer(p, false, file), Answer::Lead, "{p}");
        }
        for p in ["/work/live", "/work/ask", "/work/beat", "/task/x"] {
            assert_eq!(answer(p, false, file), Answer::Here, "{p}");
        }
        let lead = PageAt::Lead {
            urls: vec!["http://100.71.2.3:18091".into(), "http://mini.local:18091".into()],
            https: Some("https://mini.tail.ts.net/work/".into()),
        };
        let at = |host: &str, proxied: bool| location(&lead, "/work/?view&mac=m1", proxied, host).unwrap();
        // (Each device sent the way it came: the tailnet, the LAN (a phone without Tailscale), this
        // Mac itself, `tailscale serve`.)
        assert_eq!(at("100.80.0.1:18091", false), "http://100.71.2.3:18091/work/?view&mac=m1");
        assert_eq!(at("m1.tail.ts.net:18091", false), "http://100.71.2.3:18091/work/?view&mac=m1");
        assert_eq!(at("m1.local:18091", false), "http://mini.local:18091/work/?view&mac=m1");
        assert_eq!(at("192.168.1.20:18091", false), "http://mini.local:18091/work/?view&mac=m1");
        assert_eq!(at("127.0.0.1:18091", false), "http://100.71.2.3:18091/work/?view&mac=m1");
        assert_eq!(at("m1.tail.ts.net", true), "https://mini.tail.ts.net/work/?view&mac=m1");
        // (A lead with one address: that one, whichever way.)
        let one = PageAt::Lead { urls: vec!["http://100.71.2.3:18091".into()], https: None };
        assert_eq!(location(&one, "/work/", true, "m1.local").unwrap(), "http://100.71.2.3:18091/work/");
        assert!(location(&PageAt::default(), "/work/", false, "x").is_none());
    }

    #[test]
    fn the_own_page_says_the_lead_and_this_macs_jobs() {
        let d = tempfile::tempdir().unwrap();
        let p = page(Some(d.path()), Some("m<6>"), Some(1000), 1300);
        assert!(
            p.contains("The lead (m&lt;6&gt;) isn't reachable since 5 min ago") && p.contains("hasn't written"),
            "{p}"
        );
        let s = crate::agent::Status { host: "m1".into(), app: "1.2.3".into(), beat: 1290, ..Default::default() };
        std::fs::write(d.path().join("helper.json"), serde_json::to_vec(&s).unwrap()).unwrap();
        let p = page(Some(d.path()), None, None, 1300);
        assert!(
            p.contains("since this Mac's agent started") && p.contains("m1 <span>app 1.2.3 · status 10 s ago"),
            "{p}"
        );
        assert!(p.contains("No job running"));
    }
}
