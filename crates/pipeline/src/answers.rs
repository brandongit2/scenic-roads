//! What the internet answered the steps that ask it a great deal, kept on the NAS (docs/pool.md §2,
//! durable state on the NAS): the items job's Wikidata facts and Wikipedia articles, and the
//! heritage chain's (heritagewd.py's, areadetails.py's park facts, heritage.py's labels, a register
//! a script downloaded). A step keeps them on its Mac, in its cache, where its scripts add to them as
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
//! archive that isn't the one this Mac last matched: another writer's since (the heritage-sites and
//! heritage jobs share one, and once the lead can move, another Mac may run a step meanwhile) is made
//! one with this Mac's where it can be, else left as it is, and the next start takes it. The build
//! Mac's agent sends what its caches have of the pass's answers as it starts, where the NAS hasn't
//! their archive (`seed`).

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The archives' zstd level (JSON lines: a tenth of their size).
const LEVEL: i32 = 10;

/// A step's answers: files under `dir` on this Mac, and their archive on the NAS.
pub struct Kept {
    /// The archive on the NAS.
    pub nas: PathBuf,
    /// Where they're kept on this Mac (the archive's names are under it).
    pub dir: PathBuf,
    /// This Mac's mark: the NAS's archive its files last matched, and their hashes then.
    pub mark: PathBuf,
    /// Where a file the answers lack is as it was made (the registers' snapshot the heritage
    /// chain's copy of it was made from: a file of it that isn't an answer is the snapshot's); None:
    /// such a file isn't there at all (the items job's).
    pub base: Option<PathBuf>,
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
    /// Gone here (the snapshot's again, where there's one).
    Drop,
    /// Both's, merged.
    Write(Vec<u8>),
}

impl Kept {
    /// At the step's start: this Mac's answers (`files`, by name under `dir`) and the NAS's made one,
    /// as the module's doc says; `scratch` for the archive's copies.
    pub fn sync(&self, files: &[String], scratch: &Path) -> Result<Synced> {
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
        let r = (|| {
            match mark {
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
            }
        })();
        std::fs::remove_file(&nas.copy).ok();
        r
    }

    fn read_mark(&self) -> Option<Mark> {
        serde_json::from_slice(&std::fs::read(&self.mark).ok()?).ok()
    }

    fn write_mark(&self, m: &Mark) -> Result<()> {
        crate::whole::write(&self.mark, &serde_json::to_vec(m)?)
    }

    /// The archive's name, for its copies in the scratch folder.
    fn name(&self) -> String {
        self.nas.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "answers.tar.zst".into())
    }

    /// The NAS's archive, copied here whole and hashed: None when the NAS has none, or had one that
    /// doesn't read whole (its zstd checksum), moved aside there (`<name>.bad-<unix seconds>`), so
    /// this Mac's, perhaps fewer, don't take its place unseen. One that can't be read now (an I/O
    /// error) fails the step, to be tried again.
    fn fetch(&self, scratch: &Path) -> Result<Option<Fetched>> {
        match std::fs::metadata(&self.nas) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("stat {}", self.nas.display())),
            Ok(_) => {}
        }
        std::fs::create_dir_all(scratch)?;
        let copy = scratch.join(format!("nas-{}", self.name()));
        crate::whole::copy(&self.nas, &copy)?;
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
            crate::whole::rename_over(&self.nas, &aside).with_context(|| format!("move {} aside", self.nas.display()))?;
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
                std::fs::create_dir_all(d)?;
            }
            crate::whole::copy(&local, &self.nas)?;
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
    /// place, this Mac's others gone (each the snapshot's again, where there's one); the mark says
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
                        crate::whole::write(&self.dir.join(n), &b)?;
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
            crate::whole::copy(from, &to)?;
        }
        Ok(())
    }

    /// Answer `n` gone here: the snapshot's file again (`base`, its time too, so it's no answer),
    /// where there's one, else none.
    fn drop_here(&self, n: &str) -> Result<()> {
        let to = self.dir.join(n);
        if let Some(from) = self.base.as_ref().map(|b| b.join(n)).filter(|p| p.is_file()) {
            crate::whole::copy(&from, &to)?;
            let t = std::fs::metadata(&from)?.modified()?;
            std::fs::File::options().write(true).open(&to)?.set_modified(t)?;
            return Ok(());
        }
        match std::fs::remove_file(&to) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(|| format!("remove {}", to.display())),
            _ => Ok(()),
        }
    }
}

