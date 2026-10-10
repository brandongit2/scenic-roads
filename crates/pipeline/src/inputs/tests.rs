//! The gate's tests (docs/inputs.md §4.10): no wall clock in what's decided, no real NAS.

use super::gate::{decide, Decision, Given};
use super::*;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// A drop box in memory: path → (bytes, time).
#[derive(Clone, Default)]
struct Box_ {
    files: BTreeMap<String, (Vec<u8>, u64)>,
}

impl Box_ {
    fn put(&mut self, p: &str, b: &str, t: u64) {
        self.files.insert(p.into(), (b.as_bytes().to_vec(), t));
    }
    fn listing(&self) -> Listing {
        Listing { files: self.files.iter().map(|(p, (b, t))| (p.clone(), (b.len() as u64, *t))).collect(), strays: Vec::new() }
    }
}

/// The store of accepted copies, in memory, as the job keeps it, and the sizes and times the last
/// check of each version listed (the records' `listed`, kept by the version it's of here).
#[derive(Default)]
struct Store {
    copies: RefCell<BTreeMap<String, Vec<u8>>>,
    listed: RefCell<BTreeMap<String, BTreeMap<String, (u64, u64)>>>,
}

fn version_key(i: &Index) -> String {
    serde_json::to_string(i).unwrap()
}

/// One check of `b` against `prev` with `accepted` (and what it read), the taken copies kept.
fn check(checks: &dyn Checks, b: &Box_, prev: Option<&Index>, accepted: &[&str], store: &Store, full: bool) -> Decision {
    let read = RefCell::new(Vec::<String>::new());
    let read_new = |p: &str| {
        read.borrow_mut().push(p.to_string());
        Ok(b.files[p].0.clone())
    };
    let read_old = |e: &FileEntry| store.copies.borrow().get(&e.file).cloned().context("no copy");
    let acc: BTreeSet<String> = accepted.iter().map(|s| s.to_string()).collect();
    let listing = b.listing();
    let prev_listed = prev.and_then(|i| store.listed.borrow().get(&version_key(i)).cloned()).unwrap_or_default();
    let d = decide(&Given { checks, prev, prev_listed: &prev_listed, listing: &listing, unsettled: &BTreeSet::new(), accepted: &acc, full, read_new: &read_new, read_old: &read_old }).unwrap();
    for (n, bytes) in &d.store {
        store.copies.borrow_mut().insert(n.clone(), bytes.to_vec());
    }
    store.listed.borrow_mut().insert(version_key(&d.index), d.listed.clone());
    d
}

fn ids(d: &Decision) -> Vec<String> {
    d.report.as_ref().map(|r| r.findings.iter().map(|f| f.id.clone()).collect()).unwrap_or_default()
}

fn held(d: &Decision) -> Vec<String> {
    d.report.as_ref().map(|r| r.held.clone()).unwrap_or_default()
}

const GT: &gatetest::GateTest = &gatetest::GateTest(TEST_UNIT);

#[test]
fn a_good_drop_is_taken_in_and_the_index_is_the_same_twice() {
    let store = Store::default();
    let mut b = Box_::default();
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n{\"k\": \"y\", \"v\": 2}\n", 100);
    let d = check(GT, &b, None, &[], &store, false);
    assert!(d.changed && d.report.is_none(), "{:?}", d.report);
    let e = &d.index.files["a.jsonl"];
    assert!(e.file.starts_with("sources/inputs/_gate-test/a.") && e.file.ends_with(".jsonl"), "{}", e.file);
    assert_eq!(e.facts["lines"], 2);
    assert_eq!(d.listed["a.jsonl"], (b.files["a.jsonl"].0.len() as u64, 100));
    // The index names no time: the version's name is its contents' alone.
    assert!(!serde_json::to_string(&d.index).unwrap().contains("listed"));
    // The same candidate again, from nothing: the same index, byte for byte.
    let again = check(GT, &b, None, &[], &Store::default(), false);
    assert_eq!(serde_json::to_vec(&again.index).unwrap(), serde_json::to_vec(&d.index).unwrap());
    // Checked again against it: nothing read, nothing changes.
    let d2 = check(GT, &b, Some(&d.index), &[], &store, false);
    assert!(!d2.changed && d2.read.is_empty() && d2.report.is_none());
}

