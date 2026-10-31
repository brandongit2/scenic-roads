//! The server follows the map's log (docs/maplog.md §5, §6.1; §8, step 3): a thread long-polls the
//! lead's coordinator (`GET /map/changes`, pipeline::coord::maproutes), applies each answer's
//! batches whole to its own map (each checked by its digest; one that doesn't apply fetches the
//! snapshot), and hands the result on as one catalogue, swapped whole (`Data::set_from_map`): a
//! request never reads a torn set. Its requests say its cursor (`client=server/<host>`).
//!
//! With the lead away it keeps the map it holds, asking again each `retry`; holding none (or an
//! older one than the NAS's), it takes the newest NAS snapshot (`map/<seq>.json.zst`). Neither:
//! the catalogues are served as before (Data::refresh_catalogue). The map is kept on this Mac
//! (`current.json.zst` in its cache folder, ~/Library/Caches/scenic/map), written at most every 10
//! minutes, and read at a start.
//!
//! Shadow first, then serve: the map is checked against the catalogue the catalogues give at the
//! same number (Map::check_is, logged `maplog: MISMATCH` when they differ), and served only while
//! the switch `state/pool/map-serve` on the NAS is on (kept on this Mac for starts away from it).

use crate::data::Data;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use store::map::feed::Answer;
use store::map::{Map, Snapshot};

/// The switch that serves the map (on the NAS), and this Mac's copy of it (in the cache folder).
pub const SERVE_SWITCH: &str = "state/pool/map-serve";
const SERVE_KEPT: &str = "serve";
/// The saved map, and the lead's contact kept for a start away from the NAS.
const SAVED: &str = "current.json.zst";
const LEAD_KEPT: &str = "lead.json";
/// The saved map written at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(600);
/// The NAS's snapshots and the switch looked at at most this often.
const NAS_EVERY: Duration = Duration::from_secs(600);
const SWITCH_EVERY: Duration = Duration::from_secs(60);

/// Where the lead is.
#[derive(Clone, Debug)]
pub enum Lead {
    /// As the NAS says (`state/coordinator.json`), kept on this Mac.
    FromNas,
    /// At fixed addresses (tests).
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
            stop: Default::default(),
        }
    }
}

/// Follows the log on a thread of its own, for good.
pub fn spawn(d: Arc<Data>, cfg: Config) {
    let f = Follower::new(d, cfg);
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
    map: Option<Map>,
    contact: Option<pipeline::coord::Contact>,
    saved: Option<(u64, Instant)>,
    nas_looked: Option<Instant>,
    switch_read: Option<Instant>,
    /// The last number checked against the catalogues, and the map's last handed on.
    checked: u64,
    handed: u64,
    away: bool,
}

impl Follower {
    pub(crate) fn new(d: Arc<Data>, cfg: Config) -> Follower {
        Follower {
            d,
            cfg,
            map: None,
            contact: None,
            saved: None,
            nas_looked: None,
            switch_read: None,
            checked: 0,
            handed: 0,
            away: false,
        }
    }

    fn run(mut self) {
        self.start();
        while !self.cfg.stop.load(std::sync::atomic::Ordering::SeqCst) {
            if !self.round() {
                std::thread::sleep(self.cfg.retry);
            }
        }
    }

    /// The saved map and the switch as this Mac kept them.
    pub(crate) fn start(&mut self) {
        let Some(dir) = self.cfg.cache.clone() else { return };
        if std::fs::read_to_string(dir.join(SERVE_KEPT)).is_ok_and(|s| s.trim() == "on") {
            self.d.set_serve_map(true);
        }
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
        self.read_switch();
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
        self.check();
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
        let dir = root.join(store::map::NAS_DIR);
        let Ok(names) = dir.list() else { return };
        let Some(seq) = names.iter().filter_map(|n| n.strip_suffix(".json.zst")?.parse::<u64>().ok()).max() else {
            return;
        };
        if self.map.as_ref().is_some_and(|m| m.seq >= seq) {
            return;
        }
        let got =
            dir.join(store::map::nas_name(seq)).read().map_err(anyhow::Error::from).and_then(|b| Snapshot::decode(&b));
        match got {
            Ok((m, _)) => {
                eprintln!("maplog: the NAS's map at {} (the lead away)", m.seq);
                self.adopt(m);
            }
            Err(e) => eprintln!("maplog: the NAS's map at {seq}: {e:#}"),
        }
    }

