//! `scenic`, the user's command (src/bin/scenic.rs; docs/surface.md, docs/building.md §2).

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The build and its pool, from this Mac (docs/surface.md, docs/building.md §2). With no command,
/// `status`.
#[derive(Parser, Debug)]
#[command(name = "scenic", args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
    // (`scenic` alone: `scenic status`'s.)
    #[command(flatten)]
    pub status: Places,
}

/// The NAS project folder and this Mac's agent's folder.
#[derive(Args, Debug, Default)]
pub struct Places {
    /// The NAS project folder (default: the mounted share)
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
    /// This Mac's agent's folder (default: ~/Library/Application Support/scenic/agent)
    #[arg(long, value_name = "DIR")]
    pub home: Option<PathBuf>,
}

/// This Mac's agent's folder.
#[derive(Args, Debug, Default)]
pub struct Home {
    /// This Mac's agent's folder (default: ~/Library/Application Support/scenic/agent)
    #[arg(long, value_name = "DIR")]
    pub home: Option<PathBuf>,
}

/// The NAS project folder.
#[derive(Args, Debug, Default)]
pub struct Root {
    /// The NAS project folder (default: the mounted share)
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// What the lead is doing, what waits and why, the build to the end by stage and step, regions
    Status(Places),
    /// The steps table (docs/pipeline.md): every step's title, what it does, its kind, scope, stage
    /// and where it runs
    Steps {
        /// As the build page reads it
        #[arg(long)]
        json: bool,
    },
    /// The build agent (every Mac's login item, a member of the pool, leading or not as the terms
    /// say)
    Agent {
        /// One loop, then out
        #[arg(long)]
        once: bool,
        /// Plans and says what it would start, starting nothing
        #[arg(long)]
        dry_run: bool,
        /// (Of before: accepted and ignored; the pool says what a member is.)
        #[arg(long, hide = true)]
        helper: bool,
        #[command(flatten)]
        places: Places,
    },
    /// The pool as it stands (docs/pool.md): the terms, the agents; and the owner's asks of its lead
    Pool(Pool),
    /// A task a member's agent leased, run here (the agent's)
    RunTask {
        /// The task's spec (JSON)
        #[arg(long, value_name = "FILE")]
        spec: PathBuf,
        /// Its lease
        #[arg(long, value_name = "ID")]
        lease: u64,
        /// Its folder (removed when done)
        #[arg(long, value_name = "FOLDER")]
        dir: PathBuf,
        /// Where its result is written
        #[arg(long, value_name = "FILE")]
        result: PathBuf,
        #[command(flatten)]
        root: Root,
    },
    /// Who leads the build, its term, since when, a handover under way, each member and whether it
    /// can lead (docs/pool.md §11); an ask of it
    Lead(Lead),
    /// Pause the whole build: every Mac's jobs stop at their next safe point. --here or a member
    /// (host name or id): that Mac alone, its jobs given back at their next safe point, the rest of
    /// the pool going on, until resumed (kept over restarts)
    Pause {
        /// Frozen at once, not at the next safe point (the whole build only)
        #[arg(long, conflicts_with_all = ["here", "member"])]
        now: bool,
        /// This Mac alone
        #[arg(long, conflicts_with = "member")]
        here: bool,
        /// A member alone (its host name or member id)
        member: Option<String>,
        #[command(flatten)]
        home: Home,
    },
    /// Let the build go on after `scenic pause`; --here or a member: that Mac alone
    Resume {
        /// This Mac alone
        #[arg(long, conflicts_with = "member")]
        here: bool,
        /// A member alone (its host name or member id)
        member: Option<String>,
        #[command(flatten)]
        home: Home,
    },
    /// The jobs: a running one cancelled
    #[command(subcommand)]
    Job(Job),
    /// Clear this Mac's build caches once the build is done (jobs running keep what they use),
    /// after a y/N: its agent does it, and says what it freed
    Clean {
        /// No y/N
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        home: Home,
    },
    /// This Mac's disk room target: the free space its agent keeps, freeing its caches to it as far
    /// as needed and starting no job that would cross it; with none, how it stands
    Room {
        /// The target in GB, or off
        #[arg(value_name = "GB|off")]
        what: Option<String>,
        #[command(flatten)]
        places: Places,
    },
    /// The memory this Mac gives the build's jobs together, within the memory guard's limit (off:
    /// the limit); with none, how it stands
    Memory {
        /// The memory in GB, or off
        #[arg(value_name = "GB|off")]
        what: Option<String>,
        #[command(flatten)]
        places: Places,
    },
    /// This Mac's in-use share: the cores its agent keeps free of its jobs while its owner uses it
    /// (by default 2); off: the default; with none, how it stands
    InUse {
        /// The cores kept free
        #[arg(long, value_name = "N", conflicts_with = "off")]
        cores: Option<u32>,
        /// The default again
        off: Option<Off>,
        #[command(flatten)]
        places: Places,
    },
    /// The pool's NAS load target: the busiest disk's busy share the NAS budget grants up to (by
    /// default 85, 30 to 95), kept on the NAS for the lead; off: the default; with none, how it
    /// stands
    NasTarget {
        /// The target in percent, or off
        #[arg(value_name = "PERCENT|off")]
        what: Option<String>,
        #[command(flatten)]
        root: Root,
    },
    /// The NAS reserve: the free space the NAS keeps past what bld-fetch fetches there, pool-wide,
    /// kept on the NAS; off: none; with none, how it stands
    NasRoom {
        /// The reserve in GB, or off
        #[arg(value_name = "GB|off")]
        what: Option<String>,
        #[command(flatten)]
        root: Root,
    },
    /// Remove replaced files from the NAS (the agent runs it daily)
    Gc {
        /// Says what it would remove
        #[arg(long)]
        dry_run: bool,
        /// Files replaced this many days ago or more
        #[arg(long, value_name = "N", default_value_t = 14)]
        days: u64,
        #[command(flatten)]
        root: Root,
    },
    /// Back up the user's folders (the agent runs it daily)
    Backup {
        /// A local folder too
        #[arg(long, value_name = "DIR")]
        local: Option<PathBuf>,
        #[command(flatten)]
        root: Root,
    },
    /// The jobs' timings (pipeline::timings): each kind's phases over its last N runs, with their
    /// totals and shares; the build's (its coordinator's log, asked of it from another Mac), or
    /// with --here this Mac's own jobs' alone
    Timings(Timings),
    /// The archived forecasts held to what happened (docs/pool.md §7.9): each job's forecast start
    /// and end against the history's, by step, Mac, horizon and code version (bias, median and
    /// 90th percentile of the absolute error). Reads only
    ForecastCheck {
        /// From (UTC): unix seconds, YYYY-MM-DD, YYYY-MM-DDTHH:MM, or back from now (12h, 3d);
        /// default 7d
        #[arg(long, value_name = "TIME")]
        from: Option<String>,
        /// To (the same forms; default now)
        #[arg(long, value_name = "TIME")]
        to: Option<String>,
        /// As JSON
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        root: Root,
    },
    /// The input gate (docs/inputs.md §4): every gate unit, its accepted version, its state, what
    /// holds it, its acceptances (those no check raises any more marked stale)
    Inputs(Inputs),
}

