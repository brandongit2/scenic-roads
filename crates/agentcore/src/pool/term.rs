//! Terms (docs/pool.md §6.1): the fixed lead's runs. The lead is declared once, in
//! `state/build/lead-member.json` (`declared`): the Mac mini's member id. Only the member it names
//! ever makes a term: term 1, then a re-assertion after each restart, sleep or stall. A term is a
//! file made once with create-new and never changed, `state/build/terms/<E>.json`; the current term
//! is the highest that exists. No member makes a term, and none takes the lead from the declared
//! one: with it gone, the members wait. Reading them (`Term`, `current`, `next`, the app rule) is
//! buildstore::records::term's (task #916 step 6a, docs/crates.md §6), as are the declared lead and
//! this Mac's role by it (step 7a: the map server reads it), kept at this path.

use super::nas::{Created, Nas};
use super::Member;
use anyhow::{ensure, Context, Result};

pub use buildstore::records::term::*;

/// What making a term came to (`make`).
#[derive(Debug)]
pub enum Made {
    /// This Mac's: its create made the file (in this call, or an earlier one whose answer was
    /// lost), and its bytes landed.
    Ours,
    /// Another Mac's create made it first.
    Theirs,
    /// This Mac's create made it but its bytes haven't landed, nor could it finish them: no Mac
    /// reads the term whole, and none can make the next, until this one finishes it (`finish`);
    /// why.
    Unfinished(anyhow::Error),
}

/// Whether `a` and `b` are one term but for when they were made: a try of this Mac's, its answer
/// lost, and the try after it.
fn same(a: &Term, b: &Term) -> bool {
    Term { since: 0, ..a.clone() } == Term { since: 0, ..b.clone() }
}

/// Makes term `t` with create-new: whose it is. Term 1 only once its first snapshot is made
/// (crate::pool::records::first). A create that made the file and couldn't write its bytes has
/// them written whole over it (no other Mac's create of the name can succeed); an earlier try
/// whose answer was lost is known by the term it reads (the same but for `since`). (One whose
/// answer was lost before its bytes landed can't be told from another Mac's still being written:
/// it reads as another's, and the term has no lead until the owner's `force`.)
pub fn make(nas: &dyn Nas, t: &Term) -> Result<Made> {
    ensure!(t.term >= 1 && t.from + 1 == t.term, "term {} can't follow term {}", t.term, t.from);
    ensure!(t.term > 1 || nas.exists(super::records::FIRST)?, "term 1's first snapshot isn't made yet");
    let b = serde_json::to_vec_pretty(t)?;
    Ok(match nas.create_new(&path(t.term), &b).with_context(|| format!("make term {}", t.term))? {
        Created::Made => Made::Ours,
        Created::There => match read(nas, t.term)? {
            Some(got) if same(&got, t) => Made::Ours,
            _ => Made::Theirs,
        },
        Created::Unwritten(e) => match finish(nas, t) {
            Ok(()) => Made::Ours,
            Err(f) => Made::Unfinished(f.context(format!("{e:#}"))),
        },
    })
}

/// Writes term `t`'s bytes whole over the file this Mac's create made and couldn't fill
/// (`Made::Unfinished`): only its maker may. Nothing when it reads whole already (an earlier
/// finish whose answer was lost); the same bytes again when a stale read hides that.
pub fn finish(nas: &dyn Nas, t: &Term) -> Result<()> {
    if read(nas, t.term)?.is_some_and(|got| same(&got, t)) {
        return Ok(());
    }
    // (Term E+1 made past it, it's left as it is: no Mac may lead it now.)
    ensure!(!next(nas, t.term)?, "term {} was made past term {}: not finished", t.term + 1, t.term);
    nas.write_whole(&path(t.term), &serde_json::to_vec_pretty(t)?).with_context(|| format!("finish term {}", t.term))
}

