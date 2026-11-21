//! Task #1423: what's resolved is reused. A region without water whose key is the one its accepted
//! geometry was resolved from takes that geometry; the folder's whole outcome is kept with the
//! check's state (`Recorded`) and taken by a check finding the same inputs. Each check here runs as
//! a new process would: a new `Regions`, the accepted version, the state's memo and the store
//! (a NAS root) as the last check left them.

use super::water_tests::{all, world, world_data, World};
use super::*;
use crate::inputs::gate::{decide, Decision, Given};
use crate::inputs::{FileEntry, Index, Listing};
use std::collections::HashMap;

/// The small world's folder (water_tests's, without the cover rules and the far region), and two
/// regions without water: `old` (relation 4) and `older`, naming it.
fn files() -> BTreeMap<String, Vec<u8>> {
    let mut f: BTreeMap<String, Vec<u8>> = all()
        .into_iter()
        .filter(|(p, _)| *p != "far.toml" && *p != "_rules.toml")
        .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
        .collect();
    f.insert("old.toml".into(), b"id = \"old\"\nname = \"Old\"\noutline = [\"osm:4\"]\n".to_vec());
    f.insert("x/older.toml".into(), b"id = \"older\"\nname = \"Older\"\noutline = [\"region:old\"]\n".to_vec());
    f
}

/// The state a check leaves for the next: the accepted version, the sizes and times, the memo.
#[derive(Clone, Default)]
struct State {
    index: Option<Index>,
    listed: BTreeMap<String, (u64, u64)>,
    memo: Option<serde_json::Value>,
    derived_by: Option<String>,
}

struct Fx {
    w: World,
    root: tempfile::TempDir,
}

impl Fx {
    fn new() -> Fx {
        Fx { w: world(), root: tempfile::tempdir().unwrap() }
    }

    fn nas(&self) -> store::nasfs::NasRoot {
        store::nasfs::NasRoot::local_for_test(self.root.path())
    }

    /// The checks as a check job prepares them: against the pass `names` names (the outline set's,
    /// the extract's and the land's names), the datasets named `data`.
    fn checks(&self, names: &str, data: &str) -> Regions {
        Regions::new(Some(Prepared {
            pass: Some("2026-09-28".into()),
            outlines: Some(Outlines::open(&self.w._d.path().join("outlines.sect")).unwrap()),
            data: Some(world_data(1.002, Vec::new(), HashMap::new(), data)),
            root: Some(self.nas()),
            names: names.into(),
            ..Default::default()
        }))
    }

    /// A check of `files` (every file read, as the daily full check reads them) after `s`; its
    /// files stored under the root, as the job stores them.
    fn check(&self, checks: &Regions, files: &BTreeMap<String, Vec<u8>>, s: &State) -> (Decision, State) {
        let listing = Listing {
            files: files.iter().map(|(p, b)| (p.clone(), (b.len() as u64, 1))).collect(),
            ..Default::default()
        };
        let root = self.nas();
        let rn = |p: &str| Ok(files[p].clone());
        let ro = |e: &FileEntry| -> Result<Vec<u8>> { Ok(root.join(&e.file).read()?) };
        let d = decide(&Given {
            checks,
            prev: s.index.as_ref(),
            prev_listed: &s.listed,
            listing: &listing,
            unsettled: &Default::default(),
            accepted: &Default::default(),
            full: true,
            read_new: &rn,
            read_old: &ro,
            memo: s.memo.as_ref(),
            derived_by: s.derived_by.as_deref(),
        })
        .unwrap();
        for (name, b) in &d.store {
            let p = root.join(name);
            p.parent().unwrap().create_dir_all().unwrap();
            p.write(b.as_slice()).unwrap();
        }
        let next = State {
            index: Some(d.index.clone()),
            listed: d.listed.clone(),
            memo: d.memo.clone(),
            derived_by: d.derived_by.clone(),
        };
        (d, next)
    }
}

/// What the check resolved: None when nothing (every outcome taken as recorded); else the regions
/// resolved and those of them taken as the accepted version stores them.
fn resolved(checks: &Regions) -> Option<(BTreeSet<String>, BTreeSet<String>)> {
    let last = checks.last.lock().unwrap();
    last.as_ref().map(|(_, o)| (o.resolved.keys().cloned().collect(), o.reused.clone()))
}