#[test]
fn an_error_holds_and_the_last_good_version_stays() {
    let store = Store::default();
    let mut b = Box_::default();
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n", 100);
    let v1 = check(GT, &b, None, &[], &store, false).index;
    // An edit that doesn't parse: held, the old bytes kept; a new file beside it, clean: taken.
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\nnot json\n", 200);
    b.put("b.jsonl", "{\"k\": \"z\", \"v\": 3}\n", 200);
    let d = check(GT, &b, Some(&v1), &[], &store, false);
    assert_eq!(held(&d), ["a.jsonl"]);
    assert_eq!(d.index.files["a.jsonl"], v1.files["a.jsonl"], "the held edit keeps the accepted bytes");
    assert_eq!(d.listed["a.jsonl"], ("{\"k\": \"x\", \"v\": 1}\n".len() as u64, 100), "and its listing, so it's read again next time");
    assert!(d.index.files.contains_key("b.jsonl"));
    let r = d.report.as_ref().unwrap();
    assert_eq!(r.findings[0].level, Level::Error);
    assert_eq!(r.findings[0].lines, [(2, "not json".to_string())]);
    // An error can't be accepted: accepting its id changes nothing.
    let id = r.findings[0].id.clone();
    let d2 = check(GT, &b, Some(&d.index), &[&id], &store, false);
    assert_eq!(held(&d2), ["a.jsonl"]);
    assert!(accept(tempfile::tempdir().unwrap().path(), TEST_UNIT, &r.findings[0], "-", "test").is_err());
    // A held new file is simply absent.
    let mut b2 = Box_::default();
    b2.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n", 100);
    b2.put("c.jsonl", "{oops\n", 300);
    let d3 = check(GT, &b2, Some(&v1), &[], &store, false);
    assert_eq!(held(&d3), ["c.jsonl"]);
    assert!(!d3.changed && !d3.index.files.contains_key("c.jsonl"));
    // A file not of the shape is an error, not passed over.
    b2.put("notes.csv", "a,b\n", 300);
    let d4 = check(GT, &b2, Some(&v1), &[], &store, false);
    assert_eq!(held(&d4), ["c.jsonl", "notes.csv"]);
}

#[test]
fn a_warning_holds_until_accepted_and_a_removal_too() {
    let store = Store::default();
    let mut b = Box_::default();
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n", 100);
    let v1 = check(GT, &b, None, &[], &store, false).index;
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n{\"k\": \"y\", \"v\": -4}\n", 200);
    let d = check(GT, &b, Some(&v1), &[], &store, false);
    assert_eq!(held(&d), ["a.jsonl"]);
    let w = d.report.as_ref().unwrap().findings[0].clone();
    assert_eq!(w.level, Level::Warning);
    assert!(valid_id(&w.id) && w.id.starts_with("gt-neg."), "{}", w.id);
    // Accepted: taken in, the version records it was taken with it.
    let v2 = check(GT, &b, Some(&v1), &[&w.id], &store, false);
    assert!(v2.changed && v2.report.is_none());
    assert_eq!(v2.index.accepted, [(w.id.clone(), vec!["a.jsonl".to_string()])].into());
    assert_ne!(v2.index.files["a.jsonl"], v1.files["a.jsonl"]);
    // Unaccepted afterwards: the version already in stays (it isn't held again).
    let v3 = check(GT, &b, Some(&v2.index), &[], &store, false);
    assert!(!v3.changed && v3.report.is_none());
    // A removal is a change: held (and the file kept) until its warning's accepted.
    b.files.remove("a.jsonl");
    let r = check(GT, &b, Some(&v2.index), &[], &store, false);
    assert_eq!(held(&r), ["a.jsonl"]);
    assert!(!r.changed && r.index.files.contains_key("a.jsonl"), "a held removal keeps the file");
    let rid = r.report.as_ref().unwrap().findings[0].id.clone();
    assert!(rid.starts_with("gt-removed."));
    let r2 = check(GT, &b, Some(&v2.index), &[&rid], &store, false);
    assert!(r2.changed && r2.index.files.is_empty() && r2.report.is_none());
    // The removed file's warnings no longer count: their acceptances are stale.
    // (The removal's acceptance counts while the version it was taken with is current: not
    // stale; the next change of version drops it.)
    assert_eq!(r2.index.accepted.keys().cloned().collect::<Vec<_>>(), [rid.clone()]);
    b.put("c.jsonl", "{\"k\": \"z\", \"v\": 1}\n", 300);
    let r3 = check(GT, &b, Some(&r2.index), &[&rid], &store, false);
    assert!(r3.changed && r3.index.accepted.is_empty());
}

#[test]
fn finding_ids_are_stable_across_unrelated_edits_and_new_when_a_flagged_line_changes() {
    let store = Store::default();
    let one = |text: &str| {
        let mut b = Box_::default();
        b.put("a.jsonl", text, 100);
        ids(&check(GT, &b, None, &[], &store, false))
    };
    let base = one("{\"k\": \"x\", \"v\": -1}\n{\"k\": \"y\", \"v\": 2}\n");
    // An unrelated line edited, or another added: the same id.
    assert_eq!(one("{\"k\": \"x\", \"v\": -1}\n{\"k\": \"y\", \"v\": 3}\n{\"k\": \"w\", \"v\": 9}\n"), base);
    // The flagged line moved down a line: still the same (its content, not its number).
    assert_eq!(one("{\"k\": \"q\", \"v\": 0}\n{\"k\": \"x\", \"v\": -1}\n"), base);
    // The flagged line edited, or a new flagged line: a new id.
    assert_ne!(one("{\"k\": \"x\", \"v\": -2}\n"), base);
    assert_ne!(one("{\"k\": \"x\", \"v\": -1}\n{\"k\": \"z\", \"v\": -5}\n"), base);
    // Another file with the same lines: another finding.
    let mut b = Box_::default();
    b.put("b.jsonl", "{\"k\": \"x\", \"v\": -1}\n{\"k\": \"y\", \"v\": 2}\n", 100);
    assert_ne!(ids(&check(GT, &b, None, &[], &store, false)), base);
    // A removal names the version removed: accepting one doesn't accept a later one's.
    let mut b = Box_::default();
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n", 100);
    let v1 = check(GT, &b, None, &[], &store, false).index;
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 2}\n", 200);
    let v2 = check(GT, &b, Some(&v1), &[], &store, false).index;
    let empty = Box_::default();
    assert_ne!(ids(&check(GT, &empty, Some(&v1), &[], &store, false)), ids(&check(GT, &empty, Some(&v2), &[], &store, false)));
}

