//! What the internet answered the steps that ask it a great deal, kept on the NAS (docs/pool.md §2,
//! durable state on the NAS): the items job's Wikidata facts and Wikipedia articles, and the
//! heritage chain's (crate::heritagewd's and crate::whsshapes' Wikidata and Wikipedia
//! caches, pipeline::areadetails' park facts). A step keeps them on its Mac, in its cache, where its scripts add to them as
//! they fetch (a chunk or a file at a time), and on the NAS as one archive a pass (`Kept::nas`: tar,
//! zstd with its checksum), written whole (crate::whole) when the step starts and the NAS hasn't what
//! this Mac has, and when it ends, failed or not, if it changed them: a few large writes a run, never
//! an answer at a time (the NAS takes some 20–55 small files a second). A step stopped (the agent's
//! SIGTERM: scenic-build has no handler) doesn't run its end: what it fetched goes at the next start.
//!
//! Which copy counts, at the step's start (`Kept::sync`): this Mac's while the NAS's archive is the
//! one its files last matched (sent from here or taken from there: the mark beside them), so what it
//! fetched since goes to the NAS, and a file deleted here since (a cache cut short, or facts to ask
//! for again) stays deleted, the archive sent without it. When the NAS's isn't that one (another Mac
//! ran the step since, or this Mac has none of its own yet), the two are made one (`reconcile`):
//! what only one side changed, added or deleted since they last matched, that side's; a file both
//! changed, merged when it's answers kept by key, a run only adding to them (`merge`); and when one
//! can't be merged, the NAS's set taken whole, this Mac's other answers gone. The NAS has none, or
//! one that doesn't read whole (moved aside, `<archive>.bad-<unix seconds>`, never written over):
//! this Mac's seed it.
//!
//! At its end (`Kept::keep`), what the step changed goes to the NAS the same way, but never over an
//! archive that isn't the one this Mac last matched: another writer's since (once the lead can
//! move, another Mac may run a step meanwhile) is made one with this Mac's where it can be, else
//! left as it is, and the next start takes it. The lead's agent sends what its caches have of the
//! pass's answers as it starts, where the NAS hasn't their archive (`seed`). A pass's heritage
//! answers start from the last pass's (`heritage_start`): what the chain asked before is asked
//! again only when its cache entry is deleted.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use store::nasfs::NasPath;

/// The archives' zstd level (JSON lines: a tenth of their size).
const LEVEL: i32 = 10;

/// A step's answers: files under `dir` on this Mac, and their archive on the NAS.
pub struct Kept {
    /// The archive on the NAS.
    pub nas: store::nasfs::NasPath,
    /// Where they're kept on this Mac (the archive's names are under it).
    pub dir: PathBuf,
    /// This Mac's mark: the NAS's archive its files last matched, and their hashes then.
    pub mark: PathBuf,
    /// The answers' lease on the NAS (`LEASE`), taken around each sync and keep, with the budgets on
    /// (`leased`); None: none taken (the agent as it was).
    pub lease: Option<store::nasfs::NasPath>,
    /// The lease while a sync or keep holds it: checked still this holder's before each write to
    /// the NAS (`still_ours`).
    pub holding: std::sync::Mutex<Option<Lease>>,
}

/// A sync's or keep's hold of the answers' lease: given back as it ends.
struct Holding<'a>(&'a Kept);

impl Drop for Holding<'_> {
    fn drop(&mut self) {
        self.0.holding.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

impl Kept {
    /// The answers' lease taken for a sync or keep (when it's `leased`).
    fn hold(&self) -> Result<Option<Holding<'_>>> {
        let Some(p) = self.lease.as_ref() else { return Ok(None) };
        let l = Lease::take(p)?;
        *self.holding.lock().unwrap_or_else(|e| e.into_inner()) = Some(l);
        Ok(Some(Holding(self)))
    }

    /// Before a write to the NAS: the lease, when one's held, still this holder's as its file says
    /// (lapsed, or another taker's since: the sync or keep stops before it writes).
    fn still_ours(&self) -> Result<()> {
        match self.holding.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some(l) => l.ours(),
            None => Ok(()),
        }
    }
}

/// What `Kept::sync` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Synced {
    /// Neither has any.
    None,
    /// This Mac's are the NAS's.
    Same,
    /// This Mac's went to the NAS: it had none, or this Mac changed them since.
    Sent,
    /// The NAS's came here.
    Took,
    /// This Mac's and another writer's made one, here and on the NAS.
    Merged,
}

impl Synced {
    pub fn words(&self) -> &'static str {
        match self {
            Synced::None => "none yet",
            Synced::Same => "the same here as on the NAS",
            Synced::Sent => "sent to the NAS",
            Synced::Took => "taken from the NAS",
            Synced::Merged => "made one with the NAS's, and sent",
        }
    }
}

/// Files by name under a folder, with their hash16.
type Files = BTreeMap<String, String>;

/// A mark: what this Mac's files were when they last matched the NAS's archive.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Mark {
    /// The archive (hash16).
    archive: String,
    /// Each file by its name under `dir`: its hash16.
    files: Files,
}

/// The NAS's archive, copied into the step's scratch folder.
struct Fetched {
    copy: PathBuf,
    hash: String,
}

/// What becomes of a file as two sets are made one (`Kept::reconcile`).
enum Do {
    /// This Mac's as it is (or none, as it is).
    Keep,
    /// The NAS's.
    Take,
    /// Gone here.
    Drop,
    /// Both's, merged.
    Write(Vec<u8>),
}

impl Kept {
    /// At the step's start: this Mac's answers (`files`, by name under `dir`) and the NAS's made one,
    /// as the module's doc says; `scratch` for the archive's copies.
    pub fn sync(&self, files: &[String], scratch: &Path) -> Result<Synced> {
        let _held = self.hold()?;
        let here = hashes(&self.dir, files)?;
        let mark = self.read_mark();
        let Some(nas) = self.fetch(scratch)? else {
            if here.is_empty() {
                return Ok(Synced::None);
            }
            self.send(&here, scratch)?;
            return Ok(Synced::Sent);
        };
        let r = (|| {
            match mark {
                // (This Mac's are the NAS's, or newer: what changed here since goes, deletions too.)
                Some(m) if m.archive == nas.hash => {
                    if m.files == here {
                        return Ok(Synced::Same);
                    }
                    self.send(&here, scratch)?;
                    Ok(Synced::Sent)
                }
                m => {
                    let base = m.map(|m| m.files).unwrap_or_default();
                    match self.reconcile(&nas, &here, &base, scratch)? {
                        Ok(true) => Ok(Synced::Merged),
                        Ok(false) => Ok(Synced::Took),
                        Err(n) => {
                            eprintln!("answers: {n} changed here and on the NAS since this Mac last matched {}, and can't be merged: the NAS's answers taken", self.nas.display());
                            self.take(&nas, &here, scratch)?;
                            Ok(Synced::Took)
                        }
                    }
                }
            }
        })();
        std::fs::remove_file(&nas.copy).ok();
        r
    }

    /// At the step's end, failed or not: this Mac's answers (`files`) sent to the NAS when they
    /// changed since they last matched its archive (made one with another writer's archive there
    /// since, where they can be, else left: the next start takes the NAS's); whether they were.
    pub fn keep(&self, files: &[String], scratch: &Path) -> Result<bool> {
        let _held = self.hold()?;
        let here = hashes(&self.dir, files)?;
        let mark = self.read_mark();
        // (Nothing changed here since: nothing to send, whatever the NAS has now.)
        if mark.as_ref().is_some_and(|m| m.files == here) && self.nas.is_file() {
            return Ok(false);
        }
        let Some(nas) = self.fetch(scratch)? else {
            if here.is_empty() {
                return Ok(false);
            }
            self.send(&here, scratch)?;
            return Ok(true);
        };
        let r = (|| match mark {
            Some(m) if m.archive == nas.hash => {
                self.send(&here, scratch)?;
                Ok(true)
            }
            m => {
                let base = m.map(|m| m.files).unwrap_or_default();
                match self.reconcile(&nas, &here, &base, scratch)? {
                    Ok(sent) => Ok(sent),
                    Err(n) => {
                        eprintln!("answers: {n} changed here and on the NAS since this Mac last matched {}, and can't be merged: left as it is there (the next start takes it)", self.nas.display());
                        Ok(false)
                    }
                }
            }
        })();
        std::fs::remove_file(&nas.copy).ok();
        r
    }

    fn read_mark(&self) -> Option<Mark> {
        serde_json::from_slice(&std::fs::read(&self.mark).ok()?).ok()
    }

    fn write_mark(&self, m: &Mark) -> Result<()> {
        store::whole::write(&self.mark, &serde_json::to_vec(m)?)
    }

    /// The archive's name, for its copies in the scratch folder.
    fn name(&self) -> String {
        self.nas.file_name().map(str::to_string).unwrap_or_else(|| "answers.tar.zst".into())
    }

