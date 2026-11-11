//! The pool's one budget for Wikimedia (docs/pool.md §7.7, §12 phase 4's seventh batch): every
//! request a job makes to Wikimedia's sites, and to the query services that answer Wikidata's
//! queries, is taken from one budget for the whole pool, whichever Mac and process makes it, so the
//! pool together is never past the rate one polite client keeps (no Mac can reliably know the
//! address its requests leave from: a Tailscale exit node, IPv4 or IPv6; so one budget, not one an
//! address). The budget is enforced at egress, not by the steps: a job reaches the internet only
//! through its agent's proxy (`proxy`: `HTTPS_PROXY` set for every job, the job run under a sandbox
//! profile that denies it every outbound connection but to the Mac itself, `SANDBOX`), which takes a
//! token for each connection to a metered host before it opens it.
//!
//! Time is cut into windows (`Rate::window_ms`), each with its tokens (`Rate::tokens`), taken in
//! batches (`Given::Batch`) from the lead (`Lead`, docs/coordination.md step 7:
//! `POST /pool/budget/take`), which counts each window's tokens in memory by its own clock alone,
//! giving one a take: no Mac holds tokens it hasn't spent, so a 429's hold stops every Mac at its
//! next token. A batch is spendable until the lead's window ends, counted on this Mac from when it
//! asked (`Given::Batch::for_ms`), so it is never spent past its window whatever the Macs' clocks
//! say. The agent gives its proxy the lead (`Budget::set_lead`). The lead is the one source: out of
//! reach, or not given yet, this Mac takes nothing, asking again each window. (A share of its own
//! meanwhile would add to what the lead gives the Macs that reach it: a Mac away over Tailscale, or
//! whose asks time out, while the others are answered.)
//!
//! A batch unspent at its window's end is lost (a budget only ever errs below its rate).
//!
//! The query services are budgeted by the time their queries take as well as their count (the
//! Wikidata Query Service's limit is query time a minute): a connection to one that lasted longer
//! than a token's time owes the rest, paid before the process's next connection to them
//! (`Budget::charge`).
//!
//! A 429 or a `Retry-After` seen by any process holds every process and Mac: the process tells its
//! proxy (`/hold`: `seen`), which tells the lead (every class's tokens given to none till it ends,
//! `Budget::hold`); with the lead out of reach, this Mac alone holds.
//!
//! A change of lead (`Budget::set_lead`) drops the batch held.
//!
//! The answers' lease (agentcore::answers) is the lead's too: a job asks its proxy
//! (`/lease/answers`), which asks the lead (`Budget::answers`); with no lead given yet, or none
//! answering, the proxy says so (503) and the job asks again.
//!
//! A clock of its own (`Clock`), so the tests decide everything without the real one.

pub mod nas;
pub mod net;
pub mod proxy;
pub mod snmp;
pub use proxy::Proxy;

/// The budgets as the status shows them (`scenic status`, the agent's `budgets`): the proxy's
/// address, the connections it has opened (metered, each class's, and the rest), the holds it was
/// told, and why the lead was last out of reach (nothing taken meanwhile).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub proxy: String,
    pub wiki: u64,
    pub query: u64,
    pub other: u64,
    pub holds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_until_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away: Option<String>,
}

/// The two-Mac test's (tools/check/pool-two-macs.sh): every host the proxy opens a connection to is
/// this address on the Mac (`127.0.0.1:<port>`), so the fake jobs' requests to Wikimedia are
/// metered by the real budget and reach no one.
pub const TEST_PROXY_TO: &str = "SCENIC_TEST_PROXY_TO";

#[cfg(any(test, feature = "testkit"))]
thread_local! {
    /// The unit tests' `TEST_PROXY_TO`: where the hosts of the proxies this thread starts are.
    pub static TEST_RESOLVE: std::cell::RefCell<Option<proxy::Resolve>> = const { std::cell::RefCell::new(None) };
}

/// proxy::listen.
pub fn listen(port: u16) -> std::io::Result<std::net::TcpListener> {
    proxy::listen(port)
}