#[test]
fn only_files_whose_size_or_time_changed_are_read_and_a_touch_changes_nothing() {
    let store = Store::default();
    let mut b = Box_::default();
    for i in 0..5 {
        b.put(&format!("f{i}.jsonl"), &format!("{{\"k\": \"{i}\", \"v\": {i}}}\n"), 100);
    }
    let v1 = check(GT, &b, None, &[], &store, false).index;
    // One new file, one touched (same bytes, a new time): only those two read; the touch changes
    // nothing, so neither does the version but for the new file.
    b.put("f9.jsonl", "{\"k\": \"9\", \"v\": 9}\n", 300);
    let (bytes, _) = b.files["f2.jsonl"].clone();
    b.files.insert("f2.jsonl".into(), (bytes, 300));
    let d = check(GT, &b, Some(&v1), &[], &store, false);
    let mut read = d.read.clone();
    read.sort();
    assert_eq!(read, ["f2.jsonl", "f9.jsonl"]);
    assert_eq!(d.index.files["f2.jsonl"], v1.files["f2.jsonl"]);
    // A touch alone: read, and nothing changes (no new version).
    let mut b2 = Box_::default();
    for i in 0..5 {
        b2.put(&format!("f{i}.jsonl"), &format!("{{\"k\": \"{i}\", \"v\": {i}}}\n"), 100);
    }
    let (bytes, _) = b2.files["f0.jsonl"].clone();
    b2.files.insert("f0.jsonl".into(), (bytes, 500));
    let t = check(GT, &b2, Some(&v1), &[], &store, false);
    assert_eq!(t.read, ["f0.jsonl"]);
    assert!(!t.changed && t.index == v1 && t.listed_changed);
    // Its new time kept: the next check reads nothing.
    let again = check(GT, &b2, Some(&v1), &[], &store, false);
    assert!(again.read.is_empty() && !again.changed && !again.listed_changed);
    // --full: every file read; still nothing changes.
    let f = check(GT, &b2, Some(&v1), &[], &store, true);
    assert_eq!(f.read.len(), 5);
    assert!(!f.changed);
}

/// A unit of pairs for the gate's cross-file rules: `<x>.data` with `<x>.toml`; a `.data` file
/// "bad" is an error; a version holding left.data "left-old" beside right.data "right-new" raises a
/// warning (a combination neither the candidate nor the old version has).
struct Pairs;

impl Checks for Pairs {
    fn unit(&self) -> &'static str {
        "_pairs"
    }
    fn version(&self) -> u32 {
        1
    }
    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        if bytes == b"bad" {
            c.findings.push(Finding::new("p-bad", Level::Error, &[path], &[], vec![path.into()], format!("{path} is bad")));
        }
        if let Some(r) = std::str::from_utf8(bytes).ok().and_then(|t| t.strip_prefix("see ")) {
            c.refers.push(r.trim().to_string());
        }
        c
    }
    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let get = |p: &str| -> Result<Option<String>> { Ok(if v.paths.contains(p) { Some(String::from_utf8_lossy(&(v.read)(p)?).into_owned()) } else { None }) };
        Ok(if get("left.data")?.as_deref() == Some("left-old") && get("right.data")?.as_deref() == Some("right-new") {
            vec![Finding::new("p-gap", Level::Warning, &["left", "right"], &[], vec!["left.data".into(), "right.data".into()], "a gap between left and right".into())]
        } else {
            Vec::new()
        })
    }
    fn partners(&self, path: &str, all: &BTreeSet<String>) -> Vec<String> {
        let other = match path.rsplit_once('.') {
            Some((s, "data")) => format!("{s}.toml"),
            Some((s, "toml")) => format!("{s}.data"),
            _ => return Vec::new(),
        };
        all.contains(&other).then_some(other).into_iter().collect()
    }
}