    /// The NAS's archive, copied here whole and hashed: None when the NAS has none, or had one that
    /// doesn't read whole (its zstd checksum), moved aside there (`<name>.bad-<unix seconds>`), so
    /// this Mac's, perhaps fewer, don't take its place unseen. One that can't be read now (an I/O
    /// error) fails the step, to be tried again.
    fn fetch(&self, scratch: &Path) -> Result<Option<Fetched>> {
        match self.nas.metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("stat {}", self.nas.display())),
            Ok(_) => {}
        }
        std::fs::create_dir_all(scratch)?;
        let copy = scratch.join(format!("nas-{}", self.name()));
        self.nas.copy_to_local(&copy)?;
        if let Err(e) = readable(&copy) {
            std::fs::remove_file(&copy).ok();
            let now = crate::agent::jobs::now_s();
            let mut aside = self.nas.with_file_name(format!("{}.bad-{now}", self.name()));
            for k in 2.. {
                if !aside.exists() {
                    break;
                }
                aside = self.nas.with_file_name(format!("{}.bad-{now}-{k}", self.name()));
            }
            self.still_ours()?;
            self.nas.rename_over(&aside).with_context(|| format!("move {} aside", self.nas.display()))?;
            eprintln!("answers: {} doesn't read whole ({e:#}): moved aside as {}", self.nas.display(), aside.display());
            return Ok(None);
        }
        let hash = store::naming::hash16_file(&copy)?;
        Ok(Some(Fetched { copy, hash }))
    }

    /// This Mac's files (`here`) to the NAS, as one archive written whole; the mark says so.
    fn send(&self, here: &Files, scratch: &Path) -> Result<()> {
        std::fs::create_dir_all(scratch)?;
        let local = scratch.join(self.name());
        let r = (|| {
            pack(&self.dir, &here.keys().cloned().collect::<Vec<_>>(), &local)?;
            let archive = store::naming::hash16_file(&local)?;
            if let Some(d) = self.nas.parent() {
                d.create_dir_all()?;
            }
            self.still_ours()?;
            self.nas.copy_from_local(&local)?;
            self.write_mark(&Mark { archive, files: here.clone() })
        })();
        std::fs::remove_file(&local).ok();
        r
    }

    /// The NAS's archive (`nas`, its copy here) unpacked into a folder of its own, whole (so one cut
    /// short changes nothing here), for `f` to work from; its files and their hashes.
    fn unpacked<T>(&self, nas: &Fetched, scratch: &Path, f: impl FnOnce(&Path, &Files) -> Result<T>) -> Result<T> {
        let tmp = scratch.join("answers-in");
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp)?;
        let r = (|| {
            unpack(&nas.copy, &tmp)?;
            let mut names = Vec::new();
            files_under(&tmp, &tmp, &mut names);
            let theirs = hashes(&tmp, &names)?;
            f(&tmp, &theirs)
        })();
        std::fs::remove_dir_all(&tmp).ok();
        r
    }

    /// The NAS's archive (`nas`) taken over this Mac's answers (`here`), whole: its files put in
    /// place, this Mac's others gone; the mark says
    /// so.
    fn take(&self, nas: &Fetched, here: &Files, scratch: &Path) -> Result<()> {
        self.unpacked(nas, scratch, |tmp, theirs| {
            for n in theirs.keys() {
                self.put_in(&tmp.join(n), n)?;
            }
            for n in here.keys().filter(|n| !theirs.contains_key(*n)) {
                self.drop_here(n)?;
            }
            self.write_mark(&Mark { archive: nas.hash.clone(), files: theirs.clone() })
        })
    }

    /// This Mac's answers (`here`) and the NAS's archive (`nas`) made one, from what they were when
    /// this Mac last matched it (`base`: none, when it never did): a file only one side changed (or
    /// added, or deleted), that side's; one both changed, merged (`merge`). Worked out whole before
    /// anything changes here, then put in place, and sent when the result isn't the NAS's (the mark
    /// says what's sent or taken): Ok(whether it was sent). Err(a file's name), changing nothing,
    /// when a file both changed can't be merged.
    fn reconcile(&self, nas: &Fetched, here: &Files, base: &Files, scratch: &Path) -> Result<Result<bool, String>> {
        self.unpacked(nas, scratch, |tmp, theirs| {
            let names: BTreeSet<&String> = here.keys().chain(theirs.keys()).chain(base.keys()).collect();
            let mut todo: Vec<(&String, Do)> = Vec::new();
            for n in names {
                let (l, t, b) = (here.get(n), theirs.get(n), base.get(n));
                let d = if l == t || t == b {
                    Do::Keep
                } else if l == b {
                    if t.is_some() {
                        Do::Take
                    } else {
                        Do::Drop
                    }
                } else if l.is_some() && t.is_some() {
                    match merge(n, &std::fs::read(tmp.join(n))?, &std::fs::read(self.dir.join(n))?) {
                        Some(m) => Do::Write(m),
                        None => return Ok(Err(n.clone())),
                    }
                } else {
                    return Ok(Err(n.clone()));
                };
                todo.push((n, d));
            }
            let mut now: Vec<String> = Vec::new();
            for (n, d) in todo {
                match d {
                    Do::Keep if here.contains_key(n) => now.push(n.clone()),
                    Do::Keep => {}
                    Do::Take => {
                        self.put_in(&tmp.join(n), n)?;
                        now.push(n.clone());
                    }
                    Do::Drop => self.drop_here(n)?,
                    Do::Write(b) => {
                        store::whole::write(&self.dir.join(n), &b)?;
                        now.push(n.clone());
                    }
                }
            }
            let now = hashes(&self.dir, &now)?;
            if &now == theirs {
                self.write_mark(&Mark { archive: nas.hash.clone(), files: now })?;
                return Ok(Ok(false));
            }
            self.send(&now, scratch)?;
            Ok(Ok(true))
        })
    }

    /// The NAS's file `n`, unpacked at `from`, put in place here.
    fn put_in(&self, from: &Path, n: &str) -> Result<()> {
        let to = self.dir.join(n);
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d)?;
        }
        if std::fs::rename(from, &to).is_err() {
            store::whole::copy(from, &to)?;
        }
        Ok(())
    }

    /// Answer `n` gone here.
    fn drop_here(&self, n: &str) -> Result<()> {
        let to = self.dir.join(n);
        match std::fs::remove_file(&to) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("remove {}", to.display()))
            }
            _ => Ok(()),
        }
    }
}

/// A file both sides changed made one (`Kept::reconcile`), by its name: answers kept as a JSON line
/// per item (`.jsonl`: by its "qid", or its "prop" and "id"), or as a JSON object's entries
/// (crate::heritagewd's short descriptions, pipeline::areadetails' park facts), which
/// runs only add to: the NAS's, then this Mac's the NAS's lack; and the days the items job fetched
/// (`fetched-<date>.json`), the first and the last. None for any other file, or one that isn't
/// what its name says.
fn merge(name: &str, theirs: &[u8], ours: &[u8]) -> Option<Vec<u8>> {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    if leaf.ends_with(".jsonl") {
        return merge_lines(theirs, ours);
    }
    if leaf.starts_with("fetched-") && leaf.ends_with(".json") {
        return merge_days(theirs, ours);
    }
    if ["wd/enwiki-shortdesc.json", "areas-wikidata.json", "special-wd-labels.json"].contains(&name) {
        return merge_entries(theirs, ours);
    }
    None
}

/// JSON lines by key: `theirs`, then those of `ours` with a key `theirs` lacks. (A last line cut
/// short, a run stopped mid-write, is dropped: crate::items's reader does the same.)
fn merge_lines(theirs: &[u8], ours: &[u8]) -> Option<Vec<u8>> {
    let lines = |b: &[u8]| -> Option<Vec<(String, Vec<u8>)>> {
        let whole = &b[..b.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1)];
        whole.split(|&c| c == b'\n').filter(|l| !l.is_empty()).map(|l| Some((line_key(l)?, l.to_vec()))).collect()
    };
    let (t, o) = (lines(theirs)?, lines(ours)?);
    let have: std::collections::HashSet<&str> = t.iter().map(|(k, _)| k.as_str()).collect();
    let mut out = Vec::new();
    for (_, l) in t.iter().chain(o.iter().filter(|(k, _)| !have.contains(k.as_str()))) {
        out.extend_from_slice(l);
        out.push(b'\n');
    }
    Some(out)
}

/// A JSON line's key: its "qid", or its "prop" and "id" (crate::heritagewd's answers by register ID).
fn line_key(line: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(line).ok()?;
    if let Some(q) = v.get("qid").and_then(|q| q.as_str()) {
        return Some(q.to_string());
    }
    let (p, i) = (v.get("prop")?.as_str()?, v.get("id")?);
    Some(format!("{p}\t{}", i.as_str().map_or_else(|| i.to_string(), str::to_string)))
}

/// A JSON object's entries: `theirs`, then those of `ours` with a key `theirs` lacks.
fn merge_entries(theirs: &[u8], ours: &[u8]) -> Option<Vec<u8>> {
    let (Ok(serde_json::Value::Object(mut t)), Ok(serde_json::Value::Object(o))) =
        (serde_json::from_slice(theirs), serde_json::from_slice(ours))
    else {
        return None;
    };
    for (k, v) in o {
        t.entry(k).or_insert(v);
    }
    serde_json::to_vec(&t).ok()
}

/// The days anything was fetched (`{first, last}`): the first of both, the last of both.
fn merge_days(theirs: &[u8], ours: &[u8]) -> Option<Vec<u8>> {
    let (Ok(serde_json::Value::Object(t)), Ok(serde_json::Value::Object(o))) =
        (serde_json::from_slice::<serde_json::Value>(theirs), serde_json::from_slice::<serde_json::Value>(ours))
    else {
        return None;
    };
    let days =
        |k: &str| [&t, &o].into_iter().filter_map(|m| m.get(k)?.as_str().map(str::to_string)).collect::<Vec<_>>();
    serde_json::to_vec(
        &serde_json::json!({"first": days("first").into_iter().min(), "last": days("last").into_iter().max()}),
    )
    .ok()
}

/// Each of `names` under `dir` that's there, with its hash16.
fn hashes(dir: &Path, names: &[String]) -> Result<Files> {
    let mut out = BTreeMap::new();
    for n in names {
        let p = dir.join(n);
        if p.is_file() {
            out.insert(n.clone(), store::naming::hash16_file(&p)?);
        }
    }
    Ok(out)
}

/// The regular files under `dir`, by their names under `base`.
fn files_under(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            files_under(base, &e.path(), out);
        } else if t.is_file() {
            if let Ok(rel) = e.path().strip_prefix(base) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
}

/// Whether archive `p` reads whole: its zstd frames to their end, their checksums right.
fn readable(p: &Path) -> Result<()> {
    let mut z = zstd::Decoder::new(std::fs::File::open(p)?)?;
    std::io::copy(&mut z, &mut std::io::sink())?;
    Ok(())
}

