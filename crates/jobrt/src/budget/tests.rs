//! The budget's tests: every decision by a clock that moves only when waited on (`StepClock`), so
//! no result depends on the machine's speed, its load or the time of day.

use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

/// Each window's tokens, by class, over `ledgers`.
fn per_window(ledgers: &[Vec<Token>]) -> BTreeMap<(Class, u64), u32> {
    let mut m = BTreeMap::new();
    for t in ledgers.iter().flatten() {
        *m.entry((t.class, t.window)).or_default() += 1;
    }
    m
}

pub(super) const T0: u64 = 1_800_000_000_000;

/// A budget taking from `lead`, keeping what it gives.
pub(super) fn budget(lead: &Arc<FakeLead>, me: &str, clock: &Arc<StepClock>) -> Arc<Budget> {
    let b = Arc::new(Budget::new(me, clock.clone()).with_ledger());
    b.set_lead(Some(lead.clone()));
    b
}

/// A lead as a test sets it: answering batches of `batch` tokens (the lead's own gives one) of a
/// window's `Rate::tokens`, counted by the clock, or down (`up` false).
pub(super) struct FakeLead {
    clock: Arc<StepClock>,
    pub up: std::sync::atomic::AtomicBool,
    pub batch: u32,
    pub given: Mutex<BTreeMap<(Class, u64), u32>>,
    held: Mutex<u64>,
}

pub(super) fn fake_lead(clock: &Arc<StepClock>) -> Arc<FakeLead> {
    Arc::new(FakeLead {
        clock: clock.clone(),
        up: true.into(),
        batch: 1,
        given: Mutex::new(BTreeMap::new()),
        held: Mutex::new(0),
    })
}

impl Lead for FakeLead {
    fn take(&self, class: Class) -> Result<Given, String> {
        if !self.up.load(Ordering::SeqCst) {
            return Err("connection refused".into());
        }
        let (r, now) = (class.rate(), self.clock.now_ms());
        let held = *self.held.lock().unwrap();
        if now < held {
            return Ok(Given::Wait { ms: held - now });
        }
        let w = now / r.window_ms;
        let mut g = self.given.lock().unwrap();
        let n = g.entry((class, w)).or_default();
        if *n >= r.tokens {
            return Ok(Given::Wait { ms: (w + 1) * r.window_ms - now });
        }
        let batch = self.batch.min(r.tokens - *n);
        *n += batch;
        Ok(Given::Batch { n: batch, window: w, for_ms: (w + 1) * r.window_ms - now })
    }
    fn hold(&self, secs: u64) -> Result<(), String> {
        if !self.up.load(Ordering::SeqCst) {
            return Err("connection refused".into());
        }
        let mut h = self.held.lock().unwrap();
        *h = (*h).max(self.clock.now_ms() + secs * 1000);
        Ok(())
    }
    fn answers(&self, _: LeaseOp, _: &str, _: &str) -> Result<LeaseSaid, String> {
        Err("not this test's".into())
    }
}

#[test]
fn hosts_are_metered_by_class() {
    assert_eq!(class_of("query.wikidata.org"), Some(Class::Query));
    assert_eq!(class_of("qlever.dev"), Some(Class::Query));
    assert_eq!(class_of("QLEVER.DEV."), Some(Class::Query));
    assert_eq!(class_of("en.wikipedia.org"), Some(Class::Wiki));
    assert_eq!(class_of("www.wikidata.org"), Some(Class::Wiki));
    assert_eq!(class_of("dumps.wikimedia.org"), Some(Class::Wiki));
    assert_eq!(class_of("wikipedia.org"), Some(Class::Wiki));
    assert_eq!(class_of("notwikipedia.org"), None);
    assert_eq!(class_of("wikipedia.org.example.com"), None);
    assert_eq!(class_of("s3.amazonaws.com"), None);
    assert_eq!(class_of("127.0.0.1"), None);
}

/// Two processes (two budgets, as two agents' proxies would be) taking from one lead, many
/// threads each: no window gives more than its tokens, and every request is served.
#[test]
fn two_processes_sharing_one_budget_never_pass_its_rate() {
    let clock = Arc::new(StepClock::at(T0));
    let lead = fake_lead(&clock);
    let (a, b) = (budget(&lead, "m-a", &clock), budget(&lead, "m-b", &clock));
    let mut ts = Vec::new();
    for (i, x) in [a.clone(), a.clone(), b.clone(), b.clone()].into_iter().enumerate() {
        ts.push(std::thread::spawn(move || {
            for _ in 0..150 {
                x.take(if i % 2 == 0 { Class::Wiki } else { Class::Query });
            }
        }));
    }
    ts.into_iter().for_each(|t| t.join().unwrap());
    let w = per_window(&[a.ledger(), b.ledger()]);
    let total: u32 = w.values().sum();
    assert_eq!(total, 600);
    for ((class, win), n) in &w {
        assert!(*n <= class.rate().tokens, "{class:?} window {win}: {n} tokens, more than {}", class.rate().tokens);
    }
    // (300 of each: at least so many windows passed.)
    let wins = |c: Class| w.keys().filter(|(k, _)| *k == c).count() as u32;
    assert!(wins(Class::Wiki) >= 300 / Class::Wiki.rate().tokens);
    assert!(wins(Class::Query) >= 300 / Class::Query.rate().tokens);
}

