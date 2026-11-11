//! **dsm**, the agent's part that drops this Mac's own stale SMB sessions on the NAS through DSM's
//! web API (task #567; docs/macs.md, Stale SMB handles; docs/pool.md §3). A Mac that dies, or
//! leaves home, with the share mounted keeps a session on the NAS until the NAS times it out
//! (minutes to hours), and its leases block the other Macs' deletes and reads meanwhile (11 Nov:
//! GC sat 63 minutes in one unlink on the mini as the M4 left).
//!
//! The API (DSM 7.2, verified against the NAS, 11 Nov): `SYNO.API.Auth` `login` (version 6,
//! `format=sid`) and `logout`; `SYNO.Core.CurrentConnection` version 1 `list` (a row per client
//! connection: `protocol` "SMB", `type` "SMB3", `who` the user, `from` the client's address,
//! `descr` the shares it has connected ("home, personal": a Mac's mounts share one connection),
//! `pid` the NAS's smbd serving it, `time` when it last connected one of them, in the NAS's local
//! time, beside `systime`, the NAS's clock) and `kick_connection` (`service_conn`, a JSON list of
//! `{pid, type, who, from}`; `http_conn` `[]`), as DSM's Resource Monitor sends them (its
//! resource.js). The API allows any user to call; DSM shows the Kick button to administrators
//! alone, and the account in keys.env (`NAS_DSM_USER`, `NAS_DSM_PASS`, read through jobrt::keys) is
//! in administrators, with only the DSM application allowed. `kick_connection` answers error 105
//! when any one of the connections asked can't be kicked (one gone by then, its own web session),
//! the others kicked all the same, so what it ended is read from a second list. Every request is a
//! POST (the password and the session id never in a URL), over HTTPS checked against the NAS's
//! tailnet name's certificate (store::nas::DSM_NAME), at home by its LAN address; logged out after
//! each use.
//!
//! **Whose sessions:** `who` the share's SMB user (store::nas::smb_user), the share among `descr`'s
//! (Time Machine's own connection never), `from` one of this Mac's addresses: those it has now (`own_addrs`), and, away
//! from home, the LAN addresses it had at home (`home-addresses.json` in the agent's folder, written
//! while at home), used for 24 hours at most and only for sessions last connected before it was last seen at
//! home, so an address the LAN has since given another Mac is never taken for this one's. Sessions
//! through Tailscale can't be told apart: the NAS's Tailscale runs in userspace and every tunnelled
//! client shows as 127.0.0.1, so they are never dropped.
//!
//! **When** (`Watch`): at the agent's start, once the NAS answers (its keys read), the sessions
//! last connected before the start but the newest (the live one: the share answering, its session
//! is this Mac's newest); on leaving home (at home: yes, then no), those of the addresses it had at home
//! (the share can't be unmounted cleanly by then: the LAN is gone, so an unmount ends nothing on
//! the NAS; only DSM, over Tailscale, can); once this Mac is paused (from the menu bar, alone or
//! with the build) with no job left running, at home, all its sessions, the live one too (it
//! reconnects at the next call, holding none of the jobs' leases); and with the self-heal's
//! remount (crate::agent::heal), all of them. The last two kick the live session, so they run under
//! the pool's lock (no tick mid-write), as the remount does. A failure (DSM unreachable, no keys,
//! refused) is a warning in the log and never stops the agent; one that didn't reach DSM is tried
//! again each minute for half an hour (a Mac that just left may have no network yet).

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use store::nasfs::NasRoot;

/// The remembered home addresses' file, in the agent's folder.
pub const HOME_FILE: &str = "home-addresses.json";
/// Remembered home addresses are used this long after they were last seen at home (a LAN gives a
/// left Mac's address to another only once its lease is out).
const HOME_FOR: u64 = 24 * 3600;
/// The remembered addresses written again at least this often while at home.
const HOME_WRITE_EVERY: u64 = 60;
/// A request to DSM may take this long in all.
const TIMEOUT: Duration = Duration::from_secs(10);
/// A drop that didn't reach DSM is tried again this often, this many times.
const RETRY_EVERY: Duration = Duration::from_secs(60);
const RETRIES: u32 = 30;
/// A drop under the pool's lock waits this long at most for it.
const LOCK_WAIT: Duration = Duration::from_secs(60);

/// DSM's account (Debug shows neither).
#[derive(Clone, PartialEq, Eq)]
pub struct Keys {
    user: String,
    pass: String,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Keys(..)")
    }
}

/// The account in a keys file's text (jobrt::keys), when both keys are there.
pub fn parse_keys(s: &str) -> Option<Keys> {
    Some(Keys { user: jobrt::keys::value(s, "NAS_DSM_USER")?, pass: jobrt::keys::value(s, "NAS_DSM_PASS")? })
}

/// A connection as DSM lists it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Conn {
    #[serde(default)]
    pub protocol: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub who: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub descr: String,
    #[serde(default)]
    pub pid: i64,
    #[serde(default)]
    pub time: String,
}

