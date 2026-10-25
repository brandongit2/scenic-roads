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
const PATHS: &[&str] = &["state/build/manifest.json", "state/build/sizes.json", "state/journal"];

/// Whether line `l` (raw, its comment cut) with code `c` makes or writes a hand-off or an entry,
/// or names the manifest's file or the journal's folder.
fn writes_records(l: &str, c: &str) -> bool {
    let made =
        MARKERS.iter().any(|m| c.contains(m)) && !c.contains("struct Handoff {") && !c.contains("impl Handoff {");
    made || PATHS.iter().any(|p| l.contains(p))
}

/// The writers outside `Out`: by file (from the repository's root), item name and what the line
/// says (an anchor), with why.
const EXEMPT: &[(&str, &str, &str, &str)] = &[
    ("crates/pipeline/src/coord/mod.rs", "raw_again", "crate::handoff::write(&journal.join(crate::handoff::RAW_AGAIN)", "a hand-off not taken: its raw tiles' archives alone, kept for the next merge to name (outside the manifest)"),
    ("crates/pipeline/src/coord/mod.rs", "route", "crate::handoff::write(&journal.join(folder(&d.worker)), &h)", "the coordinator journaling a helper's hand-off (its job's Out wrote it) while its pool is off: the merge's path"),
    ("crates/pipeline/src/rawpack.rs", "finish_with", "^crate::handoff::write(&dir,&crate::handoff::Handoff { raw:", "a job's raw tiles' archives, named by their area (crate::rawpack::named_for, which the merge checks), outside the manifest"),
    ("crates/pipeline/src/agent/pool.rs", "write_today", "nas.write_whole(\"state/build/manifest.json\"", "the lead writing today's files from its merged records, for their readers"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): a check run by hand"),
    ("crates/pipeline/src/agent/room.rs", "retire", "\"state/build/manifest.json\"", "reads the manifest (today's file): the cache sweep keeps every copy it names"),
    ("crates/pipeline/src/bin/scenic.rs", "inputs", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): `scenic inputs`'s report"),
    ("crates/pipeline/src/agent/mod.rs", "plan_seen", "\"state/build/manifest.json\"", "reads only the manifest's and the jobs' records' size and time (an idle agent's plan digest, task #154)"),
    ("crates/pipeline/src/agent/gc.rs", "run_once", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): GC's live set"),
    ("crates/pipeline/src/agent/mod.rs", "region_jobs", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the plan"),
    ("crates/pipeline/src/agent/mod.rs", "checklist", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the status's checklist"),
    ("crates/pipeline/examples/checklist.rs", "main", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the checklist worked out by hand, read only"),
    ("crates/pipeline/src/agent/mod.rs", "as_built_parts", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the status's progress and the regions' steps"),
    ("crates/pipeline/src/agent/mod.rs", "as_read", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the plan's keys"),
    ("crates/pipeline/src/agent/mod.rs", "input_digests", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the elevation pool's digest it names"),
    ("crates/pipeline/src/agent/mod.rs", "tend_gate", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the gate's plan"),
    ("crates/pipeline/src/agent/pool.rs", "regions_app", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the app the newer readers' switch (the regions', the elevation datasets') was recorded on, for the pool's views and handovers"),
    ("crates/pipeline/src/bin/scenic-build.rs", "step_main", "\"state/build/manifest.json\"", "regions-fixture: a scratch folder's records for a two-Mac procedure, never the project folder's"),
    ("crates/pipeline/src/heritage.rs", "registers_now", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the heritage registers' accepted version the agent's plan keys heritage-sites on"),
    ("crates/pipeline/src/inputs/regions/read.rs", "source_now", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the regions' accepted version the readers read"),
    ("crates/pipeline/src/inputs/sources.rs", "open_now", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the source declarations' accepted version the agent's plan keys the 3D buildings on"),
    ("crates/pipeline/src/inputs/served.rs", "RECORDS", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): the map server's copy of the translations' and descriptions' accepted versions"),
    ("crates/pipeline/src/agent/room.rs", "heritage_remade", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): room-making's look at the heritage chain"),
    ("crates/pipeline/src/pool/records.rs", "TODAY", "\"state/build/manifest.json\"", "today's files, term 1's first snapshot made from them"),
    ("crates/pipeline/src/pool/journal.rs", "DIR", "\"state/journal\"", "the journal's own module: its folder"),
    ("crates/pipeline/src/pool/journal.rs", "REJECTED", "\"state/journal/rejected\"", "the journal's own module: the refusals' notes"),
    ("crates/pipeline/src/pool/journal_gc.rs", "PATH", "\"state/journal/gc.json\"", "the journal's GC's own fence marker (written by the lead alone, through its NAS): the entries to remove, never a record or a step's output"),
    ("crates/pipeline/src/agent/pool.rs", "entry_of", "Handoff::default()", "a pooled job's entry: the hand-offs its job's Out wrote, gathered for the journal"),
    ("crates/pipeline/src/osmpass.rs", "latest_pass_by", "\"state/build/manifest.json\"", "reads the manifest (today's file; the lead writes it from its records): a pass complete by the records (docs/pool.md §7.4)"),
    ("crates/pipeline/src/agent/resume.rs", "unmerged", "-> Handoff {", "its signature: the member's unacknowledged entries laid together for its job's Out to read (never handed off)"),
    ("crates/pipeline/src/agent/resume.rs", "unmerged", "Handoff::default()", "a resumable job's member's unacknowledged entries laid together, for its Out to read over the snapshot (never handed off: its own saves are)"),
    ("crates/pipeline/src/inputs/elevation_tests.rs", "the_check_job_records_the_switch_once_with_the_first_accepted_index", "\"state/build/manifest.json\"), b\"{}\"", "a test's scratch NAS root: its empty records"),
    ("crates/pipeline/src/inputs/elevation_tests.rs", "the_check_job_records_the_switch_once_with_the_first_accepted_index", "read_record(", "a test reading back the records its check job wrote"),
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
        if rel == "crates/pipeline/src/out.rs" || rel == "crates/pipeline/src/pool/sim.rs" || rel.ends_with("/tests.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let code: Vec<String> = lines.iter().map(|l| code_of(l)).collect();
        let tests = test_lines(&code);
        let mut covered = 0; // (The lines an anchored exemption has taken in, up to this one.)
        for i in 0..lines.len() {
            let l = lines[i].split("//").next().unwrap_or("");
            if i < covered {
                continue;
            }
            if tests[i] || !writes_records(l, &code[i]) {
                continue;
            }
            let name = enclosing_rs(&code, i).map_or_else(|| "<top level>".to_string(), |e| e.0);
            if let Some((k, n)) = EXEMPT.iter().enumerate().find_map(|(k, (ef, en, anchor, _))| {
                (*ef == rel && *en == name).then(|| anchored(&lines, i, l, anchor)).flatten().map(|n| (k, n))
            }) {
                used.insert(k);
                covered = i + n;
                continue;
            }
            bad.push(format!("{rel}:{} in {name}: {}", i + 1, l.trim()));
        }
    }
    let unused = EXEMPT
        .iter()
        .enumerate()
        .filter(|(k, _)| !used.contains(k))
        .map(|(_, (f, n, a, _))| format!("{f}: {n}: {a}"))
        .collect();
    (bad, unused)
}

/// How many lines an exemption's anchor takes in at line `i` (`l` is the line cut of its comment), or
/// None if it doesn't hold: a plain anchor is a part of the line (one line); one starting with `^` is
/// how the item's text begins at the line, its next lines (trimmed) joined, so an exemption names a
/// statement as it was before rustfmt broke it across lines, not any line with the same words, and
/// the lines it spans are covered by it (and by nothing else).
fn anchored(lines: &[&str], i: usize, l: &str, anchor: &str) -> Option<usize> {
    match anchor.strip_prefix('^') {
        Some(rest) => (1..=(lines.len() - i).min(6))
            .find(|n| lines[i..i + n].iter().map(|l| l.trim()).collect::<String>().starts_with(rest)),
        None => l.contains(anchor).then_some(1),
    }
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
    assert!(
        unused.is_empty(),
        "exemptions matching no line that writes the records (remove them):\n{}",
        unused.join("\n")
    );
}

/// The markers: a hand-off made or written, an entry written, the manifest's file written; not the
/// type's own definition, a read of the manifest, or a marker in a string.
#[test]
fn the_markers() {
    let w = |l: &str| writes_records(l, &code_of(l));
    assert!(
        w("crate::handoff::write(&dir, &h)?;")
            && w("let h = Handoff { raw, ..Default::default() };")
            && w("journal::write(&nas, &e)?;")
    );
    assert!(
        w("std::fs::write(root.join(\"state/build/manifest.json\"), b)?;")
            && w("nas.write_whole(\"state/build/manifest.json\", &b)?;")
    );
    assert!(
        w("let h = Handoff::default();")
            && w("let m = read_record(&root.join(\"state/build/manifest.json\"))?;")
            && w("const J: &str = \"state/journal\";")
    );
    assert!(
        !w("pub struct Handoff {")
            && !w("impl Handoff {")
            && !w("let m = read_record(&root.join(\"state/build/jobs.json\"))?;")
    );
    assert!(!w("eprintln!(\"handoff::write( failed\");"));
}
