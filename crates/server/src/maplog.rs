//! The server follows the map's log (docs/maplog.md §5, §6.1): a thread long-polls the lead's
//! coordinator (`GET /map/changes`, pipeline::coord::maproutes), applies each answer's batches whole
//! to its own map (each checked by its digest; one that doesn't apply fetches the snapshot), and
//! hands the result on as one catalogue, swapped whole (`Data::set_map`): a request never reads a
//! torn set. Its requests say its cursor (`client=server/<host>`). It runs wherever the server runs:
//! the Mac mini's, and a laptop's on demand while it runs (#874, #882 step 7a; crate::retire). The
//! map is all it serves: no catalogue is read
//! (docs/maplog.md §8, step 6).
//!
//! The mirror is a client of the log too, with a cursor of its own (`MirrorLink`).
//!
//! The map is kept on this Mac (`current.json.zst` in its cache folder, ~/Library/Caches/scenic/map),
//! written at most every 10 minutes, and read at a start: a start away from the lead and the NAS
//! serves it (the offline map). With the lead away it keeps the map it holds, asking again each
//! `retry`; holding none (or an older one than the NAS's), it takes the newest NAS snapshot
//! (`map/<seq>.json.zst`).

use crate::data::Data;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use store::map::feed::Answer;
use store::map::{Map, Snapshot};

/// The saved map, and the lead's contact kept for a start away from the NAS.
const SAVED: &str = "current.json.zst";
const LEAD_KEPT: &str = "lead.json";
/// The saved map written at most this often by a resident server (`Config::save_every`).
pub(crate) const SAVE_EVERY: Duration = Duration::from_secs(600);
/// The NAS's snapshots looked at at most this often.
const NAS_EVERY: Duration = Duration::from_secs(600);

/// Where the lead is.
#[derive(Clone, Debug)]
pub enum Lead {
    /// As the NAS says (`state/coordinator.json`), kept on this Mac.
    FromNas,
    /// At fixed addresses (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    At { urls: Vec<String>, token: String },
}

#[derive(Clone, Debug)]
pub struct Config {
    /// The folder of the saved map (None: none kept).
    pub cache: Option<PathBuf>,
    pub lead: Lead,
    /// How long a request waits for a batch (`?wait=`), and how soon a lead that didn't answer is
    /// asked again.
    pub wait_s: u64,
    pub retry: Duration,
    /// This server's cursor's name.
    pub client: String,
    /// The saved map written at most this often once the map moved: `SAVE_EVERY`; none for a
    /// server on demand (`on_demand`), which may quit any time and whose downloads are of the map
    /// it serves.
    pub save_every: Duration,
    /// Set to stop following (tests).
    pub stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Config {
    /// This Mac's: the lead as the NAS says, the map kept in ~/Library/Caches/scenic/map.
    pub fn this_mac() -> Config {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        Config {
            cache: Some(home.join("Library/Caches/scenic/map")),
            lead: Lead::FromNas,
            wait_s: store::map::feed::WAIT_S,
            retry: Duration::from_secs(10),
            client: format!("server/{}", crate::data::host()),
            save_every: SAVE_EVERY,
            stop: Default::default(),
        }
    }

    /// `this_mac`, for a server started on demand (a laptop's offline map): the map kept each time
    /// it moves, so the map a later start away from the lead and the NAS reads is the one its
    /// downloads were copied for.
    pub fn on_demand() -> Config {
        Config { save_every: Duration::ZERO, ..Config::this_mac() }
    }
}

/// Follows the log on a thread of its own, for good: first, before it returns, the map this Mac
/// kept, else the NAS's newest snapshot, served.
pub fn spawn(d: Arc<Data>, cfg: Config) {
    let mut f = Follower::new(d, cfg);
    f.start();
    if f.map.is_none() {
        f.from_nas();
    }
    std::thread::Builder::new().name("maplog".into()).spawn(move || f.run()).ok();
}

/// What a request to the lead gave.
enum Got {
    Answer(Answer),
    /// It answered without a map (503: not the lead, its log not open), or not at all.
    Away(String),
}

pub(crate) struct Follower {
    d: Arc<Data>,
    cfg: Config,
    link: Link,
    map: Option<Map>,
    saved: Option<(u64, Instant)>,
    nas_looked: Option<Instant>,
    /// The map's last `seq` handed on.
    handed: u64,
    away: bool,
}

impl Follower {
    pub(crate) fn new(d: Arc<Data>, cfg: Config) -> Follower {
        let link = Link::new(d.clone(), &cfg);
        Follower { d, cfg, link, map: None, saved: None, nas_looked: None, handed: 0, away: false }
    }

