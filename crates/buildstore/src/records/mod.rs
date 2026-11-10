//! The build's records as a reader finds them (docs/pool.md §6.2; task #916 step 6a,
//! docs/crates.md §6): the NAS's operations (`nas`), the current term (`term`), and which file holds
//! the records now: the one path's snapshot or brief while it's the current term's (`ONE`,
//! docs/coordination.md step 8b), else the current term's own, or before the pool today's files.
//! Written once, generic over what is read of a snapshot (`Snapshot`): the pool's whole records
//! (pipeline::pool::records::Records, with the agent's keys) or what an `Out` reads (`Recorded`).
//! Writing them (saving, merging, taking a term up) is the pool's.

pub mod member;
pub mod nas;
pub mod term;

use anyhow::{ensure, Context, Result};
use nas::Nas;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A member as the pool names it (§5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Its id, made once (member::member_id), named in everything it writes.
    pub id: String,
    /// Its host name: only a label (renaming a Mac, or macOS adding "-2" after a clash, changes
    /// nothing).
    pub host: String,
    /// The app it runs (§6.1, the app rule).
    pub app: String,
}

/// Whether `s` is a member id: `m-` and 16 lowercase hex digits.
pub fn is_member_id(s: &str) -> bool {
    s.strip_prefix("m-")
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Term 1's records in today's layout: the manifest, the job keys, the unverified uploads.
pub const TODAY: [&str; 3] = ["state/build/manifest.json", "state/build/jobs.json", "state/build/pending.json"];

/// The path of term 1's first snapshot (pipeline::pool::records::first), which only its maker
/// writes.
pub const FIRST: &str = "state/build/term/1/first.json";

/// The records' one path (docs/coordination.md step 8b): the lead's newest snapshot, whatever its
/// term, written after each save on its own disk. A reader reads it first and stats the term after
/// its own (`at_one`): a read and a stat, where the term's own took five calls.
pub const ONE: &str = "state/build/records.json";

/// The one path's brief (`BriefOf`): its snapshot's manifest and keys alone, written after it.
pub const ONE_BRIEF: &str = "state/build/brief.json";

/// The path of term `term`'s snapshot, written beside the one path by that term's lead alone: what
/// a late write landed over the one path can't hide (docs/coordination.md step 8b).
pub fn path(term: u64) -> String {
    format!("state/build/term/{term}/records.json")
}

/// The path of term `term`'s brief (`BriefOf`): its snapshot's manifest and keys alone (written
/// while a member heard reads it: the rollout gate of docs/coordination.md step 8b).
pub fn brief_path(term: u64) -> String {
    format!("state/build/term/{term}/brief.json")
}

/// What a reader reads of a term's snapshot.
pub trait Snapshot: Default + DeserializeOwned {
    /// The term it's of.
    fn term(&self) -> u64;
    /// Today's files read as term 1's records (numbered 0).
    fn today(nas: &dyn Nas) -> Result<Self>;
    /// Numbered `seq` in its term.
    fn with_seq(self, seq: u64) -> Self;
}

/// What a reader reads of a term's brief (a term after the first's snapshot's manifest and keys
/// alone, a quarter of its bytes: written whole by its lead after each snapshot), or of snapshot
/// `S` where there's none.
pub trait BriefOf<S>: DeserializeOwned {
    /// The term it's of.
    fn term(&self) -> u64;
    /// Snapshot `s`'s.
    fn of(s: S) -> Self;
    /// Today's files' (before the pool).
    fn today(nas: &dyn Nas) -> Result<Self>;
}

/// Refuses to read today's files as the records when the pool has begun (term 1's first snapshot
/// is there, never removed) but no term was found (term::current: the lead's hint, the terms'
/// listing and term 1 itself not read, stale): today's files may be days old once the lead stops
/// writing them, and GC's live set would be theirs.
fn no_pool(nas: &dyn Nas) -> Result<()> {
    ensure!(!nas.exists(FIRST)?, "the pool has begun and no term can be read now: the records aren't read");
    Ok(())
}

/// JSON file `p` parsed, or the default when there's none.
pub fn read_or_default<T: Default + DeserializeOwned>(nas: &dyn Nas, p: &str) -> Result<T> {
    match nas.read(p)? {
        Some(b) => serde_json::from_slice(&b).with_context(|| format!("parse {p}")),
        None => Ok(T::default()),
    }
}

/// Term `term`'s snapshot, as read now; None when it has none (its lead never saved one, or the
/// read is stale). An error when what's there can't be read now, or parsed.
pub fn load<S: Snapshot>(nas: &dyn Nas, term: u64) -> Result<Option<S>> {
    let Some(b) = nas.read(&path(term))? else { return Ok(None) };
    let r: S = serde_json::from_slice(&b).with_context(|| format!("parse {}", path(term)))?;
    ensure!(r.term() == term, "{} holds term {}'s records", path(term), r.term());
    Ok(Some(r))
}

/// The records a reader (a job, the map server, a catalogue) has while term `term` is current and
/// the one path holds `one` (its snapshot, behind the current term: a term after it being taken
/// up, its lead on an app before the one path, or an earlier term's save landed there late): the
/// current term's own snapshot, or while it has none yet (its lead taking it up, or gone before
/// saving one) the newest of a term before it, walking down only to `one`'s term, which counts as
/// its term's and the earlier terms' (the newest of a term above it is its own snapshot, which
/// only that term's lead writes); with no `one`, down to term 1, as `term_one` says (`today` as it
/// says). None before any.
pub fn newest_over<S: Snapshot>(nas: &dyn Nas, term: u64, one: Option<S>, today: bool) -> Result<Option<S>> {
    let floor = one.as_ref().map_or(2, |o| o.term() + 1).max(2);
    for f in (floor..=term).rev() {
        if let Some(r) = load(nas, f)? {
            return Ok(Some(r));
        }
    }
    if one.is_some() || term == 0 {
        return Ok(one);
    }
    term_one(nas, today).map(Some)
}

/// What's at one path `p` (`ONE`, `ONE_BRIEF`), parsed, and whether it's the current term's: no
/// term after its own made (a stat, `term::next`). None: nothing there (no lead on an app that
/// writes it yet).
fn at_one<T: DeserializeOwned>(nas: &dyn Nas, p: &str, term_of: impl Fn(&T) -> u64) -> Result<Option<(T, bool)>> {
    let Some(b) = nas.read(p)? else { return Ok(None) };
    let r: T = serde_json::from_slice(&b).with_context(|| format!("parse {p}"))?;
    let current = !term::next(nas, term_of(&r))?;
    Ok(Some((r, current)))
}

/// The one path's snapshot (`ONE`) as read now, whatever its term; None when there's none: the
/// take-up of a lead whose Mac led no term before it reads it (pipeline::pool::records::start).
pub fn load_one<S: Snapshot>(nas: &dyn Nas) -> Result<Option<S>> {
    let Some(b) = nas.read(ONE)? else { return Ok(None) };
    Ok(Some(serde_json::from_slice(&b).with_context(|| format!("parse {ONE}"))?))
}

/// The build's records as a reader has them now (§6.2): the one path's while no term after its own
/// is made (`ONE`: a read and a stat); else the current term's snapshot, or while it has none the
/// newest of a term before it, down to the one path's (`newest_over`); before the pool's first
/// term, today's files (a Mac's own records, its jobs saving them: crate::out::Out::save_held). A
/// reader never reads today's files beside a term's snapshot.
pub fn now<S: Snapshot>(nas: &dyn Nas) -> Result<S> {
    let one = match at_one::<S>(nas, ONE, S::term)? {
        Some((r, true)) => return Ok(r),
        Some((r, false)) => Some(r),
        None => None,
    };
    let cur = term::current(nas).context("the current term")?;
    if cur.term == 0 && one.is_none() {
        no_pool(nas)?;
        return S::today(nas);
    }
    Ok(newest_over(nas, cur.term, one, true)?.unwrap_or_default())
}

/// `now`'s brief (`B`): the one path's (`ONE_BRIEF`) while no term after its own is made; else a
/// term's brief where it has one (its snapshot where it has none: a lead on an app from before
/// briefs, or one stopped between the two writes), down to the one path's; today's files before
/// the pool. One read pairs keys and manifest of one version (invariant 4).
pub fn brief_now<S: Snapshot, B: BriefOf<S>>(nas: &dyn Nas) -> Result<B> {
    let one = match at_one::<B>(nas, ONE_BRIEF, B::term)? {
        Some((b, true)) => return Ok(b),
        Some((b, false)) => Some(b),
        None => None,
    };
    let cur = term::current(nas).context("the current term")?;
    if cur.term == 0 && one.is_none() {
        no_pool(nas)?;
        return B::today(nas);
    }
    let floor = one.as_ref().map_or(2, |o| o.term() + 1).max(2);
    for f in (floor..=cur.term).rev() {
        if let Some(b) = nas.read(&brief_path(f))? {
            let b: B = serde_json::from_slice(&b).with_context(|| format!("parse {}", brief_path(f)))?;
            ensure!(b.term() == f, "{} holds term {}'s brief", brief_path(f), b.term());
            return Ok(b);
        }
        if let Some(r) = load::<S>(nas, f)? {
            return Ok(B::of(r));
        }
    }
    match one {
        Some(b) => Ok(b),
        None => Ok(B::of(term_one::<S>(nas, true)?)),
    }
}

/// Term 1's records as a take-up or a reader starts from them: its lead's newest snapshot; while it
/// has none, its first (`FIRST`); while that can't be read whole either (its maker stopped midway,
/// or its bytes landed out of order; or a stale read), today's three files once it has stayed so
/// longer than any stale read (`today`: the caller's to say), no lead having written them since
/// (term 1's writes them after its snapshot, and none is there), else `FirstUnreadable`.
pub fn term_one<S: Snapshot>(nas: &dyn Nas, today: bool) -> Result<S> {
    if let Some(r) = load(nas, 1)? {
        return Ok(r);
    }
    match nas.read(FIRST)?.map(|b| serde_json::from_slice::<S>(&b)) {
        Some(Ok(r)) if r.term() == 1 => Ok(r),
        _ if today => Ok(S::today(nas)?.with_seq(1)),
        _ => Err(FirstUnreadable.into()),
    }
}

/// `term_one`'s error while term 1 has no snapshot that reads whole: tried again, until it has
/// stayed so longer than any stale read (`today`).
#[derive(Debug)]
pub struct FirstUnreadable;

impl std::fmt::Display for FirstUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "term 1's first records can't be read whole yet: try again shortly")
    }
}

