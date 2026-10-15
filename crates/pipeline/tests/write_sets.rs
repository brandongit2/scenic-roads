//! Nothing writes the build's records but through `Out` (docs/pool.md §7.3, §7.7, §12 phase 4's
//! sixth batch): a step's job changes the manifest only through `pipeline::out::Out`, which puts
//! and removes only its step's outputs (crate::agent::steps::writes, the function the lead's merge
//! checks every entry by). So every item outside `#[cfg(test)]` code that makes or writes a
//! hand-off (`handoff::write(`, a `Handoff { … }` or `Handoff::default()` made) or a journal entry
//! (`journal::write(`), or names the manifest's file or the journal's folder by path, is `Out`
//! itself, or is listed in `EXEMPT` by file, item and what the line says, with why (a reader's
//! too, so each new line naming them is looked at): the merge's own paths (the coordinator journaling a helper's
//! hand-off, the driver writing a job's entry, the lead writing today's files from its records),
//! and the writers of what lies outside the records (the raw tiles' archives a hand-off names). A
//! new writer fails here until it goes through `Out` or is listed; an exemption that no longer
//! matches a line fails too, so the list can't go stale. Run by tools/app/publish.sh: it gates
//! publish. It's best-effort, a scan of the source's text: a writer that names none of the markers
//! (a path put together in pieces, a hand-off made by deserializing) passes it.

mod common;

use common::*;

/// What writes or makes a hand-off or a journal entry, in a line's code (its strings blanked).
const MARKERS: &[&str] = &["handoff::write(", "journal::write(", "Handoff {", "Handoff::default("];

/// The records' paths, named in a line (its strings too): the manifest's file and the journal's
/// folder. Their readers are listed too, so a new line naming them is looked at.
const PATHS: &[&str] = &["state/build/manifest.json", "state/journal"];

/// Whether line `l` (raw, its comment cut) with code `c` makes or writes a hand-off or an entry,
/// or names the manifest's file or the journal's folder.
fn writes_records(l: &str, c: &str) -> bool {
    let made = MARKERS.iter().any(|m| c.contains(m)) && !c.contains("struct Handoff {") && !c.contains("impl Handoff {");
    made || PATHS.iter().any(|p| l.contains(p))
}

