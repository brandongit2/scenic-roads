//! The drop boxes' listing (docs/inputs.md §4.2): the lead lists every unit's drop box and its
//! acceptances every two minutes, off its loop (a thread of its own: a listing can take half a
//! minute under load), and keeps the last listing. A file changed in the last `QUIET_S` is left for
//! the next listing. The listing is only a trigger: what changed is decided by content, in the
//! check (crate::inputs::gate). Asked (`ask`), it lists at once: `scenic inputs check`, the Regions
//! panel's writes, an acceptance.

use super::Listing;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// How often the drop boxes are listed.
pub const EVERY: Duration = Duration::from_secs(120);

/// A unit as last listed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    /// What of its drop box held still.
    pub listing: Listing,
    /// Its acceptances' ids.
    pub accepted: BTreeSet<String>,
    /// When it was listed (seconds since the epoch).
    pub at: u64,
}

#[derive(Default)]
struct Shared {
    /// By unit: the last listing as made (for the quiet rule) and what held still.
    units: BTreeMap<String, (Listing, Listed)>,
    /// A listing asked for now.
    asked: bool,
    /// Why the last listing of a unit failed, by unit (kept until one succeeds).
    failed: BTreeMap<String, String>,
    stop: bool,
}

/// The listing thread.
pub struct Watch {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    /// Starts listing the drop boxes of the units on the gate under `root` (crate::inputs::units,
    /// read again each time) every `every`.
    pub fn start(root: PathBuf, every: Duration) -> Watch {
        let shared: Arc<(Mutex<Shared>, Condvar)> = Default::default();
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("inputs-watch".into())
            .spawn(move || loop {
                let units = super::units(&root);
                // (A file left for not holding still: listed again once it may have.)
                let mut unsettled = false;
                for unit in units {
                    let Some(checks) = super::checks(unit) else { continue };
                    let raw = super::list(&root, unit, checks.recursive());
                    let acc = super::acceptances(&root, unit);
                    let at = crate::agent::jobs::now_s();
                    let (lock, _) = &*s;
                    let mut g = lock.lock().unwrap();
                    match (raw, acc) {
                        (Ok(raw), Ok(accepted)) => {
                            let listing = super::settle(&raw, at);
                            unsettled |= listing.files.len() < raw.files.len();
                            g.units.insert(unit.to_string(), (raw, Listed { listing, accepted, at }));
                            g.failed.remove(unit);
                        }
                        (Err(e), _) | (_, Err(e)) => {
                            g.failed.insert(unit.to_string(), format!("{e:#}"));
                        }
                    }
                }
                let (lock, cv) = &*s;
                let mut g = lock.lock().unwrap();
                if g.stop {
                    return;
                }
                if !g.asked {
                    let wait = if unsettled { every.min(Duration::from_secs(super::QUIET_S + 1)) } else { every };
                    g = cv.wait_timeout_while(g, wait, |g| !g.asked && !g.stop).unwrap().0;
                }
                if g.stop {
                    return;
                }
                g.asked = false;
            })
            .expect("spawn the inputs' listing thread");
        Watch { shared, thread: Some(thread) }
    }

    /// Lists again at once.
    pub fn ask(&self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap().asked = true;
        cv.notify_all();
    }

    /// Unit `unit` as last listed: None before its first listing.
    pub fn get(&self, unit: &str) -> Option<Listed> {
        self.shared.0.lock().unwrap().units.get(unit).map(|(_, l)| l.clone())
    }

    /// Every unit's acceptances as last listed.
    pub fn accepted(&self) -> BTreeMap<String, BTreeSet<String>> {
        self.shared.0.lock().unwrap().units.iter().map(|(u, (_, l))| (u.clone(), l.accepted.clone())).collect()
    }

    /// Why unit `unit`'s last listing failed, while it does.
    pub fn failed(&self, unit: &str) -> Option<String> {
        self.shared.0.lock().unwrap().failed.get(unit).cloned()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap().stop = true;
        cv.notify_all();
        if let Some(t) = self.thread.take() {
            // (A listing hanging on the share: left to end by itself.)
            if t.is_finished() {
                t.join().ok();
            }
        }
    }
}
