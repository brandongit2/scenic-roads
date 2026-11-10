//! What the menu bar item (tools/status) shows, as `scenic menu` prints it, from this Mac's own disk
//! alone (docs/roles.md §3.4: never the NAS, so a hung share or a Mac away from home never holds
//! it): this Mac's own agent's status (the lead's `status.json`, a member's `helper.json`, the
//! fresher), and on a member the lead's line as the lead's last beat answer carried it
//! (crate::coord::beats::LEAD_VIEW: the build's state in a line, its pause, when the lead last
//! answered); the running job's log; and this Mac's hold and its ask (Scenic.app's Reply:
//! tools/status/main.swift).

use crate::coord::beats::LeadView;
use std::path::Path;

/// The menu's answer at `now` (unix seconds), from agent folder `home`
/// (`~/Library/Application Support/scenic/agent`).
pub fn status(home: &Path, now: u64) -> serde_json::Value {
    let read = |p: &str| -> Option<serde_json::Value> {
        std::fs::read(home.join(p)).ok().and_then(|b| serde_json::from_slice(&b).ok())
    };
    // (Either file may be left from a part this Mac had before: the one its agent writes now is
    // the fresher.)
    let beat = |v: &Option<serde_json::Value>| v.as_ref().and_then(|v| v["beat"].as_u64());
    let (lead, member) = (read("status.json"), read("helper.json"));
    let is_member = beat(&member) > beat(&lead);
    let status = if is_member { member } else { lead }.unwrap_or(serde_json::Value::Null);
    let job = status["jobs"][0]["id"].as_str().or(status["job"]["id"].as_str());
    let log =
        job.map(|id| home.join("logs").join(format!("{}.log", id.replace([' ', '/'], "-"))).display().to_string());
    // This Mac paused alone (jobrt::control::Hold, task #224), as its agent keeps it, and its ask
    // while the agent hasn't taken it up.
    let held_here = jobrt::control::read_hold(home).held;
    let hold_asked = home.join(jobrt::control::HOLD_REQUEST).exists();
    // (`local`: always, for a menu bar before docs/roles.md step 5, which requires it.)
    let mut out = serde_json::json!({"status": status, "local": true, "now": now, "log": log, "held_here": held_here, "hold_asked": hold_asked});
    if is_member {
        out["lead"] = serde_json::to_value(LeadView::read(home).unwrap_or_default()).unwrap_or_default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::beat::{Handle, LoopView, NasWrite, ToLead};
    use crate::agent::bus::Snap;
    use crate::agent::jobs::now_s;
    use crate::agent::{Status, Verdict};
    use crate::coord::beats::{Beats, LEAD_VIEW};
    use jobrt::control::HoldState;
    use std::sync::Arc;
    use std::time::Instant;

    const X: &str = "m-000000000000000a";

    /// A member's agent folder in `d`, beating the lead's coordinator at `port` (with key `token`)
    /// from its own beat thread, its NAS copy written by `nas`; its registry keeping the lead's
    /// line where the agent's pool side keeps it.
    fn member(d: &Path, port: u16, token: &str, nas: NasWrite) -> (Handle, ToLead) {
        let home = d.join("member");
        std::fs::create_dir_all(&home).unwrap();
        let beats = Arc::new(Beats::default());
        beats.keep_lead_view(&home.join(LEAD_VIEW));
        beats.set_own(crate::agent::pool::Heartbeat {
            pool: crate::pool::Beat { member: X.into(), host: "m1".into(), ..Default::default() },
            beat_http: true,
            ..Default::default()
        });
        let mut h = Handle::new(home, Snap::new(HoldState::default()));
        h.never_tick();
        h.write_nas_by(nas);
        h.spawn();
        let to = ToLead {
            urls: vec![format!("http://127.0.0.1:{port}")],
            token: token.into(),
            member: X.into(),
            held: Default::default(),
            listed: Instant::now(),
            cap: None,
            beats,
            home: d.join("member"),
        };
        (h, to)
    }

    /// One beat of the member's loop: its status (`jobs` its running job), its NAS copy at `nas`.
    fn publish(h: &mut Handle, to: &ToLead, d: &Path, jobs: Vec<crate::agent::JobView>) {
        let mut status = Status { host: "m1".into(), beat: now_s(), ..Default::default() };
        status.set_jobs(jobs, true);
        h.publish(LoopView {
            status,
            local: d.join("member/helper.json"),
            shared: Some(store::nasfs::NasPath::local_for_test(&d.join("nas/state/helpers/m1.json"))),
            lead: Some(to.clone()),
            hold_at: 0,
            at: Instant::now(),
        });
    }

    /// docs/roles.md §5 step 5: the lead's line rides in its beat's answer; the member keeps it
    /// (`lead-view.json`), and its menu shows it beside its own status.
    #[test]
    fn menu_reads_the_lead_from_the_beat() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("lead"), "lead", "");
        c.set_beats(Some(Arc::new(Beats::default())));
        let said = Verdict { state: "building".into(), said: "Building on mini: 3 jobs".into() };
        c.set_said("mini", said.clone(), Vec::new());
        let pause = jobrt::control::Pause::new(jobrt::control::Mode::Drain, "the build page");
        c.set_pause(Some(pause.clone()), pause.at);
        let (mut h, to) = member(d.path(), port, &c.contact.token, Arc::new(|_, _| Ok(())));
        publish(&mut h, &to, d.path(), Vec::new());
        store::testkit::eventually("the lead's line kept", || d.path().join("member").join(LEAD_VIEW).exists());
        let m = status(&d.path().join("member"), now_s());
        assert_eq!(m["status"]["host"], "m1", "{m}");
        assert_eq!(m["lead"]["lead"]["host"], "mini");
        assert_eq!(serde_json::from_value::<Verdict>(m["lead"]["lead"]["said"].clone()).unwrap(), said);
        assert_eq!(m["lead"]["lead"]["pause"]["by"], "the build page");
        assert_eq!(m["lead"]["lead"]["all_new"], true, "nobody old around");
        assert!(now_s() - m["lead"]["at"].as_u64().unwrap() < 60);
        // (The member's agent's all_new is the lead's word: crate::agent's `all_new`.)
        assert!(to.beats.lead_view().lead.is_some_and(|l| l.all_new));
        c.stop();
    }

    /// docs/roles.md §5 step 5, with the slow-NAS fake: every write of the member's NAS copy hangs
    /// until the share answers. Its menu still shows its own status, its job, and the lead's line,
    /// each as fresh as its last beat; and its answer is had from its own disk, the NAS untouched.
    #[test]
    fn menu_works_with_the_nas_hung() {
        let (answers, asked) = (store::testkit::Signal::new(), store::testkit::Signal::new());
        let (a, k) = (answers.clone(), asked.clone());
        let hung: NasWrite = Arc::new(move |_, _| {
            k.set();
            a.wait("the share answers");
            Ok(())
        });
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("lead"), "lead", "");
        c.set_beats(Some(Arc::new(Beats::default())));
        let (mut h, to) = member(d.path(), port, &c.contact.token, hung);
        let job = |id: &str| crate::agent::JobView {
            id: id.into(),
            what: format!("{id}'s work"),
            started: now_s(),
            paused: None,
            pausing: None,
            tail: String::new(),
            parts: Vec::new(),
            part: None,
            progress: None,
            mem_mb: None,
            threads: None,
            waiting: None,
            slot: Some(0),
            live: None,
        };
        for (i, (state, said)) in
            [("idle", "Nothing to build"), ("building", "Building: 2 jobs")].into_iter().enumerate()
        {
            c.set_said("mini", Verdict { state: state.into(), said: said.into() }, Vec::new());
            publish(&mut h, &to, d.path(), vec![job(&format!("unit 6/1/{i}"))]);
            store::testkit::eventually("the lead's newest line on the member's menu", || {
                status(&d.path().join("member"), now_s())["lead"]["lead"]["said"]["said"] == said
            });
            let m = status(&d.path().join("member"), now_s());
            assert_eq!(m["status"]["jobs"][0]["id"], format!("unit 6/1/{i}"), "{m}");
            assert!(m["log"].as_str().is_some_and(|l| l.ends_with(&format!("unit-6-1-{i}.log"))), "{m}");
        }
        assert!(asked.is_set() && !answers.is_set(), "the NAS's copy asked, and hung throughout");
        answers.set();
        c.stop();
    }

    /// A member's menu's lead line (`Reply.lead` in tools/status/main.swift): a member building
    /// while the lead isn't, the build paused from the page, heard 5 s before the status fixture's
    /// `now`. Pinned on both sides by tools/status/test/lead-view-said.json (the Swift selfTest
    /// reads it beside status-said.json; `SCENIC_BLESS=1` writes it anew).
    #[test]
    fn the_menu_bars_lead_line_is_scenics() {
        let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/status/test/lead-view-said.json");
        let mut lead = Status { host: "mini".into(), beat: 1_700_000_020, ..Default::default() };
        lead.helpers = vec![crate::agent::asks::tests::said_status()];
        lead.say(None, &HoldState::default(), false);
        let said = lead.build_said(1_700_000_025);
        let pause =
            jobrt::control::Pause { mode: jobrt::control::Mode::Drain, by: "the build page".into(), at: 1_700_000_010 };
        let line = crate::coord::held::LeadLine {
            host: "mini".into(),
            app: "2026-10-30".into(),
            said,
            pause: Some(pause),
            all_new: true,
            flags: vec![crate::agent::flags::Flag {
                kind: "idle".into(),
                mac: "m1".into(),
                said: "m1 has a slot free while 4 targets it could take wait (slope-deep 4): cores: 0 of 8 left".into(),
            }],
        };
        let ours = serde_json::to_value(LeadView { at: 1_700_000_025, lead: Some(line) }).unwrap();
        if std::env::var_os("SCENIC_BLESS").is_some() {
            std::fs::write(&f, serde_json::to_vec_pretty(&ours).unwrap()).unwrap();
        }
        let menu: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        assert_eq!(ours, menu, "tools/status/test/lead-view-said.json: SCENIC_BLESS=1 writes it anew");
        // (The fixture's member pauses alone, its three jobs finishing: running, so building.)
        assert_eq!(ours["lead"]["said"]["said"], "Building on m1; the lead: Nothing to build");
    }

    /// The lead's line: its own verdict; a member in touch with a job running makes the build
    /// building while the lead isn't, not while the build is paused or the member is out of touch.
    #[test]
    fn the_build_said_counts_a_member_building() {
        let mut lead = Status { host: "mini".into(), beat: 1_000, ..Default::default() };
        let mut m = crate::agent::asks::tests::said_status();
        m.beat = 1_000;
        lead.helpers = vec![m];
        lead.say(None, &HoldState::default(), false);
        assert_eq!(lead.build_said(1_000).state, "building");
        assert_eq!(
            lead.build_said(1_000 + crate::pool::beat::OUT_OF_TOUCH_S + 1).said,
            "Nothing to build",
            "out of touch"
        );
        lead.pause = Some(jobrt::control::Pause::new(jobrt::control::Mode::Drain, "t"));
        lead.say(None, &HoldState::default(), false);
        assert_eq!(lead.build_said(1_000).state, "paused");
    }

    /// The lead's own menu: its status, the fresher of its two files (a helper's left from a
    /// member's part before), no lead's line; with no status, none.
    #[test]
    fn the_leads_menu_shows_its_own_status() {
        let home = tempfile::tempdir().unwrap();
        let now = 1_791_300_000;
        let put = |f: &str, host: &str, beat: u64| {
            std::fs::write(home.path().join(f), serde_json::json!({ "host": host, "beat": beat }).to_string()).unwrap()
        };
        assert!(status(home.path(), now)["status"].is_null());
        put("status.json", "mini", now - 5);
        put("helper.json", "mini", now - 86_400);
        let m = status(home.path(), now);
        assert_eq!(m["status"]["beat"], now - 5);
        assert!(m.get("lead").is_none() && m["log"].is_null(), "{m}");
        // (A member again: its helper's file, the fresher, and the lead's line, none heard yet.)
        put("helper.json", "mini", now - 1);
        let m = status(home.path(), now);
        assert_eq!((m["status"]["beat"].as_u64(), m["lead"]["at"].as_u64()), (Some(now - 1), Some(0)), "{m}");
    }
}
