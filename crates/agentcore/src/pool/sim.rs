//! The pool's simulator (docs/pool.md §13, docs/coordination.md step 9): two to four Macs, any of
//! which may be the declared lead, running the pool's driver (crate::pool::driver), the lead's
//! coordinator (crate::coord, in process: its handlers, lease table, hand-offs' registry and store
//! on a folder of its own) and the member's beat (crate::agent::beat's post, crate::coord::beats's
//! registry, coordwire's client) over a network fake and a model of the NAS's faults, on a schedule
//! drawn from a seed, with the invariants of §4 and docs/coordination.md §8 step 9 checked at every
//! step that could break one, and its progress at the end.
//!
//! **Real:** the driver; the lead's coordinator (`Coordinator::in_process`: every request answered
//! by its own `route` and its store's durability wait, `answer_json`, as over HTTP; its grants by
//! its scheduler, its own jobs' by `ask_own`, released by beats, `held::beat`; hand-offs taken into
//! its store, `handoffs::Handoffs`; its epoch raised, `Store::raise_epoch`; another lead's state
//! taken up from the backup, `load_pool_state`; its store backed up, `store::back_up_once`, and
//! restored, `Store::restore`, on a real folder); the member's beat (`agent::beat::ToLead::post_on`:
//! its body, the lead's answer into `coord::beats::Beats`, the hand-offs it lacks posted) on
//! coordwire's client (`Client::through`: its rounds and errors, `LeaseCalls`' asks), the
//! grants' wire types; the records' brief a job opens (crate::agent::brief::Briefs::fresh: the
//! lead's coordinator's in process, a member's over the network, `GET /pool/brief` answered from
//! `Coordinator::brief`; the lead away, the job not started); GC on the lead (its queue,
//! store::map::gc::GcQueue, on the Mac's own folder, opened again at each start; the map's feed
//! and its clients' cursors, store::map::feed::Feed; the coordinator's `/gc/chunk` and `/gc/gone`;
//! the merge's refusal, crate::agent::gcqueue::refusal), with its grace (`GRACE_S`, 1 h) and its
//! cursors' TTL (`CURSOR_TTL_S`, 6 h) as they are.
//!
//! **Modelled** (`mac`): the agent's loop around them (its slots and jobs, when it asks and beats,
//! its role read from the declaration and its restart into a new one, `agent::pool::took_up`'s and
//! `epochs`' few lines, `lead::after`'s calls), the jobs (a few minutes each, their hand-offs
//! drawn: now and then taking their targets' files as their brief names them, `Handoff::reused`),
//! the map's publishing (a batch of the map's log after each save of the records, `World::publish`)
//! and the map servers' asks of the lead's feed (`World::map_poll`), the `gc` job (a chunk now and
//! then, removed at once), the network (`World::call`), the NAS (below), the clocks, the owner
//! (moving the lead, restoring its disk). The store's files are written without their `fsync` (`seglog::UNSYNCED`):
//! a crash is the files cut by the simulator itself.
//!
//! The NAS's model (`World`): create-new is atomic on the server, and the file's bytes land a step
//! later (a Mac asleep in between leaves it empty meanwhile), or never (`Cfg::cuts`: the share
//! gone between, the file the creator's to finish); a whole write is its temporary file, then its
//! rename a step later (a Mac asleep in between renames when it wakes, over whatever others wrote
//! meanwhile); a rename over a file another Mac has open fails busy, and the write fails after a
//! few tries; a create or a whole write that did its work may answer an error (`Cfg::cuts`: its
//! answer lost); each Mac keeps what it read and stat'ed for up to `Cfg::stale` seconds, so it may
//! read an older version or miss a new file; its own writes it sees at once.
//!
//! The network's (`World::call`, `Cfg::net` and the rest): a request to a Mac asleep, gone, cut
//! off or running no coordinator reaches nothing; one dropped likewise; one delayed is answered
//! that much later, or past the client's time arrives late, done then, its answer lost; an answer
//! lost (the request done); a Mac cut off from the others for minutes to hours (`Cfg::partitions`),
//! or hearing no answer while heard (`Cfg::one_way`); the lead's process killed while a request's
//! change is on its way to its store (`Cfg::kills`: the line landed, torn or not, unanswered), any
//! Mac's process crashing (`Cfg::crashes`), the lead's disk replaced and its store restored from
//! the NAS backup (`Cfg::restores`), and the owner moving the lead (`Cfg::moves`: to another Mac, or
//! back to the one before; as the lead's save is under way, landing after the next lead's; or to a
//! Mac while the lead's reads fail, so it stops granting at `driver::VOUCH_S`).
//!
//! Each Mac runs in a thread of its own, but one at a time: every NAS operation, request and look at
//! a clock waits for the Mac's turn, and after each the schedule (`World::after`) says whose turn
//! is next, puts the Mac to sleep there or not, and brings the faults and the owner's changes. Once
//! the faults stop every Mac stays awake for a while; then the declared lead must lead, alone,
//! caught up, its records at the one path must name every hand-off owed, and no take-up may have
//! been made in the run's last ten minutes. Checked throughout: no target under two live leases
//! (the jobs running, all Macs', and each coordinator's table), two leads never granting at once,
//! no lease id granted twice, every hand-off merged once or refused with why (never applied again
//! once acknowledged), acknowledgements only of what a saved snapshot names, §4's epoch and
//! records invariants, a job's brief never older than the one its lease was granted from (from the
//! same coordinator), GC removing no name the records it saw name nor one a map server heard within
//! the cursor TTL serves, and no records naming a name GC removed (but another disk's queue's,
//! docs/maplog.md §7.2's limit, counted).

mod mac;
mod place;

use super::driver::{self, Io};
use super::journal::{Entry, LeaseId};
use super::lead;
use super::nas::{Created, Nas};
use super::records::{self, Records};
use crate::coord::Coordinator;
use anyhow::{anyhow, bail, Result};
use coordwire::client::Wired;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// The longest a read keeps its file open (s): a rename over it meanwhile fails busy.
const OPEN: u64 = 8;
/// How many times the model's whole write tries its rename.
const BUSY_TRIES: u32 = 4;
/// A Mac's loop: every 20 s, and a little.
const LOOP_S: u64 = 20;
/// The clocks' start: half an hour before a day ends (hand-offs' keys of two days).
const T0: u64 = 1_791_331_200 - 1_800;
/// The units the jobs build, again and again, and their step (the plan's: offered to every member).
const TARGETS: [&str; 5] = ["6-1-0", "6-1-1", "6-1-2", "6-1-3", "6-1-4"];
const STEP: &str = "unit";
/// A development build's app number.
const DEV: u64 = u64::MAX;
/// Where the lead's coordinator says it is (`state/coordinator.json`: which Mac).
const CONTACT: &str = "state/coordinator.json";
/// A request's time at the client (s, docs/coordination.md §5.2): a later answer is lost to it.
const CLIENT_S: u64 = 15;

/// Per step of a Mac while the faults last: that it falls asleep right there; and right after it
/// wrote a temporary file or made a file with create-new, the moments a sleep does the most (a
/// rename landing late, a file empty meanwhile).
const P_SLEEP: f64 = 0.004;
const P_SLEEP_MIDWAY: f64 = 0.05;
/// Per step: that the owner installs a newer app on a Mac.
const P_APP: f64 = 0.0004;
/// With `Cfg::cuts`: per create, that its bytes don't land (the share gone between); per create
/// and whole write, that its answer is lost (done, said failed).
const P_CUT: f64 = 0.03;
const P_LOST: f64 = 0.01;
/// With `Cfg::fails`: per step of a Mac while the faults last, that the share answers none of its
/// reads and stats for a while (1 to 10 minutes), its writes landing.
const P_AWAY: f64 = 0.001;
/// With `Cfg::stalls`: per operation while the faults last, that the share stalls on it (5 to 10
/// minutes, its Mac waiting, awake).
const P_STALL: f64 = 0.0001;
/// With `Cfg::net`, per request while the faults last: dropped (never read), delayed, its answer
/// lost.
const P_DROP: f64 = 0.02;
const P_DELAY: f64 = 0.03;
const P_ANSWER_LOST: f64 = 0.02;
/// Per step while the faults last: a Mac cut off from the others (`Cfg::partitions`), or hearing
/// none of its lead's answers while it's heard (`Cfg::one_way`).
const P_PARTITION: f64 = 0.0004;
const P_ONE_WAY: f64 = 0.0004;
/// With `Cfg::kills`: per request that changed the lead's store, that its process dies right then.
const P_KILL: f64 = 0.01;
/// With `Cfg::crashes`: per step, that the Mac's process crashes (its memory gone, its saved state
/// and disk kept).
const P_CRASH: f64 = 0.0003;
/// With `Cfg::restores`: per step of the declared lead, that its disk is replaced and its store
/// restored from the NAS backup.
const P_RESTORE: f64 = 0.0005;
/// With `Cfg::moves`: per step, that the owner moves the lead; per save of the lead's records'
/// temporary file, that the owner moves it right then and the lead's Mac sleeps past the next
/// lead's take-up (its rename landing after that lead's save).
const P_MOVE: f64 = 0.0005;
const P_MOVE_MIDSAVE: f64 = 0.004;
/// Per step: that the turn stays with the Mac that has it.
const P_KEEP: f64 = 0.85;
/// How long the declared lead's take-up waits on records another Mac holds (a late save over the
/// one path older than its brief) before the owner declares that Mac again, as the wait says to (s).
const OWNER_S: u64 = 1200;
/// How long the run's end must have seen no take-up (s).
const QUIET_S: u64 = 600;
/// At most how long a run goes on past its end while its lead still merges what the faults left,
/// or isn't caught up yet (s).
const MERGING_S: u64 = 3600;

/// What a run is.
#[derive(Clone, Copy, Debug)]
struct Cfg {
    /// The faults stop this long into the run (s); every Mac then stays awake.
    faults: u64,
    /// The run ends this long into it, or goes on (`World::goes_on`) while its lead still merges
    /// what the faults left.
    end: u64,
    /// The longest a Mac keeps what it read or stat'ed (s).
    stale: u64,
    /// A member leaves for good (never wakes) partway through.
    leave: bool,
    /// The clocks' skew, up to this either way (s); 0: up to one second.
    skew: u64,
    /// Now and then the owner runs a development build on a Mac, or rolls its app back.
    downgrade: bool,
    /// Creates whose bytes don't land, and the NAS's answers lost.
    cuts: bool,
    /// Every create, read, stat and whole write taking this long (s), lo..=hi, its Mac waiting on
    /// it, awake: the share under load; (0, 0): no time.
    op_s: (u64, u64),
    /// Now and then a restart without the state its driver saved (lost, or a copy's).
    lose: bool,
    /// Now and then a Mac whose reads the share doesn't answer for a while.
    fails: bool,
    /// Now and then an operation the share stalls on for minutes: its step minutes long.
    stalls: bool,
    /// Requests dropped, delayed (some arriving after the client gave up), answers lost.
    net: bool,
    /// Macs cut off from the others for minutes to hours (at most half the faults' time, 6 h).
    partitions: bool,
    /// Macs heard by the lead while hearing none of its answers.
    one_way: bool,
    /// The lead's process killed as a request's change goes to its store.
    kills: bool,
    /// Any Mac's process crashing now and then.
    crashes: bool,
    /// The lead's disk replaced, its store restored from the NAS backup.
    restores: bool,
    /// The owner moving the lead.
    moves: bool,
    /// How many Macs (0: two to four, by the seed).
    macs: usize,
    /// Mac 1 asleep for seven hours from five minutes after the lead's first take-up (`STALE_S` in:
    /// a declaration the owner wrote, as the run's is): its map server unheard past GC's cursor
    /// TTL (store::map::gc::CURSOR_TTL_S), its member's hand-offs kept past the grace.
    nap: bool,
}

impl Cfg {
    /// The pool's, none of the knobs on.
    fn pool() -> Cfg {
        Cfg {
            faults: 2400,
            end: 3900,
            stale: 30,
            leave: false,
            skew: 0,
            downgrade: false,
            cuts: false,
            op_s: (0, 0),
            lose: false,
            fails: false,
            stalls: false,
            net: false,
            partitions: false,
            one_way: false,
            kills: false,
            crashes: false,
            restores: false,
            moves: false,
            macs: 0,
            nap: false,
        }
    }