/// The agent's proxy, taking from the pool's budget as `me` (its lead's, once given:
/// `Budget::set_lead`), serving `listener` (proxy::listen's).
pub fn start_proxy(me: &str, listener: std::net::TcpListener) -> anyhow::Result<Proxy> {
    let to: Option<std::net::SocketAddr> = std::env::var(TEST_PROXY_TO)
        .ok()
        .and_then(|a| a.parse().ok())
        .filter(|a: &std::net::SocketAddr| a.ip().is_loopback());
    let resolve: Option<proxy::Resolve> = to.map(|a| Arc::new(move |_: &str, _: u16| Some(a)) as proxy::Resolve);
    #[cfg(any(test, feature = "testkit"))]
    let resolve = resolve.or_else(|| TEST_RESOLVE.with(|r| r.borrow().clone()));
    Proxy::start_listening(Arc::new(Budget::new(me, Arc::new(SysClock))), resolve, listener)
}

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The longest a 429's hold lasts (s): a `Retry-After` past it is taken as this.
pub const HOLD_MAX_S: u64 = 900;
/// The hold a 429 without a `Retry-After` gives (s).
pub const HOLD_DEFAULT_S: u64 = 60;
/// The environment variable a job is told its proxy's address by (`http://127.0.0.1:<port>`), to
/// report a 429 to (`/hold`: `seen`).
pub const PROXY_ENV: &str = "SCENIC_BUDGETS";

/// A step's answer from `url`: a 429, or a `Retry-After`, from a metered host told to its proxy
/// (`/hold`), so the budget holds every process and Mac that long (`HOLD_DEFAULT_S` when it says
/// none). Whether a proxy was told (false: not metered, nothing to tell, the budgets off, or the
/// proxy didn't answer: the step's own wait is all).
pub fn seen(url: &str, status: u16, retry_after: Option<&str>) -> bool {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', '?', ':']).next().unwrap_or("");
    if class_of(host).is_none() || (status != 429 && retry_after.is_none_or(|r| r.trim().is_empty())) {
        return false;
    }
    tell(&format!("/hold?s={}&host={host}", hold_s(retry_after)))
}

/// How long a refusal saying `retry_after` holds the pool (s): its seconds (at least one, at most
/// `HOLD_MAX_S`), `HOLD_DEFAULT_S` when it says none (or a date). A step waiting out a 429 waits
/// this long, the hold `seen` told the same.
pub fn hold_s(retry_after: Option<&str>) -> u64 {
    let s = retry_after.and_then(|r| r.trim().parse::<f64>().ok()).filter(|s| s.is_finite());
    s.map_or(HOLD_DEFAULT_S, |s| s.max(1.0).ceil() as u64).clamp(1, HOLD_MAX_S)
}

/// A process's request to `url` is asked again (crate::fetch's retries): told to its proxy, which
/// counts it for the host (crate::budget::net); false when there's none (not a job).
pub fn retried(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', '?', ':']).next().unwrap_or("");
    tell(&format!("/retry?host={host}"))
}

/// `path` posted to this process's proxy (`PROXY_ENV`), straight (not through itself).
fn tell(path: &str) -> bool {
    let Ok(proxy) = std::env::var(PROXY_ENV) else { return false };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();
    agent.post(&format!("{}{path}", proxy.trim_end_matches('/'))).send_empty().is_ok()
}
/// The sandbox profile every job runs under with the budgets on (`sandbox-exec -p`): every outbound
/// connection denied but to the Mac itself (its proxy, its coordinator) and its local sockets (the
/// name service's), so a job that goes around the proxy fails at once, never reaching Wikimedia
/// unmetered. The NAS is the kernel's (SMB), not the job's sockets: unaffected.
pub const SANDBOX: &str = "(version 1)\n(allow default)\n(deny network-outbound)\n(allow network-outbound (remote ip \"localhost:*\"))\n(allow network-outbound (remote unix-socket))\n";

/// What a budget meters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Wikimedia's sites: Wikipedia, Wikidata's API and pages, Commons, the dumps.
    Wiki,
    /// The query services answering Wikidata's queries (WDQS, QLever's): by count and query time.
    Query,
}

/// A class's rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    /// A window's length (ms).
    pub window_ms: u64,
    /// The tokens a window has, the pool's whole.
    pub tokens: u32,
    /// A connection's time a token pays for (ms; 0: count alone): one lasting longer owes the rest.
    pub per_token_ms: u64,
}

impl Class {
    pub const ALL: [Class; 2] = [Class::Wiki, Class::Query];

    /// Its name, in a take of the lead.
    pub fn name(self) -> &'static str {
        match self {
            Class::Wiki => "wiki",
            Class::Query => "query",
        }
    }

    /// The class named `name` (`name`'s).
    pub fn named(name: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The pool's rate, the politeness of one client (the owner's policy, not a step's figure):
    /// Wikimedia's sites four requests a second (the owner's choice, 14 Oct 2026); the query
    /// services one query in two seconds, and two seconds of query time a token (so at most half
    /// of the Wikidata Query Service's minute of query time a minute).
    pub const fn rate(self) -> Rate {
        match self {
            Class::Wiki => Rate { window_ms: 10_000, tokens: 40, per_token_ms: 0 },
            Class::Query => Rate { window_ms: 10_000, tokens: 5, per_token_ms: 2_000 },
        }
    }
}

