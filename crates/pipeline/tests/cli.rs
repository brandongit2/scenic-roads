//! Every program's command line (pipeline::cli), run as the built program: every binary of the
//! workspace has one; with `--help` (or `-h`) each command prints its usage and exits 0, with a
//! flag it doesn't take it's refused with its usage (exit 2), and neither does anything: its home
//! (HOME, the working folder) is left empty, no NAS or agent reached. And the calls of these
//! programs in the docs and in tools/ name commands and flags they take.

use clap::Command;
use pipeline::cli::{command, PROGRAMS};
use std::path::Path;

fn repo() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The built binary of each of this crate's programs.
fn exe(prog: &str) -> &'static str {
    match prog {
        "scenic" => env!("CARGO_BIN_EXE_scenic"),
        "scenic-build" => env!("CARGO_BIN_EXE_scenic-build"),
        "scenic-metrics" => env!("CARGO_BIN_EXE_scenic-metrics"),
        "extract" => env!("CARGO_BIN_EXE_extract"),
        "elev" => env!("CARGO_BIN_EXE_elev"),
        "tile" => env!("CARGO_BIN_EXE_tile"),
        "areaflags" => env!("CARGO_BIN_EXE_areaflags"),
        "landcover" => env!("CARGO_BIN_EXE_landcover"),
        "railfreq" => env!("CARGO_BIN_EXE_railfreq"),
        "trees" => env!("CARGO_BIN_EXE_trees"),
        "bldtile" => env!("CARGO_BIN_EXE_bldtile"),
        "terrainsub" => env!("CARGO_BIN_EXE_terrainsub"),
        "lakefill" => env!("CARGO_BIN_EXE_lakefill"),
        "coastcheck" => env!("CARGO_BIN_EXE_coastcheck"),
        "terrain" => env!("CARGO_BIN_EXE_terrain"),
        p => panic!("no binary for {p}: add it to exe() and pipeline::cli::PROGRAMS"),
    }
}

/// Every binary in the workspace (crates/*/src/bin/*.rs, crates/*/src/main.rs) is one of
/// pipeline::cli's programs, run by the tests below, or the map server, whose own tests
/// (crates/server/tests/cli.rs) run it the same way. A new binary fails here until it's added.
#[test]
fn every_binary_has_a_checked_command_line() {
    let mut bins = Vec::new();
    for c in std::fs::read_dir(repo().join("crates")).unwrap().flatten() {
        if c.path().join("src/main.rs").exists() {
            bins.push(c.file_name().to_string_lossy().into_owned());
        }
        for b in std::fs::read_dir(c.path().join("src/bin")).into_iter().flatten().flatten() {
            if let Some(n) = b.file_name().to_string_lossy().strip_suffix(".rs") {
                bins.push(n.to_string());
            }
        }
    }
    bins.sort();
    let mut known: Vec<String> = PROGRAMS.iter().map(|p| p.to_string()).chain(["server".to_string()]).collect();
    known.sort();
    assert_eq!(bins, known, "the workspace's binaries and pipeline::cli::PROGRAMS (and server) differ");
    for p in PROGRAMS {
        assert!(Path::new(exe(p)).exists(), "{p}");
    }
}