    /// The default mix: the knobs, drawn from the seed.
    fn mixed(seed: u64) -> Cfg {
        let mut r = Rng(seed ^ 0x6B6E_6F62_735F_6D69);
        let mut c = Cfg::pool();
        c.leave = r.chance(0.25);
        if r.chance(0.25) {
            c.skew = r.range(30, 1200);
        }
        if r.chance(0.15) {
            c.stale = r.range(60, 300);
        }
        c.downgrade = r.chance(0.25);
        c.cuts = r.chance(0.5);
        if r.chance(0.2) {
            c.op_s = (1, 4);
        }
        c.lose = r.chance(0.25);
        c.fails = r.chance(0.2);
        c.stalls = r.chance(0.15);
        c.net = r.chance(0.6);
        c.partitions = r.chance(0.35);
        c.one_way = r.chance(0.25);
        c.kills = r.chance(0.35);
        c.crashes = r.chance(0.3);
        c.restores = r.chance(0.25);
        c.moves = r.chance(0.4);
        c
    }

    /// Every knob on: the faults of docs/coordination.md step 9 at once.
    fn all(seed: u64) -> Cfg {
        Cfg {
            net: true,
            partitions: true,
            one_way: true,
            kills: true,
            crashes: true,
            restores: true,
            moves: true,
            ..Cfg::mixed(seed)
        }
    }
}

/// splitmix64.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi.max(lo) - lo + 1)
    }

    fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64) < p * (1u64 << 53) as f64
    }
}

/// A Mac as the world sees it.
struct MacW {
    app: u64,
    skew: i64,
    asleep_until: u64,
    /// The seconds it slept (each sleep counted as it begins; cut short at the faults' end): its
    /// awake clock is the run's time less them.
    slept: u64,
    /// Waiting on the share (or a slow answer) until then, awake.
    busy_until: u64,
    /// Its next loop (it waits between loops).
    ready_at: u64,
    /// What it says it leads, checked: the epoch it leads in, the highest it led in.
    leads: Option<u64>,
    led: u64,
    /// Left for good: it never wakes.
    gone: bool,
    /// Its driver's last step said it leads, its records caught up.
    caught: bool,
    /// The records its driver kept on its own disk leading (driver::Io::keep): through restarts,
    /// lost with its disk or its saved state.
    local: Option<Vec<u8>>,
    /// Its agent's folder (its coordinator's under it, `coord/`).
    dir: PathBuf,
    /// Its coordinator, while its agent runs as the lead, and that coordinator's clock.
    coord: Option<Arc<Coordinator>>,
    /// Its coordinator's store's epoch, as its driver last had it (`Io::epoch`), and as its last
    /// raise left it: that epoch, the newest epoch landed at the one path and the newest taken up
    /// then (a take-up's step may end long after, its Mac asleep midway).
    epoch: u64,
    raised: Option<(u64, Option<u64>, Option<u64>)>,
    clock: Option<Arc<store::clock::Virtual>>,
    /// Its process died (killed, crashed, its disk replaced): every operation fails until its
    /// thread restarts it; whether its saved state went with it (`lose`), and its disk.
    crashed: bool,
    lose: bool,
    /// The keys of the hand-offs its driver keeps (`Saved::mine`), as of its last step saved.
    mine: BTreeSet<String>,
    /// Its disk's map log, as the lead's agent keeps it (modelled: crate::agent::publish): the
    /// map's `seq` it published last, and the map's feed with its clients' cursors
    /// (store::map::feed::Feed, kept across its restarts as the log's snapshot keeps them); its GC
    /// queue's file beside (store::map::gc::FILE, opened at each start of its coordinator). All
    /// gone with its disk (`gen`: how many disks it has had).
    map_seq: u64,
    feed: Arc<store::map::feed::Feed>,
    gen: u64,
    /// Its map server's map: the lead's Mac whose log it follows, that disk's `gen`, its `seq`.
    map_at: Option<(usize, u64, u64)>,
    /// Its coordinators started so far (each one's number, for the briefs it serves).
    coords: u64,
}

/// What a Mac keeps of what it read and stat'ed: the bytes (None: no file), and until when.
#[derive(Default)]
struct Cache {
    files: BTreeMap<String, (Option<Vec<u8>>, u64)>,
}

/// What a hand-off is to the checks: one the lead's check takes, one it refuses (outside its
/// step's outputs), or one the lead of an odd epoch refuses (a check that depends on the lead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Taken,
    Refused,
    Either,
}

/// Whether records name hand-off `k` as what it is: taken, refused, or either.
fn names(r: &Records, k: &str, kind: Kind) -> bool {
    match kind {
        Kind::Taken => r.reflected.contains(k),
        Kind::Refused => r.rejected.contains_key(k),
        Kind::Either => r.handles(k),
    }
}

/// A hand-off made in a run: what it is, which Mac's job made it, whether a lead's store took it,
/// and whether its member's saved state was lost after its job (the double fault §5.3 accepts: a
/// copy left only in a lead's store, then that store gone or no longer the lead's).
#[derive(Clone, Copy, Debug)]
struct Made {
    kind: Kind,
    mac: usize,
    posted: bool,
    state_lost: bool,
}

/// A job running: its Mac and its targets.
#[derive(Clone, Debug)]
struct Running {
    mac: usize,
    targets: Vec<String>,
}

/// A request delayed past its client's time: done when it arrives, its answer lost.
struct Late {
    at: u64,
    from: usize,
    to: usize,
    path: String,
    body: Vec<u8>,
}

/// The simulated world: the NAS's files, the network, the Macs, the schedule, and what's checked.
struct World {
    cfg: Cfg,
    t: u64,
    rng: Rng,
    files: BTreeMap<String, Vec<u8>>,
    /// Who has each file open, until when (its creator, until its bytes land).
    open: BTreeMap<String, BTreeMap<usize, u64>>,
    /// When each temporary file was written, and every file's last writer and when.
    tmps: BTreeMap<String, u64>,
    writes: BTreeMap<String, (usize, u64)>,
    caches: Vec<Cache>,
    macs: Vec<MacW>,
    /// The step just made was a temporary file's write or a create.
    midway: bool,
    turn: usize,
    done: bool,
    drained: bool,
    /// When the run ends, gone on past its config's end while the lead merges; and how many
    /// hand-offs the one path's records lacked when it last went on.
    end: u64,
    behind: Option<usize>,
    /// Gone on past its config's end while the declared lead's take-up waited: its take-ups must
    /// settle by the run's own end, not its config's.
    waited: bool,
    /// The declared lead, and the one before it (the owner's moves).
    declared: usize,
    before: Option<usize>,
    /// The take-ups, by epoch: by which Mac, and when; the Mac that led each epoch.
    took: BTreeMap<u64, (usize, u64)>,
    leaders: BTreeMap<u64, usize>,
    /// Each epoch's last snapshot landed at the one path: its number, and the hand-offs it names.
    landed: BTreeMap<u64, (u64, BTreeSet<String>)>,
    /// Every hand-off made, by key; those a lead acknowledged, in which epoch.
    made: BTreeMap<String, Made>,
    acked: BTreeMap<String, u64>,
    /// The hand-offs each Mac's coordinator's store holds (taken, not yet merged), as last seen.
    stored: Vec<BTreeSet<String>>,
    /// Every lease seen granted (epoch, id): its worker and targets.
    granted: BTreeMap<(u64, u64), (String, Vec<String>)>,
    /// The leases each Mac's coordinator has held in its process (cleared as one starts).
    known: Vec<BTreeSet<(u64, u64)>>,
    /// The jobs running, by lease (epoch, id).
    running: BTreeMap<(u64, u64), Running>,
    /// The epochs whose lead was seen caught up.
    caught: BTreeSet<u64>,
    /// Until when the share answers none of each Mac's reads (`Cfg::fails`, a move's).
    away: Vec<u64>,
    /// Until when each Mac is cut off from the others, and hears no answer.
    cut: Vec<u64>,
    deaf: Vec<u64>,
    late: Vec<Late>,
    /// Each map a lead's Mac published (that Mac, its disk's `gen`, the `seq`): the content names
    /// it holds.
    maps: BTreeMap<(usize, u64, u64), BTreeSet<String>>,
    /// The content names GC removed from the NAS: by which Mac's queue (its disk's `gen`), when.
    removed: BTreeMap<String, (usize, u64, u64)>,
    /// Each map server's last ask of a lead's feed (server, lead): when, by the lead's clock.
    polled: BTreeMap<(usize, usize), u64>,
    /// Each job's lease's grant: the coordinator that granted it (its Mac, its number) and the
    /// brief it served then (docs/coordination.md step 8d).
    grant_marks: BTreeMap<(u64, u64), (usize, u64, Option<(u64, u64)>)>,
    /// The brief last served over the network: by which coordinator (its Mac, its number).
    served: Option<(usize, u64)>,
    /// The hand-offs a landed snapshot refused for a name GC removed.
    gc_refused: BTreeSet<String>,
    /// Since when the declared lead's take-up has waited on the records of the Mac that led an
    /// epoch (`OWNER_S`), and that epoch.
    stuck: Option<(u64, u64)>,
    /// The epochs each Mac landed a snapshot of at the one path (epoch, Mac), and those two Macs
    /// led (`repeats`).
    landed_by: BTreeSet<(u64, usize)>,
    repeated: BTreeSet<u64>,
    /// The run's folder (each Mac's under it), kept until the run ends.
    _dir: tempfile::TempDir,
    wrong: Vec<String>,
    counts: Counts,
    trace: Option<Vec<String>>,
}

/// The app version of number `n`: a minute later each; a development build's.
fn app(n: u64) -> String {
    if n == DEV {
        return "development".into();
    }
    format!("20261006-{:02}{:02}-{n:07x}", 12 + n / 60, n % 60)
}

fn id(k: usize) -> String {
    format!("m-{:016x}", k + 1)
}

fn host(k: usize) -> String {
    format!("mac{k}")
}

fn logical(t: &str) -> String {
    format!("sim/{t}")
}

fn content(t: &str, k: &str) -> String {
    format!("sim/{t}.{k}.x")
}

/// The declaration naming Mac `k`.
fn declaration(k: usize, since: u64) -> Vec<u8> {
    serde_json::to_vec(&lead::Declared { member: id(k), host: host(k), since }).unwrap()
}

/// Whether records are self-consistent (invariant 4): each unit's key and its file in the manifest
/// from the same hand-off, nothing from a refused one, and every refusal saying why.
fn consistent(r: &Records) -> std::result::Result<(), String> {
    for (t, k) in &r.keys.unit {
        if r.manifest.get(&logical(t)) != Some(&content(t, k)) {
            return Err(format!("{t}'s key is {k}, its file {:?}", r.manifest.get(&logical(t))));
        }
    }
    for (l, c) in &r.manifest {
        let Some(t) = l.strip_prefix("sim/") else { continue };
        if r.keys.unit.get(t).map(|k| content(t, k)).as_ref() != Some(c) {
            return Err(format!("{l} is {c}, its key {:?}", r.keys.unit.get(t)));
        }
        if c.contains(".bogus.") {
            return Err(format!("{l} is a refused hand-off's {c}"));
        }
    }
    if let Some(k) = r.reflected.iter().find(|k| r.rejected.contains_key(*k)) {
        return Err(format!("{k} is both applied and refused"));
    }
    if let Some((k, _)) = r.rejected.iter().find(|(_, why)| why.trim().is_empty()) {
        return Err(format!("{k} is refused with no why"));
    }
    Ok(())
}

/// The simulator's step's outputs (crate::agent::steps::writes' stand-in): a unit job writes and
/// removes its units' files.
fn writes(_step: &str, l: &str, _removal: bool) -> Option<bool> {
    Some(l.starts_with("sim/"))
}

/// Whether hand-off `e` is a flaky one: one the lead of an odd epoch refuses (its keys begin `f`).
fn flaky(e: &Entry) -> bool {
    e.handoff.done.as_ref().is_some_and(|(_, d)| d.iter().any(|(_, k)| k.starts_with('f')))
}

/// The lead's check of a hand-off: every change within its step's outputs, by the merge's own check
/// (crate::agent::steps::outside_by, refused by every lead); and a flaky one the lead of an odd
/// epoch refuses (a stand-in for a check by the lead's state), so one a lead refused may be taken
/// by the next.
fn check(e: &Entry, r: &Records) -> std::result::Result<(), String> {
    if let Some(why) = crate::agent::seam::for_tests().outside_by(e, &|l, removal| writes(&e.step, l, removal)) {
        return Err(why);
    }
    if flaky(e) && r.epoch % 2 == 1 {
        return Err(format!("refused by epoch {}'s lead", r.epoch));
    }
    Ok(())
}