/// The query services' hosts, by name (WDQS's), metered by count and query time (`Class::Query`).
pub const QUERY_HOSTS: [&str; 3] = ["query.wikidata.org", "query-main.wikidata.org", "query-scholarly.wikidata.org"];
/// The query services' domains, with their subdomains (QLever's, which the heritage chain asks).
pub const QUERY_DOMAINS: [&str; 2] = ["qlever.dev", "qlever.cs.uni-freiburg.de"];
/// Wikimedia's sites' domains, with their subdomains (`Class::Wiki`).
pub const WIKIMEDIA: [&str; 13] = [
    "wikipedia.org",
    "wikidata.org",
    "wikimedia.org",
    "wikimediafoundation.org",
    "mediawiki.org",
    "wiktionary.org",
    "wikibooks.org",
    "wikinews.org",
    "wikiquote.org",
    "wikisource.org",
    "wikiversity.org",
    "wikivoyage.org",
    "wmflabs.org",
];

/// The class a host's connections are metered by (None: not metered).
pub fn class_of(host: &str) -> Option<Class> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let h = h.trim_start_matches('[').trim_end_matches(']');
    let under = |d: &str| h == d || h.ends_with(&format!(".{d}"));
    if QUERY_HOSTS.contains(&h) || QUERY_DOMAINS.iter().any(|d| under(d)) {
        return Some(Class::Query);
    }
    WIKIMEDIA.iter().any(|d| under(d)).then_some(Class::Wiki)
}

/// The time a budget goes by.
pub trait Clock: Send + Sync {
    /// Milliseconds since the epoch.
    fn now_ms(&self) -> u64;
    /// Waits until `t` (ms since the epoch).
    fn sleep_until_ms(&self, t: u64);
}

/// The Mac's clock.
pub struct SysClock;

impl Clock for SysClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    fn sleep_until_ms(&self, t: u64) {
        let now = self.now_ms();
        if t > now {
            std::thread::sleep(std::time::Duration::from_millis(t - now));
        }
    }
}

/// A clock that moves only when it's waited on: a wait moves it to its end at once (the tests'; no
/// real time passes, so nothing depends on the machine's speed or load).
#[derive(Default)]
pub struct StepClock(Mutex<u64>);

impl StepClock {
    pub fn at(t: u64) -> StepClock {
        StepClock(Mutex::new(t))
    }
    pub fn set(&self, t: u64) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *g = (*g).max(t);
    }
}

impl Clock for StepClock {
    fn now_ms(&self) -> u64 {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn sleep_until_ms(&self, t: u64) {
        self.set(t);
    }
}

/// The lead's answer to a take (`POST /pool/budget/take {class}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Given {
    /// A batch of `n` tokens of the lead's window `window`, spendable for `for_ms` from the ask:
    /// what is left of that window on the lead's clock, so counted from the ask the batch's time
    /// ends no later than the window does, however late the answer came.
    Batch { n: u32, window: u64, for_ms: u64 },
    /// None now (the window's tokens all given, or the pool held by a 429): ask again in `ms`.
    Wait { ms: u64 },
}

/// What is asked of the answers' lease (`POST /pool/lease/answers`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeaseOp {
    /// Taken by this holder: when no one holds it, it lapsed, or it's this holder's already.
    Take,
    /// Renewed: only while it's this holder's and hasn't lapsed (a holder whose lease lapsed may
    /// have been followed by another's sync: it never writes on it again).
    Renew,
    /// Given up: removed only while it's this holder's.
    Release,
}

impl LeaseOp {
    pub fn named(name: &str) -> Option<LeaseOp> {
        match name {
            "take" => Some(LeaseOp::Take),
            "renew" => Some(LeaseOp::Renew),
            "release" => Some(LeaseOp::Release),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LeaseOp::Take => "take",
            LeaseOp::Renew => "renew",
            LeaseOp::Release => "release",
        }
    }
}

/// The lead's answer about the answers' lease: whether the ask was done (`ok`), and who holds it
/// now and until when (unix s; none: "" and 0).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeaseSaid {
    pub ok: bool,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub until: u64,
}

