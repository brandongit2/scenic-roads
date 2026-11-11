//! The records' brief as an agent reads it (docs/coordination.md step 8d, task #1037): from the
//! lead, not the NAS. The lead's agent reads the brief its coordinator serves, in process
//! (crate::coord::Coordinator::brief); a member's asks the lead's coordinator over HTTP
//! (`GET /pool/brief`, crate::coord::brief), naming the one it holds (`If-None-Match`), so an
//! unchanged brief is a 304 of a few hundred bytes, and holds the last one it had (`Briefs`).
//!
//! Who reads what:
//! - **A job** (its `Out`, buildstore::out::BRIEF_ENV): the brief as the lead has it as the job
//!   starts (`Briefs::fresh`), written to a local file its agent names: never older than the records
//!   its lease was granted from, so it never reuses a name GC freed before the grant (the merge's
//!   refusal, crate::agent::gcqueue::refusal). The lead not answering: the job doesn't start, its
//!   lease given back (`Fresh::Away`): a stale copy (its last, or the NAS's, which an SMB cache can
//!   hold for minutes) is what such a refusal comes from.
//! - **The agent's views and plan** (`Briefs::now`: the pool's view, a job's command, the lead's
//!   plan and checklist): the lead's now; the lead not answering, the last one had, else the NAS's.
//! - **The NAS's copy** (`state/build/brief.json`), still written by the lead on every save: read
//!   when the lead's app serves no brief (an app before this step: `Fresh::Nas`), no lead is known,
//!   the lead has saved none yet in its process; and always by the map server and the CLI.

use crate::coord::brief::{Held, PATH};
use crate::pool::records::Brief;
use anyhow::Result;
use std::sync::{Arc, Mutex};