#[test]
fn paired_files_and_files_naming_a_held_one_are_held_together() {
    let store = Store::default();
    let mut b = Box_::default();
    b.put("reg.data", "one", 100);
    b.put("reg.toml", "about one", 100);
    let v1 = check(&Pairs, &b, None, &[], &store, false).index;
    // The data broken, its description edited cleanly: both held, both kept as accepted.
    b.put("reg.data", "bad", 200);
    b.put("reg.toml", "about two", 200);
    let d = check(&Pairs, &b, Some(&v1), &[], &store, false);
    assert_eq!(held(&d), ["reg.data", "reg.toml"]);
    assert!(!d.changed);
    // A new file naming a held new one: held with it.
    let mut b2 = b.clone();
    b2.put("reg.data", "one", 100);
    b2.put("reg.toml", "about one", 100);
    b2.put("new.data", "bad", 300);
    b2.put("user.data", "see new.data", 300);
    let d2 = check(&Pairs, &b2, Some(&v1), &[], &store, false);
    assert_eq!(held(&d2), ["new.data", "user.data"]);
}

#[test]
fn a_version_checked_whole_holds_every_change_when_old_and_new_files_clash() {
    let store = Store::default();
    let mut b = Box_::default();
    b.put("left.data", "left-old", 100);
    b.put("right.data", "right-old", 100);
    let v1 = check(&Pairs, &b, None, &[], &store, false).index;
    // The left edit broken (held, so left-old stays) and the right edit clean: taken alone it would
    // make left-old beside right-new, which neither the candidate nor the old version has.
    b.put("left.data", "bad", 200);
    b.put("right.data", "right-new", 200);
    let d = check(&Pairs, &b, Some(&v1), &[], &store, false);
    assert_eq!(held(&d), ["left.data", "right.data"]);
    assert!(!d.changed && d.index == v1);
    let r = d.report.unwrap();
    assert!(r.together.as_deref().is_some_and(|t| t.contains("a gap between left and right")), "{:?}", r.together);
    let gap = r.findings.iter().find(|f| f.id.starts_with("p-gap.")).unwrap().id.clone();
    // The gap accepted: the right edit goes in, the left stays held (its error).
    let d2 = check(&Pairs, &b, Some(&v1), &[&gap], &store, false);
    assert_eq!(held(&d2), ["left.data"]);
    assert!(d2.changed && d2.index.files["right.data"].file != v1.files["right.data"].file);
    assert!(d2.index.accepted.contains_key(&gap));
}

#[test]
fn quiet_files_alone_are_settled() {
    let now = Listing { files: [("a".to_string(), (1, 100)), ("b".to_string(), (2, 198)), ("c".to_string(), (1, 150))].into(), strays: vec![] };
    // a and c old enough; b changed 2 s before the listing: left for the next.
    assert_eq!(settle(&now, 200).files.keys().cloned().collect::<Vec<_>>(), ["a", "c"]);
    // The same in two listings a few seconds apart is no proof it's whole: by its time alone.
    assert_eq!(settle(&now, 203).files.keys().cloned().collect::<Vec<_>>(), ["a", "c"]);
    assert_eq!(settle(&now, 208).files.len(), 3);
}

/// A file written at T, listed at T+2 and T+5 (unchanged), the check at T+6: never taken while it
/// may be half written, and the key recorded is the one of what the check used, so a later
/// listing checks it again (docs/inputs.md §4.2).
#[test]
fn a_file_too_recent_is_neither_checked_nor_in_the_key_recorded() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("nas");
    let dropbox = root.join("inputs").join(TEST_UNIT);
    std::fs::create_dir_all(&dropbox).unwrap();
    std::fs::create_dir_all(root.join("state/build")).unwrap();
    let now = crate::agent::jobs::now_s();
    let t = now - 6;
    let p = dropbox.join("a.jsonl");
    std::fs::write(&p, "{\"k\": \"x\", \"v\": 1}\n").unwrap();
    std::fs::File::options().write(true).open(&p).unwrap().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t)).unwrap();
    let raw = list(&root, TEST_UNIT, false).unwrap();
    // The listings at T+2 and T+5: it isn't settled at either.
    assert!(settle(&raw, t + 2).files.is_empty() && settle(&raw, t + 5).files.is_empty());
    let planned = Planned { listing: settle(&raw, t + 5), accepted: BTreeSet::new() };
    let m: BTreeMap<String, String> = BTreeMap::new();
    let key = check_key(GT, &planned.listing, &planned.accepted, &m);
    // The check at T+6, given what the key was made from: it isn't taken, nor taken as removed.
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_UNIT, false, Some(planned)).unwrap();
    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    assert!(open(&root, &m, TEST_UNIT).unwrap().is_none());
    // Once it has held still, the listing (and so the key) is another: it's checked, and taken.
    let later = settle(&raw, t + 11);
    assert_eq!(later.files.len(), 1);
    assert_ne!(check_key(GT, &later, &BTreeSet::new(), &m), key);
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_UNIT, false, Some(Planned { listing: later, accepted: BTreeSet::new() })).unwrap();
    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    assert!(open(&root, &m, TEST_UNIT).unwrap().unwrap().index.files.contains_key("a.jsonl"));
    // A planned listing a file has changed since: left as it was (here, out), not taken half-read.
    std::fs::write(&p, "{\"k\": \"x\", \"v\": 1}\n{\"k\":").unwrap();
    let stale = Planned { listing: Listing { files: [("a.jsonl".to_string(), (1, 1))].into(), strays: vec![] }, accepted: BTreeSet::new() };
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_UNIT, false, Some(stale)).unwrap();
    let m2: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    assert_eq!(m2.get(&logical(TEST_UNIT)), m.get(&logical(TEST_UNIT)));
    assert!(!m2.contains_key(&held_logical(TEST_UNIT)));
}