impl std::error::Error for FirstUnreadable {}

/// The parts of the build's records an `Out` reads (crate::out::Out::open_with,
/// `recorded_sizes`).
#[derive(Default, Deserialize)]
pub(crate) struct Recorded {
    /// The term they're of.
    pub term: u64,
    /// Logical name → content name.
    pub manifest: BTreeMap<String, String>,
    /// Uploads not yet verified on the NAS: content name → SHA-256.
    pub pending: BTreeMap<String, String>,
    /// The sizes of the files the manifest names (crate::out::SIZES).
    #[serde(default)]
    pub sizes: BTreeMap<String, u64>,
}

impl Snapshot for Recorded {
    fn term(&self) -> u64 {
        self.term
    }

    /// The pending read before the manifest, as pipeline::pool::records::Records::today reads them.
    fn today(nas: &dyn Nas) -> Result<Recorded> {
        let pending = read_or_default(nas, TODAY[2])?;
        Ok(Recorded {
            term: 1,
            manifest: read_or_default(nas, TODAY[0])?,
            pending,
            sizes: read_or_default(nas, crate::out::SIZES)?,
        })
    }

    fn with_seq(self, _: u64) -> Self {
        self
    }
}

/// The manifest alone of a brief or a snapshot (`read_manifest`).
#[derive(Deserialize)]
struct Listed {
    term: u64,
    manifest: BTreeMap<String, String>,
}