/// Where the brief comes from.
pub enum Lead<'a> {
    /// This Mac's coordinator: it leads.
    Here(&'a crate::coord::Coordinator),
    /// The lead's coordinator, over HTTP: a member's client to it.
    At(&'a crate::coord::client::Client),
    /// No lead known (out of the pool, or no contact yet).
    None,
}

/// What an ask of the lead found (`Briefs::fresh`).
#[derive(Debug)]
pub enum Fresh {
    /// The lead's brief now.
    Got(Held),
    /// The NAS's is the one to read, as before this step: why (the lead's app serves none, no lead
    /// known, the lead has saved none in this process yet).
    Nas(String),
    /// The lead didn't answer: why.
    Away(String),
}

/// A member's last brief from its lead, held between asks.
#[derive(Default)]
pub struct Briefs {
    held: Mutex<Option<Held>>,
    /// One ask of the lead at a time: a second asker waits for it and asks naming what it got.
    asking: Mutex<()>,
}

impl Briefs {
    /// The lead's brief now: asked of `lead`, naming the one held (a 304 keeps it).
    pub fn fresh(&self, lead: Lead<'_>) -> Fresh {
        let c = match lead {
            Lead::Here(c) => {
                return match c.brief() {
                    Some(h) => Fresh::Got(h),
                    None => Fresh::Nas("this Mac's coordinator has saved no brief yet in this process".into()),
                }
            }
            Lead::At(c) => c,
            Lead::None => return Fresh::Nas("no lead known".into()),
        };
        let _one = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        let held = self.held();
        let tag = held.as_ref().map(Held::tag);
        match c.get_tagged(PATH, tag.as_deref()) {
            Ok((200, b, _)) => match Held::parse(b) {
                Ok(h) => {
                    *self.held.lock().unwrap_or_else(|e| e.into_inner()) = Some(h.clone());
                    Fresh::Got(h)
                }
                Err(e) => Fresh::Away(format!("its brief can't be read: {e:#}")),
            },
            Ok((304, _, _)) => match held {
                Some(h) => Fresh::Got(h),
                None => Fresh::Away("it answered 304 to no brief".into()),
            },
            // (An app before this step: no such route.)
            Ok((404, _, _)) => Fresh::Nas("the lead's app serves no brief".into()),
            Ok((503, _, _)) => Fresh::Nas("the lead has saved no brief yet in its process".into()),
            Ok((code, b, _)) => Fresh::Away(format!("it answered {code}: {}", String::from_utf8_lossy(&b))),
            Err(e) => Fresh::Away(format!("{e:#}")),
        }
    }

    /// The brief to show and plan by: the lead's now (`fresh`); the lead not answering, the last
    /// one had from it, else the NAS's; the NAS's when that's the one (`Fresh::Nas`). An error when
    /// none can be read now.
    pub fn now(&self, lead: Lead<'_>, root: &store::nasfs::NasRoot) -> Result<Arc<Brief>> {
        match self.fresh(lead) {
            Fresh::Got(h) => Ok(h.brief),
            Fresh::Away(_) if self.held().is_some() => Ok(self.held().expect("held").brief),
            Fresh::Away(_) | Fresh::Nas(_) => on_nas(root).map(Arc::new),
        }
    }

    /// The last brief had from the lead over HTTP.
    pub fn held(&self) -> Option<Held> {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The NAS's brief (crate::pool::records::Brief::now), read within `nasread::DEADLINE` (a call named
/// by its one path).
pub fn on_nas(root: &store::nasfs::NasRoot) -> Result<Brief> {
    let r = root.clone();
    super::nasread::with(&root.join(crate::pool::records::ONE_BRIEF), move || {
        Brief::now(&r).map_err(std::io::Error::other)
    })
    .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::client::Client;
    use crate::pool::records::Records;

    fn records(seq: u64) -> Records {
        let mut r = Records { epoch: 849, seq, ..Default::default() };
        r.manifest.insert("base/6-1-1".into(), format!("base/6-1-1.{seq:016}.base"));
        r
    }

    /// A NAS root on which the pool has begun and no brief can be read: a reader of the NAS's fails.
    fn no_brief_on(d: &std::path::Path) -> store::nasfs::NasRoot {
        std::fs::create_dir_all(d.join("state/build")).unwrap();
        std::fs::write(d.join(buildstore::records::lead::LEAD), b"{}").unwrap();
        store::nasfs::NasRoot::local_for_test(d)
    }

    /// docs/coordination.md step 8d: a member has its lead's brief by HTTP, the one it holds named
    /// (304 while unchanged, kept as it is), a newer save's once there is one, the same as the
    /// lead's agent has in process. The lead not answering: `Away` (a job doesn't start), the views
    /// keep the last one had, else the NAS's. A lead on an older app (no such route: 404), or one
    /// with none saved yet (503), or none known: the NAS's, as before.
    #[test]
    fn a_member_has_the_leads_brief_by_its_tag() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        let token = c.contact.token.clone();
        let lead = Client::at(vec![format!("http://127.0.0.1:{port}")], token.clone(), "m1");
        let b = Briefs::default();
        assert!(matches!(b.fresh(Lead::At(&lead)), Fresh::Nas(_)), "none saved yet");
        c.set_brief(Some(&records(1)));
        let Fresh::Got(h) = b.fresh(Lead::At(&lead)) else { panic!("the lead's") };
        assert_eq!(h.mark, (849, 1));
        assert_eq!(*h.json, *c.brief().unwrap().json, "the bytes the lead serves (and the NAS has)");
        // Unchanged: a 304, no body; the one held kept.
        let (code, body, tag) = lead.get_tagged(PATH, Some(&h.tag())).unwrap();
        assert_eq!((code, body.len(), tag.as_deref()), (304, 0, Some("\"849-1\"")));
        let Fresh::Got(same) = b.fresh(Lead::At(&lead)) else { panic!("the lead's") };
        assert!(Arc::ptr_eq(&same.brief, &h.brief), "kept, not read again");
        // A newer save: the newer, as the lead's own agent has it.
        c.set_brief(Some(&records(2)));
        let Fresh::Got(h2) = b.fresh(Lead::At(&lead)) else { panic!("the lead's") };
        let Fresh::Got(here) = Briefs::default().fresh(Lead::Here(&c)) else { panic!("in process") };
        assert_eq!((h2.mark, here.mark), ((849, 2), (849, 2)));
        assert_eq!(h2.brief.manifest["base/6-1-1"], "base/6-1-1.0000000000000002.base");
        // The lead away: a job's ask fails; the views keep the last one, never the NAS's.
        let nas = no_brief_on(&d.path().join("nas"));
        let away = Client::at(vec![coordwire::testkit::dead()], token.clone(), "m1");
        assert!(matches!(b.fresh(Lead::At(&away)), Fresh::Away(_)));
        assert_eq!(b.now(Lead::At(&away), &nas).unwrap().seq, 2);
        assert!(Briefs::default().now(Lead::At(&away), &nas).is_err(), "none held: the NAS's (none here)");
        // A lead on an older app: the NAS's, as before.
        let (old, asked) = coordwire::testkit::answering(404);
        let old = Client::at(vec![old], token, "m1");
        assert!(matches!(b.fresh(Lead::At(&old)), Fresh::Nas(_)));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(b.fresh(Lead::None), Fresh::Nas(_)));
        // No longer leading: none served.
        c.set_brief(None);
        assert!(matches!(b.fresh(Lead::At(&lead)), Fresh::Nas(_)));
        c.stop();
    }
}