    fn run(mut self) {
        while !self.cfg.stop.load(std::sync::atomic::Ordering::SeqCst) {
            if !self.round() {
                std::thread::sleep(self.cfg.retry);
            }
        }
    }

    /// The map this Mac kept, served.
    pub(crate) fn start(&mut self) {
        let Some(dir) = self.cfg.cache.clone() else { return };
        // (The copy an older server kept of the map-serve switch, gone with it.)
        std::fs::remove_file(dir.join("serve")).ok();
        match std::fs::read(dir.join(SAVED)) {
            Ok(b) => match Snapshot::decode(&b) {
                Ok((m, _)) => {
                    eprintln!("maplog: the map at {} from this Mac", m.seq);
                    self.saved = Some((m.seq, Instant::now()));
                    self.adopt(m);
                }
                Err(e) => {
                    eprintln!("maplog: this Mac's map set aside: {e:#}");
                    std::fs::rename(dir.join(SAVED), dir.join("current.bad.json.zst")).ok();
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("maplog: this Mac's map: {e}"),
        }
    }

    /// One request to the lead and what follows from it; false when the lead didn't answer (wait
    /// `retry` before the next).
    pub(crate) fn round(&mut self) -> bool {
        let since = self.map.as_ref().map_or(0, |m| m.seq);
        let answered = match self.ask(since) {
            Got::Answer(a) => {
                if self.away {
                    eprintln!("maplog: the lead answers again");
                    self.away = false;
                }
                match a.reset {
                    Some(head) => self.reset(head),
                    None => self.apply(a),
                }
                true
            }
            Got::Away(why) => {
                if !self.away {
                    eprintln!("maplog: the lead doesn't answer ({why}); asked again each {:?}", self.cfg.retry);
                    self.away = true;
                }
                self.from_nas();
                false
            }
        };
        self.save();
        answered
    }

    /// The batches of `a` applied in turn to the map, then handed on once; one that doesn't apply
    /// (its digest, a key missing) fetches the snapshot instead.
    fn apply(&mut self, a: Answer) {
        let Some(m) = self.map.as_mut() else {
            // (None held, yet batches: the lead answered from 0. Its snapshot.)
            return self.reset(a.head);
        };
        let mut wrong = None;
        for b in &a.batches {
            if b.seq <= m.seq {
                continue;
            }
            match m.apply_in(b) {
                Ok(u) => drop(u),
                Err(e) => {
                    wrong = Some(e);
                    break;
                }
            }
        }
        self.hand_on();
        if let Some(e) = wrong {
            eprintln!("maplog: MISMATCH: {e:#}; the lead's snapshot instead");
            self.reset(a.head);
        }
    }

    /// The lead's snapshot taken whole (a reset, a batch that didn't apply).
    fn reset(&mut self, head: u64) {
        match self.snapshot() {
            Ok(m) => {
                eprintln!("maplog: the lead's map at {} (asked from {head})", m.seq);
                self.adopt(m);
            }
            Err(e) => eprintln!("maplog: the lead's snapshot: {e:#}"),
        }
    }

    /// The newest NAS snapshot, when this server holds no map or an older one (at most each
    /// `NAS_EVERY` while it holds one).
    fn from_nas(&mut self) {
        if self.map.is_some() && self.nas_looked.is_some_and(|t| t.elapsed() < NAS_EVERY) {
            return;
        }
        self.nas_looked = Some(Instant::now());
        let Some(root) = self.d.nas_root() else { return };
        match store::map::newest_nas(&root) {
            Ok(Some(m)) if self.map.as_ref().is_none_or(|h| h.seq < m.seq) => {
                eprintln!("maplog: the NAS's map at {} (the lead away)", m.seq);
                self.adopt(m);
            }
            Ok(_) => {}
            Err(e) => eprintln!("maplog: the NAS's map: {e:#}"),
        }
    }

    fn adopt(&mut self, m: Map) {
        self.map = Some(m);
        self.hand_on();
    }

    /// The map's catalogue handed to the server, when it moved on.
    fn hand_on(&mut self) {
        let Some(m) = &self.map else { return };
        if m.seq == self.handed {
            return;
        }
        match m.catalogue() {
            Ok(c) => {
                self.handed = m.seq;
                self.d.set_map(Arc::new(c));
            }
            Err(e) => eprintln!("maplog: MISMATCH: the map at {} isn't a catalogue: {e:#}", m.seq),
        }
    }

    /// The map kept on this Mac, when it moved on and the last save is `save_every` old.
    fn save(&mut self) {
        let (Some(dir), Some(m)) = (&self.cfg.cache, &self.map) else { return };
        if self.saved.is_some_and(|(s, t)| s == m.seq || t.elapsed() < self.cfg.save_every) {
            return;
        }
        let r = (|| -> Result<()> {
            std::fs::create_dir_all(dir)?;
            let b = Snapshot::of(m, &Default::default()).encode()?;
            let tmp = dir.join(format!("{SAVED}.tmp"));
            std::fs::write(&tmp, b)?;
            std::fs::rename(&tmp, dir.join(SAVED))?;
            Ok(())
        })();
        match r {
            Ok(()) => self.saved = Some((m.seq, Instant::now())),
            Err(e) => eprintln!("maplog: keeping the map on this Mac: {e:#}"),
        }
    }

    fn ask(&mut self, since: u64) -> Got {
        let path = format!("/map/changes?since={since}&wait={}&client={}", self.cfg.wait_s, self.cfg.client);
        match self.link.get(&path, self.cfg.wait_s + 30) {
            Ok(b) => match serde_json::from_slice::<Answer>(&b) {
                Ok(a) => Got::Answer(a),
                Err(e) => Got::Away(format!("its answer doesn't parse: {e}")),
            },
            Err(why) => Got::Away(why),
        }
    }

    fn snapshot(&mut self) -> Result<Map> {
        let b = match self.link.get("/map/snapshot", 600) {
            Ok(b) => b,
            Err(why) => bail!("{why}"),
        };
        Ok(Snapshot::decode(&b).context("the lead's snapshot")?.0)
    }

    #[cfg(test)]
    pub(crate) fn seq(&self) -> Option<u64> {
        self.map.as_ref().map(|m| m.seq)
    }
}

/// The way to the lead's coordinator: its addresses and key, fixed or as the NAS says.
struct Link {
    d: Arc<Data>,
    lead: Lead,
    /// Where the lead's contact is kept for a start away from the NAS.
    cache: Option<PathBuf>,
    contact: Option<pipeline::coord::Contact>,
}

impl Link {
    fn new(d: Arc<Data>, cfg: &Config) -> Link {
        Link { d, lead: cfg.lead.clone(), cache: cfg.cache.clone(), contact: None }
    }

    /// The lead's addresses and key: fixed, or as the NAS says (kept on this Mac), read again
    /// after the lead didn't answer.
    fn contact(&mut self) -> Option<pipeline::coord::Contact> {
        if let Lead::At { urls, token } = &self.lead {
            return Some(pipeline::coord::Contact { urls: urls.clone(), token: token.clone(), page: None });
        }
        if self.contact.is_none() {
            let kept = self.cache.as_ref().map(|d| d.join(LEAD_KEPT));
            let from_nas = self.d.nas_root().and_then(|r| pipeline::coord::contact_path(&r).read().ok());
            let read = from_nas.as_deref().and_then(|b| serde_json::from_slice::<pipeline::coord::Contact>(b).ok());
            if let (Some(c), Some(k)) = (&read, &kept) {
                // (Written only when it changed: this is read again each `retry` while the lead's away.)
                if let (Some(dir), Ok(b)) = (k.parent(), serde_json::to_vec(c)) {
                    if std::fs::read(k).ok().as_deref() != Some(&b[..]) {
                        std::fs::create_dir_all(dir).ok();
                        keep_private(k, &b);
                    }
                }
            }
            self.contact =
                read.or_else(|| kept.and_then(|k| std::fs::read(k).ok()).and_then(|b| serde_json::from_slice(&b).ok()));
        }
        self.contact.clone()
    }

    fn agent(&self, secs: u64) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(secs)))
            .http_status_as_error(false)
            .build()
            .into()
    }

