//! Budgets and small leases by HTTP (docs/coordination.md §8, step 7 of task #878): the pool's
//! Wikimedia budget, the answers' lease and the members' resume progress, kept by the lead in place
//! of their files on the NAS (`state/pool/wiki/`, `state/pool/answers.lease`,
//! `state/pool/progress/`).
//!
//! - **The budget** (`POST /pool/budget/take {class}`, `POST /pool/budget/hold {s}`): each
//!   window's tokens counted in memory (`Windows`), by the lead's clock alone, one a take, so a
//!   429's hold stops every Mac at its next token; a token is spendable for what is left of its
//!   window (crate::budget::Given). The one source: a Mac that can't reach
//!   the lead takes nothing (crate::budget). Not durable: a restarted lead gives
//!   nothing in the window it first hears an ask in (its process before may have given that
//!   window's tokens), so a restart only slows. A 429's hold is kept in the store (`budget-hold`),
//!   so a restarted lead keeps it.
//! - **The answers' lease** (`POST /pool/lease/answers {op, by, token}`, crate::answers): one
//!   holder at a time, lapsing `answers::LEASE_S` after it was last taken or renewed, by the lead's
//!   clock; kept in the store (`answers-lease`) before the answer.
//! - **Resume progress** (crate::agent::resume): each member's beat carries its progress whole
//!   (`held::Beat::progress`); the lead keeps it in the store when it changes (`progress/<member>`,
//!   with the owner's releases of it: `POST /pool/progress/release {member, job}`), for its plan,
//!   for a day after the member's last word as before (the member may be asleep), and answers each
//!   beat with that member's releases (`held::Answer::progress_released`).
//! - **The gate** (`beats::BUDGETS`): a member's agent gives its proxy the lead (`OverHttp`; the
//!   lead's own agent, `InProcess`) while the lead says it takes them and no member heard within
//!   `beats::FORGET_S` doesn't say it, nor one heartbeats on the NAS alone (`held::on_nas`): an
//!   older app takes its tokens and the lease on the NAS, so every Mac does meanwhile. The lead
//!   gives batches only while every member heard says its proxy takes from it (`beats::BY_LEAD`),
//!   and its own does (`gives`): none leases on the NAS as another takes from the lead. A member
//!   writes its progress file on the NAS while its lead doesn't say `BUDGETS` (an older lead
//!   reads it there); the lead reads a member's file there while that member doesn't say it. All
//!   deleted one release after every member says it for a day (the #642 gate).
//!
//! Nothing here touches the NAS: the coordinator's handlers reach it (§2.3).

use super::{beats, Shared};
use crate::agent::resume::Progress;
use crate::budget::{Class, Given, Lead, LeaseOp, LeaseSaid};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The answers' lease's document in the store.
pub const ANSWERS: &str = "answers-lease";
/// A 429's hold in the store (`{until_ms}`): a restarted lead keeps it.
pub const HOLD: &str = "budget-hold";
/// The members' progress's documents in the store (`progress/<member id>`).
pub const PROGRESS: &str = "progress/";

/// Each class's window and the tokens given in it, by the lead's clock (ms since the epoch): one a
/// take, so a Mac holds none it hasn't spent, and a hold stops every Mac at its next token (a
/// batch of more would be spent on another Mac after a 429 told here).
#[derive(Debug, Default)]
pub struct Windows {
    given: BTreeMap<Class, (u64, u32)>,
    hold_until: u64,
    /// The window of each class it first heard an ask in: none given in it.
    first: BTreeMap<Class, u64>,
}

impl Windows {
    /// A batch of `class` at `now` (ms), or how long to wait for one.
    pub fn take(&mut self, class: Class, now: u64) -> Given {
        let r = class.rate();
        let w = now / r.window_ms;
        let end = (w + 1) * r.window_ms;
        if now < self.hold_until {
            return Given::Wait { ms: self.hold_until - now };
        }
        // (A lead restarted mid-window: its process before may have given this window's slots.)
        if *self.first.entry(class).or_insert(w) == w {
            return Given::Wait { ms: end - now };
        }
        let g = self.given.entry(class).or_insert((w, 0));
        // (Its clock set back into a window it has given in already: none till that one ends.)
        if w < g.0 {
            return Given::Wait { ms: (g.0 + 1) * r.window_ms - now };
        }
        if g.0 != w {
            *g = (w, 0);
        }
        if g.1 >= r.tokens {
            return Given::Wait { ms: end - now };
        }
        g.1 += 1;
        Given::Batch { n: 1, window: w, for_ms: end - now }
    }