impl World {
    fn new(seed: u64, macs: usize, cfg: Cfg, tracing: bool) -> World {
        let mut rng = Rng(seed);
        let dir = tempfile::tempdir().expect("a run's folder");
        let macs: Vec<MacW> = (0..macs)
            .map(|k| {
                let skew = if cfg.skew > 0 {
                    rng.below(2 * cfg.skew + 1) as i64 - cfg.skew as i64
                } else {
                    rng.below(3) as i64 - 1
                };
                MacW {
                    app: 0,
                    skew,
                    asleep_until: 0,
                    slept: 0,
                    busy_until: 0,
                    ready_at: 0,
                    leads: None,
                    led: 0,
                    gone: false,
                    caught: false,
                    local: None,
                    dir: dir.path().join(host(k)),
                    coord: None,
                    epoch: 0,
                    raised: None,
                    clock: None,
                    crashed: false,
                    lose: false,
                    mine: BTreeSet::new(),
                    map_seq: 0,
                    feed: store::map::feed::Feed::new(),
                    gen: 0,
                    map_at: None,
                    coords: 0,
                }
            })
            .collect();
        let mut files = BTreeMap::new();
        // Mac 0 the declared lead, at first: the owner's declaration (its take-up waits `STALE_S`).
        files.insert(lead::LEAD.to_string(), declaration(0, T0));
        // Today's records: one unit built.
        files.insert(
            "state/build/manifest.json".to_string(),
            serde_json::to_vec(&BTreeMap::from([(logical(TARGETS[0]), content(TARGETS[0], "k0"))])).unwrap(),
        );
        files.insert(
            "state/build/jobs.json".to_string(),
            format!("{{\"unit\": {{\"{}\": \"k0\"}}}}", TARGETS[0]).into_bytes(),
        );
        let n = macs.len();
        let mut w = World {
            cfg,
            t: 0,
            rng,
            files,
            open: BTreeMap::new(),
            tmps: BTreeMap::new(),
            writes: BTreeMap::new(),
            caches: (0..n).map(|_| Cache::default()).collect(),
            macs,
            midway: false,
            turn: 0,
            done: false,
            drained: false,
            end: cfg.end,
            behind: None,
            waited: false,
            declared: 0,
            before: None,
            took: BTreeMap::new(),
            leaders: BTreeMap::new(),
            landed: BTreeMap::new(),
            made: BTreeMap::new(),
            acked: BTreeMap::new(),
            stored: vec![BTreeSet::new(); n],
            granted: BTreeMap::new(),
            known: vec![BTreeSet::new(); n],
            running: BTreeMap::new(),
            caught: BTreeSet::new(),
            away: vec![0; n],
            cut: vec![0; n],
            deaf: vec![0; n],
            late: Vec::new(),
            maps: BTreeMap::new(),
            removed: BTreeMap::new(),
            polled: BTreeMap::new(),
            grant_marks: BTreeMap::new(),
            served: None,
            gc_refused: BTreeSet::new(),
            stuck: None,
            landed_by: BTreeSet::new(),
            repeated: BTreeSet::new(),
            _dir: dir,
            wrong: Vec::new(),
            counts: BTreeMap::new(),
            trace: tracing.then(Vec::new),
        };
        w.pick(0);
        w
    }

    fn note(&mut self, s: impl FnOnce() -> String) {
        if let Some(tr) = &mut self.trace {
            tr.push(format!("{:>5} {}", self.t, s()));
        }
    }

    fn wrong(&mut self, s: String) {
        self.note(|| format!("WRONG: {s}"));
        self.wrong.push(format!("at {} s: {s}", self.t));
    }

    fn count(&mut self, what: &'static str) {
        *self.counts.entry(what).or_default() += 1;
    }

    fn clock(&self, me: usize) -> u64 {
        (T0 as i64 + self.t as i64 + self.macs[me].skew) as u64
    }

    fn awake_clock(&self, me: usize) -> u64 {
        1_000_000 + self.t - self.macs[me].slept
    }

    fn awake(&self, k: usize) -> bool {
        self.macs[k].asleep_until <= self.t && !self.macs[k].gone
    }

    fn faulting(&self) -> bool {
        self.t < self.cfg.faults
    }

    // The schedule.

    /// After a Mac's step: time passes, the faults' events, late requests arrive, and whose turn is
    /// next.
    fn after(&mut self, me: usize) {
        if self.done {
            return;
        }
        self.count("steps");
        let dt = self.rng.below(2);
        self.advance(self.t + dt);
        // The owner, as the waiting take-up says: the Mac whose records the brief is of declared
        // again.
        if let Some((since, e)) = self.stuck.filter(|&(since, _)| self.t >= since + OWNER_S) {
            self.stuck = None;
            match self.leaders.get(&e).copied().filter(|&k| k != self.declared && !self.macs[k].gone) {
                Some(k) => {
                    let waited = self.t - since;
                    self.note(|| {
                        format!(
                            "the owner declares mac{k} again: the records of epoch {e} are on its disk ({waited} s)"
                        )
                    });
                    self.count("leads moved by the owner to the Mac with the newest records");
                    self.move_lead_to(k);
                }
                None => self.note(|| format!("the owner can't declare epoch {e}'s lead again")),
            }
        }
        if self.faulting() {
            self.faults(me);
        }
        self.arrive();
        self.pick(me);
    }

    /// Time passes to `to`: no further than the faults' end while they last (every Mac wakes then,
    /// its links mended).
    fn advance(&mut self, to: u64) {
        self.t = if self.drained { to } else { to.min(self.cfg.faults) };
        if self.t >= self.cfg.faults && !self.drained {
            self.drained = true;
            let t = self.t;
            for m in &mut self.macs {
                if !m.gone && m.asleep_until > t {
                    m.slept -= m.asleep_until - t;
                    m.asleep_until = t;
                }
            }
            for k in 0..self.macs.len() {
                (self.cut[k], self.deaf[k], self.away[k]) = (0, 0, 0);
            }
            self.note(|| "the faults stop: every Mac awake and reached from now".into());
        }
        if self.t >= self.end && !self.goes_on() {
            self.done = true;
        }
    }

    /// The hand-offs the records must name: every one made but those a Mac gone for good still
    /// holds, and those whose every copy is gone with a double fault (`lost`).
    fn owed(&self) -> impl Iterator<Item = (&String, &Made)> {
        self.made
            .iter()
            .filter(|(k, m)| !self.lost(k, m) && !(self.macs[m.mac].gone && self.macs[m.mac].mine.contains(*k)))
    }

    /// Whether hand-off `k` is gone with the double fault docs/coordination.md §5.3 accepts: its
    /// member's saved state lost after its job, and no copy kept anywhere now (no member's saved
    /// state, no store of the declared lead's), so no lead can merge it.
    fn lost(&self, k: &str, m: &Made) -> bool {
        m.state_lost && !self.macs.iter().any(|w| w.mine.contains(k)) && !self.stored[self.declared].contains(k)
    }

    /// At the run's end: whether it goes on ten minutes more, its lead still merging what the
    /// faults left (the one path's records lack hand-offs, fewer than when it last went on, the
    /// first time any), or not caught up yet, or the declared lead not leading yet (its take-up
    /// waiting: on the Mac that has the newest records, `OWNER_S`, then `STALE_S` once declared
    /// again); for `MERGING_S` at most.
    fn goes_on(&mut self) -> bool {
        let Some(&h) = self.took.keys().next_back() else { return false };
        let r = self.files.get(records::ONE).and_then(|b| serde_json::from_slice::<Records>(b).ok());
        let lack = r.as_ref().map_or(0, |r| self.owed().filter(|(k, m)| !names(r, k, m.kind)).count());
        let behind = r.is_some() && self.macs.iter().any(|m| m.leads == Some(h) && !m.gone && !m.caught);
        let merging = lack > 0 && self.behind.is_none_or(|b| lack < b);
        let d = &self.macs[self.declared];
        let waiting = !d.gone && d.leads.is_none();
        // (Taken up since it waited: ten minutes more, to see the take-ups settle.)
        let settling = self.waited && self.took.values().next_back().is_some_and(|&(_, at)| at + QUIET_S > self.t);
        if !(merging || behind || waiting || settling) || self.t >= self.cfg.end + MERGING_S {
            return false;
        }
        self.behind = Some(lack);
        self.end = self.t + QUIET_S;
        if merging {
            self.count("ten minutes more, the lead merging");
            self.note(|| format!("the run goes on: the records lack {lack} hand-offs"));
        } else if behind {
            self.count("ten minutes more, the lead not caught up");
            self.note(|| format!("the run goes on: epoch {h}'s lead isn't caught up"));
        } else if settling && !waiting {
            self.count("ten minutes more, the take-ups settling");
            self.note(|| "the run goes on: the take-ups settling".to_string());
        } else {
            self.waited = true;
            self.count("ten minutes more, the declared lead not leading yet");
            let d = self.declared;
            self.note(|| format!("the run goes on: the declared lead mac{d} doesn't lead yet"));
        }
        true
    }

    /// Mac `k`, awake, falls asleep for `d` s.
    fn sleep(&mut self, k: usize, d: u64) {
        self.macs[k].asleep_until = self.t + d;
        self.macs[k].slept += d;
        self.count("sleeps");
        self.note(|| format!("mac{k} sleeps {d} s"));
    }

    fn faults(&mut self, me: usize) {
        if self.cfg.nap
            && me == 1
            && self.t >= driver::STALE_S + 300
            && !self.counts.contains_key("Macs asleep seven hours")
        {
            self.sleep(1, 7 * 3600);
            self.count("Macs asleep seven hours");
        }
        if self.cfg.fails && self.rng.chance(P_AWAY) {
            let d = self.rng.range(60, 600);
            self.away[me] = self.away[me].max(self.t + d);
            self.count("shares away from a Mac");
            self.note(|| format!("the share answers none of mac{me}'s reads for {d} s"));
        }
        let p = if std::mem::take(&mut self.midway) { P_SLEEP_MIDWAY } else { P_SLEEP };
        if self.rng.chance(p) && !self.macs[me].gone {
            let d = match self.rng.below(10) {
                0..=5 => self.rng.range(5, 90),
                6..=8 => self.rng.range(90, 900),
                _ => self.rng.range(900, 2400),
            };
            self.sleep(me, d);
        }
        if self.rng.chance(P_APP) {
            let newest = self.macs.iter().map(|m| m.app).filter(|&a| a != DEV).max().unwrap_or(0);
            if self.cfg.downgrade && self.rng.chance(0.5) {
                // A development build on one Mac, or the published app rolled back there.
                let at = self.rng.below(self.macs.len() as u64) as usize;
                self.macs[at].app =
                    if self.rng.chance(0.6) { DEV } else { newest.saturating_sub(self.rng.range(1, 3)) };
                let v = app(self.macs[at].app);
                self.note(|| format!("mac{at} gets app {v}"));
            } else {
                // A release: every Mac takes it at its next loop.
                let n = newest + self.rng.range(1, 3);
                for m in &mut self.macs {
                    m.app = n;
                }
                self.note(|| format!("every Mac gets app {}", app(n)));
            }
        }
        // The network's faults.
        if self.cfg.partitions && self.rng.chance(P_PARTITION) && !self.macs[me].gone {
            let most = (self.cfg.faults / 2).clamp(600, 6 * 3600);
            let d = self.rng.range(300, most);
            self.cut[me] = self.cut[me].max(self.t + d);
            self.count("Macs cut off");
            self.note(|| format!("mac{me} is cut off from the others for {d} s"));
        }
        if self.cfg.one_way && self.rng.chance(P_ONE_WAY) && !self.macs[me].gone {
            let d = self.rng.range(120, 3600);
            self.deaf[me] = self.deaf[me].max(self.t + d);
            self.count("one-way links");
            self.note(|| format!("mac{me} is heard but hears no answer for {d} s"));
        }
        // Processes.
        if self.cfg.crashes && self.rng.chance(P_CRASH) && !self.macs[me].gone {
            self.crash(me, "its process crashes");
        }
        let d = self.declared;
        if self.cfg.restores && d == me && self.rng.chance(P_RESTORE) {
            self.restore(d);
        }
        // The owner.
        if self.cfg.moves && self.rng.chance(P_MOVE) {
            self.move_lead(false);
        }
    }

    /// Whose turn is next: this Mac's again, most often, while it can run; else another's that can
    /// (awake, not waiting on the share, its loop due), at random; else time jumps to the next
    /// waking or loop.
    fn pick(&mut self, me: usize) {
        while !self.done {
            let t = self.t;
            let can = |m: &MacW| m.asleep_until <= t && m.busy_until <= t && m.ready_at <= t;
            if can(&self.macs[me]) && self.rng.chance(P_KEEP) {
                self.turn = me;
                return;
            }
            let ready: Vec<usize> = (0..self.macs.len()).filter(|&k| can(&self.macs[k])).collect();
            if !ready.is_empty() {
                self.turn = ready[self.rng.below(ready.len() as u64) as usize];
                return;
            }
            let next =
                self.macs.iter().map(|m| m.asleep_until.max(m.busy_until).max(m.ready_at)).min().unwrap_or(t + 1);
            let next = self.late.iter().map(|l| l.at).min().map_or(next, |l| l.min(next));
            self.advance(next.max(t + 1));
            self.arrive();
        }
    }