/// `names` under `dir` as one archive `to`: tar (without macOS's extended attributes), zstd with
/// its checksum.
fn pack(dir: &Path, names: &[String], to: &Path) -> Result<()> {
    let list = to.with_file_name(format!(
        "{}.list",
        to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    std::fs::write(&list, names.iter().map(|n| format!("{n}\0")).collect::<String>())?;
    let mut tar = store::spawn::spawn(
        store::spawn::command("tar")
            .env("COPYFILE_DISABLE", "1")
            .arg("--null")
            .arg("-cf")
            .arg("-")
            .arg("-C")
            .arg(dir)
            .arg("-T")
            .arg(&list)
            .stdout(Stdio::piped()),
    )
    .context("run tar")?;
    let r = (|| -> Result<()> {
        let mut z = zstd::Encoder::new(std::fs::File::create(to)?, LEVEL)?;
        z.include_checksum(true)?;
        std::io::copy(tar.stdout.as_mut().context("tar's output")?, &mut z)?;
        z.finish()?.sync_all()?;
        Ok(())
    })();
    let st = tar.wait()?;
    std::fs::remove_file(&list).ok();
    r.with_context(|| format!("write {}", to.display()))?;
    ensure!(st.success(), "tar of {} failed: {st}", dir.display());
    Ok(())
}

/// Archive `from` unpacked into `dir`.
fn unpack<'a>(from: impl Into<store::nasfs::Loc<'a>>, dir: &Path) -> Result<()> {
    let from = from.into();
    let mut tar =
        store::spawn::spawn(store::spawn::command("tar").arg("-xf").arg("-").arg("-C").arg(dir).stdin(Stdio::piped()))
            .context("run tar")?;
    let r = (|| -> Result<()> {
        let mut z = zstd::Decoder::new(from.open()?)?;
        match std::io::copy(&mut z, tar.stdin.as_mut().context("tar's input")?) {
            // (tar ends at the archive's end without reading the padding after it: written after
            // that, as it can be on a loaded Mac, the padding meets a closed pipe. tar's status
            // says whether the archive was unpacked.)
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            r => r.map(|_| ()).map_err(Into::into),
        }
    })();
    drop(tar.stdin.take());
    let st = tar.wait()?;
    r.with_context(|| format!("read {}", from.display()))?;
    ensure!(st.success(), "tar couldn't unpack {}: {st}", from.display());
    Ok(())
}

/// The items job's answers for pass `date` in its cache `dir` (crate::items's): the items' facts,
/// their Wikipedia articles and the days they were fetched.
pub fn items_files(dir: &Path, date: &str) -> Vec<String> {
    [format!("facts-{date}.jsonl"), format!("wp-{date}.jsonl"), format!("fetched-{date}.json")]
        .into_iter()
        .filter(|n| dir.join(n).is_file())
        .collect()
}

/// The items job's for pass `date`: its cache `dir`, its archive in the NAS's project folder `root`.
pub fn items(root: &store::nasfs::NasRoot, dir: &Path, date: &str) -> Kept {
    Kept {
        nas: root.join(format!("sources/items/{date}/answers.tar.zst")),
        dir: dir.to_path_buf(),
        mark: dir.join(format!("kept-{date}.json")),
        lease: None,
        holding: Default::default(),
    }
}

/// What the heritage chain makes again on every run from the pass, in its working copy: not answers.
/// The named places' export (`osm/`, the heritage job's), crate::whsshapes' extracts of the pass's
/// planet (`whs/`) and crate::heritagewd's matches (`wd/items.jsonl`).
const REMADE: [&str; 6] = [
    "whs/tagged.osm.pbf",
    "whs/ids.txt",
    "whs/whs.osm.pbf",
    "whs/whs.geojsonseq",
    "whs/relations.opl",
    "wd/items.jsonl",
];

/// The heritage chain's answers in this pass's working copy `epoch` (`heritage-<date>` in the cache):
/// its files, but what the chain makes again each run (`REMADE`, `osm/`), the marks (names from a
/// dot) and temporary files.
pub fn heritage_files(epoch: &Path) -> Vec<String> {
    let mut names = Vec::new();
    files_under(epoch, epoch, &mut names);
    let mut out: Vec<String> = names
        .into_iter()
        .filter(|n| {
            !n.split('/').any(|c| c.starts_with('.'))
                && !store::whole::is_tmp(Path::new(n))
                && !n.starts_with("osm/")
                && !REMADE.contains(&n.as_str())
        })
        .collect();
    out.sort();
    out
}

/// The heritage chain's for pass `date`: its working copy `epoch`, its archive in the NAS's project
/// folder `root`.
pub fn heritage(root: &store::nasfs::NasRoot, epoch: &Path, date: &str) -> Kept {
    Kept {
        nas: heritage_archive(root, date),
        dir: epoch.to_path_buf(),
        mark: epoch.join(".kept.json"),
        lease: None,
        holding: Default::default(),
    }
}

/// The heritage chain's archive of pass `date`'s answers.
pub fn heritage_archive(root: &store::nasfs::NasRoot, date: &str) -> store::nasfs::NasPath {
    root.join(format!("sources/items/{date}/heritage.tar.zst"))
}

/// A new working copy `epoch` of pass `date`'s heritage answers, when the NAS has none for the pass
/// yet: the newest earlier pass's answers unpacked into it (they're mostly still right; what's
/// wanted again is what's deleted from the caches), so the chain doesn't ask everything again.
/// With no earlier pass's (the first pass since the registers became an input, docs/inputs.md
/// §6.5, Migration), the caches of the registers' snapshot the chain worked in before (`legacy`,
/// `sources/registers/legacy`'s archive: its wd/, desc/ and whs/, read only) with the pass's
/// answers kept beside it laid over them (`sources/items/<date>/heritage-<id>.tar.zst`: its wd/,
/// desc/, whs/ and areas-wikidata.json), less what the chain makes again each run (`REMADE`); the
/// job's keep then sends them as the pass's archive, under the answers' lease. Without a snapshot,
/// the pass's answers alone; a snapshot named (`legacy` given) that can't be read is an error, so
/// an empty copy is never kept as the pass's answers. The archive it started from, when there was
/// one (the snapshot's, else the pass's answers').
pub fn heritage_start(
    root: &store::nasfs::NasRoot,
    epoch: &Path,
    date: &str,
    legacy: Option<&store::nasfs::NasPath>,
) -> Result<Option<store::nasfs::NasPath>> {
    if heritage_archive(root, date).exists() || epoch.join(".kept.json").exists() {
        return Ok(None);
    }
    let mut earlier: Vec<String> = match root.join("sources/items").read_dir() {
        Ok(rd) => rd.into_iter().map(|e| e.name).filter(|d| crate::osmpass::is_date(d) && d.as_str() < date).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).context("list sources/items"),
    };
    earlier.sort();
    for d in earlier.iter().rev() {
        let a = heritage_archive(root, d);
        if a.is_file() {
            std::fs::create_dir_all(epoch)?;
            unpack(&a, epoch)?;
            return Ok(Some(a));
        }
    }
    // (A snapshot the manifest names but that can't be read now, the NAS away: an error, the job
    // tried again; never an empty copy kept as the pass's answers.)
    if let Some(l) = legacy {
        let md = l.metadata().with_context(|| format!("the registers' snapshot {} can't be read", l.display()))?;
        ensure!(md.is_file(), "the registers' snapshot {} isn't a file", l.display());
    }
    let kept: Vec<store::nasfs::NasPath> = match root.join(format!("sources/items/{date}")).read_dir() {
        Ok(rd) => rd
            .into_iter()
            .map(|e| e.path)
            .filter(|p| p.file_name().is_some_and(|n| n.starts_with("heritage-") && n.ends_with(".tar.zst")))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).context("list the pass's answers"),
    };
    ensure!(kept.len() <= 1, "more than one heritage answers archive for {date}: {kept:?}");
    let Some(from) = legacy.cloned().or_else(|| kept.first().cloned()) else { return Ok(None) };
    let parent = epoch.parent().context("the working copy's folder")?;
    let tmp = parent.join(format!(".heritage-start-{date}"));
    std::fs::remove_dir_all(&tmp).ok();
    let r = (|| -> Result<()> {
        std::fs::create_dir_all(epoch)?;
        let lay = |from: &store::nasfs::NasPath, take: &dyn Fn(&str) -> bool| -> Result<()> {
            std::fs::remove_dir_all(&tmp).ok();
            std::fs::create_dir_all(&tmp)?;
            unpack(from, &tmp)?;
            let mut names = Vec::new();
            files_under(&tmp, &tmp, &mut names);
            for n in names.iter().filter(|n| take(n)) {
                let to = epoch.join(n);
                std::fs::create_dir_all(to.parent().context("a file's folder")?)?;
                std::fs::rename(tmp.join(n), &to).with_context(|| format!("put {n} in {}", epoch.display()))?;
            }
            Ok(())
        };
        let cache = |n: &str| ["wd/", "desc/", "whs/"].iter().any(|d| n.starts_with(d));
        if let Some(l) = legacy {
            lay(l, &cache)?;
        }
        if let Some(a) = kept.first() {
            lay(a, &|n: &str| cache(n) || n == "areas-wikidata.json")?;
        }
        for n in REMADE {
            std::fs::remove_file(epoch.join(n)).ok();
        }
        Ok(())
    })();
    std::fs::remove_dir_all(&tmp).ok();
    r.with_context(|| format!("the heritage answers from {}", from.display()))?;
    Ok(Some(from))
}

/// The answers this Mac's agent cache (`cache`) has for the newest complete pass on the NAS
/// (`root`), sent there where it hasn't their archive: the items job's (`items/`), and the heritage
/// chain's (the pass's working copy, `heritage-<date>/`). The lead's agent, as it starts, so a cache
/// from before the NAS kept answers, or one a stopped job left, needn't wait for the step's next
/// run (for the next pass, perhaps months away). What it sent, in words.
///
/// With the budgets on (`leased`), alone among this Mac's steps that keep answers (`Local::writer`:
/// it waits for those under way, and none starts meanwhile), and each archive's keep under the
/// answers' lease on the NAS.
pub fn seed(root: &store::nasfs::NasRoot, cache: &Path, scratch: &Path, leased: bool) -> Result<Vec<String>> {
    let Some(date) = crate::osmpass::latest_pass(root) else { return Ok(Vec::new()) };
    let _local = if leased { Some(Local::writer(cache)?) } else { None };
    let lacks = |k: &Kept| -> Result<bool> {
        match k.nas.metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e).with_context(|| format!("stat {}", k.nas.display())),
            Ok(_) => Ok(false),
        }
    };
    let mut said = Vec::new();
    let it = cache.join("items");
    let (k, files) = (items(root, &it, &date).leased(root, leased), items_files(&it, &date));
    if !files.is_empty() && lacks(&k)? && k.keep(&files, scratch)? {
        said.push(format!("the items job's answers for the {date} pass sent to the NAS ({} files)", files.len()));
    }
    let epoch = cache.join(format!("heritage-{date}"));
    if epoch.join(".done").is_file() {
        let (k, files) = (heritage(root, &epoch, &date).leased(root, leased), heritage_files(&epoch));
        if !files.is_empty() && lacks(&k)? && k.keep(&files, scratch)? {
            said.push(format!(
                "the heritage chain's answers for the {date} pass sent to the NAS ({} files)",
                files.len()
            ));
        }
    }
    Ok(said)
}

