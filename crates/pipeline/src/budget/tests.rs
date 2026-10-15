//! The budget's tests: every decision by a clock that moves only when waited on (`StepClock`), so
//! no result depends on the machine's speed, its load or the time of day.

use super::*;
use std::collections::BTreeMap;

/// Each window's tokens, by class, over `ledgers`.
fn per_window(ledgers: &[Vec<Token>]) -> BTreeMap<(Class, u64), u32> {
    let mut m = BTreeMap::new();
    for t in ledgers.iter().flatten() {
        *m.entry((t.class, t.window)).or_default() += 1;
    }
    m
}

const T0: u64 = 1_800_000_000_000;

fn budget(root: &Path, me: &str, clock: &Arc<StepClock>) -> Arc<Budget> {
    Arc::new(Budget::new(root, me, clock.clone()).with_ledger())
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

/// Two processes (two budgets, as two agents' proxies would be) sharing one budget on one NAS
/// folder, many threads each: no window gives more than its tokens, and every request is served.
#[test]
fn two_processes_sharing_one_budget_never_pass_its_rate() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(StepClock::at(T0));
    let (a, b) = (budget(dir.path(), "m-a", &clock), budget(dir.path(), "m-b", &clock));
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
    assert!(a.ledger().iter().chain(b.ledger().iter()).all(|t| t.source == Source::Nas));
}

/// A query's time past its token's is owed, and paid from the window's tokens before the next.
#[test]
fn a_querys_time_is_paid_for() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(StepClock::at(T0));
    let a = budget(dir.path(), "m-a", &clock);
    let t = a.take(Class::Query);
    // (Seven seconds: four tokens' time, three owed.)
    a.charge(Class::Query, 7_000);
    let u = a.take(Class::Query);
    assert_eq!(u.window, t.window, "five tokens a window: one and three owed and one fit");
    let leased = (0..5).filter(|s| a.dir(Class::Query).join(format!("{}-{s}", t.window)).is_file()).count();
    assert_eq!(leased, 5);
    // (The window's spent: the next token is the next window's.)
    assert_eq!(a.take(Class::Query).window, t.window + 1);
}

/// A 429 told to one process holds every other on the NAS, and itself, till its wait ends.
#[test]
fn a_429_holds_every_process() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(StepClock::at(T0));
    let (a, b) = (budget(dir.path(), "m-a", &clock), budget(dir.path(), "m-b", &clock));
    let first = b.take(Class::Wiki);
    assert!(a.hold(45));
    let until = (T0 + 45_000) / 10_000;
    // (B's batch was taken before: it may still be spent, its tokens this window's; nothing after.)
    let mut seen = Vec::new();
    for _ in 0..8 {
        seen.push(b.take(Class::Wiki));
    }
    let early: Vec<_> = seen.iter().filter(|t| t.window < until).collect();
    assert!(early.iter().all(|t| t.window == first.window), "{early:?}");
    assert!(early.len() < Class::Wiki.rate().batch as usize);
    assert!(seen.iter().any(|t| t.window >= until));
    // (A holds itself until then too.)
    assert!(a.take(Class::Query).window * 10_000 >= T0 + 45_000 - 10_000);
    assert!(clock.now_ms() >= a.held_until());
    // (A wait past the most is the most.)
    let c = budget(dir.path(), "m-c", &clock);
    c.hold(1_000_000);
    assert_eq!(c.held_until(), clock.now_ms() + HOLD_MAX_S * 1000);
}

/// The NAS out of reach: each Mac takes the rate divided by the members it knows, in memory.
#[test]
fn the_nas_away_divides_the_rate_by_the_members() {
    let dir = tempfile::tempdir().unwrap();
    let away = dir.path().join("not-mounted");
    let clock = Arc::new(StepClock::at(T0));
    let a = budget(&away, "m-a", &clock);
    a.members.store(2, Ordering::Relaxed);
    for _ in 0..35 {
        a.take(Class::Wiki);
    }
    let w = per_window(&[a.ledger()]);
    assert!(w.values().all(|n| *n <= Class::Wiki.rate().tokens / 2), "{w:?}");
    assert!(a.ledger().iter().all(|t| t.source == Source::Alone));
    assert!(a.away().is_some());
    assert!(!away.exists(), "nothing made where the NAS should be");
    // (Back: leases on the NAS again.)
    std::fs::create_dir_all(&away).unwrap();
    clock.set(clock.now_ms() + 10_000);
    assert_eq!(a.take(Class::Wiki).source, Source::Nas);
    assert!(a.away().is_none());
    // (A hold with the NAS away holds this Mac, and says the NAS didn't take it.)
    let b = budget(&dir.path().join("gone"), "m-b", &clock);
    assert!(!b.hold(20));
    assert_eq!(b.held_until(), clock.now_ms() + 20_000);
}

/// The leases' files of windows long past are removed by their names as new windows are leased.
#[test]
fn old_leases_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(StepClock::at(T0));
    let a = budget(dir.path(), "m-a", &clock);
    let first = a.take(Class::Query).window;
    let f = a.dir(Class::Query).join(format!("{first}-0"));
    assert!(f.is_file());
    clock.set((first + KEEP) * 10_000);
    a.take(Class::Query);
    assert!(f.is_file(), "kept for KEEP windows");
    clock.set((first + KEEP + 1) * 10_000);
    a.take(Class::Query);
    assert!(!f.exists(), "removed once KEEP windows behind");
}