impl BriefOf<Recorded> for Listed {
    fn term(&self) -> u64 {
        self.term
    }

    fn of(r: Recorded) -> Listed {
        Listed { term: r.term, manifest: r.manifest }
    }

    fn today(nas: &dyn Nas) -> Result<Listed> {
        Ok(Listed { term: 1, manifest: read_or_default(nas, TODAY[0])? })
    }
}

/// The records' manifest as a reader has it now (`brief_now`: the current term's brief, a quarter of
/// its snapshot's bytes), read only: the accepted versions, for a reader that needs no `Out`.
pub fn read_manifest(nas: &dyn Nas) -> Result<BTreeMap<String, String>> {
    Ok(brief_now::<Recorded, Listed>(nas)?.manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::nasfs::NasRoot;

    /// A reader finds the current term's brief, else its snapshot, else today's files before the
    /// pool: the manifest and what an `Out` reads of the snapshot alike.
    #[test]
    fn a_reader_finds_the_current_terms_records() {
        let d = tempfile::tempdir().unwrap();
        let nas = NasRoot::local_for_test(d.path());
        let write = |p: &str, b: &str| {
            let p = d.path().join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b).unwrap();
        };
        write(TODAY[0], r#"{"a": "a.1"}"#);
        write(TODAY[2], r#"{"a.1": "sha"}"#);
        assert_eq!(read_manifest(&nas).unwrap().len(), 1);
        let r: Recorded = now(&nas).unwrap();
        assert_eq!((r.term, r.manifest.len(), r.pending.len()), (1, 1, 1));

        let term = |e: u64| {
            format!(
                r#"{{"term":{e},"member":"m-000000000000000a","host":"h","app":"x","since":1,"how":"h","from":{}}}"#,
                e - 1
            )
        };
        write(&term::path(1), &term(1));
        write(&term::path(2), &term(2));
        write(FIRST, r#"{"term":1,"seq":1,"manifest":{},"keys":{},"pending":{}}"#);
        write(
            &path(2),
            r#"{"term":2,"seq":3,"manifest":{"b":"b.1","c":"c.1"},"keys":{"x":1},"pending":{},"sizes":{"b.1":7},"reflected":[]}"#,
        );
        let r: Recorded = now(&nas).unwrap();
        assert_eq!((r.term, r.manifest.len(), r.sizes.get("b.1")), (2, 2, Some(&7)));
        assert_eq!(read_manifest(&nas).unwrap().len(), 2, "its snapshot, with no brief");
        write(&brief_path(2), r#"{"term":2,"seq":3,"manifest":{"b":"b.1"},"keys":{}}"#);
        assert_eq!(read_manifest(&nas).unwrap().len(), 1, "its brief");
        write(&brief_path(2), r#"{"term":1,"seq":3,"manifest":{},"keys":{}}"#);
        assert!(read_manifest(&nas).is_err(), "another term's");
    }

    /// A NAS whose calls are counted (reads, stats, listings).
    struct Counted<'a>(&'a NasRoot, std::cell::Cell<u64>);

    impl Nas for Counted<'_> {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<nas::Created> {
            self.0.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.0.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.1.set(self.1.get() + 1);
            self.0.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.1.set(self.1.get() + 1);
            self.0.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.1.set(self.1.get() + 1);
            self.0.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.0.remove(path)
        }
    }

    /// docs/coordination.md step 8b: a reader reads the one path and stats the term after its own,
    /// two NAS calls where the current term's own took five (the lead's hint read, its term stat'd,
    /// the next stat'd, the term read, its snapshot read). Behind the current term (a term after it
    /// being taken up, a lead on an older app, an earlier save landed late), it walks the terms down
    /// to the one path's term alone, and reads the one path's where they have none.
    #[test]
    fn a_reader_reads_the_one_path_and_stats_the_next_term() {
        let d = tempfile::tempdir().unwrap();
        let root = NasRoot::local_for_test(d.path());
        let nas = Counted(&root, Default::default());
        let write = |p: &str, b: &str| root.write_whole(p, b.as_bytes()).unwrap();
        let term = |e: u64| {
            format!(
                r#"{{"term":{e},"member":"m-000000000000000a","host":"h","app":"x","since":1,"how":"h","from":{}}}"#,
                e - 1
            )
        };
        let snap = |e: u64, seq: u64, c: &str| {
            format!(r#"{{"term":{e},"seq":{seq},"manifest":{{"a":"{c}"}},"pending":{{}},"sizes":{{}}}}"#)
        };
        let brief = |e: u64, c: &str| format!(r#"{{"term":{e},"manifest":{{"a":"{c}"}}}}"#);
        write(FIRST, r#"{"term":1,"seq":1,"manifest":{},"keys":{},"pending":{}}"#);
        for e in 1..=3 {
            write(&term::path(e), &term(e));
        }
        write(term::HINT, &term(3));
        write(&path(3), &snap(3, 4, "a.3"));
        write(&brief_path(3), &brief(3, "a.3"));
        // Before (a lead on an app without the one path): five calls past the one path's read.
        let calls = |f: &dyn Fn() -> String| {
            nas.1.set(0);
            (f(), nas.1.get())
        };
        let manifest = || now::<Recorded>(&nas).unwrap().manifest["a"].clone();
        let briefed = || read_manifest(&nas).unwrap()["a"].clone();
        assert_eq!(calls(&manifest), ("a.3".into(), 6), "the one path read, then the term's own five");
        // After: the one path, current.
        write(ONE, &snap(3, 5, "a.3.5"));
        write(ONE_BRIEF, &brief(3, "a.3.5"));
        assert_eq!(calls(&manifest), ("a.3.5".into(), 2));
        assert_eq!(calls(&briefed), ("a.3.5".into(), 2));
        // Term 4 made and not saved yet: the one path's still, no walk past it.
        write(&term::path(4), &term(4));
        assert_eq!(calls(&manifest).0, "a.3.5");
        assert_eq!(calls(&briefed).0, "a.3.5");
        // Its own saved (its lead on an older app): its own.
        write(&path(4), &snap(4, 1, "a.4"));
        write(&brief_path(4), &brief(4, "a.4"));
        assert_eq!(calls(&manifest).0, "a.4");
        assert_eq!(calls(&briefed).0, "a.4");
        // An earlier term's save landed late over the one path: the term's own while it's written.
        write(ONE, &snap(2, 9, "a.2"));
        write(ONE_BRIEF, &brief(2, "a.2"));
        assert_eq!(calls(&manifest).0, "a.4");
        assert_eq!(calls(&briefed).0, "a.4");
    }
}
