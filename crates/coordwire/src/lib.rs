//! The coordinator's client side (docs/crates.md, task #916 step 6c): how a worker reaches the
//! lead's coordinator (`Contact`, found as lead says: each Mac's agent keeps its lead's; the headers
//! its answers carry), its client's transport (client), a task's side shared by its job and its worker (task), the app's releases from the
//! lead (releases: every Mac's updater, a publish), and the map now as a job or a command reads it
//! from the lead (`map_now`); and what a Mac's services share about the network
//! (net: who may connect, the addresses, the listener; step 7a, for the map server). The coordinator
//! itself and the lease calls (they carry its own types) are agentcore's coord, which re-exports
//! these at their old paths.

pub mod client;
pub mod lead;
pub mod net;
pub mod releases;
pub mod task;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

use serde::{Deserialize, Serialize};

/// How workers reach the coordinator (`lead::find`).
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

/// The rollout's bridge (task #1119 review): where an app before #1119 finds the lead, its agent,
/// its map server and its updater alike (`state/coordinator.json` on the NAS, `Contact` as JSON).
/// No app since reads it; the lead writes it (`keep_old_contact`) so a Mac still on an older app
/// reaches the lead and so updates: without it, the older lead's agent stopping for its own update
/// took the file off, and no older member could reach the new lead or update again. It goes, with
/// the file, once every Mac's updater has run the new app for a day (#642's rule: docs/storage.md,
/// What the NAS's state/ holds).
pub const OLD_CONTACT: &str = "state/coordinator.json";

/// `OLD_CONTACT` written whole as `c`, when what's there differs (read first: one small read each
/// time the lead keeps its contact, a write only when it changed).
pub fn keep_old_contact(root: &store::nasfs::NasRoot, c: &Contact) -> anyhow::Result<()> {
    let p = root.join(OLD_CONTACT);
    if p.read().ok().and_then(|b| serde_json::from_slice::<Contact>(&b).ok()).as_ref() != Some(c) {
        p.write_whole(serde_json::to_vec_pretty(c)?)?;
    }
    Ok(())
}

/// The headers every answer of a pool's lead's coordinator carries: the epoch its leases are
/// granted in (docs/coordination.md §4.3; 0 from a member's broker) and its member.
pub const POOL_EPOCH: &str = "x-pool-epoch";
pub const POOL_MEMBER: &str = "x-pool-member";

/// The map now, for a job or a command (docs/maplog.md §6.7, §6.10): the lead's (`GET
/// /map/snapshot` from its coordinator, found as `lead::find` says); None when
/// there's no coordinator or it holds no map yet (503): the NAS keeps no copy of the map. A root
/// that isn't the NAS (a test root, a local copy laid out as the project folder) holds its own,
/// read when no lead answers there (store::map::root_map). A coordinator that fails otherwise is an
/// error: a job that publishes or lists from the map never goes by an older one than the lead's.
pub fn map_now(root: &store::nasfs::NasRoot) -> anyhow::Result<Option<store::map::Map>> {
    use anyhow::Context;
    let home = lead::this_macs_agent();
    if let Some(c) = crate::client::Client::to_lead(home.as_deref(), Some(root), &store::sys::host_name())? {
        if let Some(b) = c.get_ready("/map/snapshot").context("the lead's map")? {
            return Ok(Some(store::map::Snapshot::decode(&b).context("the lead's map")?.0));
        }
    }
    store::map::root_map(&root.unchecked_path())
}

/// Its unit tests' process is offline from its start (buildstore::fetch::go_offline), and the
/// process's test flag set (buildstore::out::go_testing: net's names, URLs and LAN addresses a
/// test's), before any test runs, as agentcore's are, where net was (task #916 step 7a): run by the
/// loader as the test binary starts. Only in `cfg(test)`, this crate's own unit tests.
#[cfg(test)]
#[used]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(not(target_os = "macos"), link_section = ".init_array")]
static TEST_START: extern "C" fn() = {
    extern "C" fn start() {
        buildstore::fetch::go_offline();
        buildstore::out::go_testing();
    }
    start
};

#[cfg(test)]
mod test_start;
