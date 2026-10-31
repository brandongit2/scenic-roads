//! What the menu bar item (tools/status) shows, as `scenic menu` prints it: this Mac's own agent's
//! status when it runs here (written every few seconds), else the lead's heartbeat on the NAS
//! (`state/status.json`); whether it's this Mac's; for this Mac's, the running job's log; and this
//! Mac's hold and its ask (Scenic.app's Reply: tools/status/main.swift). (Scenic reads this Mac's
//! view of the pool from the agent's own files.) (Served by each Mac's map server as
//! `/api/build` until #874, when the map server came to run on the Mac mini alone.)

use std::path::Path;

/// The menu's answer at `now` (unix seconds): `home` the app's folder (`~/Library/Application
/// Support/scenic`), `lead` the lead's heartbeat as the NAS has it (Null without the NAS).
pub fn status(home: &Path, lead: serde_json::Value, now: u64) -> serde_json::Value {
    let read = |p: &str| -> Option<serde_json::Value> {
        std::fs::read(home.join(p)).ok().and_then(|b| serde_json::from_slice(&b).ok())
    };
    let own = read("agent/status.json");
    let fresh = own.as_ref().and_then(|v| v["beat"].as_u64()).is_some_and(|b| now.saturating_sub(b) < 120);
    // A helper on this Mac (docs/building.md §2, The pool; a pool's member): its own status, fresh.
    let helper = read("agent/helper.json").filter(|h| h["beat"].as_u64().is_some_and(|b| now.saturating_sub(b) < 120));
    // (The lead's heartbeat on the NAS, written every two minutes at least while there's a lead:
    // older than ten, there's none to show, and a member shows its own status in its place, so its
    // menu bar stays fresh.)
    let (mut status, local, helping) = match own {
        Some(v) if fresh => (v, true, false),
        _ => {
            let lead_fresh = lead["beat"].as_u64().is_some_and(|b| now.saturating_sub(b) < 600);
            match &helper {
                Some(h) if !lead_fresh => (h.clone(), true, true),
                _ => (lead, false, false),
            }
        }
    };
    // Its own status, fresh, in place of what the lead last read of it (which stops while the lead
    // sleeps); not when it's the status shown.
    if let (Some(h), Some(obj)) = (helper.filter(|h| !helping && h["host"] != status["host"]), status.as_object_mut()) {
        let host = h["host"].clone();
        let mut list: Vec<serde_json::Value> =
            obj.get("helpers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        list.retain(|x| x["host"] != host);
        list.push(serde_json::json!({"host": host, "beat": h["beat"], "job": h["job"], "pause": h["pause"], "held": h["held"], "here": true}));
        obj.insert("helpers".into(), serde_json::Value::Array(list));
    }
    let agent = home.join("agent");
    let log = if local {
        status["job"]["id"]
            .as_str()
            .map(|id| agent.join("logs").join(format!("{}.log", id.replace([' ', '/'], "-"))).display().to_string())
    } else {
        None
    };
    // This Mac paused alone (crate::control::Hold, task #224), as its agent keeps it, and its ask
    // while the agent hasn't taken it up.
    let held_here = crate::control::read_hold(&agent).held;
    let hold_asked = agent.join(crate::control::HOLD_REQUEST).exists();
    serde_json::json!({"status": status, "local": local, "now": now, "log": log, "held_here": held_here, "hold_asked": hold_asked})
}

/// The lead's heartbeat (`state/status.json`) as the NAS has it; Null when it can't be read.
pub fn lead_heartbeat(root: Option<&store::nasfs::NasRoot>) -> serde_json::Value {
    root.and_then(|r| r.join("state/status.json").read().ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_members_own_status_stands_in_for_a_lead_not_heard_from() {
        // 12 Oct: no lead for three days, the lead's heartbeat on the NAS frozen, and this member's
        // menu bar with it. Its own status, fresh, is shown in its place.
        let home = tempfile::tempdir().unwrap();
        let now = 1_791_300_000;
        std::fs::create_dir_all(home.path().join("agent")).unwrap();
        std::fs::write(
            home.path().join("agent/helper.json"),
            serde_json::json!({ "host": "member", "beat": now - 5, "job": null }).to_string(),
        )
        .unwrap();
        // The lead heard from: its status, this member's beside it as a helper.
        let body = status(home.path(), serde_json::json!({ "host": "lead", "beat": now - 100 }), now);
        assert_eq!((body["status"]["host"].as_str(), body["local"].as_bool()), (Some("lead"), Some(false)));
        assert_eq!(body["status"]["helpers"][0]["host"], "member");
        // Not for ten minutes: this member's own, as this Mac's.
        let body = status(home.path(), serde_json::json!({ "host": "lead", "beat": now - 3 * 86_400 }), now);
        assert_eq!((body["status"]["host"].as_str(), body["local"].as_bool()), (Some("member"), Some(true)));
        assert!(body["status"]["helpers"].is_null() && body.get("offline").is_none(), "{body}");
    }

    #[test]
    fn the_lead_heartbeat_is_read_from_the_nas() {
        let nas = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(nas.path().join("state")).unwrap();
        std::fs::write(nas.path().join("state/status.json"), r#"{"host": "lead", "beat": 7}"#).unwrap();
        let r = store::nasfs::NasRoot::local_for_test(nas.path());
        assert_eq!(lead_heartbeat(Some(&r))["host"], "lead");
        assert!(lead_heartbeat(None).is_null());
    }
}