/// A query's time past its token's is owed, and paid from the window's tokens before the next.
#[test]
fn a_querys_time_is_paid_for() {
    let clock = Arc::new(StepClock::at(T0));
    let lead = fake_lead(&clock);
    let a = budget(&lead, "m-a", &clock);
    let t = a.take(Class::Query);
    // (Seven seconds: four tokens' time, three owed.)
    a.charge(Class::Query, 7_000);
    let u = a.take(Class::Query);
    assert_eq!(u.window, t.window, "five tokens a window: one and three owed and one fit");
    assert_eq!(lead.given.lock().unwrap()[&(Class::Query, t.window)], 5);
    // (The window's spent: the next token is the next window's.)
    assert_eq!(a.take(Class::Query).window, t.window + 1);
}

/// A 429 told to one process holds every other through the lead, and itself, till its wait ends.
#[test]
fn a_429_holds_every_process() {
    let clock = Arc::new(StepClock::at(T0));
    let lead = fake_lead(&clock);
    let (a, b) = (budget(&lead, "m-a", &clock), budget(&lead, "m-b", &clock));
    b.take(Class::Wiki);
    assert!(a.hold(45));
    let until = (T0 + 45_000) / 10_000;
    assert!(b.take(Class::Wiki).window >= until, "none of B's after the hold");
    // (A holds itself until then too.)
    assert!(a.take(Class::Query).window * 10_000 >= T0 + 45_000 - 10_000);
    assert!(clock.now_ms() >= a.held_until());
    // (A wait past the most is the most.)
    let c = budget(&lead, "m-c", &clock);
    c.hold(1_000_000);
    assert_eq!(c.held_until(), clock.now_ms() + HOLD_MAX_S * 1000);
}

/// The state's lock isn't held while a thread asks the lead: what the status reads, and a hold
/// told here, answer while the asking lock is held.
#[test]
fn the_status_never_waits_on_the_lead() {
    let clock = Arc::new(StepClock::at(T0));
    let a = budget(&fake_lead(&clock), "m-a", &clock);
    let _leasing = a.leasing.lock().unwrap();
    assert!(a.away().is_none() && a.held_until() == 0);
    assert_eq!(a.hold_here(10).1, T0 + 10_000);
    a.charge(Class::Query, 5_000);
}

/// docs/coordination.md step 7: the lead down, a Mac takes nothing (a share of its own would add to
/// what the lead gives the Macs that reach it), asking again once a window; the lead back, its
/// batches again. A 429 told meanwhile holds this Mac alone.
#[test]
fn the_budget_waits_for_the_lead_while_it_is_down() {
    let clock = Arc::new(StepClock::at(T0));
    let lead = fake_lead(&clock);
    let a = budget(&lead, "m-a", &clock);
    a.take(Class::Wiki);
    // (Down: nothing till the lead is back, however long.)
    lead.up.store(false, Ordering::SeqCst);
    assert!(!a.hold(20), "a hold not told: this Mac alone");
    assert_eq!(a.held_until(), clock.now_ms() + 20_000);
    let t = {
        let a = a.clone();
        std::thread::spawn(move || a.take(Class::Wiki))
    };
    while clock.now_ms() < T0 + 600_000 {
        std::thread::yield_now();
    }
    assert!(a.away().is_some_and(|e| e.contains("the lead")), "{:?}", a.away());
    lead.up.store(true, Ordering::SeqCst);
    let t = t.join().unwrap();
    assert!(t.window * 10_000 >= T0 + 600_000 - 10_000, "{t:?}");
    assert_eq!(a.ledger().len(), 2, "none of its own");
    assert!(a.away().is_none());
    assert!(a.hold(30));
    let t = a.take(Class::Wiki);
    assert!(t.window * 10_000 + 10_000 > clock.now_ms() && clock.now_ms() >= a.held_until());
}

/// No lead given yet (the agent starting): nothing taken, asked again each window, until one is.
#[test]
fn nothing_is_taken_before_a_lead_is_given() {
    let clock = Arc::new(StepClock::at(T0));
    let a = Arc::new(Budget::new("m-a", clock.clone()).with_ledger());
    let t = {
        let a = a.clone();
        std::thread::spawn(move || a.take(Class::Wiki))
    };
    while clock.now_ms() < T0 + 60_000 {
        std::thread::yield_now();
    }
    assert!(a.away().is_some_and(|e| e.contains("no lead")), "{:?}", a.away());
    assert!(!a.hold(5), "a hold with no lead: this Mac alone");
    assert!(a.answers(LeaseOp::Take, "x", "t").is_err());
    let lead = fake_lead(&clock);
    a.set_lead(Some(lead.clone()));
    t.join().unwrap();
    assert_eq!(lead.given.lock().unwrap().values().sum::<u32>(), 1);
}

/// A batch the lead gives is spent only within what was left of its window when asked: a batch
/// asked late in a window isn't spent in the next; a change of lead drops the batch held.
#[test]
fn a_leads_batch_ends_with_its_window() {
    let clock = Arc::new(StepClock::at(T0 + 9_999));
    let mut lead = fake_lead(&clock);
    Arc::get_mut(&mut lead).unwrap().batch = 4;
    let a = budget(&lead, "m-a", &clock);
    let first = a.take(Class::Wiki);
    clock.set(T0 + 10_000);
    let next = a.take(Class::Wiki);
    assert_eq!(next.window, first.window + 1, "the batch's three left lapsed with its window");
    assert_eq!(lead.given.lock().unwrap().values().sum::<u32>(), 8);
    a.set_lead(Some(lead.clone()));
    a.take(Class::Wiki);
    assert_eq!(lead.given.lock().unwrap().values().sum::<u32>(), 12, "the batch held dropped");
}
