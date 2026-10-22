//! Every rule that grants a job lives in the scheduler (crate::coord::sched, docs/pool.md §7.2):
//! the live coordinator and the forecast both call `Sched::pack`, so neither can keep a second copy
//! that drifts. Outside `#[cfg(test)]` code, no line of the crate's source names a grant rule
//! (`RULES`) but in the scheduler itself and where the budget's limits are defined
//! (crate::budget::nas), or as listed in `EXEMPT` with why; an exemption that no longer matches
//! fails too, so the list can't go stale.

use std::path::{Path, PathBuf};

/// What names a grant rule: the NAS budget's fits, the packing's cores and provisional need, the
/// steps kept to the lead, the scheduler's picks, and the forecast's old copies.
const RULES: &[&str] = &[
    "Limits::fits(",
    "fits_job(",
    "fits_own(",
    "fits_own_job(",
    "fits_quiet(",
    "provisional_mb(",
    "packed_share(",
    "outside_cores(",
    "ask_cores(",
    "every::grants(",
    "pick_every(",
    "pick_own(",
    "cold_fits(",
    "OLD_APP_GAP_S",
    "room_ok",
];

/// Where they may be named: the scheduler, and the budget's limits where they're defined.
const HOME: &[&str] = &["src/coord/sched.rs", "src/budget/nas.rs"];

/// Lines naming one elsewhere, by file and what the line says, with why.
const EXEMPT: &[(&str, &str, &str)] = &[
    (
        "src/coord/mod.rs",
        "s.leases.set_threads(lease, ask_cores(&a));",
        "a page's or worker's task (not a job): its threads, the Mac's cores",
    ),
    ("src/coord/mod.rs", "threads: ask_cores(&a),", "a task's grant (not a job): its threads, the Mac's cores"),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// A file's code outside its tests, comments dropped.
fn code(text: &str) -> Vec<&str> {
    // (Its tests' module: `mod tests` under a `cfg` naming `test`.)
    let end = text
        .match_indices("\nmod tests")
        .map(|(i, _)| i)
        .find(|&i| text[..i].lines().last().is_some_and(|l| l.starts_with("#[cfg(") && l.contains("test")))
        .unwrap_or(text.len());
    text[..end].lines().filter(|l| !l.trim_start().starts_with("//")).collect()
}

#[test]
fn no_grant_rule_lives_outside_the_scheduler() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    let mut found = Vec::new();
    let mut used = vec![false; EXEMPT.len()];
    for f in files {
        let rel = f.strip_prefix(root).unwrap().to_string_lossy().to_string();
        if HOME.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for l in code(&text) {
            if !RULES.iter().any(|r| l.contains(r)) {
                continue;
            }
            match EXEMPT.iter().position(|(file, line, _)| *file == rel && l.contains(line)) {
                Some(k) => used[k] = true,
                None => found.push(format!("{rel}: {}", l.trim())),
            }
        }
    }
    assert!(found.is_empty(), "grant rules outside crate::coord::sched:\n{}", found.join("\n"));
    let stale: Vec<_> = EXEMPT.iter().zip(&used).filter(|(_, u)| !**u).map(|(e, _)| e.1).collect();
    assert!(stale.is_empty(), "exemptions that match nothing now: {stale:?}");
}
