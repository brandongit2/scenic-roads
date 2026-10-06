//! The devices that help through the build page (docs/workers.md §7): a page asks the build Mac to
//! let it help (`/work/join`), with a secret it made and alone keeps; the owner accepts or declines
//! the ask there (the menu bar, `scenic devices`), matching the code both show; an accepted
//! device's secret is its key from then on, until the owner forgets the device. Kept in the
//! coordinator's `devices.json`: the secrets' hashes, never the secrets.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::Path;

/// How long a declined ask is remembered (its page learns so), and an unanswered one kept.
const DECLINED_S: u64 = 600;
const ASK_S: u64 = 86400;
/// The most asks waiting at once (a page asking again replaces its own; past this, the oldest goes).
const ASKS_MAX: usize = 8;

/// A device: asking, accepted or declined.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// The page's own id (its browser's), and what it is ("Safari on iPad").
    pub id: String,
    pub label: String,
    /// SHA-256 of its secret, hex.
    pub hash: String,
    /// Where it asked from (its address, as the coordinator saw it), and when (unix seconds).
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
    /// The code its page shows and the build Mac with its ask: the same four digits on both, so
    /// the right one is accepted.
    pub fn code(&self) -> String {
        format!("{:04}", u32::from_str_radix(&self.hash[..8.min(self.hash.len())], 16).unwrap_or(0) % 10_000)
    }
}

/// An ask, or an accepted device, as the build Mac shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    pub id: String,
    pub label: String,
    pub code: String,
    pub from: String,
    pub at: u64,
}

/// What the build Mac shows: the asks waiting, and the devices accepted.
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

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Devices {
    #[serde(default)]
    pub list: Vec<Device>,
}

/// A secret's hash, as kept.
pub fn hash(secret: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(secret.as_bytes()))
}

/// Whether a page's own id, its label and its secret are what one sends.
fn valid(id: &str, label: &str, secret: &str) -> bool {
    (1..=40).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && (1..=80).contains(&label.chars().count()) && !label.chars().any(char::is_control) && (32..=128).contains(&secret.len()) && secret.chars().all(|c| c.is_ascii_hexdigit())
}

impl Devices {
    /// The devices kept in `path` (none when there's no file or it doesn't read).
    pub fn load(path: &Path) -> Devices {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        crate::whole::write(path, &serde_json::to_vec_pretty(self)?)?;
        if let Ok(f) = std::fs::File::open(path) {
            store::sys::set_mode(&f, 0o600).ok();
        }
        Ok(())
    }

    /// Asks gone stale: a declined one past DECLINED_S, an unanswered one past ASK_S, and the
    /// oldest past ASKS_MAX waiting.
    fn tidy(&mut self, now: u64) {
        self.list.retain(|d| d.accepted > 0 || (d.declined > 0 && now.saturating_sub(d.declined) < DECLINED_S) || (d.declined == 0 && now.saturating_sub(d.asked) < ASK_S));
        let mut asking: Vec<(u64, String)> = self.list.iter().filter(|d| d.accepted == 0 && d.declined == 0).map(|d| (d.asked, d.id.clone())).collect();
        if asking.len() > ASKS_MAX {
            asking.sort();
            let drop: Vec<String> = asking[..asking.len() - ASKS_MAX].iter().map(|a| a.1.clone()).collect();
            self.list.retain(|d| d.accepted > 0 || d.declined > 0 || !drop.contains(&d.id));
        }
    }

    /// A page's ask to help, with the secret it made: kept, replacing that page's earlier ask or
    /// answer (`owner`: it carried the build's own key, a page from before devices asked: accepted
    /// at once). Where it stands, and its code; an error when what it sent isn't an ask.
    pub fn ask(&mut self, id: &str, label: &str, secret: &str, from: &str, owner: bool, now: u64) -> Result<(State, String)> {
        anyhow::ensure!(valid(id, label, secret), "an ask is a page's id, what it is, and its secret (32 to 128 hex digits)");
        let h = hash(secret);
        if let Some(d) = self.list.iter().find(|d| d.id == id && d.hash == h && d.accepted > 0) {
            return Ok((State::Accepted, d.code()));
        }
        self.list.retain(|d| !(d.id == id && d.accepted == 0));
        let d = Device { id: id.into(), label: label.into(), hash: h, from: from.into(), asked: now, accepted: if owner { now } else { 0 }, declined: 0 };
        let code = d.code();
        self.list.push(d);
        self.tidy(now);
        Ok((if owner { State::Accepted } else { State::Asking }, code))
    }

    /// Where a page's ask stands (its secret proves it's the page's).
    pub fn state(&self, id: &str, secret: &str, now: u64) -> State {
        let h = hash(secret);
        match self.list.iter().find(|d| d.id == id && d.hash == h) {
            Some(d) if d.accepted > 0 => State::Accepted,
            Some(d) if d.declined > 0 => if now.saturating_sub(d.declined) < DECLINED_S { State::Declined } else { State::None },
            Some(d) if now.saturating_sub(d.asked) < ASK_S => State::Asking,
            _ => State::None,
        }
    }

