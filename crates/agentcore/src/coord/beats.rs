//! The members' beats as this Mac hears them by HTTP (docs/coordination.md §8, step 3 of task
//! #878): one `POST /pool/beat` a member every 10 s carries its pool heartbeat
//! (crate::agent::pool::Heartbeat) and its agent's status besides the leases it holds
//! (crate::coord::held).
//!
//! - **On the lead** its coordinator keeps each member's last (`heard`), in memory, and answers
//!   with the others' heartbeats (`members`), its own among them: the members learn each other's
//!   brokers and the lead's from the lead, not the NAS. The lead's readers of a member's heartbeat
//!   (crate::agent::pool::Side::read_heartbeat) and of the members' statuses (the status's
//!   `helpers`) read these first.
//! - **On a member** the lead's answer fills the same registry (`answered`); its leases released
//!   wait for the agent's loop (`take_released`).
//! - **The words** a beat and an answer say (`SPEAKS`, `VIEW`, `ASKS`, `BUDGETS`, `BY_LEAD`): steps
//!   3, 4, 5 and 7's rollout gates read them, deleted with their NAS paths (task #1036); each is
//!   still said, for the app before, which falls back to the NAS without it. `HANDOFF`'s gate (step
//!   6, the journal) stands (`journal_on`).
//!
//! Nothing here touches the NAS: the coordinator's handlers reach it (§2.3).

use crate::agent::pool::Heartbeat;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// What a member's beat says it speaks, and a lead's answer that it takes: its heartbeat and
/// status ride in the beat.
pub const SPEAKS: &str = "beat";

/// What a member's beat says it reads: the page's data from the lead (`/pool/view`,
/// docs/coordination.md step 4), not the NAS's files.
pub const VIEW: &str = "view";

/// What a lead's answer says it serves: its members' build pages (docs/roles.md §3.3). A member's
/// port redirects its page to a lead saying it, heard within `PAGE_AWAY_S`; else (an older lead,
/// or none answering) it shows its own (crate::coord::memberpage).
pub const PAGE: &str = "page";

/// What a member's beat says, and a lead's answer: its app serves the build page at the
/// coordinator's root and its data and asks under `/api/`, and asks them there (task #1101). The
/// lead answers the old `/work/` addresses too while a member heard doesn't say it (`old_page`); a
/// member's port, while its lead doesn't (`lead_root`): crate::coord::oldpage.
pub const ROOT: &str = "page-root";

/// What a member's beat says it speaks, and a lead's answer that it serves (docs/coordination.md
/// step 5, crate::coord::asks): the lead's messages, this member's hold and the settings'
/// generation in the answer; its own mail, holds and settings by HTTP alone.
pub const ASKS: &str = "asks";

/// What a member's beat says it speaks, and a lead's answer that it takes (docs/coordination.md
/// step 6, crate::coord::handoffs): its jobs' hand-offs posted by HTTP (`POST /pool/handoff`), its
/// beat listing those it keeps, the answer saying which are merged and which the lead lacks. A
/// member whose lead doesn't say it writes them to the journal (crate::pool::driver::Route); the
/// lead lists the journal while a member heard doesn't say it (`journal_on`).
pub const HANDOFF: &str = "handoff";

/// What a member's beat says it speaks, and a lead's answer that it serves (docs/coordination.md
/// step 7, crate::coord::budgets): the Wikimedia budget and the answers' lease from the lead, its
/// resume progress in its beat.
pub const BUDGETS: &str = "budgets";

/// What a member's beat says it reads: the build's records at their one path
/// (buildstore::records::ONE, docs/coordination.md step 8b), not the term's own brief. The lead's
/// saves write that beside while a member heard doesn't say it (`per_term`).
pub const RECORDS: &str = "records";

/// What a member's beat says: its agent's proxy takes the Wikimedia budget from the lead
/// (jobrt::budget::Budget::set_lead). Said for a lead on the app before, which gives batches only
/// while every member heard says it.
pub const BY_LEAD: &str = "budgets-by-lead";

