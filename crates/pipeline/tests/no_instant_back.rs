//! No code in the workspace dates an `Instant` back from now (now minus a duration): it panics on a
//! Mac booted less than `d` ago, so a test doing it depends on the machine. A limit the code reads
//! is made injectable instead (the agent's `grace` and `pass_max`, say), and a test sets it.

use std::path::Path;

fn scan(dir: &Path, found: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target" && n != ".git") {
                scan(&p, found);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            let text = std::fs::read_to_string(&p).unwrap();
            // (Spelled apart, so this file doesn't find itself.)
            let now = ["Instant::", "now()"].concat();
            for (i, l) in text.lines().enumerate() {
                if let Some(at) = l.find(&now) {
                    if l[at + now.len()..].trim_start().starts_with('-') {
                        found.push(format!("{}:{}: {}", p.display(), i + 1, l.trim()));
                    }
                }
            }
        }
    }
}

#[test]
fn no_instant_is_dated_back_from_now() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = Vec::new();
    scan(crates, &mut found);
    assert!(
        found.is_empty(),
        "an Instant dated back from now (panics on a Mac booted lately; make the limit injectable):\n{}",
        found.join("\n")
    );
}
