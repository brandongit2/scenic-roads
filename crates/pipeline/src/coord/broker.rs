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
    c: Arc<Coordinator>,
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

    /// Its server, as the run's steps take this Mac's broker (`pool::Cx::broker`, the lead's
    /// coordinator or this).
    pub fn server(&self) -> &Arc<Coordinator> {
        &self.c
    }
}

impl std::ops::Deref for Broker {
    type Target = Coordinator;
    fn deref(&self) -> &Coordinator {
        &self.c
    }
}

/// A test's broker (its state in `dir`), on a port the test held from the start; and the port.
#[cfg(test)]
pub(crate) fn start_for_test(dir: &Path, me: &str, app: &str) -> (Broker, u16) {
    let (listener, port) = super::test_listener();
    (Broker::start_on(dir, listener, me, app).unwrap(), port)
}