    // The owner's and the processes' events.

    /// Mac `k`'s process dies (`why`): its coordinator gone with it (its store as its files are),
    /// its jobs with it; every operation of it fails until its thread restarts it.
    fn crash(&mut self, k: usize, why: &str) {
        if self.macs[k].crashed {
            return;
        }
        self.macs[k].crashed = true;
        self.stop_coord(k, false);
        self.running.retain(|_, r| r.mac != k);
        self.count("processes crashed");
        self.note(|| format!("mac{k}: {why}"));
    }

    /// Mac `k`'s coordinator stopped (`clean`: its store written as it stops) or gone with its
    /// process (what its store had written stays, the rest gone).
    fn stop_coord(&mut self, k: usize, clean: bool) {
        let Some(c) = self.macs[k].coord.take() else { return };
        self.macs[k].clock = None;
        let store = c.shared.lock().unwrap().keep.store().cloned();
        if let (false, Some(s)) = (clean, &store) {
            // (What was submitted is on its disk first: what a crash keeps of the last append is
            // the simulator's to cut, `kill`.)
            s.wait(s.last(), crate::pool::store::DURABLE_WAIT);
            s.kill();
        }
        drop(c);
    }

    /// The owner replaces lead `k`'s disk (docs/coordination.md §4.4): its process gone, its agent's
    /// folder with its saved state and records, its store restored from the NAS backup
    /// (`Store::restore`, the owner's `scenic lead restore`), or none when there's no backup.
    fn restore(&mut self, k: usize) {
        self.crash(k, "its disk is replaced");
        self.macs[k].lose = true;
        self.macs[k].local = None;
        std::fs::remove_dir_all(&self.macs[k].dir).ok();
        self.stored[k].clear();
        // (Its map log and GC's queue with it: a new disk's start afresh, docs/maplog.md §7.2.)
        let m = &mut self.macs[k];
        (m.map_seq, m.feed, m.gen) = (0, store::map::feed::Feed::new(), m.gen + 1);
        let backup = self.files.get(crate::pool::store::BACKUP_PATH).cloned();
        let snap = backup.and_then(|b| serde_json::from_slice::<crate::pool::store::Snapshot>(&b).ok());
        match snap {
            Some(b) => match crate::pool::store::Store::restore(&self.macs[k].dir.join("coord/store"), &b) {
                Ok(s) => {
                    s.stop();
                    self.count("lead stores restored from the backup");
                    self.note(|| format!("mac{k}'s store restored from {}'s backup at epoch {}", b.member, b.epoch));
                }
                Err(e) => self.wrong(format!("mac{k}'s store can't be restored from the backup: {e:#}")),
            },
            None => self.note(|| format!("mac{k}'s disk replaced with no backup on the NAS")),
        }
    }

    /// The owner declares Mac `to` the lead (docs/pool.md §6.1).
    fn move_lead_to(&mut self, to: usize) {
        let old = self.declared;
        (self.declared, self.before) = (to, Some(old));
        let since = self.clock(to);
        self.files.insert(lead::LEAD.to_string(), declaration(to, since));
        self.writes.insert(lead::LEAD.to_string(), (usize::MAX, self.t));
    }

    /// Epoch `e` led by two Macs, the first's take-up never on the NAS (no snapshot of it landed).
    fn repeats(&mut self, e: u64) {
        if self.repeated.insert(e) {
            self.count("epochs repeated: a lead's take-up never reached the NAS");
            self.note(|| format!("epoch {e} repeated: a lead's take-up never reached the NAS"));
        }
    }

    /// Mac `me`, the declared lead, waits to take up: on the records of the Mac that led epoch `e`.
    fn waits_on(&mut self, me: usize, e: u64) {
        if me == self.declared && self.stuck.is_none() {
            self.stuck = Some((self.t, e));
        }
    }

    /// The owner moves the lead (docs/pool.md §6.1): to the one before it now and then, else to
    /// another Mac awake, by editing the declaration. `midsave`: as the lead's save is under way
    /// (its Mac then sleeps past the next lead's take-up). Now and then the lead's reads fail from
    /// then on for longer than `STALE_S`, so it can't read the move and stops granting at `VOUCH_S`.
    fn move_lead(&mut self, midsave: bool) {
        let old = self.declared;
        let others: Vec<usize> = (0..self.macs.len()).filter(|&k| k != old && !self.macs[k].gone).collect();
        if others.is_empty() {
            return;
        }
        let to = match self.before.filter(|b| others.contains(b)) {
            Some(b) if self.rng.chance(0.4) => {
                self.count("leads moved back");
                b
            }
            _ => others[self.rng.below(others.len() as u64) as usize],
        };
        self.declared = to;
        self.before = Some(old);
        let since = self.clock(to);
        self.files.insert(lead::LEAD.to_string(), declaration(to, since));
        self.writes.insert(lead::LEAD.to_string(), (usize::MAX, self.t));
        self.count("leads moved");
        self.note(|| format!("the owner moves the lead from mac{old} to mac{to}"));
        if midsave {
            let d = driver::STALE_S + self.rng.range(120, 900);
            self.count("leads moved mid-save");
            self.sleep(old, d);
        } else if self.rng.chance(0.3) {
            let d = driver::STALE_S + self.rng.range(60, 900);
            self.away[old] = self.away[old].max(self.t + d);
            self.count("leads moved with their reads failing");
            self.note(|| format!("the share answers none of mac{old}'s reads for {d} s"));
        }
    }

    // The network.

    /// Whether Mac `from`'s request reaches Mac `to`'s coordinator: None, or what came of it.
    fn reaches(&mut self, from: usize, to: usize) -> Option<Wired> {
        if !self.awake(to) || self.macs[to].crashed {
            return Some(Wired::Unreached(format!("mac{to} doesn't answer")));
        }
        if self.macs[to].coord.is_none() {
            return Some(Wired::Unreached(format!("mac{to} runs no coordinator")));
        }
        if from != to && (self.cut[from] > self.t || self.cut[to] > self.t) {
            self.count("requests across a cut");
            return Some(Wired::Unreached(format!("mac{from} and mac{to} are cut off")));
        }
        if self.cfg.net && self.faulting() && from != to && self.rng.chance(P_DROP) {
            self.count("requests dropped");
            return Some(Wired::Unreached("dropped".into()));
        }
        None
    }

    /// Mac `from`'s request to Mac `to`'s coordinator (`path`, `body`): what came of it.
    fn call(&mut self, from: usize, to: usize, path: &str, body: &[u8]) -> Wired {
        if let Some(w) = self.reaches(from, to) {
            return w;
        }
        let faulting = self.cfg.net && self.faulting() && from != to;
        if faulting && self.rng.chance(P_DELAY) {
            let d = self.rng.range(1, 60);
            if d > CLIENT_S {
                self.count("requests arriving late");
                self.late.push(Late { at: self.t + d, from, to, path: path.to_string(), body: body.to_vec() });
                return Wired::TimedOut(format!("no answer in {CLIENT_S} s"));
            }
            self.count("requests delayed");
            self.macs[from].busy_until = self.macs[from].busy_until.max(self.t + d);
        }
        let Some((code, b)) = self.deliver(from, to, path, body) else {
            return Wired::TimedOut("the connection closed unanswered".into());
        };
        if (faulting && self.rng.chance(P_ANSWER_LOST)) || self.deaf[from] > self.t {
            self.count("answers lost");
            return Wired::TimedOut("the answer was lost".into());
        }
        Wired::Answered(code, b)
    }

    /// The requests delayed past their clients' time that arrive now: done, their answers lost.
    fn arrive(&mut self) {
        while let Some(i) = self.late.iter().position(|l| l.at <= self.t) {
            let l = self.late.remove(i);
            let reached = self.awake(l.to)
                && !self.macs[l.to].crashed
                && self.macs[l.to].coord.is_some()
                && self.cut[l.from] <= self.t
                && self.cut[l.to] <= self.t;
            if reached {
                self.note(|| format!("mac{}'s {} arrives at mac{}, late", l.from, l.path, l.to));
                self.deliver(l.from, l.to, &l.path, &l.body);
            }
        }
    }

    /// Request `path` delivered to Mac `to`'s coordinator: its answer, as `http::json` gives it (an
    /// error a 400), or `GET /pool/brief` its (crate::coord::brief: the brief's bytes, 503 while it
    /// has none; never gzipped nor 304 here); None when its process was killed as its change went
    /// to its store (`Cfg::kills`).
    fn deliver(&mut self, from: usize, to: usize, path: &str, body: &[u8]) -> Option<(u16, Vec<u8>)> {
        let c = self.macs[to].coord.clone()?;
        self.tick(to);
        if path == crate::coord::brief::PATH {
            self.served = Some((to, self.macs[to].coords));
            return Some(match c.brief() {
                Some(h) => (200, h.json.to_vec()),
                None => (503, b"{\"error\": \"no brief saved yet\"}".to_vec()),
            });
        }
        let store = c.shared.lock().unwrap().keep.store().cloned();
        let before = store.as_ref().map(|s| (s.last(), log_len(&self.macs[to].dir)));
        let r = match c.call(path, body, &host(from)) {
            Ok(r) => r,
            Err(e) => (400, serde_json::json!({ "error": format!("{e:#}") })),
        };
        let changed = store.as_ref().zip(before).is_some_and(|(s, (last, _))| s.last() > last);
        if changed && self.cfg.kills && self.faulting() && self.rng.chance(P_KILL) {
            self.kill(to, before.map_or(0, |b| b.1));
            return None;
        }
        self.seen_coord(to);
        Some((r.0, serde_json::to_vec(&r.1).unwrap_or_default()))
    }

