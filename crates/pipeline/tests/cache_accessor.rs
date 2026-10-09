//! The build agent's caches are read only through their accessor (store::cachefile;
//! dem/cachefile.py): a guard for docs/plan.md §8's room-making mid-job. Every function outside
//! tests that names a cache path (`MARKERS`) uses the accessor, or is listed in `EXEMPT` with why
//! it may not: it builds a job's arguments, counts, or the path isn't the agent's cache at all.
//! A new reader of the caches fails here until it's routed through the accessor (or, if it truly
//! reads no cache file, listed with its reason).

use std::path::{Path, PathBuf};

/// What names a cache path in Rust: the caches' folders and files, and the variables that hold
/// them.
const MARKERS: &[&str] = &[
    "\"chm10",
    "join(\"aws-terrarium\")",
    "\"dem-cache.",
    "dem-cache.{",
    "\"heritage-merged-",
    "room::MONTHS",
    "items/months",
    "join(\"blobs\")",
    "join(\"base\")",
    "chm.join(",
    "tools.cache",
    "cache.join(",
    "self.dir.join(\"packs\")",
];

/// What names a cache path in the Python steps.
const PY_MARKERS: &[&str] = &["OUT / \"months\""];

/// Functions that name a cache path without reading a cache file, by file (from the repository's
/// root) and name, with why.
const EXEMPT: &[(&str, &str, &str)] = &[
    // The room-maker itself: its deletions are store::cachefile's; the rest counts sizes.
    ("crates/pipeline/src/agent/room.rs", "*", "the room-maker: deletes through store::cachefile, counts sizes"),
    ("crates/pipeline/src/agent/mod.rs", "step_args", "a job's arguments: where its caches are"),
    ("crates/pipeline/src/agent/mod.rs", "need_of", "counts the pack cache's bytes"),
    ("crates/pipeline/src/agent/mod.rs", "try_start_said", "room-making's arguments"),
    ("crates/pipeline/src/agent/mod.rs", "tend_caches", "room-making's arguments"),
    ("crates/pipeline/src/agent/mod.rs", "resources", "counts sizes"),
    ("crates/pipeline/src/agent/mod.rs", "helper_job", "counts what room-making can free"),
    ("crates/pipeline/src/agent/mod.rs", "count_caches", "counts sizes"),
    ("crates/pipeline/src/agent/backup.rs", "*", "the NAS's backups' own blobs/, not the agent's cache"),
    ("crates/pipeline/src/agent/gc.rs", "*", "the NAS's base/ folder, not the agent's cache"),
    ("crates/pipeline/src/scache.rs", "*", "a unit folder's own scenic cache (its scache/), not the agent's"),
    ("crates/pipeline/src/dem/mod.rs", "*", "elev's slice of the DEM cache in the unit's own folder, not the agent's"),
    ("crates/pipeline/src/bin/terrain.rs", "*", "the terrain scan tool's own cache, not the agent's"),
    ("crates/pipeline/src/answers.rs", "*", "the items and heritage answers, which room-making never deletes"),
    ("crates/pipeline/src/unit.rs", "dem_units", "the units' kept samples (dem-units/), which room-making never deletes"),
    ("crates/pipeline/src/unit.rs", "scenic_kept", "the units' kept results (scenic-units/), which room-making never deletes"),
    ("crates/pipeline/src/unit.rs", "move_kept_to_shared", "the units' kept results, which room-making never deletes"),
    ("crates/pipeline/src/unit.rs", "dem_samples_keep", "the units' kept samples, which room-making never deletes"),
    ("crates/pipeline/src/unit.rs", "dem_samples_keep_with", "reads elev's dem-cache.* in the unit's own folder, not the agent's cache; its copy kept by keep_dem_copy (store::cachefile)"),
    ("crates/pipeline/src/unit.rs", "dem_copies", "a path: keep_dem_copy writes the copies through store::cachefile, dem_cache_slice_with holds each it reads"),
    ("crates/pipeline/src/unit.rs", "base_packs", "a path: keep_for_packs writes there through store::cachefile, open_units reads it"),
    ("crates/pipeline/src/buildtiles.rs", "tile_copy", "a path: fetch_tile holds each copy (store::cachefile) before the buildings step reads it"),
    ("crates/pipeline/src/unit.rs", "dem_cache_slice", "reads the seed dem_seed holds just before (build_folder)"),
    ("crates/pipeline/src/unit.rs", "tail", "a task's arguments: where its caches are"),
    ("crates/pipeline/src/unit.rs", "run_tail", "a task's arguments: where its caches are"),
    ("crates/pipeline/src/unit.rs", "prepare_folder", "the unit's steps' arguments; the seed held by dem_seed just before"),
    ("crates/pipeline/src/trees/tests.rs", "*", "tests: their own caches"),
    ("crates/pipeline/src/rawpack.rs", "put", "the packer's own new archive, which it holds (publish)"),
    ("crates/pipeline/src/rawpack.rs", "put_up", "deletes through drop_copy (store::cachefile)"),
    ("crates/pipeline/src/rawpack.rs", "merge_due", "deletes through drop_copy (store::cachefile)"),
    ("crates/pipeline/src/rawpack.rs", "commit", "deletes through drop_copy (store::cachefile)"),
    ("crates/pipeline/src/bin/scenic-build.rs", "step_main", "the steps' arguments: where their caches are"),
    ("crates/pipeline/src/bin/scenic-build.rs", "registers_extract", "the registers' snapshot, which room-making never deletes"),
    ("crates/pipeline/src/agent/mod.rs", "plan", "counts the pack cache's bytes"),
    ("crates/pipeline/src/agent/mod.rs", "region_work", "a job's arguments: where its caches are"),
    ("crates/pipeline/src/treepacks.rs", "*", "a test bench's own squares folder"),
    ("crates/pipeline/src/bin/scenic-metrics.rs", "canopy", "chooses the canopy folder; fetch_file holds each square"),
    ("crates/pipeline/src/bin/scenic-build.rs", "unit_snap", "the unit's tools' arguments"),
    ("crates/pipeline/src/bin/scenic-build.rs", "unit_step", "the unit's tools' arguments; the squares and packs held as they're copied ahead"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rail_step", "the rail job's own stop pairs (pairs-*.bin), which room-making never deletes"),
    ("crates/pipeline/src/bin/scenic-build.rs", "heritage_epoch", "the heritage epoch's folder, which room-making never deletes"),
    ("crates/pipeline/src/bin/scenic-build.rs", "peaks_step", "the peaks job's arguments: RawTiles holds what it reads"),
    ("crates/pipeline/src/bin/scenic-build.rs", "terrain_step", "the terrain job's arguments: RawTiles holds what it reads"),
    ("crates/pipeline/src/bin/scenic-build.rs", "terrain_pieces", "terrain's pieces' arguments: RawTiles holds what it reads"),
    ("crates/pipeline/src/bin/scenic-build.rs", "terrain_lo_step", "terrain's assemblies' arguments: RawTiles holds what it reads"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if !matches!(p.file_name().and_then(|n| n.to_str()), Some("target" | "tests" | ".venv" | "node_modules")) {
                files(&p, ext, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// The name of the function a line `i` of `lines` is in (the last `fn <name>` / `def <name>` above
/// it), and that function's text (to the next one at its indentation or less).
fn enclosing(lines: &[&str], i: usize, kw: &str) -> Option<(String, String)> {
    let starts = |l: &str| {
        let t = l.trim_start();
        let t = t.strip_prefix("pub(crate) ").or_else(|| t.strip_prefix("pub(super) ")).or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        t.strip_prefix(kw).map(|r| r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string())
    };
    let indent = |l: &str| l.len() - l.trim_start().len();
    let (at, name) = (0..=i).rev().find_map(|k| starts(lines[k]).map(|n| (k, n)))?;
    let end = (at + 1..lines.len()).find(|&k| starts(lines[k]).is_some() && indent(lines[k]) <= indent(lines[at]) && !lines[k].trim().is_empty()).unwrap_or(lines.len());
    Some((name, lines[at..end].join("\n")))
}

#[test]
fn every_cache_read_goes_through_the_accessor() {
    let root = root();
    let mut bad = Vec::new();
    let mut rs = Vec::new();
    files(&root.join("crates"), "rs", &mut rs);
    for f in rs {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if rel.starts_with("crates/store/src/cachefile.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // (Tests aside: they make their own caches.)
        let end = (1..lines.len()).find(|&k| lines[k].trim_start().starts_with("mod tests") && lines[k - 1].contains("cfg(test)")).unwrap_or(lines.len());
        for i in 0..end {
            let l = lines[i];
            if l.trim_start().starts_with("//") || !MARKERS.iter().any(|m| l.contains(m)) {
                continue;
            }
            let Some((name, body)) = enclosing(&lines, i, "fn ") else { continue };
            if body.contains("cachefile::") || EXEMPT.iter().any(|(ef, en, _)| *ef == rel && (*en == "*" || *en == name)) {
                continue;
            }
            bad.push(format!("{rel}:{} in fn {name}: {}", i + 1, l.trim()));
        }
    }
    let mut py = Vec::new();
    files(&root.join("dem"), "py", &mut py);
    for f in py {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if l.trim_start().starts_with('#') || !PY_MARKERS.iter().any(|m| l.contains(m)) {
                continue;
            }
            let Some((name, body)) = enclosing(&lines, i, "def ") else { continue };
            // (The counts from before the indexes, {month}.json, which room-making never deletes;
            // the progress line's estimate of the bytes to move, which relies on nothing it reads.)
            if body.contains("cachefile.") || name == "_cached" || name == "_plan" {
                continue;
            }
            bad.push(format!("{rel}:{} in def {name}: {}", i + 1, l.trim()));
        }
    }
    assert!(bad.is_empty(), "cache paths named outside the accessor (store::cachefile, dem/cachefile.py); route them through it, or list the function in EXEMPT with why:\n{}", bad.join("\n"));
}

/// Every exemption still names a function that's there (a stale one would let a new reader by).
#[test]
fn every_exemption_names_a_function_there() {
    let root = root();
    for (f, name, _) in EXEMPT {
        let text = std::fs::read_to_string(root.join(f)).unwrap_or_else(|_| panic!("{f} isn't there"));
        assert!(*name == "*" || text.contains(&format!("fn {name}(")) || text.contains(&format!("fn {name}<")), "{f}: no fn {name}");
    }
}

/// The Python steps' side, under the same chaos (tools/check/pageviews_chaos.py): pageviews.py's
/// lookups, while another process deletes every cache file no job holds as fast as it can, give
/// what the first did. (Passed over, said, where python3 has no `compression.zstd`: before 3.14.)
#[test]
fn the_python_steps_hold_what_they_read() {
    let ok = std::process::Command::new("python3").args(["-I", "-c", "from compression import zstd"]).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("python3 with compression.zstd isn't here: pageviews_chaos.py not run");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    // (No network, as the script makes sure for itself too: SCENIC_FETCH_OFFLINE, and a proxy that
    // refuses for what it runs.)
    let out = std::process::Command::new("python3").env("SCENIC_FETCH_OFFLINE", "1").env("ALL_PROXY", "http://127.0.0.1:9").env("HTTPS_PROXY", "http://127.0.0.1:9").env("HTTP_PROXY", "http://127.0.0.1:9").arg("-I").arg(root().join("tools/check/pageviews_chaos.py")).arg(d.path()).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("40 lookups alike") && !err.contains("\"deleted\": 0"), "{err}");
}

/// Every request to the internet asks `pipeline::fetch::online` first, so a check run on scratch
/// data with SCENIC_FETCH_OFFLINE=1 (or a test's `go_offline`) can't download, whatever path it
/// takes: each function making a request (`.call()`) asks, or is listed with why it needn't.
#[test]
fn every_internet_request_asks_first() {
    const ASKED_ELSEWHERE: &[(&str, &str, &str)] = &[
        ("crates/pipeline/src/coord/client.rs", "*", "the build Mac's coordinator, on the LAN"),
        ("crates/pipeline/src/bin/scenic-metrics.rs", "meta_get", "fetch_file asks before its first"),
    ];
    let root = root();
    let mut rs = Vec::new();
    files(&root.join("crates"), "rs", &mut rs);
    let mut bad = Vec::new();
    for f in rs {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let end = (1..lines.len()).find(|&k| lines[k].trim_start().starts_with("mod tests") && lines[k - 1].contains("cfg(test)")).unwrap_or(lines.len());
        for i in 0..end {
            if !lines[i].contains(".call()") || lines[i].trim_start().starts_with("//") {
                continue;
            }
            let Some((name, body)) = enclosing(&lines, i, "fn ") else { continue };
            if body.contains("online(") || ASKED_ELSEWHERE.iter().any(|(ef, en, _)| *ef == rel && (*en == "*" || *en == name)) {
                continue;
            }
            bad.push(format!("{rel}:{} in fn {name}", i + 1));
        }
    }
    assert!(bad.is_empty(), "requests that don't ask pipeline::fetch::online first:\n{}", bad.join("\n"));
}
