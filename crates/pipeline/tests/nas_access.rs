//! Every NAS access goes through store::nasfs (docs/nas-access.md §5, task #530). The NAS project
//! folder is a `NasRoot`, its files `NasPath`s: no `Path`, so a raw `std::fs` call on a NAS file
//! doesn't compile, and the type is the guard there. What this scan catches is the escapes, in
//! every item outside `#[cfg(test)]` code and outside store::nasfs itself:
//!
//! - an `unchecked_path()` call (the one way to a raw path) not listed in `EXEMPT` with how many
//!   the item makes and why;
//! - a `NasRoot::from_arg` (a root from a string the user or a parent process gave) not listed in
//!   `FROM_ARG`; `NasRoot::find` and `local_for_test` are the other constructors;
//! - a NAS location named literally (`/Volumes/`, the NAS's name, the project's folder on the share,
//!   `smb://`) outside store::nas, not listed in `LITERALS`;
//! - in tools/, dem/ and producers/, a script naming the NAS (`TOOLS_MARKERS`) not listed in
//!   `TOOLS` (decision D4: the operator tools and the Python steps are listed, not migrated).
//!
//! Each list fails when an entry matches nothing (or a count differs), so none can go stale.
//!
//! scans: crates tools dem producers

mod common;

use common::*;
use std::collections::BTreeMap;

