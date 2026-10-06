//! What the internet answered the steps that ask it a great deal, kept on the NAS (docs/pool.md §2,
//! durable state on the NAS): the items job's Wikidata facts and Wikipedia articles, and the
//! heritage chain's (heritagewd.py's, areadetails.py's park facts, heritage.py's labels, a register
//! a script downloaded). A step keeps them on its Mac, in its cache, where its scripts add to them as
//! they fetch (a chunk or a file at a time), and on the NAS as one archive a pass (`Kept::nas`: tar,
//! zstd), written whole (crate::whole) when the step starts and the NAS hasn't what this Mac has, and
//! when it ends, failed or not, if it added to them: a few large writes a run, never an answer at a
//! time (the NAS takes some 20–55 small files a second).
//!
//! Which copy counts, at the step's start (`Kept::sync`): this Mac's while the NAS's archive is the
//! one its files last matched (sent from here or taken from there: the mark beside them), so what it
//! fetched since goes to the NAS (a file of it deleted here since is taken back from it first);
//! else the NAS's, taken over this Mac's (another Mac ran the step since, or this one has none), any
//! other file of this Mac's kept and sent at the step's end; this Mac's alone while the NAS has none
//! (the first run seeds it), or has one that doesn't read whole (its zstd checksum). One Mac runs a
//! step at a time (the agent's claims and leases), so one writes an archive at a time.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
}

/// What `Kept::sync` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Synced {
    /// Neither has any.
    None,
    /// This Mac's are the NAS's.
    Same,
    /// This Mac's went to the NAS: it had none, or this Mac has fetched more since.
    Sent,
    /// The NAS's came here.
    Took,
}

impl Synced {
    pub fn words(&self) -> &'static str {
        match self {
            Synced::None => "none yet",
            Synced::Same => "the same here as on the NAS",
            Synced::Sent => "sent to the NAS",
            Synced::Took => "taken from the NAS",
        }
    }
}

/// A mark: what this Mac's files were when they last matched the NAS's archive.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Mark {
    /// The archive (hash16).
    archive: String,
    /// Each file by its name under `dir`: its hash16.
    files: BTreeMap<String, String>,
}

