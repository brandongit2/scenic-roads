//! The devices that help through the build page (docs/workers.md §7). A page asks the build Mac to
//! let it help (`/work/join`), with a secret it made and alone keeps; the build Mac gives the ask a
//! name and a code of its own making (the code unlike any other ask's waiting), and only that page
//! and the build Mac's owner see the code. The owner accepts or declines the ask there (the menu
//! bar, `scenic devices`) by its code: the device whose page shows it. An accepted device's secret
//! is its key from then on, for its own tasks and to pause the build, until the owner forgets it.
//! Kept in the coordinator's `devices.json`: the secrets' hashes, never the secrets.
//!
//! An ask never takes another's place: one with another secret is another ask, whatever page it
//! says it is, and an answer is for one ask alone. Asks are few: past ASKS_MAX waiting, FROM_ONE
//! from an address, or HOURLY an address made in the last hour, the next is refused, never one
//! waiting dropped for it.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::Path;

/// How long a declined ask is remembered (its page learns so), and an unanswered one kept.
const DECLINED_S: u64 = 600;
const ASK_S: u64 = 86400;
/// The most asks waiting at once, all told and from one address, and the most an address makes in
/// an hour.
const ASKS_MAX: usize = 8;
const FROM_ONE: usize = 3;
const HOURLY: usize = 10;

/// A device: asking, accepted or declined.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// The ask's name, made here and never again: what the owner's answer names (with its code),
    /// and what a device is forgotten by.
    pub ask: String,
    /// The page's own id (its browser's), and what it is ("Safari on iPad").
    pub id: String,
    pub label: String,
    /// SHA-256 of its secret, hex.
    pub hash: String,
    /// The four digits its page shows, and the owner with its ask.
    pub code: String,
    /// Where it asked from (crate::net::source), and when (unix seconds).
    #[serde(default)]
    pub from: String,
    pub asked: u64,
    /// When it was accepted, or declined (0: not).
    #[serde(default)]
    pub accepted: u64,
    #[serde(default)]
    pub declined: u64,
}

impl Device {
    fn waiting(&self) -> bool {
        self.accepted == 0 && self.declined == 0
    }

    fn state(&self) -> State {
        match () {
            _ if self.accepted > 0 => State::Accepted,
            _ if self.declined > 0 => State::Declined,
            _ => State::Asking,
        }
    }
}

/// An ask, or an accepted device, as the build Mac's owner sees it (`/work/devices`, never a page).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    pub ask: String,
    pub id: String,
    pub label: String,
    pub code: String,
    pub from: String,
    /// When it asked, or was accepted.
    pub at: u64,
}

/// What the build Mac's owner sees: the asks waiting, and the devices accepted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub asking: Vec<Shown>,
    pub accepted: Vec<Shown>,
}

/// Where an ask stands, for its page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    None,
    Asking,
    Accepted,
    Declined,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::None => "none",
            State::Asking => "asking",
            State::Accepted => "accepted",
            State::Declined => "declined",
        }
    }
}

/// A page's ask, as it came: its id, what it is, its secret, and where from (crate::net::source).
pub struct Asked<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub secret: &'a str,
    pub from: &'a str,
}

