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
//! The tokens are on the NAS, leased in batches (the share takes 20 to 55 creates a second, fewer
//! over Tailscale): time is cut into windows (`Rate::window_ms`) and a window's tokens into slots
//! of a batch each; a lease is a slot's file made with create-new
//! (`state/pool/wiki/<class>/<window>-<slot>`), so two processes, on one Mac or two, can't both
//! lease one slot, and no window gives more tokens than it has, whatever their timing. Nothing is
//! written in place, nor by two Macs (docs/pool.md §2, principle 3): each file is made once, and
//! removed once its window is long past (`KEEP`). A batch unspent at its window's end is lost (a
//! budget only ever errs below its rate). A window is a Mac's clock's: Macs a second apart may
//! spend one window's tokens over its length and a second.
//!
//! The query services are budgeted by the time their queries take as well as their count (the
//! Wikidata Query Service's limit is query time a minute): a connection to one that lasted longer
//! than a token's time owes the rest, paid before the process's next connection to them
//! (`Budget::charge`).
//!
//! A 429 or a `Retry-After` seen by any process holds every process and Mac: the process tells its
//! proxy (`/hold`, dem/budget.py), which takes every slot of the windows the wait spans, so no one
//! can lease them (`Budget::hold`). With the NAS out of reach, each Mac takes the rate divided by
//! the members it knows (`Budget::members`), in memory.
//!
//! Behind `state/pool/budgets` (crate::agent::pool::BUDGETS): off, no proxy, no sandbox, the agent
//! as it was. A clock of its own (`Clock`), so the tests decide everything without the real one.

#[cfg(not(target_os = "wasi"))]
pub mod proxy;
#[cfg(not(target_os = "wasi"))]
pub use proxy::Proxy;

/// The budgets as the status shows them (`scenic status`, the agent's `budgets`): the proxy's
/// address, the connections it has opened (metered, each class's, and the rest), the holds it was
/// told, and why the NAS was last out of reach (its share of the rate taken meanwhile).
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

/// No proxy where there are no sockets (WebAssembly's build of the agent never runs jobs).
#[cfg(target_os = "wasi")]
pub struct Proxy;

#[cfg(target_os = "wasi")]
impl Proxy {
    pub fn view(&self) -> View {
        View::default()
    }
    pub fn env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    pub fn url(&self) -> String {
        String::new()
    }
    pub fn set_members(&self, _n: usize) {}
}

/// The two-Mac test's (tools/check/pool-two-macs.sh): every host the proxy opens a connection to is
/// this address on the Mac (`127.0.0.1:<port>`), so the fake jobs' requests to Wikimedia are
/// metered by the real budget and reach no one.
pub const TEST_PROXY_TO: &str = "SCENIC_TEST_PROXY_TO";

/// The agent's proxy, taking from the pool's budget on the NAS's project folder `root` as `me`.
pub fn start_proxy(root: &Path, me: &str) -> anyhow::Result<Proxy> {
    #[cfg(not(target_os = "wasi"))]
    {
        let to: Option<std::net::SocketAddr> = std::env::var(TEST_PROXY_TO).ok().and_then(|a| a.parse().ok()).filter(|a: &std::net::SocketAddr| a.ip().is_loopback());
        let resolve: Option<proxy::Resolve> = to.map(|a| Arc::new(move |_: &str, _: u16| Some(a)) as proxy::Resolve);
        Proxy::start(Arc::new(Budget::new(root, me, Arc::new(SysClock))), resolve)
    }
    #[cfg(target_os = "wasi")]
    {
        let _ = (root, me);
        anyhow::bail!("no proxy in WebAssembly")
    }
}

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The budgets' folder on the NAS, under the project folder.
pub const DIR: &str = "state/pool/wiki";
/// How many windows behind the current one a lease's file is kept: a Mac whose clock runs behind
/// another's by less than this never leases a slot that was removed (and so leased twice).
pub const KEEP: u64 = 30;
/// The longest a 429's hold lasts (s): a `Retry-After` past it is taken as this.
pub const HOLD_MAX_S: u64 = 900;
/// How much of a hold is taken on the NAS (s): its windows' slots leased, a create each.
pub const HOLD_NAS_MAX_S: u64 = 300;
/// The most windows behind a process's last cleaning it cleans by name; past it, by a listing.
pub const CLEAN_BY_NAME: u64 = 64;
/// The hold a 429 without a `Retry-After` gives (s).
pub const HOLD_DEFAULT_S: u64 = 60;
/// The environment variable a job is told its proxy's address by (`http://127.0.0.1:<port>`), to
/// report a 429 to (`/hold`; dem/budget.py).
pub const PROXY_ENV: &str = "SCENIC_BUDGETS";
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
    /// The tokens a lease takes (a window has `tokens / batch` slots).
    pub batch: u32,
    /// A connection's time a token pays for (ms; 0: count alone): one lasting longer owes the rest.
    pub per_token_ms: u64,
}

