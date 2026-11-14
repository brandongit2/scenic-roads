//! A member's broker (docs/roles.md §3.2, docs/pool.md §8): the tasks of its Mac's own jobs, the
//! `/work/*` calls those jobs make, and what the lead's build page
//! asks of this Mac (`/api/live`, `/api/settings`). The page itself is the lead's: asked here, it
//! is sent there, or this Mac's own is shown while the lead isn't reached (crate::coord::memberpage,
//! docs/roles.md §3.3). Not a
//! coordinator: it offers no plan, holds no job's lease, reads none of the files a lead kept in
//! the folder (the leases, measures and pause of when this Mac once led are the lead's: a
//! member's agent removes them, crate::agent::room::member_leftovers), keeps nothing in the pool's
//! store and runs no phase sharer.
//! Its answers say epoch 0 (`Shared::broker`), so a member that still asks this Mac as the lead
//! after a change of lead reads the lead's contact again, whatever its app.
//!
//! It answers on the coordinator's port and folder (one runs at a time on a Mac), its jobs' token
//! the coordinator's, so a job outlives this Mac's change of role with it.

use super::{Coordinator, Mode};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct Broker {
    pub(super) c: Arc<Coordinator>,
}

impl Broker {
    /// Starts answering on `port`, its state in `dir` (the coordinator's); `me`: this Mac's name.
    pub fn start(
        the_plan: &'static dyn crate::agent::seam::Plan,
        dir: &Path,
        port: u16,
        me: &str,
        app: &str,
    ) -> Result<Broker> {
        let listeners = anyhow::Context::with_context(super::listeners_on(port), || format!("listen on port {port}"))?;
        Broker::start_on(the_plan, dir, listeners, me, app)
    }

    /// `start`, answering on `listeners`, already listening.
    pub fn start_on(
        the_plan: &'static dyn crate::agent::seam::Plan,
        dir: &Path,
        listeners: impl Into<super::Listeners>,
        me: &str,
        app: &str,
    ) -> Result<Broker> {
        Ok(Broker {
            c: Arc::new(Coordinator::start_with(
                the_plan,
                dir,
                Some(listeners.into()),
                me,
                app,
                Mode::Broker,
                store::clock::real(),
            )?),
        })
    }

    /// Its addresses and token (its jobs' token is `job_token`).
    pub fn contact(&self) -> &super::Contact {
        &self.c.contact
    }

    /// The token its own jobs offer their tasks with (`SCENIC_COORD_TOKEN`).
    pub fn job_token(&self) -> &str {
        &self.c.job_token
    }

    pub fn port(&self) -> u16 {
        self.c.port()
    }

    /// The pool's member it is, said in its answers with epoch 0.
    pub fn set_member(&self, member: Option<&str>) {
        self.c.set_member(member)
    }

    /// Its history's events after number `seen` (all of them when its numbers went back: a broker
    /// started again), and its last number: its jobs' tasks done or given back, which its agent
    /// notes to the lead (crate::coord::notes).
    pub fn history_since(&self, seen: u64) -> (u64, Vec<super::history::Event>) {
        let s = self.c.shared.lock().unwrap();
        let seq = s.history.seq();
        (seq, s.history.since(if seq < seen { 0 } else { seen }, usize::MAX))
    }

    /// A job's tasks closed as it ends (`owner`: its pid).
    pub fn close_tasks(&self, owner: u32) {
        self.c.close_tasks(owner)
    }

    /// Its task leases lapsed, offered again.
    pub fn expire(&self) -> Vec<super::lease::Lease> {
        self.c.expire()
    }

    /// Its jobs' tasks paused as the build is (the lead's pause, as this member knows it).
    pub fn pause(&self) -> Option<jobrt::control::Pause> {
        self.c.pause()
    }

    pub fn set_pause(&self, pause: Option<jobrt::control::Pause>, at: u64) {
        self.c.set_pause(pause, at)
    }

    /// How its tasks are drawn (crate::agent::placement).
    pub fn set_placement(&self, p: Option<crate::agent::placement::Placement>) {
        self.c.set_placement(p)
    }

    /// The members' brokers (CORS: their pages may call it).
    pub fn set_brokers(&self, b: Option<super::brokers::Brokers>) {
        self.c.set_brokers(b)
    }

    /// Where the build page is, as this Mac's agent last knew the lead (crate::coord::memberpage).
    pub fn set_page_at(&self, at: super::memberpage::PageAt) {
        self.c.set_page_at(at)
    }

    /// Where this Mac's settings lie (`/api/settings`, `/api/hold`; its own page's status).
    pub fn set_settings(&self, home: &Path, root: Option<&store::nasfs::NasRoot>) {
        self.c.set_settings(home, root)
    }

    /// `/api/live` answered from `store` from here on.
    pub fn set_live(&self, store: Arc<crate::agent::live::Store>) {
        self.c.set_live(store)
    }

    pub fn has_live(&self) -> bool {
        self.c.has_live()
    }

    pub fn stop(&self) {
        self.c.stop()
    }

    /// Its server, for a test's look inside.
    #[cfg(test)]
    pub(crate) fn server(&self) -> &Coordinator {
        &self.c
    }
}

/// What brokers this Mac's tasks, for the run's steps (`poolside::Cx::broker`): the lead's coordinator
/// or a member's broker.
#[derive(Clone)]
pub enum Brokering {
    Lead(Arc<Coordinator>),
    Member(Broker),
}

impl Brokering {
    /// Its addresses, for the heartbeat.
    pub fn urls(&self) -> &[String] {
        match self {
            Brokering::Lead(c) => &c.contact.urls,
            Brokering::Member(b) => &b.c.contact.urls,
        }
    }

    pub fn set_brokers(&self, b: Option<super::brokers::Brokers>) {
        match self {
            Brokering::Lead(c) => c.set_brokers(b),
            Brokering::Member(m) => m.set_brokers(b),
        }
    }
}

/// A test's broker (its state in `dir`), on a port the test held from the start; and the port.
#[cfg(test)]
pub(crate) fn start_for_test(dir: &Path, me: &str, app: &str) -> (Broker, u16) {
    let (listener, port) = super::test_listener();
    (Broker::start_on(crate::agent::seam::for_tests(), dir, listener, me, app).unwrap(), port)
}

#[cfg(test)]
impl Broker {
    /// `e` in its history, as its jobs' tasks put them there.
    pub(crate) fn note_for_test(&self, e: super::history::Event) -> u64 {
        self.c.note(e)
    }
}

#[cfg(test)]
mod tests {
    use super::super::history::Event;
    use super::*;

    /// Task #1160: a member's broker keeps its history in memory alone (no file in its folder: its
    /// agent notes its tasks to the lead, crate::coord::notes), handed out after a number, all of it
    /// once its numbers went back (a broker started again).
    #[test]
    fn a_brokers_history_is_its_memory_handed_out_after_a_number() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("coord");
        let (b, _) = start_for_test(&dir, "m1", "");
        let task = |w: &str| Event { worker: Some(w.into()), ..Event::new("task") };
        let first = b.c.note(task("m1-1"));
        b.c.note(task("m1-2"));
        let (seq, evs) = b.history_since(0);
        assert_eq!((seq, evs.len()), (first + 1, 2), "no start of its own: its agent notes that");
        let (_, evs) = b.history_since(first);
        assert_eq!(evs.iter().map(|e| e.worker.clone().unwrap()).collect::<Vec<_>>(), ["m1-2"]);
        assert_eq!(b.history_since(seq + 50).1.len(), 2, "numbers gone back: all again");
        assert!(!dir.join("history.jsonl").exists());
        b.c.stop();
    }
}