/// The lead as the budget's source (docs/coordination.md step 7): its coordinator over HTTP on a
/// member, in process on the lead. An error: the lead out of reach (why).
pub trait Lead: Send + Sync {
    /// A batch of `class`'s tokens, or how long to wait for one.
    fn take(&self, class: Class) -> Result<Given, String>;
    /// Every class held `secs` from now (a 429 seen).
    fn hold(&self, secs: u64) -> Result<(), String>;
    /// The answers' lease asked `op` by holder `token` (`by`: who, for whoever looks).
    fn answers(&self, op: LeaseOp, by: &str, token: &str) -> Result<LeaseSaid, String>;
}

/// A token given: its class, and the lead's window it was given in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub class: Class,
    pub window: u64,
}

/// What an ask of the lead found.
enum Got {
    /// A batch: its window, until when it may be spent (ms, this Mac's clock), its tokens.
    Batch { window: u64, until: u64, n: u32 },
    /// None until then (ms).
    Wait(u64),
}

/// A class's state in this process.
#[derive(Default)]
struct Held {
    /// The lead's window of the batch held, until when it may be spent (ms, this Mac's clock), and
    /// the tokens left of it.
    window: u64,
    until: u64,
    left: u32,
    /// Tokens owed (a query's time past its token's).
    owed: u32,
}

#[derive(Default)]
struct State {
    held: BTreeMap<Class, Held>,
    /// Every process here waits until then (ms): a 429 seen.
    hold_until: u64,
    /// What was given, kept for the tests (`Budget::ledger`).
    ledger: Option<Vec<Token>>,
    /// Why the lead was last out of reach, for the status.
    away: Option<String>,
    /// The lead not asked again before then (ms): it didn't answer in this window.
    lead_away_until: u64,
}

/// The pool's budget, as one process (an agent's proxy) takes from it.
pub struct Budget {
    /// Who asks (a member id, or the host's name): named in the answers' lease, for whoever looks.
    me: String,
    clock: Arc<dyn Clock>,
    /// The lead it takes from (`set_lead`); None: none given yet (nothing taken).
    lead: Mutex<Option<Arc<dyn Lead>>>,
    st: Mutex<State>,
    /// Held by the one thread asking the lead at a time: never with `st`.
    leasing: Mutex<()>,
}

impl Budget {
    pub fn new(me: &str, clock: Arc<dyn Clock>) -> Budget {
        Budget {
            me: me.to_string(),
            clock,
            lead: Mutex::new(None),
            st: Mutex::new(State::default()),
            leasing: Mutex::new(()),
        }
    }