    /// Lead `k`'s process killed as a request's change goes to its store: the change's line landed
    /// whole, torn, or not at all (the store's own `seglog::read` cuts a torn one), unanswered.
    fn kill(&mut self, k: usize, before: u64) {
        let dir = self.macs[k].dir.clone();
        self.crash(k, "its process is killed mid-append");
        let after = log_len(&dir);
        let cut = match self.rng.below(3) {
            0 => before,
            1 => before + after.saturating_sub(before) / 2,
            _ => after,
        };
        // (Not when its writer compacted meanwhile: a new segment, the change in its snapshot.)
        if let (Some(seg), true) = (last_segment(&dir), after >= before) {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&seg) {
                f.set_len(cut).ok();
            }
        }
        self.count("leads killed mid-append");
        self.note(|| format!("mac{k}'s store's last append cut at {cut} of {after} bytes (from {before})"));
    }

    /// Mac `k`'s coordinator's clock to its Mac's.
    fn tick(&mut self, k: usize) {
        let to = self.clock(k);
        if let Some(c) = &self.macs[k].clock {
            let now = store::clock::Clock::wall(&**c).duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            if to > now {
                c.advance(std::time::Duration::from_secs(to - now));
            }
        }
    }

    /// After a request to Mac `k`'s coordinator: its leases checked (no target under two live
    /// leases of its table; no lease id granted twice), and what its store holds of the hand-offs.
    fn seen_coord(&mut self, k: usize) {
        let Some(c) = self.macs[k].coord.clone() else { return };
        self.holds(k, c.handoffs_taken().into_iter().collect());
        let jobs = c.live_jobs();
        let mut by_target: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for (e, n, worker, targets) in &jobs {
            self.known[k].insert((*e, *n));
            for t in targets {
                if let Some((e2, n2)) = by_target.insert(t, (*e, *n)) {
                    self.wrong(format!("mac{k}'s coordinator holds {t} under two live leases: {e2}-{n2} and {e}-{n}"));
                }
            }
            match self.granted.get(&(*e, *n)) {
                Some((w, ts)) if w != worker || ts != targets => self.wrong(format!(
                    "lease {e}-{n} granted twice: to {w} for {ts:?}, and to {worker} for {targets:?} by mac{k}"
                )),
                Some(_) => {}
                None => {
                    self.granted.insert((*e, *n), (worker.clone(), targets.clone()));
                    self.count("leases granted");
                }
            }
        }
    }

    /// Whether two leads' coordinators grant at once (docs/pool.md §4, invariant 1): each Mac's
    /// awake, its clock now.
    fn one_granting(&mut self) {
        let mut granting = Vec::new();
        for k in 0..self.macs.len() {
            if self.awake(k) && !self.macs[k].crashed && self.macs[k].coord.is_some() {
                self.tick(k);
                if self.macs[k].coord.as_ref().is_some_and(|c| c.granting()) {
                    granting.push(k);
                }
            }
        }
        if granting.len() > 1 {
            self.wrong(format!("macs {granting:?} grant at once"));
        }
    }

    // The jobs.

    /// Mac `me` starts a job of lease (`epoch`, `n`) over `targets`, granted by Mac `by`'s
    /// coordinator: none of them under another job's live lease, its id never another's.
    ///
    /// But one: a lease the granting coordinator never held in its process, of an earlier epoch's
    /// lead, granted after the last backup of that lead's store this one took up (the owner moved
    /// the lead from a Mac asleep or cut off, or restored the lead's disk from its backup): no lead
    /// can know of it, and its job runs beside the new one until its Mac is heard from (its beat
    /// listing a lease the lead doesn't know, which the answer releases). The work is done twice;
    /// the merge takes the later lease's, by lease order (docs/coordination.md §4.4, §6.1). Counted,
    /// and the records checked as ever.
    fn starts(&mut self, me: usize, by: usize, epoch: u64, n: u64, worker: &str, targets: &[String]) {
        let mut doubled = Vec::new();
        for (l, r) in &self.running {
            if let Some(t) = r.targets.iter().find(|t| targets.contains(t)) {
                let w = format!(
                    "{t} under two live leases: mac{}'s {}-{} and mac{me}'s {epoch}-{n} (granted by mac{by})",
                    r.mac, l.0, l.1
                );
                doubled.push((*l, w));
            }
        }
        for (l, w) in doubled {
            if l.0 < epoch && !self.known[by].contains(&l) {
                self.count("jobs doubled: a lease the granting lead never held");
                self.note(|| format!("{w}: mac{by}'s coordinator never held {}-{}", l.0, l.1));
            } else {
                self.wrong(w);
            }
        }
        match self.granted.get(&(epoch, n)) {
            Some((w, ts)) if w != worker || ts != targets => self
                .wrong(format!("lease {epoch}-{n} granted twice: to {w} for {ts:?}, and to {worker} for {targets:?}")),
            Some(_) => {}
            None => {
                self.granted.insert((epoch, n), (worker.to_string(), targets.to_vec()));
                self.count("leases granted");
            }
        }
        if self.running.insert((epoch, n), Running { mac: me, targets: targets.to_vec() }).is_some() {
            self.wrong(format!("mac{me} runs lease {epoch}-{n} twice"));
        }
        self.count("jobs started");
        self.note(|| format!("mac{me} starts {epoch}-{n} on {targets:?}"));
    }

    // The NAS.

    fn see(&mut self, me: usize, path: &str, v: Option<Vec<u8>>) {
        let until = self.t + self.cfg.stale;
        self.caches[me].files.insert(path.to_string(), (v, until));
    }

    fn fetch(&mut self, me: usize, path: &str) -> Option<Vec<u8>> {
        if let Some((v, until)) = self.caches[me].files.get(path) {
            if *until > self.t {
                return v.clone();
            }
        }
        let v = self.files.get(path).cloned();
        let until = self.t + self.rng.below(self.cfg.stale + 1);
        self.caches[me].files.insert(path.to_string(), (v.clone(), until));
        v
    }

    fn read(&mut self, me: usize, path: &str) -> Option<Vec<u8>> {
        let v = self.fetch(me, path);
        if v.is_some() {
            let until = self.t + self.rng.below(OPEN + 1);
            let held = self.open.entry(path.to_string()).or_default().entry(me).or_insert(0);
            *held = (*held).max(until);
        }
        v
    }

    /// An operation's time on a share under load: its Mac waits on it, awake.
    fn slow(&mut self, me: usize) {
        if self.cfg.op_s.1 > 0 {
            let d = self.rng.range(self.cfg.op_s.0, self.cfg.op_s.1);
            self.macs[me].busy_until = self.t + d;
            *self.counts.entry("operation seconds").or_default() += d;
        }
        if self.cfg.stalls && self.faulting() && self.rng.chance(P_STALL) {
            let d = self.rng.range(300, 600);
            self.macs[me].busy_until = self.macs[me].busy_until.max(self.t + d);
            self.count("operations stalled");
            self.note(|| format!("the share stalls mac{me}'s operation {d} s"));
        }
    }

    /// Whether the share answers Mac `me`'s read or stat of `path`.
    fn answers(&mut self, me: usize, path: &str) -> Result<()> {
        if self.t < self.away[me] {
            self.count("reads the share didn't answer");
            bail!("{path}: the share doesn't answer");
        }
        Ok(())
    }

    fn create(&mut self, me: usize, path: &str) -> bool {
        if let Some(b) = self.files.get(path).cloned() {
            self.see(me, path, Some(b));
            return false;
        }
        self.files.insert(path.to_string(), Vec::new());
        self.writes.insert(path.to_string(), (me, self.t));
        self.open.entry(path.to_string()).or_default().insert(me, u64::MAX);
        self.midway = true;
        self.see(me, path, Some(Vec::new()));
        true
    }

    fn fill(&mut self, me: usize, path: &str, b: &[u8]) {
        if self.files.get(path).is_none_or(|x| !x.is_empty()) || self.writes.get(path).map(|w| w.0) != Some(me) {
            self.wrong(format!("{path} changed between mac{me}'s create and its bytes"));
        }
        self.files.insert(path.to_string(), b.to_vec());
        self.writes.insert(path.to_string(), (me, self.t));
        if let Some(h) = self.open.get_mut(path) {
            h.remove(&me);
        }
        self.see(me, path, Some(b.to_vec()));
        self.landed(me, path, b, true);
    }

    /// A create whose bytes don't land: the file left empty, no longer held by its maker.
    fn cut(&mut self, me: usize, path: &str) {
        if let Some(h) = self.open.get_mut(path) {
            h.remove(&me);
        }
        self.count("creates cut short");
        self.note(|| format!("mac{me}'s create of {path}: its bytes don't land"));
    }

    fn write_tmp(&mut self, me: usize, tmp: &str, path: &str, b: &[u8]) {
        self.files.insert(tmp.to_string(), b.to_vec());
        self.tmps.insert(tmp.to_string(), self.t);
        self.midway = true;
        // (The owner moves the lead as its save is under way: its rename lands after the next
        // lead's take-up saved.)
        if path == records::ONE
            && self.cfg.moves
            && self.faulting()
            && me == self.declared
            && self.rng.chance(P_MOVE_MIDSAVE)
        {
            self.move_lead(true);
        }
    }

    fn rename(&mut self, me: usize, tmp: &str, path: &str) -> bool {
        if self.open.get(path).is_some_and(|h| h.iter().any(|(&who, &until)| who != me && until > self.t)) {
            self.count("busy renames");
            return false;
        }
        let b = self.files.remove(tmp).unwrap_or_default();
        let since = self.tmps.remove(tmp).unwrap_or(self.t);
        let mine = self.writes.get(path).map(|w| w.0) == Some(me);
        let finishing = mine && self.files.get(path).is_some_and(|x| x.is_empty());
        if let Some(&(who, at)) = self.writes.get(path) {
            if who != me && at > since {
                self.count("late renames over another's write");
                self.note(|| {
                    format!("mac{me}'s rename of {path}, written at {since}, lands over mac{who}'s write at {at}")
                });
            }
        }
        self.files.insert(path.to_string(), b.clone());
        self.writes.insert(path.to_string(), (me, self.t));
        self.see(me, path, Some(b.clone()));
        self.landed(me, path, &b, finishing);
        true
    }

    /// A file's bytes landed: checked by what it is. `made`: by its create (or its maker finishing
    /// it).
    fn landed(&mut self, me: usize, path: &str, b: &[u8], made: bool) {
        if path.starts_with("state/journal/") || path.contains("/terms/") || path.contains("/term/") {
            self.wrong(format!("mac{me} wrote {path}: no journal or term on the NAS since tasks #1124, #1125"));
        }
        if path == lead::LEAD && !made {
            self.wrong(format!("mac{me} wrote over the lead's declaration"));
        } else if path == records::ONE {
            // The one path (docs/coordination.md steps 8b, 8c): its leaders' snapshots alone, in
            // the epoch each leads, consistent; within an epoch never going back nor losing a
            // hand-off; and none of an epoch at or after one a lead acknowledged a hand-off in
            // lacks it.
            let r = match serde_json::from_slice::<Records>(b) {
                Ok(r) => r,
                Err(err) => return self.wrong(format!("the one path's records can't be parsed: {err}")),
            };
            let e = r.epoch;
            if let Err(why) = consistent(&r) {
                self.wrong(format!("epoch {e}'s records at {} aren't consistent: {why}", r.seq));
            }
            // (A save landed late over the one path, not following the brief there
            // (`records::follows`): no take-up starts from it, nor reader trusts it past the lead's
            // next save. Counted; not taken for its epoch's records.)
            let brief =
                self.files.get(records::ONE_BRIEF).and_then(|b| serde_json::from_slice::<records::Mark>(b).ok());
            if let Some(m) = brief.filter(|m| !records::follows(&r, m)) {
                self.count("late saves the brief fences");
                return self
                    .note(|| format!("mac{me}'s save of {} landed late, past the brief ({m}): fenced", r.mark()));
            }
            // (An epoch two Macs led: the first's take-up never reached the NAS, so the next one's,
            // past no snapshot or backup of it, repeated its number (docs/pool.md §4 invariant 2's
            // limit). Its two leads' snapshots aren't compared with each other; what either
            // acknowledged is checked as ever.)
            if self.leaders.get(&e).is_some_and(|&o| o != me) && !self.landed_by.contains(&(e, me)) {
                self.repeats(e);
            }
            self.landed_by.insert((e, me));
            let repeated = self.repeated.contains(&e);
            if (self.leaders.get(&e).is_some_and(|&o| o != me) && !repeated) || e > self.macs[me].epoch {
                self.wrong(format!(
                    "mac{me} wrote epoch {e}'s records (its store's epoch {}, epoch {e} led by {:?})",
                    self.macs[me].epoch,
                    self.leaders.get(&e)
                ));
            }
            let handled: BTreeSet<String> = r.reflected.iter().chain(r.rejected.keys()).cloned().collect();
            for (k, why) in &r.rejected {
                if why.contains("by GC") && self.gc_refused.insert(k.clone()) {
                    self.count("hand-offs refused: a name GC removed under their lease");
                }
            }
            if let Some(k) = handled.iter().find(|k| !self.made.contains_key(*k)) {
                self.wrong(format!("epoch {e}'s records name {k}, which no job handed off"));
            }
            if let Some((seq, was)) = self.landed.get(&e).filter(|_| !repeated) {
                let back = (r.seq <= *seq).then(|| format!("epoch {e}'s records went from {seq} to {}", r.seq));
                let lost =
                    was.difference(&handled).next().map(|k| format!("epoch {e}'s records at {} lost {k}", r.seq));
                for w in back.into_iter().chain(lost) {
                    self.wrong(w);
                }
            }
            if self.landed.keys().next_back().is_some_and(|&last| last > e) {
                self.count("the one path's records landed late");
            }
            let lost: Vec<String> =
                self.acked.iter().filter(|&(k, &at)| at <= e && !handled.contains(k)).map(|(k, _)| k.clone()).collect();
            if let Some(k) = lost.first() {
                self.wrong(format!(
                    "epoch {e}'s records at {} lack {k}, acknowledged in epoch {} ({} in all)",
                    r.seq,
                    self.acked[k],
                    lost.len()
                ));
            }
            // (No content name GC removed: but one another disk's queue removed, which this lead's
            // queue never knew of, docs/maplog.md §7.2's limit of a lead moved or its disk
            // replaced, found only by the uploads' check.)
            let gen = self.macs[me].gen;
            for n in r.manifest.values() {
                match self.removed.get(n) {
                    Some(&(by, g, at)) if by == me && g == gen => self.wrong(format!(
                        "epoch {e}'s records at {} name {n}, which mac{me}'s GC removed at {at}",
                        r.seq
                    )),
                    Some(_) => self.count("records naming a file another disk's GC removed"),
                    None => {}
                }
            }
            self.note(|| format!("mac{me} saves epoch {e}'s records at {} ({} hand-offs)", r.seq, handled.len()));
            self.landed.insert(e, (r.seq, handled));
        }
    }

    // The map and GC (docs/maplog.md §5.4, §7).

    /// Lead `k`'s agent publishes the map of records `r` (modelled: a batch of the map's log after
    /// each save that reached the NAS, setting the names the records name and freeing those its
    /// last map named and these don't), into its feed and GC's queue `q` (`GcQueue::batch`), then
    /// tells the queue of the records as saved (crate::agent::gcqueue::records, as the agent's
    /// loop does), at `now` (its clock).
    fn publish(&mut self, k: usize, r: &Records, q: &store::map::gc::GcQueue, now: u64) {
        let (gen, seq) = (self.macs[k].gen, self.macs[k].map_seq);
        let names: BTreeSet<String> = r.manifest.values().cloned().collect();
        let was = self.maps.get(&(k, gen, seq)).cloned().unwrap_or_default();
        if seq == 0 || names != was {
            let set: BTreeMap<String, serde_json::Value> = names
                .iter()
                .filter(|n| !was.contains(*n))
                .map(|n| (format!("f/{n}"), serde_json::json!({ "file": n })))
                .collect();
            let b = store::map::Batch {
                seq: seq + 1,
                at: now.to_string(),
                set: serde_json::from_value(serde_json::json!(set)).expect("entries"),
                freed: was.difference(&names).cloned().collect(),
                ..Default::default()
            };
            let map = Arc::new(map_of(seq + 1, &names));
            self.macs[k].feed.push(map, &b);
            q.batch(&b, now);
            self.maps.insert((k, gen, seq + 1), names);
            self.macs[k].map_seq = seq + 1;
        }
        crate::agent::gcqueue::records(q, r, now);
    }

    /// Mac `from`'s map server asks lead `to`'s feed for the changes since its map (`/map/changes`,
    /// store::map::feed::Feed::changes: its cursor noted as heard now), over the network, and
    /// takes the head's map (by its batches, or a reset).
    fn map_poll(&mut self, from: usize, to: usize) {
        if self.reaches(from, to).is_some() {
            return;
        }
        self.tick(to);
        let (now, gen) = (self.clock(to), self.macs[to].gen);
        let since = match self.macs[from].map_at {
            Some((l, g, s)) if (l, g) == (to, gen) => s,
            _ => 0,
        };
        use store::map::feed::Changes;
        let head = match self.macs[to].feed.changes(since, Some(&host(from)), now) {
            Changes::NotReady => return,
            Changes::Reset(h) | Changes::Batches { head: h, .. } => h,
        };
        self.polled.insert((from, to), now);
        self.macs[from].map_at = Some((to, gen, head));
    }

    /// Lead `k`'s GC removed `names` from the NAS (its queue told them deleted first, `/gc/chunk`):
    /// none the records it last saw name, nor the map of a map server heard within GC's cursor TTL
    /// (store::map::gc::CURSOR_TTL_S); one whose server it hasn't heard so long counted.
    fn gc_removes(&mut self, k: usize, names: &[String], records: Option<&Records>) {
        let (gen, now) = (self.macs[k].gen, self.clock(k));
        for n in names {
            if records.is_some_and(|r| r.manifest.values().any(|v| v == n)) {
                self.wrong(format!("mac{k}'s GC removed {n}, which the records it saw name"));
            }
            for j in 0..self.macs.len() {
                let Some((l, g, seq)) = self.macs[j].map_at.filter(|&(l, g, _)| (l, g) == (k, gen)) else { continue };
                if !self.maps.get(&(l, g, seq)).is_some_and(|m| m.contains(n)) {
                    continue;
                }
                let heard = self.polled.get(&(j, k)).copied().unwrap_or(0);
                if now.saturating_sub(heard) < store::map::gc::CURSOR_TTL_S {
                    self.wrong(format!(
                        "mac{k}'s GC removed {n}, which mac{j}'s map server (heard {} s ago) serves",
                        now - heard
                    ));
                } else {
                    self.count("names GC removed from a map server unheard past the cursor TTL");
                }
            }
            self.note(|| format!("mac{k}'s GC removes {n}"));
            // (Again by another disk's queue, which never knew of the first: a removal of nothing,
            // gone already counting as gone.)
            match self.removed.insert(n.clone(), (k, gen, self.t)) {
                Some((by, g, _)) if (by, g) == (k, gen) => self.wrong(format!("mac{k}'s GC removed {n} twice")),
                Some(_) => self.count("names GC removed again by another disk's queue"),
                None => {}
            }
            self.count("names GC removed");
        }
    }

    /// Mac `me` opens the brief for its job of lease (`epoch`, `n`), had from Mac `from`'s
    /// coordinator (its number `coord`): never older than the one that coordinator served as it
    /// granted the lease (docs/coordination.md step 8d).
    fn opens(&mut self, me: usize, lease: (u64, u64), from: Option<(usize, u64)>, mark: (u64, u64)) {
        let Some(&(by, c, Some(at))) = self.grant_marks.get(&lease) else { return };
        if from == Some((by, c)) && mark < at {
            self.wrong(format!(
                "mac{me}'s job of {}-{} opens the brief of {mark:?}, older than {at:?} its lease was granted from",
                lease.0, lease.1
            ));
        }
    }

    // The hand-offs.

    /// Mac `me`'s job handed off `e` (its driver's step keeps it): what it is.
    fn handed_off(&mut self, me: usize, e: &Entry) {
        let Some(key) = e.key() else { return self.wrong(format!("mac{me}'s hand-off of {} has no day", e.lease)) };
        let kind = if crate::agent::seam::for_tests().outside_by(e, &|l, removal| writes(&e.step, l, removal)).is_some()
        {
            self.count("hand-offs outside their step's outputs, refused by every lead");
            Kind::Refused
        } else if !e.handoff.reused.is_empty() {
            self.count("hand-offs reusing files on the NAS");
            Kind::Either
        } else if flaky(e) {
            Kind::Either
        } else {
            Kind::Taken
        };
        if self.made.insert(key.clone(), Made { kind, mac: me, posted: false, state_lost: false }).is_some() {
            self.wrong(format!("two hand-offs of key {key}"));
        }
    }

    /// Lead `me`'s coordinator holds `keys` taken (crate::coord::handoffs): posted.
    fn holds(&mut self, me: usize, keys: BTreeSet<String>) {
        for k in keys.difference(&self.stored[me]) {
            if let Some(m) = self.made.get_mut(k) {
                if std::mem::replace(&mut m.posted, true) {
                    *self.counts.entry("hand-offs posted again").or_default() += 1;
                }
            }
            *self.counts.entry("hand-offs posted").or_default() += 1;
        }
        self.stored[me] = keys;
    }

    /// Lead `me` acknowledged `keys` in epoch `epoch` (`Out::merged`, once its saved state is on
    /// disk): each named by a snapshot of that epoch landed.
    fn ack(&mut self, me: usize, epoch: u64, keys: &[String]) {
        for key in keys {
            self.acked.entry(key.clone()).or_insert(epoch);
            self.count("hand-offs acknowledged");
            if !self.landed.get(&epoch).is_some_and(|(_, h)| h.contains(key)) {
                self.wrong(format!("mac{me} acknowledged {key} as epoch {epoch}'s before a snapshot of it named it"));
            }
        }
    }

    /// Member `me` heard its lead's answers say `keys` merged: each named by a snapshot landed.
    fn told_merged(&mut self, me: usize, keys: &[String]) {
        for k in keys {
            self.count("acknowledgements heard");
            if !self.landed.values().any(|(_, h)| h.contains(k)) {
                self.wrong(format!("mac{me} was told {k} merged, which no snapshot landed names"));
            }
        }
    }

    /// Lead `me` applied `keys` in epoch `e`: none acknowledged in it or an epoch before (merged
    /// once: a lead of an earlier epoch, moved from and leading on with its reads failing, merges
    /// into records no later lead takes up).
    fn applied(&mut self, me: usize, e: u64, keys: &[String]) {
        for k in keys {
            if let Some(&at) = self.acked.get(k).filter(|&&at| at <= e) {
                self.wrong(format!("mac{me} applied {k} again in epoch {e}, acknowledged in epoch {at}"));
            }
        }
    }

    /// A Mac restarted with its saved state lost: the hand-offs it kept (`keeps`) now have no copy
    /// but a lead's store's, if any.
    fn state_lost(&mut self, keeps: &BTreeSet<String>) {
        let named = self.landed.values().next_back().map(|(_, h)| h.clone()).unwrap_or_default();
        for k in keeps.iter().filter(|k| !named.contains(*k)) {
            if let Some(m) = self.made.get_mut(k) {
                m.state_lost = true;
                *self.counts.entry("hand-offs kept by a saved state lost").or_default() += 1;
            }
        }
    }

    // What the Macs say they believe and do.

    fn leads(&mut self, me: usize, e: Option<u64>) {
        if let Some(e) = e {
            if let Some(&other) = self.leaders.get(&e).filter(|&&o| o != me) {
                if self.landed_by.contains(&(e, other)) && !self.repeated.contains(&e) {
                    self.wrong(format!("mac{me} leads epoch {e}, which mac{other} led"));
                } else {
                    self.repeats(e);
                }
            }
            if e <= self.macs[me].led {
                self.wrong(format!("mac{me} leads epoch {e} after epoch {}", self.macs[me].led));
            }
            // (As of its epoch's raise: what landed and was taken up since, its step under way, is
            // the fences' to keep out of what it does.)
            let (landed, took) = match self.macs[me].raised {
                Some((r, landed, took)) if r == e => (landed, took),
                _ => (
                    self.landed.keys().filter(|&&k| k != e).next_back().copied(),
                    self.took.keys().next_back().copied(),
                ),
            };
            let repeated = self.repeated.contains(&e);
            if let Some(last) = landed.filter(|&k| k >= e && !(repeated && k == e)) {
                self.wrong(format!("mac{me} leads epoch {e}, not past the records' epoch {last}"));
            }
            if let Some(last) = took.filter(|&k| k >= e && !(repeated && k == e)) {
                self.wrong(format!("mac{me} leads epoch {e}, not past epoch {last} taken up before"));
            }
            self.leaders.insert(e, me);
            self.took.insert(e, (me, self.t));
            self.stuck = None;
            self.macs[me].led = e;
            self.count("take-ups");
            self.note(|| format!("mac{me} leads epoch {e}"));
        } else if let Some(was) = self.macs[me].leads {
            self.note(|| format!("mac{me} no longer leads epoch {was}"));
        }
        self.macs[me].leads = e;
    }

    /// Mac `me`, leading epoch `e`, says its records reflect what's handed off: every hand-off its
    /// step was given (`given`: posted it, and its own) but those its check holds (`held`) is named
    /// by its epoch's last snapshot landed.
    fn caught_up(&mut self, me: usize, e: u64, given: &[String], held: &[(String, String)]) {
        if self.caught.insert(e) {
            self.count("leads caught up");
        }
        let named = self.landed.get(&e).map(|(_, h)| h.clone()).unwrap_or_default();
        let held: BTreeSet<&String> = held.iter().map(|(k, _)| k).collect();
        let missing: Vec<&String> = given.iter().filter(|k| !named.contains(*k) && !held.contains(k)).collect();
        if let Some(k) = missing.first() {
            self.wrong(format!(
                "mac{me} says epoch {e}'s records are caught up; they lack {k}, handed to its step ({} in all)",
                missing.len()
            ));
        }
    }

    /// The checks at the end of a run: the declared lead leads, alone, caught up, its last take-up
    /// before its config's end's last ten minutes (the run's own, when it went on while the
    /// declared lead's take-up waited), and the one path's records name every hand-off
    /// owed.
    fn finish(&mut self) {
        let Some((&h, &(_, at))) = self.took.iter().next_back() else { return self.wrong("no take-up".into()) };
        let end = if self.waited { self.t } else { self.cfg.end };
        if at + QUIET_S > end {
            self.wrong(format!(
                "epoch {h} was taken up at {at} s, in the run's last ten minutes (of {end} s) or after: the take-ups don't settle"
            ));
        }
        let d = self.declared;
        for k in 0..self.macs.len() {
            let leads = self.macs[k].leads;
            if k == d && leads != Some(h) {
                self.wrong(format!("the declared lead mac{d} leads {leads:?} at the end, the last epoch {h}"));
            }
            if k == d && leads == Some(h) && !self.macs[k].caught {
                self.wrong(format!("the lead mac{d} isn't caught up at the end"));
            }
            // (A Mac gone for good never wakes to read the move: it leads nothing.)
            if k != d && leads.is_some() && !self.macs[k].gone {
                self.wrong(format!("mac{k} leads {leads:?} at the end, mac{d} declared"));
            }
        }
        let gone = self.made.iter().filter(|(k, m)| self.macs[m.mac].gone && self.macs[m.mac].mine.contains(*k));
        *self.counts.entry("hand-offs a Mac gone for good holds").or_default() += gone.count() as u64;
        let lost = self.made.iter().filter(|(k, m)| self.lost(k, m)).count() as u64;
        *self.counts.entry("hand-offs lost with a saved state and the lead's store").or_default() += lost;
        match self.files.get(records::ONE).and_then(|b| serde_json::from_slice::<Records>(b).ok()) {
            Some(r) => {
                if let Err(why) = consistent(&r) {
                    self.wrong(format!("epoch {}'s last records aren't consistent: {why}", r.epoch));
                }
                let lost: Vec<String> =
                    self.owed().filter(|(k, m)| !names(&r, k, m.kind)).map(|(k, _)| k.clone()).collect();
                for k in lost {
                    let m = self.made[&k];
                    self.wrong(format!(
                        "hand-off {k} is lost: epoch {}'s last records don't name it (its Mac's state lost: {}, kept by {:?}, in stores {:?})",
                        r.epoch,
                        m.state_lost,
                        (0..self.macs.len()).filter(|&j| self.macs[j].mine.contains(&k)).collect::<Vec<_>>(),
                        (0..self.macs.len()).filter(|&j| self.stored[j].contains(&k)).collect::<Vec<_>>()
                    ));
                }
            }
            None => self.wrong("no records at the one path at the end".into()),
        }
    }
}

