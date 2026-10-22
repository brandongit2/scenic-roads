//! The translators' checker, `tools/names/check.py`, run by the translations' check (docs/inputs.md
//! §6.3): its rules on each line (`problem`: romanisation that isn't Hepburn, English in the
//! original's word order, a half translation…) and its stretches of English that drifted from their
//! names (`drift`) are the translations' warnings. One checker, so what the translators run on a
//! batch before dropping it and what the input check says of it can't differ.
//!
//! It runs as the Python steps do, from the app's copy of `dem/` (`uv run`, the Python that
//! `dem/.python-version` pins, so its Unicode tables, and with them its verdicts, are the same on
//! every Mac), on the file's bytes written to the job's scratch, in its `--flagged` mode: a JSON
//! object a flagged line or stretch.

use anyhow::{bail, Context, Result};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// The checker, as the to-do lists carry it beside the translators' brief.
pub const CHECK: &str = include_str!("../../../../tools/names/check.py");

/// The Python it runs in, as `dem/.python-version` pins it (the app's copy of `dem/` is this
/// repository's, so the pin read here is the one `uv run` honours there).
pub const PYTHON_VERSION: &str = include_str!("../../../../dem/.python-version");

/// How to run Python: the program and its first arguments, where, and where to put the files.
#[derive(Clone, Debug)]
pub struct Python {
    pub cmd: Vec<String>,
    pub cwd: PathBuf,
    pub scratch: PathBuf,
}

impl Python {
    /// The app's: `uv run` in its copy of `dem/` (the pinned Python, no project synced: the
    /// checker needs the standard library alone), isolated from the environment's.
    pub fn uv(dem: &Path, scratch: &Path) -> Python {
        Python {
            cmd: ["uv", "run", "--no-project", "python", "-I"].map(String::from).to_vec(),
            cwd: dem.to_path_buf(),
            scratch: scratch.to_path_buf(),
        }
    }
}

/// A stand-in for the checker (tests: what it would flag in a file's bytes, with no Python run).
pub type Stub = fn(&[u8]) -> Vec<Flag>;

thread_local! {
    /// How the checker runs on this thread: the check job's (it checks on the thread that set it),
    /// or a test's own.
    static PYTHON: RefCell<Option<Python>> = const { RefCell::new(None) };
    static STUB: RefCell<Option<Stub>> = const { RefCell::new(None) };
}

/// Sets how the checker runs on this thread (`scenic-build inputs --dem`).
pub fn set(p: Python) {
    PYTHON.with(|c| *c.borrow_mut() = Some(p));
}

/// Puts a stand-in for the checker on this thread (the gate's tests).
pub fn stub(s: Stub) {
    STUB.with(|c| *c.borrow_mut() = Some(s));
}

/// How the checker runs on a thread (`here`), for the pool threads a check hands its files to.
#[derive(Clone)]
pub struct Setting(Option<Python>, Option<Stub>);

impl Setting {
    /// Makes it how the checker runs on the calling thread.
    pub fn install(&self) {
        PYTHON.with(|c| *c.borrow_mut() = self.0.clone());
        STUB.with(|c| *c.borrow_mut() = self.1);
    }
}

/// How the checker runs on this thread.
pub fn here() -> Setting {
    Setting(PYTHON.with(|c| c.borrow().clone()), STUB.with(|c| *c.borrow()))
}

/// A line, or a stretch of lines, the checker flags (line numbers from 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    pub from: usize,
    pub to: usize,
    pub problem: String,
}

/// What the checker flags in a file's bytes. An error when it can't be run (no Python set, or it
/// fails): the check job fails, and runs again, rather than take a file unchecked.
pub fn flagged(bytes: &[u8]) -> Result<Vec<Flag>> {
    if let Some(s) = STUB.with(|c| *c.borrow()) {
        return Ok(s(bytes));
    }
    let Some(py) = PYTHON.with(|c| c.borrow().clone()) else {
        bail!("the translations' checker has no Python to run in (scenic-build inputs --dem)")
    };
    std::fs::create_dir_all(&py.scratch).with_context(|| format!("create {}", py.scratch.display()))?;
    // (Named without a quoted `.py`: publish.sh takes every quoted one in the pipeline for a Python
    // step of the app's dem/, and this one isn't.)
    let script = py.scratch.join(format!("check-{}", store::naming::hash16(CHECK.as_bytes()))).with_extension("py");
    if std::fs::read(&script).ok().as_deref() != Some(CHECK.as_bytes()) {
        crate::whole::write(&script, CHECK.as_bytes())?;
    }
    let file = py.scratch.join(format!("flagged-{}-{}.jsonl", std::process::id(), store::naming::hash16(bytes)));
    crate::whole::write(&file, bytes)?;
    let mut c = store::spawn::command(&py.cmd[0]);
    c.current_dir(&py.cwd).args(&py.cmd[1..]).arg(&script).arg("--flagged").arg(&file);
    let out =
        store::spawn::output(&mut c).with_context(|| format!("run the translations' checker ({})", py.cmd.join(" ")));
    std::fs::remove_file(&file).ok();
    let out = out?;
    if !out.status.success() {
        bail!("the translations' checker failed ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
    }
    parse(&out.stdout)
}

/// The checker's `--flagged` output read.
pub fn parse(stdout: &[u8]) -> Result<Vec<Flag>> {
    let text = std::str::from_utf8(stdout).context("the translations' checker's output isn't UTF-8")?;
    let mut out = Vec::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value =
            serde_json::from_str(l).with_context(|| format!("the translations' checker said {l:?}"))?;
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).map(|x| x as usize);
        let problem = v
            .get("problem")
            .and_then(|p| p.as_str())
            .with_context(|| format!("the translations' checker said {l:?}"))?
            .to_string();
        let (from, to) = match (n("line"), n("from"), n("to")) {
            (Some(i), _, _) => (i, i),
            (None, Some(a), Some(b)) if a <= b => (a, b),
            _ => bail!("the translations' checker said {l:?}"),
        };
        anyhow::ensure!(from >= 1, "the translations' checker said {l:?}");
        out.push(Flag { from, to, problem });
    }
    Ok(out)
}