    /// One that keeps what it gives (`ledger`): the tests'.
    pub fn with_ledger(self) -> Budget {
        self.state().ledger = Some(Vec::new());
        self
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What it has given (with `with_ledger`).
    pub fn ledger(&self) -> Vec<Token> {
        self.state().ledger.clone().unwrap_or_default()
    }

    /// Takes from `lead` from now (None: none, nothing taken): the agent's word. The batch held is
    /// dropped (it's the other lead's).
    pub fn set_lead(&self, lead: Option<Arc<dyn Lead>>) {
        *self.lead.lock().unwrap_or_else(|e| e.into_inner()) = lead;
        let mut st = self.state();
        st.lead_away_until = 0;
        for h in st.held.values_mut() {
            h.left = 0;
        }
    }

    fn lead(&self) -> Option<Arc<dyn Lead>> {
        self.lead.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Who asks (a member id, or the host's name).
    pub fn me(&self) -> &str {
        &self.me
    }

    /// The answers' lease asked `op` of the lead by holder `token` (`by`); an error: no lead given
    /// yet, or it didn't answer (why).
    pub fn answers(&self, op: LeaseOp, by: &str, token: &str) -> Result<LeaseSaid, String> {
        let lead = self.lead().ok_or_else(|| "no lead known yet".to_string())?;
        lead.answers(op, by, token)
    }

    /// Why the lead was last out of reach (None: it answered since).
    pub fn away(&self) -> Option<String> {
        self.state().away.clone()
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// A token of `class`, waiting for one as long as it takes (with what it owes paid first). The
    /// state's lock is never held across the lead's I/O (a status or a connection not metered never
    /// waits on it); one thread asks at a time (`leasing`), the others waiting for its batch.
    pub fn take(&self, class: Class) -> Token {
        let rate = class.rate();
        loop {
            // (From the batch held, when it has tokens: no I/O.)
            let w = {
                let mut st = self.state();
                let now = self.clock.now_ms();
                if now < st.hold_until {
                    let until = st.hold_until;
                    drop(st);
                    self.clock.sleep_until_ms(until);
                    continue;
                }
                if let Some(t) = Self::from_batch(&mut st, class, now) {
                    return t;
                }
                now / rate.window_ms
            };
            let leasing = self.leasing.lock().unwrap_or_else(|e| e.into_inner());
            // (Another thread may have leased while this one waited: its batch first.)
            let asked = {
                let st = self.state();
                let now = self.clock.now_ms();
                if now < st.hold_until || st.held.get(&class).is_some_and(|h| now < h.until && h.left > 0) {
                    continue;
                }
                now
            };
            let away_until = self.state().lead_away_until;
            let got = match self.lead() {
                // (The lead didn't answer in this window: none till the next.)
                Some(_) if asked < away_until => Got::Wait(away_until),
                Some(l) => match l.take(class) {
                    Ok(Given::Batch { n, window, for_ms }) => Got::Batch { window, until: asked + for_ms, n },
                    Ok(Given::Wait { ms }) => Got::Wait(asked + ms.max(1)),
                    Err(e) => self.lead_away(w, rate, format!("the lead: {e}")),
                },
                // (No lead given yet, as the agent starts: none till it is.)
                None => self.lead_away(w, rate, "no lead known yet".into()),
            };
            match got {
                Got::Batch { window, until, n } => {
                    let mut st = self.state();
                    st.away = None;
                    let h = st.held.entry(class).or_default();
                    (h.window, h.until, h.left) = (window, until, n);
                }
                Got::Wait(until) => {
                    drop(leasing);
                    self.clock.sleep_until_ms(until);
                }
            }
        }
    }

    /// The lead out of reach in window `w` (why): none from anywhere else, the lead being the one
    /// source, and not asked again this window (each ask of a lead that's down may wait out its
    /// connect).
    fn lead_away(&self, w: u64, rate: Rate, why: String) -> Got {
        let mut st = self.state();
        st.lead_away_until = (w + 1) * rate.window_ms;
        st.away = Some(why);
        Got::Wait(st.lead_away_until)
    }

    /// A token from the batch `class` holds, while it may be spent at `now` (what's owed paid from
    /// it first), if it has one.
    fn from_batch(st: &mut State, class: Class, now: u64) -> Option<Token> {
        let h = st.held.entry(class).or_default();
        if now >= h.until {
            h.left = 0;
        }
        while h.left > 0 && h.owed > 0 {
            h.left -= 1;
            h.owed -= 1;
        }
        if h.left == 0 {
            return None;
        }
        h.left -= 1;
        let t = Token { class, window: h.window };
        if let Some(l) = st.ledger.as_mut() {
            l.push(t);
        }
        Some(t)
    }

    /// A connection of `class` lasted `ms`: what it took past its token's time is owed, paid before
    /// this process's next connection of the class.
    pub fn charge(&self, class: Class, ms: u64) {
        let r = class.rate();
        if r.per_token_ms == 0 || ms <= r.per_token_ms {
            return;
        }
        let owed = ms.div_ceil(r.per_token_ms) - 1;
        let mut st = self.state();
        let h = st.held.entry(class).or_default();
        h.owed = h.owed.saturating_add(owed.min(u32::MAX as u64) as u32);
    }

    /// A 429 or a `Retry-After` of `secs` (capped at `HOLD_MAX_S`): every class held here until it
    /// ends (`hold_here`), and by the lead (`hold_pool`), so every other process and Mac waits too.
    /// Whether the pool took it (false: held here alone, the lead out of reach).
    pub fn hold(&self, secs: u64) -> bool {
        let (now, until) = self.hold_here(secs);
        self.hold_pool(now, until)
    }

    /// The hold from `now` to `until` (ms) told the lead. Whether it was.
    pub fn hold_pool(&self, now: u64, until: u64) -> bool {
        let told = match self.lead() {
            Some(l) => l.hold(until.saturating_sub(now).div_ceil(1000).max(1)),
            None => Err("no lead known yet".into()),
        };
        if let Err(e) = &told {
            eprintln!("budgets: a 429's hold not told the lead ({e}): held on this Mac alone");
        }
        told.is_ok()
    }

    /// The hold in this process: until `secs` from now (capped); (now, its end) in ms.
    pub fn hold_here(&self, secs: u64) -> (u64, u64) {
        let secs = secs.clamp(1, HOLD_MAX_S);
        let now = self.clock.now_ms();
        let until = now + secs * 1000;
        let mut st = self.state();
        st.hold_until = st.hold_until.max(until);
        for h in st.held.values_mut() {
            h.left = 0;
        }
        (now, until)
    }

    /// Until when this process holds (ms; 0: none).
    pub fn held_until(&self) -> u64 {
        self.state().hold_until
    }
}

#[cfg(test)]
mod tests;