/// The answers' lease, under the NAS's project folder (docs/pool.md §7.7, "answered"): one sync or
/// keep at a time across the Macs, so two Macs making their answers one with the NAS's never each
/// send theirs over the other's (a merge lost: the answers fetched again). With the budgets on
/// (crate::budget), in place of the agent holding the steps that keep answers while it seeds them.
pub const LEASE: &str = "state/pool/answers.lease";
/// A lease lapses this long after its holder last renewed it (s): a holder gone (a Mac asleep, a
/// process killed) costs a wait this long, never a torn archive (each is written whole).
pub const LEASE_S: u64 = 600;
/// How often a waiting taker looks again.
const LEASE_AGAIN: std::time::Duration = std::time::Duration::from_secs(5);

impl Kept {
    /// With the answers' lease (`LEASE` under the NAS's project folder `root`) taken around its
    /// syncs and keeps when `on` (the budgets on).
    pub fn leased(mut self, root: &store::nasfs::NasRoot, on: bool) -> Kept {
        self.lease = on.then(|| root.join(LEASE));
        self
    }
}

/// What a lease's file says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LeaseFile {
    /// Who holds it (the host's name and process).
    by: String,
    /// The holder's own mark, so it knows its file from another's.
    token: String,
    /// Until when it holds unless renewed (unix seconds).
    until: u64,
}

/// What a try for the lease found.
#[derive(Debug, PartialEq, Eq)]
pub enum Tried {
    /// It's ours.
    Taken,
    /// Another holds it, until then (unix seconds; who).
    Held(u64, String),
}

/// The answers' lease, held: renewed by a thread of its own while it's held, given up when dropped
/// (its file removed only while it's still this holder's).
pub struct Lease {
    path: NasPath,
    token: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    renewer: Option<std::thread::JoinHandle<()>>,
}

impl Lease {
    /// Takes the lease at `path`, waiting while another holds it (a lapsed one broken).
    pub fn take(path: &NasPath) -> Result<Lease> {
        let token = format!(
            "{}-{}-{}",
            crate::agent::jobs::now_s(),
            std::process::id(),
            store::naming::hash16(format!("{:?}", std::time::Instant::now()).as_bytes())
        );
        let by = format!("{} (pid {})", hostname(), std::process::id());
        let mut said = false;
        loop {
            match try_lease(path, &by, &token, crate::agent::jobs::now_s())? {
                Tried::Taken => break,
                Tried::Held(until, who) => {
                    if !said {
                        eprintln!(
                            "answers: waiting for the answers' lease ({who} holds it, until {until} at the latest)"
                        );
                        said = true;
                    }
                    std::thread::sleep(LEASE_AGAIN);
                }
            }
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (p, t, s) = (path.clone(), token.clone(), stop.clone());
        let renewer = std::thread::Builder::new()
            .name("answers-lease".into())
            .spawn(move || {
                let mut last = std::time::Instant::now();
                while !s.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    if last.elapsed() >= std::time::Duration::from_secs(LEASE_S / 4) {
                        last = std::time::Instant::now();
                        // (A renewal that finds the file another's, or gone, decides nothing: a breaker
                        // may have it aside a moment and put it back; `ours`, before each write,
                        // reads the file then.)
                        if !renew_lease(&p, &t, crate::agent::jobs::now_s()) {
                            eprintln!("answers: the answers' lease wasn't this process's as it renewed it (lapsed and taken, or aside a moment): its next write checks it first");
                        }
                    }
                }
            })
            .ok();
        Ok(Lease { path: path.clone(), token, stop, renewer })
    }

    /// Whether it's still this holder's, as the NAS's file says now: it names this holder and
    /// hasn't lapsed. An error to stop the work it guards, else.
    pub fn ours(&self) -> Result<()> {
        lease_is(&self.path, &self.token, crate::agent::jobs::now_s())
    }
}

/// Whether the lease at `path` is the holder of `token`'s at `now`: an error saying why not.
pub fn lease_is(path: &NasPath, token: &str, now: u64) -> Result<()> {
    match read_lease(path).with_context(|| format!("read {}", path.display()))? {
        Some(h) if h.token == token && h.until > now => Ok(()),
        Some(h) if h.token == token => {
            anyhow::bail!("the answers' lease lapsed ({} s past its end): nothing written", now - h.until)
        }
        Some(h) => anyhow::bail!("the answers' lease is {}'s now: nothing written", h.by),
        None => anyhow::bail!("the answers' lease is gone (broken by another taker): nothing written"),
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.renewer.take() {
            t.join().ok();
        }
        release_lease(&self.path, &self.token);
    }
}

fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut b = [0u8; 256];
        // SAFETY: gethostname writes at most the length given into the buffer it's given.
        if unsafe { libc::gethostname(b.as_mut_ptr().cast(), b.len()) } == 0 {
            return String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned();
        }
    }
    "unknown".into()
}

fn read_lease(path: &NasPath) -> std::io::Result<Option<LeaseFile>> {
    match path.read() {
        Ok(b) => Ok(serde_json::from_slice(&b).ok().or(Some(LeaseFile {
            by: "a holder whose lease doesn't read whole".into(),
            token: String::new(),
            until: 0,
        }))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// One try for the lease at `path` at `now` (unix seconds): made with create-new; one there that
/// has lapsed broken (moved aside by a name of this taker's own, then removed once it's seen to be
/// the lapsed one; one renewed meanwhile, or another's made since, is put back with create-new),
/// then tried again.
pub fn try_lease(path: &NasPath, by: &str, token: &str, now: u64) -> Result<Tried> {
    try_lease_with(path, by, token, now, &mut || {})
}

/// `try_lease`, with `between` run after a lapsed lease is read and before it's moved aside (the
/// tests': its holder renewing meanwhile).
fn try_lease_with(path: &NasPath, by: &str, token: &str, now: u64, between: &mut dyn FnMut()) -> Result<Tried> {
    let mine = LeaseFile { by: by.to_string(), token: token.to_string(), until: now + LEASE_S };
    if let Some(d) = path.parent() {
        d.create_dir_all().with_context(|| format!("make {}", d.display()))?;
    }
    for _ in 0..3 {
        match create_new(path, &serde_json::to_vec(&mine)?) {
            Ok(()) => return Ok(Tried::Taken),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("make {}", path.display())),
        }
        let Some(held) = read_lease(path)? else { continue };
        if held.token == token {
            return Ok(Tried::Taken);
        }
        if held.until > now {
            return Ok(Tried::Held(held.until, held.by));
        }
        // (Lapsed: broken.)
        between();
        let aside = path.with_file_name(format!("{}.lapsed-{token}", path.file_name().unwrap_or_default()));
        match path.rename(&aside) {
            Ok(()) => {
                let moved = read_lease(&aside)?;
                // (Not the lapsed one as it was read: renewed since (its `until` moved on, the same
                // token), or another's made since. Put back. Put back over a lease a third taker
                // made meanwhile (AlreadyExists), the one moved aside is lost: its holder finds so at
                // its next check, before it writes, or its next renewal.)
                if moved.as_ref().is_some_and(|m| *m != held) {
                    let b = aside.read()?;
                    match create_new(path, &b) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e).with_context(|| format!("put back {}", path.display())),
                    }
                }
                aside.remove_file().ok();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("break the lapsed {}", path.display())),
        }
    }
    let held = read_lease(path)?.map_or((now, "another".to_string()), |h| (h.until, h.by));
    Ok(Tried::Held(held.0, held.1))
}

/// The lease at `path` renewed at `now` by the holder of `token` (written whole, by a temporary
/// name: its one writer); whether it's still theirs.
pub fn renew_lease(path: &NasPath, token: &str, now: u64) -> bool {
    match read_lease(path) {
        Ok(Some(mut h)) if h.token == token => {
            h.until = now + LEASE_S;
            serde_json::to_vec(&h).ok().is_some_and(|b| path.write_whole(b).is_ok())
        }
        // (The NAS not answering now: kept, tried again next time.)
        Err(_) => true,
        _ => false,
    }
}

/// The lease at `path` given up by the holder of `token`: removed only while it's theirs.
pub fn release_lease(path: &NasPath, token: &str) {
    if read_lease(path).ok().flatten().is_some_and(|h| h.token == token) {
        path.remove_file().ok();
    }
}

fn create_new(p: &NasPath, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = p.open_with(std::fs::File::options().write(true).create_new(true))?;
    f.write_all(b)
}

/// This Mac's hold on its copies of the answers (under the agent's cache), with writer preference
/// (docs/pool.md §7.7): the steps that keep answers read and add to them for hours (`reader`, any
/// number at once); the agent sending them to the NAS as it starts (`writer`, `seed`) waits for the
/// readers under way, and a reader that starts while it waits waits for it, so a sender never waits
/// on new readers. Two flock'd files in the cache: a gate (a writer holds it while it waits and
/// sends; a reader passes through it) and the use (shared by readers, a writer's alone).
pub struct Local {
    _gate: Option<std::fs::File>,
    _use: std::fs::File,
}

impl Local {
    fn open(cache: &Path, name: &str) -> Result<std::fs::File> {
        std::fs::create_dir_all(cache).with_context(|| format!("make {}", cache.display()))?;
        let p = cache.join(name);
        std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&p)
            .with_context(|| format!("open {}", p.display()))
    }

    /// A step's hold, for its run: shared with other steps', after any writer waiting.
    pub fn reader(cache: &Path) -> Result<Local> {
        let gate = Self::open(cache, "answers.gate")?;
        flock(&gate, false)?;
        let u = Self::open(cache, "answers.use")?;
        flock(&u, false)?;
        // (Through the gate: let go at once.)
        drop(gate);
        Ok(Local { _gate: None, _use: u })
    }

    /// The sender's hold: alone, once the readers under way have ended; no reader starts meanwhile.
    pub fn writer(cache: &Path) -> Result<Local> {
        Self::writer_then(cache, || {})
    }

    /// `writer`, `gated` run once it holds the gate, before it waits for the readers (the tests').
    fn writer_then(cache: &Path, gated: impl FnOnce()) -> Result<Local> {
        let gate = Self::open(cache, "answers.gate")?;
        flock(&gate, true)?;
        gated();
        let u = Self::open(cache, "answers.use")?;
        flock(&u, true)?;
        Ok(Local { _gate: Some(gate), _use: u })
    }

    /// The step's hold as a job's run takes it: with the budgets on (its agent's proxy named in its
    /// environment, crate::budget::PROXY_ENV), a reader's of the agent's cache `cache`; else none.
    pub fn for_job(cache: &Path) -> Result<Option<Local>> {
        if std::env::var_os(crate::budget::PROXY_ENV).is_none() {
            return Ok(None);
        }
        let _p = crate::timings::phase(
            "waiting for the agent to send the pass's answers (if it is)",
            crate::timings::Class::Disk,
        );
        Local::reader(cache).map(Some)
    }
}