/// Makes the term after `cur` naming `me` (a re-assertion): the term when this Mac's create won,
/// None when another's did; an error when the term is made but unfinished (`make` keeps the term,
/// to finish it).
pub fn claim(nas: &dyn Nas, cur: &Current, me: &Member, how: &str, now: u64) -> Result<Option<Term>> {
    let t = Term::after(cur, me, how, now)?;
    match make(nas, &t)? {
        Made::Ours => Ok(Some(t)),
        Made::Theirs => Ok(None),
        Made::Unfinished(e) => Err(e),
    }
}

/// Term 1 (§12, phase 1), where no term was made yet: made by the declared lead, or in a new pool
/// with no lead declared by the first Mac in it, naming itself (which then declares itself:
/// crate::pool::driver); its first snapshot first, from today's files
/// (crate::pool::records::first), both with create-new, so of Macs making it at once one makes
/// each. The term once it's made (by this Mac or another); None while it's being made.
pub fn bootstrap(nas: &dyn Nas, me: &Member, now: u64) -> Result<Option<Term>> {
    if nas.exists(&path(1))? {
        return read(nas, 1);
    }
    let how = "the first Mac in the pool".to_string();
    super::records::first(nas)?;
    let t =
        Term { term: 1, member: me.id.clone(), host: me.host.clone(), app: me.app.clone(), since: now, how, from: 0 };
    match make(nas, &t)? {
        Made::Ours => Ok(Some(t)),
        Made::Theirs => read(nas, 1),
        Made::Unfinished(e) => Err(e.context(Unfinished(t))),
    }
}

/// A term this Mac's create made and couldn't fill (term 1's, `bootstrap`): its maker's to finish
/// (`finish`), in the error's context.
#[derive(Debug)]
pub struct Unfinished(pub Term);

impl std::fmt::Display for Unfinished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "term {} made, its bytes not written", self.0.term)
    }
}

/// Writes the lead's hint, `state/build/lead.json` (§6.1): never the truth (a stale lead may write
/// it late; `current` checks upward from it).
pub fn write_hint(nas: &dyn Nas, t: &Term) -> Result<()> {
    nas.write_whole(HINT, &serde_json::to_vec_pretty(t)?)
}

#[cfg(test)]
mod tests {
    use super::super::nas::Mem;
    use super::*;
    use anyhow::bail;

    fn member(id: &str, app: &str) -> Member {
        Member { id: id.into(), host: format!("{id}-host"), app: app.into() }
    }

    /// Task #882 step 1: the role is the declaration against this Mac's member id; either unread,
    /// it's unknown.
    #[test]
    fn role_reads_the_declared_lead() {
        let nas = Mem::default();
        let (a, b) = (member("m-000000000000000a", "x"), member("m-000000000000000b", "x"));
        assert_eq!(role(&nas, Some(&a.id)), Role::Unknown, "none declared");
        declare(&nas, &a, 100).unwrap();
        assert_eq!(role(&nas, Some(&a.id)), Role::Lead);
        assert_eq!(role(&nas, Some(&b.id)), Role::Member);
        assert_eq!(role(&nas, None), Role::Unknown, "no member file yet");
        // (Being made: no bytes yet.)
        let empty = Mem::default();
        empty.create_new(LEAD, b"").unwrap();
        assert_eq!(role(&empty, Some(&a.id)), Role::Unknown);
    }

    #[test]
    fn the_current_term_is_the_highest_found_by_one_listing_then_checks_upward() {
        let nas = Mem::default();
        assert_eq!(current(&nas).unwrap(), Current::default());
        let a = member("m-000000000000000a", "20261005-2202-61eb22c");
        let t1 = bootstrap(&nas, &a, 100).unwrap().unwrap();
        let t2 = claim(&nas, &current(&nas).unwrap(), &a, "re-asserted after a gap", 200).unwrap().unwrap();
        assert_eq!(current(&nas).unwrap(), Current { term: 2, lead: Some(t2.clone()) });
        // A hint behind the truth is checked upward from; one naming no term is passed over.
        write_hint(&nas, &t1).unwrap();
        assert_eq!(current(&nas).unwrap().term, 2);
        nas.write_whole(HINT, &serde_json::to_vec(&Term { term: 9, ..t1.clone() }).unwrap()).unwrap();
        assert_eq!(current(&nas).unwrap().term, 2);
        // A term made, its bytes not landed yet: current, its lead unknown.
        nas.create_new(&path(3), b"").unwrap();
        assert_eq!(current(&nas).unwrap(), Current { term: 3, lead: None });
        assert!(next(&nas, 2).unwrap() && !next(&nas, 3).unwrap());
    }