/// The writers outside `Out`: by file (from the repository's root), item name and what the line
/// says (an anchor), with why.
const EXEMPT: &[(&str, &str, &str, &str)] = &[
    ("crates/pipeline/src/coord/mod.rs", "raw_again", "crate::handoff::write(&journal.join(crate::handoff::RAW_AGAIN)", "a hand-off not taken: its raw tiles' archives alone, kept for the next merge to name (outside the manifest)"),
    ("crates/pipeline/src/coord/mod.rs", "route", "crate::handoff::write(&journal.join(folder(&d.worker)), &h)", "the coordinator journaling a helper's hand-off (its job's Out wrote it) while its pool is off: the merge's path"),
    ("crates/pipeline/src/handoff.rs", "merge_from", "write(&b.join(RAW_AGAIN), &Handoff { raw: again", "the merge keeping the raw tiles' archives it couldn't name yet (outside the manifest)"),
    ("crates/pipeline/src/rawpack.rs", "finish_with", "crate::handoff::write(&dir, &crate::handoff::Handoff { raw:", "a job's raw tiles' archives, named by their area (crate::rawpack::named_for, which the merge checks), outside the manifest"),
    ("crates/pipeline/src/agent/pool.rs", "write_today", "nas.write_whole(\"state/build/manifest.json\"", "the lead writing today's files from its merged records, for their readers"),
    ("crates/pipeline/src/agent/shadow.rs", "own_entry", "let handoff = Handoff { changes,", "the shadow run's entry of a job of the build Mac's, from the files its Out saved, into the shadow's own state (crate::agent::shadow)"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rekey_check", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): a check run by hand"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check_terrain", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): a check run by hand"),
    ("crates/pipeline/src/bin/scenic-build.rs", "water_key_check", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): a check run by hand (its copy in memory, never written)"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): a check run by hand"),
    ("crates/pipeline/src/bin/scenic.rs", "inputs", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): `scenic inputs`'s report"),
    ("crates/pipeline/src/agent/gc.rs", "run", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): GC's live set"),
    ("crates/pipeline/src/agent/mod.rs", "region_jobs", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the plan"),
    ("crates/pipeline/src/agent/mod.rs", "checklist", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the status's checklist"),
    ("crates/pipeline/src/agent/mod.rs", "region_progress", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the status's progress"),
    ("crates/pipeline/src/agent/mod.rs", "rekey_records", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): re-keying with the pool off (its writes through Out)"),
    ("crates/pipeline/src/agent/mod.rs", "as_read", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the plan's keys"),
    ("crates/pipeline/src/agent/mod.rs", "tend_gate", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the gate's plan"),
    ("crates/pipeline/src/inputs/served.rs", "RECORDS", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the map server's copy of the translations' and descriptions' accepted versions"),
    ("crates/pipeline/src/agent/room.rs", "heritage_remade", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): room-making's look at the heritage chain"),
    ("crates/pipeline/src/agent/shadow.rs", "compare", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the shadow run's comparison"),
    ("crates/pipeline/src/agent/shadow.rs", "files_of", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the shadow run's look at a job's files"),
    ("crates/pipeline/src/agent/pool.rs", "READ_THROUGH", "\"state/build/manifest.json\"", "today's files, read past the share's cache"),
    ("crates/pipeline/src/pool/records.rs", "TODAY", "\"state/build/manifest.json\"", "today's files, term 1's first snapshot made from them"),
    ("crates/pipeline/src/agent/pool.rs", "POOL_FILES", "\"state/journal\"", "the pool's files `scenic pool off` moves aside"),
    ("crates/pipeline/src/pool/journal.rs", "DIR", "\"state/journal\"", "the journal's own module: its folder"),
    ("crates/pipeline/src/pool/journal.rs", "REJECTED", "\"state/journal/rejected\"", "the journal's own module: the refusals' notes"),
    ("crates/pipeline/src/agent/mod.rs", "send_outbox", "Handoff::default()", "a helper's outbox job (pool off): the hand-offs its job's Out wrote, gathered for the coordinator"),
    ("crates/pipeline/src/agent/pool.rs", "entry_of", "Handoff::default()", "a pooled job's entry: the hand-offs its job's Out wrote, gathered for the journal"),
    ("crates/pipeline/src/osmpass.rs", "latest_pass_by", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): a pass complete by the records (docs/pool.md §7.4)"),
    ("crates/pipeline/src/agent/resume.rs", "unmerged", "-> Handoff {", "its signature: the member's unacknowledged entries laid together for its job's Out to read (never handed off)"),
    ("crates/pipeline/src/agent/mod.rs", "unmerged_for", "-> crate::handoff::Handoff {", "its signature: a resumable job's member's unacknowledged entries, for its Out to read over the snapshot (never handed off)"),
    ("crates/pipeline/src/agent/resume.rs", "unmerged", "Handoff::default()", "a resumable job's member's unacknowledged entries laid together, for its Out to read over the snapshot (never handed off: its own saves are)"),
    ("crates/pipeline/src/agent/shadow.rs", "look_outbox", "Handoff::default()", "the shadow run's look at a helper's outbox: the hand-offs its Out wrote"),
];

/// The lines writing the records outside `Out`, and the exemptions that matched none.
fn scan() -> (Vec<String>, Vec<String>) {
    let root = root();
    let mut bad = Vec::new();
    let mut used = std::collections::BTreeSet::<usize>::new();
    let mut rs = Vec::new();
    files(&root.join("crates"), "rs", &mut rs);
    for f in rs {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        // (The writer itself; and test code in files of its own, `#[cfg(test)] mod sim` or `tests`.)
        if rel == "crates/pipeline/src/out.rs" || rel == "crates/pipeline/src/pool/sim.rs" || rel.ends_with("/tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let code: Vec<String> = lines.iter().map(|l| code_of(l)).collect();
        let tests = test_lines(&code);
        for i in 0..lines.len() {
            let l = lines[i].split("//").next().unwrap_or("");
            if tests[i] || !writes_records(l, &code[i]) {
                continue;
            }
            let name = enclosing_rs(&code, i).map_or_else(|| "<top level>".to_string(), |e| e.0);
            if let Some(k) = EXEMPT.iter().position(|(ef, en, anchor, _)| *ef == rel && *en == name && l.contains(anchor)) {
                used.insert(k);
                continue;
            }
            bad.push(format!("{rel}:{} in {name}: {}", i + 1, l.trim()));
        }
    }
    let unused = EXEMPT.iter().enumerate().filter(|(k, _)| !used.contains(k)).map(|(_, (f, n, a, _))| format!("{f}: {n}: {a}")).collect();
    (bad, unused)
}

#[test]
fn nothing_writes_the_records_but_through_out() {
    let (bad, _) = scan();
    assert!(bad.is_empty(), "the records written outside pipeline::out::Out (a hand-off, a journal entry, the manifest's file); write through Out, or list the item in EXEMPT with why:\n{}", bad.join("\n"));
}

/// Every exemption still matches a line it exempts.
#[test]
fn every_exemption_matches_a_line() {
    let (_, unused) = scan();
    assert!(unused.is_empty(), "exemptions matching no line that writes the records (remove them):\n{}", unused.join("\n"));
}

/// The markers: a hand-off made or written, an entry written, the manifest's file written; not the
/// type's own definition, a read of the manifest, or a marker in a string.
#[test]
fn the_markers() {
    let w = |l: &str| writes_records(l, &code_of(l));
    assert!(w("crate::handoff::write(&dir, &h)?;") && w("let h = Handoff { raw, ..Default::default() };") && w("journal::write(&nas, &e)?;"));
    assert!(w("std::fs::write(root.join(\"state/build/manifest.json\"), b)?;") && w("nas.write_whole(\"state/build/manifest.json\", &b)?;"));
    assert!(w("let h = Handoff::default();") && w("let m = read_record(&root.join(\"state/build/manifest.json\"))?;") && w("const J: &str = \"state/journal\";"));
    assert!(!w("pub struct Handoff {") && !w("impl Handoff {") && !w("let m = read_record(&root.join(\"state/build/jobs.json\"))?;"));
    assert!(!w("eprintln!(\"handoff::write( failed\");"));
}