impl Conn {
    /// Its client's address.
    fn addr(&self) -> Option<IpAddr> {
        self.from.parse::<IpAddr>().ok().map(|a| a.to_canonical())
    }

    fn said(&self) -> String {
        format!("{} from {} (pid {}, {} last connected {})", self.kind, self.from, self.pid, self.descr, self.time)
    }
}

/// DSM's list: the connections, and the NAS's clock as it listed them.
#[derive(Clone, Debug)]
pub struct Listing {
    pub conns: Vec<Conn>,
    pub systime: String,
}

impl Listing {
    /// When `c` last connected a share, on this Mac's clock at `now` (Unix s): its age by the NAS's own clock (its
    /// `time` and `systime`, both the NAS's local time), back from `now`.
    fn connected(&self, c: &Conn, now: u64) -> Option<u64> {
        let age = local_secs_systime(&self.systime)? - local_secs_time(&c.time)?;
        Some(now.saturating_sub(age.max(0) as u64))
    }
}

/// Seconds of a proleptic Gregorian date and time (no time zone: two of them only differenced).
fn civil_secs(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe) * 86_400 + h * 3600 + mi * 60 + s
}

/// "2026/11/11 06:58:17" (a connection's `time`).
fn local_secs_time(t: &str) -> Option<i64> {
    let (date, clock) = t.trim().split_once(' ')?;
    let d: Vec<i64> = date.split('/').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let c: Vec<i64> = clock.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    (d.len() == 3 && c.len() == 3).then(|| civil_secs(d[0], d[1], d[2], c[0], c[1], c[2]))
}

/// "Wed Nov 11 15:29:45 2026\n" (the listing's `systime`).
fn local_secs_systime(t: &str) -> Option<i64> {
    let w: Vec<&str> = t.split_whitespace().collect();
    let [_, mon, day, clock, year] = w.as_slice() else { return None };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mo = MONTHS.iter().position(|m| m == mon)? as i64 + 1;
    let c: Vec<i64> = clock.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    (c.len() == 3).then_some(())?;
    Some(civil_secs(year.parse().ok()?, mo, day.parse().ok()?, c[0], c[1], c[2]))
}

/// The LAN addresses this Mac had at home, as of `at` (Unix s).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Home {
    pub addrs: Vec<IpAddr>,
    pub at: u64,
}

/// Which of this Mac's sessions a drop ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mine {
    /// Its addresses now: their sessions last connected before `before` (Unix s), but the newest of them
    /// all when `keep_newest` (the live one).
    pub now: Vec<IpAddr>,
    pub before: u64,
    pub keep_newest: bool,
    /// Away from home: the addresses it had there, their sessions last connected before it was last seen
    /// there.
    pub home: Option<Home>,
}

impl Mine {
    /// Away from home: an address it had there that it holds again now isn't a left one (back
    /// home, or a blip that looked like leaving): taken out.
    fn refresh(&mut self, own: &[IpAddr]) {
        if let Some(h) = &mut self.home {
            h.addrs.retain(|a| !own.contains(a));
        }
    }

    fn is_empty(&self) -> bool {
        self.now.is_empty() && self.home.as_ref().is_none_or(|h| h.addrs.is_empty())
    }
}

/// Of `l`'s connections, this Mac's to drop (`m`): SMB, of the share's user `user` and the share
/// `share`, from its addresses (task #567: never another Mac's, never a tunnelled one, which shows
/// as the NAS's own loopback).
pub fn pick(l: &Listing, user: &str, share: &str, m: &Mine, now: u64) -> Vec<Conn> {
    let ours = |c: &&Conn| {
        c.protocol == "SMB"
            && c.who == user
            && c.descr.split(',').any(|d| d.trim().eq_ignore_ascii_case(share))
            && c.pid > 0
    };
    let connected = |c: &Conn| l.connected(c, now);
    let on_now: Vec<&Conn> =
        l.conns.iter().filter(ours).filter(|c| c.addr().is_some_and(|a| m.now.contains(&a))).collect();
    let newest = on_now.iter().filter_map(|c| Some((connected(c)?, c.pid))).max().map(|(_, pid)| pid);
    let mut out: Vec<Conn> = on_now
        .into_iter()
        .filter(|c| connected(c).is_some_and(|b| b < m.before))
        .filter(|c| !(m.keep_newest && Some(c.pid) == newest))
        .cloned()
        .collect();
    if let Some(h) = &m.home {
        for c in l.conns.iter().filter(ours) {
            let left = c.addr().is_some_and(|a| h.addrs.contains(&a) && !m.now.contains(&a));
            if left && connected(c).is_some_and(|b| b < h.at) && !out.iter().any(|o| o.pid == c.pid) {
                out.push(c.clone());
            }
        }
    }
    out
}

/// What went wrong with DSM.
#[derive(Debug)]
pub enum Failed {
    /// DSM wasn't reached (no network, Tailscale down, the NAS off): worth trying again.
    Unreached(anyhow::Error),
    /// DSM answered no (bad keys, no permission, an error it named).
    Refused(anyhow::Error),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Unreached(e) => write!(f, "DSM not reached: {e:#}"),
            Failed::Refused(e) => write!(f, "{e:#}"),
        }
    }
}