    /// Every class held `secs` from `now` (capped at crate::budget::HOLD_MAX_S): until when (ms).
    pub fn hold(&mut self, secs: u64, now: u64) -> u64 {
        self.hold_until = self.hold_until.max(now + secs.clamp(1, crate::budget::HOLD_MAX_S) * 1000);
        self.hold_until
    }
}

/// The answers' lease as the lead keeps it (`ANSWERS`): who holds it, their token, until when
/// (unix s).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswersLease {
    pub by: String,
    pub token: String,
    pub until: u64,
}

/// A member's progress as the lead keeps it (`PROGRESS`): as its beat last said it, and the owner's
/// releases of it (job, when its progress began: crate::agent::resume::Held::began).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub progress: Progress,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub released: BTreeSet<(String, u64)>,
}

/// What the lead keeps of this step.
#[derive(Debug, Default)]
pub struct Small {
    pub windows: Windows,
    /// Its own agent's proxy takes from it (`Coordinator::set_budget_by_lead`): the gate's.
    pub own_by_lead: bool,
    pub answers: Option<AnswersLease>,
    pub progress: BTreeMap<String, Kept>,
}

impl Small {
    /// As its store's documents hold them (the budget's windows afresh).
    pub fn of(docs: &crate::pool::store::Docs) -> Small {
        let answers = docs.get(ANSWERS).and_then(|v| serde_json::from_value(v.clone()).ok());
        let progress = docs
            .range(PROGRESS.to_string()..)
            .take_while(|(k, _)| k.starts_with(PROGRESS))
            .filter_map(|(k, v)| Some((k[PROGRESS.len()..].to_string(), serde_json::from_value(v.clone()).ok()?)))
            .collect();
        let hold_until = docs.get(HOLD).and_then(|v| v["until_ms"].as_u64()).unwrap_or(0);
        Small { windows: Windows { hold_until, ..Windows::default() }, own_by_lead: false, answers, progress }
    }
}

