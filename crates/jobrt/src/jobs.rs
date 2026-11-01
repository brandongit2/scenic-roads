//! A job's side of its agent's protocol (docs/building.md §2): the lines it says how far it is in
//! (`report`, `stage`, `within`, `part`) on stderr, which goes to its log, and the agent's reading
//! of that log (`tail`, `error_of`, `parts`, `said`); a program it runs, watched for its own
//! (`run_watched`). Running the jobs is the agent's (pipeline's agent::jobs, which re-exports these).

use std::fs::File;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the epoch.
pub fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The last `n` lines of a log.
pub fn tail(log: &Path, n: usize) -> String {
    let s = end_of(log);
    // Progress bars redraw with carriage returns: keep each line's last state. (The parts' lines
    // are the status's list, not text to show.)
    let lines: Vec<&str> = s
        .lines()
        .map(|l| l.rsplit('\r').next().unwrap_or(l))
        .filter(|l| !l.trim().is_empty() && !l.starts_with("parts: "))
        .collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// What a failed run said of its failure: its error (anyhow's `Error: …` and its causes) or its
/// panic, from its last run's log; empty when it said neither (killed, say). Found by its marker,
/// not by position: the timings' table and progress lines may follow it.
pub fn error_of(log: &Path) -> String {
    let s = end_of(log);
    let lines: Vec<&str> = s.lines().map(|l| l.rsplit('\r').next().unwrap_or(l)).collect();
    let Some(i) = lines.iter().rposition(|l| l.starts_with("Error: ") || l.contains("panicked at")) else {
        return String::new();
    };
    let panic = lines[i].contains("panicked at");
    let mut out = vec![lines[i].trim_end()];
    for l in &lines[i + 1..] {
        // anyhow's causes ("Caused by:", then indented lines); a panic's message on its next line.
        let more =
            l.starts_with("Caused by:") || l.starts_with("    ") || l.trim().is_empty() || (panic && out.len() == 1);
        if !more || out.len() >= 12 {
            break;
        }
        if !l.trim().is_empty() {
            out.push(l.trim_end());
        }
    }
    out.join("\n")
}

/// The line each run of a job begins with in its log (`Running::start`).
pub const RUN_START: &str = "=== run of ";

/// A log's last 64 KB (logs grow long: only the end is read), from its last run's start on.
fn end_of(log: &Path) -> String {
    end_of_run(log).0
}

/// A log's last 64 KB from its last run's start on (`end_of`), and whether that start is in it
/// (else the run said more than that since it began: what came before is out of sight).
fn end_of_run(log: &Path) -> (String, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(log) else { return (String::new(), true) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(64 << 10);
    let _ = f.seek(SeekFrom::Start(from));
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    let s = String::from_utf8_lossy(&b).into_owned();
    match s.rfind(&format!("\n{RUN_START}")).map(|i| i + 1).or(s.starts_with(RUN_START).then_some(0)) {
        Some(i) => (s[i..].split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()), true),
        // (The whole log, when it's shorter than that: a log of no run's start is all one run's.)
        None => (s, from == 0),
    }
}

/// Says how far a job is, for the agent (`progress`): `progress: <done>/<total> <unit>` on stderr,
/// which goes to the job's log. The unit stays the same for the job's (or its part's) whole
/// course: the time left is measured from the pace in it (a word in brackets after it, as in
/// `steps (what's being done)`, may change).
pub fn report(done: u64, total: u64, unit: &str) {
    eprintln!("progress: {}/{total} {unit}", done.min(total));
}

/// Says how far a job is, `report`'s way, counting the item under way by how much of it is done
/// (`progress: 2.375/6 z6 tiles`): a job whose items each take long moves on within each.
pub fn report_f(done: f64, total: u64, unit: &str) {
    let d = (done.clamp(0.0, total as f64) * 1000.0).floor() / 1000.0;
    eprintln!("progress: {d}/{total} {unit}");
}

/// The stage a job said it began last (`stage`): a program's progress within it counts as the
/// stage's (`within`).
static FRAME: std::sync::Mutex<Option<(u64, u64, String)>> = std::sync::Mutex::new(None);

/// Says a job begins stage `k` of its `n` (`report`, in `unit`), and that a program run within it
/// says how far the stage is (`within`).
pub fn stage(k: u64, n: u64, unit: &str) {
    if let Ok(mut f) = FRAME.lock() {
        *f = Some((k, n, unit.to_string()));
    }
    report(k, n, unit);
}

/// The stage under way (`stage`) is `frac` done, as a program it runs says: `<k + frac>/<n>`.
pub fn within(frac: f64) {
    let frame = FRAME.lock().ok().and_then(|f| f.clone());
    if let Some((k, n, unit)) = frame {
        report_f(k as f64 + frac.clamp(0.0, 1.0), n, &unit);
    }
}

/// What a unit of progress is, for measuring its pace: the unit without a word in brackets after it
/// (`steps (clipping…)` and `steps (filtering…)` are both steps).
pub fn unit_key(unit: &str) -> &str {
    match unit.find(" (") {
        Some(i) if unit.ends_with(')') => &unit[..i],
        _ => unit,
    }
}

/// How far a line of a program's output says it is (0–1): a `progress: <done>/<total> …` line, or
/// a bar ending in a percentage (osmium's `[=====>        ]  52% `).
pub fn fraction_of(line: &str) -> Option<f64> {
    let l = line.trim();
    if let Some(rest) = l.strip_prefix("progress: ") {
        let (d, t) = rest.split_once(' ').map_or(rest, |x| x.0).split_once('/')?;
        let (d, t) = (d.parse::<f64>().ok()?, t.parse::<f64>().ok()?);
        return (t > 0.0).then(|| (d / t).clamp(0.0, 1.0));
    }
    if l.starts_with('[') {
        let pct: f64 = l.rsplit(']').next()?.trim().strip_suffix('%')?.trim().parse().ok()?;
        return Some((pct / 100.0).clamp(0.0, 1.0));
    }
    None
}

/// A program's output read as it comes, a line at a time to `each` (a bar's redraws, after carriage
/// returns, each a line, as each comes: osmium redraws its bar without a newline), until it closes.
pub fn each_line(mut from: impl std::io::Read, mut each: impl FnMut(&str)) {
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let mut say = |line: &mut Vec<u8>| {
        if !line.iter().all(u8::is_ascii_whitespace) {
            each(&String::from_utf8_lossy(line));
        }
        line.clear();
    };
    loop {
        let k = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &b in &buf[..k] {
            if b == b'\n' || b == b'\r' {
                say(&mut line);
            } else {
                line.push(b);
            }
        }
    }
    say(&mut line);
}

/// Runs `c` with its standard error read here as it comes: each fraction it says (`fraction_of`)
/// to `seen`, every other line on to this job's log. (osmium says how far it is with `--progress`.)
pub fn run_watched(c: &mut Command, mut seen: impl FnMut(f64)) -> std::io::Result<ExitStatus> {
    let mut child = store::spawn::spawn(&mut c.stderr(Stdio::piped()))?;
    if let Some(err) = child.stderr.take() {
        each_line(err, |l| match fraction_of(l) {
            Some(f) => seen(f),
            None => eprintln!("{l}"),
        });
    }
    child.wait()
}

/// Says which of a job's parts (more than one, in order) it begins, for the agent (`parts`):
/// `parts: <i> <their names, JSON>` on stderr, the whole list each time, so the status lists them
/// under the job (done, under way, to come) however long ago it began.
pub fn part(i: usize, names: &[&str]) {
    eprintln!("parts: {i} {}", serde_json::to_string(names).unwrap_or_default());
}

/// A job's parts and the one it's on: its log's last `parts:` line (`part`).
pub fn parts(log: &Path) -> Option<(usize, Vec<String>)> {
    end_of(log).lines().rev().find_map(|l| {
        let (i, names) = l.trim().strip_prefix("parts: ")?.split_once(' ')?;
        let names: Vec<String> = serde_json::from_str(names).ok()?;
        let i: usize = i.parse().ok()?;
        (i < names.len()).then_some((i, names))
    })
}

/// What a job's log says of how far it is (`said`).
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// Its last `progress: <done>/<total> <unit>` line.
    Progress(f64, f64, String),
    /// Nothing since the part under way, or this run, began.
    NoneYet,
    /// Neither within the log's end: output since pushed it out of sight, and what it last said
    /// holds.
    Unknown,
}