/// DSM's web API.
pub struct Api {
    base: String,
    agent: ureq::Agent,
}

/// The NAS's DSM addresses for its tailnet name (store::nas::dsm_addrs), kept a minute (away from
/// home, the LAN name's lookup takes seconds to fail, and a drop makes four requests); any other
/// name as usual.
#[derive(Debug, Default)]
struct NasResolver {
    other: ureq::unversioned::resolver::DefaultResolver,
    last: std::sync::Mutex<Option<(Instant, Vec<std::net::SocketAddr>)>>,
}

impl ureq::unversioned::resolver::Resolver for NasResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        if uri.host() != Some(store::nas::DSM_NAME) {
            return self.other.resolve(uri, config, timeout);
        }
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let addrs = match &*last {
            Some((t, a)) if t.elapsed() < Duration::from_secs(60) && !a.is_empty() => a.clone(),
            _ => {
                let a = store::nas::dsm_addrs();
                *last = Some((Instant::now(), a.clone()));
                a
            }
        };
        let mut out = self.empty();
        for a in addrs.into_iter().take(16) {
            out.push(a);
        }
        if out.is_empty() {
            Err(ureq::Error::HostNotFound)
        } else {
            Ok(out)
        }
    }
}

/// DSM's error codes as words (DSM's own: 1xx any API's, 4xx the login's).
fn error_words(code: i64) -> &'static str {
    match code {
        105 => "not permitted (the account must be in administrators, with the DSM application allowed; a kick says it too for a connection gone by then)",
        106 | 107 | 119 => "the session expired or was ended",
        400 => "no such account, or a wrong password (NAS_DSM_USER, NAS_DSM_PASS)",
        401 => "the account is disabled",
        402 => "the account may not use the DSM application",
        403 | 404 | 406 => "the account asks for a second factor, which the agent can't give",
        407 => "this address is blocked by DSM's auto block",
        _ => "an error",
    }
}

impl Api {
    /// The NAS's.
    pub fn nas() -> Api {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .timeout_connect(Some(Duration::from_secs(5)))
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::new(),
            NasResolver::default(),
        );
        Api { base: format!("https://{}:{}", store::nas::DSM_NAME, store::nas::DSM_PORT), agent }
    }

    /// One at `base` (a test's fake DSM: `http://127.0.0.1:<port>`).
    pub fn at(base: &str) -> Api {
        let config = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).build();
        Api { base: base.trim_end_matches('/').to_string(), agent: config.into() }
    }

    /// One call of `api`'s `method` (form fields `form` beside them), its `data` when DSM says it
    /// succeeded.
    fn call(&self, api: &str, version: &str, method: &str, form: &[(&str, &str)]) -> Result<serde_json::Value, Failed> {
        let mut f: Vec<(&str, &str)> = vec![("api", api), ("version", version), ("method", method)];
        f.extend_from_slice(form);
        let url = format!("{}/webapi/entry.cgi", self.base);
        let body =
            self.agent.post(&url).send_form(f).and_then(|mut r| r.body_mut().read_to_string()).map_err(
                |e| match e {
                    ureq::Error::StatusCode(c) => Failed::Refused(anyhow!("DSM answered HTTP {c} to {api} {method}")),
                    e => Failed::Unreached(anyhow!("{api} {method}: {e}")),
                },
            )?;
        let v: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| Failed::Refused(anyhow!("{api} {method}: an answer that isn't JSON ({e})")))?;
        if v["success"].as_bool() == Some(true) {
            return Ok(v["data"].clone());
        }
        let code = v["error"]["code"].as_i64().unwrap_or(-1);
        Err(Failed::Refused(anyhow!("{api} {method}: DSM error {code}, {}", error_words(code))))
    }

    /// Logged in as `k`: the session's id.
    pub fn login(&self, k: &Keys) -> Result<String, Failed> {
        let d = self.call(
            "SYNO.API.Auth",
            "6",
            "login",
            &[("account", &k.user), ("passwd", &k.pass), ("session", "scenic-sessions"), ("format", "sid")],
        )?;
        d["sid"].as_str().map(str::to_string).ok_or_else(|| Failed::Refused(anyhow!("login: no session id")))
    }

    pub fn logout(&self, sid: &str) -> Result<(), Failed> {
        self.call("SYNO.API.Auth", "6", "logout", &[("session", "scenic-sessions"), ("_sid", sid)]).map(|_| ())
    }

    /// Every connection now.
    pub fn list(&self, sid: &str) -> Result<Listing, Failed> {
        let d = self.call("SYNO.Core.CurrentConnection", "1", "list", &[("_sid", sid)])?;
        let conns = serde_json::from_value(d["items"].clone())
            .map_err(|e| Failed::Refused(anyhow!("SYNO.Core.CurrentConnection list: {e}")))?;
        Ok(Listing { conns, systime: d["systime"].as_str().unwrap_or("").to_string() })
    }

    /// `conns` ended (kicked), as DSM's Resource Monitor does.
    pub fn kick(&self, sid: &str, conns: &[Conn]) -> Result<(), Failed> {
        let service: Vec<serde_json::Value> = conns
            .iter()
            .map(|c| serde_json::json!({"pid": c.pid, "type": c.kind, "who": c.who, "from": c.from}))
            .collect();
        let service = serde_json::Value::from(service).to_string();
        self.call(
            "SYNO.Core.CurrentConnection",
            "1",
            "kick_connection",
            &[("service_conn", &service), ("http_conn", "[]"), ("_sid", sid)],
        )
        .map(|_| ())
    }

    /// Logged in, this Mac's sessions `m` listed and kicked, logged out (whatever happened between);
    /// the ones that ended. The kick under `pool`'s lock when given (waited for `LOCK_WAIT` at most).
    pub fn drop_mine(
        &self,
        k: &Keys,
        m: &Mine,
        now: u64,
        pool: Option<&super::pool::Shared>,
    ) -> Result<Vec<Conn>, Failed> {
        let sid = self.login(k)?;
        let r = (|| {
            let l = self.list(&sid)?;
            let drop = pick(&l, store::nas::smb_user(), store::nas::SHARE, m, now);
            if !drop.is_empty() {
                let _g = match pool {
                    Some(p) => Some(super::pool::lock_within(p, LOCK_WAIT).ok_or_else(|| {
                        Failed::Unreached(anyhow!("the pool's step held its lock for {LOCK_WAIT:?}"))
                    })?),
                    None => None,
                };
                let kicked = self.kick(&sid, &drop);
                // (What ended, by a second list: a kick refused for one connection ends the others.)
                let left = self.list(&sid)?;
                let gone: Vec<Conn> = drop
                    .into_iter()
                    .filter(|c| !left.conns.iter().any(|l| l.pid == c.pid && l.from == c.from))
                    .collect();
                if gone.is_empty() {
                    kicked?;
                }
                return Ok(gone);
            }
            Ok(drop)
        })();
        if let Err(e) = self.logout(&sid) {
            eprintln!("agent: DSM: logging out: {e}");
        }
        r
    }
}