/// An accepted file changed or deleted after the plan listed it: neither read nor taken as removed,
/// it stays as accepted; the next listing, which differs, checks it again.
#[test]
fn an_accepted_file_changed_or_gone_since_the_plan_stays_accepted() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("nas");
    let dropbox = root.join("inputs").join(TEST_UNIT);
    std::fs::create_dir_all(&dropbox).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let put = |name: &str, text: &str| {
        let p = dropbox.join(name);
        std::fs::write(&p, text).unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
    };
    put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n");
    put("b.jsonl", "{\"k\": \"y\", \"v\": 2}\n");
    let store = Store::default();
    let listing = list(&root, TEST_UNIT, false).unwrap();
    let mut b = Box_::default();
    for n in ["a.jsonl", "b.jsonl"] {
        b.files.insert(n.into(), (std::fs::read(dropbox.join(n)).unwrap(), listing.files[n].1));
    }
    let v1 = check(GT, &b, None, &[], &store, false);
    // The plan lists them; then a is rewritten (half-written, say) and b deleted.
    let planned = Planned { listing: listing.clone(), accepted: BTreeSet::new() };
    std::fs::write(dropbox.join("a.jsonl"), "{\"k\": \"x\",").unwrap();
    std::fs::remove_file(dropbox.join("b.jsonl")).unwrap();
    let (l, acc, unsettled) = gate::candidate(&root, TEST_UNIT, false, Some(planned), crate::agent::jobs::now_s()).unwrap();
    assert_eq!(unsettled, ["a.jsonl".to_string(), "b.jsonl".to_string()].into());
    let read_new = |p: &str| -> Result<Vec<u8>> { panic!("{p} read though it changed since the plan") };
    let read_old = |e: &FileEntry| store.copies.borrow().get(&e.file).cloned().context("no copy");
    let d2 = decide(&gate::Given { checks: GT, prev: Some(&v1.index), prev_listed: &v1.listed, listing: &l, unsettled: &unsettled, accepted: &acc, full: false, read_new: &read_new, read_old: &read_old }).unwrap();
    assert!(!d2.changed && d2.report.is_none() && d2.index == v1.index, "both stay as accepted, b not taken as removed");
    assert_eq!(d2.listed, v1.listed, "and their listed times stay, so the next check reads them again");
}

/// A nested unit (`timetables/gtfs`'s kind): its copies and records under its nested folder, its
/// version read back, its records' names the steps table's.
#[test]
fn a_nested_units_check_keeps_its_records_in_its_folder() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("nas");
    let dropbox = root.join("inputs").join(TEST_NESTED);
    std::fs::create_dir_all(&dropbox).unwrap();
    std::fs::create_dir_all(root.join("state/build")).unwrap();
    let put = |text: &str| {
        let p = dropbox.join("a.jsonl");
        std::fs::write(&p, text).unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60)).unwrap();
    };
    put("{\"k\": \"x\", \"v\": 1}\n");
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_NESTED, false, None).unwrap();
    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    let a = open(&root, &m, TEST_NESTED).unwrap().unwrap();
    assert!(a.version.starts_with("sources/inputs/_gate-nest/inner/@index.") && a.index.files["a.jsonl"].file.starts_with("sources/inputs/_gate-nest/inner/a."));
    for l in m.keys() {
        assert_eq!(unit_of(l), Some(TEST_NESTED), "{l}");
        assert!(crate::agent::steps::row("inputs").is_some_and(|s| (s.writes)(l, false)), "{l}");
    }
    // A change of version: the check's state names the index it replaced.
    put("{\"k\": \"x\", \"v\": 22}\n");
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_NESTED, false, None).unwrap();
    let m2: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    assert_eq!(read_listed(&root, &m2, TEST_NESTED).unwrap().replaced.as_deref(), Some(a.version.as_str()));
    // A copy named like a record can't be one: '@' starts no drop-box name.
    assert!(record_of("sources/inputs/_gate-nest/inner/index").is_none() && record_of("sources/inputs/x/b/@index").is_some_and(|r| r.0 == "x/b"));
    assert_eq!(flat(TEST_NESTED), "_gate-nest+inner");
}