/// Runs `prog args` in an empty home: its exit code, stdout and stderr; the home left empty.
fn run(prog: &str, args: &[String]) -> (i32, String, String) {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = store::spawn::command(exe(prog));
    cmd.args(args)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env_remove("SCENIC_COORD_URLS")
        .env_remove("SCENIC_COORD_TOKEN");
    let o = store::spawn::output(&mut cmd).unwrap();
    let left: Vec<_> = std::fs::read_dir(home.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(left.is_empty(), "{prog} {args:?} wrote {left:?}");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// Every command of `c` (itself, its subcommands', theirs), as the words naming it.
fn commands(c: &Command, words: Vec<String>, out: &mut Vec<Vec<String>>) {
    out.push(words.clone());
    for s in c.get_subcommands().filter(|s| s.get_name() != "help") {
        let mut w = words.clone();
        w.push(s.get_name().to_string());
        commands(s, w, out);
    }
}

#[test]
fn every_command_helps_and_refuses_without_acting() {
    for p in PROGRAMS {
        let mut all = Vec::new();
        commands(&command(p).unwrap(), Vec::new(), &mut all);
        for words in all {
            for help in ["--help", "-h"] {
                let a: Vec<String> = words.iter().cloned().chain([help.to_string()]).collect();
                let (code, out, err) = run(p, &a);
                assert_eq!(code, 0, "{p} {a:?}: {err}");
                assert!(out.contains("Usage:"), "{p} {a:?}: {out}");
                assert!(err.is_empty(), "{p} {a:?}: {err}");
            }
            let a: Vec<String> = words.iter().cloned().chain(["--nwo".to_string()]).collect();
            let (code, out, err) = run(p, &a);
            assert_eq!(code, 2, "{p} {a:?}: {out}{err}");
            assert!(err.contains("--nwo") && err.contains("Usage:"), "{p} {a:?}: {err}");
            assert!(out.is_empty(), "{p} {a:?}: {out}");
        }
    }
}

#[test]
fn the_programs_list_their_commands() {
    for p in PROGRAMS {
        let c = command(p).unwrap();
        if !c.has_subcommands() {
            continue;
        }
        for top in [&["--help"][..], &["-h"], &["help"]] {
            let a: Vec<String> = top.iter().map(|s| s.to_string()).collect();
            let (code, out, err) = run(p, &a);
            assert_eq!(code, 0, "{p} {a:?}: {err}");
            for s in c.get_subcommands() {
                assert!(out.contains(s.get_name()), "{p} --help lacks {}: {out}", s.get_name());
            }
        }
        let (code, _, err) = run(p, &["no-such-command".to_string()]);
        assert_eq!(code, 2, "{p}: {err}");
        assert!(err.contains("no-such-command"), "{p}: {err}");
    }
}

#[test]
fn what_went_wrong_before() {
    let s = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // `scenic pause --help` paused the build; a typo ran the default.
    let (code, out, _) = run("scenic", &s(&["pause", "--help"]));
    assert_eq!(code, 0);
    assert!(out.contains("--now"));
    let (code, _, err) = run("scenic", &s(&["pause", "--nwo"]));
    assert_eq!(code, 2);
    assert!(err.contains("--nwo"));
    // A flag's value can't be the help asked for, nor missing; nor one given twice.
    let (code, _, _) = run("scenic", &s(&["gc", "--days", "--help"]));
    assert_ne!(code, 0);
    let (code, _, err) = run("scenic", &s(&["gc", "--days"]));
    assert_eq!(code, 2);
    assert!(err.contains("--days"));
    let (code, _, _) = run("scenic-build", &s(&["unit", "--pass", "a", "--pass", "b"]));
    assert_eq!(code, 2);
    // An argument too many.
    let (code, _, _) = run("scenic-build", &s(&["verify", "stray"]));
    assert_eq!(code, 2);
    let (code, _, _) = run("scenic", &s(&["in-use", "3"]));
    assert_eq!(code, 2, "in-use takes off, or --cores");
    // `scenic --root x pool` ran the status.
    let (code, _, _) = run("scenic", &s(&["--root", "/nonexistent", "pool"]));
    assert_eq!(code, 2);
}

/// The command `words` name in `c` (its subcommands followed as far as they go), with the commands
/// above it; None when `c` has commands and the first word isn't one.
fn find<'a>(c: &'a Command, words: &[&str]) -> Option<Vec<&'a Command>> {
    let mut chain = vec![c];
    for w in words {
        match chain.last().unwrap().find_subcommand(w) {
            Some(s) => chain.push(s),
            None if chain.len() == 1 && c.has_subcommands() => return None,
            None => break,
        }
    }
    Some(chain)
}

/// Whether the command (the last of `chain`) takes `--flag`: its own, or one above it gives every
/// command below (global).
fn takes(chain: &[&Command], flag: &str) -> bool {
    let long = &flag[2..];
    long == "help"
        || chain.iter().enumerate().any(|(i, c)| {
            c.get_arguments().any(|a| a.get_long() == Some(long) && (i + 1 == chain.len() || a.is_global_set()))
        })
}