/// The length of Mac `k`'s store's last log segment, in its folder `dir`.
fn log_len(dir: &std::path::Path) -> u64 {
    last_segment(dir).and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len())
}

fn last_segment(dir: &std::path::Path) -> Option<PathBuf> {
    let store = dir.join("coord/store");
    let last = *store::seglog::segments(&store).ok()?.last()?;
    Some(store.join(store::seglog::segment_name(last)))
}

/// The map at `seq` holding content names `names`, each at its logical name's `f/` key.
fn map_of(seq: u64, names: &BTreeSet<String>) -> store::map::Map {
    let e: BTreeMap<String, serde_json::Value> =
        names.iter().map(|n| (format!("f/{n}"), serde_json::json!({ "file": n }))).collect();
    store::map::Map::new(seq, String::new(), serde_json::from_value(serde_json::json!(e)).expect("entries"))
}

/// The world, shared by the Macs' threads.
struct Shared {
    w: Mutex<World>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, World> {
        self.w.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What a Mac's step gets once the run is over: its thread ends.
const STOP: &str = "the run is over";
/// What a dead process's operations get, until its thread restarts it.
const DEAD: &str = "this process is dead";

/// A Mac's hold on the world: its NAS, its requests, its clocks.
#[derive(Clone)]
struct Sim {
    sh: Arc<Shared>,
    me: usize,
}

impl Sim {
    fn turn(&self) -> Result<MutexGuard<'_, World>> {
        let mut w = self.sh.lock();
        while !w.done && w.turn != self.me {
            w = self.sh.cv.wait(w).unwrap_or_else(|e| e.into_inner());
        }
        if w.done {
            bail!(STOP);
        }
        Ok(w)
    }

    /// Runs `f` on the world in this Mac's turn, then the schedule's next step; an error once its
    /// process is dead.
    fn op<T>(&self, f: impl FnOnce(&mut World, usize) -> T) -> Result<T> {
        let mut w = self.turn()?;
        if w.macs[self.me].crashed {
            w.after(self.me);
            self.sh.cv.notify_all();
            bail!(DEAD);
        }
        let r = f(&mut w, self.me);
        w.after(self.me);
        if w.turn != self.me || w.done {
            self.sh.cv.notify_all();
        }
        Ok(r)
    }

    /// Runs `f` on the world in this Mac's turn, with no step of its own (its notes, counts and
    /// what its process keeps in memory), even when its process is dead.
    fn quiet<T>(&self, f: impl FnOnce(&mut World) -> T) -> Result<T> {
        let mut w = self.turn()?;
        Ok(f(&mut w))
    }

    /// `quiet`, an error once its process is dead.
    fn alive<T>(&self, f: impl FnOnce(&mut World) -> T) -> Result<T> {
        let mut w = self.turn()?;
        if w.macs[self.me].crashed {
            bail!(DEAD);
        }
        Ok(f(&mut w))
    }

    fn over(&self) -> bool {
        self.sh.lock().done
    }

    fn note(&self, s: impl FnOnce() -> String) {
        self.quiet(|w| w.note(s)).ok();
    }

    fn count(&self, what: &'static str) {
        self.quiet(|w| w.count(what)).ok();
    }

    /// A request to Mac `to`'s coordinator over the network (coordwire::client::Wire's).
    fn wire(&self, to: usize, path: &str, body: Option<&[u8]>) -> Wired {
        let me = self.me;
        match self.op(|w, _| w.call(me, to, path, body.unwrap_or_default())) {
            Ok(r) => r,
            Err(e) => Wired::Unreached(format!("{e:#}")),
        }
    }
}

impl Nas for Sim {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        if !self.op(|w, me| {
            w.slow(me);
            w.create(me, path)
        })? {
            return Ok(Created::There);
        }
        let (cut, lost) = self.op(|w, me| {
            if w.cfg.cuts && w.rng.chance(P_CUT) {
                w.cut(me, path);
                return (true, false);
            }
            w.fill(me, path, bytes);
            (false, w.cfg.cuts && w.rng.chance(P_LOST))
        })?;
        if cut {
            return Ok(Created::Unwritten(anyhow!("write {path}: the share went away (made, and left short)")));
        }
        if lost {
            self.count("lost answers");
            bail!("create {path}: the answer was lost");
        }
        Ok(Created::Made)
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let tmp = format!("{path}.mac{}.tmp", self.me);
        self.op(|w, me| {
            w.slow(me);
            w.write_tmp(me, &tmp, path, bytes)
        })?;
        for _ in 0..BUSY_TRIES {
            if self.op(|w, me| w.rename(me, &tmp, path))? {
                if self.op(|w, _| w.cfg.cuts && w.rng.chance(P_LOST))? {
                    self.count("lost answers");
                    bail!("write {path}: the answer was lost");
                }
                return Ok(());
            }
        }
        self.op(|w, _| {
            w.files.remove(&tmp);
            w.tmps.remove(&tmp);
        })?;
        bail!("rename over {path}: busy")
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.op(|w, me| {
            w.slow(me);
            w.answers(me, path)?;
            Ok(w.read(me, path))
        })?
    }