    fn adopt(&mut self, m: Map) {
        self.map = Some(m);
        self.hand_on();
    }

    /// The map's catalogue handed to the server, while the map is served and it moved on.
    fn hand_on(&mut self) {
        let Some(m) = &self.map else { return };
        if !self.d.serves_map() || m.seq == self.handed {
            return;
        }
        match m.catalogue() {
            Ok(c) => {
                self.handed = m.seq;
                self.d.set_from_map(Arc::new(c));
            }
            Err(e) => eprintln!("maplog: MISMATCH: the map at {} isn't a catalogue: {e:#}", m.seq),
        }
    }

    /// The map checked against the catalogue the catalogues give, once per number they share.
    fn check(&mut self) {
        let Some(m) = &self.map else { return };
        let c = self.d.read_catalogue();
        if c.n != m.seq || c.n == self.checked || c.n == 0 {
            return;
        }
        self.checked = c.n;
        match m.check_is(&c) {
            Ok(()) => eprintln!("maplog: the map at {} is catalogue {}", m.seq, c.n),
            Err(e) => eprintln!("maplog: MISMATCH at {}: {e:#}", c.n),
        }
    }

    /// The map kept on this Mac, when it moved on and the last save is `SAVE_EVERY` old.
    fn save(&mut self) {
        let (Some(dir), Some(m)) = (&self.cfg.cache, &self.map) else { return };
        if self.saved.is_some_and(|(s, t)| s == m.seq || t.elapsed() < SAVE_EVERY) {
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

    /// The switch on the NAS, at most each `SWITCH_EVERY`, kept on this Mac; as it was while it
    /// can't be read.
    fn read_switch(&mut self) {
        if matches!(self.cfg.lead, Lead::At { .. }) || self.switch_read.is_some_and(|t| t.elapsed() < SWITCH_EVERY) {
            return;
        }
        self.switch_read = Some(Instant::now());
        let Some(root) = self.d.nas_root() else { return };
        let on = match root.join(SERVE_SWITCH).read() {
            Ok(b) => String::from_utf8_lossy(&b).trim() != "off",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return,
        };
        if on != self.d.serves_map() {
            self.d.set_serve_map(on);
            if let Some(dir) = &self.cfg.cache {
                std::fs::create_dir_all(dir).ok();
                std::fs::write(dir.join(SERVE_KEPT), if on { "on\n" } else { "off\n" }).ok();
            }
            self.handed = 0;
            self.hand_on();
        }
    }

    /// The lead's addresses and key: fixed, or as the NAS says (kept on this Mac), read again
    /// after the lead didn't answer.
    fn contact(&mut self) -> Option<pipeline::coord::Contact> {
        if let Lead::At { urls, token } = &self.cfg.lead {
            return Some(pipeline::coord::Contact { urls: urls.clone(), token: token.clone(), page: None });
        }
        if self.contact.is_none() {
            let kept = self.cfg.cache.as_ref().map(|d| d.join(LEAD_KEPT));
            let from_nas = self.d.nas_root().and_then(|r| pipeline::coord::contact_path(&r).read().ok());
            let read = from_nas.as_deref().and_then(|b| serde_json::from_slice::<pipeline::coord::Contact>(b).ok());
            if let (Some(c), Some(k)) = (&read, &kept) {
                if let (Some(dir), Ok(b)) = (k.parent(), serde_json::to_vec(c)) {
                    std::fs::create_dir_all(dir).ok();
                    keep_private(k, &b);
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

    fn ask(&mut self, since: u64) -> Got {
        let path = format!("/map/changes?since={since}&wait={}&client={}", self.cfg.wait_s, self.cfg.client);
        match self.get(&path, self.cfg.wait_s + 30) {
            Ok(b) => match serde_json::from_slice::<Answer>(&b) {
                Ok(a) => Got::Answer(a),
                Err(e) => Got::Away(format!("its answer doesn't parse: {e}")),
            },
            Err(why) => Got::Away(why),
        }
    }

    fn snapshot(&mut self) -> Result<Map> {
        let b = match self.get("/map/snapshot", 600) {
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
mod tests;