/// The lead's clock in ms.
fn now_ms(s: &Shared) -> u64 {
    s.clock.wall().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A coordinator that keeps no store (a member's broker, a viewer) is no lead: it refuses.
fn lead_only(s: &Shared) -> Result<()> {
    anyhow::ensure!(s.keep.store().is_some(), "this coordinator keeps no store: not the lead's");
    Ok(())
}

/// `POST /pool/budget/take`: a batch only while every Mac takes from it (`gives`); else a wait
/// for a window.
pub fn take(s: &mut Shared, class: Class) -> Result<Given> {
    lead_only(s)?;
    let now = now_ms(s);
    if !gives(s) {
        return Ok(Given::Wait { ms: class.rate().window_ms });
    }
    Ok(s.small.windows.take(class, now))
}

/// Whether it gives batches (the gate, docs/coordination.md step 7): every member heard within
/// `beats::FORGET_S` says `beats::BUDGETS` (none on the NAS: `on_nas`) and says its proxy takes
/// from the lead (`beats::BY_LEAD`), and its own agent's proxy does: no Mac leases on the NAS
/// meanwhile, nor holds a batch from it (a move drops it, crate::budget::Budget::set_lead). A
/// member that moves back to the NAS waits a window first (crate::budget::SWITCH_MS), by which
/// time every batch given before the gate turned has ended.
pub fn gives(s: &Shared) -> bool {
    let unix = super::held::wall(s);
    s.small.own_by_lead
        && s.beats.as_ref().is_some_and(|b| {
            !super::held::on_nas(s, b, unix, beats::BUDGETS) && !super::held::on_nas(s, b, unix, beats::BY_LEAD)
        })
}

/// `POST /pool/budget/hold`: until when (ms).
pub fn hold(s: &mut Shared, secs: u64) -> Result<u64> {
    lead_only(s)?;
    let now = now_ms(s);
    eprintln!("coordinator: the Wikimedia budget held {secs} s (a 429 seen)");
    let until = s.small.windows.hold(secs, now);
    s.keep.put(HOLD, serde_json::json!({ "until_ms": until }));
    Ok(until)
}

/// `POST /pool/lease/answers`: the ask done or not, and who holds it now; its change submitted to
/// the store (the caller waits for it before it answers).
pub fn answers(s: &mut Shared, op: LeaseOp, by: &str, token: &str) -> Result<LeaseSaid> {
    lead_only(s)?;
    anyhow::ensure!(!token.is_empty() && token.len() <= 128 && by.len() <= 200, "a holder's token and name");
    let now = super::held::wall(s);
    let held = s.small.answers.clone();
    let mine = held.as_ref().is_some_and(|h| h.token == token);
    let live = held.as_ref().is_some_and(|h| h.until > now);
    let ok = match op {
        LeaseOp::Take => mine || !live,
        LeaseOp::Renew => mine && live,
        LeaseOp::Release => mine,
    };
    let after = match (op, ok) {
        (LeaseOp::Release, true) => None,
        (_, true) => {
            Some(AnswersLease { by: by.to_string(), token: token.to_string(), until: now + crate::answers::LEASE_S })
        }
        (_, false) => held.filter(|h| h.until > now),
    };
    if ok {
        match &after {
            Some(l) => s.keep.put(ANSWERS, serde_json::to_value(l)?),
            None => s.keep.del(ANSWERS),
        }
        s.small.answers = after.clone();
    }
    let (by, until) = after.map_or((String::new(), 0), |h| (h.by, h.until));
    Ok(LeaseSaid { ok, by, until })
}

/// Member `p.member`'s progress as its beat (or, the lead's own, its agent) says it now: kept when
/// it changed (a release kept while the job it names began then still); the owner's releases of it.
pub fn heard(s: &mut Shared, p: Progress) -> Vec<(String, u64)> {
    if !crate::pool::is_member_id(&p.member) || s.keep.store().is_none() {
        return Vec::new();
    }
    let key = format!("{PROGRESS}{}", p.member);
    let was = s.small.progress.get(&p.member).cloned().unwrap_or_default();
    if was.progress == p {
        return was.released.into_iter().collect();
    }
    let released: BTreeSet<(String, u64)> =
        was.released.into_iter().filter(|(job, at)| p.jobs.get(job).is_some_and(|h| h.began() == *at)).collect();
    let k = Kept { progress: p, released };
    if k.progress.jobs.is_empty() {
        s.keep.del(&key);
        s.small.progress.remove(&k.progress.member);
    } else if let Ok(v) = serde_json::to_value(&k) {
        s.keep.put(&key, v);
        s.small.progress.insert(k.progress.member.clone(), k.clone());
    }
    k.released.into_iter().collect()
}

/// `POST /pool/progress/release`: the owner's release of `member`'s progress of `job`
/// (`scenic pool release <member> <job>`): the lead runs it wherever it fits from then. What it
/// released, or why nothing.
pub fn release(s: &mut Shared, member: &str, job: &str) -> Result<String> {
    lead_only(s)?;
    let k = s.small.progress.get(member).cloned().ok_or_else(|| anyhow::anyhow!("{member} has no progress"))?;
    let h = k.progress.jobs.get(job).ok_or_else(|| {
        anyhow::anyhow!(
            "{member}'s progress holds no {job:?} (it holds {:?})",
            k.progress.jobs.keys().collect::<Vec<_>>()
        )
    })?;
    let r = (job.to_string(), h.began());
    if k.released.contains(&r) {
        return Ok(format!("{job}'s progress on {member} was released already"));
    }
    let mut k = k.clone();
    k.released.insert(r);
    s.keep.put(&format!("{PROGRESS}{member}"), serde_json::to_value(&k)?);
    let worker = k.progress.worker.clone();
    s.small.progress.insert(member.to_string(), k);
    Ok(format!("{job}'s progress on {member} ({worker}) released: the lead runs it wherever it fits"))
}

/// Whether the budget and the answers' lease are taken on the NAS (and members write their
/// progress files there): a member heard doesn't say `beats::BUDGETS`, or one heartbeats on the
/// NAS alone (the gate). Without its agent's registry, as before the pool's side is attached: yes.
pub fn on_nas(s: &Shared) -> bool {
    let unix = super::held::wall(s);
    s.beats.as_ref().is_none_or(|b| super::held::on_nas(s, b, unix, beats::BUDGETS))
}

/// The lead's coordinator over HTTP, a member's agent's (its proxy's lead,
/// crate::budget::Budget::set_lead).
pub struct OverHttp {
    pub urls: Vec<String>,
    pub token: String,
    pub worker: String,
}

impl OverHttp {
    fn post(&self, path: &str, body: serde_json::Value) -> std::result::Result<serde_json::Value, String> {
        let c = super::client::Client::at(self.urls.clone(), self.token.clone(), &self.worker);
        match c.post_json(path, &body) {
            Ok((200, v)) => Ok(v),
            Ok((code, v)) => Err(format!("{path} answered {code}: {}", v["error"].as_str().unwrap_or(""))),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

impl Lead for OverHttp {
    fn take(&self, class: Class) -> std::result::Result<Given, String> {
        let v = self.post("/pool/budget/take", serde_json::json!({ "class": class.name() }))?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    fn hold(&self, secs: u64) -> std::result::Result<(), String> {
        self.post("/pool/budget/hold", serde_json::json!({ "s": secs })).map(|_| ())
    }

    fn answers(&self, op: LeaseOp, by: &str, token: &str) -> std::result::Result<LeaseSaid, String> {
        let v = self.post("/pool/lease/answers", serde_json::json!({ "op": op, "by": by, "token": token }))?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
}

/// The lead's own coordinator, in process: the lead's agent's proxy's lead.
pub struct InProcess(pub std::sync::Arc<super::Coordinator>);

impl Lead for InProcess {
    fn take(&self, class: Class) -> std::result::Result<Given, String> {
        self.0.budget_take(class).map_err(|e| format!("{e:#}"))
    }

    fn hold(&self, secs: u64) -> std::result::Result<(), String> {
        self.0.budget_hold(secs).map(|_| ()).map_err(|e| format!("{e:#}"))
    }

    fn answers(&self, op: LeaseOp, by: &str, token: &str) -> std::result::Result<LeaseSaid, String> {
        self.0.answers_lease(op, by, token).map_err(|e| format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_800_000_000_000;

    /// docs/coordination.md step 7: three Macs' budgets (each its proxy's, many threads) taking
    /// from one lead's windows on one clock: no window gives more than its class's tokens, every
    /// token comes from the lead, and every request is served.
    #[test]
    fn the_wiki_budget_holds_across_three_macs() {
        use crate::budget::{Budget, Source, StepClock};
        use std::sync::{Arc, Mutex};
        struct Here(Arc<Mutex<Windows>>, Arc<StepClock>);
        impl Lead for Here {
            fn take(&self, class: Class) -> std::result::Result<Given, String> {
                use crate::budget::Clock;
                Ok(self.0.lock().unwrap().take(class, self.1.now_ms()))
            }
            fn hold(&self, secs: u64) -> std::result::Result<(), String> {
                use crate::budget::Clock;
                self.0.lock().unwrap().hold(secs, self.1.now_ms());
                Ok(())
            }
            fn answers(&self, _: LeaseOp, _: &str, _: &str) -> std::result::Result<LeaseSaid, String> {
                Err("not this test's".into())
            }
        }
        let d = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0 + 3_000));
        let lead = Arc::new(Mutex::new(Windows::default()));
        let macs: Vec<Arc<Budget>> = ["m-a", "m-b", "m-c"]
            .into_iter()
            .map(|m| {
                let b = Budget::new(&store::nasfs::NasRoot::local_for_test(d.path()), m, clock.clone()).with_ledger();
                b.set_lead(Some(Arc::new(Here(lead.clone(), clock.clone()))));
                Arc::new(b)
            })
            .collect();
        let mut ts = Vec::new();
        for (i, b) in macs.iter().cycle().take(9).enumerate() {
            let b = b.clone();
            ts.push(std::thread::spawn(move || {
                for _ in 0..80 {
                    b.take(if i % 3 == 2 { Class::Query } else { Class::Wiki });
                }
            }));
        }
        ts.into_iter().for_each(|t| t.join().unwrap());
        let mut per: BTreeMap<(Class, u64), u32> = BTreeMap::new();
        for t in macs.iter().flat_map(|b| b.ledger()) {
            assert_eq!(t.source, Source::Lead);
            *per.entry((t.class, t.window)).or_default() += 1;
        }
        assert_eq!(per.values().sum::<u32>(), 720);
        for ((class, w), n) in &per {
            assert!(*n <= class.rate().tokens, "{class:?} window {w}: {n} tokens, past {}", class.rate().tokens);
        }
        assert!(!per.contains_key(&(Class::Wiki, T0 / 10_000)), "none in the window it first heard an ask in");
        // (Its clock set back into a window it gave in: none till that window ends.)
        let last = per.keys().filter(|(c, _)| *c == Class::Wiki).map(|(_, w)| *w).max().unwrap();
        let back = lead.lock().unwrap().take(Class::Wiki, (last - 1) * 10_000);
        assert_eq!(back, Given::Wait { ms: 20_000 });
        assert!(!d.path().join(crate::budget::DIR).exists(), "nothing on the NAS");
        // (Another Mac's 429 held the lead just after this Mac took a token: no Mac spends one
        // till the hold ends, this one neither.)
        use crate::budget::Clock;
        macs[0].take(Class::Wiki);
        let now = clock.now_ms();
        let until = lead.lock().unwrap().hold(25, now);
        // (A restarted lead keeps a 429's hold: the store's.)
        let d2 = tempfile::tempdir().unwrap();
        let (c, _port) = crate::coord::start_for_test(&d2.path().join("lead"), "lead", "");
        let held_to = c.budget_hold(30).unwrap();
        let docs = c.shared.lock().unwrap().keep.store().unwrap().snapshot().docs;
        assert_eq!(Small::of(&docs).windows.hold_until, held_to);
        let t = macs[0].take(Class::Wiki);
        assert!(clock.now_ms() >= until && t.window * 10_000 + 10_000 > until, "{t:?}");
    }

    /// The answers' lease: one holder; a renew only while its own and live; a lapsed one taken by
    /// another, its first holder's renew then refused; a release only by its holder.
    #[test]
    fn the_answers_lease_is_one_holders() {
        let d = tempfile::tempdir().unwrap();
        let (c, _port) = crate::coord::start_for_test(&d.path().join("lead"), "lead", "");
        let ask = |op, by: &str, t: &str| c.answers_lease(op, by, t).unwrap();
        assert!(ask(LeaseOp::Take, "m1", "a").ok);
        assert!(ask(LeaseOp::Take, "m1", "a").ok, "a repeat");
        let other = ask(LeaseOp::Take, "m4", "b");
        assert!(!other.ok && other.by == "m1");
        assert!(ask(LeaseOp::Renew, "m1", "a").ok);
        assert!(!ask(LeaseOp::Renew, "m4", "b").ok && !ask(LeaseOp::Release, "m4", "b").ok);
        assert!(ask(LeaseOp::Release, "m1", "a").ok);
        assert!(ask(LeaseOp::Take, "m4", "b").ok);
        // (Lapsed: taken by another; its first holder can't renew it, then or after.)
        c.shared.lock().unwrap().small.answers.as_mut().unwrap().until = 1;
        assert!(ask(LeaseOp::Take, "m1", "c").ok);
        assert!(!ask(LeaseOp::Renew, "m4", "b").ok);
        // (Kept in the store: a restart has it.)
        let docs = c.shared.lock().unwrap().keep.store().unwrap().snapshot().docs;
        assert_eq!(Small::of(&docs).answers.map(|l| l.token), Some("c".to_string()));
    }

    /// The gate: the budget goes by the lead only while every member heard says `budgets` (and
    /// none on an older app heartbeats on the NAS); a member's beat carries its progress, the
    /// answer the owner's releases of it; a member hearing an older lead goes back to the NAS.
    #[test]
    fn the_budget_goes_by_the_lead_only_while_every_member_says_it() {
        use crate::agent::resume::Held;
        use crate::coord::held;
        use std::sync::Arc;
        let d = tempfile::tempdir().unwrap();
        let (c, _port) = crate::coord::start_for_test(&d.path().join("lead"), "lead", "");
        let at_lead = Arc::new(beats::Beats::default());
        at_lead.set_nas_too(false);
        c.set_beats(Some(at_lead.clone()));
        let (a, b) = ("m-000000000000000a", "m-000000000000000b");
        let all = [held::SPEAKS, beats::SPEAKS, beats::VIEW, beats::ASKS, beats::HANDOFF, beats::BUDGETS];
        let beat = |m: &str, speaks: &[&str], progress: Option<Progress>| held::Beat {
            member: format!("{m}-host"),
            speaks: speaks.iter().map(|w| w.to_string()).collect(),
            heartbeat: Some(crate::agent::pool::Heartbeat {
                pool: crate::pool::Beat { member: m.into(), host: format!("{m}-host"), beat: 1, ..Default::default() },
                beat_http: true,
                ..Default::default()
            }),
            progress,
            ..Default::default()
        };
        let pass = "osm-pass 2026-10-01".to_string();
        let p = Progress {
            member: a.into(),
            worker: "a-host".into(),
            jobs: [(pass.clone(), Held { at: 100, ..Default::default() })].into(),
        };
        let ans = c.member_beat(&beat(a, &all, Some(p.clone())));
        assert!(ans.speaks.iter().any(|w| w == beats::BUDGETS) && !ans.budgets_on_nas && !c.budgets_on_nas());
        assert_eq!(c.progress().into_iter().map(|k| k.progress).collect::<Vec<_>>(), [p.clone()]);
        // (No batch while a Mac may still lease on the NAS: the member's beat not yet saying its
        // proxy takes from the lead, then the lead's own agent's not.)
        let gives = || gives(&c.shared.lock().unwrap());
        assert!(!gives());
        assert!(matches!(c.budget_take(Class::Wiki).unwrap(), Given::Wait { .. }));
        let by_lead = [&all[..], &[beats::BY_LEAD]].concat();
        c.member_beat(&beat(a, &by_lead, Some(p.clone())));
        assert!(!gives());
        c.set_budget_by_lead(true);
        assert!(gives());
        // (An older member heard: every Mac on the NAS meanwhile.)
        let old = c.member_beat(&beat(b, &all[..5], None));
        assert!(old.budgets_on_nas && c.budgets_on_nas() && !gives());
        let at_a = beats::Beats::default();
        at_a.answered(&old, 10);
        assert!(at_a.lead_budgets() && at_a.budgets_on_nas());
        // (The owner's release, in the answer to the member's next beat.)
        c.release_progress(a, &pass).unwrap();
        let ans = c.member_beat(&beat(a, &all, Some(p)));
        assert_eq!(ans.progress_released, [(pass, 100)]);
        at_a.answered(&ans, 11);
        assert_eq!(at_a.progress_released().len(), 1);
        // (An older lead: the NAS.)
        at_a.lead_older(12);
        assert!(!at_a.lead_budgets() && at_a.budgets_on_nas());
    }

    /// A member's progress kept when its beat changes it, the owner's release answered with it, and
    /// dropped once the job it names begins again.
    #[test]
    fn a_members_progress_and_its_release_are_kept() {
        use crate::agent::resume::Held;
        let d = tempfile::tempdir().unwrap();
        let (c, _port) = crate::coord::start_for_test(&d.path().join("lead"), "lead", "");
        let m = "m-000000000000000b";
        let p = |at: u64| Progress {
            member: m.into(),
            worker: "mac-b".into(),
            jobs: [("osm-pass 2026-10-01".to_string(), Held { at, ..Default::default() })].into(),
        };
        assert!(c.progress_heard(p(100)).is_empty());
        assert!(c.release_progress(m, "osm-pass 2026-09-01").is_err());
        assert!(c.release_progress(m, "osm-pass 2026-10-01").unwrap().contains("released"));
        assert!(c.release_progress(m, "osm-pass 2026-10-01").unwrap().contains("already"));
        assert_eq!(c.progress_heard(p(100)), [("osm-pass 2026-10-01".to_string(), 100)]);
        let docs = c.shared.lock().unwrap().keep.store().unwrap().snapshot().docs;
        assert_eq!(Small::of(&docs).progress[m].released.len(), 1);
        assert!(c.progress_heard(p(200)).is_empty(), "begun again: its own");
        assert_eq!(c.progress().len(), 1);
        c.progress_heard(Progress { jobs: BTreeMap::new(), ..p(0) });
        assert!(c.progress().is_empty());
        let docs = c.shared.lock().unwrap().keep.store().unwrap().snapshot().docs;
        assert!(Small::of(&docs).progress.is_empty());
    }
}