/// Whether the budgets are on for this job (its agent's proxy named in its environment): its
/// answers' syncs and keeps take the lease.
pub fn budgets_on_for_job() -> bool {
    std::env::var_os(crate::budget::PROXY_ENV).is_some()
}

/// A flock of `f`, exclusive or shared, waited for.
fn flock(f: &std::fs::File, exclusive: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let op = if exclusive { libc::LOCK_EX } else { libc::LOCK_SH };
        loop {
            // SAFETY: flock on a descriptor `f` owns.
            if unsafe { libc::flock(f.as_raw_fd(), op) } == 0 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e).context("flock the answers' copies");
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (f, exclusive);
        Ok(())
    }
}

/// Whether a flock of `f` could be had now (the tests').
#[cfg(test)]
fn could_lock(f: &std::fs::File, exclusive: bool) -> bool {
    use std::os::fd::AsRawFd;
    let op = (if exclusive { libc::LOCK_EX } else { libc::LOCK_SH }) | libc::LOCK_NB;
    // SAFETY: flock on a descriptor `f` owns.
    let ok = unsafe { libc::flock(f.as_raw_fd(), op) } == 0;
    if ok {
        // SAFETY: as above.
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, b: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// The archive's files and contents, unpacked elsewhere.
    fn archived(k: &Kept, scratch: &Path) -> BTreeMap<String, String> {
        let d = scratch.join("look");
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        unpack(&k.nas, &d).unwrap();
        let mut names = Vec::new();
        files_under(&d, &d, &mut names);
        names.into_iter().map(|n| (n.clone(), read(&d.join(&n)))).collect()
    }

    /// JSON lines of items, by QID.
    fn qids(q: &[&str]) -> String {
        q.iter().map(|q| format!("{{\"qid\":\"{q}\"}}\n")).collect()
    }

    #[test]
    fn the_first_run_seeds_the_nas_and_another_mac_takes_its_answers() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        // The lead's cache, from before the NAS kept answers: two epochs' (only this pass's go).
        let m4 = d.path().join("m4/items");
        put(&m4.join("facts-2026-09-28.jsonl"), "{\"qid\":\"Q1\"}\n");
        put(&m4.join("wp-2026-09-28.jsonl"), "{\"qid\":\"Q1\",\"n\":2}\n");
        put(&m4.join("fetched-2026-09-28.json"), "{\"first\":\"2026-10-05\"}");
        put(&m4.join("facts-2026-08-31.jsonl"), "old\n");
        let k = items(&store::nasfs::NasRoot::local_for_test(&root), &m4, "2026-09-28");
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert!(root.join("sources/items/2026-09-28/answers.tar.zst").is_file());
        assert_eq!(
            archived(&k, &scratch).keys().collect::<Vec<_>>(),
            ["facts-2026-09-28.jsonl", "fetched-2026-09-28.json", "wp-2026-09-28.jsonl"]
        );
        // Again: nothing to send, at the start or the end.
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Same);
        assert!(!k.keep(&items_files(&m4, "2026-09-28"), &scratch).unwrap());
        // Another Mac, without any: it takes them.
        let m1 = d.path().join("m1/items");
        let k1 = items(&store::nasfs::NasRoot::local_for_test(&root), &m1, "2026-09-28");
        assert_eq!(k1.sync(&items_files(&m1, "2026-09-28"), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&m1.join("facts-2026-09-28.jsonl")), "{\"qid\":\"Q1\"}\n");
        assert_eq!(items_files(&m1, "2026-09-28").len(), 3);
        // No temporary file left in the NAS's folder or the scratch folder.
        assert_eq!(std::fs::read_dir(root.join("sources/items/2026-09-28")).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&scratch).unwrap().flatten().filter(|e| e.file_name() != "look").count(), 0);
        // Neither has any of another pass: nothing done.
        assert_eq!(
            items(&store::nasfs::NasRoot::local_for_test(&root), &m1, "2026-10-12")
                .sync(&items_files(&m1, "2026-10-12"), &scratch)
                .unwrap(),
            Synced::None
        );
        assert!(!root.join("sources/items/2026-10-12").exists());
    }

    #[test]
    fn what_a_mac_fetched_since_goes_up_and_another_writers_is_kept_with_it() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        let (m4, m1) = (d.path().join("m4"), d.path().join("m1"));
        let facts = |m: &Path| m.join("facts-2026-09-28.jsonl");
        put(&facts(&m4), &qids(&["Q1"]));
        let k4 = items(&store::nasfs::NasRoot::local_for_test(&root), &m4, "2026-09-28");
        let k1 = items(&store::nasfs::NasRoot::local_for_test(&root), &m1, "2026-09-28");
        let files = |dir: &Path| items_files(dir, "2026-09-28");
        k4.sync(&files(&m4), &scratch).unwrap();
        // The step fetches more: sent at its end.
        put(&facts(&m4), &qids(&["Q1", "Q2"]));
        put(&m4.join("wp-2026-09-28.jsonl"), &qids(&["Q1"]));
        assert!(k4.keep(&files(&m4), &scratch).unwrap());
        assert_eq!(archived(&k4, &scratch)["facts-2026-09-28.jsonl"], qids(&["Q1", "Q2"]));
        // A run stopped before its end (no keep): sent at the next one's start.
        put(&facts(&m4), &qids(&["Q1", "Q2", "Q3"]));
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Sent);
        // The other Mac takes them and fetches more (the lead moved); the lead's run, begun
        // before, fetches others: its start makes the two one, each's kept.
        assert_eq!(k1.sync(&files(&m1), &scratch).unwrap(), Synced::Took);
        put(&facts(&m1), &qids(&["Q1", "Q2", "Q3", "Q5"]));
        assert!(k1.keep(&files(&m1), &scratch).unwrap());
        put(&facts(&m4), &qids(&["Q1", "Q2", "Q3", "Q4"]));
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Merged);
        assert_eq!(read(&facts(&m4)), qids(&["Q1", "Q2", "Q3", "Q5", "Q4"]));
        assert_eq!(archived(&k4, &scratch)["facts-2026-09-28.jsonl"], qids(&["Q1", "Q2", "Q3", "Q5", "Q4"]));
        // The other Mac's next start takes that: it changed nothing since.
        assert_eq!(k1.sync(&files(&m1), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&facts(&m1)), qids(&["Q1", "Q2", "Q3", "Q5", "Q4"]));
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Same);
        // A file both changed that isn't answers by key: the NAS's taken whole at the start.
        put(&facts(&m1), "not a JSON line\n");
        assert!(k1.keep(&files(&m1), &scratch).unwrap());
        put(&facts(&m4), &qids(&["Q1", "Q6"]));
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&facts(&m4)), "not a JSON line\n");
    }

    #[test]
    fn a_file_deleted_here_stays_deleted() {
        // (The review's case, 2026-10-06.) Items' facts deleted to ask Wikidata again: not brought
        // back from the NAS; the archive sent without them.
        let d = tempfile::tempdir().unwrap();
        let (root, scratch, m4) = (d.path().join("nas"), d.path().join("scratch"), d.path().join("m4"));
        put(&m4.join("facts-2026-09-28.jsonl"), "{\"qid\":\"Q1\",\"old\":1}\n");
        put(&m4.join("wp-2026-09-28.jsonl"), "{\"qid\":\"Q1\"}\n");
        let k = items(&store::nasfs::NasRoot::local_for_test(&root), &m4, "2026-09-28");
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        std::fs::remove_file(m4.join("facts-2026-09-28.jsonl")).unwrap();
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert!(!m4.join("facts-2026-09-28.jsonl").exists());
        assert_eq!(archived(&k, &scratch).keys().collect::<Vec<_>>(), ["wp-2026-09-28.jsonl"]);
        // Another Mac with its own (it took them before): its next start takes the deletion too.
        let m1 = d.path().join("m1");
        put(&m1.join("facts-2026-09-28.jsonl"), "{\"qid\":\"Q1\",\"old\":1}\n");
        put(&m1.join("wp-2026-09-28.jsonl"), "{\"qid\":\"Q1\"}\n");
        let k1 = items(&store::nasfs::NasRoot::local_for_test(&root), &m1, "2026-09-28");
        // (Its mark, as the lead's was before the deletion: an archive it matched then.)
        let first = Mark { archive: "0".repeat(16), files: hashes(&m1, &items_files(&m1, "2026-09-28")).unwrap() };
        k1.write_mark(&first).unwrap();
        assert_eq!(k1.sync(&items_files(&m1, "2026-09-28"), &scratch).unwrap(), Synced::Took);
        assert!(!m1.join("facts-2026-09-28.jsonl").exists());
        // The heritage chain's park facts, cut short by a stopped run (sent at the next start), then
        // deleted by hand for the step to seed them again: they stay deleted.
        let epoch = d.path().join("cache/heritage-2026-09-28");
        let h = heritage(&store::nasfs::NasRoot::local_for_test(&root), &epoch, "2026-09-28");
        put(&epoch.join("areas-wikidata.json"), "{\"Q1\": {}}");
        assert!(h.keep(&heritage_files(&epoch), &scratch).unwrap());
        put(&epoch.join("areas-wikidata.json"), "{\"Q1\": {}, \"Q2");
        assert_eq!(h.sync(&heritage_files(&epoch), &scratch).unwrap(), Synced::Sent);
        std::fs::remove_file(epoch.join("areas-wikidata.json")).unwrap();
        assert_eq!(h.sync(&heritage_files(&epoch), &scratch).unwrap(), Synced::Sent);
        assert!(!epoch.join("areas-wikidata.json").exists(), "the cut-short file isn't back");
        assert!(archived(&h, &scratch).is_empty());
        // A file deleted here, which another Mac has: gone there too at its next start.
        put(&epoch.join("wd/ids.jsonl"), "{}");
        assert!(h.keep(&heritage_files(&epoch), &scratch).unwrap());
        let other = d.path().join("m1/heritage-2026-09-28");
        let o = heritage(&store::nasfs::NasRoot::local_for_test(&root), &other, "2026-09-28");
        assert_eq!(o.sync(&heritage_files(&other), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&other.join("wd/ids.jsonl")), "{}");
        std::fs::remove_file(epoch.join("wd/ids.jsonl")).unwrap();
        assert!(h.keep(&heritage_files(&epoch), &scratch).unwrap());
        assert_eq!(o.sync(&heritage_files(&other), &scratch).unwrap(), Synced::Took);
        assert!(!other.join("wd/ids.jsonl").exists());
    }

    #[test]
    fn keep_makes_another_writers_archive_one_with_this_macs_or_leaves_it() {
        // (The review's case, 2026-10-06.) Another writer's archive on the NAS since this Mac's
        // start (another Mac leading meanwhile, or heritage-sites and heritage on two Macs): not
        // written over.
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        let (m4, m1) = (d.path().join("m4"), d.path().join("m1"));
        let files = |dir: &Path| items_files(dir, "2026-09-28");
        put(&m4.join("facts-2026-09-28.jsonl"), &qids(&["Q1"]));
        let (k4, k1) = (
            items(&store::nasfs::NasRoot::local_for_test(&root), &m4, "2026-09-28"),
            items(&store::nasfs::NasRoot::local_for_test(&root), &m1, "2026-09-28"),
        );
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Sent);
        // The lead's run starts (Same) and runs long; the other Mac takes the archive,
        // fetches articles, keeps them.
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Same);
        assert_eq!(k1.sync(&files(&m1), &scratch).unwrap(), Synced::Took);
        put(&m1.join("wp-2026-09-28.jsonl"), &qids(&["Q1"]));
        put(&m1.join("fetched-2026-09-28.json"), "{\"first\":\"2026-10-06\",\"last\":\"2026-10-06\"}");
        assert!(k1.keep(&files(&m1), &scratch).unwrap());
        // The lead's run ends: its facts and the other's articles both kept, here and there.
        put(&m4.join("facts-2026-09-28.jsonl"), &qids(&["Q1", "Q2"]));
        put(&m4.join("fetched-2026-09-28.json"), "{\"first\":\"2026-10-05\",\"last\":\"2026-10-05\"}");
        assert!(k4.keep(&files(&m4), &scratch).unwrap());
        let a = archived(&k4, &scratch);
        assert_eq!(
            (a["facts-2026-09-28.jsonl"].clone(), a["wp-2026-09-28.jsonl"].clone()),
            (qids(&["Q1", "Q2"]), qids(&["Q1"]))
        );
        assert_eq!(read(&m4.join("wp-2026-09-28.jsonl")), qids(&["Q1"]));
        assert_eq!(a["fetched-2026-09-28.json"], "{\"first\":\"2026-10-05\",\"last\":\"2026-10-06\"}");
        // A file both added to: the lines of both.
        assert_eq!(k1.sync(&files(&m1), &scratch).unwrap(), Synced::Took);
        put(&m1.join("facts-2026-09-28.jsonl"), &qids(&["Q1", "Q2", "Q7"]));
        assert!(k1.keep(&files(&m1), &scratch).unwrap());
        put(&m4.join("facts-2026-09-28.jsonl"), &qids(&["Q1", "Q2", "Q8"]));
        assert!(k4.keep(&files(&m4), &scratch).unwrap());
        assert_eq!(archived(&k4, &scratch)["facts-2026-09-28.jsonl"], qids(&["Q1", "Q2", "Q7", "Q8"]));
        assert_eq!(k1.sync(&files(&m1), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&m1.join("facts-2026-09-28.jsonl")), qids(&["Q1", "Q2", "Q7", "Q8"]));
        // One both changed that can't be merged: left as it is on the NAS, nothing changed here;
        // the next start takes the NAS's.
        put(&m1.join("facts-2026-09-28.jsonl"), "the other's\n");
        assert!(k1.keep(&files(&m1), &scratch).unwrap());
        let theirs = k4.nas.read().unwrap();
        put(&m4.join("facts-2026-09-28.jsonl"), "this Mac's\n");
        assert!(!k4.keep(&files(&m4), &scratch).unwrap());
        assert_eq!(k4.nas.read().unwrap(), theirs);
        assert_eq!(read(&m4.join("facts-2026-09-28.jsonl")), "this Mac's\n");
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&m4.join("facts-2026-09-28.jsonl")), "the other's\n");
    }

    #[test]
    fn an_archive_that_doesnt_read_whole_is_moved_aside() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch, m4) = (d.path().join("nas"), d.path().join("scratch"), d.path().join("m4"));
        let folder = root.join("sources/items/2026-09-28");
        let nas = folder.join("answers.tar.zst");
        put(&nas, "not an archive");
        put(&m4.join("facts-2026-09-28.jsonl"), "a\n");
        let k = items(&store::nasfs::NasRoot::local_for_test(&root), &m4, "2026-09-28");
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert_eq!(archived(&k, &scratch)["facts-2026-09-28.jsonl"], "a\n");
        let aside = |n: usize| {
            std::fs::read_dir(&folder)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("answers.tar.zst.bad-"))
                .count()
                == n
        };
        assert!(aside(1), "the damaged one kept beside it");
        // A byte of it changed since (its checksum): a Mac with none takes nothing and changes
        // nothing here, the archive moved aside too; one with its own sends them.
        let mut b = std::fs::read(&nas).unwrap();
        let n = b.len();
        b[n - 6] ^= 1;
        std::fs::write(&nas, &b).unwrap();
        let m1 = d.path().join("m1");
        assert_eq!(
            items(&store::nasfs::NasRoot::local_for_test(&root), &m1, "2026-09-28").sync(&[], &scratch).unwrap(),
            Synced::None
        );
        assert!(!m1.exists() && !nas.exists());
        assert!(aside(2), "each kept");
        put(&m1.join("wp-2026-09-28.jsonl"), "w\n");
        assert_eq!(
            items(&store::nasfs::NasRoot::local_for_test(&root), &m1, "2026-09-28")
                .sync(&items_files(&m1, "2026-09-28"), &scratch)
                .unwrap(),
            Synced::Sent
        );
        assert_eq!(archived(&k, &scratch).keys().collect::<Vec<_>>(), ["wp-2026-09-28.jsonl"]);
    }

    #[test]
    fn padding_written_after_tar_ended_is_no_error() {
        // tar ends at the archive's end, not reading the padding after it. A loaded Mac can write
        // the padding of tar's own records (up to 10 KB) after that; a megabyte of it, more than a
        // pipe holds, is written after that every time.
        let d = tempfile::tempdir().unwrap();
        put(&d.path().join("in/facts.jsonl"), &qids(&["Q1"]));
        let archive = d.path().join("answers.tar.zst");
        pack(&d.path().join("in"), &["facts.jsonl".to_string()], &archive).unwrap();
        let mut f = std::fs::File::options().append(true).open(&archive).unwrap();
        std::io::Write::write_all(&mut f, &zstd::encode_all(&vec![0u8; 1 << 20][..], 1).unwrap()).unwrap();
        drop(f);
        let out = d.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        unpack(&archive, &out).unwrap();
        assert_eq!(read(&out.join("facts.jsonl")), qids(&["Q1"]));
    }

    #[test]
    fn the_heritage_chains_answers_are_its_working_copys_files_and_a_new_pass_starts_from_the_last() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        let epoch = d.path().join("cache/heritage-2026-09-28");
        assert_eq!(
            heritage_start(&store::nasfs::NasRoot::local_for_test(&root), &epoch, "2026-09-28", None).unwrap(),
            None,
            "no earlier pass's: an empty copy"
        );
        let k = heritage(&store::nasfs::NasRoot::local_for_test(&root), &epoch, "2026-09-28");
        assert_eq!(k.sync(&heritage_files(&epoch), &scratch).unwrap(), Synced::None);
        // A run: the caches it asked, a park facts file; what it makes again each run; a temporary
        // file; a marker.
        put(&epoch.join("wd/ids.jsonl"), "wd/ids.jsonl");
        put(&epoch.join("whs/wd-p757.json"), "whs/wd-p757.json");
        put(&epoch.join("areas-wikidata.json"), "{}");
        for f in ["wd/items.jsonl", "whs/whs.osm.pbf", "osm/named.geojsonseq", "wd/wp.jsonl.tmp", ".done"] {
            put(&epoch.join(f), "made again");
        }
        let files = heritage_files(&epoch);
        assert_eq!(files, ["areas-wikidata.json", "wd/ids.jsonl", "whs/wd-p757.json"]);
        assert!(k.keep(&files, &scratch).unwrap());
        assert!(root.join("sources/items/2026-09-28/heritage.tar.zst").is_file());
        // A Mac without the pass's copy takes the answers.
        let other = d.path().join("m1/heritage-2026-09-28");
        assert_eq!(
            heritage_start(&store::nasfs::NasRoot::local_for_test(&root), &other, "2026-09-28", None).unwrap(),
            None,
            "the pass has its own"
        );
        let k1 = heritage(&store::nasfs::NasRoot::local_for_test(&root), &other, "2026-09-28");
        assert_eq!(k1.sync(&heritage_files(&other), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&other.join("wd/ids.jsonl")), "wd/ids.jsonl");
        assert_eq!(heritage_files(&other), files);
        assert!(!k1.keep(&heritage_files(&other), &scratch).unwrap(), "its run adds nothing: nothing sent");
        // A new pass: its copy starts from the last pass's answers, and sends them as its own.
        let next = d.path().join("cache/heritage-2027-03-01");
        assert_eq!(
            heritage_start(&store::nasfs::NasRoot::local_for_test(&root), &next, "2027-03-01", None).unwrap(),
            Some(heritage_archive(&store::nasfs::NasRoot::local_for_test(&root), "2026-09-28"))
        );
        assert_eq!(heritage_files(&next), files);
        let k2 = heritage(&store::nasfs::NasRoot::local_for_test(&root), &next, "2027-03-01");
        assert_eq!(k2.sync(&heritage_files(&next), &scratch).unwrap(), Synced::Sent);
        assert!(heritage_archive(&store::nasfs::NasRoot::local_for_test(&root), "2027-03-01").is_file());
        // (Once it has its own, or a copy of its own: never again over it.)
        std::fs::remove_file(next.join("wd/ids.jsonl")).unwrap();
        assert_eq!(
            heritage_start(&store::nasfs::NasRoot::local_for_test(&root), &next, "2027-03-01", None).unwrap(),
            None
        );
        assert!(!next.join("wd/ids.jsonl").exists());
    }

    #[test]
    fn the_first_pass_since_the_registers_became_an_input_starts_from_the_snapshots_caches_and_its_answers() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        let date = "2026-09-28";
        // The registers' snapshot: its registers (not answers), its caches, a file the chain makes
        // again.
        let snap = d.path().join("snap");
        let names = |dir: &Path| {
            let mut n = Vec::new();
            files_under(dir, dir, &mut n);
            n.sort();
            n
        };
        for (f, b) in [
            ("es/andalucia.geojson", "register"),
            ("wd/ids.jsonl", "old ids"),
            ("wd/items.jsonl", "remade"),
            ("desc/en.jsonl", "old desc"),
            ("whs/wd-p757.json", "old whs"),
        ] {
            put(&snap.join(f), b);
        }
        let legacy = root.join("sources/registers/legacy.1111111111111111.tar.zst");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        pack(&snap, &names(&snap), &legacy).unwrap();
        // The pass's answers kept beside it: newer ids, park facts, something else.
        let ans = d.path().join("ans");
        for (f, b) in [("wd/ids.jsonl", "new ids"), ("areas-wikidata.json", "{}"), ("other.txt", "not taken")] {
            put(&ans.join(f), b);
        }
        let kept = root.join(format!("sources/items/{date}/heritage-2222222222222222.tar.zst"));
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        pack(&ans, &names(&ans), &kept).unwrap();
        let before = names(&root);
        // No snapshot named: the pass's answers alone.
        let alone = d.path().join("alone").join(format!("heritage-{date}"));
        assert_eq!(
            heritage_start(&store::nasfs::NasRoot::local_for_test(&root), &alone, date, None).unwrap(),
            Some(kept.clone()).map(store::nasfs::NasPath::local_for_test)
        );
        assert_eq!(heritage_files(&alone), ["areas-wikidata.json", "wd/ids.jsonl"]);
        assert_eq!(read(&alone.join("wd/ids.jsonl")), "new ids");
        // A snapshot named but not there (the NAS away): an error, nothing made.
        let away = d.path().join("away").join(format!("heritage-{date}"));
        assert!(heritage_start(
            &store::nasfs::NasRoot::local_for_test(&root),
            &away,
            date,
            Some(&store::nasfs::NasPath::local_for_test(&root.join("sources/registers/gone.tar.zst")))
        )
        .is_err());
        assert!(heritage_files(&away).is_empty());
        let epoch = d.path().join("cache").join(format!("heritage-{date}"));
        assert_eq!(
            heritage_start(
                &store::nasfs::NasRoot::local_for_test(&root),
                &epoch,
                date,
                Some(&store::nasfs::NasPath::local_for_test(&legacy))
            )
            .unwrap(),
            Some(legacy.clone()).map(store::nasfs::NasPath::local_for_test)
        );
        assert_eq!(
            heritage_files(&epoch),
            ["areas-wikidata.json", "desc/en.jsonl", "wd/ids.jsonl", "whs/wd-p757.json"]
        );
        assert_eq!(read(&epoch.join("wd/ids.jsonl")), "new ids", "the pass's answers over the snapshot's");
        assert_eq!(read(&epoch.join("desc/en.jsonl")), "old desc");
        assert!(!d.path().join("cache").join(format!(".heritage-start-{date}")).exists());
        assert_eq!(names(&root), before, "the NAS read only");
        // The job's keep sends them as the pass's archive; a later start never lays them again.
        let k = heritage(&store::nasfs::NasRoot::local_for_test(&root), &epoch, date);
        assert_eq!(k.sync(&heritage_files(&epoch), &scratch).unwrap(), Synced::Sent);
        assert_eq!(archived(&k, &scratch).get("wd/ids.jsonl").map(String::as_str), Some("new ids"));
        let other = d.path().join("m1").join(format!("heritage-{date}"));
        assert_eq!(
            heritage_start(
                &store::nasfs::NasRoot::local_for_test(&root),
                &other,
                date,
                Some(&store::nasfs::NasPath::local_for_test(&legacy))
            )
            .unwrap(),
            None
        );
        // Two answers archives for the pass: refused.
        heritage_archive(&store::nasfs::NasRoot::local_for_test(&root), date).remove_file().unwrap();
        std::fs::copy(&kept, kept.with_file_name("heritage-3333333333333333.tar.zst")).unwrap();
        assert!(heritage_start(
            &store::nasfs::NasRoot::local_for_test(&root),
            &d.path().join("m2/heritage-x"),
            date,
            Some(&store::nasfs::NasPath::local_for_test(&legacy))
        )
        .is_err());
    }

    #[test]
    fn answers_by_key_merge_and_other_files_dont() {
        // Lines by "qid", or "prop" and "id"; a last line cut short dropped; the NAS's first.
        let m = merge(
            "facts-2026-09-28.jsonl",
            b"{\"qid\":\"Q1\",\"v\":1}\n{\"qid\":\"Q2\"}\n",
            b"{\"qid\":\"Q1\",\"v\":2}\n{\"qid\":\"Q3\"}\n{\"qid\":\"Q4",
        )
        .unwrap();
        assert_eq!(String::from_utf8(m).unwrap(), "{\"qid\":\"Q1\",\"v\":1}\n{\"qid\":\"Q2\"}\n{\"qid\":\"Q3\"}\n");
        let m = merge(
            "wd/ids.jsonl",
            b"{\"prop\":\"P1216\",\"id\":\"1\",\"rows\":[]}\n",
            b"{\"prop\":\"P1216\",\"id\":\"2\",\"rows\":[]}\n",
        )
        .unwrap();
        assert_eq!(m.iter().filter(|&&c| c == b'\n').count(), 2);
        assert!(merge("wd/wp.jsonl", b"{\"qid\":\"Q1\"}\n", b"no key\n").is_none());
        // Entries of the caches kept as objects; the days fetched, first and last.
        assert_eq!(
            merge("areas-wikidata.json", b"{\"a\":1,\"b\":2}", b"{\"b\":3,\"c\":4}").unwrap(),
            b"{\"a\":1,\"b\":2,\"c\":4}"
        );
        assert_eq!(
            merge(
                "fetched-2026-09-28.json",
                b"{\"first\":\"2026-10-06\",\"last\":\"2026-10-07\"}",
                b"{\"first\":\"2026-10-05\",\"last\":\"2026-10-06\"}"
            )
            .unwrap(),
            b"{\"first\":\"2026-10-05\",\"last\":\"2026-10-07\"}"
        );
        // A register's download, or any other: not merged.
        assert!(merge("nrhp.json", b"{}", b"{}").is_none());
        assert!(merge("es/new-register.json", b"[]", b"[1]").is_none());
    }

    #[test]
    fn the_agent_seeds_what_the_nas_lacks_once() {
        let d = tempfile::tempdir().unwrap();
        let (root, cache, scratch) = (d.path().join("nas"), d.path().join("cache"), d.path().join("scratch"));
        // No complete pass: nothing.
        assert!(seed(&store::nasfs::NasRoot::local_for_test(&root), &cache, &scratch, false).unwrap().is_empty());
        put(&root.join("sources/osm/2026-09-28/pass.0000000000000001.json"), "{}");
        put(&cache.join("items/facts-2026-09-28.jsonl"), &qids(&["Q1"]));
        // (An older pass's: not this pass's to seed.)
        put(&cache.join("items/facts-2026-08-31.jsonl"), &qids(&["Q0"]));
        let epoch = cache.join("heritage-2026-09-28");
        put(&epoch.join(".done"), "");
        put(&epoch.join("areas-wikidata.json"), "{}");
        let said = seed(&store::nasfs::NasRoot::local_for_test(&root), &cache, &scratch, false).unwrap();
        assert_eq!(said.len(), 2, "{said:?}");
        let (k, h) = (
            items(&store::nasfs::NasRoot::local_for_test(&root), &cache.join("items"), "2026-09-28"),
            heritage(&store::nasfs::NasRoot::local_for_test(&root), &epoch, "2026-09-28"),
        );
        assert_eq!(archived(&k, &scratch).keys().collect::<Vec<_>>(), ["facts-2026-09-28.jsonl"]);
        assert_eq!(archived(&h, &scratch).keys().collect::<Vec<_>>(), ["areas-wikidata.json"]);
        // The NAS has them: nothing more, whatever this Mac changed since (the steps' to send).
        put(&cache.join("items/facts-2026-09-28.jsonl"), &qids(&["Q1", "Q2"]));
        assert!(seed(&store::nasfs::NasRoot::local_for_test(&root), &cache, &scratch, false).unwrap().is_empty());
        assert_eq!(archived(&k, &scratch)["facts-2026-09-28.jsonl"], qids(&["Q1"]));
    }

    /// The answers' lease: one holder; another waits while it's renewed; a lapsed one broken and
    /// taken; a holder whose lease was broken can't renew it, nor remove the new holder's.
    #[test]
    fn the_answers_lease_is_one_holders_and_a_lapsed_one_is_broken() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nas/state/pool/answers.lease");
        let t = 1_800_000_000;
        assert_eq!(try_lease(&store::nasfs::NasPath::local_for_test(&p), "m4", "a", t).unwrap(), Tried::Taken);
        assert_eq!(
            try_lease(&store::nasfs::NasPath::local_for_test(&p), "m4", "a", t + 1).unwrap(),
            Tried::Taken,
            "its own again"
        );
        assert_eq!(
            try_lease(&store::nasfs::NasPath::local_for_test(&p), "m1", "b", t + 1).unwrap(),
            Tried::Held(t + LEASE_S, "m4".into())
        );
        // (Renewed: held past its first end.)
        assert!(renew_lease(&store::nasfs::NasPath::local_for_test(&p), "a", t + 500));
        assert_eq!(
            try_lease(&store::nasfs::NasPath::local_for_test(&p), "m1", "b", t + LEASE_S + 1).unwrap(),
            Tried::Held(t + 500 + LEASE_S, "m4".into())
        );
        // (Lapsed: broken and taken.)
        assert_eq!(
            try_lease(&store::nasfs::NasPath::local_for_test(&p), "m1", "b", t + 500 + LEASE_S + 1).unwrap(),
            Tried::Taken
        );
        assert!(
            !renew_lease(&store::nasfs::NasPath::local_for_test(&p), "a", t + 500 + LEASE_S + 2),
            "the old holder's isn't its own any more"
        );
        release_lease(&store::nasfs::NasPath::local_for_test(&p), "a");
        assert!(p.is_file(), "the old holder can't remove the new one's");
        release_lease(&store::nasfs::NasPath::local_for_test(&p), "b");
        assert!(!p.exists());
        // (Nothing left aside.)
        assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 0);
        // (A file that doesn't read whole is a lapsed one.)
        put(&p, "{\"by\":");
        assert_eq!(try_lease(&store::nasfs::NasPath::local_for_test(&p), "m1", "c", t).unwrap(), Tried::Taken);
    }

    /// A holder that renews between a taker's read of its lapsed lease and the taker's move aside
    /// keeps it: the taker puts it back (it differs from what was read), and neither holds it twice.
    #[test]
    fn a_lease_renewed_while_it_is_being_broken_is_put_back() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nas/state/pool/answers.lease");
        let t = 1_800_000_000;
        assert_eq!(try_lease(&store::nasfs::NasPath::local_for_test(&p), "m4", "a", t).unwrap(), Tried::Taken);
        let late = t + LEASE_S + 1;
        let (p2, mut renewed) = (p.clone(), false);
        let r = try_lease_with(&store::nasfs::NasPath::local_for_test(&p), "m1", "b", late, &mut || {
            renewed = renew_lease(&store::nasfs::NasPath::local_for_test(&p2), "a", late)
        })
        .unwrap();
        assert!(renewed);
        assert_eq!(r, Tried::Held(late + LEASE_S, "m4".into()));
        assert!(
            lease_is(&store::nasfs::NasPath::local_for_test(&p), "a", late).is_ok()
                && lease_is(&store::nasfs::NasPath::local_for_test(&p), "b", late).is_err()
        );
        assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 1, "nothing left aside");
    }

    /// A put-back over a lease a third taker made meanwhile: the one moved aside is lost, and its
    /// holder's check says so (it writes nothing more); one holder only.
    #[test]
    fn a_put_back_after_a_third_taker_leaves_one_holder() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nas/state/pool/answers.lease");
        let t = 1_800_000_000;
        assert_eq!(try_lease(&store::nasfs::NasPath::local_for_test(&p), "m4", "a", t).unwrap(), Tried::Taken);
        let late = t + LEASE_S + 1;
        // (B, the breaker, has moved A's lease aside; C takes the path; B's put-back of A's fails.)
        let aside = p.with_file_name("answers.lease.lapsed-x");
        std::fs::rename(&p, &aside).unwrap();
        assert_eq!(try_lease(&store::nasfs::NasPath::local_for_test(&p), "m2", "c", late).unwrap(), Tried::Taken);
        assert!(create_new(&store::nasfs::NasPath::local_for_test(&p), &std::fs::read(&aside).unwrap())
            .is_err_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists));
        std::fs::remove_file(&aside).unwrap();
        // (A finds its lease gone at its check, and can't renew it; C holds it alone.)
        assert!(
            lease_is(&store::nasfs::NasPath::local_for_test(&p), "a", late).is_err()
                && !renew_lease(&store::nasfs::NasPath::local_for_test(&p), "a", late)
        );
        assert!(lease_is(&store::nasfs::NasPath::local_for_test(&p), "c", late).is_ok());
    }

    /// The holder's renewal while a breaker has its lease aside (renewed just before the breaker
    /// read it, so put back): the renewal fails, but the lease is still the holder's once it's
    /// back, and its check before a write passes.
    #[test]
    fn a_renewal_while_the_lease_is_aside_loses_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nas/state/pool/answers.lease");
        let l = Lease::take(&store::nasfs::NasPath::local_for_test(&p)).unwrap();
        let aside = p.with_file_name("answers.lease.lapsed-b");
        std::fs::rename(&p, &aside).unwrap();
        assert!(
            !renew_lease(&store::nasfs::NasPath::local_for_test(&p), &l.token, crate::agent::jobs::now_s()),
            "aside: not found"
        );
        assert!(l.ours().is_err(), "nor written meanwhile");
        create_new(&store::nasfs::NasPath::local_for_test(&p), &std::fs::read(&aside).unwrap()).unwrap();
        std::fs::remove_file(&aside).unwrap();
        l.ours().unwrap();
        drop(l);
        assert!(!p.exists());
    }

    /// A keep whose lease was lost (here: taken by another after it lapsed) writes nothing to the NAS.
    #[test]
    fn a_keep_whose_lease_is_lost_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (root, cache, scratch) = (d.path().join("nas"), d.path().join("cache"), d.path().join("scratch"));
        put(&cache.join("items/facts-2026-09-28.jsonl"), &qids(&["Q1"]));
        let k = items(&store::nasfs::NasRoot::local_for_test(&root), &cache.join("items"), "2026-09-28")
            .leased(&store::nasfs::NasRoot::local_for_test(&root), true);
        let _held = k.hold().unwrap().unwrap();
        // (Another takes the path: the holder's file replaced.)
        std::fs::remove_file(root.join(LEASE)).unwrap();
        assert_eq!(
            try_lease(
                &store::nasfs::NasPath::local_for_test(&root.join(LEASE)),
                "m1",
                "other",
                crate::agent::jobs::now_s()
            )
            .unwrap(),
            Tried::Taken
        );
        let e = k
            .send(&hashes(&cache.join("items"), &items_files(&cache.join("items"), "2026-09-28")).unwrap(), &scratch)
            .unwrap_err();
        assert!(format!("{e:#}").contains("nothing written"), "{e:#}");
        assert!(!k.nas.exists(), "no archive written");
        // (Its lease, given back, leaves the other's.)
        drop(_held);
        assert!(lease_is(
            &store::nasfs::NasPath::local_for_test(&root.join(LEASE)),
            "other",
            crate::agent::jobs::now_s()
        )
        .is_ok());
    }

    /// Writer preference on this Mac: a sender waits for the step under way, and a step starting
    /// while it waits waits for it.
    #[test]
    fn the_sender_waits_for_readers_and_new_readers_wait_for_it() {
        let d = tempfile::tempdir().unwrap();
        let cache = d.path().join("cache");
        let r1 = Local::reader(&cache).unwrap();
        // (Two readers at once.)
        let r0 = Local::reader(&cache).unwrap();
        drop(r0);
        let (got_tx, got_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let (gated_tx, gated_rx) = std::sync::mpsc::channel::<()>();
        let c = cache.clone();
        let w = std::thread::spawn(move || {
            let l = Local::writer_then(&c, || gated_tx.send(()).unwrap()).unwrap();
            got_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            drop(l);
        });
        // (Until the writer holds the gate: its own word, a channel; it dropping it ends the wait.)
        gated_rx.recv().expect("the writer took the gate");
        let gate = std::fs::File::open(cache.join("answers.gate")).unwrap();
        assert!(!could_lock(&gate, false));
        let (r2_tx, r2_rx) = std::sync::mpsc::channel();
        let c = cache.clone();
        let r2 = std::thread::spawn(move || {
            let l = Local::reader(&c).unwrap();
            r2_tx.send(()).unwrap();
            drop(l);
        });
        // (The writer waits for the first reader.)
        assert!(got_rx.try_recv().is_err());
        drop(r1);
        got_rx.recv().unwrap();
        // (The writer has it alone: the new reader waits still.)
        assert!(r2_rx.try_recv().is_err());
        let u = std::fs::File::open(cache.join("answers.use")).unwrap();
        assert!(!could_lock(&u, false));
        go_tx.send(()).unwrap();
        w.join().unwrap();
        r2_rx.recv().unwrap();
        r2.join().unwrap();
    }

    /// The two-Mac run's (docs/pool.md §13; run by hand on each Mac at once, never by the suite):
    /// the answers' lease at `SCENIC_TEST_LEASE` (a scratch folder of the NAS, never the project
    /// folder) taken five times, held three seconds each, each hold's start and end printed
    /// (`held <me> <start ms> <end ms>`) for a look that no two Macs' holds overlap.
    #[test]
    #[ignore]
    fn two_macs_take_the_answers_lease_in_turn() {
        let p = PathBuf::from(
            std::env::var("SCENIC_TEST_LEASE").expect("SCENIC_TEST_LEASE: the lease's path in a scratch folder"),
        );
        assert!(!p.to_string_lossy().contains("/projects/scenic-roads"), "not the project folder");
        let me = hostname();
        let ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        for _ in 0..5 {
            let l = Lease::take(&store::nasfs::NasPath::local_for_test(&p)).unwrap();
            let start = ms();
            std::thread::sleep(std::time::Duration::from_secs(3));
            let end = ms();
            drop(l);
            println!("held {me} {start} {end}");
        }
    }

    /// Leased, a sync and a keep take the lease and give it back; the agent's seed too.
    #[test]
    fn leased_syncs_keeps_and_seeds_give_the_lease_back() {
        let d = tempfile::tempdir().unwrap();
        let (root, cache, scratch) = (d.path().join("nas"), d.path().join("cache"), d.path().join("scratch"));
        put(&root.join("sources/osm/2026-09-28/pass.0000000000000001.json"), "{}");
        put(&cache.join("items/facts-2026-09-28.jsonl"), &qids(&["Q1"]));
        let said = seed(&store::nasfs::NasRoot::local_for_test(&root), &cache, &scratch, true).unwrap();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(!root.join(LEASE).exists());
        let k = items(&store::nasfs::NasRoot::local_for_test(&root), &cache.join("items"), "2026-09-28")
            .leased(&store::nasfs::NasRoot::local_for_test(&root), true);
        assert_eq!(k.lease.clone().map(|l| l.unchecked_path()), Some(root.join(LEASE)));
        put(&cache.join("items/facts-2026-09-28.jsonl"), &qids(&["Q1", "Q2"]));
        assert!(k.keep(&items_files(&cache.join("items"), "2026-09-28"), &scratch).unwrap());
        assert_eq!(k.sync(&items_files(&cache.join("items"), "2026-09-28"), &scratch).unwrap(), Synced::Same);
        assert!(!root.join(LEASE).exists());
        assert!(items(&store::nasfs::NasRoot::local_for_test(&root), &cache, "x")
            .leased(&store::nasfs::NasRoot::local_for_test(&root), false)
            .lease
            .is_none());
    }
}
