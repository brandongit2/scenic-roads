//! The inputs crate's tests that need what only pipeline has (task #916 step 6d, docs/crates.md
//! §6.2): its steps' jobs held to their write sets as the steps table has them
//! (crate::out::open, crate::agent::steps), and the plan's reading of the lake fill
//! (crate::agent::build). A crate below pipeline can't dev-depend on it, so they live here.

/// The gate's jobs (crate::inputs::gate), as its tests in the inputs crate have them otherwise.
#[cfg(test)]
mod gate {
    use crate::inputs::*;
    use std::collections::{BTreeMap, BTreeSet};

    const GT: &gatetest::GateTest = &gatetest::GateTest(TEST_UNIT);

    /// A file written at T, listed at T+2 and T+5 (unchanged), the check at T+6: never taken while it
    /// may be half written, and the key recorded is the one of what the check used, so a later
    /// listing checks it again (docs/inputs.md §4.2).
    #[test]
    fn a_file_too_recent_is_neither_checked_nor_in_the_key_recorded() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let inputs_dir = root.join("inputs").join(TEST_UNIT);
        std::fs::create_dir_all(&inputs_dir).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let now = jobrt::jobs::now_s();
        let t = now - 6;
        let p = inputs_dir.join("a.jsonl");
        std::fs::write(&p, "{\"k\": \"x\", \"v\": 1}\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t))
            .unwrap();
        let raw = list(&store::nasfs::NasRoot::local_for_test(&root), TEST_UNIT, false).unwrap();
        // The listings at T+2 and T+5: it isn't settled at either.
        assert!(settle(&raw, t + 2).files.is_empty() && settle(&raw, t + 5).files.is_empty());
        let planned = Planned { listing: settle(&raw, t + 5), accepted: BTreeSet::new() };
        let m: BTreeMap<String, String> = BTreeMap::new();
        let key = check_key(GT, &planned.listing, &planned.accepted, &m);
        // The check at T+6, given what the key was made from: it isn't taken, nor taken as removed.
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_UNIT, false, Some(planned)).unwrap();
        let m: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        assert!(open(&store::nasfs::NasRoot::local_for_test(&root), &m, TEST_UNIT).unwrap().is_none());
        // Once it has held still, the listing (and so the key) is another: it's checked, and taken.
        let later = settle(&raw, t + 11);
        assert_eq!(later.files.len(), 1);
        assert_ne!(check_key(GT, &later, &BTreeSet::new(), &m), key);
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_UNIT, false, Some(Planned { listing: later, accepted: BTreeSet::new() })).unwrap();
        let m: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        assert!(open(&store::nasfs::NasRoot::local_for_test(&root), &m, TEST_UNIT)
            .unwrap()
            .unwrap()
            .index
            .files
            .contains_key("a.jsonl"));
        // A planned listing a file has changed since: left as it was (here, out), not taken half-read.
        std::fs::write(&p, "{\"k\": \"x\", \"v\": 1}\n{\"k\":").unwrap();
        let stale = Planned {
            listing: Listing { files: [("a.jsonl".to_string(), (1, 1))].into(), strays: vec![], ..Default::default() },
            accepted: BTreeSet::new(),
        };
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_UNIT, false, Some(stale)).unwrap();
        let m2: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        assert_eq!(m2.get(&logical(TEST_UNIT)), m.get(&logical(TEST_UNIT)));
        assert!(!m2.contains_key(&held_logical(TEST_UNIT)));
    }

    /// A nested unit (`timetables/gtfs`'s kind): its copies and records under its nested folder, its
    /// version read back, its records' names the steps table's.
    #[test]
    fn a_nested_units_check_keeps_its_records_in_its_folder() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let inputs_dir = root.join("inputs").join(TEST_NESTED);
        std::fs::create_dir_all(&inputs_dir).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let put = |text: &str| {
            let p = inputs_dir.join("a.jsonl");
            std::fs::write(&p, text).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
                .unwrap();
        };
        put("{\"k\": \"x\", \"v\": 1}\n");
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_NESTED, false, None).unwrap();
        let m: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        let a = open(&store::nasfs::NasRoot::local_for_test(&root), &m, TEST_NESTED).unwrap().unwrap();
        assert!(
            a.version.starts_with("sources/inputs/_gate-nest/inner/@index.")
                && a.index.files["a.jsonl"].file.starts_with("sources/inputs/_gate-nest/inner/a.")
        );
        for l in m.keys() {
            assert_eq!(unit_of(l), Some(TEST_NESTED), "{l}");
            assert!(crate::agent::steps::row("inputs").is_some_and(|s| (s.writes)(l, false)), "{l}");
        }
        // A change of version: the check's state names the index it replaced.
        put("{\"k\": \"x\", \"v\": 22}\n");
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_NESTED, false, None).unwrap();
        let m2: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        assert_eq!(
            read_listed(&store::nasfs::NasRoot::local_for_test(&root), &m2, TEST_NESTED).unwrap().replaced.as_deref(),
            Some(a.version.as_str())
        );
        // A copy named like a record can't be one: '@' starts no inputs-folder name.
        assert!(
            record_of("sources/inputs/_gate-nest/inner/index").is_none()
                && record_of("sources/inputs/x/b/@index").is_some_and(|r| r.0 == "x/b")
        );
        assert_eq!(flat(TEST_NESTED), "_gate-nest+inner");
    }

    /// The job end to end, on a folder standing in for the NAS: the files stored content-named, the
    /// records changes handed off, the accepted version read back through `open`; a held change's
    /// report named while it's held, and gone once it's taken in.
    #[test]
    fn the_job_hands_off_the_accepted_version_and_the_report() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let inputs_dir = root.join("inputs").join(TEST_UNIT);
        std::fs::create_dir_all(&inputs_dir).unwrap();
        let put = |name: &str, text: &str| {
            let p = inputs_dir.join(name);
            std::fs::write(&p, text).unwrap();
            // (Old enough to have held still.)
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        };
        put("a.jsonl", "{\"k\": \"x\", \"v\": 1}\n");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_UNIT, false, None).unwrap();
        let m: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        let a = open(&store::nasfs::NasRoot::local_for_test(&root), &m, TEST_UNIT).unwrap().unwrap();
        assert!(a.version.starts_with("sources/inputs/_gate-test/@index.") && root.join(&a.version).exists());
        assert_eq!(a.read("a.jsonl").unwrap(), b"{\"k\": \"x\", \"v\": 1}\n");
        assert!(!m.contains_key(&held_logical(TEST_UNIT)));
        // A warning: held, its report named; the version as it was.
        put("a.jsonl", "{\"k\": \"x\", \"v\": -1}\n");
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_UNIT, false, None).unwrap();
        let m2: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        assert_eq!(m2[&logical(TEST_UNIT)], a.version);
        let r = read_report(&store::nasfs::NasRoot::local_for_test(&root), &m2[&held_logical(TEST_UNIT)]).unwrap();
        assert_eq!(r.held, ["a.jsonl"]);
        // The status's view of it.
        let v = view::of(
            &store::nasfs::NasRoot::local_for_test(&root),
            &m2,
            &[TEST_UNIT],
            &BTreeSet::new(),
            &BTreeMap::new(),
            &mut view::Cache::default(),
        );
        assert_eq!((v.len(), v[0].state, v[0].held.clone()), (1, view::State::Held, vec!["a.jsonl".to_string()]));
        assert!(v[0].line().contains("1 warning held"), "{}", v[0].line());
        // Checked again while held: still held, its findings shown, flagged checking.
        let v = view::of(
            &store::nasfs::NasRoot::local_for_test(&root),
            &m2,
            &[TEST_UNIT],
            &[TEST_UNIT.to_string()].into(),
            &BTreeMap::new(),
            &mut view::Cache::default(),
        );
        assert!(v[0].state == view::State::Held && v[0].checking && !v[0].findings.is_empty());
        // Accepted: taken in, the report gone; the version replaced made young (its index touched),
        // so GC keeps it restorable.
        assert!(accept(&store::nasfs::NasRoot::local_for_test(&root), TEST_UNIT, &r.findings[0], "-", "test").unwrap());
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        std::fs::File::options().write(true).open(root.join(&a.version)).unwrap().set_modified(old).unwrap();
        let mut out =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "inputs").unwrap();
        gate::run(&mut out, TEST_UNIT, false, None).unwrap();
        let m3: BTreeMap<String, String> = buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(
            &root.join("state/build/manifest.json"),
        ))
        .unwrap();
        assert_ne!(m3[&logical(TEST_UNIT)], a.version);
        assert!(
            std::fs::metadata(root.join(&a.version)).unwrap().modified().unwrap()
                > old + std::time::Duration::from_secs(86400)
        );
        assert!(!m3.contains_key(&held_logical(TEST_UNIT)));
        assert!(m3.contains_key(&listed_logical(TEST_UNIT)));
        assert_eq!(
            open(&store::nasfs::NasRoot::local_for_test(&root), &m3, TEST_UNIT)
                .unwrap()
                .unwrap()
                .read("a.jsonl")
                .unwrap(),
            b"{\"k\": \"x\", \"v\": -1}\n"
        );
        let v = view::of(
            &store::nasfs::NasRoot::local_for_test(&root),
            &m3,
            &[TEST_UNIT],
            &BTreeSet::new(),
            &BTreeMap::new(),
            &mut view::Cache::default(),
        );
        assert_eq!(v[0].state, view::State::Ok);
        assert!(v[0].stale.is_empty(), "the acceptance is the version's: {:?}", v[0].stale);
        // The NAS root a step gets refuses an inputs folder path, naming the caller.
        let out =
            buildstore::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s")).unwrap();
        let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| out.path("inputs/_gate-test/a.jsonl")))
            .unwrap_err();
        let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("tests.rs") && msg.contains("inputs::open"), "{msg}");
        assert_eq!(out.path("sources/inputs/x"), store::nasfs::NasPath::local_for_test(root.join("sources/inputs/x")));
    }
}

