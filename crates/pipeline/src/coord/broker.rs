//! A member's broker (docs/roles.md §3.2, docs/pool.md §8): the tasks of its Mac's own jobs, the
//! `/work/*` calls those jobs and the pages taking their tasks make, and `/work/live`. Not a
//! coordinator: it offers no plan, holds no job's lease (those of a term this Mac once led are
//! left to the lead as it starts), keeps nothing in the pool's store and runs no phase sharer.
//! Its answers say term 0 (`Shared::broker`), so a member that still asks this Mac as the lead
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
    pub fn start(dir: &Path, port: u16, me: &str, app: &str) -> Result<Broker> {
        let listener = anyhow::Context::with_context(store::spawn::listen((super::LISTEN, port)), || {
            format!("listen on port {port}")
        })?;
        Broker::start_on(dir, listener, me, app)
    }

    /// `start`, answering on `listener`, already listening.
    pub fn start_on(dir: &Path, listener: std::net::TcpListener, me: &str, app: &str) -> Result<Broker> {
        Ok(Broker { c: Arc::new(Coordinator::start_with(dir, listener, me, app, Mode::Broker)?) })
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

    /// The pool's member it is, said in its answers with term 0.
    pub fn set_member(&self, member: Option<&str>) {
        self.c.set_member(member)
    }

    /// A job's tasks closed as it ends (`owner`: its pid).
    pub fn close_tasks(&self, owner: u32) {
        self.c.close_tasks(owner)
    }

    /// Its task leases lapsed, offered again.
    pub fn expire(&self) -> Vec<super::lease::Lease> {
        self.c.expire()
    }

    /// Its pages paused as the build is (the lead's pause, as this member knows it).
    pub fn pause(&self) -> Option<crate::control::Pause> {
        self.c.pause()
    }

    pub fn set_pause(&self, pause: Option<crate::control::Pause>, at: u64) {
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

    pub fn stop(&self) {
        self.c.stop()
    }

    /// Its server, for a test's look inside.
    #[cfg(test)]
    pub(crate) fn server(&self) -> &Coordinator {
        &self.c
    }
}

/// What brokers this Mac's tasks, for the run's steps (`pool::Cx::broker`): the lead's coordinator
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

/// The build page's data and asks, served by whichever of this Mac's servers answers its port
/// (the lead's coordinator, a member's broker, a viewer). A broker serves them only until the
/// lead serves every page (docs/roles.md §5 step 4); then they leave `Broker`.
pub trait Pages {
    fn set_settings(&self, home: &Path, root: Option<&store::nasfs::NasRoot>);
    fn take_inputs_asks(&self) -> Vec<crate::inputs::Ask>;
    fn has_live(&self) -> bool;
    fn set_live(&self, store: Arc<crate::agent::live::Store>);
    fn has_catalogues(&self) -> bool;
    fn set_catalogues(&self, cv: Arc<crate::agent::cataloguesview::CataloguesView>);
    fn has_built(&self) -> bool;
    fn set_built(&self, bv: Arc<crate::agent::builtview::BuiltView>);
    fn has_pageview(&self) -> bool;
    fn set_pageview(&self, pv: Arc<crate::agent::pageview::PageView>);
    fn set_forward(&self, home: Option<std::path::PathBuf>);
    fn take_page_acts(&self) -> Vec<crate::pool::driver::Act>;
    fn set_retry_blocked(&self, why: Option<String>);
}

macro_rules! pages_by {
    ($t:ty, $s:ident => $c:expr) => {
        impl Pages for $t {
            fn set_settings(&$s, home: &Path, root: Option<&store::nasfs::NasRoot>) { Coordinator::set_settings($c, home, root) }
            fn take_inputs_asks(&$s) -> Vec<crate::inputs::Ask> { Coordinator::take_inputs_asks($c) }
            fn has_live(&$s) -> bool { Coordinator::has_live($c) }
            fn set_live(&$s, store: Arc<crate::agent::live::Store>) { Coordinator::set_live($c, store) }
            fn has_catalogues(&$s) -> bool { Coordinator::has_catalogues($c) }
            fn set_catalogues(&$s, cv: Arc<crate::agent::cataloguesview::CataloguesView>) { Coordinator::set_catalogues($c, cv) }
            fn has_built(&$s) -> bool { Coordinator::has_built($c) }
            fn set_built(&$s, bv: Arc<crate::agent::builtview::BuiltView>) { Coordinator::set_built($c, bv) }
            fn has_pageview(&$s) -> bool { Coordinator::has_pageview($c) }
            fn set_pageview(&$s, pv: Arc<crate::agent::pageview::PageView>) { Coordinator::set_pageview($c, pv) }
            fn set_forward(&$s, home: Option<std::path::PathBuf>) { Coordinator::set_forward($c, home) }
            fn take_page_acts(&$s) -> Vec<crate::pool::driver::Act> { Coordinator::take_page_acts($c) }
            fn set_retry_blocked(&$s, why: Option<String>) { Coordinator::set_retry_blocked($c, why) }
        }
    };
}
pages_by!(Coordinator, self => self);
pages_by!(Broker, self => &self.c);

/// A test's broker (its state in `dir`), on a port the test held from the start; and the port.
#[cfg(test)]
pub(crate) fn start_for_test(dir: &Path, me: &str, app: &str) -> (Broker, u16) {
    let (listener, port) = super::test_listener();
    (Broker::start_on(dir, listener, me, app).unwrap(), port)
}