/// The programs whose command lines have commands: their calls are told apart from prose by them.
fn with_commands() -> Vec<(&'static str, Command)> {
    PROGRAMS.iter().map(|p| (*p, command(p).unwrap())).filter(|(_, c)| c.has_subcommands()).collect()
}

/// Every command quoted in the docs (`` `scenic …` ``, `` `scenic-build …` `` and the other
/// programs with commands, in docs/*.md and README.md) is a command of its program, with only
/// flags it takes.
#[test]
fn the_docs_commands_are_the_programs() {
    let progs = with_commands();
    let mut files = vec![repo().join("README.md")];
    for e in std::fs::read_dir(repo().join("docs")).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "md") {
            files.push(e.path());
        }
    }
    let (mut checked, mut bad) = (0, Vec::new());
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        // (A quote may run over a line's end: the text joined, each quote kept with its line.)
        let lines: Vec<&str> = text.lines().collect();
        for (n, l) in lines.iter().enumerate() {
            let joined = match lines.get(n + 1) {
                Some(next) => format!("{l} {}", next.trim_start()),
                None => l.to_string(),
            };
            for (k, span) in joined.split('`').enumerate() {
                // Quotes are the odd pieces; one starting on the next line is that line's.
                if k % 2 == 0 || joined.split('`').take(k).map(|p| p.len() + 1).sum::<usize>() > l.len() {
                    continue;
                }
                let Some((p, c)) = progs.iter().find(|(p, _)| span.starts_with(&format!("{p} "))) else { continue };
                let words: Vec<&str> = span[p.len()..]
                    .split(|c: char| c.is_whitespace() || "[]|".contains(c))
                    .filter(|w| !w.is_empty())
                    .collect();
                let names: Vec<&str> = words
                    .iter()
                    .take_while(|w| w.chars().next().is_some_and(|c| c.is_ascii_lowercase()))
                    .copied()
                    .collect();
                if names.is_empty() {
                    continue;
                }
                let Some(chain) = find(c, &names) else {
                    bad.push(format!("{}:{}: no command {p} {}", f.display(), n + 1, names[0]));
                    continue;
                };
                for w in &words {
                    let w = w.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
                    let w = w.split('=').next().unwrap_or(w);
                    if w.starts_with("--") && w.len() > 2 && !takes(&chain, w) {
                        bad.push(format!("{}:{}: {p} {} doesn't take {w}", f.display(), n + 1, names.join(" ")));
                    }
                }
                checked += 1;
            }
        }
    }
    assert!(
        bad.is_empty(),
        "the docs name commands or flags the programs don't have (fix the docs):\n{}",
        bad.join("\n")
    );
    assert!(checked >= 20, "{checked}");
}