    #[test]
    fn of_two_macs_making_a_term_one_wins_and_a_retry_knows_its_own() {
        let nas = Mem::default();
        let (a, b) = (
            member("m-000000000000000a", "20261005-2202-61eb22c"),
            member("m-000000000000000b", "20261005-2202-61eb22c"),
        );
        // Term 1 only by bootstrap, its records first.
        assert!(Term::after(&Current::default(), &a, "taken over", 100).is_err());
        let t1 = Term {
            term: 1,
            member: a.id.clone(),
            host: a.host.clone(),
            app: a.app.clone(),
            since: 100,
            how: "by hand".into(),
            from: 0,
        };
        assert!(make(&nas, &t1).is_err() && !nas.exists(&path(1)).unwrap());
        let t1 = bootstrap(&nas, &a, 100).unwrap().unwrap();
        let cur = current(&nas).unwrap();
        let ta = Term::after(&cur, &a, "taken over", 200).unwrap();
        let tb = Term::after(&cur, &b, "taken over", 200).unwrap();
        assert!(matches!(make(&nas, &ta).unwrap(), Made::Ours));
        assert!(matches!(make(&nas, &tb).unwrap(), Made::Theirs), "b's create lost");
        assert!(matches!(make(&nas, &ta).unwrap(), Made::Ours), "a's own, tried again (its first call's answer lost)");
        assert!(
            matches!(make(&nas, &Term { since: 230, ..ta.clone() }).unwrap(), Made::Ours),
            "tried again a moment later"
        );
        assert_eq!(read(&nas, 2).unwrap(), Some(ta.clone()));
        assert_eq!(read(&nas, 1).unwrap(), Some(t1));
        // A term can only follow the one before it.
        assert!(make(&nas, &Term { term: 4, ..ta.clone() }).is_err());
        // A file under another term's name is an error.
        nas.create_new(&path(3), &serde_json::to_vec(&ta).unwrap()).unwrap();
        assert!(read(&nas, 3).is_err());
    }

    #[test]
    fn term_one_is_the_first_macs_to_make() {
        let nas = Mem::default();
        let (m4, m1) = (
            Member { id: "m-0000000000000004".into(), host: "Mac-mini".into(), app: "development".into() },
            Member { id: "m-0000000000000001".into(), host: "MacBook-Air".into(), app: "development".into() },
        );
        nas.write_whole("state/build/manifest.json", br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        let t = bootstrap(&nas, &m4, 11).unwrap().unwrap();
        assert_eq!((t.term, t.member.as_str(), t.from), (1, "m-0000000000000004", 0));
        assert_eq!(bootstrap(&nas, &m1, 12).unwrap(), Some(t), "then every Mac sees it");
        // Its first snapshot made first, from today's files.
        let r = super::super::Records::newest(&nas, 1, false).unwrap().unwrap();
        assert_eq!((r.term, r.seq, r.manifest.len()), (1, 1, 1));
    }

    /// A NAS whose next create makes the file and then fails before its bytes (the share went away
    /// between the two), or makes it whole and loses its answer; and whose whole writes fail while
    /// `down`.
    #[derive(Default)]
    struct CutCreate {
        mem: Mem,
        cut: std::cell::Cell<bool>,
        lost_answer: std::cell::Cell<bool>,
        down: std::cell::Cell<bool>,
    }

    impl Nas for CutCreate {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            if self.cut.replace(false) {
                self.mem.create_new(path, b"")?;
                return Ok(Created::Unwritten(anyhow::anyhow!("write {path} (made, and left short)")));
            }
            if self.lost_answer.replace(false) {
                self.mem.create_new(path, bytes)?;
                bail!("create {path}: the answer was lost");
            }
            self.mem.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            ensure!(!self.down.get(), "write {path}: the share is away");
            self.mem.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.mem.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.mem.exists(path)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.mem.list(dir)
        }
        fn remove(&self, path: &str) -> Result<()> {
            self.mem.remove(path)
        }
    }