/// A lead whose last answer is older than this (s) is taken for not reachable by a member's page:
/// three of its beats (crate::agent::beat::EVERY, 10 s) missed.
pub const PAGE_AWAY_S: u64 = 35;

/// A member not heard from this long is forgotten (s, crate::agent::pool::FORGET_AFTER).
pub const FORGET_S: u64 = crate::agent::pool::FORGET_AFTER.as_secs();

/// Where a member keeps its lead's last answer's line, in its agent's folder (`LeadView`, written
/// at each answer): what `scenic menu` shows of the build (docs/roles.md §3.4), no NAS read.
pub const LEAD_VIEW: &str = "lead-view.json";

/// The lead's line as a member last heard it (`LEAD_VIEW`): when the lead last answered its beat
/// (unix s, this Mac's clock), and its line then (None: a lead before it, or one whose agent
/// hasn't said it yet).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LeadView {
    pub at: u64,
    #[serde(default)]
    pub lead: Option<super::held::LeadLine>,
}

impl LeadView {
    /// The view kept in agent folder `home` (none, or unreadable: None).
    pub fn read(home: &std::path::Path) -> Option<LeadView> {
        serde_json::from_slice(&std::fs::read(home.join(LEAD_VIEW)).ok()?).ok()
    }
}

/// One member's last beat.
#[derive(Clone, Debug)]
pub struct Heard {
    pub beat: Heartbeat,
    /// Its agent's status (crate::agent::Status, as JSON; its plan's parts left out by the member).
    pub status: Option<serde_json::Value>,
    /// When heard (this Mac's clock, unix s).
    pub at: u64,
    /// Beats heard from it since this process started.
    pub n: u64,
    /// What its beat said it speaks.
    pub speaks: Vec<String>,
}

struct Inner {
    heard: BTreeMap<String, Heard>,
    own: Option<Heartbeat>,
    lead_takes: bool,
    released: Vec<u64>,
    /// The lead's last answer's re-grants (`Answer::threads`: lease, threads), for the agent's loop
    /// to tell its jobs (`lead_threads`).
    threads: Vec<(u64, u32)>,
    /// When the lead last answered (unix s), and whether that answer said `PAGE`.
    lead_answered: Option<(u64, bool)>,
    /// Whether the lead's last answer said `ROOT`.
    lead_root: bool,
    /// The asks (`ASKS`): whether the lead serves them.
    lead_asks: bool,
    /// On a member: the lead's messages to it as its last answer carried them, the number up to
    /// which its pool side took them (its beat says it), its hold as the lead keeps it, the
    /// settings as last fetched and the generation the lead last said.
    from_lead: Vec<(u64, serde_json::Value)>,
    took: u64,
    hold: Option<jobrt::control::HoldState>,
    settings: Option<super::asks::Settings>,
    /// The settings an earlier process last had (`had_settings`), until the lead's are fetched.
    had: Option<super::asks::Settings>,
    settings_gen: u64,
    /// Its asks not yet answered 200 (`ask`), kept at `queue_at` (crate::coord::asks::QUEUE).
    queue: Vec<super::asks::Ask>,
    queue_at: Option<std::path::PathBuf>,
    /// On a member: whether its lead takes hand-offs by HTTP (`HANDOFF`; None: not answered since
    /// this process started); its hand-offs kept whole, as its pool side last saved them (its beat
    /// lists them, posting those the lead lacks); those the lead's answers said merged, for the
    /// pool side to drop.
    lead_handoffs: Option<bool>,
    handoffs: Vec<crate::pool::Entry>,
    merged: Vec<String>,
    /// On a member: its resume progress as its agent last noted it (its beat carries it), and the
    /// owner's releases of it as the lead's last answer said them.
    progress: Option<crate::agent::resume::Progress>,
    progress_released: Vec<(String, u64)>,
    /// On a member: the lead's line as its last answer carried it, and where it's kept
    /// (`keep_lead_view`: `LEAD_VIEW` in the agent's folder).
    lead_view: LeadView,
    lead_view_at: Option<std::path::PathBuf>,
    /// On a member: the term its lead's last answer said it leads (`Answer::term`), and when that
    /// answer came (unix s): its driver's step takes it while fresh (`lead_says`).
    lead_term: Option<(crate::pool::term::Term, u64)>,
}