/// The NAS's archive, copied into the step's scratch folder.
struct Fetched {
    copy: PathBuf,
    hash: String,
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
            // (The NAS's is the archive this Mac's files last matched: they're it, or newer. A file of
            // it deleted here since is taken back from it, the others kept as they are.)
            if let Some(m) = mark.as_ref().filter(|m| m.archive == nas.hash && !here.is_empty()) {
                let gone: Vec<String> = m.files.keys().filter(|n| !here.contains_key(*n)).cloned().collect();
                let mut now = here.clone();
                if !gone.is_empty() {
                    self.unpack_over(&nas, scratch, Some(&gone))?;
                    now.extend(hashes(&self.dir, &gone)?);
                }
                if m.files == now {
                    return Ok(if gone.is_empty() { Synced::Same } else { Synced::Took });
                }
                self.send(&now, scratch)?;
                return Ok(Synced::Sent);
            }
            // (One damaged: this Mac's go in its place. A failure here taking a sound one fails the
            // step, the NAS's left as it is.)
            if let Err(e) = readable(&nas.copy) {
                eprintln!("answers: {} can't be read ({e:#})", self.nas.display());
                if here.is_empty() {
                    return Ok(Synced::None);
                }
                self.send(&here, scratch)?;
                return Ok(Synced::Sent);
            }
            self.take(&nas, scratch)?;
            Ok(Synced::Took)
        })();
        std::fs::remove_file(&nas.copy).ok();
        r
    }

    /// At the step's end, failed or not: this Mac's answers (`files`) sent to the NAS when they've
    /// changed since they last matched its archive; whether they were.
    pub fn keep(&self, files: &[String], scratch: &Path) -> Result<bool> {
        let here = hashes(&self.dir, files)?;
        if here.is_empty() || (self.read_mark().is_some_and(|m| m.files == here) && self.nas.is_file()) {
            return Ok(false);
        }
        self.send(&here, scratch)?;
        Ok(true)
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

    /// The NAS's archive, copied here whole and hashed (None when the NAS has none; one that can't be
    /// read now fails the step, to be tried again).
    fn fetch(&self, scratch: &Path) -> Result<Option<Fetched>> {
        match std::fs::metadata(&self.nas) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("stat {}", self.nas.display())),
            Ok(_) => {}
        }
        std::fs::create_dir_all(scratch)?;
        let copy = scratch.join(format!("nas-{}", self.name()));
        crate::whole::copy(&self.nas, &copy)?;
        let hash = store::naming::hash16_file(&copy)?;
        Ok(Some(Fetched { copy, hash }))
    }

    /// This Mac's files (`here`) to the NAS, as one archive written whole; the mark says so.
    fn send(&self, here: &BTreeMap<String, String>, scratch: &Path) -> Result<()> {
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

    /// The NAS's archive (its copy here) taken over this Mac's files; the mark says so.
    fn take(&self, nas: &Fetched, scratch: &Path) -> Result<()> {
        let names = self.unpack_over(nas, scratch, None)?;
        let files = hashes(&self.dir, &names)?;
        self.write_mark(&Mark { archive: nas.hash.clone(), files })
    }

    /// The NAS's archive (its copy here) unpacked over this Mac's files, or only those named
    /// `only`: whole into a folder of its own first, so one cut short changes nothing here. The
    /// names put in place.
    fn unpack_over(&self, nas: &Fetched, scratch: &Path, only: Option<&[String]>) -> Result<Vec<String>> {
        let tmp = scratch.join("answers-in");
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp)?;
        let r = (|| {
            unpack(&nas.copy, &tmp)?;
            let mut names = Vec::new();
            files_under(&tmp, &tmp, &mut names);
            names.retain(|n| only.is_none_or(|o| o.contains(n)));
            for n in &names {
                let to = self.dir.join(n);
                if let Some(d) = to.parent() {
                    std::fs::create_dir_all(d)?;
                }
                if std::fs::rename(tmp.join(n), &to).is_err() {
                    crate::whole::copy(&tmp.join(n), &to)?;
                }
            }
            Ok(names)
        })();
        std::fs::remove_dir_all(&tmp).ok();
        r
    }
}

