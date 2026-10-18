//! Every command of every program with commands (pipeline::cli::tables::ALL, the tables their
//! dispatchers match), run as the built program: with `--help` it prints its usage and exits 0,
//! with a flag it doesn't know or an argument too many it's refused (exit 2), and neither does
//! anything: its home (HOME, the working folder) is left empty, no NAS or agent reached.

use pipeline::cli::{tables::ALL, Cmd, Table, ANY};
use std::path::Path;

/// Every command quoted in the docs (`` `scenic …` ``, `` `scenic-build …` `` and the others' in
/// docs/*.md and README.md) is a command of its program's table, with only flags it takes.
#[test]
fn the_docs_commands_pass_the_tables() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = vec![repo.join("README.md")];
    for e in std::fs::read_dir(repo.join("docs")).unwrap().flatten() {
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
                let Some(t) = ALL.iter().find(|t| span.starts_with(&format!("{} ", t.prog))) else { continue };
                let words: Vec<&str> = span[t.prog.len()..]
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
                let found = t.cmds.iter().filter(|c| {
                    let n: Vec<&str> = c.name.split(' ').collect();
                    n.len() <= names.len() && n[..] == names[..n.len()]
                });
                let Some(c) = found.max_by_key(|c| c.name.split(' ').count()) else {
                    bad.push(format!("{}:{}: no command {} {}", f.display(), n + 1, t.prog, names[0]));
                    continue;
                };
                for w in &words {
                    let w = w.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
                    let w = w.split('=').next().unwrap_or(w);
                    if w.starts_with("--")
                        && w.len() > 2
                        && w != "--help"
                        && !(c.flags.contains(&w) || c.opts.contains(&w) || t.common.contains(&w))
                    {
                        bad.push(format!("{}:{}: {} {} doesn't take {w}", f.display(), n + 1, t.prog, c.name));
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
/// line, or `"scenic-build"), ["<step>", …, "--flag"`) names a command of its table and only flags
/// that command takes.
#[test]
fn the_tools_calls_pass_the_tables() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    let mut dirs = vec![repo.join("tools")];
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
            for t in ALL {
                for (i, _) in l.match_indices(t.prog) {
                    // The program's name alone (not `scenic` in `scenic-build`, nor a folder).
                    let after = &l[i + t.prog.len()..];
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
                    let Some(c) = words.first().and_then(|w| t.get(w)) else { continue };
                    for w in &words[1..] {
                        let w = w.trim_end_matches([']', ';', '`', ')', '.', ':']);
                        if w.starts_with("--") && w.len() > 2 && !w.contains('=') && w != "--help" {
                            assert!(
                                c.flags.contains(&w) || c.opts.contains(&w) || t.common.contains(&w),
                                "{}:{}: {} {} doesn't take {w}: {l}",
                                f.display(),
                                n + 1,
                                t.prog,
                                c.name
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

fn exe(prog: &str) -> &'static str {
    match prog {
        "scenic" => env!("CARGO_BIN_EXE_scenic"),
        "scenic-build" => env!("CARGO_BIN_EXE_scenic-build"),
        "scenic-metrics" => env!("CARGO_BIN_EXE_scenic-metrics"),
        "lakefill" => env!("CARGO_BIN_EXE_lakefill"),
        "coastcheck" => env!("CARGO_BIN_EXE_coastcheck"),
        p => panic!("no binary for {p}"),
    }
}

/// The command line of `c` with `rest` after it (scenic-metrics: its unit folder first).
fn line(t: &Table, c: &Cmd, rest: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = c.name.split(' ').map(String::from).collect();
    if t.prog == "scenic-metrics" {
        a.insert(0, "unit-dir".into());
    }
    a.extend(rest.iter().map(|s| s.to_string()));
    a
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

#[test]
fn every_command_helps_and_refuses_without_acting() {
    for t in ALL {
        assert!(!t.cmds.is_empty());
        for c in t.cmds {
            for help in ["--help", "-h"] {
                let a = line(t, c, &[help]);
                let (code, out, err) = run(t.prog, &a);
                assert_eq!(code, 0, "{} {a:?}: {err}", t.prog);
                assert!(out.contains(&format!("usage: {} {}", t.prog, c.name)), "{} {a:?}: {out}", t.prog);
            }
            let a = line(t, c, &["--nwo"]);
            let (code, out, err) = run(t.prog, &a);
            assert_eq!(code, 2, "{} {a:?}: {out}{err}", t.prog);
            assert!(
                err.contains("unknown flag --nwo") && err.contains(&format!("usage: {} {}", t.prog, c.name)),
                "{} {a:?}: {err}",
                t.prog
            );
            assert!(out.is_empty(), "{} {a:?}: {out}", t.prog);
            if c.pos != ANY {
                let stray = vec!["stray"; c.pos + 1];
                let a = line(t, c, &stray);
                let (code, _, err) = run(t.prog, &a);
                assert_eq!(code, 2, "{} {a:?}: {err}", t.prog);
                assert!(err.contains("unexpected argument"), "{} {a:?}: {err}", t.prog);
            }
        }
    }
}

#[test]
fn the_programs_list_their_commands() {
    for t in ALL {
        for top in [&["--help"][..], &["-h"], &["help"]] {
            let a: Vec<String> = top.iter().map(|s| s.to_string()).collect();
            let (code, out, err) = run(t.prog, &a);
            assert_eq!(code, 0, "{} {a:?}: {err}", t.prog);
            for c in t.cmds {
                assert!(out.contains(&format!("{} {}", t.prog, c.name)), "{} --help lacks {}: {out}", t.prog, c.name);
            }
        }
        let (code, _, err) = run(t.prog, &["no-such-command".to_string()]);
        assert_eq!(code, 2, "{}: {err}", t.prog);
        assert!(err.contains("unknown command"), "{}: {err}", t.prog);
    }
}

#[test]
fn what_went_wrong_before() {
    // `scenic pause --help` paused the build; a typo ran the default.
    let (code, out, _) = run("scenic", &["pause".into(), "--help".into()]);
    assert_eq!(code, 0);
    assert!(out.contains("--now"));
    let (code, _, err) = run("scenic", &["pause".into(), "--nwo".into()]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown flag --nwo"));
    // A flag's value can't be the help asked for, nor missing.
    let (code, _, _) = run("scenic", &["gc".into(), "--days".into(), "--help".into()]);
    assert_eq!(code, 0);
    let (code, _, err) = run("scenic", &["gc".into(), "--days".into()]);
    assert_eq!(code, 2);
    assert!(err.contains("--days needs a value"));
}