/// The rail-feeds job's (crate::rail).
#[cfg(test)]
mod rail {
    use crate::rail::*;
    use buildstore::out::Out;
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_feeds_list_names_each_zip() {
        let d = tempfile::tempdir().unwrap();
        let mut out = crate::out::open(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("root")),
            &d.path().join("scratch"),
            "rail-feeds",
        )
        .unwrap();
        let cached = d.path().join("c.zip");
        std::fs::write(&cached, b"cached").unwrap();
        let cname = out.put_file(&zip_logical("mdb-1"), "zip", &cached).unwrap();
        let fetched: Fetched = [(cname.clone(), "2026-09-30".to_string())].into();
        out.put_bytes(FETCHED, "json", &serde_json::to_vec(&fetched).unwrap()).unwrap();
        let gtfs = d.path().join("gtfs");
        std::fs::create_dir_all(&gtfs).unwrap();
        std::fs::write(gtfs.join("sncf.zip"), b"new").unwrap();
        let found = d.path().join("feeds.json");
        std::fs::write(&found, r#"{"feeds": [{"id": "sncf", "zip": "new", "fetched": "2026-10-04"}, {"id": "mdb-1", "zip": "cache", "fetched": "2026-09-30"}, {"id": "mdb-2", "status": "replaced by sncf"}]}"#).unwrap();
        assert_eq!(put_feeds(&mut out, &found, &gtfs, FEEDS).unwrap(), 1);
        let v: Value = serde_json::from_slice(&(out.path(out.get(FEEDS).unwrap())).read().unwrap()).unwrap();
        let f = v["feeds"].as_array().unwrap();
        assert_eq!(f[0]["zip"], Value::from(out.get(&zip_logical("sncf")).unwrap()));
        assert_eq!((f[0]["fetched"].as_str(), f[1]["fetched"].as_str()), (Some("2026-10-04"), Some("2026-09-30")));
        assert_eq!(f[1]["zip"], Value::from(cname.as_str()));
        assert!(f[2].get("zip").is_none());
        // For counting: the zips' paths on the NAS.
        let c = feeds_for_counting(&out, FEEDS).unwrap();
        assert!(c["feeds"][0]["path"].as_str().unwrap().ends_with(".zip") && c["feeds"][2].get("path").is_none());
        // A zip fetched again, the same file as the cached one: kept once, counting from its new day.
        std::fs::write(gtfs.join("mdb-1.zip"), b"cached").unwrap();
        std::fs::write(&found, r#"{"feeds": [{"id": "mdb-1", "zip": "new", "fetched": "2026-10-20"}]}"#).unwrap();
        assert_eq!(put_feeds(&mut out, &found, &gtfs, FEEDS).unwrap(), 1);
        assert_eq!(out.get(&zip_logical("mdb-1")), Some(cname.as_str()));
        // The ferry list beside it: a zip the rail list just uploaded named as the NAS's.
        std::fs::write(&found, r#"{"feeds": [{"id": "mdb-1", "name": "M", "zip": "cache", "fetched": "2026-10-20"}, {"id": "ferry", "status": "http 404"}]}"#).unwrap();
        assert_eq!(put_feeds(&mut out, &found, &gtfs, FERRY_FEEDS).unwrap(), 0);
        let c = feeds_for_counting(&out, FERRY_FEEDS).unwrap();
        assert_eq!(c["feeds"][0]["fetched"], "2026-10-20");
        assert!(c["feeds"][0]["path"].as_str().unwrap().ends_with(".zip") && c["feeds"][1].get("path").is_none());
        assert_eq!(read_fetched(&out).unwrap()[&cname], "2026-10-20");
    }

    /// A declaration's pin goes in through the rail-feeds job's hand-off, as its downloads do: once
    /// a term is made (the records the lead's alone), the job's save is a hand-off naming the zip and
    /// its day; a feed with a zip already isn't pinned again.
    #[test]
    fn a_pin_is_handed_off_by_rail_feeds() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("root"), d.path().join("scratch"));
        // (A term made: the records are the lead's, and only a hand-off saves.)
        std::fs::create_dir_all(root.join("state/build/terms")).unwrap();
        std::fs::create_dir_all(root.join("old/gtfs")).unwrap();
        std::fs::write(root.join("old/gtfs/mbta.zip"), b"m").unwrap();
        let decls = [
            serde_json::json!({"id": "mbta", "format": "gtfs", "pin": {"zip": "old/gtfs/mbta.zip", "day": "2026-09-28"}}),
            serde_json::json!({"id": "sncf", "format": "gtfs"}),
        ];
        let handoff = d.path().join("handoff.json");
        let mut out = crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &scratch, "rail-feeds")
            .unwrap()
            .handing_off(&handoff);
        assert_eq!(pin_seeds(&mut out, &decls, &scratch.join("pins")).unwrap(), 1);
        out.save().unwrap();
        assert!(root.join("old/gtfs/mbta.zip").exists(), "the pin's zip is copied, not moved");
        // (Its hand-offs: the folder's files.)
        fn files(d: &Path) -> Vec<PathBuf> {
            std::fs::read_dir(d)
                .unwrap()
                .flatten()
                .flat_map(|e| if e.path().is_dir() { files(&e.path()) } else { vec![e.path()] })
                .collect()
        }
        let named: String =
            files(&handoff).iter().map(|f| String::from_utf8_lossy(&std::fs::read(f).unwrap()).into_owned()).collect();
        let zip = out.get(&zip_logical("mbta")).unwrap().to_string();
        assert!(named.contains(&zip) && named.contains(FETCHED), "{named}");
        assert_eq!(read_fetched(&out).unwrap()[&zip], "2026-09-28");
        assert!(!root.join("state/build/manifest.json").exists(), "the records untouched: handed off");
        assert_eq!(pin_seeds(&mut out, &decls, &scratch.join("pins")).unwrap(), 0, "pinned once");
    }

    #[test]
    fn an_upload_failing_loses_no_download() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("root"), d.path().join("scratch"));
        let gtfs = d.path().join("gtfs");
        std::fs::create_dir_all(&gtfs).unwrap();
        std::fs::write(gtfs.join("sncf.zip"), b"new").unwrap();
        // The second new zip can't be uploaded (it isn't there): the first stays in the sources,
        // with its day, though its local copy is gone; no list.
        let found = d.path().join("feeds.json");
        std::fs::write(&found, r#"{"feeds": [{"id": "sncf", "zip": "new", "fetched": "2026-10-04"}, {"id": "renfe", "zip": "new", "fetched": "2026-10-04"}]}"#).unwrap();
        let mut out = crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &scratch, "rail-feeds").unwrap();
        assert!(put_feeds(&mut out, &found, &gtfs, FEEDS).is_err());
        let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
        let sncf = out.get(&zip_logical("sncf")).unwrap().to_string();
        assert!(!gtfs.join("sncf.zip").exists() && out.path(&sncf).exists());
        assert_eq!(read_fetched(&out).unwrap()[&sncf], "2026-10-04");
        assert!(out.get(FEEDS).is_none());
        // What a failed run downloaded, likewise.
        std::fs::write(gtfs.join("a.zip"), b"a").unwrap();
        std::fs::create_dir_all(gtfs.join("b.zip")).unwrap();
        let mut out = crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &scratch, "rail-feeds").unwrap();
        assert!(keep_downloads(&mut out, &gtfs).is_err());
        let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch).unwrap();
        let a = out.get(&zip_logical("a")).unwrap().to_string();
        assert!(read_fetched(&out).unwrap().get(&a).is_some_and(|d| d.len() == 10));
        assert!(read_fetched(&out).unwrap().contains_key(&sncf), "the other days kept");
    }
}