/// Each of `names` under `dir` that's there, with its hash16.
fn hashes(dir: &Path, names: &[String]) -> Result<BTreeMap<String, String>> {
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
    Kept { nas: root.join(format!("sources/items/{date}/answers.tar.zst")), dir: dir.to_path_buf(), mark: dir.join(format!("kept-{date}.json")) }
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

/// The heritage chain's for pass `date`: its working copy `epoch` of the registers' snapshot `id`
/// (`registers-<id>`), its archive in the NAS's project folder `root`.
pub fn heritage(root: &Path, epoch: &Path, date: &str, id: &str) -> Kept {
    Kept { nas: root.join(format!("sources/items/{date}/heritage-{id}.tar.zst")), dir: epoch.to_path_buf(), mark: epoch.join(".kept.json") }
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
    fn what_a_mac_fetched_since_goes_up_and_a_newer_nas_wins() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("nas"), d.path().join("scratch"));
        let (m4, m1) = (d.path().join("m4"), d.path().join("m1"));
        put(&m4.join("facts-2026-09-28.jsonl"), "a\n");
        let k4 = items(&root, &m4, "2026-09-28");
        let k1 = items(&root, &m1, "2026-09-28");
        let files = |dir: &Path| items_files(dir, "2026-09-28");
        k4.sync(&files(&m4), &scratch).unwrap();
        // The step fetches more: sent at its end.
        put(&m4.join("facts-2026-09-28.jsonl"), "a\nb\n");
        put(&m4.join("wp-2026-09-28.jsonl"), "w\n");
        assert!(k4.keep(&files(&m4), &scratch).unwrap());
        assert_eq!(archived(&k4, &scratch)["facts-2026-09-28.jsonl"], "a\nb\n");
        // A run cut short before its end (no keep): sent at the next one's start.
        put(&m4.join("facts-2026-09-28.jsonl"), "a\nb\nc\n");
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Sent);
        assert_eq!(archived(&k4, &scratch)["facts-2026-09-28.jsonl"], "a\nb\nc\n");
        // The other Mac takes them and fetches more (the lead moved): the NAS's is newer than the
        // build Mac's, whose own since (d) gives way to it.
        assert_eq!(k1.sync(&files(&m1), &scratch).unwrap(), Synced::Took);
        put(&m1.join("facts-2026-09-28.jsonl"), "a\nb\nc\ne\n");
        assert!(k1.keep(&files(&m1), &scratch).unwrap());
        put(&m4.join("facts-2026-09-28.jsonl"), "a\nb\nc\nd\n");
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&m4.join("facts-2026-09-28.jsonl")), "a\nb\nc\ne\n");
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Same);
        // A file deleted here since (a cache cleared): taken from the NAS again, not sent without it.
        std::fs::remove_file(m4.join("wp-2026-09-28.jsonl")).unwrap();
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&m4.join("wp-2026-09-28.jsonl")), "w\n");
        assert_eq!(archived(&k4, &scratch).len(), 2);
        // And with another fetched more since, unsent: that one kept as it is, both sent.
        put(&m4.join("facts-2026-09-28.jsonl"), "a\nb\nc\ne\nf\n");
        std::fs::remove_file(m4.join("wp-2026-09-28.jsonl")).unwrap();
        assert_eq!(k4.sync(&files(&m4), &scratch).unwrap(), Synced::Sent);
        assert_eq!(read(&m4.join("facts-2026-09-28.jsonl")), "a\nb\nc\ne\nf\n");
        assert_eq!(read(&m4.join("wp-2026-09-28.jsonl")), "w\n");
        let a = archived(&k4, &scratch);
        assert_eq!((a["facts-2026-09-28.jsonl"].as_str(), a["wp-2026-09-28.jsonl"].as_str()), ("a\nb\nc\ne\nf\n", "w\n"));
    }

    #[test]
    fn an_archive_that_cant_be_read_gives_way_to_this_macs() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch, m4) = (d.path().join("nas"), d.path().join("scratch"), d.path().join("m4"));
        let nas = root.join("sources/items/2026-09-28/answers.tar.zst");
        put(&nas, "not an archive");
        put(&m4.join("facts-2026-09-28.jsonl"), "a\n");
        let k = items(&root, &m4, "2026-09-28");
        assert_eq!(k.sync(&items_files(&m4, "2026-09-28"), &scratch).unwrap(), Synced::Sent);
        assert_eq!(archived(&k, &scratch)["facts-2026-09-28.jsonl"], "a\n");
        // A byte of it changed since (its checksum): a Mac with none takes nothing, and changes
        // nothing here; one with its own sends them.
        let mut b = std::fs::read(&nas).unwrap();
        let n = b.len();
        b[n - 6] ^= 1;
        std::fs::write(&nas, &b).unwrap();
        let m1 = d.path().join("m1");
        assert_eq!(items(&root, &m1, "2026-09-28").sync(&[], &scratch).unwrap(), Synced::None);
        assert!(!m1.exists());
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
        let k = heritage(&root, &epoch, "2026-09-28", "7acb8655abb6");
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
        let k1 = heritage(&root, &other, "2026-09-28", "7acb8655abb6");
        assert_eq!(k1.sync(&heritage_files(&other, &snap), &scratch).unwrap(), Synced::Took);
        assert_eq!(read(&other.join("wd/ids.jsonl")), "wd/ids.jsonl and more");
        assert_eq!(read(&other.join("es/new-register.json")), "[]");
        assert_eq!(read(&other.join("whs/wd-p757.json")), "whs/wd-p757.json");
        assert_eq!(heritage_files(&other, &snap), files);
        // Its run adds nothing: nothing sent.
        assert!(!k1.keep(&heritage_files(&other, &snap), &scratch).unwrap());
    }
}
