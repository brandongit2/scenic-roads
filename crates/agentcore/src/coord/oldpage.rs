//! The build page's old addresses during the rollout of task #1101, which moved the page from
//! `/work/` to the coordinator's root and its data and the owner's asks from `/work/<name>` to
//! `/api/<name>` (`super::API`). An app before it still asks the old ones: an older member's agent
//! (`/work/pause`, `/work/inputs`, `/work/tocome`, with the build's key), an older member's port
//! sending the page to the lead's `/work/` (docs/roles.md §3.3), the page an older lead served
//! asking a member's `/work/live`, `/work/settings` and `/work/asked`, and a page left open from
//! before. While one may (`gated`), a coordinator answers them as the new ones (`new_path`); a
//! member's port sends the page to an older lead's old addresses (`old_path`), an agent asks an
//! older lead's (coordwire's `Client::post_api`), and the page asks an older Mac's (its `remote`
//! and `ask`, web/buildpage/live.js and settings.js).
//!
//! The gate lifts by itself once every Mac runs an app that says `beats::ROOT`: on the lead, when
//! every member heard says it; on a member's port, when its lead's answer does. Then the old
//! addresses aren't answered (refused, as any path not one). This module, `post_api`'s second ask,
//! `memberpage::location`'s `work/`, the page's second asks and `/work/` checks, the menu bar's
//! `/work/` in `Panel.inPage` and the word go one release after every Mac has run the new app for a
//! day (the #642 gate).

use super::beats::Beats;
use super::memberpage::PageAt;

/// Whether a coordinator answers the old addresses at `now` (unix s): on a member's port (a
/// broker, a viewer: `member`), while its lead (`page_at`) isn't known to serve the new ones; on
/// the lead, while a member heard (`beats`) doesn't say it asks them (`Beats::old_page`).
pub fn gated(member: bool, page_at: &PageAt, beats: Option<&Beats>, now: u64) -> bool {
    if member {
        return !matches!(page_at, PageAt::Lead { root: true, .. });
    }
    beats.is_some_and(|b| b.old_page(now))
}

/// An old address's new one (`path` under `/work`; `page_file`: a name the page serves): the page
/// (`/work/`, its files), the page's data and asks (`/work/<name>`), and what the page served at
/// `/work/` asks (`/work/api/<name>`, its root that address). None: not the page's (the workers'
/// `/work/ask` …).
pub fn new_path(path: &str, page_file: impl Fn(&str) -> bool) -> Option<String> {
    let rest = if path == "/work" { "" } else { path.strip_prefix("/work/")? };
    if let Some(n) = rest.strip_prefix("api/").filter(|n| super::API.contains(n)) {
        return Some(format!("/api/{n}"));
    }
    if super::API.contains(&rest) {
        return Some(format!("/api/{rest}"));
    }
    page_file(rest).then(|| format!("/{rest}"))
}

/// A new address's old one (`path_and_query`), for an older lead: `/` and the page's files under
/// `/work/`, `/api/<name>` as `/work/<name>`.
pub fn old_path(path_and_query: &str) -> String {
    let rest = path_and_query.strip_prefix('/').unwrap_or(path_and_query);
    format!("/work/{}", rest.strip_prefix("api/").unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_addresses_are_the_new_ones() {
        let file = |f: &str| ["", "index.html", "dash.js", "icons/icon-192.png"].contains(&f);
        for (old, new) in [
            ("/work", Some("/")),
            ("/work/", Some("/")),
            ("/work/index.html", Some("/index.html")),
            ("/work/icons/icon-192.png", Some("/icons/icon-192.png")),
            ("/work/swarm", Some("/api/swarm")),
            ("/work/pause", Some("/api/pause")),
            ("/work/api/live", Some("/api/live")),
            // (The workers' stay theirs; nothing else is the page's.)
            ("/work/ask", None),
            ("/work/beat", None),
            ("/work/in/1/u/a", None),
            ("/work/api/ask", None),
            ("/work/nope.js", None),
            ("/workshop", None),
            ("/swarm", None),
        ] {
            assert_eq!(new_path(old, file).as_deref(), new, "{old}");
        }
        assert_eq!(old_path("/?view&mac=m1"), "/work/?view&mac=m1");
        assert_eq!(old_path("/dash.js"), "/work/dash.js");
        assert_eq!(old_path("/api/swarm"), "/work/swarm");
    }

    #[test]
    fn a_lead_answers_them_while_a_member_heard_asks_them_a_member_while_its_lead_does() {
        use super::super::beats::ROOT;
        let away = PageAt::default();
        assert!(!gated(false, &away, None, 100), "no pool: no other app");
        let b = Beats::default();
        assert!(!gated(false, &away, Some(&b), 100), "no member heard");
        let hb = |id: &str| crate::agent::pool::Heartbeat {
            pool: crate::pool::Beat { member: id.into(), host: id.into(), beat: 100, ..Default::default() },
            ..Default::default()
        };
        b.heard(hb("m1"), vec![ROOT.into()], None, 100);
        assert!(!gated(false, &away, Some(&b), 100));
        b.heard(hb("m4"), vec![], None, 100);
        assert!(gated(false, &away, Some(&b), 100), "m4 on an older app");
        b.heard(hb("m4"), vec![ROOT.into()], None, 110);
        assert!(!gated(false, &away, Some(&b), 110), "m4 updated: lifted");
        assert!(gated(true, &away, None, 110), "a member's port: its lead not known yet");
        let lead = |root| PageAt::Lead { urls: vec!["http://100.64.0.6:8090".into()], https: None, root };
        assert!(gated(true, &lead(false), None, 110), "an older lead's page asks this Mac's /work/live");
        assert!(!gated(true, &lead(true), None, 110));
    }
}