/// Items making `unchecked_path()` calls: (file, item, how many, why).
const EXEMPT: &[(&str, &str, usize, &str)] = &[
    // The store's own NAS readers and writers, below the type.
    ("crates/store/src/iopool.rs", "open_len", 1, "the I/O pool runs the open on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "read_all", 1, "the I/O pool runs the read on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "stat", 1, "the I/O pool runs the stat on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "exists", 1, "the I/O pool runs the stat on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "list", 1, "the I/O pool runs the listing on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "create_dir_all", 1, "the I/O pool runs the make on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "rename", 2, "the I/O pool runs the rename on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "write_new", 1, "the I/O pool runs the create-new on its worker thread, under its timeout"),
    ("crates/store/src/iopool.rs", "write", 1, "the I/O pool runs the write on its worker thread, under its timeout"),
    ("crates/store/src/range.rs", "open", 1, "a PooledFile keeps its path for its errors (opened and counted by the pool)"),
    ("crates/store/src/naming.rs", "write_atomic_as", 1, "a content-named upload: its own temporary name, an exclusive rename, a read-back past the cache, counted once by its caller as the upload lands"),
    ("crates/store/src/blobs.rs", "get", 1, "a whole local copy by crate::sys::copy_data, counted by its caller (pipeline::localread)"),
    // A child process's or a program's argument.
    ("crates/pipeline/src/agent/gc.rs", "run", 1, "the project folder as the NAS sees it, for the uploads' check over SSH (store::nas::on_nas)"),
    ("crates/pipeline/src/agent/jobs.rs", "root_arg", 1, "a child process's --root, its spec's: the spawners' one way out"),
    ("crates/pipeline/src/agent/mod.rs", "osm_pass_cmd", 2, "the OSM pass's --planet and --planetiler arguments"),
    ("crates/pipeline/src/osmpass.rs", "filtered_nas", 1, "osmium's input, streamed over SSH from its path on the NAS"),
    ("crates/pipeline/src/osmpass.rs", "run_pass", 1, "the extract program's input, read where it lies"),
    ("crates/pipeline/src/osmpass.rs", "patch_ferries", 1, "osmium's input, read where it lies"),
    ("crates/pipeline/src/ovconv.rs", "ferries_job", 1, "osmium's input, read where it lies"),
    ("crates/pipeline/src/treepacks.rs", "run_args", 2, "the trees program's --chm-store and --leaf folders"),
    ("crates/pipeline/src/treepacks.rs", "build_lo", 1, "the trees program's mids, as its arguments"),
    ("crates/pipeline/src/buildtiles.rs", "stage", 1, "a tile linked for the scenic program to read where it lies"),
    ("crates/pipeline/src/rail.rs", "cache_index", 1, "a zip's path in the counters' index, opened by the counting program"),
    ("crates/pipeline/src/rail.rs", "feeds_for_counting", 1, "a zip's path in the feeds list, opened by the counting program"),
    ("crates/pipeline/src/inputs/gtfs.rs", "zips", 1, "the counters' zip reader opens a member by its file's path, each time"),
    ("crates/pipeline/src/dem/declared.rs", "locate", 1, "the converting program's input, read where it lies"),
    ("crates/pipeline/src/offload.rs", "run_task", 1, "a step's place on the NAS, as its program's argument"),
    ("crates/pipeline/src/bin/scenic-build.rs", "step_main", 1, "the filtered planet streamed over SSH from its path on the NAS"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pois_step", 1, "the extract program's input, read where it lies"),
    ("crates/pipeline/src/bin/scenic-build.rs", "heritage_step", 1, "osmium's input, read where it lies"),
    ("crates/pipeline/src/bin/scenic-build.rs", "areas_over_cover", 1, "osmium's input, read where it lies"),
    ("crates/pipeline/src/bin/scenic-build.rs", "whs_over_cover", 1, "osmium's input, read where it lies (no room for a copy)"),
    ("crates/pipeline/src/bin/scenic-build.rs", "heritage_inputs", 3, "the registers and matches linked for heritage.py to read where they lie"),
    ("crates/pipeline/src/bin/scenic-build.rs", "unit_snap", 1, "the programs' NAS folder, as their argument"),
    ("crates/pipeline/src/bin/scenic-build.rs", "unit_step", 2, "the programs' NAS folders, as their arguments"),
    // A library or a reader that opens a path itself.
    ("crates/pipeline/src/bin/scenic-build.rs", "catalogue", 2, "store::catalogue's writer: its own temporary name and read-back"),
    ("crates/pipeline/src/bin/scenic-build.rs", "open_units", 2, "basepart's watched copies read the base packs by path"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pageview_months", 1, "pageviews' store of the months' indexes, read and written by path"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rail_feeds_step", 3, "railfeeds reads the catalogue, the checked list and the keys by path"),
    ("crates/pipeline/src/bin/scenic-build.rs", "lakefill_step", 3, "lakefill's readers open the basemap, the outlines and the zips by path"),
    ("crates/pipeline/src/inputs/regions/data.rs", "read", 1, "GDAL and the shapefile reader read the datasets by path"),
    ("crates/pipeline/src/namestodo.rs", "translation_files", 1, "the names crate's tables read their files by path"),
    // A raw path for what isn't a file's read or write.
    ("crates/pipeline/src/out.rs", "file", 1, "a local lock file named after the root's real path"),
    ("crates/pipeline/src/agent/room.rs", "ours", 1, "a local cache file's real path against the root's"),
    ("crates/pipeline/src/agent/room.rs", "nas_free", 1, "the share's free space: its mount's statfs"),
    ("crates/pipeline/src/agent/cond.rs", "resources", 1, "the share's free space: its mount's statfs"),
    ("crates/pipeline/src/coord/mod.rs", "nas_prober", 1, "the share's mount: its statfs"),
    ("crates/pipeline/src/budget/nas.rs", "probe_dir", 1, "the NAS's speed probe measures raw calls, never counted as the build's NAS work"),
    ("crates/pipeline/src/agent/live.rs", "add_files", 1, "a note's full path (store::nasio's ring) shown under the root"),
    ("crates/server/src/data.rs", "set_nas", 1, "the I/O pool's probe: a stat of the root itself, on its own thread"),
];

/// Items constructing a root from a user's or a parent's string (`NasRoot::from_arg`): (file, item).
const FROM_ARG: &[(&str, &str)] = &[
    ("crates/pipeline/src/bin/scenic.rs", "root"),
    ("crates/pipeline/src/bin/scenic.rs", "main"),
    ("crates/pipeline/src/bin/scenic-build.rs", "step_main"),
    ("crates/pipeline/src/bin/scenic-build.rs", "unit_step"),
    // (`--pyramids <dir>`, `--into <dir>`: a scratch store of GSI's tiles, task #806.)
    ("crates/pipeline/src/bin/scenic-build.rs", "pyramids_arg"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pyramids_fill"),
    ("crates/pipeline/src/bin/scenic-polar.rs", "main"),
    ("crates/server/src/main.rs", "main"),
    ("crates/server/src/data.rs", "find_nas"),
    ("crates/pipeline/examples/checklist.rs", "main"),
    ("crates/pipeline/examples/base_measure.rs", "main"),
];

/// Items naming a NAS location literally (outside store::nas): (file, item, why).
const LITERALS: &[(&str, &str, &str)] = &[];

/// What names the NAS in a script.
const TOOLS_MARKERS: &[&str] = &["/Volumes/", "fishandchips", "personal/projects/scenic-roads", "smb://"];

/// The scripts allowed to name the NAS (D4: listed, not migrated): (path prefix, why).
const TOOLS: &[(&str, &str)] = &[
    ("tools/nas/", "tar over SSH and the moves: the bulk path, where a per-file layer would be slower"),
    ("tools/app/publish.sh", "publishing: copies the app to the NAS and the Macs"),
    ("tools/app/install.sh", "installing a Mac: the share's mount"),
    ("tools/app/remote-tests.sh", "the test Mac's run: whether the NAS is mounted"),
    ("tools/dev/health.sh", "the health check: the share's mount"),
    ("tools/taskboard/launchd/backup.sh", "the task board's nightly backup to the NAS"),
    ("tools/status/service.swift", "the status app: the share's mount"),
    ("tools/check/", "the two-Mac tests: a scratch folder of the NAS"),
    ("tools/worldmap/", "the build page map's outlines (countries.json, lakes.json), made by hand from the NAS's Natural Earth copies, read only"),
    ("producers/", "the producers' scripts, run by hand: their sources' paths (D4: listed, not migrated)"),
];

/// What names a NAS location in Rust.
const LITERAL_MARKERS: &[&str] = &["/Volumes/", "fishandchips", "personal/projects/scenic-roads", "smb://"];

/// Every non-test Rust file of the workspace's crates, with its lines' code (`code_of`) and which
/// lines are test code.
fn rust_files() -> Vec<(String, Vec<String>, Vec<String>, Vec<bool>)> {
    let root = root();
    let mut rs = Vec::new();
    files(&root.join("crates"), "rs", &mut rs);
    let mut out = Vec::new();
    for f in rs {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if rel.contains("/tests/") || rel.ends_with("/tests.rs") || rel.ends_with("_tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let code: Vec<String> = lines.iter().map(|l| code_of(l)).collect();
        let tests = test_lines(&code);
        out.push((rel, lines, code, tests));
    }
    out
}

/// The items making `unchecked_path()` calls and how many: (file, item) → count.
fn escapes() -> BTreeMap<(String, String), usize> {
    let mut found = BTreeMap::new();
    for (rel, _, code, tests) in rust_files() {
        if rel == "crates/store/src/nasfs.rs" {
            continue;
        }
        for (i, c) in code.iter().enumerate() {
            let n = c.matches(".unchecked_path()").count();
            if tests[i] || n == 0 {
                continue;
            }
            let (name, _) = enclosing_rs(&code, i).unwrap_or_else(|| ("<top level>".into(), String::new()));
            *found.entry((rel.clone(), name)).or_default() += n;
        }
    }
    found
}

#[test]
fn every_escape_from_the_type_is_listed() {
    let found = escapes();
    let mut bad = Vec::new();
    for ((f, item), n) in &found {
        match EXEMPT.iter().find(|(ef, ei, _, _)| ef == f && ei == item) {
            Some((_, _, want, _)) if want == n => {}
            Some((_, _, want, _)) => bad.push(format!("{f}: {item}: {n} unchecked_path() calls, EXEMPT says {want}")),
            None => bad.push(format!("{f}: {item}: {n} unchecked_path() calls, not in EXEMPT")),
        }
    }
    for (f, item, n, _) in EXEMPT {
        if !found.contains_key(&(f.to_string(), item.to_string())) {
            bad.push(format!("{f}: {item}: in EXEMPT ({n}) but makes none (remove it)"));
        }
    }
    assert!(
        bad.is_empty(),
        "raw paths to the NAS outside store::nasfs: go through a NasPath method, or list the item in EXEMPT with why:\n{}",
        bad.join("\n")
    );
}

#[test]
fn roots_are_made_from_arguments_only_where_listed() {
    let mut found = std::collections::BTreeSet::new();
    for (rel, _, code, tests) in rust_files() {
        if rel == "crates/store/src/nasfs.rs" {
            continue;
        }
        for (i, c) in code.iter().enumerate() {
            if tests[i] || !c.contains("NasRoot::from_arg") {
                continue;
            }
            let (name, _) = enclosing_rs(&code, i).unwrap_or_else(|| ("<top level>".into(), String::new()));
            found.insert((rel.clone(), name));
        }
    }
    let listed: std::collections::BTreeSet<(String, String)> =
        FROM_ARG.iter().map(|(f, i)| (f.to_string(), i.to_string())).collect();
    let new: Vec<String> = found.difference(&listed).map(|(f, i)| format!("(\"{f}\", \"{i}\"),")).collect();
    let stale: Vec<String> = listed.difference(&found).map(|(f, i)| format!("{f}: {i}")).collect();
    assert!(
        new.is_empty() && stale.is_empty(),
        "NasRoot::from_arg outside FROM_ARG:\n{}\nlisted, making none:\n{}",
        new.join("\n"),
        stale.join("\n")
    );
}

#[test]
fn nas_locations_are_named_only_where_listed() {
    let mut found = std::collections::BTreeSet::new();
    for (rel, lines, code, tests) in rust_files() {
        if rel == "crates/store/src/nas.rs" {
            continue;
        }
        for (i, l) in lines.iter().enumerate() {
            // (In code or a string, not in a comment: `smb://` isn't one.)
            let c = without_comment(l);
            if tests[i] || !LITERAL_MARKERS.iter().any(|m| c.contains(m)) {
                continue;
            }
            let (name, _) = enclosing_rs(&code, i).unwrap_or_else(|| ("<top level>".into(), String::new()));
            found.insert((rel.clone(), name));
        }
    }
    let listed: std::collections::BTreeSet<(String, String)> =
        LITERALS.iter().map(|(f, i, _)| (f.to_string(), i.to_string())).collect();
    let new: Vec<String> = found.difference(&listed).map(|(f, i)| format!("(\"{f}\", \"{i}\", \"\"),")).collect();
    let stale: Vec<String> = listed.difference(&found).map(|(f, i)| format!("{f}: {i}")).collect();
    assert!(
        new.is_empty() && stale.is_empty(),
        "NAS locations named outside store::nas and LITERALS:\n{}\nlisted, naming none:\n{}",
        new.join("\n"),
        stale.join("\n")
    );
}

#[test]
fn scripts_name_the_nas_only_where_listed() {
    let root = root();
    let mut found = std::collections::BTreeSet::new();
    for dir in ["tools", "dem", "producers"] {
        for ext in ["sh", "zsh", "py", "mjs", "js", "swift", "c", "toml", "plist"] {
            let mut fs = Vec::new();
            files(&root.join(dir), ext, &mut fs);
            for f in fs {
                let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                let Ok(text) = std::fs::read_to_string(&f) else { continue };
                if TOOLS_MARKERS.iter().any(|m| text.contains(m)) {
                    found.insert(rel);
                }
            }
        }
    }
    let mut bad: Vec<String> = found
        .iter()
        .filter(|f| !TOOLS.iter().any(|(p, _)| f.starts_with(p)))
        .map(|f| format!("(\"{f}\", \"\"),"))
        .collect();
    bad.extend(
        TOOLS
            .iter()
            .filter(|(p, _)| !found.iter().any(|f| f.starts_with(p)))
            .map(|(p, _)| format!("{p}: listed, names none")),
    );
    assert!(bad.is_empty(), "scripts naming the NAS outside TOOLS:\n{}", bad.join("\n"));
}

/// `l` up to its line comment: the first `//` not part of a URL's `://`.
fn without_comment(l: &str) -> &str {
    let mut at = 0;
    while let Some(i) = l[at..].find("//") {
        let i = at + i;
        if !l[..i].ends_with(':') {
            return &l[..i];
        }
        at = i + 2;
    }
    l
}

/// A URL's `//` isn't a comment: a share named by its URL is found.
#[test]
fn a_url_is_not_a_comment() {
    assert_eq!(without_comment(r#"let u = "smb://fishandchips/x"; // y"#), r#"let u = "smb://fishandchips/x"; "#);
    assert!(LITERAL_MARKERS.iter().any(|m| without_comment(r#"mount("smb://h/personal")"#).contains(m)));
    assert_eq!(without_comment("x // /Volumes/a"), "x ");
}

/// The scan's own rule: a call in a string or a comment isn't one; one in a test item isn't
/// counted.
#[test]
fn the_scan_counts_calls_in_code_alone() {
    let src = "fn a(p: &NasPath) {\n    let _ = p.unchecked_path(); // .unchecked_path()\n    let s = \".unchecked_path()\";\n}\n#[cfg(test)]\nfn t(p: &NasPath) { p.unchecked_path(); }";
    let code: Vec<String> = src.lines().map(code_of).collect();
    let tests = test_lines(&code);
    let n: usize =
        code.iter().zip(&tests).filter(|(_, t)| !**t).map(|(c, _)| c.matches(".unchecked_path()").count()).sum();
    assert_eq!(n, 1);
}
