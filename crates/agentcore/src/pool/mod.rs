//! The pool (docs/pool.md): the Mac mini leads the build, the other Macs are its members. This is the core of its protocol, the part
//! that can lose work if it's wrong (§12, phases 1 and 2), kept apart from the agent and the
//! coordinator so the same code runs against the share and against a model of its faults (§13):
//!
//! - `nas`: the operations the protocol makes on the NAS, with what the SMB share guarantees and
//!   what it doesn't (§3);
//! - `term`: the declared lead (`state/build/lead-member.json`) and its terms, a file each made once
//!   with create-new (§6.1): term 1, then its re-assertions;
//! - `records`: the build's records in one snapshot per term (§6.2), term 1's first made from
//!   today's three files; the merge of journal entries into them, and taking up a term;
//! - `journal`: the jobs' hand-offs as entries keyed by lease, what a member keeps until its lead's
//!   saved records name them (posted by HTTP, crate::coord::handoffs: docs/coordination.md step 6),
//!   and the NAS's journal an older lead reads them from (§7.3; the rollout gate);
//! - `beat`: the fields of a member's heartbeat the protocol reads and writes (§10);
//! - `driver`: what a member is in the pool and does as that, one step per loop of the agent's,
//!   through an I/O trait (the NAS's operations and the clocks): the declared lead taking up its
//!   terms, leading, re-asserting after a restart or a gap (§6.6); a member telling the lead of its
//!   entries; the API the agent's loop calls;
//! - `sim` (tests): the simulator, which runs the driver on two to four Macs on seeded schedules of
//!   sleeps, stale reads, delayed renames, busy files, cut creates and lost answers, and checks the
//!   invariants of §4 at every step and its progress at the end.
//!
//! Every file the protocol writes is one of three kinds (§2, principle 3): a member's own (its
//! heartbeat, its journal entries), one made once with create-new and never changed (terms, notes
//! of refusals), or one term's (its records); the lead's hint aside, which nothing takes for the
//! truth. So a Mac that slept through a change and writes late writes where no one reads:
//! nothing needs refusing (invariant 5).
//!
//! What the agent and the coordinator do around it (messages over HTTP, granting jobs, the
//! write-set checks they pass to the merge, the listings of the journal the driver asks for,
//! staying awake, the history) stays theirs; the simulator plays that part as the design has it
//! (`sim::Mac`).

pub mod beat;
pub mod driver;
pub mod journal;
pub mod nas;
pub mod reachable;
pub mod records;
pub mod store;
pub mod term;

#[cfg(test)]
mod sim;

pub use beat::Beat;
pub use driver::Driver;
pub use journal::{Entry, LeaseId, Mine};
pub use nas::Nas;
pub use records::Records;
pub use term::{Current, Term};

// A member and its id as the records name them: buildstore::records's (task #916 step 6a); this
// Mac's id and its hold on it (step 7a: the map server reads the id).
pub use buildstore::records::member::{member_id, MemberLock};
pub use buildstore::records::{is_member_id, Member};