    fn exists(&self, path: &str) -> Result<bool> {
        self.op(|w, me| {
            w.slow(me);
            w.answers(me, path)?;
            Ok(w.fetch(me, path).is_some())
        })?
    }
}

impl Io for Sim {
    fn now(&self) -> u64 {
        let me = self.me;
        self.quiet(|w| w.clock(me)).unwrap_or(0)
    }

    fn awake(&self) -> u64 {
        let me = self.me;
        self.quiet(|w| w.awake_clock(me)).unwrap_or(0)
    }

    fn kept(&self) -> Result<Option<Vec<u8>>> {
        let me = self.me;
        self.alive(|w| w.macs[me].local.clone())
    }

    fn keep(&self, records: &[u8]) -> Result<()> {
        let me = self.me;
        self.alive(|w| w.macs[me].local = Some(records.to_vec()))
    }

    /// The store's epoch, raised past `past` and past another Mac's backup by `RESTORE_EPOCHS`
    /// (crate::agent::pool::epochs: its backup read through the NAS, the raise in this Mac's turn).
    fn epoch(&self, past: u64) -> Result<u64> {
        let me = self.me;
        let past = match crate::pool::store::read_backup(self) {
            Ok(Some(b)) if b.member != host(me) => past.max(b.epoch + crate::pool::store::RESTORE_EPOCHS),
            Ok(_) | Err(_) => past,
        };
        self.alive(|w| {
            let c = w.macs[me].coord.clone().ok_or_else(|| anyhow!("no coordinator's store in this process"))?;
            w.tick(me);
            let e = c.raise_epoch(past)?;
            let landed = w.landed.keys().filter(|&&k| k != e).next_back().copied();
            w.macs[me].raised = Some((e, landed, w.took.keys().next_back().copied()));
            w.macs[me].epoch = e;
            Ok(e)
        })?
    }
}

/// What a run gave: what went wrong, the counts of what happened, and its events (when traced).
struct Ran {
    wrong: Vec<String>,
    counts: Counts,
    trace: Vec<String>,
}

/// Runs seed `seed`: two to four Macs, by the seed.
fn run(seed: u64, cfg: Cfg, tracing: bool) -> Ran {
    store::seglog::UNSYNCED.store(true, Ordering::Relaxed);
    let macs = if cfg.macs > 0 { cfg.macs } else { 2 + (seed % 3) as usize };
    let sh = Arc::new(Shared { w: Mutex::new(World::new(seed, macs, cfg, tracing)), cv: Condvar::new() });
    let threads: Vec<_> = (0..macs)
        .map(|k| {
            let sh = sh.clone();
            std::thread::spawn(move || {
                let sim = Sim { sh: sh.clone(), me: k };
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mac::Mac::new(sim, k, seed).run()));
                if let Err(p) = r {
                    let why = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    let mut w = sh.lock();
                    w.wrong(format!("mac{k} panicked: {why}"));
                    w.done = true;
                    sh.cv.notify_all();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().ok();
    }
    let mut w = sh.lock();
    w.finish();
    for k in 0..w.macs.len() {
        w.stop_coord(k, true);
    }
    Ran {
        wrong: std::mem::take(&mut w.wrong),
        counts: std::mem::take(&mut w.counts),
        trace: w.trace.take().unwrap_or_default(),
    }
}

/// How many times each kind of thing happened.
type Counts = BTreeMap<&'static str, u64>;

/// Runs every seed of `seeds`, each with the run `cfg` gives it, on all cores: the seeds that went
/// wrong, with what did, and the counts of what happened in all.
fn run_all(seeds: Range<u64>, cfg: impl Fn(u64) -> Cfg + Sync) -> (Vec<(u64, Vec<String>)>, Counts) {
    let next = AtomicU64::new(seeds.start);
    let out = Mutex::new((Vec::new(), BTreeMap::new()));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let seed = next.fetch_add(1, Ordering::Relaxed);
                if seed >= seeds.end {
                    return;
                }
                let r = run(seed, cfg(seed), false);
                let mut o = out.lock().unwrap();
                if !r.wrong.is_empty() {
                    o.0.push((seed, r.wrong));
                }
                for (k, v) in r.counts {
                    *o.1.entry(k).or_insert(0) += v;
                }
            });
        }
    });
    let (mut bad, counts) = out.into_inner().unwrap();
    bad.sort();
    (bad, counts)
}

/// Runs `seeds`; the first that goes wrong is run again traced, and shown.
fn check_all(seeds: Range<u64>, cfg: impl Fn(u64) -> Cfg + Sync) -> Counts {
    let n = seeds.end - seeds.start;
    let (bad, counts) = run_all(seeds, &cfg);
    if let Some((seed, _)) = bad.first() {
        let r = run(*seed, cfg(*seed), true);
        let tail = &r.trace[r.trace.len().saturating_sub(200)..];
        let seeds: Vec<u64> = bad.iter().map(|b| b.0).take(20).collect();
        panic!(
            "{} of {n} schedules went wrong (seeds {seeds:?}…); seed {seed} ({:?}):\n{}\n\nits last events:\n{}",
            bad.len(),
            cfg(*seed),
            r.wrong.join("\n"),
            tail.join("\n")
        );
    }
    counts
}

/// A count of `name`, from the environment, or `or`.
fn var(name: &str, or: u64) -> u64 {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(or)
}

/// Each kind of take-up and fault, and what the knobs bring, that the default runs must see at
/// least three times (a simulator that never got there would pass too).
const KINDS: [&str; 31] = [
    "hand-offs outside their step's outputs, refused by every lead",
    "take-ups from its own records",
    "take-ups from the one path's records",
    "sleeps",
    "busy renames",
    "take-ups tried again",
    "lost answers",
    "Macs gone for good",
    "operation seconds",
    "saved states lost",
    "leads caught up",
    "shares away from a Mac",
    "hand-offs posted",
    "hand-offs posted again",
    "hand-offs acknowledged",
    "acknowledgements heard",
    "leases granted",
    "jobs stopped, their leases not the lead's",
    "requests dropped",
    "requests delayed",
    "requests arriving late",
    "answers lost",
    "Macs cut off",
    "one-way links",
    "leads killed mid-append",
    "processes crashed",
    "lead stores restored from the backup",
    "leads moved",
    "leads moved with their reads failing",
    "jobs not started: the lead away",
    "hand-offs reusing files on the NAS",
];