/// The registry, shared by a member's pool side, its agent's beat and (on the lead) its coordinator.
pub struct Beats {
    inner: Mutex<Inner>,
    /// Held while the lead's line is written to its file (`save_lead_view`), apart from `inner`:
    /// a slow disk never holds up the beats.
    lead_view_write: Mutex<()>,
}

impl std::fmt::Debug for Beats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = self.inner.lock().unwrap();
        f.debug_struct("Beats").field("heard", &g.heard.len()).finish()
    }
}

impl Default for Beats {
    fn default() -> Beats {
        Beats {
            inner: Mutex::new(Inner {
                heard: BTreeMap::new(),
                own: None,
                lead_takes: false,
                released: Vec::new(),
                threads: Vec::new(),
                lead_answered: None,
                lead_root: false,
                lead_term: None,
                lead_asks: false,
                from_lead: Vec::new(),
                took: 0,
                hold: None,
                settings: None,
                had: None,
                settings_gen: 0,
                queue: Vec::new(),
                queue_at: None,
                lead_handoffs: None,
                handoffs: Vec::new(),
                merged: Vec::new(),
                progress: None,
                progress_released: Vec::new(),
                lead_view: LeadView::default(),
                lead_view_at: None,
            }),
            lead_view_write: Mutex::new(()),
        }
    }
}

impl Beats {
    /// Member `h`'s beat heard at `at`, speaking `speaks`, with its status.
    pub fn heard(&self, h: Heartbeat, speaks: Vec<String>, status: Option<serde_json::Value>, at: u64) {
        let mut g = self.inner.lock().unwrap();
        let n = g.heard.get(&h.pool.member).map_or(0, |x| x.n) + 1;
        g.heard.insert(h.pool.member.clone(), Heard { beat: h, status, at, n, speaks });
    }

    /// The members heard within `FORGET_S` of `now`, the others dropped.
    pub fn fresh(&self, now: u64) -> Vec<Heard> {
        let mut g = self.inner.lock().unwrap();
        g.heard.retain(|_, x| now.saturating_sub(x.at) < FORGET_S);
        g.heard.values().cloned().collect()
    }

    /// Member `id`'s heartbeat, this Mac's own or one heard within `FORGET_S`.
    pub fn get(&self, id: &str, now: u64) -> Option<Heartbeat> {
        let g = self.inner.lock().unwrap();
        if let Some(o) = g.own.as_ref().filter(|o| o.pool.member == id) {
            return Some(o.clone());
        }
        g.heard.get(id).filter(|x| now.saturating_sub(x.at) < FORGET_S).map(|x| x.beat.clone())
    }

    /// The heartbeats a lead answers member `to` with: its own and those heard, but `to`'s.
    pub fn members(&self, now: u64, to: &str) -> Vec<Heartbeat> {
        let own = self.own();
        self.fresh(now).into_iter().map(|x| x.beat).chain(own).filter(|h| h.pool.member != to).collect()
    }

    /// This Mac's own heartbeat, as its pool side last composed it.
    pub fn set_own(&self, h: Heartbeat) {
        self.inner.lock().unwrap().own = Some(h);
    }

    pub fn own(&self) -> Option<Heartbeat> {
        self.inner.lock().unwrap().own.clone()
    }