/// What an ask got.
#[derive(Debug, PartialEq)]
pub enum Joined {
    /// Where it stands, and its code while it waits (else ""); `new`: it's a new ask (else its page
    /// asking again with the same secret: as it was).
    Stands { state: State, code: String, new: bool },
    /// Refused: too many asks waiting, or made lately (why, for its page).
    TooMany(&'static str),
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Devices {
    #[serde(default)]
    pub list: Vec<Device>,
    /// The asks taken in the last hour (where from, when), counted against HOURLY.
    #[serde(skip)]
    recent: Vec<(String, u64)>,
}

/// A secret's hash, as kept.
pub fn hash(secret: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(secret.as_bytes()))
}

/// Whether a page's id, its label and its secret are what one sends: the label plain ASCII (what a
/// page makes of its browser's name), so nothing in it can pass for something else.
fn valid(id: &str, label: &str, secret: &str) -> bool {
    (1..=40).contains(&id.len())
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        && (1..=80).contains(&label.len())
        && label.bytes().all(|c| c.is_ascii_graphic() || c == b' ')
        && (32..=128).contains(&secret.len())
        && secret.bytes().all(|c| c.is_ascii_hexdigit())
}

impl Devices {
    /// The devices kept in `path`: none when there's no file. One that doesn't read is said so and
    /// set aside (`devices.json.bad`), and its devices ask again.
    pub fn load(path: &Path) -> Devices {
        match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                eprintln!("coordinator: {} doesn't read ({e}): set aside, and its devices ask again", path.display());
                std::fs::rename(path, path.with_extension("json.bad")).ok();
                Devices::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Devices::default(),
            Err(e) => {
                eprintln!("coordinator: reading {}: {e} (its devices ask again)", path.display());
                Devices::default()
            }
        }
    }

    /// Kept in `path`, whole and readable by this Mac's user alone from the start; flushed to the
    /// disk when `durable` (an answer, a device forgotten: an ask alone isn't, as one lost to a crash
    /// is only asked again).
    pub fn save(&self, path: &Path, durable: bool) -> Result<()> {
        use std::io::Write;
        let tmp = crate::whole::tmp_name(path);
        let r = (|| -> Result<()> {
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
            let mut f = o.open(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(self)?)?;
            if durable {
                f.sync_all()?;
            }
            drop(f);
            crate::whole::rename_over(&tmp, path)?;
            Ok(())
        })();
        if r.is_err() {
            std::fs::remove_file(&tmp).ok();
        }
        r
    }

    /// Asks gone stale: a declined one past DECLINED_S, an unanswered one past ASK_S; and the asks
    /// counted against HOURLY, an hour on.
    fn tidy(&mut self, now: u64) {
        self.list.retain(|d| d.accepted > 0 || (d.declined > 0 && now.saturating_sub(d.declined) < DECLINED_S) || (d.declined == 0 && now.saturating_sub(d.asked) < ASK_S));
        self.recent.retain(|(_, t)| now.saturating_sub(*t) < 3600);
    }

    /// A page's ask to help: kept, with a name and a code made here (`rand`: fresh randomness), and
    /// where it stands with its code; the same page asking again with the same secret, as it was. An
    /// error when what it sent isn't an ask.
    pub fn ask(&mut self, a: &Asked, now: u64, rand: u128) -> Result<Joined> {
        anyhow::ensure!(valid(a.id, a.label, a.secret), "an ask is a page's id (letters, digits, dashes), what it is (plain letters), and its secret (32 to 128 hex digits)");
        self.tidy(now);
        let h = hash(a.secret);
        if let Some(d) = self.list.iter().find(|d| d.id == a.id && d.hash == h) {
            return Ok(Joined::Stands { state: d.state(), code: if d.waiting() { d.code.clone() } else { String::new() }, new: false });
        }
        let waiting: Vec<&Device> = self.list.iter().filter(|d| d.waiting()).collect();
        if self.recent.iter().filter(|r| r.0 == a.from).count() >= HOURLY {
            return Ok(Joined::TooMany("this address asked too often lately: try again in an hour"));
        }
        if waiting.iter().filter(|d| d.from == a.from).count() >= FROM_ONE {
            return Ok(Joined::TooMany("asks from this address are waiting on the build Mac: answer them there, or cancel them on their pages"));
        }
        if waiting.len() >= ASKS_MAX {
            return Ok(Joined::TooMany("too many asks are waiting on the build Mac: answer them there first"));
        }
        // (Its code unlike any waiting: 7919 is prime to 10⁴, so the codes tried go through all of
        // them.)
        let first = (rand >> 64) as u64 % 10_000;
        let code = (0..10_000u64).map(|k| format!("{:04}", (first + k * 7919) % 10_000)).find(|c| !waiting.iter().any(|d| d.code == *c)).unwrap_or_default();
        let d = Device { ask: format!("{:016x}", rand as u64), id: a.id.into(), label: a.label.into(), hash: h, code, from: a.from.into(), asked: now, accepted: 0, declined: 0 };
        let joined = Joined::Stands { state: State::Asking, code: d.code.clone(), new: true };
        self.list.push(d);
        self.recent.push((a.from.to_string(), now));
        Ok(joined)
    }