/// Every call of these programs in tools/ (scripts and pages: `scenic-build <step> … --flag` on a
/// line, or `"scenic-build"), ["<step>", …, "--flag"`) names a command it has and only flags that
/// command takes.
#[test]
fn the_tools_calls_are_the_programs() {
    let progs = with_commands();
    let mut files = Vec::new();
    let mut dirs = vec![repo().join("tools")];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() && name != "node_modules" && !name.starts_with('.') {
                dirs.push(p);
            } else if [".sh", ".mjs", ".js", ".py"].iter().any(|x| name.ends_with(x)) {
                files.push(p);
            }
        }
    }
    let mut checked = 0;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        for (n, l) in text.lines().enumerate() {
            for (p, c) in &progs {
                for (i, _) in l.match_indices(p) {
                    // The program's name alone (not `scenic` in `scenic-build`, nor a folder).
                    let after = &l[i + p.len()..];
                    let before = l[..i].chars().last();
                    if after.starts_with(|c: char| c.is_alphanumeric() || "-_/.".contains(c))
                        || before.is_some_and(|c| c.is_alphanumeric() || "-_".contains(c))
                    {
                        continue;
                    }
                    // (A call quoted in prose ends at its closing backquote.)
                    let after = if before == Some('`') { after.split('`').next().unwrap_or("") } else { after };
                    let words: Vec<&str> = after
                        .split(|c: char| c.is_whitespace() || "\"'`,[()".contains(c))
                        .filter(|w| !w.is_empty())
                        .collect();
                    if !words.first().is_some_and(|w| c.find_subcommand(w).is_some()) {
                        continue;
                    }
                    let chain = find(c, &words).unwrap();
                    let s = chain.last().unwrap();
                    for w in &words[1..] {
                        let w = w.trim_end_matches([']', ';', '`', ')', '.', ':']);
                        if w.starts_with("--") && w.len() > 2 && !w.contains('=') {
                            assert!(
                                takes(&chain, w),
                                "{}:{}: {p} {} doesn't take {w}: {l}",
                                f.display(),
                                n + 1,
                                s.get_name()
                            );
                        }
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 5, "{checked}");
}

/// The shell tools that take no arguments of anyone's: Claude Code's hooks (their input is JSON on
/// stdin), a file the others source, and what launchd or the NAS's scheduler runs.
const SHELL_EXEMPT: &[(&str, &str)] = &[
    ("tools/dev/board.sh", "sourced by tools/dev's scripts"),
    ("tools/dev/health-due.sh", "a hook: its input on stdin"),
    ("tools/dev/report-guard.sh", "a hook: its input on stdin"),
    ("tools/dev/shell-guard.sh", "a hook: its input on stdin"),
    ("tools/dev/test-guard.sh", "a hook: its input on stdin"),
    ("tools/nas/fetch-planet.sh", "DSM's Task Scheduler runs it on the NAS"),
    ("tools/taskboard/launchd/backup.sh", "launchd runs it nightly"),
];

/// Every shell tool in tools/ checks its arguments first (docs/building.md, "Command lines"):
/// `--help` (or `-h`) prints its usage and exits 0, anything it doesn't take is refused (exit 2),
/// before it does anything; or it's one of `SHELL_EXEMPT`. (Read, not run: a tool without the check
/// would do what it does.)
#[test]
fn every_shell_tool_checks_its_arguments() {
    let mut dirs = vec![repo().join("tools")];
    let (mut seen, mut bad) = (Vec::new(), Vec::new());
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() && name != "node_modules" && !name.starts_with('.') {
                dirs.push(p);
                continue;
            }
            if !name.ends_with(".sh") {
                continue;
            }
            let rel = p.strip_prefix(repo()).unwrap().to_string_lossy().into_owned();
            if let Some(i) = SHELL_EXEMPT.iter().position(|(f, _)| *f == rel) {
                seen.push(i);
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            let checks =
                text.lines().any(|l| !l.trim_start().starts_with('#') && l.contains("--help") && l.contains("-h"));
            if !checks {
                bad.push(rel);
            }
        }
    }
    assert!(
        bad.is_empty(),
        "shell tools that don't check their arguments (a `case` taking -h|--help, refusing the rest):\n{}",
        bad.join("\n")
    );
    for (i, (f, _)) in SHELL_EXEMPT.iter().enumerate() {
        assert!(seen.contains(&i), "{f}: exempt but not there any more");
    }
}

/// tools/app/remote-tests.sh never hangs connecting (#637): with an `ssh` that never returns (as
/// when 1Password, the ssh agent, waits for an approval), its fresh-connection probe is stopped
/// within its 20 s bound and the command runs here, saying why.
#[test]
fn remote_tests_falls_back_when_ssh_hangs() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\nexec sleep 600\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir(&src).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let mut cmd = store::spawn::command("zsh");
    cmd.arg(repo().join("tools/app/remote-tests.sh"))
        .args(["k637", src.to_str().unwrap(), "echo ran-here"])
        .env("PATH", path)
        .env("SCENIC_TEST_HOST", "hung-host")
        .env_remove("SCENIC_CHECK_LOCAL")
        .env_remove("SCENIC_TESTS_LOCAL");
    let t0 = std::time::Instant::now();
    let o = store::spawn::output(&mut cmd).unwrap();
    let took = t0.elapsed().as_secs();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("no fresh ssh connection to hung-host within 20 s"), "{out}");
    assert!(out.lines().any(|l| l == "ran-here"), "{out}");
    assert!(took < 40, "took {took} s:\n{out}");
}