fn ids(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Every region of `files()`.
fn every() -> BTreeSet<String> {
    ids(&["a", "b", "disputed", "high-seas", "isl", "old", "older", "slice"])
}

/// Nothing changed: nothing resolves, and the accepted version, the sizes and times and the memo
/// are as they were, byte for byte. A change held (a region naming none) is held again the same,
/// nothing resolved either.
#[test]
fn nothing_changed_resolves_nothing() {
    let f = Fx::new();
    let first = f.checks("n", "fixture");
    let (d, s) = f.check(&first, &files(), &State::default());
    assert!(d.report.is_none(), "{:?}", d.report);
    assert_eq!(resolved(&first).map(|r| r.0), Some(every()), "the first check resolves every region");
    let again = f.checks("n", "fixture");
    let (d2, s2) = f.check(&again, &files(), &s);
    assert_eq!(resolved(&again), None, "nothing resolved");
    assert!(!d2.changed && !d2.listed_changed && d2.report.is_none(), "{:?}", d2.report);
    assert_eq!(serde_json::to_vec(&d2.index).unwrap(), serde_json::to_vec(s.index.as_ref().unwrap()).unwrap());
    assert_eq!(d2.memo, s.memo, "the memo as it was");
    // (Task #1167: the first resolve's step times kept, for the next resolve's progress.)
    let times = first.times();
    assert!(times.iter().any(|t| t.0 == super::progress::Step::Plain), "{times:?}");
    assert_eq!(serde_json::to_value(&times).unwrap(), s.memo.as_ref().unwrap()["times"]);
    assert_eq!(again.times(), times, "told the next check, which resolved nothing");
    // A change held: held again the same by the next check, which resolves nothing.
    let mut held = files();
    held.insert("y.toml".into(), b"id = \"y\"\nname = \"Y\"\noutline = [\"region:nowhere\"]\n".to_vec());
    let c = f.checks("n", "fixture");
    let (d3, s3) = f.check(&c, &held, &s2);
    let r3 = d3.report.clone().expect("held");
    assert_eq!(r3.held, ["y.toml"]);
    let c = f.checks("n", "fixture");
    let (d4, _) = f.check(&c, &held, &s3);
    assert_eq!(resolved(&c), None, "the held change's outcome recorded");
    assert_eq!(d4.report.map(|r| (r.held, r.findings)), Some((r3.held, r3.findings)));
    assert_eq!(serde_json::to_vec(&d4.index).unwrap(), serde_json::to_vec(&d3.index).unwrap());
}

/// One region's file changed: it and exactly the regions reading it resolve, the rest taken as
/// accepted; the version taken in is byte for byte a whole resolve's of the same inputs.
#[test]
fn a_region_changed_resolves_it_and_its_dependants_alone() {
    let f = Fx::new();
    let (_, s) = f.check(&f.checks("n", "fixture"), &files(), &State::default());
    let water = ids(&["a", "b", "disputed", "high-seas", "isl", "slice"]);
    // (Every region with water reads every region without: its water is less theirs.)
    for (path, edit, anew) in [
        (
            "old.toml",
            "id = \"old\"\nname = \"Old\"\noutline = [{ buffer = \"osm:4\", by = \"12 nmi\" }]\n",
            vec!["old", "older"],
        ),
        ("x/older.toml", "id = \"older\"\nname = \"Older!\"\noutline = [\"region:old\"]\n", vec!["older"]),
        ("w/isl.toml", &super::water_tests::ISL.replace("Island", "The island"), vec![]),
    ] {
        let mut edited = files();
        edited.insert(path.into(), edit.as_bytes().to_vec());
        let c = f.checks("n", "fixture");
        let (d, _) = f.check(&c, &edited, &s);
        assert!(d.changed && d.report.is_none(), "{path}: {:?}", d.report);
        let (all, reused) = resolved(&c).expect("resolved");
        assert_eq!(all, every(), "{path}");
        let made: BTreeSet<String> = all.difference(&reused).cloned().collect();
        assert_eq!(made, water.iter().cloned().chain(anew.iter().map(|s| s.to_string())).collect(), "{path}");
        // A whole resolve of the same inputs, nothing accepted before: the same version.
        let g = tempfile::tempdir().unwrap();
        let whole = Fx { w: world(), root: g };
        let w = whole.checks("n", "fixture");
        let (dw, _) = whole.check(&w, &edited, &State::default());
        assert_eq!(resolved(&w).map(|r| r.1), Some(BTreeSet::new()), "nothing reused");
        assert_eq!(serde_json::to_vec(&d.index).unwrap(), serde_json::to_vec(&dw.index).unwrap(), "{path}");
        for (name, b) in &dw.store {
            assert_eq!(f.nas().join(name).read().unwrap(), **b, "{path}: {name}");
        }
    }
}

/// What every region reads changed (the pass's outline set, extract or land: the names it's
/// resolved against): every region resolves. The datasets changed: the regions reading them (those
/// with water) resolve, those without are taken as accepted.
#[test]
fn a_shared_input_changed_resolves_what_reads_it() {
    let f = Fx::new();
    let (_, s) = f.check(&f.checks("n", "fixture"), &files(), &State::default());
    let c = f.checks("n2", "fixture");
    let (d, _) = f.check(&c, &files(), &s);
    assert!(d.report.is_none(), "{:?}", d.report);
    assert_eq!(resolved(&c), Some((every(), BTreeSet::new())), "the pass's names");
    let c = f.checks("n", "fixture2");
    f.check(&c, &files(), &s);
    assert_eq!(resolved(&c), Some((every(), ids(&["old", "older"]))), "the datasets");
}

/// The other passes' outline sets (what a region pinned to one is compared with) are in the check's
/// key and the outcome's: a check with another set of them takes no outcome recorded without.
#[test]
fn the_other_passes_outline_sets_are_in_the_keys() {
    let f = Fx::new();
    let (_, s) = f.check(&f.checks("n", "fixture"), &files(), &State::default());
    let mut c = f.checks("n", "fixture");
    c.ctx.as_mut().unwrap().others_names = "2026-08-31 sources/osm/2026-08-31/outlines.1.sect".into();
    f.check(&c, &files(), &s);
    assert_eq!(resolved(&c).map(|r| r.1), Some(ids(&["old", "older"])), "resolved again, the plain regions reused");
    let r = Regions::default();
    let root = f.nas();
    let l = Listing::default();
    let none = BTreeSet::new();
    let mut m = BTreeMap::from([
        ("sources/osm/2026-09-28/pass".to_string(), "p".to_string()),
        ("sources/osm/2026-09-28/outlines".to_string(), "o".to_string()),
    ]);
    let k = crate::inputs::check_key(&r, &root, &l, &none, &m).unwrap();
    m.insert("sources/osm/2026-08-31/outlines".into(), "o0".into());
    assert_ne!(crate::inputs::check_key(&r, &root, &l, &none, &m).unwrap(), k);
}

/// A problem and a warning, as a resolution says them.
fn said() -> Vec<resolve::Problem> {
    let p = |warning: bool, check: &'static str, n: &str| resolve::Problem {
        warning,
        check,
        about: vec![n.into(), "b".into()],
        found: vec!["overlap".into()],
        files: vec![format!("{n}.toml")],
        message: format!("{n}: said"),
    };
    vec![p(false, "rg-ref", "a"), p(true, "rg-cover", "c")]
}