    /// A member's lead answered its beat at `now`: the others' heartbeats as it has them, its
    /// releases for the loop.
    pub fn answered(&self, a: &super::held::Answer, now: u64) {
        let takes = a.speaks.iter().any(|w| w == SPEAKS);
        let mut g = self.inner.lock().unwrap();
        g.lead_takes = takes;
        g.lead_answered = Some((now, a.speaks.iter().any(|w| w == PAGE)));
        g.lead_root = a.speaks.iter().any(|w| w == ROOT);
        g.lead_term = a.term.clone().map(|t| (t, now));
        if takes {
            let before = std::mem::take(&mut g.heard);
            for h in &a.members {
                let n = before.get(&h.pool.member).map_or(0, |x| x.n) + 1;
                g.heard.insert(
                    h.pool.member.clone(),
                    Heard { beat: h.clone(), status: None, at: now, n, speaks: Vec::new() },
                );
            }
        }
        g.released.extend(a.released.iter().copied());
        g.threads = a.threads.clone();
        g.lead_asks = a.speaks.iter().any(|w| w == ASKS);
        if g.lead_asks {
            g.from_lead = a.msgs.clone();
            g.hold = a.hold.clone();
            g.settings_gen = a.settings_gen;
        }
        g.lead_handoffs = Some(a.speaks.iter().any(|w| w == HANDOFF));
        g.merged.extend(a.merged.iter().cloned());
        g.progress_released = a.progress_released.clone();
        g.lead_view = LeadView { at: now, lead: a.lead.clone() };
        drop(g);
        self.save_lead_view();
    }

    /// The lead's line as its last answer carried it, and when (`LEAD_VIEW`).
    pub fn lead_view(&self) -> LeadView {
        self.inner.lock().unwrap().lead_view.clone()
    }

    /// The lead's line as it is now, written where it's kept (`keep_lead_view`), outside the
    /// registry's lock (one writer at a time, each writing the latest), and not synced: it's the
    /// menu bar's view, said again at the next answer, and a sync of the disk every beat would cost
    /// a busy Mac's builds what the menu never needs.
    fn save_lead_view(&self) {
        let _w = self.lead_view_write.lock().unwrap();
        let (view, at) = {
            let g = self.inner.lock().unwrap();
            (g.lead_view.clone(), g.lead_view_at.clone())
        };
        let Some(p) = at else { return };
        let tmp = store::whole::tmp_name(&p);
        let w = serde_json::to_vec(&view)
            .map_err(std::io::Error::from)
            .and_then(|b| std::fs::write(&tmp, b))
            .and_then(|()| store::whole::rename_over(&tmp, &p));
        if let Err(e) = w {
            std::fs::remove_file(&tmp).ok();
            eprintln!("pool: the lead's line for the menu bar, {}: {e}", p.display());
        }
    }

    /// The lead's line kept at `path` from now (`LEAD_VIEW` in the agent's folder), at each answer.
    pub fn keep_lead_view(&self, path: &std::path::Path) {
        self.inner.lock().unwrap().lead_view_at = Some(path.to_path_buf());
    }

    /// How this member's hand-offs go to its lead, as its last answer said.
    pub fn handoff_route(&self) -> crate::pool::driver::Route {
        use crate::pool::driver::Route;
        match self.inner.lock().unwrap().lead_handoffs {
            Some(true) => Route::Http,
            Some(false) => Route::Journal,
            None => Route::Wait,
        }
    }

    /// Its hand-offs kept whole, as its pool side saved them last: what its beat lists.
    pub fn set_handoffs(&self, h: Vec<crate::pool::Entry>) {
        self.inner.lock().unwrap().handoffs = h;
    }

    /// The keys of its hand-offs kept whole, while its lead takes them by HTTP.
    pub fn handoff_keys(&self) -> Vec<String> {
        let g = self.inner.lock().unwrap();
        if g.lead_handoffs != Some(true) {
            return Vec::new();
        }
        g.handoffs.iter().filter_map(crate::pool::Entry::key).collect()
    }

    /// Its hand-offs kept whole with keys `keys` (those the lead lacks).
    pub fn handoffs_of(&self, keys: &[String]) -> Vec<crate::pool::Entry> {
        let g = self.inner.lock().unwrap();
        g.handoffs.iter().filter(|e| e.key().is_some_and(|k| keys.contains(&k))).cloned().collect()
    }

