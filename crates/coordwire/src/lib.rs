//! The coordinator's client side (docs/crates.md, task #916 step 6c): how a worker reaches the
//! lead's coordinator (`Contact` on the NAS, the headers its answers carry), its client's transport
//! (client) and a task's side shared by its job and its worker (task). The coordinator itself and
//! the lease calls (they carry its own types) are pipeline's coord, which re-exports these at their
//! old paths.

pub mod client;
pub mod task;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

use serde::{Deserialize, Serialize};

/// How workers reach the coordinator: `state/coordinator.json` on the NAS.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    /// Its addresses, in the order to try.
    pub urls: Vec<String>,
    pub token: String,
    /// The build page's address over HTTPS when `tailscale serve` proxies the coordinator (a Mac's
    /// menu bar opens it: its web view loads no plain HTTP but the LAN's). None otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

pub fn contact_path(root: &store::nasfs::NasRoot) -> store::nasfs::NasPath {
    root.join("state/coordinator.json")
}

/// The headers every answer of a pool's lead's coordinator carries: its term and its member.
pub const POOL_TERM: &str = "x-pool-term";
pub const POOL_MEMBER: &str = "x-pool-member";