/// A kept memo naming a step this app doesn't know (a later app's, or one renamed since) still
/// reads: its outcomes are kept, so nothing resolves again, and its other steps' times are told on.
#[test]
fn a_memo_naming_an_unknown_step_keeps_its_regions() {
    let f = Fx::new();
    let first = f.checks("n", "fixture");
    let (_, mut s) = f.check(&first, &files(), &State::default());
    let times = first.times();
    assert!(!times.is_empty());
    let kept = s.memo.as_mut().unwrap()["times"].as_array_mut().unwrap();
    kept.insert(0, serde_json::json!(["SomeLaterStep", 2.5]));
    let again = f.checks("n", "fixture");
    let (d, _) = f.check(&again, &files(), &s);
    assert_eq!(resolved(&again), None, "nothing resolved");
    assert!(!d.changed, "{:?}", d.report);
    assert_eq!(again.times(), times, "the known steps' times told on");
}

/// The memo's outcome read back says what was said, word for word, in order, each problem's level
/// kept (a warning stays a warning).
#[test]
fn a_recorded_outcome_says_the_same_problems() {
    let out = resolve::Outcome { problems: said(), ..Default::default() };
    let r = Recorded::of("k", &out);
    let m = serde_json::to_value(Memo { fmt: 1, outcomes: vec![r], times: vec![] }).unwrap();
    let back: Memo = serde_json::from_value(m).unwrap();
    assert_eq!(back.outcomes[0].problems(), out.problems);
}

/// A kept piece's problems read back with their level (a warning stays a warning).
#[test]
fn a_kept_piece_keeps_a_problems_level() {
    let r: std::result::Result<(), Vec<resolve::Problem>> = Err(said());
    let mut p = jobrt::resume_log::Put::default();
    kept::result(&mut p, &r, |_, _| {});
    let mut g = jobrt::resume_log::Get(&p.0);
    let back = kept::get_result(&mut g, |_| Ok(())).unwrap();
    g.end().unwrap();
    assert_eq!(back, r);
}