/// An acceptance whose file was edited (and taken in without the warning) or removed is no longer
/// the version's: stale, which `scenic inputs unaccept --stale` removes.
#[test]
fn acceptances_of_files_changed_since_are_stale() {
    let store = Store::default();
    let mut b = Box_::default();
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": -1}\n", 100);
    b.put("b.jsonl", "{\"k\": \"y\", \"v\": -2}\n", 100);
    let d = check(GT, &b, None, &[], &store, false);
    let ws: Vec<String> = d.report.unwrap().findings.iter().map(|f| f.id.clone()).collect();
    let refs: Vec<&str> = ws.iter().map(String::as_str).collect();
    let v1 = check(GT, &b, None, &refs, &store, false).index;
    assert_eq!(v1.accepted.len(), 2);
    // a fixed: its warning no longer the version's; b's stays.
    b.put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n", 200);
    let v2 = check(GT, &b, Some(&v1), &refs, &store, false).index;
    assert_eq!(v2.accepted.keys().cloned().collect::<Vec<_>>(), [ws.iter().find(|w| v1.accepted[*w] == ["b.jsonl"]).unwrap().clone()]);
    // In the status: a's acceptance stale.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let name = format!("{}/@index.0123456789abcdef.json", store_dir(TEST_UNIT));
    std::fs::create_dir_all(root.join(store_dir(TEST_UNIT))).unwrap();
    std::fs::write(root.join(&name), serde_json::to_vec(&v2).unwrap()).unwrap();
    let m: BTreeMap<String, String> = [(logical(TEST_UNIT), name)].into();
    let acc: BTreeMap<String, BTreeSet<String>> = [(TEST_UNIT.to_string(), ws.iter().cloned().collect())].into();
    let v = view::of(root, &m, &[TEST_UNIT], &BTreeSet::new(), &acc, &mut view::Cache::default());
    let a_id = ws.iter().find(|w| v1.accepted[*w] == ["a.jsonl"]).unwrap();
    assert_eq!(v[0].stale, [a_id.clone()]);
}

#[test]
fn listing_passes_over_the_macs_files_drafts_and_the_apps_folders_and_names_strays() {
    let d = tempfile::tempdir().unwrap();
    let b = d.path().join("inputs").join(TEST_UNIT);
    for f in ["a.jsonl", ".DS_Store", "_draft.jsonl", "@eaDir/x", "#recycle/y", "todo/z", "how/w", "sub/v.jsonl"] {
        let p = b.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
    }
    let l = list(d.path(), TEST_UNIT, false).unwrap();
    assert_eq!(l.files.keys().cloned().collect::<Vec<_>>(), ["a.jsonl"]);
    assert_eq!(l.strays, ["sub/"]);
    let r = list(d.path(), TEST_UNIT, true).unwrap();
    assert_eq!(r.files.keys().cloned().collect::<Vec<_>>(), ["a.jsonl", "sub/v.jsonl"]);
    assert!(list(d.path(), "none-yet", false).unwrap().files.is_empty());
}

#[test]
fn the_key_follows_the_listing_the_acceptances_and_the_index() {
    let l = Listing { files: [("a.jsonl".to_string(), (10, 100))].into(), strays: vec![] };
    let m: BTreeMap<String, String> = BTreeMap::new();
    let none = BTreeSet::new();
    let k = check_key(GT, &l, &none, &m);
    assert_eq!(k, check_key(GT, &l.clone(), &none, &m));
    let mut l2 = l.clone();
    l2.files.insert("a.jsonl".into(), (10, 101));
    assert_ne!(check_key(GT, &l2, &none, &m), k, "a touch is a trigger");
    assert_ne!(check_key(GT, &l, &["gt-neg.0123456789abcdef".to_string()].into(), &m), k);
    let m2: BTreeMap<String, String> = [(logical(TEST_UNIT), "sources/inputs/_gate-test/@index.0123456789abcdef.json".to_string())].into();
    assert_ne!(check_key(GT, &l, &none, &m2), k);
}

#[test]
fn acceptances_are_written_once_and_undone() {
    let d = tempfile::tempdir().unwrap();
    let w = Finding::new("gt-neg", Level::Warning, &["a.jsonl"], &["x"], vec!["a.jsonl".into()], "a.jsonl: 1 line with a negative v".into());
    assert!(accept(d.path(), TEST_UNIT, &w, "m-0123456789abcdef", "test").unwrap());
    assert!(!accept(d.path(), TEST_UNIT, &w, "m-fedcba9876543210", "again").unwrap(), "create-new: the first stays");
    let a = read_acceptance(d.path(), TEST_UNIT, &w.id).unwrap();
    assert_eq!((a.member.as_str(), a.message.as_str()), ("m-0123456789abcdef", "a.jsonl: 1 line with a negative v"));
    assert_eq!(acceptances(d.path(), TEST_UNIT).unwrap(), [w.id.clone()].into());
    assert!(unaccept(d.path(), TEST_UNIT, &w.id).unwrap());
    assert!(acceptances(d.path(), TEST_UNIT).unwrap().is_empty());
    // An ask: all the held warnings (not its errors), and an id not held refused.
    let e = Finding::new("gt-line", Level::Error, &["b.jsonl"], &["y"], vec!["b.jsonl".into()], "b.jsonl: bad".into());
    let r = Report { findings: vec![w.clone(), e], ..Default::default() };
    let said = apply_ask(d.path(), &Ask { unit: TEST_UNIT.into(), all: true, by: "test".into(), ..Default::default() }, Some(&r), "-").unwrap();
    assert_eq!(said.len(), 1);
    assert_eq!(acceptances(d.path(), TEST_UNIT).unwrap(), [w.id.clone()].into());
    assert!(apply_ask(d.path(), &Ask { unit: TEST_UNIT.into(), accept: vec!["gt-neg.ffffffffffffffff".into()], ..Default::default() }, Some(&r), "-").is_err());
    // Asks left for the agent are taken in order, once.
    let home = d.path().join("agent");
    ask(&home, &Ask { unit: "a".into(), at: 2, ..Default::default() }).unwrap();
    ask(&home, &Ask { unit: "b".into(), at: 1, ..Default::default() }).unwrap();
    assert_eq!(take_asks(&home).iter().map(|a| a.unit.as_str()).collect::<Vec<_>>(), ["b", "a"]);
    assert!(take_asks(&home).is_empty());
}