impl Rate {
    pub fn slots(&self) -> u32 {
        (self.tokens / self.batch).max(1)
    }
}

impl Class {
    pub const ALL: [Class; 2] = [Class::Wiki, Class::Query];

    /// Its name, as its folder on the NAS.
    pub fn name(self) -> &'static str {
        match self {
            Class::Wiki => "wiki",
            Class::Query => "query",
        }
    }

    /// The pool's rate, the politeness of one client (the owner's policy, not a step's figure):
    /// Wikimedia's sites two requests a second, serially as their etiquette asks; the query
    /// services one query in two seconds, and two seconds of query time a token (so at most half
    /// of the Wikidata Query Service's minute of query time a minute).
    pub fn rate(self) -> Rate {
        match self {
            Class::Wiki => Rate { window_ms: 10_000, tokens: 20, batch: 4, per_token_ms: 0 },
            Class::Query => Rate { window_ms: 10_000, tokens: 5, batch: 1, per_token_ms: 2_000 },
        }
    }
}

/// The query services' hosts, by name (WDQS's), metered by count and query time (`Class::Query`).
pub const QUERY_HOSTS: [&str; 3] = ["query.wikidata.org", "query-main.wikidata.org", "query-scholarly.wikidata.org"];
/// The query services' domains, with their subdomains (QLever's, which heritagewd.py asks).
pub const QUERY_DOMAINS: [&str; 2] = ["qlever.dev", "qlever.cs.uni-freiburg.de"];
/// Wikimedia's sites' domains, with their subdomains (`Class::Wiki`). dem/budget.py keeps the same
/// lists (tests/budgets.rs checks that they're the same).
pub const WIKIMEDIA: [&str; 13] = ["wikipedia.org", "wikidata.org", "wikimedia.org", "wikimediafoundation.org", "mediawiki.org", "wiktionary.org", "wikibooks.org", "wikinews.org", "wikiquote.org", "wikisource.org", "wikiversity.org", "wikivoyage.org", "wmflabs.org"];

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

/// Where a token came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A lease on the NAS.
    Nas,
    /// This Mac's share of the rate, the NAS out of reach.
    Alone,
}

/// A token given: its class, its window, where it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub class: Class,
    pub window: u64,
    pub source: Source,
}

/// What a lease of a window's slot found.
enum Leased {
    /// This slot is ours.
    Slot,
    /// Every slot of the window is taken.
    Full,
    /// The NAS can't be reached (or written): the error.
    Away(io::Error),
}

/// A class's state in this process.
#[derive(Default)]
struct Held {
    /// The window of the batch held, and the tokens left of it.
    window: u64,
    left: u32,
    /// Tokens owed (a query's time past its token's).
    owed: u32,
    /// The windows below which every lease's file has been removed (by this process's reckoning).
    cleaned: Option<u64>,
    /// Tokens given in each window with the NAS away (this Mac's share).
    alone: BTreeMap<u64, u32>,
}