    /// A GET of `path` at the lead's addresses in turn: the first 200's body.
    fn get(&mut self, path: &str, secs: u64) -> std::result::Result<Vec<u8>, String> {
        let Some(c) = self.contact() else { return Err("no contact for the lead".into()) };
        let mut why = String::from("no address");
        let _w = store::waits::net();
        for url in &c.urls {
            let r = self
                .agent(secs)
                .get(&format!("{url}{path}"))
                .header("Authorization", &format!("Bearer {}", c.token))
                .call();
            match r {
                Ok(mut resp) if resp.status() == 200 => {
                    match resp.body_mut().with_config().limit(1 << 30).read_to_vec() {
                        Ok(b) => return Ok(b),
                        Err(e) => why = format!("{url}: {e}"),
                    }
                }
                Ok(resp) => why = format!("{url}: {}", resp.status()),
                Err(e) => why = format!("{url}: {e}"),
            }
        }
        // (Read from the NAS again next time: the lead may have moved, or made a new key.)
        self.contact = None;
        Err(why)
    }
}

/// The mirror's client of the map's log (docs/maplog.md §6.2), `mirror/<host>`: each round it says
/// its cursor, the oldest map whose files it still copies from the NAS (`since`), so the lead's GC
/// keeps every file a map since then names; the map its round copies (`copying`) and the last it
/// was through at (`synced`), which the lead notes in the history for publishing's times. Its
/// cursor is made each round from what's on this Mac's disk (the downloads' pins and the files
/// here: downloads::Plan::copies_from), so it is as durable as they are and never newer than
/// what's copied; the batches answered are passed over (the mirror's sync is whole each round).
pub struct MirrorLink {
    link: Link,
    client: String,
    synced: Option<u64>,
}

