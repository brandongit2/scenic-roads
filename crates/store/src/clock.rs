//! Time as code that waits sees it: the real clock, or a test's virtual one. Code whose decisions
//! turn on how long something took or waited (how long to wait on a worker, how long a worker took)
//! reads the time and sleeps through a [`Clock`], so its tests give it a [`Virtual`] one: time then
//! moves only when the code sleeps, the same on an idle Mac or a loaded one, and what a test's
//! other party does (a worker taking a task, handing it back) happens at the virtual moments the
//! test names, on the thread that sleeps, in order.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub trait Clock: Send + Sync + std::fmt::Debug {
    /// The monotonic time.
    fn now(&self) -> Instant;
    /// The wall clock.
    fn wall(&self) -> SystemTime;
    fn sleep(&self, d: Duration);
}

/// The real clock.
#[derive(Debug, Default)]
pub struct Real;

impl Clock for Real {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The real clock, shared.
pub fn real() -> Arc<dyn Clock> {
    Arc::new(Real)
}

type Event = Box<dyn FnOnce() + Send>;

/// A test's clock: it starts at the moment it's made (both its monotonic time and its wall clock)
/// and moves only when it's slept on (or `advance`d), at once. Events set for a moment (`at`,
/// `after`) run when a sleep passes it, in their order, on the sleeping thread, the clock reading
/// their moment.
pub struct Virtual {
    start: Instant,
    start_wall: SystemTime,
    state: Mutex<VState>,
}

#[derive(Default)]
struct VState {
    at: Duration,
    events: BTreeMap<(Duration, u64), Event>,
    next: u64,
    /// How many sleeps it was asked for, and their total.
    sleeps: u64,
    slept: Duration,
}

impl std::fmt::Debug for Virtual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        write!(f, "Virtual({:?}, {} events)", s.at, s.events.len())
    }
}

impl Virtual {
    pub fn new() -> Arc<Virtual> {
        Arc::new(Virtual { start: Instant::now(), start_wall: SystemTime::now(), state: Mutex::new(VState::default()) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The time since it was made.
    pub fn elapsed(&self) -> Duration {
        self.lock().at
    }

    /// How many sleeps it was asked for, and their total.
    pub fn slept(&self) -> (u64, Duration) {
        let s = self.lock();
        (s.sleeps, s.slept)
    }

    /// Runs `f` once the clock reaches `t` after it was made (at the next sleep, if it's past).
    pub fn at(&self, t: Duration, f: impl FnOnce() + Send + 'static) {
        let mut s = self.lock();
        let n = s.next;
        s.next += 1;
        s.events.insert((t, n), Box::new(f));
    }

    /// Runs `f` once the clock is `d` later than now.
    pub fn after(&self, d: Duration, f: impl FnOnce() + Send + 'static) {
        let t = self.elapsed() + d;
        self.at(t, f);
    }

    /// Moves the clock `d` on, running the events met on the way (not counted as a sleep).
    pub fn advance(&self, d: Duration) {
        let to = self.lock().at + d;
        loop {
            let ev = {
                let mut s = self.lock();
                match s.events.first_key_value() {
                    Some((k, _)) if k.0 <= to => {
                        let (k, ev) = s.events.pop_first().unwrap();
                        s.at = s.at.max(k.0);
                        ev
                    }
                    _ => {
                        s.at = s.at.max(to);
                        return;
                    }
                }
            };
            ev();
        }
    }
}

impl Clock for Virtual {
    fn now(&self) -> Instant {
        self.start + self.lock().at
    }

    fn wall(&self) -> SystemTime {
        self.start_wall + self.lock().at
    }

    fn sleep(&self, d: Duration) {
        {
            let mut s = self.lock();
            s.sleeps += 1;
            s.slept += d;
        }
        self.advance(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_virtual_clock_moves_only_when_slept_on_and_runs_its_events_in_order() {
        let c = Virtual::new();
        let (t0, w0) = (c.now(), c.wall());
        let seen = Arc::new(Mutex::new(Vec::new()));
        for (t, name) in [(3, "c"), (1, "a"), (1, "b"), (10, "d")] {
            let (s, c2) = (seen.clone(), c.clone());
            c.at(Duration::from_secs(t), move || s.lock().unwrap().push((name, c2.elapsed())));
        }
        assert_eq!(c.now(), t0);
        c.sleep(Duration::from_secs(5));
        assert_eq!((c.now() - t0, c.wall().duration_since(w0).unwrap()), (Duration::from_secs(5), Duration::from_secs(5)));
        assert_eq!(*seen.lock().unwrap(), [("a", Duration::from_secs(1)), ("b", Duration::from_secs(1)), ("c", Duration::from_secs(3))]);
        // An event may set another, which runs in its turn.
        let c2 = c.clone();
        let s = seen.clone();
        c.after(Duration::from_secs(1), move || {
            let s2 = s.clone();
            c2.after(Duration::from_secs(1), move || s2.lock().unwrap().push(("f", Duration::ZERO)));
            s.lock().unwrap().push(("e", Duration::ZERO));
        });
        c.advance(Duration::from_secs(10));
        assert_eq!(seen.lock().unwrap().iter().map(|x| x.0).collect::<String>(), "abcefd");
        assert_eq!((c.elapsed(), c.slept()), (Duration::from_secs(15), (1, Duration::from_secs(5))));
    }
}