#[test]
fn the_pool_keeps_its_invariants_through_hundreds_of_schedules() {
    // (The default's: POOL_SIM_SEEDS mixed schedules, 160 by default, within the suite's budget;
    // the long runs below are by hand.)
    let n = var("POOL_SIM_SEEDS", 160);
    let counts = check_all(0..n, Cfg::mixed);
    eprintln!("{counts:#?}");
    for what in KINDS {
        assert!(counts.get(what).is_some_and(|&n| n >= 3), "{what}: {:?} in all the runs", counts.get(what));
    }
}

#[test]
#[ignore]
fn the_pool_keeps_its_invariants_through_thousands_of_schedules() {
    // #repro: POOL_SIM_SEEDS mixed schedules (2,000) from POOL_SIM_FROM (0), each of 40 minutes'
    // faults, docs/coordination.md step 9's run; and as many with every fault at once.
    let (from, n) = (var("POOL_SIM_FROM", 0), var("POOL_SIM_SEEDS", 2000));
    let started = std::time::Instant::now();
    let counts = check_all(from..from + n, Cfg::mixed);
    eprintln!("{n} mixed schedules in {:.0} s\n{counts:#?}", started.elapsed().as_secs_f64());
    let started = std::time::Instant::now();
    let counts = check_all(from..from + n, Cfg::all);
    eprintln!("{n} schedules of every fault in {:.0} s\n{counts:#?}", started.elapsed().as_secs_f64());
}

#[test]
#[ignore]
fn the_pool_keeps_its_invariants_through_a_long_run() {
    // #repro: POOL_SIM_SEEDS schedules (2,000 by default) from POOL_SIM_FROM (1,000,000) of
    // POOL_SIM_MINUTES' faults each (240: partitions up to two hours), the knobs mixed as by
    // default. (A run goes on past its end while its last lead still merges what the faults left:
    // `World::goes_on`.)
    let (from, n, faults) =
        (var("POOL_SIM_FROM", 1_000_000), var("POOL_SIM_SEEDS", 2000), var("POOL_SIM_MINUTES", 240) * 60);
    let started = std::time::Instant::now();
    let counts = check_all(from..from + n, |seed| Cfg { faults, end: faults + 1500, ..Cfg::mixed(seed) });
    eprintln!(
        "{n} schedules of {} minutes' faults in {:.0} s\n{counts:#?}",
        faults / 60,
        started.elapsed().as_secs_f64()
    );
}

#[test]
fn a_run_goes_on_only_while_its_lead_merges() {
    // (The long run's: hours of faults with the lead asleep leave hundreds of hand-offs. The run
    // goes on while its records lack fewer each ten minutes, or while the declared lead's take-up
    // waits; a lead that merges none is wrong at the end, as before.)
    let mut w = World::new(0, 4, Cfg::pool(), false);
    let (k1, k2) = ("2026-10-06/1-1".to_string(), "2026-10-06/1-2".to_string());
    w.took.insert(1, (0, 0));
    let d = w.declared;
    (w.macs[d].leads, w.macs[d].caught) = (Some(1), true);
    let m = Made { kind: Kind::Taken, mac: 1, posted: true, state_lost: false };
    w.made.insert(k1.clone(), m);
    w.made.insert(k2.clone(), m);
    let mut r = Records { epoch: 1, seq: 1, ..Default::default() };
    let save = |w: &mut World, r: &Records| w.files.insert(records::ONE.to_string(), serde_json::to_vec(r).unwrap());
    save(&mut w, &r);
    w.t = w.cfg.end;
    assert!(w.goes_on(), "two hand-offs lacking: ten minutes more");
    assert!(!w.goes_on(), "none merged since: the run ends");
    r.reflected.insert(k1);
    save(&mut w, &r);
    assert!(w.goes_on(), "one merged since: ten minutes more");
    (w.t, w.behind) = (w.cfg.end + MERGING_S, Some(2));
    assert!(!w.goes_on(), "gone on as long as it may: the run ends");
    (w.t, w.behind) = (w.cfg.end, None);
    r.reflected.insert(k2);
    save(&mut w, &r);
    assert!(!w.goes_on(), "every hand-off named: the run ends");
    w.macs[d].leads = None;
    assert!(w.goes_on(), "the declared lead's take-up waiting: ten minutes more");
    w.t = w.cfg.end + MERGING_S;
    assert!(!w.goes_on(), "gone on as long as it may: the run ends");
}

#[test]
fn every_fault_at_once_keeps_the_invariants() {
    // docs/coordination.md step 9's faults all on: drops, delays and lost answers, one-way links,
    // partitions, the lead killed mid-append, crashes, the lead's disk restored from its backup,
    // the lead moved (back, mid-save, its reads failing), with the NAS's.
    let counts = check_all(10_000..10_000 + var("POOL_SIM_SEEDS", 60), Cfg::all);
    for what in ["leads killed mid-append", "lead stores restored from the backup", "leads moved", "Macs cut off"] {
        assert!(counts.get(what).is_some_and(|&n| n >= 3), "{what}: {counts:?}");
    }
}

#[test]
fn gc_keeps_its_grace_and_cursors_through_hours_away() {
    // GC on the lead (docs/maplog.md §7, tasks #1106, #1134) through hours of faults: three Macs,
    // mac1 asleep seven hours from five minutes after the first take-up (its map server unheard past CURSOR_TTL_S, 6 h),
    // Macs cut off for up to six hours, the network's faults. A name is removed only past GRACE_S
    // (1 h), the records it saw naming none, nor any map server heard within the TTL; no records
    // name a removed name (but another disk's queue's, counted). A hand-off reusing a name removed
    // under its lease is refused, saying why (counted: rare, since a live lease, an away member's
    // too, holds every name freed after its grant, and a member's map server heard holds its map's).
    let faults = 7 * 3600 + 1800;
    let counts = check_all(30_000..30_000 + var("POOL_SIM_SEEDS", 12), |_| Cfg {
        faults,
        end: faults + 1500,
        macs: 3,
        nap: true,
        net: true,
        partitions: true,
        ..Cfg::pool()
    });
    eprintln!("{counts:#?}");
    for what in [
        "names GC removed",
        "names GC removed from a map server unheard past the cursor TTL",
        "Macs asleep seven hours",
    ] {
        assert!(counts.get(what).is_some_and(|&n| n >= 1), "{what}: {counts:?}");
    }
}

#[test]
fn the_lead_moves_and_moves_back() {
    // The owner's moves alone, often (docs/pool.md §6.1): to another Mac, back to the one before,
    // as the lead's save is under way (its rename landing after the next lead's save: the take-up's
    // brief fence, `records::brief_mark`), and with the lead's reads failing past `STALE_S` (it
    // stops granting at `VOUCH_S`). Two leads never grant at once; nothing acknowledged is lost.
    let counts = check_all(20_000..20_000 + var("POOL_SIM_SEEDS", 60), |_| Cfg {
        moves: true,
        faults: 7200,
        end: 8700,
        ..Cfg::pool()
    });
    for what in [
        "leads moved",
        "leads moved back",
        "leads moved with their reads failing",
        "take-ups from the one path's records",
    ] {
        assert!(counts.get(what).is_some_and(|&n| n >= 3), "{what}: {counts:?}");
    }
}

#[test]
fn a_seed_runs_the_same_every_time() {
    for seed in [11, 12, 13] {
        let (a, b) = (run(seed, Cfg::all(seed), true), run(seed, Cfg::all(seed), true));
        assert!(a.trace.len() > 20 && a.counts["steps"] > 500, "{} events, {:?}", a.trace.len(), a.counts);
        assert_eq!(a.trace, b.trace);
        assert_eq!(a.wrong, b.wrong);
    }
}

#[test]
#[ignore]
fn the_knobs_one_at_a_time() {
    // #repro: POOL_SIM_SEEDS schedules (1,000) with each knob alone, and how many went wrong.
    let n = var("POOL_SIM_SEEDS", 1000);
    let p = Cfg::pool();
    let knobs: [(&str, Cfg); 20] = [
        ("none", p),
        ("a Mac leaving", Cfg { leave: true, ..p }),
        ("stale 60 s", Cfg { stale: 60, ..p }),
        ("stale 300 s", Cfg { stale: 300, ..p }),
        ("skew 300 s", Cfg { skew: 300, ..p }),
        ("skew 1200 s", Cfg { skew: 1200, ..p }),
        ("downgrades", Cfg { downgrade: true, ..p }),
        ("NAS cuts and lost answers", Cfg { cuts: true, ..p }),
        ("slow operations", Cfg { op_s: (1, 4), ..p }),
        ("saved states lost", Cfg { lose: true, ..p }),
        ("reads failing", Cfg { fails: true, ..p }),
        ("operations stalled", Cfg { stalls: true, ..p }),
        ("drops, delays, lost answers", Cfg { net: true, ..p }),
        ("partitions", Cfg { partitions: true, ..p }),
        ("one-way links", Cfg { one_way: true, ..p }),
        ("kills mid-append", Cfg { kills: true, ..p }),
        ("crashes", Cfg { crashes: true, ..p }),
        ("restores from the backup", Cfg { restores: true, ..p }),
        ("lead moves", Cfg { moves: true, ..p }),
        ("every fault", Cfg::all(0)),
    ];
    for (name, cfg) in knobs {
        let (bad, c) = run_all(0..n, |seed| if name == "every fault" { Cfg::all(seed) } else { cfg });
        let get = |k: &str| c.get(k).copied().unwrap_or(0);
        eprintln!(
            "{name}: {} of {n} wrong; leases {}, take-ups {}, posted again {}{}",
            bad.len(),
            get("leases granted"),
            get("take-ups"),
            get("hand-offs posted again"),
            bad.first().map(|(s, w)| format!("; seed {s}: {}", w[0])).unwrap_or_default()
        );
    }
}

#[test]
#[ignore]
fn every_seed_runs_the_same_every_time() {
    // #repro: POOL_SIM_SEEDS seeds (400), each run twice: the same events, wrongs and counts.
    let n = var("POOL_SIM_SEEDS", 400);
    let differ: Vec<u64> = (0..n)
        .filter(|&seed| {
            let (a, b) = (run(seed, Cfg::mixed(seed), true), run(seed, Cfg::mixed(seed), true));
            a.trace != b.trace || a.wrong != b.wrong || a.counts != b.counts
        })
        .collect();
    assert!(differ.is_empty(), "{} of {n} seeds ran differently: {differ:?}", differ.len());
}

#[test]
#[ignore]
fn a_seeds_events() {
    // #repro: POOL_SIM_SEED's events, its knobs as by default, or as POOL_SIM_KNOBS lists them
    // (none: "plain"; "all", "leave", "downgrade", "cuts", "slow", "lose", "fails", "stalls", "net",
    // "partitions", "one-way", "kills", "crashes", "restores", "moves", "skew=<s>", "stale=<s>"),
    // its faults POOL_SIM_MINUTES long (40), to look into one.
    let seed = var("POOL_SIM_SEED", 0);
    let cfg = match std::env::var("POOL_SIM_KNOBS") {
        Ok(knobs) => knobs.split(',').fold(Cfg::pool(), |c, k| match k.split_once('=') {
            Some(("skew", v)) => Cfg { skew: v.parse().unwrap_or(0), ..c },
            Some(("stale", v)) => Cfg { stale: v.parse().unwrap_or(30), ..c },
            _ => match k {
                "all" => Cfg::all(seed),
                "leave" => Cfg { leave: true, ..c },
                "downgrade" => Cfg { downgrade: true, ..c },
                "cuts" => Cfg { cuts: true, ..c },
                "slow" => Cfg { op_s: (1, 4), ..c },
                "lose" => Cfg { lose: true, ..c },
                "fails" => Cfg { fails: true, ..c },
                "stalls" => Cfg { stalls: true, ..c },
                "net" => Cfg { net: true, ..c },
                "partitions" => Cfg { partitions: true, ..c },
                "one-way" => Cfg { one_way: true, ..c },
                "kills" => Cfg { kills: true, ..c },
                "crashes" => Cfg { crashes: true, ..c },
                "restores" => Cfg { restores: true, ..c },
                "moves" => Cfg { moves: true, ..c },
                _ => c,
            },
        }),
        Err(_) => Cfg::mixed(seed),
    };
    let cfg = match std::env::var("POOL_SIM_MINUTES").ok().and_then(|s| s.parse::<u64>().ok()) {
        Some(m) => Cfg { faults: m * 60, end: m * 60 + 1500, ..cfg },
        None => cfg,
    };
    let r = run(seed, cfg, true);
    eprintln!("{cfg:?}\n{}\n\n{}\n\n{:#?}", r.trace.join("\n"), r.wrong.join("\n"), r.counts);
}
