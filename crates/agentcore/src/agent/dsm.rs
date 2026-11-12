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
//! in administrators, with only the DSM application allowed. A kick ends the connection's smbd, so
//! every share it holds. `kick_connection` answers error 105 when any one of the connections asked
//! can't be kicked (one gone by then), the others kicked all the same, so after a 105 what it ended
//! is read from a second list. Every request is a POST (the password and the session id never in a
//! URL), over HTTPS checked against the NAS's tailnet name's certificate (store::nas::DSM_NAME), at
//! home by its LAN address; logged out after each use. A certificate that fails (lapsed: it is
//! Let's Encrypt's, 90 days, renewed on the NAS by the owner) is a warning saying so and nothing is
//! sent; there is no unchecked fallback.
//!
//! **Whose sessions:** `who` the share's SMB user (store::nas::smb_user, every Mac's), the share
//! among `descr`'s and no Time Machine there, `from` one of this Mac's addresses: those it has now
//! (`own_addrs`), and, away from home, the LAN addresses it had at home (`home-addresses.json` in
//! the agent's folder, written while at home), used for 24 hours at most and only for sessions last
//! connected before it was last seen at home, so an address the LAN has since given another Mac is
//! never taken for this one's. A session through Tailscale comes from the Mac's tailnet address
//! (the NAS's Tailscale is a TUN device: DSM's `from` is the client's own address, verified 11 Nov),
//! which is among its addresses now at home and away; a tailnet address is one Mac's alone and
//! never given to another, so the tailnet addresses it had earlier in the agent's run count as its
//! own too, with no time limit.
//!
//! **When** (`Watch`): at the agent's start, once the NAS answers (its keys read), the sessions of
//! its addresses now last connected before the start but the newest (the live one: a dead session
//! stopped before the session replacing it began, so the live one connected last); with the
//! self-heal's remount (crate::agent::heal), the same as of the remount; on leaving home (at home:
//! yes, then no), those of the addresses it had at home (the share can't be unmounted cleanly by
//! then: the LAN is gone, so an unmount ends nothing on the NAS; only DSM, over Tailscale, can);
//! once the share is mounted by its LAN name after it was mounted through Tailscale (home again),
//! the same as at the start as of then, with every tailnet address it has had (a tunnelled session
//! the forced unmount didn't end, or one a sleep or a dropped tunnel left behind away, holds its
//! leases as a left LAN one does; the new LAN session is the newest, so kept).
//! The live session is never kicked: the server, Finder and the agent's own beats hold or reopen
//! files on the share within seconds, so a kick only swaps it for a fresh one while their open
//! handles go stale (task #567's review). A failure (DSM unreachable, no keys, refused) is a
//! warning in the log and never stops the agent; one that didn't reach DSM is tried again each
//! minute for half an hour (a Mac that just left may have no network yet).

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
    /// Its addresses now: their sessions last connected before `before` (Unix s), but the newest of
    /// them all (the live one).
    pub now: Vec<IpAddr>,
    pub before: u64,
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
/// `share` (none holding Time Machine: a kick ends every share of the connection), from its
/// addresses (task #567: never another Mac's, its LAN or its tailnet address), never the live one.
pub fn pick(l: &Listing, user: &str, share: &str, m: &Mine, now: u64) -> Vec<Conn> {
    let ours = |c: &&Conn| {
        let shares = || c.descr.split(',').map(str::trim);
        c.protocol == "SMB"
            && c.who == user
            && shares().any(|d| d.eq_ignore_ascii_case(share))
            && !shares().any(|d| d.eq_ignore_ascii_case("Time Machine"))
            && c.pid > 0
    };
    let connected = |c: &Conn| l.connected(c, now);
    let on_now: Vec<&Conn> =
        l.conns.iter().filter(ours).filter(|c| c.addr().is_some_and(|a| m.now.contains(&a))).collect();
    let newest = on_now.iter().filter_map(|c| Some((connected(c)?, c.pid))).max().map(|(_, pid)| pid);
    let mut out: Vec<Conn> = on_now
        .into_iter()
        .filter(|c| connected(c).is_some_and(|b| b < m.before))
        .filter(|c| Some(c.pid) != newest)
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

/// The NAS's DSM addresses (store::nas::dsm_addrs) for its tailnet name, the only name an
/// `Api::nas` asks for: looked up once as it is made, outside every request's `TIMEOUT` (away from
/// home, the LAN name's lookup alone takes 5 s to fail).
#[derive(Debug)]
struct NasResolver(Vec<std::net::SocketAddr>);

impl ureq::unversioned::resolver::Resolver for NasResolver {
    fn resolve(
        &self,
        _: &ureq::http::Uri,
        _: &ureq::config::Config,
        _: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let mut out = self.empty();
        for a in self.0.iter().take(16) {
            out.push(*a);
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
    /// The NAS's, for one drop.
    pub fn nas() -> Api {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .timeout_connect(Some(Duration::from_secs(5)))
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::new(),
            NasResolver(store::nas::dsm_addrs()),
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
                    // (rustls's handshake failures, a certificate refused among them, come as
                    // InvalidData: not the network's, so not tried again.)
                    ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::InvalidData => Failed::Refused(anyhow!(
                        "{api} {method}: TLS with DSM failed ({e}): its certificate must be valid for {} (renewed on the NAS, docs/macs.md); nothing was sent",
                        store::nas::DSM_NAME
                    )),
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
    /// the ones that ended.
    pub fn drop_mine(&self, k: &Keys, m: &Mine, now: u64) -> Result<Vec<Conn>, Failed> {
        let sid = self.login(k)?;
        let r = (|| {
            let drop = pick(&self.list(&sid)?, store::nas::smb_user(), store::nas::SHARE, m, now);
            if drop.is_empty() {
                return Ok(drop);
            }
            match self.kick(&sid, &drop) {
                Err(Failed::Refused(e)) => {
                    // (A kick refused for one connection ends the others: what ended, by a second
                    // list.)
                    let left = self.list(&sid)?;
                    let gone: Vec<Conn> = drop
                        .into_iter()
                        .filter(|c| !left.conns.iter().any(|l| l.pid == c.pid && l.from == c.from))
                        .collect();
                    if gone.is_empty() {
                        return Err(Failed::Refused(e));
                    }
                    Ok(gone)
                }
                r => r.map(|()| drop),
            }
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
    Lan,
    Remount,
}

impl Why {
    fn words(self) -> &'static str {
        match self {
            Why::Start => "the agent started",
            Why::Left => "this Mac left home",
            Why::Lan => "the share mounted by its LAN name again",
            Why::Remount => "the self-heal's remount",
        }
    }
}

/// What the loop saw this round.
pub struct Seen {
    pub now: u64,
    /// At home (cond::home).
    pub home: bool,
    /// The NAS's project folder answers.
    pub nas: bool,
    /// The share is mounted by the NAS's LAN name (store::nas::lan_mounted): with `nas`, false is
    /// a mount through Tailscale.
    pub lan: bool,
    /// This Mac's addresses now (store::nas::own_addrs).
    pub addrs: Vec<IpAddr>,
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
    /// The share was last seen mounted through Tailscale (not by the LAN name).
    tunnelled: bool,
    /// Every tailnet address this Mac has had in the agent's run.
    tailnet: Vec<IpAddr>,
    /// The LAN addresses as last seen at home, and the file they're kept in.
    home: Option<Home>,
    file: PathBuf,
    written: Option<Home>,
}

impl Watch {
    /// For an agent started at `started`, its folder `dir`.
    pub fn new(started: u64, dir: &std::path::Path) -> Watch {
        let file = dir.join(HOME_FILE);
        let home = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Home>(&b).ok());
        Watch {
            started,
            start_asked: false,
            was_home: None,
            tunnelled: false,
            tailnet: Vec::new(),
            written: home.clone(),
            home,
            file,
        }
    }

    /// The remembered home addresses to use away at `now`: within `HOME_FOR`.
    fn left(&self, now: u64) -> Option<Home> {
        self.home.clone().filter(|h| !h.addrs.is_empty() && h.at + HOME_FOR >= now)
    }

    /// The drops due this round.
    pub fn round(&mut self, s: &Seen) -> Vec<(Why, Mine)> {
        let mut out = Vec::new();
        for a in s.addrs.iter().filter(|a| tailscale(a)) {
            if !self.tailnet.contains(a) {
                self.tailnet.push(*a);
            }
        }
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
            out.push((Why::Start, Mine { now: s.addrs.clone(), before: self.started, home }));
        }
        if self.was_home == Some(true) && !s.home {
            if let Some(home) = self.left(s.now) {
                out.push((Why::Left, Mine { now: Vec::new(), before: 0, home: Some(home) }));
            }
        }
        self.was_home = Some(s.home);
        if s.nas && !s.lan {
            self.tunnelled = true;
        } else if s.nas && s.lan && std::mem::take(&mut self.tunnelled) {
            let mut now = s.addrs.clone();
            now.extend(self.tailnet.iter().filter(|a| !s.addrs.contains(a)));
            out.push((Why::Lan, Mine { now, before: s.now, home: None }));
        }
        out
    }

    /// The self-heal's remount at `now`: the sessions of this Mac's addresses `addrs` but the live
    /// one (the remount ends that one itself).
    pub fn remount(now: u64, addrs: Vec<IpAddr>) -> (Why, Mine) {
        (Why::Remount, Mine { now: addrs, before: now, home: None })
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
    /// Its thread, kicking through an `api` made for each drop.
    pub fn spawn(api: fn() -> Api) -> Option<Part> {
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new().name("dsm".into()).spawn(move || run(api, rx));
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

fn run(api: fn() -> Api, rx: Receiver<Ask>) {
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
        match api().drop_mine(k, &mine, super::jobs::now_s()) {
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
    use std::sync::{Arc, RwLock};

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

    /// Only this Mac's: its addresses (not another Mac's, LAN or tailnet), the share's user, the share (no connection holding Time Machine); those last connected before
    /// the start, never the live (newest) one.
    #[test]
    fn only_this_macs_stale_sessions_are_picked() {
        let l = Listing {
            systime: SYSTIME.into(),
            conns: vec![
                conn(1, "192.168.2.15", "personal", "2026/11/08 09:09:51"),
                conn(2, "192.168.2.15", "home, personal", "2026/11/11 14:30:00"),
                conn(3, "192.168.2.33", "personal", "2026/11/11 06:58:17"),
                conn(4, "100.101.4.7", "personal", "2026/11/11 10:00:00"),
                conn(5, "192.168.2.15", "Time Machine", "2026/11/04 09:45:06"),
                Conn { who: "someone".into(), ..conn(6, "192.168.2.15", "personal", "2026/11/01 00:00:00") },
                Conn { protocol: "HTTP/HTTPS".into(), ..conn(7, "192.168.2.15", "personal", "2026/11/01 00:00:00") },
                conn(8, "192.168.2.15", "personal", "2026/11/11 14:59:00"),
                conn(9, "192.168.2.15", "personal, Time Machine", "2026/11/02 00:00:00"),
            ],
        };
        let start = NOW - 10 * 60;
        let m = Mine { now: vec![ip("192.168.2.15"), ip("100.70.85.80")], before: start, home: None };
        // (1 and 2 last connected before the start; 8 after it, the newest: live, kept.)
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1, 2]);
        // Asked after every one of them (a remount): the newest is still the live one, kept.
        let m = Mine { before: NOW, ..m };
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1, 2]);
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
        let m = Mine { now: vec![], before: 0, home: Some(Home { addrs: vec![ip("192.168.2.15")], at: left_at }) };
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1]);
        // Holding that address again (home, a blip): nothing.
        let mut back = m.clone();
        back.refresh(&[ip("192.168.2.15")]);
        assert!(back.is_empty());
        assert!(pick(&l, "brandontsang", "personal", &back, NOW).is_empty());
    }

    /// Through Tailscale (the NAS's Tailscale a TUN device): a session comes from the Mac's tailnet
    /// address, one of its own. A stale tunnelled one is dropped, the newest kept whichever address
    /// it is on, and another Mac's tailnet address is never picked.
    #[test]
    fn tunnelled_sessions_are_this_macs_by_its_tailnet_address() {
        let l = Listing {
            systime: SYSTIME.into(),
            conns: vec![
                conn(1, "100.70.85.80", "personal", "2026/11/10 18:00:00"),
                conn(2, "100.101.4.7", "personal", "2026/11/10 18:00:00"),
                conn(3, "100.70.85.80", "personal", "2026/11/11 14:59:00"),
            ],
        };
        // Away: its addresses now are a hotel's LAN one and its tailnet one; the live session is
        // the newest, tunnelled.
        let m = Mine { now: vec![ip("10.0.0.7"), ip("100.70.85.80")], before: NOW, home: None };
        assert_eq!(pids(&pick(&l, "brandontsang", "personal", &m, NOW)), vec![1]);
        // Home again, mounted by the LAN name since: the tunnelled ones are older, both dropped.
        let mut home = l.clone();
        home.conns.push(conn(4, "192.168.2.15", "personal", "2026/11/11 14:59:30"));
        let m = Mine { now: vec![ip("192.168.2.15"), ip("100.70.85.80")], before: NOW, home: None };
        assert_eq!(pids(&pick(&home, "brandontsang", "personal", &m, NOW)), vec![1, 3]);
    }

    /// Mounted by the LAN name after a mount through Tailscale (home again): a drop of its
    /// addresses as of then, with every tailnet address it had in the run (one Tailscale holds no
    /// longer too), once; never while the share stays on the LAN or was never tunnelled.
    #[test]
    fn the_watch_asks_once_the_share_is_on_the_lan_again() {
        let d = tempfile::tempdir().unwrap();
        let mut w = Watch::new(1000, d.path());
        let lan = |now, addrs: &[&str]| Seen {
            now,
            home: true,
            nas: true,
            lan: true,
            addrs: addrs.iter().map(|a| ip(a)).collect(),
        };
        let away =
            Seen { now: 1020, home: false, nas: true, lan: false, addrs: vec![ip("10.0.0.7"), ip("100.70.85.80")] };
        assert_eq!(w.round(&lan(1000, &["192.168.2.15"]))[0].0, Why::Start);
        assert!(w.round(&lan(1010, &["192.168.2.15"])).is_empty(), "on the LAN, never tunnelled");
        assert!(w.round(&away).iter().all(|(why, _)| *why == Why::Left));
        // Home, still on the tunnel (the remount waits for idle), then not mounted for a round:
        // nothing yet.
        assert!(w.round(&Seen { home: true, now: 1040, ..away }).is_empty());
        let unmounted = Seen { now: 1050, home: true, nas: false, lan: false, addrs: vec![ip("192.168.2.15")] };
        assert!(w.round(&unmounted).is_empty());
        let a = w.round(&lan(1060, &["192.168.2.15"]));
        assert_eq!(
            a,
            vec![(Why::Lan, Mine { now: vec![ip("192.168.2.15"), ip("100.70.85.80")], before: 1060, home: None })]
        );
        assert!(w.round(&lan(1080, &["192.168.2.15", "100.70.85.80"])).is_empty(), "once");
    }

    fn seen(now: u64, home: bool, nas: bool) -> Seen {
        Seen { now, home, nas, lan: false, addrs: vec![ip("192.168.2.15"), ip("100.70.85.80")] }
    }

    #[test]
    fn the_watch_asks_at_start_and_on_leaving() {
        let d = tempfile::tempdir().unwrap();
        let mut w = Watch::new(1000, d.path());
        // (No NAS yet: no start's drop.)
        assert!(w.round(&seen(1000, true, false)).is_empty());
        let a = w.round(&seen(1020, true, true));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0, Why::Start);
        assert_eq!(a[0].1, Mine { now: seen(0, true, true).addrs, before: 1000, home: None });
        // (The home addresses kept, the tunnel's left out.)
        let kept: Home = serde_json::from_slice(&std::fs::read(d.path().join(HOME_FILE)).unwrap()).unwrap();
        assert_eq!(kept, Home { addrs: vec![ip("192.168.2.15")], at: 1000 });
        assert!(w.round(&seen(1040, true, true)).is_empty(), "once");
        // Left: the addresses it had at home, as last seen there.
        let a = w.round(&seen(1060, false, false));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].0, Why::Left);
        assert_eq!(a[0].1.home, Some(Home { addrs: vec![ip("192.168.2.15")], at: 1040 }));
        assert!(a[0].1.now.is_empty());
        assert!(w.round(&seen(1080, false, true)).is_empty(), "once");
        // Home again, then left again: once more.
        assert!(w.round(&seen(1100, true, true)).is_empty());
        assert_eq!(
            w.round(&seen(1120, false, true))[0].1.home,
            Some(Home { addrs: vec![ip("192.168.2.15")], at: 1100 })
        );
    }

    /// An agent started away uses the home addresses its Mac kept, within a day of leaving.
    #[test]
    fn started_away_it_uses_the_kept_home_addresses_for_a_day() {
        let d = tempfile::tempdir().unwrap();
        let h = Home { addrs: vec![ip("192.168.2.15")], at: 5000 };
        std::fs::write(d.path().join(HOME_FILE), serde_json::to_vec(&h).unwrap()).unwrap();
        let mut w = Watch::new(6000, d.path());
        let a = w.round(&seen(6000, false, true));
        assert_eq!(a[0].0, Why::Start);
        assert_eq!(a[0].1.home, Some(h.clone()));
        let mut w = Watch::new(5000 + HOME_FOR + 1, d.path());
        assert_eq!(w.round(&seen(5000 + HOME_FOR + 1, false, true))[0].1.home, None);
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

    fn serve(fake: Arc<RwLock<Fake>>) -> String {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        // (store::spawn::accept takes only a connection waiting: polled.)
        l.set_nonblocking(true).unwrap();
        std::thread::spawn(move || loop {
            let mut s = match store::spawn::accept(&l) {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(_) => return,
            };
            s.set_nonblocking(false).unwrap();
            {
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
                let b = answer(&mut fake.write().unwrap(), line.trim(), form).to_string();
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
        let fake = Arc::new(RwLock::new(Fake {
            conns: vec![
                conn(1, "192.168.2.15", "personal", "2026/11/08 09:09:51"),
                conn(2, "192.168.2.33", "personal", "2026/11/11 06:58:17"),
                conn(3, "100.101.4.7", "personal", "2026/11/11 10:00:00"),
                conn(4, "192.168.2.15", "personal", "2026/11/11 14:59:00"),
            ],
            ..Default::default()
        }));
        let api = Api::at(&serve(fake.clone()));
        let m = Mine { now: vec![ip("192.168.2.15")], before: NOW - 600, home: None };
        let d = api.drop_mine(&keys(), &m, NOW).unwrap();
        assert_eq!(pids(&d), vec![1]);
        let f = fake.write().unwrap();
        assert_eq!(pids(&f.conns), vec![2, 3, 4]);
        assert!(f.sessions.is_empty(), "logged out");
        let methods: Vec<String> =
            f.asked.iter().map(|(_, form)| form.iter().find(|(k, _)| k == "method").unwrap().1.clone()).collect();
        assert_eq!(methods, ["login", "list", "kick_connection", "logout"]);
        assert!(f.asked.iter().all(|(line, _)| line.starts_with("POST /webapi/entry.cgi ") && !line.contains("pw")));
    }

    /// A kick refused for one connection (gone by then) ends the others: what ended is read back,
    /// once.
    #[test]
    fn a_partly_refused_kick_says_what_ended() {
        let fake = Arc::new(RwLock::new(Fake {
            conns: vec![
                conn(1, "192.168.2.15", "personal", "2026/11/08 09:09:51"),
                conn(2, "192.168.2.15", "personal", "2026/11/09 09:09:51"),
                conn(3, "192.168.2.15", "personal", "2026/11/11 14:59:00"),
            ],
            unkickable: vec![2],
            ..Default::default()
        }));
        let api = Api::at(&serve(fake.clone()));
        let m = Mine { now: vec![ip("192.168.2.15")], before: NOW, home: None };
        assert_eq!(pids(&api.drop_mine(&keys(), &m, NOW).unwrap()), vec![1]);
        assert_eq!(fake.read().unwrap().asked.len(), 5, "login, list, kick, list, logout");
        // All refused: the error.
        let e = api.drop_mine(&keys(), &m, NOW).unwrap_err();
        assert!(matches!(e, Failed::Refused(_)) && e.to_string().contains("105"), "{e}");
        assert!(fake.write().unwrap().sessions.is_empty(), "logged out after a failure too");
    }

    /// Bad keys are refused, saying so; a DSM not there is "not reached" (tried again).
    #[test]
    fn failures_say_why() {
        let fake = Arc::new(RwLock::new(Fake::default()));
        let api = Api::at(&serve(fake));
        let bad = parse_keys("NAS_DSM_USER=scenic-sessions\nNAS_DSM_PASS=wrong\n").unwrap();
        let m = Mine { now: vec![ip("192.168.2.15")], before: NOW, home: None };
        let e = api.drop_mine(&bad, &m, NOW).unwrap_err();
        assert!(matches!(e, Failed::Refused(_)) && e.to_string().contains("wrong password"), "{e}");
        let closed = store::spawn::listen("127.0.0.1:0").unwrap();
        let base = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        assert!(matches!(Api::at(&base).drop_mine(&keys(), &m, NOW), Err(Failed::Unreached(_))));
    }

    /// TLS that fails (here a server that doesn't speak it; a lapsed certificate fails the same
    /// way, rustls's InvalidData) is refused, saying so, and not tried again as unreached.
    #[test]
    fn a_failed_tls_handshake_is_refused() {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        let base = format!("https://{}", l.local_addr().unwrap());
        l.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            for _ in 0..5000 {
                match store::spawn::accept(&l) {
                    Ok((mut s, _)) => {
                        s.set_nonblocking(false).ok();
                        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").ok();
                        std::thread::sleep(Duration::from_millis(500));
                        return;
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        let m = Mine { now: vec![ip("192.168.2.15")], before: NOW, home: None };
        let e = Api::at(&base).drop_mine(&keys(), &m, NOW).unwrap_err();
        assert!(matches!(e, Failed::Refused(_)) && e.to_string().contains("certificate"), "{e}");
    }
}