    #[test]
    fn a_create_cut_short_is_its_makers_to_finish() {
        // A lead re-asserting as the NAS comes back (what a gap is): its create made term 2, and
        // its bytes failed. (Review M4: its next try read the term as another's, and no Mac could
        // follow it but by the owner's force.)
        let nas = CutCreate::default();
        let (a, b) = (member("m-000000000000000a", "development"), member("m-000000000000000b", "development"));
        let t1 = bootstrap(&nas, &a, 100).unwrap().unwrap();
        let cur = Current { term: 1, lead: Some(t1) };
        let t2 = Term::after(&cur, &a, "re-asserted after a gap", 200).unwrap();
        // Finished at once when the share takes the whole write.
        nas.cut.set(true);
        assert!(matches!(make(&nas, &t2).unwrap(), Made::Ours));
        assert_eq!(current(&nas).unwrap(), Current { term: 2, lead: Some(t2.clone()) }, "read whole by every Mac");
        // Not when the share is away: unfinished, until its maker finishes it.
        let t3 = Term::after(&current(&nas).unwrap(), &a, "re-asserted after a gap", 300).unwrap();
        nas.cut.set(true);
        nas.down.set(true);
        assert!(matches!(make(&nas, &t3).unwrap(), Made::Unfinished(_)));
        assert_eq!(current(&nas).unwrap(), Current { term: 3, lead: None });
        assert!(
            claim(&nas, &current(&nas).unwrap(), &b, "taken over by b-host", 900).is_err(),
            "no Mac follows it meanwhile"
        );
        nas.down.set(false);
        finish(&nas, &t3).unwrap();
        assert_eq!(current(&nas).unwrap(), Current { term: 3, lead: Some(t3) });
    }

    #[test]
    fn a_retried_claim_knows_its_own_term() {
        // The create made the term whole and its answer was lost: the retry, a moment later, reads
        // the term (its `since` another) as its own. (Review M4: it read it as another's.)
        let nas = CutCreate::default();
        let a = member("m-000000000000000a", "development");
        let t1 = bootstrap(&nas, &a, 100).unwrap().unwrap();
        let cur = Current { term: 1, lead: Some(t1) };
        nas.lost_answer.set(true);
        assert!(claim(&nas, &cur, &a, "re-asserted after a gap", 200).is_err());
        let t2 =
            claim(&nas, &cur, &a, "re-asserted after a gap", 230).unwrap().expect("its own, made by its first try");
        assert_eq!((t2.term, t2.since), (2, 230));
        assert_eq!(read(&nas, 2).unwrap().map(|t| (t.member, t.since)), Some((a.id.clone(), 200)));
        // Another Mac's term, the same but for its lead, is another's.
        let b = member("m-000000000000000b", "development");
        assert_eq!(claim(&nas, &cur, &b, "re-asserted after a gap", 230).unwrap(), None);
    }

    #[test]
    fn the_lead_is_declared_once() {
        let nas = Mem::default();
        let (mini, m4) = (member("m-000000000000000a", "development"), member("m-000000000000000b", "development"));
        assert_eq!(declared(&nas).unwrap(), None);
        let d = declare(&nas, &mini, 100).unwrap().unwrap();
        assert_eq!((d.member.as_str(), d.since), (mini.id.as_str(), 100));
        assert_eq!(declare(&nas, &m4, 200).unwrap(), Some(d), "a later declaration doesn't replace it");
        // The owner's edit, by hand, moves it.
        nas.write_whole(LEAD, br#"{"member": "m-000000000000000b"}"#).unwrap();
        assert_eq!(declared(&nas).unwrap().map(|d| d.member), Some(m4.id));
        // One being made (empty) declares no one yet.
        let fresh = Mem::default();
        fresh.create_new(LEAD, b"").unwrap();
        assert_eq!(declared(&fresh).unwrap(), None);
    }
}