/// `off`.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Off {
    Off,
}

#[derive(Args, Debug)]
pub struct Pool {
    #[command(subcommand)]
    pub cmd: Option<PoolCmd>,
    /// The NAS project folder (default: the mounted share)
    #[arg(long, value_name = "DIR", global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum PoolCmd {
    /// How the pool stands (the same as `scenic pool`)
    Status,
    /// A job's progress on a member's Mac given up: the lead runs it wherever it fits from then
    Release {
        /// The member (its host name or member id)
        member: String,
        /// The job ("osm-pass 2026-10-01")
        job: String,
    },
    /// The targets' memory floors the lead's coordinator keeps (crate::agent::memguard)
    Floors {
        /// Cleared: all, a step's, or one target's
        #[arg(long)]
        clear: bool,
        /// The step
        step: Option<String>,
        /// The target
        target: Option<String>,
    },
    /// The NAS budget's probe once (pipeline::budget::nas): small files made, a larger one written
    /// and read back, in a folder of its own (made, then removed), printing MB/s and creates/s
    NasProbe {
        /// The probe's folder (default: one under the project folder's state/pool/nas-probe)
        #[arg(long, value_name = "FOLDER")]
        dir: Option<PathBuf>,
    },
    /// Each step's model of what its jobs need (pipeline::needs): its fits' errors on held-out runs
    /// (memory, time, disk) and the steps that don't fit. Reads only
    Needs {
        /// The fits (the one view so far)
        #[arg(long, required = true)]
        fit: bool,
        /// The costs (costs.json or costs.jsonl; default: this Mac's coordinator's)
        #[arg(long, value_name = "FILE")]
        costs: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
pub struct Lead {
    #[command(subcommand)]
    pub cmd: Option<LeadCmd>,
    /// This Mac's agent's folder (default: ~/Library/Application Support/scenic/agent)
    #[arg(long, value_name = "DIR", global = true)]
    pub home: Option<PathBuf>,
    /// (Taken for the commands' common form; the lead's view is this Mac's agent's.)
    #[arg(long, value_name = "DIR", global = true, hide = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum LeadCmd {
    /// Who leads (the same as `scenic lead`)
    Status,
    /// Hand the lead to a member
    Give {
        /// The member (its host name or member id)
        member: String,
    },
    /// This Mac takes the lead over: once the lead is out of touch or stood down
    Take {
        /// With the lead in touch, past a term that can't be read, or over this Mac's own handover
        #[arg(long)]
        force: bool,
        /// On an app older than the term's
        #[arg(long)]
        downgrade: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum Job {
    /// Stop the running job building <target> (on this Mac or, through the lead, another) and give
    /// it back, not failed: no retry delay, nothing kept from its Mac, its scratch for the target
    /// cleared, so it runs again from the start
    Cancel {
        /// The job's target
        target: String,
        /// The job's step, where targets are shared
        #[arg(long, value_name = "STEP")]
        step: Option<String>,
        /// Held until the lead runs a newer app than this one
        #[arg(long)]
        rerun_after_update: bool,
        #[command(flatten)]
        home: Home,
    },
}

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Timings {
    #[command(subcommand)]
    pub cmd: Option<TimingsCmd>,
    /// One kind of job
    pub kind: Option<String>,
    /// Its last N runs
    #[arg(long, value_name = "N", default_value_t = 20)]
    pub last: usize,
    /// One Mac's runs
    #[arg(long, value_name = "MAC")]
    pub host: Option<String>,
    /// This Mac's own jobs alone
    #[arg(long, conflicts_with = "file")]
    pub here: bool,
    /// A timings log
    #[arg(long, value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// As JSON
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub places: Places,
}

#[derive(Subcommand, Debug)]
pub enum TimingsCmd {
    /// Publishing end to end over the last N catalogs: each unit from its job's end to its hand-off
    /// merged, its map tiles planned, leased and packed, the catalog, each Mac's server adopting it
    /// and its mirror through it; the map tiles each unit made stale, the steps that ran per
    /// catalog, and the medians
    Publish {
        /// The last N catalogs
        #[arg(long, value_name = "N", default_value_t = 10)]
        last: usize,
        /// A history file (default: the NAS's, else this Mac's coordinator's)
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// As JSON
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        places: Places,
    },
}

#[derive(Args, Debug)]
pub struct Inputs {
    #[command(subcommand)]
    pub cmd: Option<InputsCmd>,
    /// The NAS project folder (default: the mounted share)
    #[arg(long, value_name = "DIR", global = true)]
    pub root: Option<PathBuf>,
    /// This Mac's agent's folder (default: ~/Library/Application Support/scenic/agent)
    #[arg(long, value_name = "DIR", global = true)]
    pub home: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum InputsCmd {
    /// Every gate unit (the same as `scenic inputs`)
    List,
    /// The drop boxes listed now: the lead's agent checks what changed
    Check {
        /// One unit (default: every one)
        unit: Option<String>,
        /// Every file hashed
        #[arg(long)]
        full: bool,
    },
    /// A held warning accepted (or all the unit's), written from this Mac; errors can't be
    Accept {
        /// The unit
        unit: String,
        /// The finding
        #[arg(value_name = "FINDING ID")]
        id: Option<String>,
        /// All the unit's
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        all: bool,
    },
    /// An acceptance undone (or every stale one)
    Unaccept {
        /// The unit
        unit: String,
        /// The finding
        #[arg(value_name = "FINDING ID")]
        id: Option<String>,
        /// Every stale one
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        stale: bool,
    },
    /// The test unit `_gate-test` on the gate, or off it; with neither, how it stands
    Test {
        /// On or off
        state: Option<OnOff>,
        /// Off, its acceptances and records removed too
        #[arg(long)]
        forget: bool,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnOff {
    On,
    Off,
}