    /// Where a page's ask stands (its secret proves it's the page's), and its code while it waits.
    pub fn state(&self, id: &str, secret: &str, now: u64) -> (State, String) {
        let h = hash(secret);
        match self.list.iter().find(|d| d.id == id && d.hash == h) {
            Some(d) if d.accepted > 0 => (State::Accepted, String::new()),
            Some(d) if d.declined > 0 && now.saturating_sub(d.declined) < DECLINED_S => (State::Declined, String::new()),
            Some(d) if d.waiting() && now.saturating_sub(d.asked) < ASK_S => (State::Asking, d.code.clone()),
            _ => (State::None, String::new()),
        }
    }

    /// A page's ask withdrawn by the page (its secret proves it's the page's): whether one was
    /// waiting.
    pub fn cancel(&mut self, id: &str, secret: &str) -> bool {
        let h = hash(secret);
        let n = self.list.len();
        self.list.retain(|d| !(d.id == id && d.hash == h && d.waiting()));
        self.list.len() < n
    }

    /// The owner's answer to the ask waiting with `code` (and named `ask`, when the answer names
    /// it: a notification's, for that ask alone): the ask answered, when there was one. Nothing
    /// else changes: an accepted device stays until it's forgotten.
    pub fn answer(&mut self, ask: Option<&str>, code: &str, accept: bool, now: u64) -> Option<Device> {
        self.tidy(now);
        let d = self.list.iter_mut().find(|d| d.waiting() && d.code == code && ask.is_none_or(|a| d.ask == a))?;
        if accept {
            d.accepted = now;
        } else {
            d.declined = now;
        }
        Some(d.clone())
    }

    /// Forgets the accepted devices named `which` (an ask's name, or a page's id): their secrets no
    /// longer work. Those forgotten.
    pub fn forget(&mut self, which: &str) -> Vec<Device> {
        let (gone, kept): (Vec<Device>, Vec<Device>) = std::mem::take(&mut self.list).into_iter().partition(|d| d.accepted > 0 && (d.ask == which || d.id == which));
        self.list = kept;
        gone
    }

    /// The accepted device whose secret `secret` is.
    pub fn knows(&self, secret: &str) -> Option<&Device> {
        let h = hash(secret);
        self.list.iter().find(|d| d.accepted > 0 && d.hash == h)
    }

    /// The asks waiting and the devices accepted, as the owner sees them.
    pub fn view(&self, now: u64) -> View {
        let shown = |d: &Device, at: u64| Shown { ask: d.ask.clone(), id: d.id.clone(), label: d.label.clone(), code: d.code.clone(), from: d.from.clone(), at };
        View {
            asking: self.list.iter().filter(|d| d.waiting() && now.saturating_sub(d.asked) < ASK_S).map(|d| shown(d, d.asked)).collect(),
            accepted: self.list.iter().filter(|d| d.accepted > 0).map(|d| shown(d, d.accepted)).collect(),
        }
    }
}