impl MirrorLink {
    /// The mirror's link, for the server following the lead of `cfg` (its cursor `mirror/<host>`).
    pub fn new(d: Arc<Data>, cfg: &Config) -> MirrorLink {
        let host = cfg.client.strip_prefix(pipeline::agent::SERVER_CURSOR).unwrap_or(&cfg.client);
        MirrorLink {
            link: Link::new(d, cfg),
            client: format!("{}{host}", pipeline::agent::MIRROR_CURSOR),
            synced: None,
        }
    }

    /// Says the cursor (`since`) and the map the round copies: the lead's head when the cursor is
    /// outside the batches it holds (its answer a reset). The lead away, asked again next round.
    pub fn tell(&mut self, since: u64, copying: u64) -> Option<u64> {
        let mut path = format!("/map/changes?since={since}&client={}&copying={copying}", self.client);
        if let Some(s) = self.synced {
            path.push_str(&format!("&synced={s}"));
        }
        let b = self.link.get(&path, 10).ok()?;
        serde_json::from_slice::<Answer>(&b).ok()?.reset
    }

    /// A round through everything wanted at map `n`: said at once (with the cursor `since`) when
    /// it's the first through there.
    pub fn through(&mut self, n: u64, since: u64) {
        if self.synced != Some(n) {
            self.synced = Some(n);
            self.tell(since, n);
        }
    }
}

/// A file only this user reads (it holds the build's key).
fn keep_private(path: &std::path::Path, b: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let w = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(b));
    if w.is_ok() {
        std::fs::rename(&tmp, path).ok();
    }
}

#[cfg(test)]
pub(crate) mod tests;