#[test]
fn copies_are_named_by_their_path_and_content() {
    assert_eq!(copy_name("heritage", "fr-merimee.geojson", "0123456789abcdef").as_deref(), Some("sources/inputs/heritage/fr-merimee.0123456789abcdef.geojson"));
    assert_eq!(copy_name("translations", "batches/2026-10.fr.jsonl", "0123456789abcdef").as_deref(), Some("sources/inputs/translations/batches/2026-10.0123456789abcdef.fr.jsonl"));
    assert_eq!(copy_name("x", "README", "0123456789abcdef").as_deref(), Some("sources/inputs/x/README.0123456789abcdef.bin"));
    assert!(copy_name("x", "a\\b.json", "0123456789abcdef").is_none());
    assert!(in_drop_box("inputs/regions/a.toml") && in_drop_box("translations/x.jsonl") && in_drop_box("descriptions") && !in_drop_box("sources/inputs/x/a.0123456789abcdef.toml") && !in_drop_box("inputsx/a"));
}

/// The job end to end, on a folder standing in for the NAS: the files stored content-named, the
/// records changes handed off, the accepted version read back through `open`; a held change's
/// report named while it's held, and gone once it's taken in.
#[test]
fn the_job_hands_off_the_accepted_version_and_the_report() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("nas");
    let dropbox = root.join("inputs").join(TEST_UNIT);
    std::fs::create_dir_all(&dropbox).unwrap();
    let put = |name: &str, text: &str| {
        let p = dropbox.join(name);
        std::fs::write(&p, text).unwrap();
        // (Old enough to have held still.)
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
    };
    put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n");
    std::fs::create_dir_all(root.join("state/build")).unwrap();
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_UNIT, false, None).unwrap();
    let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    let a = open(&root, &m, TEST_UNIT).unwrap().unwrap();
    assert!(a.version.starts_with("sources/inputs/_gate-test/@index.") && root.join(&a.version).exists());
    assert_eq!(a.read("a.jsonl").unwrap(), b"{\"k\": \"x\", \"v\": 1}\n");
    assert!(!m.contains_key(&held_logical(TEST_UNIT)));
    // A warning: held, its report named; the version as it was.
    put("a.jsonl", "{\"k\": \"x\", \"v\": -1}\n");
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_UNIT, false, None).unwrap();
    let m2: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    assert_eq!(m2[&logical(TEST_UNIT)], a.version);
    let r = read_report(&root, &m2[&held_logical(TEST_UNIT)]).unwrap();
    assert_eq!(r.held, ["a.jsonl"]);
    // The status's view of it.
    let v = view::of(&root, &m2, &[TEST_UNIT], &BTreeSet::new(), &BTreeMap::new(), &mut view::Cache::default());
    assert_eq!((v.len(), v[0].state, v[0].held.clone()), (1, view::State::Held, vec!["a.jsonl".to_string()]));
    assert!(v[0].line().contains("1 warning held"), "{}", v[0].line());
    // Checked again while held: still held, its findings shown, flagged checking.
    let v = view::of(&root, &m2, &[TEST_UNIT], &[TEST_UNIT.to_string()].into(), &BTreeMap::new(), &mut view::Cache::default());
    assert!(v[0].state == view::State::Held && v[0].checking && !v[0].findings.is_empty());
    // Accepted: taken in, the report gone; the version replaced made young (its index touched),
    // so GC keeps it restorable.
    assert!(accept(&root, TEST_UNIT, &r.findings[0], "-", "test").unwrap());
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
    std::fs::File::options().write(true).open(root.join(&a.version)).unwrap().set_modified(old).unwrap();
    let mut out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    gate::run(&mut out, TEST_UNIT, false, None).unwrap();
    let m3: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
    assert_ne!(m3[&logical(TEST_UNIT)], a.version);
    assert!(std::fs::metadata(root.join(&a.version)).unwrap().modified().unwrap() > old + std::time::Duration::from_secs(86400));
    assert!(!m3.contains_key(&held_logical(TEST_UNIT)));
    assert!(m3.contains_key(&listed_logical(TEST_UNIT)));
    assert_eq!(open(&root, &m3, TEST_UNIT).unwrap().unwrap().read("a.jsonl").unwrap(), b"{\"k\": \"x\", \"v\": -1}\n");
    let v = view::of(&root, &m3, &[TEST_UNIT], &BTreeSet::new(), &BTreeMap::new(), &mut view::Cache::default());
    assert_eq!(v[0].state, view::State::Ok);
    assert!(v[0].stale.is_empty(), "the acceptance is the version's: {:?}", v[0].stale);
    // The NAS root a step gets refuses a drop-box path, naming the caller.
    let out = crate::out::Out::open(&root, &d.path().join("s")).unwrap();
    let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| out.path("inputs/_gate-test/a.jsonl"))).unwrap_err();
    let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("tests.rs") && msg.contains("crate::inputs::open"), "{msg}");
    assert_eq!(out.path("sources/inputs/x"), root.join("sources/inputs/x"));
}