#[derive(Default)]
struct State {
    held: BTreeMap<Class, Held>,
    /// Every process here waits until then (ms): a 429 seen.
    hold_until: u64,
    /// What was given, kept for the tests (`Budget::ledger`).
    ledger: Option<Vec<Token>>,
    /// Why the NAS was last out of reach, for the status.
    away: Option<String>,
}

/// The pool's budget, as one process (an agent's proxy) takes from it.
pub struct Budget {
    /// The NAS's project folder.
    root: PathBuf,
    /// Who leases (a member id, or the host's name): written in each lease, for whoever looks.
    me: String,
    clock: Arc<dyn Clock>,
    /// The members this Mac knows (itself among them), the NAS away: each takes the rate divided by
    /// them.
    pub members: Arc<AtomicUsize>,
    st: Mutex<State>,
    /// Held by the one thread leasing (or holding on the NAS) at a time: never with `st`.
    leasing: Mutex<()>,
}

impl Budget {
    pub fn new(root: &Path, me: &str, clock: Arc<dyn Clock>) -> Budget {
        Budget { root: root.to_path_buf(), me: me.to_string(), clock, members: Arc::new(AtomicUsize::new(1)), st: Mutex::new(State::default()), leasing: Mutex::new(()) }
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

    /// Why the NAS was last out of reach (None: it answered since).
    pub fn away(&self) -> Option<String> {
        self.state().away.clone()
    }

    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// The folder of `class`'s leases.
    pub fn dir(&self, class: Class) -> PathBuf {
        self.root.join(DIR).join(class.name())
    }

    /// A token of `class`, waiting for one as long as it takes (with what it owes paid first). The
    /// state's lock is never held across the NAS's I/O (a status or a connection not metered never
    /// waits on the share); one thread leases at a time (`leasing`), the others waiting for its batch.
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
                let w = now / rate.window_ms;
                if let Some(t) = Self::from_batch(&mut st, class, w) {
                    return t;
                }
                w
            };
            let leasing = self.leasing.lock().unwrap_or_else(|e| e.into_inner());
            // (Another thread may have leased while this one waited: its batch first.)
            {
                let st = self.state();
                if self.clock.now_ms() < st.hold_until || st.held.get(&class).is_some_and(|h| h.window == w && h.left > 0) {
                    continue;
                }
            }
            match self.lease(class, w) {
                Leased::Slot => {
                    let cleaned = {
                        let mut st = self.state();
                        st.away = None;
                        let h = st.held.entry(class).or_default();
                        h.window = w;
                        h.left = rate.batch;
                        h.cleaned
                    };
                    let next = self.clean(class, w, cleaned);
                    self.state().held.entry(class).or_default().cleaned = Some(next);
                }
                Leased::Full => {
                    drop(leasing);
                    self.clock.sleep_until_ms((w + 1) * rate.window_ms);
                }
                Leased::Away(e) => {
                    let mut st = self.state();
                    st.away = Some(format!("{}: {e}", self.dir(class).display()));
                    // (This Mac's share, in memory: the rate divided by the members it knows.)
                    let n = self.members.load(Ordering::Relaxed).max(1) as u32;
                    let share = (rate.tokens / n).max(1);
                    let h = st.held.entry(class).or_default();
                    h.alone.retain(|k, _| *k + KEEP >= w);
                    let given = h.alone.entry(w).or_default();
                    if *given < share {
                        // (A token at a time from this Mac's share, what's owed first.)
                        *given += 1;
                        if h.owed > 0 {
                            h.owed -= 1;
                            continue;
                        }
                        let t = Token { class, window: w, source: Source::Alone };
                        if let Some(l) = st.ledger.as_mut() {
                            l.push(t);
                        }
                        return t;
                    }
                    drop(st);
                    drop(leasing);
                    self.clock.sleep_until_ms((w + 1) * rate.window_ms);
                }
            }
        }
    }

    /// A token from the batch `class` holds for window `w` (what's owed paid from it first), if it
    /// has one.
    fn from_batch(st: &mut State, class: Class, w: u64) -> Option<Token> {
        let h = st.held.entry(class).or_default();
        if h.window != w {
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
        let t = Token { class, window: w, source: Source::Nas };
        if let Some(l) = st.ledger.as_mut() {
            l.push(t);
        }
        Some(t)
    }

    /// A slot of `class`'s window `w`, leased by create-new.
    fn lease(&self, class: Class, w: u64) -> Leased {
        let dir = self.dir(class);
        let mut made = false;
        for slot in 0..class.rate().slots() {
            match create_new(&dir.join(format!("{w}-{slot}")), self.me.as_bytes()) {
                Ok(()) => return Leased::Slot,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                // (Its folder not made yet: made once, the NAS's project folder there.)
                Err(e) if e.kind() == io::ErrorKind::NotFound && !made && self.root.is_dir() => {
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        return Leased::Away(e);
                    }
                    made = true;
                    match create_new(&dir.join(format!("{w}-{slot}")), self.me.as_bytes()) {
                        Ok(()) => return Leased::Slot,
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Leased::Away(e),
                    }
                }
                Err(e) => return Leased::Away(e),
            }
        }
        Leased::Full
    }

    /// The leases' files of the windows long past (`KEEP` behind `w`) that this process hasn't
    /// removed yet (from `cleaned`), by their names: no listing (docs/pool.md §3). After a gap past
    /// `CLEAN_BY_NAME` windows (this process's first lease, or one after a long idle), by one
    /// listing of the class's folder instead: the windows no process leased in can't be known by
    /// name without walking every one of them, and files another Mac leased while this one was idle
    /// may lie anywhere in the gap. Where it has reached.
    fn clean(&self, class: Class, w: u64, cleaned: Option<u64>) -> u64 {
        let to = w.saturating_sub(KEEP);
        let dir = self.dir(class);
        match cleaned {
            Some(from) if to.saturating_sub(from) <= CLEAN_BY_NAME => {
                for old in from..to {
                    for slot in 0..class.rate().slots() {
                        std::fs::remove_file(dir.join(format!("{old}-{slot}"))).ok();
                    }
                }
                to.max(from)
            }
            _ => {
                for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.split_once('-').and_then(|(win, _)| win.parse::<u64>().ok()).is_some_and(|win| win < to) {
                        std::fs::remove_file(e.path()).ok();
                    }
                }
                to
            }
        }
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
    /// ends (`hold_here`), and on the NAS (`hold_nas`), so every other process and Mac waits too.
    /// Whether the NAS took it (false: held here alone, the NAS out of reach).
    pub fn hold(&self, secs: u64) -> bool {
        let (now, until) = self.hold_here(secs);
        self.hold_nas(now, until)
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

    /// The hold from `now` to `until` (ms) on the NAS: every slot of each window it spans leased,
    /// up to `HOLD_NAS_MAX_S` of it (a hold's cost on the share: a create a slot, ten slots a
    /// window across the classes, so 300 creates at most, some 6 to 15 s on the share; past that,
    /// the other Macs, if they're still asking, meet their own 429). Whether the NAS took it.
    pub fn hold_nas(&self, now: u64, until: u64) -> bool {
        let until = until.min(now + HOLD_NAS_MAX_S * 1000);
        let _leasing = self.leasing.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = true;
        for class in Class::ALL {
            let r = class.rate();
            for w in now / r.window_ms..=(until.max(now + 1) - 1) / r.window_ms {
                // (Each slot taken in turn till the window's full; a lease made elsewhere meanwhile
                // is as good.)
                loop {
                    match self.lease(class, w) {
                        Leased::Slot => continue,
                        Leased::Full => break,
                        Leased::Away(_) => {
                            all = false;
                            break;
                        }
                    }
                }
            }
        }
        all
    }

    /// Until when this process holds (ms; 0: none).
    pub fn held_until(&self) -> u64 {
        self.state().hold_until
    }
}

/// Makes `p` with `b` in it, only if it isn't there (create-new: atomic on the share for one
/// client, and between two, as crate::agent::claims relies on).
fn create_new(p: &Path, b: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::options().write(true).create_new(true).open(p)?;
    f.write_all(b)
}

#[cfg(test)]
mod tests;