/// A file both sides changed made one (`Kept::reconcile`), by its name: answers kept as a JSON line
/// per item (`.jsonl`: by its "qid", or its "prop" and "id"), or as a JSON object's entries
/// (heritagewd.py's short descriptions, areadetails.py's park facts, heritage.py's labels), which
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
/// short, a run stopped mid-write, is dropped: dem/items.py's reader does the same.)
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

/// A JSON line's key: its "qid", or its "prop" and "id" (heritagewd.py's answers by register ID).
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
    let (Ok(serde_json::Value::Object(mut t)), Ok(serde_json::Value::Object(o))) = (serde_json::from_slice(theirs), serde_json::from_slice(ours)) else { return None };
    for (k, v) in o {
        t.entry(k).or_insert(v);
    }
    serde_json::to_vec(&t).ok()
}

/// The days anything was fetched (`{first, last}`): the first of both, the last of both.
fn merge_days(theirs: &[u8], ours: &[u8]) -> Option<Vec<u8>> {
    let (Ok(serde_json::Value::Object(t)), Ok(serde_json::Value::Object(o))) = (serde_json::from_slice::<serde_json::Value>(theirs), serde_json::from_slice::<serde_json::Value>(ours)) else { return None };
    let days = |k: &str| [&t, &o].into_iter().filter_map(|m| m.get(k)?.as_str().map(str::to_string)).collect::<Vec<_>>();
    serde_json::to_vec(&serde_json::json!({"first": days("first").into_iter().min(), "last": days("last").into_iter().max()})).ok()
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
    let list = to.with_file_name(format!("{}.list", to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    std::fs::write(&list, names.iter().map(|n| format!("{n}\0")).collect::<String>())?;
    let mut tar = Command::new("tar").env("COPYFILE_DISABLE", "1").arg("--null").arg("-cf").arg("-").arg("-C").arg(dir).arg("-T").arg(&list).stdout(Stdio::piped()).spawn().context("run tar")?;
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
fn unpack(from: &Path, dir: &Path) -> Result<()> {
    let mut tar = Command::new("tar").arg("-xf").arg("-").arg("-C").arg(dir).stdin(Stdio::piped()).spawn().context("run tar")?;
    let r = (|| -> Result<()> {
        let mut z = zstd::Decoder::new(std::fs::File::open(from)?)?;
        std::io::copy(&mut z, tar.stdin.as_mut().context("tar's input")?)?;
        Ok(())
    })();
    drop(tar.stdin.take());
    let st = tar.wait()?;
    r.with_context(|| format!("read {}", from.display()))?;
    ensure!(st.success(), "tar couldn't unpack {}: {st}", from.display());
    Ok(())
}

/// The items job's answers for pass `date` in its cache `dir` (dem/items.py's): the items' facts,
/// their Wikipedia articles and the days they were fetched.
pub fn items_files(dir: &Path, date: &str) -> Vec<String> {
    [format!("facts-{date}.jsonl"), format!("wp-{date}.jsonl"), format!("fetched-{date}.json")].into_iter().filter(|n| dir.join(n).is_file()).collect()
}

/// The items job's for pass `date`: its cache `dir`, its archive in the NAS's project folder `root`.
pub fn items(root: &Path, dir: &Path, date: &str) -> Kept {
    Kept { nas: root.join(format!("sources/items/{date}/answers.tar.zst")), dir: dir.to_path_buf(), mark: dir.join(format!("kept-{date}.json")), base: None }
}

/// What the heritage chain makes again on every run from the pass, in its working copy: not answers.
/// The named places' export (`osm/`, the heritage job's), whsshapes.py's extracts of the pass's
/// planet (`whs/`) and heritagewd.py's matches (`wd/items.jsonl`).
const REMADE: [&str; 6] = ["whs/tagged.osm.pbf", "whs/ids.txt", "whs/whs.osm.pbf", "whs/whs.geojsonseq", "whs/relations.opl", "wd/items.jsonl"];

/// The heritage chain's answers in this pass's working copy `epoch` of the registers' snapshot
/// `snap`: the files there the snapshot lacks, or has at another size or time (a script rewrites a
/// cache whole), but what the chain makes again each run (`REMADE`, `osm/`), the marks (names from
/// a dot) and temporary files.
pub fn heritage_files(epoch: &Path, snap: &Path) -> Vec<String> {
    let mut names = Vec::new();
    files_under(epoch, epoch, &mut names);
    let same = |n: &str| match (std::fs::metadata(epoch.join(n)), std::fs::metadata(snap.join(n))) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
        _ => false,
    };
    let mut out: Vec<String> = names
        .into_iter()
        .filter(|n| !n.split('/').any(|c| c.starts_with('.')) && !crate::whole::is_tmp(Path::new(n)) && !n.starts_with("osm/") && !REMADE.contains(&n.as_str()) && !same(n))
        .collect();
    out.sort();
    out
}

/// The heritage chain's for pass `date`: its working copy `epoch` of the registers' snapshot `snap`
/// (`registers-<id>`), its archive in the NAS's project folder `root`.
pub fn heritage(root: &Path, epoch: &Path, snap: &Path, date: &str, id: &str) -> Kept {
    Kept { nas: root.join(format!("sources/items/{date}/heritage-{id}.tar.zst")), dir: epoch.to_path_buf(), mark: epoch.join(".kept.json"), base: Some(snap.to_path_buf()) }
}

/// The answers this Mac's agent cache (`cache`) has for the newest complete pass on the NAS
/// (`root`), sent there where it hasn't their archive: the items job's (`items/`), and the heritage
/// chain's (each copy of the registers' snapshot made for the pass, `heritage-<date>-<id>/`, its
/// snapshot here too). The build Mac's agent, as it starts, so a cache from before the NAS kept
/// answers, or one a stopped job left, needn't wait for the step's next run (for the next pass,
/// perhaps months away). What it sent, in words.
pub fn seed(root: &Path, cache: &Path, scratch: &Path) -> Result<Vec<String>> {
    let Some(date) = crate::osmpass::latest_pass(root) else { return Ok(Vec::new()) };
    let lacks = |k: &Kept| -> Result<bool> {
        match std::fs::metadata(&k.nas) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e).with_context(|| format!("stat {}", k.nas.display())),
            Ok(_) => Ok(false),
        }
    };
    let mut said = Vec::new();
    let it = cache.join("items");
    let (k, files) = (items(root, &it, &date), items_files(&it, &date));
    if !files.is_empty() && lacks(&k)? && k.keep(&files, scratch)? {
        said.push(format!("the items job's answers for the {date} pass sent to the NAS ({} files)", files.len()));
    }
    let mut copies: Vec<PathBuf> = std::fs::read_dir(cache).into_iter().flatten().flatten().map(|e| e.path()).collect();
    copies.sort();
    for epoch in copies {
        let Some(id) = epoch.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(&format!("heritage-{date}-"))).map(str::to_string) else { continue };
        let snap = cache.join(format!("registers-{id}"));
        if !epoch.join(".done").is_file() || !snap.join(".done").is_file() {
            continue;
        }
        let (k, files) = (heritage(root, &epoch, &snap, &date, &id), heritage_files(&epoch, &snap));
        if !files.is_empty() && lacks(&k)? && k.keep(&files, scratch)? {
            said.push(format!("the heritage chain's answers for the {date} pass sent to the NAS ({} files)", files.len()));
        }
    }
    Ok(said)
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
        // The build Mac's cache, from before the NAS kept answers: two epochs' (only this pass's go).
        let m4 = d.path().join("m4/items");
        put(&m4.join("facts-2026-09-28.jsonl"), "{\"qid\":\"Q1\"}\n");
        put(&m4.join("wp-2026-09-28.jsonl"), "{\"qid\":\"Q1\",\"n\":2}\n");
        put(&m4.join("fetched-2026-09-28.json"), "{\"first\":\"2026-10-05\"}");
        put(&m4.join("facts-2026-08-31.jsonl"), "old\n");
        let k = items(&root, &m4, "2026-09-28");
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert!(root.join("sources/items/2026-09-28/answers.tar.zst").is_file());
        assert_eq!(archived(&k, &scratch).keys().collect::<Vec<_>>(), ["facts-2026-09-28.jsonl", "fetched-2026-09-28.json", "wp-2026-09-28.jsonl"]);
        // Again: nothing to send, at the start or the end.
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Same);
        assert!(!k.keep(&items_files(&m4, "2026-09-28"), &scratch).unwrap());
        // Another Mac, without any: it takes them.
        let m1 = d.path().join("m1/items");
        let k1 = items(&root, &m1, "2026-09-28");
        assert_eq!(k1.sync(&items_files(&m1, "2026-09-28"), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&m1.join("facts-2026-09-28.jsonl")), "{\"qid\":\"Q1\"}\n");
        assert_eq!(items_files(&m1, "2026-09-28").len(), 3);
        // No temporary file left in the NAS's folder or the scratch folder.
        assert_eq!(std::fs::read_dir(root.join("sources/items/2026-09-28")).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&scratch).unwrap().flatten().filter(|e| e.file_name() != "look").count(), 0);
        // Neither has any of another pass: nothing done.
        assert_eq!(items(&root, &m1, "2026-10-12").sync(&items_files(&m1, "2026-10-12"), &scratch).unwrap(), Synced::None);
        assert!(!root.join("sources/items/2026-10-12").exists());
    }

    #[test]
    fn what_a_mac_fetched_since_goes_up_and_another_writers_is_kept_with_it() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        let (m4, m1) = (d.path().join("m4"), d.path().join("m1"));
        let facts = |m: &Path| m.join("facts-2026-09-28.jsonl");
        put(&facts(&m4), &qids(&["Q1"]));
        let k4 = items(&root, &m4, "2026-09-28");
        let k1 = items(&root, &m1, "2026-09-28");
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
        // The other Mac takes them and fetches more (the lead moved); the build Mac's run, begun
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
        let k = items(&root, &m4, "2026-09-28");
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        std::fs::remove_file(m4.join("facts-2026-09-28.jsonl")).unwrap();
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert!(!m4.join("facts-2026-09-28.jsonl").exists());
        assert_eq!(archived(&k, &scratch).keys().collect::<Vec<_>>(), ["wp-2026-09-28.jsonl"]);
        // Another Mac with its own (it took them before): its next start takes the deletion too.
        let m1 = d.path().join("m1");
        put(&m1.join("facts-2026-09-28.jsonl"), "{\"qid\":\"Q1\",\"old\":1}\n");
        put(&m1.join("wp-2026-09-28.jsonl"), "{\"qid\":\"Q1\"}\n");
        let k1 = items(&root, &m1, "2026-09-28");
        // (Its mark, as the build Mac's was before the deletion: an archive it matched then.)
        let first = Mark { archive: "0".repeat(16), files: hashes(&m1, &items_files(&m1, "2026-09-28")).unwrap() };
        k1.write_mark(&first).unwrap();
        assert_eq!(k1.sync(&items_files(&m1, "2026-09-28"), &scratch).unwrap(), Synced::Took);
        assert!(!m1.join("facts-2026-09-28.jsonl").exists());
        // The heritage chain's park facts, cut short by a stopped run (sent at the next start), then
        // deleted by hand for the step to seed them again: they stay deleted.
        let snap = d.path().join("cache/registers-7acb8655abb6");
        put(&snap.join("crhp.json"), "{}");
        let epoch = d.path().join("cache/heritage-2026-09-28-7acb8655abb6");
        assert!(Command::new("cp").arg("-R").arg("-p").arg(&snap).arg(&epoch).status().unwrap().success());
        let h = heritage(&root, &epoch, &snap, "2026-09-28", "7acb8655abb6");
        put(&epoch.join("areas-wikidata.json"), "{\"Q1\": {}}");
        assert!(h.keep(&heritage_files(&epoch, &snap), &scratch).unwrap());
        put(&epoch.join("areas-wikidata.json"), "{\"Q1\": {}, \"Q2");
        assert_eq!(h.sync(&heritage_files(&epoch, &snap), &scratch).unwrap(), Synced::Sent);
        std::fs::remove_file(epoch.join("areas-wikidata.json")).unwrap();
        assert_eq!(h.sync(&heritage_files(&epoch, &snap), &scratch).unwrap(), Synced::Sent);
        assert!(!epoch.join("areas-wikidata.json").exists(), "the cut-short file isn't back");
        assert!(archived(&h, &scratch).is_empty());
        // A file of the snapshot's the archive no longer has: the snapshot's again where it's taken.
        put(&epoch.join("crhp.json"), "{\"changed\": 1}");
        assert!(h.keep(&heritage_files(&epoch, &snap), &scratch).unwrap());
        let other = d.path().join("m1/heritage-2026-09-28-7acb8655abb6");
        assert!(Command::new("cp").arg("-R").arg("-p").arg(&snap).arg(&other).status().unwrap().success());
        let o = heritage(&root, &other, &snap, "2026-09-28", "7acb8655abb6");
        assert_eq!(o.sync(&heritage_files(&other, &snap), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&other.join("crhp.json")), "{\"changed\": 1}");
        put(&epoch.join("crhp.json"), "{}");
        std::fs::File::options().write(true).open(epoch.join("crhp.json")).unwrap().set_modified(std::fs::metadata(snap.join("crhp.json")).unwrap().modified().unwrap()).unwrap();
        assert!(heritage_files(&epoch, &snap).is_empty());
        assert!(h.keep(&heritage_files(&epoch, &snap), &scratch).unwrap());
        assert_eq!(o.sync(&heritage_files(&other, &snap), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&other.join("crhp.json")), "{}");
        assert!(heritage_files(&other, &snap).is_empty(), "the snapshot's, its time too");
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
        let (k4, k1) = (items(&root, &m4, "2026-09-28"), items(&root, &m1, "2026-09-28"));
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Sent);
        // The build Mac's run starts (Same) and runs long; the other Mac takes the archive,
        // fetches articles, keeps them.
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Same);
        assert_eq!(k1.sync(&files(&m1), &scratch).unwrap(), Synced::Took);
        put(&m1.join("wp-2026-09-28.jsonl"), &qids(&["Q1"]));
        put(&m1.join("fetched-2026-09-28.json"), "{\"first\":\"2026-10-06\",\"last\":\"2026-10-06\"}");
        assert!(k1.keep(&files(&m1), &scratch).unwrap());
        // The build Mac's run ends: its facts and the other's articles both kept, here and there.
        put(&m4.join("facts-2026-09-28.jsonl"), &qids(&["Q1", "Q2"]));
        put(&m4.join("fetched-2026-09-28.json"), "{\"first\":\"2026-10-05\",\"last\":\"2026-10-05\"}");
        assert!(k4.keep(&files(&m4), &scratch).unwrap());
        let a = archived(&k4, &scratch);
        assert_eq!((a["facts-2026-09-28.jsonl"].clone(), a["wp-2026-09-28.jsonl"].clone()), (qids(&["Q1", "Q2"]), qids(&["Q1"])));
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
        let theirs = std::fs::read(&k4.nas).unwrap();
        put(&m4.join("facts-2026-09-28.jsonl"), "this Mac's\n");
        assert!(!k4.keep(&files(&m4), &scratch).unwrap());
        assert_eq!(std::fs::read(&k4.nas).unwrap(), theirs);
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
        let k = items(&root, &m4, "2026-09-28");
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert_eq!(archived(&k, &scratch)["facts-2026-09-28.jsonl"], "a\n");
        let aside = |n: usize| std::fs::read_dir(&folder).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("answers.tar.zst.bad-")).count() == n;
        assert!(aside(1), "the damaged one kept beside it");
        // A byte of it changed since (its checksum): a Mac with none takes nothing and changes
        // nothing here, the archive moved aside too; one with its own sends them.
        let mut b = std::fs::read(&nas).unwrap();
        let n = b.len();
        b[n - 6] ^= 1;
        std::fs::write(&nas, &b).unwrap();
        let m1 = d.path().join("m1");
        assert_eq!(items(&root, &m1, "2026-09-28").sync(&[], &scratch).unwrap(), Synced::None);
        assert!(!m1.exists() && !nas.exists());
        assert!(aside(2), "each kept");
        put(&m1.join("wp-2026-09-28.jsonl"), "w\n");
        assert_eq!(items(&root, &m1, "2026-09-28").sync(&items_files(&m1, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert_eq!(archived(&k, &scratch).keys().collect::<Vec<_>>(), ["wp-2026-09-28.jsonl"]);
    }

    #[test]
    fn the_heritage_chains_answers_are_what_it_changed_in_its_copy_of_the_snapshot() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        let snap = d.path().join("cache/registers-7acb8655abb6");
        for f in ["crhp.json", "wd/ids.jsonl", "wd/items.jsonl", "whs/whs.osm.pbf", "whs/wd-p757.json", "special-wd-labels.json", ".done"] {
            put(&snap.join(f), f);
        }
        // The pass's copy (cp -c -R keeps the times; -p in its fallback).
        let epoch = d.path().join("cache/heritage-2026-09-28-7acb8655abb6");
        let st = Command::new("cp").arg("-R").arg("-p").arg(&snap).arg(&epoch).status().unwrap();
        assert!(st.success());
        assert!(heritage_files(&epoch, &snap).is_empty(), "a fresh copy holds no answers");
        let k = heritage(&root, &epoch, &snap, "2026-09-28", "7acb8655abb6");
        assert_eq!(k.sync(&heritage_files(&epoch, &snap), &scratch).unwrap(), Synced::None);
        // A run: the caches it asked more for, a park facts file and a register it downloaded; what
        // it makes again each run; a temporary file; a marker.
        put(&epoch.join("wd/ids.jsonl"), "wd/ids.jsonl and more");
        put(&epoch.join("areas-wikidata.json"), "{}");
        put(&epoch.join("es/new-register.json"), "[]");
        for f in ["wd/items.jsonl", "whs/whs.osm.pbf", "osm/named.geojsonseq", "wd/wp.jsonl.tmp", ".kept-old"] {
            put(&epoch.join(f), "made again");
        }
        let files = heritage_files(&epoch, &snap);
        assert_eq!(files, ["areas-wikidata.json", "es/new-register.json", "wd/ids.jsonl"]);
        assert!(k.keep(&files, &scratch).unwrap());
        assert!(root.join("sources/items/2026-09-28/heritage-7acb8655abb6.tar.zst").is_file());
        // A Mac without the pass's copy makes it from the snapshot and takes the answers over it.
        let other = d.path().join("m1/heritage-2026-09-28-7acb8655abb6");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        assert!(Command::new("cp").arg("-R").arg("-p").arg(&snap).arg(&other).status().unwrap().success());
        let k1 = heritage(&root, &other, &snap, "2026-09-28", "7acb8655abb6");
        assert_eq!(k1.sync(&heritage_files(&other, &snap), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&other.join("wd/ids.jsonl")), "wd/ids.jsonl and more");
        assert_eq!(read(&other.join("es/new-register.json")), "[]");
        assert_eq!(read(&other.join("whs/wd-p757.json")), "whs/wd-p757.json");
        assert_eq!(heritage_files(&other, &snap), files);
        // Its run adds nothing: nothing sent.
        assert!(!k1.keep(&heritage_files(&other, &snap), &scratch).unwrap());
    }

    #[test]
    fn answers_by_key_merge_and_other_files_dont() {
        // Lines by "qid", or "prop" and "id"; a last line cut short dropped; the NAS's first.
        let m = merge("facts-2026-09-28.jsonl", b"{\"qid\":\"Q1\",\"v\":1}\n{\"qid\":\"Q2\"}\n", b"{\"qid\":\"Q1\",\"v\":2}\n{\"qid\":\"Q3\"}\n{\"qid\":\"Q4").unwrap();
        assert_eq!(String::from_utf8(m).unwrap(), "{\"qid\":\"Q1\",\"v\":1}\n{\"qid\":\"Q2\"}\n{\"qid\":\"Q3\"}\n");
        let m = merge("wd/ids.jsonl", b"{\"prop\":\"P1216\",\"id\":\"1\",\"rows\":[]}\n", b"{\"prop\":\"P1216\",\"id\":\"2\",\"rows\":[]}\n").unwrap();
        assert_eq!(m.iter().filter(|&&c| c == b'\n').count(), 2);
        assert!(merge("wd/wp.jsonl", b"{\"qid\":\"Q1\"}\n", b"no key\n").is_none());
        // Entries of the caches kept as objects; the days fetched, first and last.
        assert_eq!(merge("areas-wikidata.json", b"{\"a\":1,\"b\":2}", b"{\"b\":3,\"c\":4}").unwrap(), b"{\"a\":1,\"b\":2,\"c\":4}");
        assert_eq!(merge("fetched-2026-09-28.json", b"{\"first\":\"2026-10-06\",\"last\":\"2026-10-07\"}", b"{\"first\":\"2026-10-05\",\"last\":\"2026-10-06\"}").unwrap(), b"{\"first\":\"2026-10-05\",\"last\":\"2026-10-07\"}");
        // A register's download, or any other: not merged.
        assert!(merge("nrhp.json", b"{}", b"{}").is_none());
        assert!(merge("es/new-register.json", b"[]", b"[1]").is_none());
    }

    #[test]
    fn the_agent_seeds_what_the_nas_lacks_once() {
        let d = tempfile::tempdir().unwrap();
        let (root, cache, scratch) = (d.path().join("nas"), d.path().join("cache"), d.path().join("scratch"));
        // No complete pass: nothing.
        assert!(seed(&root, &cache, &scratch).unwrap().is_empty());
        put(&root.join("sources/osm/2026-09-28/pass.0000000000000001.json"), "{}");
        put(&cache.join("items/facts-2026-09-28.jsonl"), &qids(&["Q1"]));
        // (An older pass's: not this pass's to seed.)
        put(&cache.join("items/facts-2026-08-31.jsonl"), &qids(&["Q0"]));
        let snap = cache.join("registers-7acb8655abb6");
        put(&snap.join(".done"), "");
        put(&snap.join("crhp.json"), "{}");
        let epoch = cache.join("heritage-2026-09-28-7acb8655abb6");
        assert!(Command::new("cp").arg("-R").arg("-p").arg(&snap).arg(&epoch).status().unwrap().success());
        put(&epoch.join("areas-wikidata.json"), "{}");
        let said = seed(&root, &cache, &scratch).unwrap();
        assert_eq!(said.len(), 2, "{said:?}");
        let (k, h) = (items(&root, &cache.join("items"), "2026-09-28"), heritage(&root, &epoch, &snap, "2026-09-28", "7acb8655abb6"));
        assert_eq!(archived(&k, &scratch).keys().collect::<Vec<_>>(), ["facts-2026-09-28.jsonl"]);
        assert_eq!(archived(&h, &scratch).keys().collect::<Vec<_>>(), ["areas-wikidata.json"]);
        // The NAS has them: nothing more, whatever this Mac changed since (the steps' to send).
        put(&cache.join("items/facts-2026-09-28.jsonl"), &qids(&["Q1", "Q2"]));
        assert!(seed(&root, &cache, &scratch).unwrap().is_empty());
        assert_eq!(archived(&k, &scratch)["facts-2026-09-28.jsonl"], qids(&["Q1"]));
    }
}
