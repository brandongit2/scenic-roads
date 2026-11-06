//! The coordinator's client side (docs/crates.md, task #916 step 6c): how a worker reaches the
//! lead's coordinator (`Contact` on the NAS, the headers its answers carry), its client's transport
//! (client), a task's side shared by its job and its worker (task), and the map now as a job or a
//! command reads it from the lead (`map_now`). The coordinator itself and the lease calls (they
//! carry its own types) are pipeline's coord, which re-exports these at their old paths.

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

/// The map now, for a job or a command (docs/maplog.md §6.7, §6.10): the lead's (`GET
/// /map/snapshot` from its coordinator, as `state/coordinator.json` says to reach it); the NAS's
/// newest snapshot (up to an hour behind) when there's no coordinator or it holds no map yet (503);
/// None when there's no map at all. A coordinator that fails otherwise is an error: a job that
/// publishes or lists from the map never goes by an older one than the lead's while the lead runs.
pub fn map_now(root: &store::nasfs::NasRoot) -> anyhow::Result<Option<store::map::Map>> {
    use anyhow::Context;
    if let Some(c) = crate::client::Client::from_nas(root, &store::sys::host_name())? {
        if let Some(b) = c.get_ready("/map/snapshot").context("the lead's map")? {
            return Ok(Some(store::map::Snapshot::decode(&b).context("the lead's map")?.0));
        }
    }
    store::map::newest_nas(root)
}