/// Why a drop is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Start,
    Left,
    Paused,
    Remount,
}

impl Why {
    fn words(self) -> &'static str {
        match self {
            Why::Start => "the agent started",
            Why::Left => "this Mac left home",
            Why::Paused => "this Mac is paused",
            Why::Remount => "the self-heal's remount",
        }
    }

    /// Whether it may end the live session (under the pool's lock, then).
    fn kicks_live(self) -> bool {
        matches!(self, Why::Paused | Why::Remount)
    }
}

/// What the loop saw this round.
pub struct Seen {
    pub now: u64,
    /// At home (cond::home).
    pub home: bool,
    /// The NAS's project folder answers.
    pub nas: bool,
    /// This Mac's addresses now (store::nas::own_addrs).
    pub addrs: Vec<IpAddr>,
    /// This Mac's pause (its own or the build's) as of when, while no job runs here.
    pub paused: Option<u64>,
}

/// Whether `a` is a Tailscale address (100.64.0.0/10, fd7a:115c:a1e0::/48).
fn tailscale(a: &IpAddr) -> bool {
    match a {
        IpAddr::V4(v) => v.octets()[0] == 100 && v.octets()[1] & 0xc0 == 64,
        IpAddr::V6(v) => v.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// When the loop asks a drop, from what it sees each round.
pub struct Watch {
    /// The agent's start (Unix s).
    started: u64,
    /// Whether the start's drop was asked.
    start_asked: bool,
    was_home: Option<bool>,
    /// The LAN addresses as last seen at home, and the file they're kept in.
    home: Option<Home>,
    file: PathBuf,
    written: Option<Home>,
    /// The pause (its `at`) a drop was asked for.
    paused_for: Option<u64>,
}

impl Watch {
    /// For an agent started at `started`, its folder `dir`.
    pub fn new(started: u64, dir: &std::path::Path) -> Watch {
        let file = dir.join(HOME_FILE);
        let home = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Home>(&b).ok());
        Watch { started, start_asked: false, was_home: None, written: home.clone(), home, file, paused_for: None }
    }

    /// The remembered home addresses to use away at `now`: within `HOME_FOR`.
    fn left(&self, now: u64) -> Option<Home> {
        self.home.clone().filter(|h| !h.addrs.is_empty() && h.at + HOME_FOR >= now)
    }

    /// The drops due this round.
    pub fn round(&mut self, s: &Seen) -> Vec<(Why, Mine)> {
        let mut out = Vec::new();
        if s.home {
            let addrs: Vec<IpAddr> = s.addrs.iter().filter(|a| !tailscale(a)).copied().collect();
            if !addrs.is_empty() {
                let h = Home { addrs, at: s.now };
                let stale =
                    self.written.as_ref().is_none_or(|w| w.addrs != h.addrs || w.at + HOME_WRITE_EVERY <= s.now);
                if stale {
                    match serde_json::to_vec(&h)
                        .map_err(anyhow::Error::from)
                        .and_then(|b| super::write_replace(&self.file, &b))
                    {
                        Ok(()) => self.written = Some(h.clone()),
                        Err(e) => eprintln!("agent: {}: {e:#}", self.file.display()),
                    }
                }
                self.home = Some(h);
            }
        }
        if !self.start_asked && s.nas {
            self.start_asked = true;
            let home = if s.home { None } else { self.left(s.now) };
            out.push((Why::Start, Mine { now: s.addrs.clone(), before: self.started, keep_newest: true, home }));
        }
        if self.was_home == Some(true) && !s.home {
            if let Some(home) = self.left(s.now) {
                out.push((Why::Left, Mine { now: Vec::new(), before: 0, keep_newest: false, home: Some(home) }));
            }
        }
        // (Paused with no job from before this agent started, as it first looks: its drop was the
        // earlier agent's.)
        let first = self.was_home.is_none();
        self.was_home = Some(s.home);
        match s.paused {
            Some(at) if self.paused_for != Some(at) => {
                self.paused_for = Some(at);
                if s.home && !(first && at < self.started) {
                    out.push((
                        Why::Paused,
                        Mine { now: s.addrs.clone(), before: s.now, keep_newest: false, home: None },
                    ));
                }
            }
            Some(_) => {}
            None => self.paused_for = None,
        }
        out
    }

    /// The self-heal's remount at `now`: every session of this Mac's addresses `addrs`.
    pub fn remount(now: u64, addrs: Vec<IpAddr>) -> (Why, Mine) {
        (Why::Remount, Mine { now: addrs, before: now, keep_newest: false, home: None })
    }
}

/// A drop asked of the part.
struct Ask {
    why: Why,
    mine: Mine,
    root: Option<NasRoot>,
}

/// The part: a thread of its own running the drops one at a time, its DSM keys kept in memory
/// (read from keys.env whenever a drop brings the NAS's folder), a drop that didn't reach DSM tried
/// again (`RETRY_EVERY`, `RETRIES`; a newer one of its kind replaces it).
pub struct Part {
    tx: Sender<Ask>,
}

impl Part {
    /// Its thread, kicking through `api`; the live session's kicks under the pool's lock (`pool`,
    /// when this Mac is in it).
    pub fn spawn(api: Api, pool: Option<super::pool::Shared>) -> Option<Part> {
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new().name("dsm".into()).spawn(move || run(api, pool, rx));
        match started {
            Ok(_) => Some(Part { tx }),
            Err(e) => {
                eprintln!("agent: the DSM part's thread: {e}");
                None
            }
        }
    }

    pub fn ask(&self, why: Why, mine: Mine, root: Option<NasRoot>) {
        self.tx.send(Ask { why, mine, root }).ok();
    }
}

fn run(api: Api, pool: Option<super::pool::Shared>, rx: Receiver<Ask>) {
    let mut keys: Option<Keys> = None;
    let mut told_keys = false;
    // (Drops to try again: the ask, its tries left, when next.)
    let mut again: Vec<(Ask, u32, Instant)> = Vec::new();
    loop {
        let wait = again.iter().map(|a| a.2.saturating_duration_since(Instant::now())).min();
        let next = match wait {
            Some(w) => match rx.recv_timeout(w) {
                Ok(a) => Some(a),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(a) => Some(a),
                Err(_) => return,
            },
        };
        let (a, tries) = match next {
            Some(a) => {
                again.retain(|r| r.0.why != a.why);
                (a, RETRIES)
            }
            None => {
                let Some(k) = again.iter().position(|r| r.2 <= Instant::now()) else { continue };
                let (a, t, _) = again.remove(k);
                (a, t)
            }
        };
        if let Some(root) = &a.root {
            match jobrt::keys::read(root).ok().as_deref().and_then(parse_keys) {
                Some(k) => keys = Some(k),
                None if keys.is_none() && !told_keys => {
                    told_keys = true;
                    eprintln!("agent: warning: DSM: keys.env has no NAS_DSM_USER and NAS_DSM_PASS: this Mac's stale SMB sessions aren't dropped");
                }
                None => {}
            }
        }
        let Some(k) = &keys else {
            eprintln!("agent: warning: DSM: no keys read yet ({}): nothing dropped", a.why.words());
            continue;
        };
        let mut mine = a.mine.clone();
        mine.refresh(&store::nas::own_addrs());
        if mine.is_empty() {
            continue;
        }
        let pool = pool.as_ref().filter(|_| a.why.kicks_live());
        match api.drop_mine(k, &mine, super::jobs::now_s(), pool) {
            Ok(d) if d.is_empty() => {}
            Ok(d) => eprintln!(
                "agent: DSM: {}: ended this Mac's SMB sessions {}",
                a.why.words(),
                d.iter().map(Conn::said).collect::<Vec<_>>().join(", ")
            ),
            Err(Failed::Unreached(e)) if tries > 1 => {
                eprintln!("agent: warning: DSM: {}: not reached ({e:#}); tried again in a minute", a.why.words());
                again.push((a, tries - 1, Instant::now() + RETRY_EVERY));
            }
            Err(e) => eprintln!("agent: warning: DSM: {}: {e}", a.why.words()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    const SYSTIME: &str = "Wed Nov 11 15:00:00 2026\n";
    /// Our clock as the listing's `systime` is read.
    const NOW: u64 = 1_000_000;

    fn conn(pid: i64, from: &str, descr: &str, time: &str) -> Conn {
        Conn {
            protocol: "SMB".into(),
            kind: "SMB3".into(),
            who: "brandontsang".into(),
            from: from.into(),
            descr: descr.into(),
            pid,
            time: time.into(),
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn pids(v: &[Conn]) -> Vec<i64> {
        let mut p: Vec<i64> = v.iter().map(|c| c.pid).collect();
        p.sort();
        p
    }

    #[test]
    fn times_are_read_against_the_nas_clock() {
        let l = Listing { conns: vec![], systime: SYSTIME.into() };
        assert_eq!(l.connected(&conn(1, "a", "personal", "2026/11/11 14:00:00"), NOW), Some(NOW - 3600));
        // (Across a month and a year's end.)
        let l2 = Listing { conns: vec![], systime: "Thu Jan  1 00:00:10 2027".into() };
        assert_eq!(l2.connected(&conn(1, "a", "p", "2026/12/31 23:59:50"), NOW), Some(NOW - 20));
        assert_eq!(l.connected(&conn(1, "a", "p", "yesterday"), NOW), None);
        let none = Listing { conns: vec![], systime: "".into() };
        assert_eq!(none.connected(&conn(1, "a", "p", "2026/11/11 14:00:00"), NOW), None);
    }

    #[test]
    fn keys_are_read_and_never_shown() {
        let k = parse_keys("NAS_DSM_USER=scenic-sessions\nNAS_DSM_PASS='s3cret'\n").unwrap();
        assert!(!format!("{k:?}").contains("s3cret") && !format!("{k:?}").contains("scenic"));
        assert!(parse_keys("NAS_DSM_USER=u\n").is_none());
    }

    /// Only this Mac's: its addresses (not another Mac's, nor a tunnelled one's 127.0.0.1), the
    /// share's user, the share (not Time Machine's own connection); at the start, those last
    /// connected before it but the live (newest) one.
    #[test]
    fn only_this_macs_stale_sessions_are_picked() {
        let l = Listing {
            systime: SYSTIME.into(),
            conns: vec![
                conn(1, "192.168.2.15", "personal", "2026/11/08 09:09:51"),
                conn(2, "192.168.2.15", "home, personal", "2026/11/11 14:30:00"),
                conn(3, "192.168.2.33", "personal", "2026/11/11 06:58:17"),
                conn(4, "127.0.0.1", "personal", "2026/11/11 10:00:00"),
                conn(5, "192.168.2.15", "Time Machine", "2026/11/04 09:45:06"),
                Conn { who: "someone".into(), ..conn(6, "192.168.2.15", "personal", "2026/11/01 00:00:00") },
                Conn { protocol: "HTTP/HTTPS".into(), ..conn(7, "192.168.2.15", "personal", "2026/11/01 00:00:00") },
                conn(8, "192.168.2.15", "personal", "2026/11/11 14:59:00"),
            ],
        };
        let start = NOW - 10 * 60;
        let m =
            Mine { now: vec![ip("192.168.2.15"), ip("100.70.85.80")], before: start, keep_newest: true, home: None };
        // (1 and 2 last connected before the start; 8 after it, the newest: live, kept.)
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1, 2]);
        // Started after every one of them: the newest is still the live one.
        let m = Mine { before: NOW - 30, ..m };
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1, 2]);
        // Paused: every one of them, the live one too.
        let m = Mine { before: NOW, keep_newest: false, ..m };
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1, 2, 8]);
    }

    /// Away: the addresses it had at home, their sessions last connected before it was last seen
    /// there (one connected since is another Mac's, given the address).
    #[test]
    fn a_left_macs_home_sessions_are_picked_by_when_it_left() {
        let l = Listing {
            systime: SYSTIME.into(),
            conns: vec![
                conn(1, "192.168.2.15", "personal", "2026/11/11 09:00:00"),
                conn(2, "192.168.2.15", "personal", "2026/11/11 14:00:00"),
                conn(3, "192.168.2.33", "personal", "2026/11/11 06:58:17"),
            ],
        };
        let left_at = NOW - 3 * 3600; // 12:00 on the NAS's clock
        let m = Mine {
            now: vec![],
            before: 0,
            keep_newest: false,
            home: Some(Home { addrs: vec![ip("192.168.2.15")], at: left_at }),
        };
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1]);
        // Holding that address again (home, a blip): nothing.
        let mut back = m.clone();
        back.refresh(&[ip("192.168.2.15")]);
        assert!(back.is_empty());
        assert!(pick(&l, "brandontsang", "personal", &back, NOW).is_empty());
    }

    fn seen(now: u64, home: bool, nas: bool, paused: Option<u64>) -> Seen {
        Seen { now, home, nas, addrs: vec![ip("192.168.2.15"), ip("100.70.85.80")], paused }
    }

    #[test]
    fn the_watch_asks_at_start_on_leaving_and_once_paused() {
        let d = tempfile::tempdir().unwrap();
        let mut w = Watch::new(1000, d.path());
        // (No NAS yet: no start's drop.)
        assert!(w.round(&seen(1000, true, false, None)).is_empty());
        let a = w.round(&seen(1020, true, true, None));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0, Why::Start);
        assert_eq!(a[0].1, Mine { now: seen(0, true, true, None).addrs, before: 1000, keep_newest: true, home: None });
        // (The home addresses kept, the tunnel's left out.)
        let kept: Home = serde_json::from_slice(&std::fs::read(d.path().join(HOME_FILE)).unwrap()).unwrap();
        assert_eq!(kept, Home { addrs: vec![ip("192.168.2.15")], at: 1000 });
        assert!(w.round(&seen(1040, true, true, None)).is_empty(), "once");
        // Left: the addresses it had at home, as last seen there.
        let a = w.round(&seen(1060, false, false, None));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0, Why::Left);
        assert_eq!(a[0].1.home, Some(Home { addrs: vec![ip("192.168.2.15")], at: 1040 }));
        assert!(a[0].1.now.is_empty());
        assert!(w.round(&seen(1080, false, true, None)).is_empty(), "once");
        // Home again and paused, its jobs ended: every one of its sessions, once a pause.
        assert!(w.round(&seen(1100, true, true, None)).is_empty());
        let a = w.round(&seen(1120, true, true, Some(1110)));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0, Why::Paused);
        assert_eq!((a[0].1.before, a[0].1.keep_newest), (1120, false));
        assert!(w.round(&seen(1140, true, true, Some(1110))).is_empty(), "once a pause");
        assert!(w.round(&seen(1160, true, true, None)).is_empty());
        assert_eq!(w.round(&seen(1180, true, true, Some(1170)))[0].0, Why::Paused, "a new pause");
        // Paused away: nothing (its home sessions went as it left; tunnelled ones can't be told).
        assert!(w.round(&seen(1200, false, true, None)).iter().all(|a| a.0 == Why::Left));
        assert!(w.round(&seen(1220, false, true, Some(1210))).is_empty());
        // Started paused with no job (the pause from before it): the earlier agent's drop; not again.
        let mut w = Watch::new(2000, d.path());
        assert!(w.round(&seen(2000, true, true, Some(1500))).iter().all(|a| a.0 == Why::Start));
        assert!(w.round(&seen(2020, true, true, Some(1500))).is_empty());
        // Started paused with jobs finishing: dropped once they have.
        let mut w = Watch::new(3000, d.path());
        w.round(&seen(3000, true, true, None));
        assert_eq!(w.round(&seen(3020, true, true, Some(2500)))[0].0, Why::Paused);
    }

    /// An agent started away uses the home addresses its Mac kept, within a day of leaving.
    #[test]
    fn started_away_it_uses_the_kept_home_addresses_for_a_day() {
        let d = tempfile::tempdir().unwrap();
        let h = Home { addrs: vec![ip("192.168.2.15")], at: 5000 };
        std::fs::write(d.path().join(HOME_FILE), serde_json::to_vec(&h).unwrap()).unwrap();
        let mut w = Watch::new(6000, d.path());
        let a = w.round(&seen(6000, false, true, None));
        assert_eq!(a[0].0, Why::Start);
        assert_eq!(a[0].1.home, Some(h.clone()));
        let mut w = Watch::new(5000 + HOME_FOR + 1, d.path());
        assert_eq!(w.round(&seen(5000 + HOME_FOR + 1, false, true, None))[0].1.home, None);
    }

    /// A fake DSM: its connections, what it was asked (each request's line and form), its open
    /// sessions.
    #[derive(Default)]
    struct Fake {
        conns: Vec<Conn>,
        asked: Vec<(String, Vec<(String, String)>)>,
        /// Pids `kick_connection` won't kick (it says 105, kicking the rest).
        unkickable: Vec<i64>,
        sessions: Vec<String>,
    }

    fn decode(s: &str) -> String {
        let b = s.replace('+', " ");
        let b = b.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                out.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap(), 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    /// One request's answer from the fake.
    fn answer(f: &mut Fake, line: &str, form: Vec<(String, String)>) -> serde_json::Value {
        let get = |k: &str| form.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
        f.asked.push((line.to_string(), form.clone()));
        let authed = f.sessions.contains(&get("_sid"));
        match (get("api").as_str(), get("method").as_str()) {
            ("SYNO.API.Auth", "login") if get("account") == "scenic-sessions" && get("passwd") == "pw" => {
                f.sessions.push("sid1".into());
                serde_json::json!({"success": true, "data": {"sid": "sid1"}})
            }
            ("SYNO.API.Auth", "login") => serde_json::json!({"success": false, "error": {"code": 400}}),
            (_, _) if !authed => serde_json::json!({"success": false, "error": {"code": 119}}),
            ("SYNO.API.Auth", "logout") => {
                f.sessions.clear();
                serde_json::json!({"success": true})
            }
            ("SYNO.Core.CurrentConnection", "list") => {
                serde_json::json!({"success": true, "data": {"items": f.conns, "systime": SYSTIME, "total": f.conns.len()}})
            }
            ("SYNO.Core.CurrentConnection", "kick_connection") => {
                let asked: Vec<serde_json::Value> = serde_json::from_str(&get("service_conn")).unwrap();
                let mut refused = false;
                for a in asked {
                    let pid = a["pid"].as_i64().unwrap();
                    if f.unkickable.contains(&pid) {
                        refused = true;
                        continue;
                    }
                    f.conns.retain(|c| !(c.pid == pid && c.from == a["from"].as_str().unwrap()));
                }
                if refused {
                    serde_json::json!({"success": false, "error": {"code": 105}})
                } else {
                    serde_json::json!({"success": true})
                }
            }
            _ => serde_json::json!({"success": false, "error": {"code": 103}}),
        }
    }

    fn serve(fake: Arc<Mutex<Fake>>) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let form: Vec<(String, String)> = String::from_utf8(body)
                    .unwrap()
                    .split('&')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (decode(k), decode(v)))
                    .collect();
                let b = answer(&mut fake.lock().unwrap(), line.trim(), form).to_string();
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                s.write_all(head.as_bytes()).and_then(|_| s.write_all(b.as_bytes())).ok();
            }
        });
        base
    }

    fn keys() -> Keys {
        parse_keys("NAS_DSM_USER=scenic-sessions\nNAS_DSM_PASS=pw\n").unwrap()
    }

    /// Against a fake DSM: logged in by POST (the password never in a URL), this Mac's stale
    /// sessions kicked and no other, logged out after.
    #[test]
    fn a_drop_kicks_only_this_macs_and_logs_out() {
        let fake = Arc::new(Mutex::new(Fake {
            conns: vec![
                conn(1, "192.168.2.15", "personal", "2026/11/08 09:09:51"),
                conn(2, "192.168.2.33", "personal", "2026/11/11 06:58:17"),
                conn(3, "127.0.0.1", "personal", "2026/11/11 10:00:00"),
                conn(4, "192.168.2.15", "personal", "2026/11/11 14:59:00"),
            ],
            ..Default::default()
        }));
        let api = Api::at(&serve(fake.clone()));
        let m = Mine { now: vec![ip("192.168.2.15")], before: NOW - 600, keep_newest: true, home: None };
        let d = api.drop_mine(&keys(), &m, NOW, None).unwrap();
        assert_eq!(pids(&d), vec![1]);
        let f = fake.lock().unwrap();
        assert_eq!(pids(&f.conns), vec![2, 3, 4]);
        assert!(f.sessions.is_empty(), "logged out");
        let methods: Vec<String> =
            f.asked.iter().map(|(_, form)| form.iter().find(|(k, _)| k == "method").unwrap().1.clone()).collect();
        assert_eq!(methods, ["login", "list", "kick_connection", "list", "logout"]);
        assert!(f.asked.iter().all(|(line, _)| line.starts_with("POST /webapi/entry.cgi ") && !line.contains("pw")));
    }

    /// A kick refused for one connection (gone by then) ends the others: what ended is read back.
    #[test]
    fn a_partly_refused_kick_says_what_ended() {
        let fake = Arc::new(Mutex::new(Fake {
            conns: vec![
                conn(1, "192.168.2.15", "personal", "2026/11/08 09:09:51"),
                conn(2, "192.168.2.15", "personal", "2026/11/09 09:09:51"),
            ],
            unkickable: vec![2],
            ..Default::default()
        }));
        let api = Api::at(&serve(fake.clone()));
        let m = Mine { now: vec![ip("192.168.2.15")], before: NOW, keep_newest: false, home: None };
        assert_eq!(pids(&api.drop_mine(&keys(), &m, NOW, None).unwrap()), vec![1]);
        // All refused: the error.
        let e = api.drop_mine(&keys(), &m, NOW, None).unwrap_err();
        assert!(matches!(e, Failed::Refused(_)) && e.to_string().contains("105"), "{e}");
        assert!(fake.lock().unwrap().sessions.is_empty(), "logged out after a failure too");
    }

    /// Bad keys are refused, saying so; a DSM not there is "not reached" (tried again).
    #[test]
    fn failures_say_why() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let api = Api::at(&serve(fake));
        let bad = parse_keys("NAS_DSM_USER=scenic-sessions\nNAS_DSM_PASS=wrong\n").unwrap();
        let m = Mine { now: vec![ip("192.168.2.15")], before: NOW, keep_newest: false, home: None };
        let e = api.drop_mine(&bad, &m, NOW, None).unwrap_err();
        assert!(matches!(e, Failed::Refused(_)) && e.to_string().contains("wrong password"), "{e}");
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        assert!(matches!(Api::at(&base).drop_mine(&keys(), &m, NOW, None), Err(Failed::Unreached(_))));
    }
}
