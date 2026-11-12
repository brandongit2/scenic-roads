//! The pool (docs/pool.md): the Mac mini leads the build, the other Macs are its members. This is the core of its protocol, the part
//! that can lose work if it's wrong (§12, phases 1 and 2), kept apart from the agent and the
//! coordinator so the same code runs against the share and against a model of its faults (§13):
//!
//! - `nas`: the operations the protocol makes on the NAS, with what the SMB share guarantees and
//!   what it doesn't (§3);
//! - `lead` (buildstore::records::lead): the declared lead (`state/build/lead-member.json`, §6.1);
//! - `records`: the build's records (§6.2), numbered by the lead's store's epoch, saved whole at one
//!   path; the merge of hand-offs into them;
//! - `journal`: the jobs' hand-offs as entries keyed by lease, what a member keeps until its lead's
//!   saved records name them (posted by HTTP, crate::coord::handoffs: docs/coordination.md step 6;
//!   nothing goes to the NAS);
//! - `beat`: the fields of a member's heartbeat the protocol reads and writes (§10);
//! - `driver`: what a member is in the pool and does as that, one step per loop of the agent's,
//!   through an I/O trait (the NAS's operations, the clocks, the lead's disk and its store's
//!   epoch): the declared lead taking the records up once a process and leading; a member keeping
//!   its hand-offs for its lead; the API the agent's loop calls;
//! - `sim` (tests): the simulator, which runs the driver on two to four Macs on seeded schedules of
//!   sleeps, stale reads, delayed renames, busy files, cut creates, lost answers and lost posts,
//!   and checks the invariants of §4 at every step and its progress at the end.
//!
//! Every file the protocol writes is one made once with create-new and never changed (the
//! declaration), or the lead's (the records at their one path, which only the declared lead
//! writes; a late landing of its own is read until its next save, and never taken up:
//! docs/coordination.md step 8c).
//!
//! What the agent and the coordinator do around it (messages and hand-offs over HTTP, granting
//! jobs, the write-set checks they pass to the merge, staying awake, the history) stays theirs; the
//! simulator plays that part as the design has it (`sim::Mac`).

pub mod beat;
pub mod driver;
pub mod journal;
pub mod nas;
pub mod reachable;
pub mod records;
pub mod store;

#[cfg(test)]
mod sim;

pub use beat::Beat;
pub use driver::Driver;
pub use journal::{Entry, Mine};
pub use nas::Nas;
pub use records::Records;

// A member and its id as the records name them: buildstore::records's (task #916 step 6a); this
// Mac's id and its hold on it (step 7a: the map server reads the id); the declared lead.
pub use buildstore::records::member::{member_id, MemberLock};
pub use buildstore::records::{is_member_id, lead, Member};