/// How far a job says it is: its log's last `progress: <done>/<total> <unit>` line (build steps
/// print them: scenic-build's `progress`).
pub fn said(log: &Path) -> Said {
    let (s, whole) = end_of_run(log);
    for l in s.lines().rev().map(|l| l.rsplit('\r').next().unwrap_or(l).trim()) {
        // (One said before the part under way began, or this run, is another's: none yet.)
        if l.starts_with("parts: ") || l.starts_with("=== run of ") {
            return Said::NoneYet;
        }
        let Some(rest) = l.strip_prefix("progress: ") else { continue };
        let (frac, unit) = rest.split_once(' ').unwrap_or((rest, ""));
        let Some((d, t)) = frac.split_once('/') else { continue };
        let (Ok(d), Ok(t)) = (d.parse::<f64>(), t.parse::<f64>()) else { continue };
        if t > 0.0 {
            return Said::Progress(d.min(t), t, unit.trim().to_string());
        }
    }
    if whole {
        Said::NoneYet
    } else {
        Said::Unknown
    }
}

/// How far a job says it is (`said`), when its log's end says.
pub fn progress(log: &Path) -> Option<(f64, f64, String)> {
    match said(log) {
        Said::Progress(d, t, u) => Some((d, t, u)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jobs_parts_are_read_from_its_log() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        assert_eq!(parts(&log), None);
        // As a step prints them (pipeline::agent::jobs::part), with its output between.
        let names = ["Getting ready (osmium)", "Details: Wikidata's facts", "Uploading"];
        let line = |i: usize| format!("parts: {i} {}\n", serde_json::to_string(&names).unwrap());
        std::fs::write(
            &log,
            format!("{}progress: 0/3 parts (Getting ready)\nosmium: done\n{}some output\n", line(0), line(1)),
        )
        .unwrap();
        assert_eq!(parts(&log), Some((1, names.iter().map(|s| s.to_string()).collect())));
        // A run after it: none of the earlier run's.
        let mut f = File::options().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut f, format!("{RUN_START}t (1) ===\nstarting\n").as_bytes()).unwrap();
        assert_eq!((parts(&log), progress(&log), tail(&log, 5).as_str()), (None, None, "starting"));
        std::fs::write(
            &log,
            format!("{}progress: 0/3 parts (Getting ready)\nosmium: done\n{}some output\n", line(0), line(1)),
        )
        .unwrap();
        // Not text to show; and a line out of range is none.
        assert_eq!(tail(&log, 5), "progress: 0/3 parts (Getting ready)\nosmium: done\nsome output");
        std::fs::write(&log, format!("parts: 7 {}\n", serde_json::to_string(&names).unwrap())).unwrap();
        assert_eq!(parts(&log), None);
    }

    /// Task #438: a failed job's error is found though the timings' table and a backtrace follow
    /// it (depth-lo's "Too many open files" was past the log's last 20 lines, and the agent's log
    /// said only "failed after N s"); a panic's message too; none when it said neither.
    #[test]
    fn a_failures_error_is_found_wherever_it_is() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        let timings = "timings: composing  14.5 s\n".repeat(30);
        std::fs::write(
            &log,
            format!("{RUN_START}t (1) ===\nprogress: 0.09/3 zooms\n{timings}Error: the depth source globathy\n\nCaused by:\n    0: /x/globathy-8-81-114.tif\n    1: Too many open files (os error 24)\n\nStack backtrace:\n   0: foo\n"),
        )
        .unwrap();
        assert_eq!(
            error_of(&log),
            "Error: the depth source globathy\nCaused by:\n    0: /x/globathy-8-81-114.tif\n    1: Too many open files (os error 24)"
        );
        std::fs::write(&log, "a\nthread 'main' panicked at src/x.rs:1:2:\nboom\nnote: run with RUST_BACKTRACE=1\n")
            .unwrap();
        assert_eq!(error_of(&log), "thread 'main' panicked at src/x.rs:1:2:\nboom");
        std::fs::write(&log, "progress: 1/2 z6 tiles\n").unwrap();
        assert_eq!(error_of(&log), "");
    }

    #[test]
    fn a_parts_progress_is_its_own() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        let names = ["Writing the area's terrain to the NAS", "Packing the new raw tiles onto the NAS"];
        let line = |i: usize| format!("parts: {i} {}\n", serde_json::to_string(&names).unwrap());
        // The part before's last word isn't this part's: none yet, then its own.
        std::fs::write(&log, format!("{}progress: 64/64 packs\n{}walking the cache\n", line(0), line(1))).unwrap();
        assert_eq!(progress(&log), None);
        let mut f = File::options().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut f, b"progress: 5/10 raw tiles\n").unwrap();
        assert_eq!(progress(&log), Some((5.0, 10.0, "raw tiles".to_string())));
    }

    #[test]
    fn progress_said_long_ago_is_out_of_sight_not_gone() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        // Said, then 100 KB of a program's output: out of the log's end, so not known.
        let noise = "x".repeat(99) + "\n";
        std::fs::write(&log, format!("{RUN_START}t (1) ===\nprogress: 2.5/6 z6 tiles\n{}", noise.repeat(1000)))
            .unwrap();
        assert_eq!(said(&log), Said::Unknown);
        // Short: all of it this run's, and nothing said yet.
        std::fs::write(&log, format!("{RUN_START}t (1) ===\nstarting\n")).unwrap();
        assert_eq!(said(&log), Said::NoneYet);
        std::fs::write(&log, "starting\nprogress: 2.375/6 z6 tiles\n").unwrap();
        assert_eq!(said(&log), Said::Progress(2.375, 6.0, "z6 tiles".into()));
    }

    #[test]
    fn fractions_and_bars() {
        assert_eq!(fraction_of("progress: 3/4 vertices"), Some(0.75));
        assert_eq!(fraction_of("[=========>                    ]  30% "), Some(0.3));
        assert_eq!(fraction_of("[======] 100%"), Some(1.0));
        assert_eq!(fraction_of("progress: 1/0 none"), None);
        assert_eq!(fraction_of("cache: reused 5 of 6"), None);
        assert_eq!(unit_key("steps (clipping the tiles)"), "steps");
        assert_eq!(unit_key("z6 tiles"), "z6 tiles");
        assert_eq!(unit_key("MB (of 3)x"), "MB (of 3)x");
        let mut lines = Vec::new();
        each_line(&b"[>   ]  0% \r[=>  ]  50% \rdone\nlast"[..], |l| lines.push(l.to_string()));
        assert_eq!(lines, ["[>   ]  0% ", "[=>  ]  50% ", "done", "last"]);
        // A program's progress, seen as it runs; its other lines on to the log.
        let mut seen = Vec::new();
        let st = run_watched(
            store::spawn::command("/bin/sh")
                .args(["-c", "echo 'progress: 1/4 x' >&2; echo hi >&2; printf '[=> ] 75%%\r' >&2"]),
            |f| seen.push(f),
        )
        .unwrap();
        assert!(st.success());
        assert_eq!(seen, [0.25, 0.75]);
    }

    #[test]
    fn a_bar_redrawn_without_a_newline_is_seen_as_it_comes() {
        // (osmium's: carriage returns alone, the newline only at its end.) The program goes on past
        // its first bar once that bar's been seen here: seen only at its end, it would wait for it
        // in vain, give up after a minute and fail.
        let d = tempfile::tempdir().unwrap();
        let mark = d.path().join("seen");
        let script = r#"printf '[=>  ]  50%% \r' >&2
            i=0; until [ -e "$SEEN" ]; do i=$((i + 1)); [ "$i" -le 6000 ] || exit 1; sleep 0.01; done
            printf '[===]  100%% \n' >&2"#;
        let mut seen = Vec::new();
        let st = run_watched(store::spawn::command("/bin/sh").args(["-c", script]).env("SEEN", &mark), |f| {
            if f == 0.5 {
                std::fs::write(&mark, b"").unwrap();
            }
            seen.push(f);
        })
        .unwrap();
        assert!(st.success(), "its first bar seen only as it ended");
        assert_eq!(seen, [0.5, 1.0]);
    }
}