    /// The owner's answer to the ask of page `id`: what it is, when there was one.
    pub fn answer(&mut self, id: &str, accept: bool, now: u64) -> Option<String> {
        self.tidy(now);
        let d = self.list.iter_mut().find(|d| d.id == id && d.accepted == 0 && d.declined == 0)?;
        if accept {
            d.accepted = now;
        } else {
            d.declined = now;
        }
        let label = d.label.clone();
        // (Accepted: any other device of that page's id, an older secret, gone.)
        if accept {
            let h = self.list.iter().find(|d| d.id == id && d.accepted == now).map(|d| d.hash.clone());
            self.list.retain(|d| d.id != id || Some(&d.hash) == h.as_ref() || d.accepted == 0);
        }
        Some(label)
    }

    /// Forgets the accepted device of page `id`: its secret no longer works. What it was.
    pub fn forget(&mut self, id: &str) -> Option<String> {
        let label = self.list.iter().find(|d| d.id == id && d.accepted > 0).map(|d| d.label.clone())?;
        self.list.retain(|d| !(d.id == id && d.accepted > 0));
        Some(label)
    }

    /// Whether `secret` is an accepted device's.
    pub fn knows(&self, secret: &str) -> bool {
        let h = hash(secret);
        self.list.iter().any(|d| d.accepted > 0 && d.hash == h)
    }

    /// The asks waiting and the devices accepted, as the build Mac shows them.
    pub fn view(&self, now: u64) -> View {
        let shown = |d: &Device, at: u64| Shown { id: d.id.clone(), label: d.label.clone(), code: d.code(), from: d.from.clone(), at };
        View {
            asking: self.list.iter().filter(|d| d.accepted == 0 && d.declined == 0 && now.saturating_sub(d.asked) < ASK_S).map(|d| shown(d, d.asked)).collect(),
            accepted: self.list.iter().filter(|d| d.accepted > 0).map(|d| shown(d, d.accepted)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S1: &str = "0123456789abcdef0123456789abcdef";
    const S2: &str = "fedcba9876543210fedcba9876543210";

    #[test]
    fn a_page_asks_the_owner_answers_and_its_secret_is_its_key_until_forgotten() {
        let mut d = Devices::default();
        let (st, code) = d.ask("abc123", "Safari on iPad", S1, "100.64.0.7", false, 1000).unwrap();
        assert_eq!(st, State::Asking);
        assert_eq!(code.len(), 4);
        assert_eq!(d.view(1000).asking[0].code, code);
        assert!(!d.knows(S1));
        // Another secret can't learn how the ask stands, nor be accepted for it.
        assert_eq!(d.state("abc123", S2, 1001), State::None);
        assert_eq!(d.answer("abc123", true, 1010).as_deref(), Some("Safari on iPad"));
        assert_eq!(d.state("abc123", S1, 1011), State::Accepted);
        assert!(d.knows(S1) && !d.knows(S2));
        assert!(d.view(1011).asking.is_empty() && d.view(1011).accepted.len() == 1);
        // Asking again with its secret: accepted still. Forgotten: its secret no longer works.
        assert_eq!(d.ask("abc123", "Safari on iPad", S1, "", false, 1020).unwrap().0, State::Accepted);
        assert_eq!(d.forget("abc123").as_deref(), Some("Safari on iPad"));
        assert!(!d.knows(S1));
        assert_eq!(d.state("abc123", S1, 1030), State::None);
    }

    #[test]
    fn a_declined_ask_is_told_a_while_then_forgotten_and_asks_are_few() {
        let mut d = Devices::default();
        d.ask("p1", "Chrome on Mac", S1, "", false, 0).unwrap();
        assert!(d.answer("p1", false, 10).is_some());
        assert_eq!(d.state("p1", S1, 20), State::Declined);
        assert!(!d.knows(S1));
        assert_eq!(d.state("p1", S1, 20 + DECLINED_S), State::None);
        // (Nothing to answer twice.)
        assert!(d.answer("p1", true, 30).is_none());
        // Many pages asking: the newest ASKS_MAX kept.
        for i in 0..12u64 {
            d.ask(&format!("q{i}"), "Safari on iPhone", S2, "", false, 100 + i).unwrap();
        }
        let asking = d.view(200).asking;
        assert_eq!(asking.len(), ASKS_MAX);
        assert_eq!(asking.first().map(|a| a.id.as_str()), Some("q4"));
        // An unanswered ask lapses after ASK_S.
        assert!(d.view(100 + ASK_S + 20).asking.is_empty());
    }

    #[test]
    fn a_page_with_the_old_key_is_accepted_at_once_and_bad_asks_are_refused() {
        let mut d = Devices::default();
        assert_eq!(d.ask("old1", "Safari on iPad", S1, "", true, 5).unwrap().0, State::Accepted);
        assert!(d.knows(S1));
        assert!(d.ask("", "x", S1, "", false, 5).is_err());
        assert!(d.ask("a b", "x", S1, "", false, 5).is_err());
        assert!(d.ask("ok", "x", "short", "", false, 5).is_err());
        assert!(d.ask("ok", "x", &"g".repeat(32), "", false, 5).is_err());
        assert!(d.ask("ok", "a\nb", S1, "", false, 5).is_err());
    }

    #[test]
    fn kept_and_read_back_with_hashes_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("devices.json");
        let mut d = Devices::default();
        d.ask("abc", "Safari on iPad", S1, "", false, 1).unwrap();
        d.answer("abc", true, 2);
        d.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains(S1) && text.contains(&hash(S1)));
        assert!(Devices::load(&p).knows(S1));
        assert!(Devices::load(&dir.path().join("none.json")).list.is_empty());
    }
}