/// Whether worker name `worker` is accepted device `id`'s: its page's id, after what it is (the
/// page names itself "<label> <id>").
pub fn names(worker: &str, id: &str) -> bool {
    worker.strip_suffix(id).is_some_and(|w| w.ends_with(' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S1: &str = "0123456789abcdef0123456789abcdef";
    const S2: &str = "fedcba9876543210fedcba9876543210";
    const S3: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn asked<'a>(id: &'a str, secret: &'a str, from: &'a str) -> Asked<'a> {
        Asked { id, label: "Safari on iPad", secret, from }
    }

    fn code(j: Joined) -> String {
        match j {
            Joined::Stands { state: State::Asking, code, .. } => code,
            j => panic!("{j:?}"),
        }
    }

    #[test]
    fn a_page_asks_the_owner_answers_its_code_and_its_secret_is_its_key_until_forgotten() {
        let mut d = Devices::default();
        let c = code(d.ask(&asked("abc123", S1, "100.64.0.7"), 1000, (7 << 64) | 0xfeed).unwrap());
        assert_eq!(c, "0007");
        let v = d.view(1000);
        assert_eq!((v.asking[0].code.as_str(), v.asking[0].ask.as_str(), v.asking[0].from.as_str()), ("0007", "000000000000feed", "100.64.0.7"));
        assert!(d.knows(S1).is_none());
        // Asking again with its secret: the same ask, nothing new.
        assert_eq!(d.ask(&asked("abc123", S1, "100.64.0.7"), 1001, 99).unwrap(), Joined::Stands { state: State::Asking, code: c.clone(), new: false });
        // Another secret can't learn how the ask stands.
        assert_eq!(d.state("abc123", S2, 1001).0, State::None);
        assert_eq!(d.state("abc123", S1, 1001), (State::Asking, c.clone()));
        // An answer for another code, or naming another ask, answers nothing.
        assert!(d.answer(None, "0008", true, 1010).is_none());
        assert!(d.answer(Some("000000000000beef"), &c, true, 1010).is_none());
        assert_eq!(d.answer(Some("000000000000feed"), &c, true, 1010).map(|x| x.label), Some("Safari on iPad".to_string()));
        assert_eq!(d.state("abc123", S1, 1011).0, State::Accepted);
        assert_eq!(d.knows(S1).map(|x| x.id.as_str()), Some("abc123"));
        assert!(d.knows(S2).is_none());
        assert!(d.view(1011).asking.is_empty() && d.view(1011).accepted.len() == 1);
        // (Nothing to answer twice.)
        assert!(d.answer(None, &c, false, 1012).is_none());
        // Forgotten, by its page's id: its secret no longer works.
        assert_eq!(d.forget("abc123").len(), 1);
        assert!(d.knows(S1).is_none());
        assert_eq!(d.state("abc123", S1, 1030).0, State::None);
    }

    #[test]
    fn an_ask_never_takes_anothers_place_and_codes_are_made_here_unlike_any_waiting() {
        let mut d = Devices::default();
        // The same code drawn for two asks: the second gets another.
        let c1 = code(d.ask(&asked("ipad01", S1, "100.64.0.7"), 0, (5 << 64) | 1).unwrap());
        let c2 = code(d.ask(&asked("ipad01", S2, "192.168.1.66"), 1, (5 << 64) | 2).unwrap());
        assert_eq!((c1.as_str(), c2.as_str()), ("0005", "7924"));
        // Both wait, the first as it was: another secret with its page's id is another ask.
        assert_eq!(d.view(2).asking.len(), 2);
        assert_eq!(d.state("ipad01", S1, 2), (State::Asking, c1.clone()));
        // Accepting one leaves the other, and an accepted device stays when another of its page's
        // id is accepted.
        d.answer(None, &c1, true, 3).unwrap();
        d.answer(None, &c2, true, 4).unwrap();
        assert!(d.knows(S1).is_some() && d.knows(S2).is_some());
        // An ask with an accepted device's id and another secret is another ask, the device kept.
        let c3 = code(d.ask(&asked("ipad01", S3, "192.168.1.66"), 5, (9 << 64) | 3).unwrap());
        d.answer(None, &c3, false, 6).unwrap();
        assert!(d.knows(S1).is_some() && d.knows(S2).is_some() && d.knows(S3).is_none());
        assert_eq!(d.state("ipad01", S3, 7).0, State::Declined);
        assert_eq!(d.state("ipad01", S3, 6 + DECLINED_S).0, State::None);
        // Forgotten by an ask's name: that device alone.
        let name = d.view(7).accepted[0].ask.clone();
        assert_eq!(d.forget(&name).len(), 1);
        assert!(d.knows(S1).is_none() && d.knows(S2).is_some());
    }

    #[test]
    fn a_page_cancels_its_ask_and_asks_are_few() {
        let mut d = Devices::default();
        let c = code(d.ask(&asked("p1", S1, "192.168.1.9"), 0, 1).unwrap());
        assert!(!d.cancel("p1", S2) && d.cancel("p1", S1));
        assert!(d.answer(None, &c, true, 1).is_none());
        // From one address: FROM_ONE waiting at most.
        for i in 0..FROM_ONE {
            code(d.ask(&asked(&format!("q{i}"), S1, "192.168.1.9"), 10, i as u128).unwrap());
        }
        assert!(matches!(d.ask(&asked("q9", S1, "192.168.1.9"), 10, 9).unwrap(), Joined::TooMany(_)));
        // All told: ASKS_MAX.
        for i in FROM_ONE..ASKS_MAX {
            code(d.ask(&asked(&format!("q{i}"), S1, &format!("10.0.0.{i}")), 10, i as u128).unwrap());
        }
        assert!(matches!(d.ask(&asked("r1", S1, "10.0.0.99"), 10, 11).unwrap(), Joined::TooMany(_)));
        // None dropped for them; an unanswered ask lapses after ASK_S.
        assert_eq!(d.view(10).asking.len(), ASKS_MAX);
        assert!(d.view(10 + ASK_S).asking.is_empty());
        // An address that asked HOURLY times in the hour waits for it to pass.
        let mut e = Devices::default();
        for i in 0..HOURLY {
            code(e.ask(&asked(&format!("s{i}"), S1, "10.1.1.1"), 100, i as u128).unwrap());
            e.cancel(&format!("s{i}"), S1);
        }
        assert!(matches!(e.ask(&asked("s99", S1, "10.1.1.1"), 200, 1).unwrap(), Joined::TooMany(_)));
        assert!(matches!(e.ask(&asked("s99", S1, "10.1.1.1"), 100 + 3600, 1).unwrap(), Joined::Stands { new: true, .. }));
    }

    #[test]
    fn bad_asks_are_refused_and_a_device_works_as_itself() {
        let mut d = Devices::default();
        let g = "g".repeat(32);
        for (id, label, secret) in [("", "x", S1), ("a b", "x", S1), ("ok", "x", "short"), ("ok", "x", g.as_str()), ("ok", "a\nb", S1), ("ok", "Safari\u{202e}dapI", S1), ("ok", "Safari on iPad\u{200b}", S1)] {
            assert!(d.ask(&Asked { id, label, secret, from: "" }, 5, 2).is_err(), "{label:?}");
        }
        assert!(d.list.is_empty());
        // A worker's name is a device's when it ends with its page's id, after a space.
        assert!(names("Safari on iPad abc123", "abc123") && !names("Safari on iPadabc123", "abc123") && !names("Safari on iPad abc1234", "abc123") && !names("m4", "m4"));
    }

    #[test]
    fn kept_and_read_back_with_hashes_only_and_a_file_that_doesnt_read_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("devices.json");
        let mut d = Devices::default();
        let c = code(d.ask(&asked("abc", S1, ""), 1, 1).unwrap());
        d.answer(None, &c, true, 2);
        d.save(&p, true).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains(S1) && text.contains(&hash(S1)));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&p).unwrap().permissions()) & 0o777, 0o600);
        assert!(Devices::load(&p).knows(S1).is_some());
        assert!(Devices::load(&dir.path().join("none.json")).list.is_empty());
        std::fs::write(&p, b"{ not json").unwrap();
        assert!(Devices::load(&p).list.is_empty());
        assert!(dir.path().join("devices.json.bad").exists() && !p.exists());
    }
}