    /// Those the lead's answers said merged since last taken.
    pub fn take_merged(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.lock().unwrap().merged)
    }

    /// On the lead: whether a member heard within `FORGET_S` of `now` doesn't say `HANDOFF`: it
    /// writes the journal, which the lead lists (the rollout gate of docs/coordination.md step 6).
    pub fn journal_on(&self, now: u64) -> bool {
        self.fresh(now).iter().any(|h| !h.speaks.iter().any(|w| w == HANDOFF))
    }

    /// On the lead: whether a member heard within `FORGET_S` of `now` doesn't say `RECORDS`: its
    /// readers read the term's own brief, which the lead's saves write beside the one path (the
    /// rollout gate of docs/coordination.md step 8b).
    pub fn per_term(&self, now: u64) -> bool {
        self.fresh(now).iter().any(|h| !h.speaks.iter().any(|w| w == RECORDS))
    }

    /// On the lead: whether a member heard within `FORGET_S` of `now` doesn't say `ROOT`: it may ask
    /// the build page's old addresses (crate::coord::oldpage).
    pub fn old_page(&self, now: u64) -> bool {
        self.fresh(now).iter().any(|h| !h.speaks.iter().any(|w| w == ROOT))
    }

    /// On a member: whether its lead's last answer said `ROOT` (crate::coord::oldpage).
    pub fn lead_root(&self) -> bool {
        self.inner.lock().unwrap().lead_root
    }

    /// Whether the lead serves `ASKS` (its last answer said so; the lead's own registry once its
    /// coordinator serves them: `set_lead_asks`).
    pub fn lead_asks(&self) -> bool {
        self.inner.lock().unwrap().lead_asks
    }

    /// On the lead: its coordinator serves the asks.
    pub fn set_lead_asks(&self) {
        self.inner.lock().unwrap().lead_asks = true;
    }

    /// Its resume progress as its agent noted it last: what its beat carries.
    pub fn set_progress(&self, p: crate::agent::resume::Progress) {
        self.inner.lock().unwrap().progress = Some(p);
    }

    pub fn progress(&self) -> Option<crate::agent::resume::Progress> {
        self.inner.lock().unwrap().progress.clone()
    }

    /// The owner's releases of its progress (job, when it began) as the lead's last answer said.
    pub fn progress_released(&self) -> Vec<(String, u64)> {
        self.inner.lock().unwrap().progress_released.clone()
    }

    /// Whether the lead answered a beat within `PAGE_AWAY_S` of `now`: worth posting to now.
    pub fn lead_answering(&self, now: u64) -> bool {
        self.inner.lock().unwrap().lead_answered.is_some_and(|(at, _)| now.saturating_sub(at) < PAGE_AWAY_S)
    }

    /// On a member: the term its lead's last answer said it leads, that answer within `PAGE_AWAY_S`
    /// of `now` (docs/coordination.md step 8a): its driver learns its lead and term from it, and
    /// reads the NAS's declaration and terms only while there's none (an older lead's answers, a
    /// lead not leading or its step hung, or none answering lately: the owner may have moved it).
    /// An answer timed after `now` (this Mac's clock set back since) counts as none: else it would
    /// stay fresh for as long as the clock was set back, the lead gone meanwhile.
    pub fn lead_says(&self, now: u64) -> Option<crate::pool::term::Term> {
        let g = self.inner.lock().unwrap();
        g.lead_term.as_ref().filter(|(_, at)| *at <= now && now - at < PAGE_AWAY_S).map(|(t, _)| t.clone())
    }

    /// The lead's messages to this member as its last answer carried them, after those taken.
    pub fn from_lead(&self) -> Vec<(u64, serde_json::Value)> {
        let g = self.inner.lock().unwrap();
        g.from_lead.iter().filter(|(n, _)| *n > g.took).cloned().collect()
    }

    /// The number up to which this member took the lead's messages (its pool side's mark, on its
    /// disk): its beat says it, and the lead drops them.
    pub fn set_took(&self, n: u64) {
        self.inner.lock().unwrap().took = n;
    }

    pub fn took(&self) -> u64 {
        self.inner.lock().unwrap().took
    }

    /// This member's hold as the lead keeps it (its last answer's).
    pub fn lead_hold(&self) -> Option<jobrt::control::HoldState> {
        self.inner.lock().unwrap().hold.clone()
    }

    /// The settings as last fetched, while the lead serves them.
    pub fn settings(&self) -> Option<super::asks::Settings> {
        let g = self.inner.lock().unwrap();
        g.settings.clone().filter(|_| g.lead_asks)
    }

    /// Whether the settings fetched are older than the lead's last word (or none fetched yet).
    pub fn settings_stale(&self) -> bool {
        let g = self.inner.lock().unwrap();
        g.lead_asks && g.settings.as_ref().is_none_or(|s| s.gen != g.settings_gen)
    }

    /// The settings fetched from the lead. One this member had that the lead lacks, or has only
    /// from earlier (a new lead after a move, a lead whose store was lost), is asked of it again as
    /// it was (who, when), so the lead's settings are never older than a member's; the lead's
    /// later-wins rule makes the ask harmless when another Mac set it since.
    pub fn set_settings(&self, s: super::asks::Settings) {
        let mut g = self.inner.lock().unwrap();
        let had = g.settings.take().or(g.had.take());
        let again: Vec<super::asks::Ask> = had
            .iter()
            .flat_map(|had| &had.values)
            .filter(|(name, o)| o.at > 0 && s.values.get(*name).is_none_or(|n| n.at < o.at))
            .map(|(name, o)| {
                let (value, by, at) = (o.value.clone(), o.by.clone(), o.at);
                super::asks::Ask::Setting(super::asks::SettingAsk { name: name.clone(), value, by, at })
            })
            .filter(|a| !g.queue.contains(a))
            .collect();
        g.settings = Some(s);
        if !again.is_empty() {
            eprintln!("pool: {} of the pool's settings the lead lacks, asked of it again", again.len());
            g.queue.extend(again);
            save_queue(&g);
        }
    }

    /// The settings this member last had (control::POOL_SETTINGS, from an earlier process), so a
    /// lead that lacks them is told them (`set_settings`).
    pub fn had_settings(&self, s: Option<super::asks::Settings>) {
        self.inner.lock().unwrap().had = s;
    }

    /// Its asks kept at `path` from now (those an earlier process left read back first).
    pub fn keep_asks(&self, path: &std::path::Path) {
        let mut left: Vec<super::asks::Ask> =
            std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut g = self.inner.lock().unwrap();
        left.append(&mut g.queue);
        g.queue = left;
        g.queue_at = Some(path.to_path_buf());
        save_queue(&g);
    }

    /// Queues ask `a` for the lead (on disk, when kept, before it returns).
    pub fn ask(&self, a: super::asks::Ask) {
        let mut g = self.inner.lock().unwrap();
        g.queue.push(a);
        save_queue(&g);
    }

    /// The asks queued, in order.
    pub fn asks(&self) -> Vec<super::asks::Ask> {
        self.inner.lock().unwrap().queue.clone()
    }

    /// Ask `a` answered 200: off the queue.
    pub fn asked(&self, a: &super::asks::Ask) {
        let mut g = self.inner.lock().unwrap();
        if let Some(i) = g.queue.iter().position(|x| x == a) {
            g.queue.remove(i);
            save_queue(&g);
        }
    }

    /// Whether the lead's last answer said it takes the beat (and so reads `held_age_s`).
    pub fn lead_takes(&self) -> bool {
        self.inner.lock().unwrap().lead_takes
    }

    /// Whether the lead serves this member's page at `now` (`Ok`: it answered within `PAGE_AWAY_S`,
    /// saying `PAGE`), else since when it last answered (None: not since this process started; an
    /// older lead's answers count as none: it doesn't serve the page).
    pub fn lead_page(&self, now: u64) -> Result<(), Option<u64>> {
        match self.inner.lock().unwrap().lead_answered {
            Some((at, true)) if now.saturating_sub(at) < PAGE_AWAY_S => Ok(()),
            Some((at, true)) => Err(Some(at)),
            _ => Err(None),
        }
    }

    /// An older lead's 404 at `now`: it takes neither the beat nor its leases' word. It answered,
    /// saying no line.
    pub fn lead_older(&self, now: u64) {
        let mut g = self.inner.lock().unwrap();
        g.lead_takes = false;
        g.lead_answered = None;
        g.lead_root = false;
        g.lead_term = None;
        g.lead_asks = false;
        g.lead_handoffs = Some(false);
        g.threads.clear();
        g.lead_view = LeadView { at: now, lead: None };
        drop(g);
        self.save_lead_view();
    }

    /// The re-grants the lead's last answer said (lease, threads: docs/pool.md §7.1, Threads
    /// re-granted); none from a lead before them.
    pub fn lead_threads(&self) -> Vec<(u64, u32)> {
        self.inner.lock().unwrap().threads.clone()
    }

    /// The leases the lead said this member doesn't hold, since last taken.
    pub fn take_released(&self) -> Vec<u64> {
        std::mem::take(&mut self.inner.lock().unwrap().released)
    }
}