/// The lake fill in the plan (crate::lakefill, crate::agent::build).
#[cfg(test)]
mod lakefill {
    #[test]
    fn the_water_waits_for_the_fill_until_it_has_failed_a_few_times() {
        use crate::agent::build::{water_waits_for_lakefill as waits, LAKEFILL_HOLDS};
        assert!(waits(true, 0) && waits(true, LAKEFILL_HOLDS - 1));
        assert!(!waits(true, LAKEFILL_HOLDS) && !waits(true, LAKEFILL_HOLDS + 5));
        assert!(!waits(false, 0), "no lakefill work: nothing to wait for");
    }

    #[test]
    fn the_fill_runs_once_a_pass_and_source_and_the_water_and_terrain_name_it() {
        use crate::agent::build;
        let date = "2026-09-28";
        let mut m = std::collections::BTreeMap::new();
        let done = crate::pool::records::Keys::default();
        // Without the pass's basemap and outlines, or the source: nothing.
        assert!(build::lakefill_work(date, &m, &done, Some("a.zip 1")).is_none());
        m.insert(format!("layers/basemap/world-{date}"), "layers/basemap/world-x.0123456789abcdef.pmtiles".to_string());
        m.insert(format!("sources/osm/{date}/outlines"), "sources/osm/x/outlines.0123456789abcdef.sect".to_string());
        assert!(build::lakefill_work(date, &m, &done, None).is_none());
        let w = build::lakefill_work(date, &m, &done, Some("a.zip 1")).unwrap();
        assert_eq!(w.step, "lakefill");
        let mut done2 = crate::pool::records::Keys::default();
        done2.record("lakefill", &w.targets);
        assert!(build::lakefill_work(date, &m, &done2, Some("a.zip 1")).is_none(), "done");
        assert!(build::lakefill_work(date, &m, &done2, Some("a.zip 2")).is_some(), "a zip replaced");
        // The water's key and the depth's water pin take the fill once the manifest has it.
        let water = build::water_work(&crate::agent::build::PassCtx::default().ctx(date, &m), &done).unwrap().targets
            [0]
        .1
        .clone();
        let pin = crate::terrain_pack::water_source_pin(&m).unwrap();
        assert_eq!(pin, "layers/basemap/world-x.0123456789abcdef.pmtiles");
        m.insert(crate::lakefill::logical(date), "work/lakefill/2026-09-28.0123456789abcdef.sect".to_string());
        let filled = build::water_work(&crate::agent::build::PassCtx::default().ctx(date, &m), &done).unwrap().targets
            [0]
        .1
        .clone();
        assert_ne!(filled, water);
        assert_eq!(
            crate::terrain_pack::water_source_pin(&m).unwrap(),
            "layers/basemap/world-x.0123456789abcdef.pmtiles + work/lakefill/2026-09-28.0123456789abcdef.sect"
        );
        // A newer pass's basemap: the older pass's fill is drawn by neither, until one is made from it.
        let newer = "2027-03-01";
        m.insert(
            format!("layers/basemap/world-{newer}"),
            "layers/basemap/world-y.fedcba9876543210.pmtiles".to_string(),
        );
        assert_eq!(crate::lakefill::current(&m), None);
        assert_eq!(
            crate::terrain_pack::water_source_pin(&m).unwrap(),
            "layers/basemap/world-y.fedcba9876543210.pmtiles"
        );
        let w_new = build::water_work(&crate::agent::build::PassCtx::default().ctx(newer, &m), &done).unwrap().targets
            [0]
        .1
        .clone();
        m.insert(crate::lakefill::logical(newer), "work/lakefill/2027-03-01.0123456789abcdef.sect".to_string());
        assert_ne!(
            build::water_work(&crate::agent::build::PassCtx::default().ctx(newer, &m), &done).unwrap().targets[0].1,
            w_new
        );
        assert_eq!(crate::lakefill::current(&m).map(|(l, _)| l), Some(crate::lakefill::logical(newer)));
    }
}