#[test]
fn credits_come_from_the_descriptions_beside_the_tables() {
    let toml = "name = \"Repertoire\"\nwhat = \"Québec heritage\"\ncredit = \"Ministere de la Culture du Quebec\"\nlicence = \"CC-BY-4.0\"\nterritory = [\"CA-QC\"]\n".as_bytes();
    let (d, f, keyed) = parse_description_ok(toml);
    assert!(f.is_empty() && d.credit.starts_with("Ministere") && d.redistribute && keyed.len() == 16);
    // The credit and licence are required, the territory checked.
    let (none, f, _) = credits::parse_description("x.toml", b"what = \"x\"\nterritory = [\"france\"]\n");
    assert!(none.is_none() && f.len() == 3 && f.iter().all(|x| x.level == Level::Error), "{f:?}");
    // A credit edit changes the key's digest; the territory changes `keyed`, the credit doesn't.
    let (_, _, k2) = credits::parse_description("x.toml", b"credit = \"other\"\nlicence = \"CC-BY-4.0\"\nterritory = [\"CA-QC\"]\n");
    assert_eq!(k2, keyed);
    let mk = |credit: &str, extent: Option<credits::Extent>| credits::Described { unit: "heritage".into(), path: "qc.toml".into(), description: credits::Description { credit: credit.into(), ..d.clone() }, extent };
    let qc = credits::Extent::Box([-79.8, 45.0, -57.1, 62.6]);
    let a = credits::digest(&[mk("A", Some(qc.clone()))]);
    assert!(a.is_some() && a != credits::digest(&[mk("B", Some(qc.clone()))]));
    assert_eq!(credits::digest(&[]), None, "none: the catalog's key as it was");
    // Listed by the same rule as the table's: near Montreal its description in place of the
    // table's entry; around Tokyo neither.
    let region = |w: f64, s: f64, e: f64, n: f64| {
        let ring = vec![[w, s], [e, s], [e, n], [w, n], [w, s]];
        crate::coverage::DrawnRegion { id: "r".into(), name: "r".into(), outline: vec!["osm:1".into()], shapes: [("osm:1".to_string(), vec![vec![ring]])].into() }
    };
    let mtl = credits::catalog_credits(&[region(-73.7, 45.4, -73.5, 45.6)], &[], &[mk("Ministere", Some(qc.clone()))]);
    let whats: Vec<&str> = mtl.iter().filter_map(|c| c["what"].as_str()).collect();
    assert_eq!(whats.iter().filter(|w| **w == "Québec heritage").count(), 1);
    assert!(mtl.iter().any(|c| c["source"] == "Ministere"));
    let table = crate::rules::catalog_credits(&[region(-73.7, 45.4, -73.5, 45.6)], &[]).len();
    assert_eq!(mtl.len(), table, "the table's Quebec entry replaced, not added to");
    let tokyo = credits::catalog_credits(&[region(139.5, 35.5, 140.0, 36.0)], &[], &[mk("Ministere", Some(qc))]);
    assert!(!tokyo.iter().any(|c| c["source"] == "Ministere"));
    // A source for the whole world: anywhere; one whose extent isn't known yet: nowhere.
    let world = credits::catalog_credits(&[], &[], &[credits::Described { description: credits::Description { what: "New world source".into(), ..d.clone() }, ..mk("W", Some(credits::Extent::World("world".into()))) }]);
    assert!(world.iter().any(|c| c["what"] == "New world source" && c.get("areas").is_none()));
    assert!(!credits::catalog_credits(&[], &[], &[credits::Described { description: credits::Description { what: "Unknown".into(), ..d.clone() }, ..mk("U", None) }]).iter().any(|c| c["what"] == "Unknown"));
}

fn parse_description_ok(b: &[u8]) -> (credits::Description, Vec<Finding>, String) {
    let (d, f, k) = credits::parse_description("qc.toml", b);
    (d.unwrap(), f, k)
}