fn save_queue(g: &Inner) {
    let Some(p) = g.queue_at.as_ref() else { return };
    let w = serde_json::to_vec(&g.queue).map_err(anyhow::Error::from).and_then(|b| store::whole::write(p, &b));
    if let Err(e) = w {
        eprintln!("pool: its asks of the lead: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::held::Answer;
    use super::*;

    #[test]
    fn a_lead_lacking_a_setting_a_member_had_is_asked_it_again() {
        use super::super::asks::{Ask, Setting, Settings};
        let set = |at, v: bool| Setting { value: serde_json::Value::Bool(v), by: "t".into(), at };
        let mut had = Settings { gen: 5, ..Default::default() };
        had.values.insert("placement".into(), set(10, true));
        had.values.insert("nas-budget".into(), Setting { value: "off".into(), by: "t".into(), at: 3 });
        let b = Beats::default();
        b.had_settings(Some(had));
        // (A new lead: placement unknown, nas-budget set later on another Mac.)
        let mut lead = Settings { gen: 1, ..Default::default() };
        lead.values.insert("nas-budget".into(), Setting { value: "on".into(), by: "u".into(), at: 4 });
        b.set_settings(lead.clone());
        let q = b.asks();
        assert_eq!(q.len(), 1, "{q:?}");
        assert!(matches!(&q[0], Ask::Setting(a) if a.name == "placement" && a.at == 10));
        b.set_settings(lead);
        assert_eq!(b.asks().len(), 1, "queued once");
    }

    fn hb(id: &str, at: u64) -> Heartbeat {
        Heartbeat {
            pool: crate::pool::Beat { member: id.into(), host: format!("{id}-host"), beat: at, ..Default::default() },
            ..Default::default()
        }
    }

    /// The lead hears two members and answers each with the other and itself; one not heard for
    /// `FORGET_S` is dropped.
    #[test]
    fn the_lead_answers_each_member_with_the_others() {
        let b = Beats::default();
        b.set_own(hb("m-lead", 100));
        b.heard(hb("m-a", 100), Vec::new(), None, 100);
        b.heard(hb("m-b", 100), Vec::new(), None, 100);
        let ids = |v: Vec<Heartbeat>| v.into_iter().map(|h| h.pool.member).collect::<Vec<_>>();
        assert_eq!(ids(b.members(110, "m-a")), ["m-b", "m-lead"]);
        assert!(b.get("m-a", 110).is_some() && b.get("m-lead", 110).is_some());
        b.heard(hb("m-b", 100 + FORGET_S), Vec::new(), None, 100 + FORGET_S);
        assert_eq!(ids(b.members(100 + FORGET_S, "m-x")), ["m-b", "m-lead"], "m-a forgotten");
        assert!(b.get("m-a", 100 + FORGET_S).is_none());
    }

    /// docs/coordination.md step 8b's gate: the lead writes the term's own brief beside the
    /// one path while a member heard within `FORGET_S` doesn't say `RECORDS`; once every member
    /// heard says it, no more; a member on an older app heard again opens it again.
    #[test]
    fn the_terms_own_records_go_on_while_a_member_heard_reads_them() {
        let b = Beats::default();
        assert!(!b.per_term(100), "no member heard");
        let all = || [SPEAKS, HANDOFF, RECORDS].map(str::to_string).to_vec();
        b.heard(hb("m-a", 100), all(), None, 100);
        b.heard(hb("m-b", 100), [SPEAKS, HANDOFF].map(str::to_string).to_vec(), None, 100);
        assert!(b.per_term(110), "m-b reads the term's own");
        b.heard(hb("m-b", 120), all(), None, 120);
        assert!(!b.per_term(130));
        b.heard(hb("m-c", 2000), vec![SPEAKS.into()], None, 2000);
        assert!(b.per_term(2000), "an older member back");
        assert!(!b.per_term(2000 + FORGET_S), "forgotten");
    }

    /// A member takes the others' heartbeats from a lead that takes the beat, and its releases
    /// from any answer.
    #[test]
    fn a_member_takes_the_others_from_its_leads_answer() {
        let b = Beats::default();
        b.answered(&Answer { epoch: 1, released: vec![7], ..Default::default() }, 10);
        assert_eq!(b.take_released(), [7]);
        assert!(b.take_released().is_empty());
        let a = Answer { epoch: 1, speaks: vec![SPEAKS.into()], members: vec![hb("m-b", 9)], ..Default::default() };
        b.answered(&a, 10);
        assert!(b.get("m-b", 10).is_some());
    }

    /// docs/coordination.md step 8a: a member takes its lead's term from the lead's answer while
    /// that answer is fresh (`PAGE_AWAY_S`); an answer without one (an older lead, or one not
    /// leading), none lately (or timed after its clock, set back), or an older lead's 404, and its
    /// driver reads the NAS.
    #[test]
    fn a_member_takes_its_lead_s_term_from_a_fresh_answer_alone() {
        let b = Beats::default();
        let t = crate::pool::term::Term {
            term: 7,
            member: "m-lead".into(),
            host: "mini".into(),
            app: "x".into(),
            since: 1,
            how: "re-asserted".into(),
            from: 6,
        };
        assert_eq!(b.lead_says(10), None, "no answer yet");
        b.answered(&Answer { speaks: vec![SPEAKS.into()], term: Some(t.clone()), ..Default::default() }, 10);
        assert_eq!(b.lead_says(10 + PAGE_AWAY_S - 1), Some(t.clone()));
        assert_eq!(b.lead_says(10 + PAGE_AWAY_S), None, "the lead not answering lately");
        assert_eq!(b.lead_says(9), None, "this Mac's clock set back past the answer");
        b.answered(&Answer { speaks: vec![SPEAKS.into()], ..Default::default() }, 20);
        assert_eq!(b.lead_says(20), None, "an answer saying no term");
        b.answered(&Answer { speaks: vec![SPEAKS.into()], term: Some(t), ..Default::default() }, 30);
        b.lead_older(31);
        assert_eq!(b.lead_says(31), None, "an older lead's 404");
    }

    /// A member's page goes to a lead that says it serves it, while it answers; an older lead's
    /// answer (no `PAGE`) or its 404 never.
    #[test]
    fn a_members_page_goes_only_to_a_lead_that_serves_it() {
        let b = Beats::default();
        assert_eq!(b.lead_page(10), Err(None));
        b.answered(&Answer { speaks: vec![SPEAKS.into()], ..Default::default() }, 10);
        assert_eq!(b.lead_page(10), Err(None), "an older lead");
        b.answered(&Answer { speaks: vec![SPEAKS.into(), PAGE.into()], ..Default::default() }, 20);
        assert_eq!(b.lead_page(20 + PAGE_AWAY_S - 1), Ok(()));
        assert_eq!(b.lead_page(20 + PAGE_AWAY_S), Err(Some(20)), "not answering");
        b.lead_older(21);
        assert_eq!(b.lead_page(21), Err(None));
    }
}
