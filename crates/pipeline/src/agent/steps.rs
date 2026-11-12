//! The steps table (docs/pipeline.md): one row per step the agent runs as a job, every row the
//! same fields, so the whole pipeline is read with a few concepts:
//!
//! - **id** (`name`): the step's stored name, in the records, the journal, the leases, the timings
//!   and the floors, and the name every reader shows (task #659: no display names); renamed only
//!   with agentcore::pool::records::RENAMED (a rename would re-key what's built: so a z6 tile's roads
//!   are still `unit`).
//! - **does**: one sentence of what it makes from what.
//! - **kind**: where its output goes (`Kind`: a fetch from outside, a derived work file, a pack the
//!   map serves, a publish, upkeep).
//! - **scope**: what one target is (`Scope`: the world, a z3 tile, a z6 tile, an input's inputs
//!   folder); its noun counts a job's targets everywhere ("8 z6 tiles").
//! - **stage**: where it sits in the build's order (`STAGES`): the checklist's lines, the page's
//!   groups and colours.
//! - **place**: which Mac runs it (`Place`: the lead alone, or any member), derived from the pool's
//!   rules (agentcore::agent::every), never kept by hand.
//! - **resources**: whether members take it (the shared steps; which ready job goes first is derived from the
//!   reads, `readers_first`, never kept by hand), whether it's the lead's alone (a
//!   run alone of the whole build, or one that keeps the pass's answers: agentcore::agent::every), and
//!   what it may write (its write-set: the logical names its jobs may add, change or remove in the
//!   build's manifest). How many targets go in a job is measured, not kept here: as many as fit a
//!   batch length (agentcore::coord::sched::Sched::batch_len), a step kept to the lead's in one job.
//!
//! Every human text about a step is templated from its row (its id, `said`, `counted`): the jobs'
//! names, the checklist, the build page, `scenic status`. The tasks a job offers (`TASKS`) are
//! described alike.
//!
//! Not in it: the disk a job starts with (the owner's room target and the job's measured disk, the disk guard
//! behind it), groups of steps never run at once on one Mac (the raw tiles' accessor, the budgets'
//! proxy and the answers' lease guard what they share where it's used), and which jobs run beside
//! which (the coordinator packs a Mac's jobs by their measures; a cold start by its provisional
//! need, from its step's runs of an earlier way: docs/pool.md §7.7), nor any memory figure (a
//! step's memory is measured, task #979: one never measured runs alone).
//!
//! The step sets (`SHARED`, `ANSWERED`) are made from the table at compile time (a set whose size
//! isn't what the table gives fails the build).
//!
//! The write-sets: one outputs function per step (`writes`, its row's), which both ends use, so
//! they can't disagree (docs/pool.md §7.3, §12 phase 4's sixth batch): a job's `Out`, bound to its
//! step (buildstore::out::Out::for_step), puts and removes only names it gives, a name outside failing
//! the job; and the pool's lead refuses every journal entry with a change outside it (`outside`,
//! in agentcore::agent::pool's check). A shared step's files for its targets (`saves`, which
//! agentcore::coord's check of a member's hand-off and the merge's check use) are checked beside it.

use agentcore::agent::every::{self, Place};

use agentcore::agent::forecast::ReadTarget;
use agentcore::agent::seam::{Along, Left};
pub use agentcore::pool::records::current;
use codecs::legacy::TileId;
use serde::{Deserialize, Serialize};

/// Where a step's output goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Brings sources in from outside (`sources/`).
    Fetch,
    /// Makes work files other steps read (`work/`, `global/`, `sources/osm/<date>/`).
    Derive,
    /// Makes what the map serves (`layers/`, `base/`, `hidata/`, …).
    Pack,
    /// Makes a catalogue: the map's new data goes out.
    Publish,
    /// Keeps the build in order: checks, lists, removals, backups; nothing the map reads.
    Keep,
}

/// What one target of a step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Everything at once: one target, the step's own name (or a pass's date).
    World,
    /// A z3 tile ("3/x/y"): 8 × 8 z6 tiles. (Stored and sent as "area": `/api/steps`' `scope`.)
    #[serde(rename = "area")]
    Z3,
    /// A z6 tile ("6/x/y"). (Stored and sent as "tile".)
    #[serde(rename = "tile")]
    Z6,
    /// A z10 tile ("10/x/y"): a deep pack's (terrain::terrain_deep::PACK_Z).
    Z10,
    /// A polar block ("s/4/x/y": a pole and a z4 tile of its grid, terrain::polar::target).
    Polar,
    /// An input's inputs folder (docs/inputs.md §4.3).
    Input,
    /// A task's piece of a job (`TASKS`).
    Task,
}

impl Scope {
    /// The noun a target is counted in.
    pub const fn noun(self) -> &'static str {
        match self {
            Scope::World => "run",
            Scope::Z3 => "z3 tile",
            Scope::Z6 => "z6 tile",
            Scope::Z10 => "z10 tile",
            Scope::Polar => "polar block",
            Scope::Input => "input",
            Scope::Task => "task",
        }
    }
}

/// A stage of the build: a group of steps in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Stage {
    pub id: &'static str,
    /// What the stage makes, in a sentence.
    pub does: &'static str,
}

/// The stages, in the build's order (docs/pipeline.md §2).
pub const STAGES: [Stage; 14] = [
    Stage { id: "pass", does: "The newest planet read once for the whole world: its sets, outlines, pieces and road values, the basemap, the water, and what the regions' steps read." },
    Stage { id: "sites", does: "The heritage sites and designated areas the regions' tiles read." },
    Stage { id: "terrain", does: "The terrain packs, a tile at a time down to each place's native zoom, assembled zoomed out by z3 tile." },
    Stage { id: "slope", does: "The shading packs (steepness and direction) from the terrain, a tile at a time down to each place's native zoom, assembled zoomed out by z3 tile." },
    Stage { id: "trees", does: "The tree cover, height and leaf type, a tile at a time, assembled zoomed out by z3 tile." },
    Stage { id: "depth", does: "The depth under the seas and lakes and its shading from the depth sources: a tile at a time where a source has fine detail, then zoomed out by z3 tile as the means of the finer." },
    Stage { id: "roads", does: "Each tile's roads, their elevations and scenery: the base packs the map's details and profiles read." },
    Stage { id: "tiles", does: "The road and rail map tiles drawn from the base packs, by tile and zoomed out by z3 tile." },
    Stage { id: "finish", does: "What publishing adds around the tiles: the road index, rail stops, ferries, the world levels, and a prune." },
    Stage { id: "trains", does: "Trains a day on the regions' rail, from timetables." },
    Stage { id: "landmarks", does: "The landmarks: candidates, peaks, Wikidata facts, heritage details, ranked and drawn, and the area overlays." },
    Stage { id: "buildings", does: "Every building in the coverage, raised in 3D, a tile at a time." },
    Stage { id: "publish", does: "A catalogue: the map's new data goes out." },
    Stage { id: "upkeep", does: "Checks, to-do lists, removals and backups; nothing the map reads." },
];

/// A step's row.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    /// Its id: the stored name, the name every reader shows (task #659: no display names).
    pub name: &'static str,
    /// What it makes from what, in a sentence.
    pub does: &'static str,
    pub kind: Kind,
    pub scope: Scope,
    /// Its stage's id (`STAGES`).
    pub stage: &'static str,
    /// Other members take its jobs (the shared steps). No preference among them is kept here:
    /// which ready job goes first is derived from the reads (`readers_first`).
    pub shared: bool,
    /// A run of the whole build's, the lead's alone (not offered to members: agentcore::agent::every).
    pub alone: bool,
    /// It keeps the pass's Wikidata and Wikipedia answers (agentcore::answers): none starts while the
    /// agent sends them as it starts.
    pub answered: bool,
    /// Its write-set: whether a job of it may write `logical` (`removal`: removes it), for some
    /// target of its own.
    pub writes: fn(&str, bool) -> bool,
    /// Its jobs only change build records (no file made, read or removed): packed beside any jobs at
    /// one thread counted for no cores, at its measured memory (task #838, agentcore::coord::sched).
    pub records_only: bool,
    /// Everything one target of it reads, and so waits for (`Read`, docs/readiness.md §2): the
    /// planner, the forecast, the page and `scenic status` all answer readiness per target from it
    /// (`readiness`, `read_targets`). Empty: it reads no other step's targets.
    pub reads: &'static [Read],
    /// The code its targets' bytes depend on, each named (docs/readiness.md §2): its own version
    /// first, then codecs and borrowed code. The key's `v` lines (`key_lines`); empty for a step
    /// whose key isn't derived from its reads (`derived`).
    pub versions: &'static [(&'static str, fn() -> String)],
    /// The way it runs now, as far as its memory and time go (agentcore::coord::cost_version): a cost
    /// measured another way says nothing of what a run takes now. Bumped with a change that moves
    /// them; never a build key.
    pub cost_v: u32,
}

/// One thing a target's run reads (docs/readiness.md §2): what it waits for (`readiness`) and what
/// its key names (`key_lines`), the same declaration.
#[derive(Clone, Copy, Debug)]
pub enum Read {
    /// Some targets of another step, by this target (any order, repeats allowed; one listed that
    /// isn't among its step's targets as built is never left), keyed by `grain`.
    Targets { step: &'static str, of: fn(&str) -> Vec<String>, grain: Grain },
    /// The mids of some targets of another step (the z9 means an assembly reads of its z6 tiles):
    /// left while a target's mid isn't current (`View::mid_left`: stale, or current without its
    /// mid, made again for it), which what reads its packs doesn't wait for. Keyed by `named`: the
    /// mids it takes in (those of its z6 tiles near the coverage), by content.
    Mids { step: &'static str, of: fn(&str) -> Vec<String>, named: Ident },
    /// Every target of another step (a chain's: it reads what all its targets make, e.g.
    /// depth-root every depth-lo assembly), keyed by `grain`.
    Whole { step: &'static str, grain: Grain },
    /// An input that is no step's target (a spacing file, a pass's set): `Ready::Unknown` while
    /// the caller's `View` can't work it out; keyed on what `of` says of it for a target (its
    /// identity: the lines the run's own code reads it by, crate::agent::idents), None while it
    /// can't be worked out.
    Input { name: &'static str, of: Ident },
    /// Some targets of another step that a target reads along its own long ways (z6 tiles':
    /// roads::stage::Area), past those its name gives (its step's `Targets` read of the same step):
    /// from the pass's reaches, which the caller has worked out into its view (`View::along`: the
    /// planner's `Left::along`, the forecast's input), none where it hasn't. Waited for as a
    /// `Targets` read with a `Grain::Tiles`, only for them and not what they wait for; keyed by
    /// that `Targets` read's tiles, which name what's read in them.
    Along { step: &'static str, of: fn(&str, &roads::reach::Reach) -> Vec<String> },
    /// Some targets of another step that a target reads where what's built says, not its name (the
    /// terrain's lo packs under an enclosed sea, which the water step finds: task #982): worked out
    /// by the caller from what it holds into its view (`along_add_built`: the planner's
    /// `Left::along`, the forecast's input), none where it hasn't. Waited for as an `Along`; keyed
    /// by `grain`, as a `Targets` read of the targets `of` names.
    Built { step: &'static str, of: fn(&str, &super::build::KeyCtx) -> Vec<String>, grain: Grain },
}

/// An input's identity for a target (`Read::Input`), or what a target reads of another step's
/// outputs (`Grain::Read`): lines, each naming one thing read by content. None while it can't be
/// worked out now (the key is unknown then: the target waits).
pub type Ident = fn(&str, &super::build::KeyCtx) -> Option<Vec<String>>;

/// How a key names a read of another step's targets (docs/readiness.md §2).
#[derive(Clone, Copy, Debug)]
pub enum Grain {
    /// Their outputs by content name (`outputs_of`): the default.
    Outputs,
    /// Some of their outputs, by content (`Ident`'s lines: the pass's own of a step that keeps
    /// every pass's, the heritage slices near a z6 tile).
    Read(Ident),
    /// What it reads inside them, by the run's own code (the terrain tiles a slope tile or a z6
    /// tile's roads read, by their XXH3): a change elsewhere in those outputs doesn't re-key, so it
    /// doesn't wait for what they read (`held`), only for them.
    Tiles(Ident),
}

impl Grain {
    /// Whether a target reading with it waits for what its read targets wait for (`held`): it
    /// names their outputs, which change when those are made again; not `Tiles`, which may not.
    const fn passes_on(self) -> bool {
        !matches!(self, Grain::Tiles(_))
    }
}

/// Target `t` of `step`'s `Read::Along`s, with its reach `r`, into `along`.
pub fn along_add(along: &mut Along, step: &str, t: &str, r: &roads::reach::Reach) {
    for x in row(step).map_or(&[][..], |r| r.reads) {
        if let Read::Along { step: s, of } = *x {
            along.put(step, t, s, of(t, r));
        }
    }
}

/// Target `t` of `step`'s `Read::Built`s, by what `x` holds, into `along`.
pub fn along_add_built(along: &mut Along, step: &str, t: &str, x: &super::build::KeyCtx) {
    for r in row(step).map_or(&[][..], |r| r.reads) {
        if let Read::Built { step: s, of, .. } = *r {
            along.put(step, t, s, of(t, x));
        }
    }
}

/// Whether a target may run (`readiness`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ready {
    Yes,
    /// The reads still left: (step, target), a `Read::Whole`'s target its step's own name.
    Waits(Vec<(&'static str, String)>),
    /// The inputs that can't be worked out now.
    Unknown(Vec<&'static str>),
}

/// What a caller knows of the build, for `readiness` (docs/readiness.md §3).
pub trait View {
    /// Target `t` of `step` is left (stale or unbuilt, runnable or not).
    fn left(&self, step: &str, t: &str) -> bool;
    /// Target `t`'s mid isn't current (a `Read::Mids`): it's left, or current without its mid.
    fn mid_left(&self, step: &str, t: &str) -> bool {
        self.left(step, t)
    }
    /// Any target of `step` is left (a `Read::Whole`).
    fn any_left(&self, step: &str) -> bool;
    /// Input `name` can be worked out for target `t` (`Read::Input`); known unless the view says.
    fn known(&self, _name: &str, _t: &str) -> bool {
        true
    }
    /// What targets read along their long ways (`Read::Along`), as worked out for this view; none
    /// unless it says.
    fn along(&self) -> Option<&Along> {
        None
    }
}

/// What target `t` of `step` waits for, its row's declaration (`Step::reads`), its reads along its
/// long ways from `along`: the one list the planner and the forecast both wait on. Sorted, no
/// repeats; inputs aren't targets.
pub fn read_targets(step: &str, t: &str, along: Option<&Along>) -> Vec<ReadTarget> {
    let mut v: Vec<ReadTarget> = read_targets_passing(step, t, along).into_iter().map(|r| r.0).collect();
    v.dedup();
    v
}

/// `read_targets`, each with whether a wait passes on through it (`passes_on`): sorted, no
/// repeats.
pub fn read_targets_passing(step: &str, t: &str, along: Option<&Along>) -> Vec<(ReadTarget, bool)> {
    let mut v: Vec<(ReadTarget, bool)> =
        declared(step, t, along).into_iter().map(|(r, passes, _)| (r, passes)).collect();
    v.dedup();
    v
}

/// Whether a wait passes on through read `r` (`readiness`): a target read that isn't left still
/// holds its reader while it waits itself (`held`: a `Read::Targets` of a grain that `passes_on`,
/// a `Read::Mids`), and so does a step read whole none of whose targets is left while anything it
/// reads is (`whole_left`: a `Read::Whole` of a grain that passes on); an `Along` or `Built` read
/// holds it only while left. The one rule `readiness` and the forecast (agentcore::agent::forecast's
/// `Waits`, through `read_targets_passing` and `read_steps`) wait by.
fn passes_on(r: &Read) -> bool {
    match *r {
        Read::Targets { grain, .. } | Read::Whole { grain, .. } => grain.passes_on(),
        Read::Mids { .. } => true,
        Read::Along { .. } | Read::Built { .. } | Read::Input { .. } => false,
    }
}

/// Target `t`'s declared reads of other steps' targets (`readiness`'s list, its reads along its
/// long ways from `along`): each with whether a wait passes on through it (`passes_on`) and
/// whether it's of a mid (`Read::Mids`: left by `View::mid_left`). Sorted, no repeats.
fn declared(step: &str, t: &str, along: Option<&Along>) -> Vec<(ReadTarget, bool, bool)> {
    let mut v: Vec<(ReadTarget, bool, bool)> = Vec::new();
    for r in row(step).map_or(&[][..], |r| r.reads) {
        let passes = passes_on(r);
        match *r {
            Read::Targets { step, of, .. } => {
                v.extend(of(t).into_iter().map(|u| (ReadTarget::One(step, u), passes, false)))
            }
            Read::Mids { step, of, .. } => {
                v.extend(of(t).into_iter().map(|u| (ReadTarget::One(step, u), passes, true)))
            }
            Read::Along { step: s, .. } | Read::Built { step: s, .. } => v.extend(
                along
                    .map_or(&[][..], |a| a.get(step, t, s))
                    .iter()
                    .map(|u| (ReadTarget::One(s, u.clone()), passes, false)),
            ),
            Read::Whole { step, .. } => v.push((ReadTarget::Whole(step), passes, false)),
            Read::Input { .. } => {}
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The steps `step`'s row reads targets of, each with whether a wait passes on through it
/// (`passes_on`). Those that pass on are what `whole_left` reads on from `step` read whole: any of
/// their targets left holds what reads `step` whole while none of `step`'s own is. Sorted, no
/// repeats.
pub fn read_steps(step: &str) -> Vec<(&'static str, bool)> {
    let mut v: Vec<(&'static str, bool)> = row(step)
        .map_or(&[][..], |r| r.reads)
        .iter()
        .filter_map(|r| match *r {
            Read::Targets { step: s, .. }
            | Read::Whole { step: s, .. }
            | Read::Mids { step: s, .. }
            | Read::Along { step: s, .. }
            | Read::Built { step: s, .. } => Some((s, passes_on(r))),
            Read::Input { .. } => None,
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Whether target `t` of `step` may run in `view` (docs/readiness.md §3): `Waits` while a target or
/// step it reads is left or itself waits for one (`held`, `whole_left`), else `Unknown` while an input it reads can't be worked out, else `Yes`.
pub fn readiness(step: &str, t: &str, view: &dyn View) -> Ready {
    let mut waits: Vec<(&'static str, String)> = Vec::new();
    for (r, passes, mid) in declared(step, t, view.along()) {
        match r {
            ReadTarget::One(s, u) => {
                let left = if mid { view.mid_left(s, &u) } else { view.left(s, &u) };
                if left || (passes && held(s, &u, view)) {
                    waits.push((s, u));
                }
            }
            ReadTarget::Whole(s) => {
                if view.any_left(s) || (passes && whole_left(s, view)) {
                    waits.push((s, s.to_string()));
                }
            }
        }
    }
    waits.sort_unstable();
    waits.dedup();
    if !waits.is_empty() {
        return Ready::Waits(waits);
    }
    let unknown: Vec<&'static str> = row(step)
        .map_or(&[][..], |r| r.reads)
        .iter()
        .filter_map(|r| match *r {
            Read::Input { name, .. } => (!view.known(name, t)).then_some(name),
            _ => None,
        })
        .collect();
    if unknown.is_empty() {
        Ready::Yes
    } else {
        Ready::Unknown(unknown)
    }
}

/// Whether target `u` of `step` waits for something left upstream (`readiness` says `Waits`): what
/// it makes is about to change, so what reads it waits too, though its own record is current. So a
/// step declares only what its run reads (its key's lines), never what its reads read.
/// Only what's stale passes on: a mid made again for a target whose packs stay (`View::mid_left`)
/// holds what reads the mids, not what reads the packs made from them.
fn held(step: &str, u: &str, view: &dyn View) -> bool {
    struct Stale<'a>(&'a dyn View);
    impl View for Stale<'_> {
        fn left(&self, step: &str, t: &str) -> bool {
            self.0.left(step, t)
        }
        fn any_left(&self, step: &str) -> bool {
            self.0.any_left(step)
        }
        fn known(&self, name: &str, t: &str) -> bool {
            self.0.known(name, t)
        }
        fn along(&self) -> Option<&Along> {
            self.0.along()
        }
    }
    matches!(readiness(step, u, &Stale(view)), Ready::Waits(_))
}

/// Whether `step`, read whole, is left: a target of it is, or something any of its targets read
/// (any target of each step it reads, whole).
fn whole_left(step: &str, view: &dyn View) -> bool {
    view.any_left(step) || read_steps(step).into_iter().any(|(s, passes)| passes && whole_left(s, view))
}

/// Target `t`'s key of `step` (docs/readiness.md §4): its `derived_key` (a "none" target's is
/// `none_key`); None while it can't be worked out, or for a step whose key isn't derived
/// (`derived`).
pub fn key(step: &str, t: &str, x: &super::build::KeyCtx) -> Option<String> {
    derived_key(step, t, x)
}

/// Whether `step`'s keys are derived from its declared reads (it declares its versions). Not
/// derived: those keyed on the whole plan's state (pack(T) and lo: their owners, by every base
/// pack's reach; prune: no key; the catalogue: what's ready to list; lakefill: its source's pin,
/// the plan's; names-todo: the map's sequence served) and the pass's and upkeep's own (keyed in
/// agent::plan).
pub fn derived(step: &str) -> bool {
    row(step).is_some_and(|r| !r.versions.is_empty())
}

/// Target `t`'s scheme-2 key of `step` (docs/readiness.md §4): `hash16` of its `key_lines`; kept
/// in the context's memo when it has one (build::KeyMemo).
pub fn derived_key(step: &str, t: &str, x: &super::build::KeyCtx) -> Option<String> {
    let derive = || Some(store::naming::hash16(key_lines(step, t, x)?.join("\n").as_bytes()));
    match x.memo {
        Some(memo) => memo.get_or(step, t, derive),
        None => derive(),
    }
}

/// A "none" target's scheme-2 key (one the coverage left, kept to drop its outputs: it reads
/// nothing): its scheme, step, target and versions, then `none`. None for a step with no versions
/// declared.
pub fn none_key(step: &str, t: &str) -> Option<String> {
    let mut lines = head_lines(step, t)?;
    lines.push("none".to_string());
    Some(store::naming::hash16(lines.join("\n").as_bytes()))
}

/// Lines 1-3 of a key: the scheme, the step and target, a line per version in declared order.
/// None for a step with no versions declared (its key isn't derived yet).
fn head_lines(step: &str, t: &str) -> Option<Vec<String>> {
    let r = row(step).filter(|r| !r.versions.is_empty())?;
    let mut lines = vec![format!("scheme 2"), format!("step {} {t}", r.name)];
    lines.extend(r.versions.iter().map(|(n, v)| format!("v {n} {}", v())));
    Some(lines)
}

/// Target `t`'s key lines under scheme 2 (docs/readiness.md §4): `head_lines`, then one line per
/// thing it reads, from its declared reads (`Step::reads`), sorted: `t <step> <target>
/// <logical>=<content>` per output of a read target (`t <step> <target> -` for one with none),
/// `mid <step> <target> <content>` per mid, `w <step> <logical>=<content>` per output of a step
/// read whole, a `Grain::Read`'s lines after `t <step>` or `w <step>`, and `in <name> <line>` per
/// line of an input's identity. None while any of them can't be worked out, or for a step with no
/// versions declared.
pub fn key_lines(step: &str, t: &str, x: &super::build::KeyCtx) -> Option<Vec<String>> {
    let mut lines = head_lines(step, t)?;
    let mut reads: Vec<String> = Vec::new();
    for r in row(step)?.reads {
        match *r {
            Read::Targets { step: s, grain, .. } | Read::Built { step: s, grain, .. } => match grain {
                Grain::Outputs => {
                    let of: Vec<String> = match *r {
                        Read::Built { of, .. } => of(t, x),
                        Read::Targets { of, .. } => of(t),
                        _ => unreachable!(),
                    };
                    for u in of {
                        reads.extend(outputs_of(s, &u, x.m).map(|o| format!("t {s} {u} {o}")));
                    }
                }
                Grain::Read(f) => reads.extend(f(t, x)?.into_iter().map(|l| format!("t {s} {l}"))),
                Grain::Tiles(f) => reads.extend(f(t, x)?.into_iter().map(|l| format!("t {s} tile {l}"))),
            },
            Read::Mids { step: s, named, .. } => reads.extend(named(t, x)?.into_iter().map(|l| format!("mid {s} {l}"))),
            Read::Whole { step: s, grain } => match grain {
                Grain::Outputs => {
                    let all: Vec<String> = whole_outputs(s, x.m).collect();
                    if all.is_empty() {
                        reads.push(format!("w {s} -"));
                    }
                    reads.extend(all.into_iter().map(|o| format!("w {s} {o}")));
                }
                Grain::Read(f) | Grain::Tiles(f) => reads.extend(f(t, x)?.into_iter().map(|l| format!("w {s} {l}"))),
            },
            Read::Input { name, of } => reads.extend(of(t, x)?.into_iter().map(|l| format!("in {name} {l}"))),
            Read::Along { .. } => {}
        }
    }
    reads.sort_unstable();
    reads.dedup();
    lines.extend(reads);
    Some(lines)
}

/// The reads of target `t` of `step` whose lines can't be worked out now (`key_lines` None), by
/// what they read: what its scheme-2 key waits for.
pub fn unknown_reads(step: &str, t: &str, x: &super::build::KeyCtx) -> Vec<String> {
    let mut out = Vec::new();
    for r in row(step).map_or(&[][..], |r| r.reads) {
        let (what, known) = match *r {
            Read::Targets { step: s, grain: Grain::Read(f) | Grain::Tiles(f), .. }
            | Read::Built { step: s, grain: Grain::Read(f) | Grain::Tiles(f), .. } => {
                (format!("t {s}"), f(t, x).is_some())
            }
            Read::Mids { step: s, named, .. } => (format!("mid {s}"), named(t, x).is_some()),
            Read::Whole { step: s, grain: Grain::Read(f) | Grain::Tiles(f) } => (format!("w {s}"), f(t, x).is_some()),
            Read::Input { name, of } => (format!("in {name}"), of(t, x).is_some()),
            _ => continue,
        };
        if !known {
            out.push(what);
        }
    }
    out
}

/// Target `u` of `step`'s outputs as a key names them: `<logical>=<content>` per one in the
/// manifest `m` (`output_names`), `-` with none.
fn outputs_of(step: &str, u: &str, m: &std::collections::BTreeMap<String, String>) -> impl Iterator<Item = String> {
    let mut v: Vec<String> = output_names(step, u)
        .or_else(|| work_names(step, u))
        .unwrap_or_else(|| panic!("{step}'s outputs are named (output_names): a read of its targets keys on them"))
        .into_iter()
        .filter_map(|l| m.get(&l).map(|c| format!("{l}={c}")))
        .collect();
    if v.is_empty() {
        v.push("-".to_string());
    }
    v.into_iter()
}

/// The work files (not published) a target of `step` makes that another step's targets read:
/// a z6 tile's candidates and peaks.
fn work_names(step: &str, u: &str) -> Option<Vec<String>> {
    let t = TileId::parse(u).filter(|t| t.z == 6)?;
    matches!(current(step), "pois" | "peaks").then(|| vec![format!("work/{}/{}", current(step), t.dash())])
}

/// Every output of `step` in the manifest `m` (its write-set's names: `writes`), `<logical>=<content>`.
fn whole_outputs<'a>(
    step: &'a str,
    m: &'a std::collections::BTreeMap<String, String>,
) -> impl Iterator<Item = String> + 'a {
    let w = row(step).map(|r| r.writes);
    m.iter().filter(move |(l, _)| w.is_some_and(|w| w(l, false))).map(|(l, c)| format!("{l}={c}"))
}

/// The steps a target of `step` reads whole (`Read::Whole`), in declared order.
pub fn wholes(step: &str) -> impl Iterator<Item = &'static str> {
    row(step).map_or(&[][..], |r| r.reads).iter().filter_map(|r| match *r {
        Read::Whole { step, .. } => Some(step),
        _ => None,
    })
}

/// Whether `readiness` says `Yes`.
pub fn ready(step: &str, t: &str, view: &dyn View) -> bool {
    readiness(step, t, view) == Ready::Yes
}

/// The waiting note of a step's targets held (`seam::Planned::held`, the status's `waiting` list, the
/// checklist's notes), from what they wait for (their `Ready::Waits`, any number of targets'):
/// "wait for terrain-hi (12), terrain-lo 3/1/2, summits". Never hand-written: what a target waits
/// for is its declared reads.
pub fn wait_note<'a>(waits: impl IntoIterator<Item = &'a (&'static str, String)>) -> String {
    let mut by: std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> = Default::default();
    for (s, t) in waits {
        by.entry(s).or_default().insert(t);
    }
    let parts: Vec<String> = by
        .into_iter()
        .map(|(s, ts)| match ts.len() {
            1 if ts.contains(s) => s.to_string(),
            1 => format!("{s} {}", ts.first().unwrap()),
            n => format!("{s} ({n})"),
        })
        .collect();
    format!("wait for {}", parts.join(", "))
}

/// The steps that read input `name` (`Read::Input`).
pub fn input_readers(name: &str) -> impl Iterator<Item = &'static str> + '_ {
    TABLE
        .iter()
        .filter(move |r| r.reads.iter().any(|x| matches!(x, Read::Input { name: n, .. } if *n == name)))
        .map(|r| r.name)
}

impl View for Left {
    fn left(&self, step: &str, t: &str) -> bool {
        self.whole.contains(step) || self.targets.get(step).is_some_and(|s| s.contains(t))
    }

    fn mid_left(&self, step: &str, t: &str) -> bool {
        self.whole.contains(step) || self.mids.get(step).is_some_and(|s| s.contains(t))
    }

    fn any_left(&self, step: &str) -> bool {
        self.whole.contains(step) || self.targets.get(step).is_some_and(|s| !s.is_empty())
    }

    fn known(&self, name: &str, t: &str) -> bool {
        !self.unknown.contains(name) && !self.unknown_for.get(name).is_some_and(|u| u.targets.contains(t))
    }

    fn along(&self) -> Option<&Along> {
        Some(&self.along)
    }
}

/// The z6 tiles of z3 tile (x, y).
fn z6_under(x: u32, y: u32) -> impl Iterator<Item = (u32, u32)> {
    (0..64u32).map(move |i| (x * 8 + i % 8, y * 8 + i / 8))
}

/// (z, x, y) of a target, at zoom `z` only.
fn at(t: &str, z: u8) -> Option<(u32, u32)> {
    TileId::parse(t).filter(|u| u.z == z).map(|u| (u.x, u.y))
}

/// A tile and its edge neighbours (Horn's border: terrain::slope_pack::slope_tile).
fn with_edges(z: u8, x: u32, y: u32) -> impl Iterator<Item = (u32, u32)> {
    std::iter::once((x, y)).chain(super::build::edge_neighbours(z, x, y))
}

/// The tiles of `v` at zoom `z`, as targets.
fn named(z: u8, v: impl IntoIterator<Item = (u32, u32)>) -> Vec<String> {
    v.into_iter().map(|(x, y)| format!("{z}/{x}/{y}")).collect()
}

/// A z3 tile's z6 tiles (terrain-lo's terrain-hi, slope-lo's slope-hi, trees-lo's trees,
/// depth-lo's depth, bldfield-lo's bldtiles: their mids or packs).
fn z6_of_z3(t: &str) -> Vec<String> {
    named(6, at(t, 3).into_iter().flat_map(|(x, y)| z6_under(x, y)))
}

/// terrain-deep 10/x/y: its z6 tile (the pyramids-fill target that fetches its pyramids' tiles).
fn z6_of_z10(t: &str) -> Vec<String> {
    let pz = terrain::terrain_deep::PACK_Z;
    named(6, at(t, pz).into_iter().map(|(x, y)| (x >> (pz - 6), y >> (pz - 6))))
}

/// slope-deep 10/x/y: the deep packs of it and its edge neighbours (`slope_deep_from`).
fn sd_deep(t: &str) -> Vec<String> {
    let pz = terrain::terrain_deep::PACK_Z;
    named(pz, at(t, pz).into_iter().flat_map(|(x, y)| with_edges(pz, x, y)))
}

/// slope-hi 6/x/y: the hi packs of it and its edge neighbours, their z3 tiles' lo packs
/// (`slope_piece_from`), and its z10 tiles' slope-deep parts and terrain deep packs (their z13
/// tiles).
fn sh_hi(t: &str) -> Vec<String> {
    named(6, at(t, 6).into_iter().flat_map(|(x, y)| with_edges(6, x, y)))
}
fn sh_lo(t: &str) -> Vec<String> {
    named(3, at(t, 6).into_iter().flat_map(|(x, y)| with_edges(6, x, y)).map(|(x, y)| (x >> 3, y >> 3)))
}
fn sh_deep(t: &str) -> Vec<String> {
    let pz = terrain::terrain_deep::PACK_Z;
    let s = 1u32 << (pz - 6);
    named(pz, at(t, 6).into_iter().flat_map(|(x, y)| (0..s * s).map(move |i| (x * s + i % s, y * s + i / s))))
}

/// lake-depth 6/x/y: the terrain hi packs of it and the ring of 8 around it (its small lakes'
/// slope, read up to 160 m past a lake reaching its edge).
fn ld_hi(t: &str) -> Vec<String> {
    named(6, at(t, 6).into_iter().flat_map(|(x, y)| terrain::depth::lakes::ring((x, y))))
}

/// depth 6/x/y and depth-lo 3/x/y: the lake-depth packs their sources' reads meet
/// (terrain::depth::lakes::read_by).
fn d_lakes(t: &str) -> Vec<String> {
    TileId::parse(t).map_or_else(Vec::new, |u| named(6, terrain::depth::lakes::read_by(u.z, u.x, u.y)))
}

/// slope-lo 3/x/y: the lo terrain of it and its edge neighbours.
fn sl_lo(t: &str) -> Vec<String> {
    named(3, at(t, 3).into_iter().flat_map(|(x, y)| with_edges(3, x, y)))
}

/// unit 6/x/y: the terrain packs over its tile + roads::stage::MARGIN_KM its staged tiles and its
/// roads' heights read (`build::unit_terrain_from`, `build::unit_deep_tiles`): the lo packs of z4–8's
/// z3 tiles, the hi packs of its box's z6 tiles (z9–12's), its box's deep packs. Those along its own
/// long ways, which it stages too (roads::stage::Area), are its `Read::Along`s (`ua_*`).
fn unit_reads(t: &str, z: u8) -> Vec<String> {
    let Some((x, y)) = at(t, 6) else { return Vec::new() };
    let b = roadcore::geo::tile_box_grown(6, x, y, roads::stage::MARGIN_KM);
    named(z, packs_over(z, |zz| roadcore::geo::tiles_in(zz, b)))
}

/// The packs at zoom `z` (3: lo, 6: hi, `terrain_deep::PACK_Z`: deep) holding the tiles a z6 tile
/// stages, `tiles` by zoom: z4–8's for lo, z6's and z9–12's for hi, the deep zoom's own.
fn packs_over(z: u8, tiles: impl Fn(u8) -> Vec<(u32, u32)>) -> std::collections::BTreeSet<(u32, u32)> {
    let mut v = std::collections::BTreeSet::new();
    let mut under = |zs: std::ops::RangeInclusive<u8>, to: u8| {
        for zz in zs {
            v.extend(tiles(zz).into_iter().map(|(a, c)| (a >> (zz - to), c >> (zz - to))));
        }
    };
    match z {
        3 => under(4..=8, 3),
        6 => under(9..=12, 6),
        _ => {}
    }
    if z == 6 || z == terrain::terrain_deep::PACK_Z {
        v.extend(tiles(z));
    }
    v
}

/// unit 6/x/y's packs at zoom `z` along its own long ways (roads::stage::Area, its reach `r`) past
/// those of its box (`unit_reads`: the packs at `z` meeting it, give or take one on its edge, which
/// would be read twice over).
fn unit_reads_along(t: &str, r: &roads::reach::Reach, z: u8) -> Vec<String> {
    let Some((x, y)) = at(t, 6) else { return Vec::new() };
    let area = roads::stage::Area::of(TileId { z: 6, x, y }, Some(r));
    let [x0, x1, y0, y1] = roadcore::geo::tile_range(z, area.b);
    let boxed = |&(a, c): &(u32, u32)| (x0..=x1).contains(&a) && (y0..=y1).contains(&c);
    named(z, packs_over(z, |zz| area.along_tiles(zz)).into_iter().filter(|p| !boxed(p)))
}
fn u_lo(t: &str) -> Vec<String> {
    unit_reads(t, 3)
}
fn u_hi(t: &str) -> Vec<String> {
    unit_reads(t, 6)
}
fn u_deep(t: &str) -> Vec<String> {
    unit_reads(t, terrain::terrain_deep::PACK_Z)
}
fn ua_lo(t: &str, r: &roads::reach::Reach) -> Vec<String> {
    unit_reads_along(t, r, 3)
}
fn ua_hi(t: &str, r: &roads::reach::Reach) -> Vec<String> {
    unit_reads_along(t, r, 6)
}
fn ua_deep(t: &str, r: &roads::reach::Reach) -> Vec<String> {
    unit_reads_along(t, r, terrain::terrain_deep::PACK_Z)
}

/// polar-slope s/4/x/y: its block's terrain and its neighbours' (terrain::polar::around).
fn ps_terrain(t: &str) -> Vec<String> {
    terrain::polar::parse_target(t).map_or_else(Vec::new, |(pole, b)| {
        std::iter::once(b).chain(terrain::polar::around(pole, b)).map(|n| terrain::polar::target(pole, n)).collect()
    })
}

/// polar-root `s`: its pole's blocks (terrain::polar::blocks).
fn pole_blocks(t: &str) -> Vec<String> {
    roadcore::polar::Pole::of_letter(t).map_or_else(Vec::new, |p| {
        terrain::polar::blocks(p).into_iter().map(|b| terrain::polar::target(p, b)).collect()
    })
}

/// bldtiles 6/x/y: the normalized files of it and its 8 neighbours (`build::bld_around`).
fn bt_prep(t: &str) -> Vec<String> {
    at(t, 6).map_or_else(Vec::new, |(x, y)| {
        super::build::bld_around(x, y)
            .into_iter()
            .filter_map(|n| match n {
                (Some(a), Some(b)) => Some(format!("6/{a}/{b}")),
                _ => None,
            })
            .collect()
    })
}

/// peaks 6/x/y: its own tile's candidates.
fn own(t: &str) -> Vec<String> {
    vec![t.to_string()]
}

/// peaks 6/x/y: the terrain hi packs within 30 km of it, across the antimeridian too (its z12).
fn peaks_hi(t: &str) -> Vec<String> {
    named(
        6,
        at(t, 6)
            .into_iter()
            .flat_map(|(x, y)| super::build::tiles_in_wrapped(6, roadcore::geo::tile_box_grown(6, x, y, 30.0))),
    )
}

use super::idents;
use Grain::Outputs;
use Read::{Along as OfAlong, Built, Input as In, Mids, Targets as Of, Whole as All};

/// A row with what most steps have: 1.5 GB until measured, none of the sets.
const fn step(
    stage: &'static str,
    kind: Kind,
    scope: Scope,
    name: &'static str,
    does: &'static str,
    writes: fn(&str, bool) -> bool,
) -> Step {
    Step {
        name,
        does,
        kind,
        scope,
        stage,
        shared: false,
        alone: false,
        answered: false,
        writes,
        records_only: false,
        reads: &[],
        versions: &[],
        cost_v: 0,
    }
}

use Kind::{Derive, Fetch, Keep, Pack, Publish};
use Scope::{Input, Polar, World, Z10, Z3, Z6};

/// The table, by stage (the ranks order what's ranked).
pub const TABLE: [Step; 61] = [
    // The OSM pass: the planet filtered, its sets, extracts and road values; then its worldwide steps.
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "osm-pass",
            "Filters the newest planet, cuts its sets and z6 pieces, makes the basemap and the road values.",
            w_osm_pass,
        )
    },
    // (The declared planet onto the NAS once, before its pass: inputs::planet. Outside the records:
    // the pass reads it by its path.)
    step(
        "pass",
        Fetch,
        World,
        "planet-fetch",
        "Fetches the declared OpenStreetMap planet onto the NAS once, checked against its MD5, for the pass to read.",
        w_none,
    ),
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "pass-sets",
            "Cuts the sets a finished pass lacks in their current filters.",
            w_pass_sets,
        )
    },
    Step {
        alone: true,
        versions: &[("outlines", || super::build::OUTLINES_V.to_string())],
        reads: &[
            In { name: "pass", of: idents::pass },
            In { name: "set:outlines", of: idents::set_outlines },
            In { name: "previous-outlines", of: idents::previous_outlines },
        ],
        ..step(
            "pass",
            Derive,
            World,
            "outlines",
            "Assembles the countries' and regions' outlines from the pass's outline set.",
            w_outlines,
        )
    },
    Step {
        alone: true,
        versions: &[("trailends", || super::build::TRAILENDS_V.to_string())],
        reads: &[In { name: "set:hikes", of: idents::set_hikes }],
        ..step(
            "pass",
            Derive,
            World,
            "trailends",
            "Finds the ends of the world's hiking routes.",
            w_trailends,
        )
    },
    Step {
        alone: true,
        versions: &[("reach", || roads::reach::REACH_V.to_string())],
        reads: &[In { name: "pieces", of: idents::pieces }],
        ..step("pass", Derive, World, "reach", "Works out how far each tile's roads reach.", w_reach)
    },
    Step {
        alone: true,
        ..step(
            "pass",
            Fetch,
            World,
            "buildings",
            "Collects the world's roadside building boxes from the declared buildings release.",
            w_buildings,
        )
    },
    Step {
        alone: true,
        versions: &[("summits", || super::build::SUMMITS_V.to_string())],
        reads: &[In { name: "set:summits", of: idents::set_summits }, All { step: "terrain-lo", grain: Outputs }],
        ..step("pass", Derive, World, "summits", "Finds the world's summits.", w_summits) },
    Step {
        alone: true,
        versions: &[("labels", || super::build::LABELS_V.to_string())],
        reads: &[In { name: "set:labels", of: idents::set_labels }],
        ..step(
            "pass",
            Pack,
            World,
            "labels",
            "Ranks the world's place labels and packs them.",
            w_labels,
        )
    },
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "lakefill",
            "Fills in Canada's lakes missing from OpenStreetMap.",
            w_lakefill,
        )
    },
    Step {
        alone: true,
        versions: &[("water", || inputs::water::VERSION.to_string())],
        reads: &[In { name: "basemap", of: idents::basemap }, In { name: "lakefill", of: idents::lakefill }],
        ..step("pass", Pack, World, "water", "Draws the world's water at every zoom.", w_water)
    },
    Step {
        versions: &[("spoken", || names::spoken::RULES.to_string())],
        reads: &[All { step: "outlines", grain: Grain::Read(idents::pass_outlines) }, In { name: "language-table", of: idents::language_table }],
        ..step(
            "pass",
            Derive,
            World,
            "spoken",
            "Maps the languages spoken where, for names.",
            w_spoken,
        )
    },
    // The regions' extract (inputs::inputs::regions::extract): the pass's planet read three times,
    // a heavy NAS reader.
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "regions-extract",
            "Extracts the regions' OSM relations and ways.",
            w_regions_extract,
        )
    },
    // The coastline's land (inputs::inputs::regions::land): the pass's coastline set assembled and
    // cut by 2° cell, every node and ring held at once.
    Step {
        alone: true,
        versions: &[("regions-land", || inputs::inputs::regions::land::LAND_V.to_string())],
        reads: &[In { name: "pass", of: idents::pass }, In { name: "set:coastline", of: idents::set_coastline }],
        ..step(
            "pass",
            Derive,
            World,
            "regions-land",
            "Assembles the coastline's land from the pass's coastline.",
            w_regions_land,
        )
    },
    Step {
        alone: true,
        answered: true,
        versions: &[("heritage-sites", || super::build::HERITAGE_SITES_V.to_string())],
        reads: &[In { name: "pass", of: idents::pass }, In { name: "set:areas", of: idents::set_areas }, In { name: "registers", of: idents::registers }, In { name: "matches", of: idents::matches }, In { name: "coverage:all", of: idents::coverage_all }],
        ..step(
            "sites",
            Derive,
            World,
            "heritage-sites",
            "Finds the regions' heritage sites and designated areas.",
            w_heritage_sites,
        )
    },
    // Terrain's z6 tiles (a z6 tile each, eight a job; a z3 tile is its whole run, the scheme
    // before z6 tiles).
    Step {
        versions: &[("terrain-water", || super::build::TERRAIN_WATER_V.to_string())],
        reads: &[In { name: "water-pin", of: idents::water_pin }, In { name: "water-wants", of: idents::water_wants }],
        ..step(
            "terrain",
            Derive,
            World,
            "terrain-water",
            "Reads the water the depth lies on, for the depth's keys.",
            w_terrain_water,
        )
    },
    // (The lead's: its archives go under `sources/pyramids/`, outside the records, handed off for
    // the lead to name in their stores' indexes, as AWS's raw tiles' are: terrain::pyramids. A z6
    // tile's fetch at most 4,096 tiles in memory at once, its wanted set a few MB.)
    Step {
        versions: &[("pyramids-fill", || terrain::pyramids::V.to_string())],
        reads: &[In { name: "coverage:piece", of: idents::coverage_piece }, In { name: "pyramids", of: idents::pyramids }, In { name: "deep-targets", of: idents::deep_targets }],
        ..step(
            "terrain",
            Fetch,
            Z6,
            "pyramids-fill",
            "Fetches a tile's elevation tiles from the pyramids read tile by tile (GSI's) once, onto the NAS, for the terrain to read there.",
            w_none,
        )
    },
    // (Its memory offered by target, the agent's terrain_peak: a z6 tile's levels' bound,
    // terrain::terrain_pack::piece_peak_mb, and its z10 tiles' deep mids taken in,
    // terrain::terrain_pack::DEEP_MIDS_MB, and the z6 tile before's uploads in flight,
    // buildstore::out::UPLOAD_HELD; with the coverage its process reads,
    // buildstore::coverage::Coverage::read_peak_mb. Until a z6 tile's own run says. It waits for its z10
    // tiles' terrain-deep targets alone: their mids are its z12 tiles' quarters there.)
    Step {
        cost_v: 8,
        shared: true,
        versions: &[("terrain-hi", || terrain::terrain_pack::TERRAIN_V.to_string()), ("terrain-codec", || codecs::terrain_codec::version())],
        reads: &[In { name: "coverage:piece", of: idents::coverage_piece }, In { name: "datasets:terrain", of: idents::datasets_terrain }, In { name: "zoom-lines", of: idents::zoom_lines }, In { name: "makes-hi", of: idents::makes_hi }, Of { step: "terrain-deep", of: sh_deep, grain: Grain::Read(idents::terrain_deep_mids) }, Of { step: "pyramids-fill", of: own, grain: Grain::Read(idents::nothing) }],
        ..step(
            "terrain",
            Pack,
            Z6,
            "terrain-hi",
            "Builds a tile's terrain hi pack (z9–12).",
            w_terrain_hi,
        )
    },
    Step {
        cost_v: 1,
        shared: true,
        versions: &[("terrain-deep", || super::build::TERRAIN_DEEP_V.to_string()), ("terrain-hi", || terrain::terrain_pack::TERRAIN_V.to_string()), ("terrain-codec", || codecs::terrain_codec::version())],
        reads: &[In { name: "coverage:piece", of: idents::coverage_piece }, In { name: "datasets:terrain", of: idents::datasets_terrain }, In { name: "zoom-lines", of: idents::zoom_lines }, Of { step: "pyramids-fill", of: z6_of_z10, grain: Grain::Read(idents::nothing) }],
        ..step(
            "terrain",
            Pack,
            Z10,
            "terrain-deep",
            "Builds a z10 tile's terrain past z12 (z13–16) where a declared elevation dataset has the detail, and its mid for its z6 tile.",
            w_terrain_deep,
        )
    },
    Step {
        cost_v: 3,
        versions: &[("terrain-lo", || super::build::TERRAIN_LO_V.to_string()), ("terrain-hi", || terrain::terrain_pack::TERRAIN_V.to_string()), ("terrain-codec", || codecs::terrain_codec::version())],
        reads: &[Mids { step: "terrain-hi", of: z6_of_z3, named: idents::terrain_mids }, In { name: "datasets:terrain", of: idents::datasets_terrain }],
        ..step(
            "terrain",
            Pack,
            Z3,
            "terrain-lo",
            "Assembles a z3 tile's zoomed-out terrain from its tiles.",
            w_terrain_lo,
        )
    },
    // (Built once all terrain-lo is done, in the terrain stage: slope reads it, waiting for it,
    // docs/readiness.md decision 2.)
    Step {
        cost_v: 1,
        versions: &[("terrain-root", || 2.to_string()), ("terrain-codec", || codecs::terrain_codec::version())],
        reads: &[All { step: "terrain-lo", grain: Outputs }],
        ..step(
            "terrain",
            Pack,
            World,
            "terrain-root",
            "Builds the world-level terrain from the z3 tiles' zoomed-out packs.",
            w_terrain_root,
        )
    },
    Step {
        cost_v: 1,
        shared: true,
        versions: &[("slope-deep", || terrain::slope_pack::SLOPE_V.to_string()), ("shade-codec", || codecs::shade_codec::version())],
        reads: &[Of { step: "terrain-deep", of: sd_deep, grain: Grain::Tiles(idents::slope_tiles) }],
        ..step(
            "slope",
            Pack,
            Z10,
            "slope-deep",
            "Works out a z10 tile's shading at its deep levels (z13–16) from its deep terrain, its z13 tiles handed on to its tile's.",
            w_slope_deep,
        )
    },
    Step {
        cost_v: 5,
        shared: true,
        versions: &[("slope-hi", || terrain::slope_pack::SLOPE_V.to_string()), ("shade-codec", || codecs::shade_codec::version())],
        reads: &[Of { step: "terrain-hi", of: own, grain: Outputs }, Of { step: "terrain-hi", of: sh_hi, grain: Grain::Tiles(idents::slope_tiles) }, Of { step: "terrain-lo", of: sh_lo, grain: Grain::Tiles(idents::nothing) }, Of { step: "terrain-deep", of: sh_deep, grain: Grain::Tiles(idents::nothing) }, Of { step: "slope-deep", of: sh_deep, grain: Grain::Read(idents::deep_mids) }, In { name: "root-tiles", of: idents::root_tiles }],
        ..step(
            "slope",
            Pack,
            Z6,
            "slope-hi",
            "Works out a tile's shading from its terrain (z6–12), its z10 tiles' z13 quadrants taken in.",
            w_slope_hi,
        )
    },
    Step {
        cost_v: 1,
        versions: &[("slope-lo", || terrain::slope_pack::SLOPE_LO_V.to_string()), ("slope", || terrain::slope_pack::SLOPE_V.to_string()), ("shade-codec", || codecs::shade_codec::version())],
        reads: &[Of { step: "slope-hi", of: z6_of_z3, grain: Grain::Read(idents::slope_mids) }, Of { step: "terrain-lo", of: sl_lo, grain: Outputs }, All { step: "terrain-root", grain: Grain::Read(idents::terrain_root) }],
        ..step(
            "slope",
            Pack,
            Z3,
            "slope-lo",
            "Assembles a z3 tile's zoomed-out slope from its tiles.",
            w_slope_lo,
        )
    },
    // The polar caps past the Web Mercator tiles' 85.05° (terrain::polar; docs/polar.md): a block's
    // terrain, then its shading, then a pole's root of both.
    Step {
        cost_v: 1,
        versions: &[("polar-terrain", || terrain::polar::TERRAIN_V.to_string()), ("terrain-codec", || codecs::terrain_codec::version())],
        reads: &[In { name: "datasets:polar", of: idents::datasets_polar }],
        ..step(
            "terrain",
            Pack,
            Polar,
            "polar-terrain",
            "Samples a polar block's terrain past 85° from the elevation pool (z4–9).",
            w_polar_terrain,
        )
    },
    Step {
        versions: &[("polar-slope", || terrain::polar::SLOPE_V.to_string()), ("shade-codec", || codecs::shade_codec::version())],
        reads: &[Of { step: "polar-terrain", of: ps_terrain, grain: Grain::Read(idents::polar_neighbours) }],
        ..step(
            "slope",
            Pack,
            Polar,
            "polar-slope",
            "Works out a polar block's shading from its terrain and its neighbours' (z4–9).",
            w_polar_slope,
        )
    },
    Step {
        cost_v: 1,
        versions: &[("polar-root", || terrain::polar::ROOT_V.to_string()), ("terrain-codec", || codecs::terrain_codec::version()), ("shade-codec", || codecs::shade_codec::version())],
        reads: &[In { name: "datasets:polar", of: idents::datasets_polar }, In { name: "polar-blocks", of: idents::polar_blocks }, Of { step: "polar-terrain", of: pole_blocks, grain: Grain::Read(idents::polar_terrain_packs) }, Of { step: "polar-slope", of: pole_blocks, grain: Grain::Read(idents::polar_slope_packs) }],
        ..step(
            "slope",
            Pack,
            World,
            "polar-root",
            "Builds a pole's zoomed-out terrain and shading (z0–3) from its blocks.",
            w_polar_root,
        )
    },
    // (Cost version 3: the canopy rows read where they lie and counted, its blocks paced, task #606:
    // 2's copied whole squares uncounted, so its NAS weight was its uploads alone.)
    Step {
        cost_v: 3,
        shared: true,
        versions: &[("trees", || trees::treepacks::TREES_V.to_string())],
        reads: &[In { name: "coverage:inside", of: idents::coverage_inside }, In { name: "trees-sources", of: idents::trees_sources }],
        ..step("trees", Pack, Z6, "trees", "Maps a tile's tree cover, height and leaf type.", w_trees)
    },
    Step {
        cost_v: 1,
        versions: &[("trees-lo", || trees::treepacks::TREES_LO_V.to_string()), ("trees", || trees::treepacks::TREES_V.to_string())],
        reads: &[Mids { step: "trees", of: z6_of_z3, named: idents::trees_mids }],
        ..step(
            "trees",
            Pack,
            Z3,
            "trees-lo",
            "Assembles a z3 tile's zoomed-out tree cover from its tiles.",
            w_trees_lo,
        )
    },
    Step {
        shared: true,
        versions: &[("lake-depth", || terrain::depth::lakes::V.to_string())],
        reads: &[
            In { name: "lake-sources", of: idents::lake_sources },
            In { name: "water-source", of: idents::water_source },
            Of { step: "terrain-hi", of: ld_hi, grain: Outputs },
        ],
        ..step(
            "depth",
            Derive,
            Z6,
            "lake-depth",
            "Estimates the depth of a tile's lakes no survey covers, from GLOBathy's maximum depths and, for the small ones, the land's slope around them.",
            w_lake_depth,
        )
    },
    // (Cost version 2: composed only at a grid's own zoom, the coarser levels the means of the finer,
    // and the floor's shading's assembly made in depth-lo, task #804; 1: a grid lined up with no
    // overview sampled from one askew, task #433.)
    Step {
        cost_v: 2,
        shared: true,
        versions: &[("depth", || super::build::depth_versions("depth"))],
        reads: &[
            In { name: "depth-sources", of: idents::depth_sources },
            In { name: "water:depth", of: idents::water_depth },
            Of { step: "lake-depth", of: d_lakes, grain: Grain::Read(idents::lake_packs) },
            Built { step: "water", of: idents::depth_surface_water, grain: Grain::Tiles(idents::depth_enclosed) },
            Built { step: "terrain-lo", of: idents::depth_surface_lo, grain: Grain::Tiles(idents::depth_surface_terrain) },
        ],
        ..step(
            "depth",
            Pack,
            Z6,
            "depth",
            "Composes a tile's depth and its shading where a source has detail finer than the zoomed-out levels.",
            w_depth,
        )
    },
    Step {
        cost_v: 2,
        versions: &[("depth-lo", || super::build::depth_versions("depth-lo"))],
        reads: &[
            Of { step: "depth", of: z6_of_z3, grain: Outputs },
            In { name: "depth-sources", of: idents::depth_sources },
            In { name: "water:depth", of: idents::water_depth },
            Of { step: "lake-depth", of: d_lakes, grain: Grain::Read(idents::lake_packs) },
            Built { step: "water", of: idents::depth_surface_water, grain: Grain::Tiles(idents::depth_enclosed) },
            Built { step: "terrain-lo", of: idents::depth_surface_lo, grain: Grain::Tiles(idents::depth_surface_terrain) },
        ],
        ..step(
            "depth",
            Pack,
            Z3,
            "depth-lo",
            "Assembles a z3 tile's zoomed-out depth and its shading from its tiles' finer ones, composed from the depth sources where there are none.",
            w_depth_lo,
        )
    },
    // (The root: 64 z3 tiles of each layer read, 21 made.)
    Step {
        versions: &[("depth-root", || terrain::depth::build::DEPTH_ROOT_V.to_string()), ("depth", || super::build::depth_versions("depth-root"))],
        reads: &[All { step: "depth-lo", grain: Outputs }],
        ..step(
            "depth",
            Pack,
            World,
            "depth-root",
            "Builds the world-level depth and its shading from the z3 tiles' zoomed-out packs.",
            w_depth_root,
        )
    },
    // (Cost version: a z6 tile's measure its job's processes together, as the memory guard measures a
    // job; 0 was its programs' most, one at a time; 2: its processes started without malloc's large
    // cache, store::spawn::NO_LARGE_CACHE, where 1's measures counted 6–7 GB of freed blocks; 3: its
    // elevations sampled in batches, inputs::dem::generic::BATCH, where 2's sampled a z6 tile's
    // vertices at once: 16 GB for the densest.)
    Step {
        cost_v: 3,
        shared: true,
        versions: &[("unit", || super::build::UNIT_V.to_string())],
        reads: &[In { name: "extract", of: idents::extract }, In { name: "roads", of: idents::roads }, In { name: "ways", of: idents::ways }, In { name: "elevation", of: idents::elevation }, In { name: "unit-sources", of: idents::unit_sources }, Of { step: "terrain-lo", of: u_lo, grain: Grain::Tiles(idents::unit_tiles) }, Of { step: "terrain-hi", of: u_hi, grain: Grain::Tiles(idents::nothing) }, Of { step: "terrain-deep", of: u_deep, grain: Grain::Tiles(idents::unit_deep_tiles) }, OfAlong { step: "terrain-lo", of: ua_lo }, OfAlong { step: "terrain-hi", of: ua_hi }, OfAlong { step: "terrain-deep", of: ua_deep }, All { step: "heritage-sites", grain: Grain::Read(idents::heritage_slices) }, All { step: "reach", grain: Grain::Read(idents::nothing) }, All { step: "buildings", grain: Grain::Read(idents::roadside) }],
        ..step(
            "roads",
            Pack,
            Z6,
            "unit",
            "Builds a tile's roads, their elevations and scenery: its base pack and road values.",
            w_unit,
        )
    },
    Step {
        ..step("tiles", Pack, Z6, "pack", "Draws a tile's road and rail map tiles and its data.", w_pack)
    },
    Step {
        ..step(
            "tiles",
            Pack,
            Z3,
            "lo",
            "Draws a z3 tile's zoomed-out road and rail map tiles.",
            w_lo,
        )
    },
    Step { ..step("finish", Keep, World, "prune", "Removes what the regions no longer cover.", w_prune) },
    Step {
        versions: &[("roadunits", || super::build::ROADUNITS_V.to_string())],
        reads: &[In { name: "road-ids", of: idents::road_ids }],
        ..step(
            "finish",
            Derive,
            World,
            "roadunits",
            "Works out which tiles each road crosses.",
            w_roadunits,
        )
    },
    Step {
        versions: &[("stations", || super::build::STATIONS_V.to_string())],
        reads: &[In { name: "set:rail", of: idents::set_rail }, In { name: "built-tiles", of: idents::built_tiles }],
        ..step("finish", Pack, World, "stations", "Places the rail stops near the regions.", w_stations)
    },
    Step {
        versions: &[("ferries", || super::build::FERRIES_V.to_string())],
        reads: &[In { name: "ferries-reads", of: idents::ferries_reads }],
        ..step("finish", Pack, World, "ferries", "Maps the world's ferries.", w_ferries) },
    Step {
        versions: &[("slope-root", || 1.to_string()), ("shade-codec", || codecs::shade_codec::version())],
        reads: &[All { step: "slope-lo", grain: Outputs }, All { step: "terrain-root", grain: Grain::Read(idents::terrain_root) }],
        ..step(
            "finish",
            Pack,
            World,
            "slope-root",
            "Builds the world-level slope from the z3 tiles' zoomed-out packs.",
            w_slope_root,
        )
    },
    Step {
        // (The coverage read from the regions, the pass's outlines for its countries, then the
        // feeds' checks, in one process: 4.46 GB at its peak on the Mac mini, 23 Oct, task #578;
        // the coverage itself, shared with every step, is most of it.)
        versions: &[("rail-feeds", || super::build::RAIL_FEEDS_V.to_string())],
        reads: &[In { name: "feeds", of: idents::feeds_declared }, In { name: "feed-keys", of: idents::feed_keys }, In { name: "feeds-reads", of: idents::feeds_reads }, In { name: "coverage:all", of: idents::coverage_all }],
        ..step(
            "trains",
            Fetch,
            World,
            "rail-feeds",
            "Fetches the regions' rail and ferry timetables.",
            w_rail_feeds,
        )
    },
    Step {
        versions: &[("rail", || super::build::RAIL_V.to_string())],
        reads: &[All { step: "rail-feeds", grain: Grain::Read(idents::rail_feeds) }, In { name: "rail-reads", of: idents::rail_reads }, In { name: "coverage:all", of: idents::coverage_all }, In { name: "spacing", of: idents::spacing }],
        ..step("trains", Derive, World, "rail", "Counts trains a day on the regions' rail.", w_rail)
    },
    Step {
        shared: true,
        versions: &[("pois", || super::build::POIS_V.to_string())],
        reads: &[In { name: "extract", of: idents::extract }, In { name: "coverage:clip", of: idents::coverage_clip }, All { step: "trailends", grain: Grain::Read(idents::pass_trailends) }, In { name: "spacing", of: idents::spacing }],
        ..step("landmarks", Derive, Z6, "pois", "Finds a tile's landmark candidates.", w_pois)
    },
    Step {
        shared: true,
        versions: &[("peaks", || super::build::PEAKS_V.to_string())],
        reads: &[Of { step: "pois", of: own, grain: Outputs }, All { step: "summits", grain: Grain::Read(idents::pass_summits) }, All { step: "terrain-lo", grain: Outputs }, Of { step: "terrain-hi", of: peaks_hi, grain: Outputs }],
        ..step("landmarks", Derive, Z6, "peaks", "Measures a tile's peaks' prominence and isolation.", w_peaks)
    },
    Step {
        answered: true,
        versions: &[("items", || super::build::ITEMS_V.to_string())],
        reads: &[In { name: "pass", of: idents::pass }, All { step: "pois", grain: Grain::Read(idents::pois_now) }],
        ..step(
            "landmarks",
            Fetch,
            World,
            "items",
            "Fetches the landmarks' Wikidata facts and Wikipedia pageviews.",
            w_items,
        )
    },
    // (Cost version 1: the World Heritage outlines clipped from the pass's `whs` set, not the filtered
    // planet; 0's floor, 21.9 GB, no run with its memory timed came near: 6.4 GB, the clip's 4 GB.)
    Step {
        cost_v: 1,
        answered: true,
        versions: &[("heritage", || super::build::HERITAGE_V.to_string())],
        reads: &[In { name: "pass", of: idents::pass }, All { step: "heritage-sites", grain: Grain::Read(idents::heritage_sites_out) }, In { name: "heritage-sets", of: idents::heritage_sets }, In { name: "coverage:all", of: idents::coverage_all }],
        ..step(
            "landmarks",
            Derive,
            World,
            "heritage",
            "Adds details, fame and outlines to the heritage sites.",
            w_heritage,
        )
    },
    Step {
        versions: &[("marks", || super::build::MARKS_V.to_string())],
        reads: &[All { step: "items", grain: Grain::Read(idents::items_out) }, All { step: "heritage", grain: Grain::Read(idents::heritage_marks) }, All { step: "pois", grain: Grain::Read(idents::units_pois) }, All { step: "peaks", grain: Grain::Read(idents::units_peaks) }],
        ..step(
            "landmarks",
            Pack,
            World,
            "marks",
            "Ranks the landmarks and draws them on the map.",
            w_marks,
        )
    },
    Step {
        versions: &[("overlays", || super::build::OVERLAYS_V.to_string())],
        reads: &[In { name: "heritage-dots", of: idents::heritage_dots }, All { step: "heritage", grain: Grain::Read(idents::heritage_areas) }, In { name: "base-tiles", of: idents::base_tiles }, All { step: "items", grain: Grain::Read(idents::nothing) }, All { step: "pois", grain: Grain::Read(idents::nothing) }, All { step: "peaks", grain: Grain::Read(idents::nothing) }],
        ..step("landmarks", Pack, World, "overlays", "Draws the area overlays.", w_overlays)
    },
    // (Cost version 1: the coverage asked by distance, never grown, task #493: 0's floors, 43 GB from
    // buffering the regions' outlines, no run since came near: 0.2 GB.)
    Step {
        cost_v: 1,
        versions: &[("bld-fetch", || super::build::BLD_FETCH_V.to_string())],
        reads: &[In { name: "bld-key", of: idents::bld_key }, In { name: "coverage:all", of: idents::coverage_all }],
        ..step(
            "buildings",
            Fetch,
            World,
            "bld-fetch",
            "Fetches the 3D buildings' sources.",
            w_none,
        )
    },
    // (The 3D buildings: offered by the rows they read, agentcore::needs::OFFERED_BY.)
    Step {
        shared: true,
        versions: &[("bldprep", || bld::bld::BLDPREP_V.to_string())],
        reads: &[In { name: "bld-key", of: idents::bld_key }, In { name: "bld-rows", of: idents::bld_rows }],
        ..step(
            "buildings",
            Derive,
            Z6,
            "bldprep",
            "Reads a tile's buildings from the sources into normalized files.",
            w_bldprep,
        )
    },
    Step {
        shared: true,
        versions: &[("bldtiles", || bld::bld::BUILDINGS_V.to_string())],
        reads: &[Of { step: "bldprep", of: bt_prep, grain: Grain::Read(idents::bld_prepared) }, In { name: "bld-sources", of: idents::bld_sources }, In { name: "coverage:shapes", of: idents::coverage_shapes }, In { name: "bld-heritage", of: idents::bld_heritage }],
        ..step(
            "buildings",
            Pack,
            Z6,
            "bldtiles",
            "Raises a tile's 3D buildings and its far buildings' field.",
            w_bldtiles,
        )
    },
    Step {
        versions: &[("bldfield-lo", || bld::bld::field::LO_V.to_string())],
        reads: &[Of { step: "bldtiles", of: z6_of_z3, grain: Grain::Read(idents::bld_mids) }, In { name: "bld-sources", of: idents::bld_sources }],
        ..step(
            "buildings",
            Pack,
            Z3,
            "bldfield-lo",
            "Assembles a z3 tile's zoomed-out buildings' field from its tiles.",
            w_bldfield_lo,
        )
    },
    Step {
        ..step(
            "publish",
            Publish,
            World,
            store::catalogue::NAME,
            "Makes the catalogue the map serves: the new data goes out.",
            w_none,
        )
    },
    Step {
        ..step(
            "publish",
            Publish,
            World,
            store::catalogue::HELD_NAME,
            "Makes a catalogue held for review (inputs/hold-catalog).",
            w_none,
        )
    },
    // The gate's checks (inputs::inputs::gate): one per gate unit, ordered before the rest; reading
    // the inputs folders and the accepted copies on the NAS.
    Step {
        ..step(
            "upkeep",
            Keep,
            Input,
            "inputs",
            "Checks an input's files and takes them in.",
            w_inputs,
        )
    },
    Step {
        ..step(
            "upkeep",
            Keep,
            World,
            "names-todo",
            "Lists the names to translate and the descriptions to write.",
            w_none,
        )
    },
    Step {
        versions: &[("timetables-todo", || roads::timetables::VERSION.to_string())],
        reads: &[In { name: "transit-lines", of: idents::transit_lines }, In { name: "coverage:all", of: idents::coverage_all }],
        ..step(
            "upkeep",
            Keep,
            World,
            "timetables-todo",
            "Lists the lines with no timetable.",
            w_timetables_todo,
        )
    },
    // (Way 2: the pending uploads checked by their sizes, `wc -c` on the NAS, task #1001. Way 1's
    // runs hashed them, 813 GB in 4,844 s: weighed the whole NAS by them, the clean-up waited for a
    // quiet NAS that never came. Way 1 had put away the daily job's runs the same way, task #1000.)
    Step {
        alone: true,
        cost_v: 2,
        ..step("upkeep", Keep, World, "gc", "Removes replaced files from the NAS.", w_gc)
    },
    Step {
        alone: true,
        records_only: true,
        ..step(
            "upkeep",
            Keep,
            World,
            "redo",
            "Forgets the records of targets the owner named (`scenic redo`), so they're built again.",
            w_none,
        )
    },
    Step {
        ..step(
            "upkeep",
            Keep,
            World,
            "backup",
            "Backs up translations, descriptions, inputs and the acceptances.",
            w_none,
        )
    },
];

/// A task a running job offers to any worker (docs/coordinator.md §3), described as a step is.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Task {
    /// Its kind: the stored name.
    pub name: &'static str,
    pub does: &'static str,
    /// The step whose jobs offer it.
    pub step: &'static str,
    /// The free disk a member's job of it needs (bytes; crate::agent's `helper_need`).
    pub disk: u64,
}

pub const TASKS: [Task; 4] = [
    // (A z6 tile's last steps, its files fetched from the coordinator: 5 GB.)
    Task {
        name: "tail",
        does: "A tile's last steps (its scenic metrics), offered as the lead builds its roads.",
        step: "unit",
        disk: 5 << 30,
    },
    // (A file of tens of MB, and their tiles: 1 GB.)
    Task {
        name: terrain::terrain_task::KIND,
        does: "A group of a terrain tile's z8 subtrees.",
        step: "terrain-hi",
        disk: 1 << 30,
    },
    // (It reads the squares where they lie, and writes ~80 MB: 1 GB.)
    Task { name: trees::trees::task::KIND, does: "A row of a tree cover tile's blocks.", step: "trees", disk: 1 << 30 },
    // (A 3D buildings' z8 tile, its blocks: 5 GB.)
    Task { name: bld::bld::task::KIND, does: "A z8 tile of a 3D buildings tile.", step: "bldtiles", disk: 5 << 30 },
];

/// The row of `step` (by its name now: `current`); None for one the table doesn't know (a task's
/// kind, or a step of a newer app's).
pub fn row(step: &str) -> Option<&'static Step> {
    let step = current(step);
    TABLE.iter().find(|s| s.name == step)
}

/// The task of kind `kind`.
pub fn task(kind: &str) -> Option<&'static Task> {
    TASKS.iter().find(|t| t.name == kind)
}

/// The stage `id` names.
pub fn stage(id: &str) -> Option<&'static Stage> {
    STAGES.iter().find(|s| s.id == id)
}

/// The stage of `step` (a task's its step's).
pub fn stage_of(step: &str) -> Option<&'static Stage> {
    let id = match row(step) {
        Some(s) => s.stage,
        None => task(step).and_then(|t| row(t.step)).map(|s| s.stage)?,
    };
    stage(id)
}

/// What one target of `step` is called ("z6 tile", "z3 tile", "run", "input"; a task's "task"; an
/// unknown step's "target").
pub fn noun(step: &str) -> &'static str {
    match row(step) {
        Some(s) => s.scope.noun(),
        None if task(step).is_some() => Scope::Task.noun(),
        None => "target",
    }
}

/// `n` targets of `step`, counted in its noun: "8 z6 tiles", "1 z3 tile".
pub fn counted(step: &str, n: usize) -> String {
    format!("{n} {}{}", noun(step), if n == 1 { "" } else { "s" })
}

/// A job of `step` over `n` targets, said by its step's name: "terrain-hi: 8 z6 tiles", "outlines"
/// (a worldwide step's one run).
pub fn said(step: &str, n: usize) -> String {
    let t = current(step);
    if row(step).is_some_and(|s| s.scope == World) || n == 0 {
        return t.to_string();
    }
    format!("{t}: {}", counted(step, n))
}

/// A step's row as the build page and `scenic status` read it (`/api/steps`): everything but the
/// write-set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    pub id: String,
    pub does: String,
    pub kind: Kind,
    pub scope: Scope,
    pub noun: String,
    pub stage: String,
    pub place: Place,
    /// A task's: the step whose jobs offer it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
}

/// Every step's row, in the table's order, then every task's (`of` its step).
pub fn table() -> Vec<Info> {
    let steps = TABLE.iter().map(|s| Info {
        id: s.name.into(),
        does: s.does.into(),
        kind: s.kind,
        scope: s.scope,
        noun: s.scope.noun().into(),
        stage: s.stage.into(),
        place: every::place(super::theplan::the_plan(), s.name),
        of: None,
    });
    let tasks = TASKS.iter().map(|t| {
        let s = row(t.step).expect("a task's step has a row");
        Info {
            id: t.name.into(),
            does: t.does.into(),
            kind: s.kind,
            scope: Scope::Task,
            noun: Scope::Task.noun().into(),
            stage: s.stage.into(),
            place: Place::Any,
            of: Some(t.step.into()),
        }
    });
    steps.chain(tasks).collect()
}

/// Whether `step`'s jobs only change build records (`Step::records_only`).
pub fn records_only(step: &str) -> bool {
    row(step).is_some_and(|s| s.records_only)
}

/// Whether `step` is a run of the whole build's, the lead's alone.
pub fn alone(step: &str) -> bool {
    row(step).is_some_and(|s| s.alone)
}

/// Which of a row's sets a step set is made of.
#[derive(Clone, Copy)]
enum Set {
    Shared,
    Answered,
}

/// Whether row `s` is in `set`.
const fn in_set(s: &Step, set: Set) -> bool {
    match set {
        Set::Shared => s.shared,
        Set::Answered => s.answered,
    }
}

/// The steps of `set`, in the table's order: N of them, or the build fails.
const fn set_of<const N: usize>(set: Set) -> [&'static str; N] {
    let mut out = [""; N];
    let mut n = 0;
    let mut i = 0;
    while i < TABLE.len() {
        if in_set(&TABLE[i], set) {
            assert!(n < N, "the table has more steps in a set than its size");
            out[n] = TABLE[i].name;
            n += 1;
        }
        i += 1;
    }
    assert!(n == N, "the table has fewer steps in a set than its size");
    out
}

/// Whether some step reads several of `step`'s targets for one of its own (task #909): a slope
/// tile its terrain and its edge neighbours', an assembly its z6 tiles. Probed from the declared
/// reads (`Step::reads`) at a tile of each zoom, never listed by hand. Such a step's targets are
/// planned in compact blocks (`block_order`), so what reads them is ready a neighbourhood at a time
/// rather than once a row's done.
pub fn read_together(step: &str) -> bool {
    const PROBES: [&str; 4] = ["3/4/2", "6/32/20", "10/512/300", "12/2048/1200"];
    TABLE.iter().flat_map(|r| r.reads).any(|x| match *x {
        Read::Targets { step: s, of, .. } | Read::Mids { step: s, of, .. } if s == step => {
            PROBES.iter().any(|t| of(t).len() > 1)
        }
        _ => false,
    })
}

/// Where tile target `t` goes in a compact block order (task #909): its zoom, then its (x, y)
/// interleaved bit by bit (a Z-order curve), so any run of targets taken in order covers squares
/// of tiles, not rows. None for a target that isn't a tile.
pub fn block_order(t: &str) -> Option<(u8, u64)> {
    let u = TileId::parse(t)?;
    let spread = |v: u32| {
        let mut v = v as u64 & 0xffff_ffff;
        v = (v | (v << 16)) & 0x0000_ffff_0000_ffff;
        v = (v | (v << 8)) & 0x00ff_00ff_00ff_00ff;
        v = (v | (v << 4)) & 0x0f0f_0f0f_0f0f_0f0f;
        v = (v | (v << 2)) & 0x3333_3333_3333_3333;
        (v | (v << 1)) & 0x5555_5555_5555_5555
    };
    Some((u.z, spread(u.x) | (spread(u.y) << 1)))
}

/// Whether a target of `reader` reads, directly or through other steps, targets of `step`
/// (`Step::reads`; task #909). Derived from the table alone.
pub fn reads_from(reader: &str, step: &str) -> bool {
    fn go(r: &str, step: &str, seen: &mut Vec<&'static str>) -> bool {
        let Some(row) = row(r) else { return false };
        if seen.contains(&row.name) {
            return false;
        }
        seen.push(row.name);
        row.reads.iter().any(|x| match *x {
            Read::Targets { step: s, .. }
            | Read::Mids { step: s, .. }
            | Read::Whole { step: s, .. }
            | Read::Along { step: s, .. }
            | Read::Built { step: s, .. } => s == step || go(s, step, seen),
            Read::Input { .. } => false,
        })
    }
    reader != step && go(reader, step, &mut Vec::new())
}

/// `v` in the order ready work is granted (docs/pool.md §7.2, task #909): as given (the plan's
/// order), but each item ahead of the first before it of a step it reads (`reads_from`):
/// finishing what's started. Nothing upstream needs a rank to go first: what reads it isn't ready
/// until it's built (`readiness`), so a hand-set order could only starve what's downstream (a
/// ready slope-deep job waited while any terrain-deep job fitted). Steps the reads don't relate
/// keep the plan's order.
pub fn readers_first<T>(v: Vec<T>, step: impl Fn(&T) -> &str) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(v.len());
    for x in v {
        let at = out.iter().position(|y| reads_from(step(&x), step(y))).unwrap_or(out.len());
        out.insert(at, x);
    }
    out
}

/// The shared steps: other members take their jobs (agentcore::coord leases them), in the table's
/// order (no preference: which ready job goes first is derived from the reads, `readers_first`).
pub const SHARED: [&str; 12] = set_of(Set::Shared);
/// The steps that keep the pass's Wikidata and Wikipedia answers.
pub const ANSWERED: [&str; 3] = set_of(Set::Answered);

/// Whether a job of `step` may write `logical` (`removal`: remove it): its outputs function, the
/// one a job's `Out` and the lead's merge both use. None for a step the table doesn't know.
pub fn writes(step: &str, logical: &str, removal: bool) -> Option<bool> {
    row(step).map(|s| (s.writes)(logical, removal))
}

/// Whether any step of the table writes `logical`: a name no step makes (a file an older pipeline
/// left in the manifest) is no output of today's build.
pub fn made(logical: &str) -> bool {
    TABLE.iter().any(|s| (s.writes)(logical, false))
}

/// Whether journal entry `e`'s changes are all within its step's write-set; why not when they
/// aren't (the first outside it, and how many), or when the table has no row for its step.
pub fn outside(e: &agentcore::pool::journal::Entry) -> Option<String> {
    outside_by(e, &|l, removal| writes(&e.step, l, removal))
}

/// `outside`, by outputs function `writes` (None: the step isn't known): the simulator's steps have
/// a function of their own (agentcore::pool::sim).
pub fn outside_by(e: &agentcore::pool::journal::Entry, writes: &dyn Fn(&str, bool) -> Option<bool>) -> Option<String> {
    let out: Vec<&String> = match e
        .handoff
        .changes
        .iter()
        .map(|(l, v)| writes(l, v.is_none()).map(|ok| (l, ok)))
        .collect::<Option<Vec<_>>>()
    {
        Some(v) => v.into_iter().filter(|(_, ok)| !ok).map(|(l, _)| l).collect(),
        None => {
            return Some(format!(
                "{} isn't a step the steps table knows ({} change{})",
                e.step,
                e.handoff.changes.len(),
                if e.handoff.changes.len() == 1 { "" } else { "s" }
            ))
        }
    };
    let first = out.first()?;
    let what = if e.handoff.changes[*first].is_none() { "removes" } else { "writes" };
    Some(format!(
        "{what} {first}, outside {}'s write-set{}",
        e.step,
        if out.len() > 1 { format!(" (and {} more)", out.len() - 1) } else { String::new() }
    ))
}

/// Whether `l` is a file a job of `step` saves for `target` (what a shared step's hand-off may
/// change for each target it did): a z6 tile's base pack, road values, English and the grids its
/// packs lacked; candidates' and peaks' own files; a tree cover z6 tile's (a z6 tile's) hi packs of
/// the tree layers and its mid, an assembly's (a z3 tile's) lo packs of them; terrain's and slope's
/// z6 tiles (a z6 tile's) hi pack of their layer and its mid, an assembly's (a z3 tile's) lo pack; an
/// a z3 tile's lo pack and its z6 tiles' hi packs of terrain, slope, or the tree layers (a
/// z3 tile's whole run: a lease of the scheme before z6 tiles); a z6 tile's normalized buildings
/// (`bldprep`) or 3D buildings' hi pack, its field's hi pack and mid (`bldtiles`); a z3 tile's field
/// lo pack (`bldfield-lo`). False for the other steps.
pub fn saves(step: &str, target: &str, l: &str) -> bool {
    let dash = target.replace('/', "-");
    let tile = TileId::parse(target);
    let step = current(step);
    match step {
        "unit" => roads::unit::saved_files(&dash).iter().any(|f| f == l),
        "pois" | "peaks" => l == format!("work/{step}/{dash}"),
        // (A z6 tile's normalized buildings, and its 3D buildings' hi pack.)
        "bldprep" => tile.is_some_and(|t| t.z == 6 && l == bld::bld::work_logical(t.x, t.y)),
        "bldtiles" => tile.is_some_and(|t| {
            t.z == 6
                && (l == bld::bld::pack_logical(t.x, t.y)
                    || l == bld::bld::field::hi_logical(t.x, t.y)
                    || l == bld::bld::field::mid_logical(t.x, t.y))
        }),
        "bldfield-lo" => tile.is_some_and(|t| t.z == 3 && l == bld::bld::field::lo_logical(t.x, t.y)),
        // (A z6 tile's depth hi packs, a z3 tile's lo packs.)
        "depth" => tile.is_some_and(|t| {
            t.z == 6 && DEPTH_LAYERS.iter().any(|ly| l == terrain::depth::build::hi_logical(ly, t.x, t.y))
        }),
        "depth-lo" => tile.is_some_and(|t| {
            t.z == 3 && DEPTH_LAYERS.iter().any(|ly| l == terrain::depth::build::lo_logical(ly, t.x, t.y))
        }),
        "trees" if tile.is_some_and(|u| u.z == 6) => tile.is_some_and(|t| {
            l == trees::treepacks::mid_logical(t.x, t.y)
                || trees::treepacks::LAYERS.iter().any(|layer| l == format!("layers/{layer}/hi/{dash}"))
        }),
        "trees-lo" => {
            tile.is_some_and(|u| u.z == 3)
                && trees::treepacks::LAYERS.iter().any(|layer| l == format!("layers/{layer}/lo/{dash}"))
        }
        "terrain-hi" if tile.is_some_and(|u| u.z == 6) => tile.is_some_and(|t| {
            l == terrain::terrain_pack::mid_logical(t.x, t.y) || l == format!("layers/terrain/hi/{dash}")
        }),
        "slope-hi" if tile.is_some_and(|u| u.z == 6) => tile
            .is_some_and(|t| l == terrain::slope_pack::mid_logical(t.x, t.y) || l == format!("layers/slope/hi/{dash}")),
        // (Its sources' packs.)
        "lake-depth" => {
            tile.is_some_and(|t| t.z == 6 && terrain::depth::lakes::tile_of(l).is_some_and(|f| f.1 == (t.x, t.y)))
        }
        // (A z10 tile's deep pack, a retired one too; slope's and its deep mid.)
        "terrain-deep" => tile.is_some_and(|t| {
            t.z == terrain::terrain_deep::PACK_Z
                && (terrain::terrain_deep::pack_tile(l) == Some((t.x, t.y))
                    || terrain::terrain_deep::mid_tile(l) == Some((t.x, t.y)))
        }),
        "slope-deep" => tile.is_some_and(|t| {
            t.z == terrain::terrain_deep::PACK_Z
                && (terrain::slope_pack::deep_tile(l) == Some((t.x, t.y))
                    || terrain::slope_pack::deep_mid_tile(l) == Some((t.x, t.y)))
        }),
        "terrain-lo" | "slope-lo" => {
            tile.is_some_and(|u| u.z == 3) && l == format!("layers/{}/lo/{dash}", step.trim_end_matches("-lo"))
        }
        "terrain-hi" | "slope-hi" | "trees" => {
            let Some(q) = tile.filter(|u| u.z == 3) else { return false };
            let layers: &[&str] = match step {
                "terrain-hi" => &["terrain"],
                "slope-hi" => &["slope"],
                _ => &trees::treepacks::LAYERS,
            };
            layers.iter().any(|layer| {
                l == format!("layers/{layer}/lo/{dash}")
                    || l.strip_prefix(&format!("layers/{layer}/hi/6-"))
                        .and_then(|r| r.split_once('-'))
                        .and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?)))
                        .is_some_and(|(x, y)| (x >> 3, y >> 3) == (q.x, q.y))
            })
        }
        _ => false,
    }
}

/// Whether manifest `m` names a published file (under `layers/` or `base/`) a job of `step` saves
/// for tile `target` (`saves`' files of it, looked up by name): false for a target recorded built
/// that made nothing to publish (a deep tile with no fine data there: terrain::terrain_deep,
/// `PRESENCE`; task #839). None for a step whose files aren't told by name here (a worldwide or
/// root step's, a work step's, a z3 tile's whole run of the scheme before z6 tiles).
pub fn output_named(step: &str, target: &str, m: &std::collections::BTreeMap<String, String>) -> Option<bool> {
    Some(output_names(step, target)?.iter().any(|l| m.contains_key(l)))
}

/// The published files' names (under `layers/` or `base/`) a job of `step` saves for tile `target`,
/// any of which names its output (`output_named`; the build page's map asks the latest catalogue,
/// agentcore::agent::builtview, task #866). None for a step whose files aren't told by name here.
pub fn output_names(step: &str, target: &str) -> Option<Vec<String>> {
    let t = TileId::parse(target)?;
    let dash = target.replace('/', "-");
    let layers =
        |ls: &[&str], scope: &str| ls.iter().map(|ly| format!("layers/{ly}/{scope}/{dash}")).collect::<Vec<_>>();
    let step = current(step);
    let names: Vec<String> = match (step, t.z) {
        ("unit", 6) => roads::unit::saved_files(&dash)
            .into_iter()
            .filter(|l| l.starts_with("layers/") || l.starts_with("base/"))
            .collect(),
        ("terrain-deep", z) if z == terrain::terrain_deep::PACK_Z => {
            vec![terrain::terrain_deep::pack_logical(t.x, t.y), format!("{}{dash}", terrain::terrain_deep::RETIRED)]
        }
        ("slope-deep", z) if z == terrain::terrain_deep::PACK_Z => vec![terrain::slope_pack::deep_logical(t.x, t.y)],
        ("terrain-hi", 6) => layers(&["terrain"], "hi"),
        ("slope-hi", 6) => layers(&["slope"], "hi"),
        ("trees", 6) => layers(&trees::treepacks::LAYERS, "hi"),
        ("terrain-lo", 3) => layers(&["terrain"], "lo"),
        ("slope-lo", 3) => layers(&["slope"], "lo"),
        ("trees-lo", 3) => layers(&trees::treepacks::LAYERS, "lo"),
        ("depth", 6) => DEPTH_LAYERS.iter().map(|ly| terrain::depth::build::hi_logical(ly, t.x, t.y)).collect(),
        ("depth-lo", 3) => DEPTH_LAYERS.iter().map(|ly| terrain::depth::build::lo_logical(ly, t.x, t.y)).collect(),
        ("bldtiles", 6) => vec![bld::bld::pack_logical(t.x, t.y), bld::bld::field::hi_logical(t.x, t.y)],
        ("bldfield-lo", 3) => vec![bld::bld::field::lo_logical(t.x, t.y)],
        _ => return None,
    };
    Some(names)
}

// The write-sets. A name's parts, split at '/'; a tile in a name is `z-x-y` (`tile`), a pass by its
// planet's date (`date`).

/// Whether `s` is tile `z-x-y` of zoom `z` (x and y within it, no leading zeros but "0").
fn tile(s: &str, z: u32) -> bool {
    let mut p = s.split('-');
    let num = |t: Option<&str>| {
        t.filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && (t.len() == 1 || !t.starts_with('0')))
            .and_then(|t| t.parse::<u32>().ok())
    };
    match (num(p.next()), num(p.next()), num(p.next()), p.next()) {
        (Some(tz), Some(x), Some(y), None) => tz == z && (x as u64) < (1u64 << z) && (y as u64) < (1u64 << z),
        _ => false,
    }
}

/// Whether `s` is a day, `YYYY-MM-DD`.
fn date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Whether `l` is a pack of layer `layer` (or of a layer `layer` accepts) at one of `scopes` ("root",
/// "lo", "hi"): `layers/<layer>/root/0-0-0`, `…/lo/3-x-y`, `…/hi/6-x-y`.
fn pack_of(l: &str, layer: &dyn Fn(&str) -> bool, scopes: &[&str]) -> bool {
    let p: Vec<&str> = l.split('/').collect();
    match p.as_slice() {
        ["layers", ly, scope, t] if layer(ly) && scopes.contains(scope) => match *scope {
            "root" => *t == "0-0-0",
            "lo" => tile(t, 3),
            "hi" => tile(t, 6),
            _ => false,
        },
        _ => false,
    }
}

/// `pack_of` for one layer by name.
fn pack(l: &str, layer: &str, scopes: &[&str]) -> bool {
    pack_of(l, &|ly| ly == layer, scopes)
}

const ALL: [&str; 3] = ["root", "lo", "hi"];

fn parts(l: &str) -> Vec<&str> {
    l.split('/').collect()
}

fn w_none(_: &str, _: bool) -> bool {
    false
}

/// The clean-up's: it removes the names removed steps made (agentcore::agent::gc::RETIRED) and the
/// depth packs no current target names (agentcore::agent::gc::DepthKeep), and writes nothing.
fn w_gc(l: &str, removal: bool) -> bool {
    removal && (agentcore::agent::gc::retired(l) || w_depth(l, true) || w_depth_lo(l, true))
}

/// The pass's sources (`sources/osm/<date>/…`: its filtered planet, sets, outlines, extracts, road
/// values, the mark that it's complete) and its basemap; as it completes, the older passes' entries
/// removed (inputs::osmpass::retire_older: their sources, items, heritage, summits, route ends).
fn w_osm_pass(l: &str, removal: bool) -> bool {
    match parts(l).as_slice() {
        ["sources", "osm", d, _, ..] => date(d),
        ["layers", "basemap", w] => w.strip_prefix("world-").is_some_and(date),
        ["sources", "items", d, _, ..] | ["work", "heritage", d, _, ..] => removal && date(d),
        ["work", "summits", d] | ["work", "trailends", d] => removal && date(d),
        _ => false,
    }
}

/// The sets a pass lacks in their current filters.
fn w_pass_sets(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "sets", _] if date(d))
}

/// The pass's outlines, written (never removed: the pass retires them).
fn w_outlines(l: &str, removal: bool) -> bool {
    !removal && matches!(parts(l).as_slice(), ["sources", "osm", d, "outlines"] if date(d))
}

fn w_trailends(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "trailends", d] if date(d))
}

fn w_reach(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "reach"] if date(d))
}

/// The world's roadside buildings' index, per Overture release.
fn w_buildings(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "buildings", _, "index"])
}

fn w_summits(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "summits", d] if date(d))
}

/// The labels' packs, every zoom (those it didn't write removed).
fn w_labels(l: &str, _: bool) -> bool {
    pack(l, "labels", &ALL)
}

/// The pass's lakes filled from Canada's own data (the other passes' removed).
fn w_lakefill(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "lakefill", d] if date(d))
}

/// The water's packs, every zoom (those it didn't write removed, and the small water's before it,
/// which it replaced), and the seas closed off from the world ocean (inputs::water::ENCLOSED).
fn w_water(l: &str, removal: bool) -> bool {
    pack(l, "water", &ALL) || l == inputs::water::ENCLOSED || (removal && pack(l, "smallwater", &ALL))
}

/// The heritage sites by z6 tile, their positions and areas (those of the pass it didn't write
/// removed), and the base files the chain reads.
fn w_heritage_sites(l: &str, _: bool) -> bool {
    match parts(l).as_slice() {
        ["work", "heritage", d, "pos" | "areas" | "parts", t] => date(d) && tile(t, 6),
        ["work", "heritage", d, "base", _] => date(d),
        _ => false,
    }
}

/// A gate unit's accepted index, its held report and its check's state (inputs::inputs::record_of:
/// `sources/inputs/<unit>/@index`, `…/@held`, `…/@listed`, the unit's name nested or not), the
/// report removed once nothing's held.
fn w_inputs(l: &str, _: bool) -> bool {
    inputs::inputs::record_of(l).is_some()
}

fn w_regions_land(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "regions-land"] if inputs::osmpass::is_date(d))
}

fn w_regions_extract(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "regions-extract"] if inputs::osmpass::is_date(d))
}

fn w_spoken(l: &str, _: bool) -> bool {
    l == "global/spoken"
}

/// A z6 tile's hi pack and mid (a z6 tile); a z3 tile's whole run's lo pack and its z6 tiles' hi
/// packs.
fn w_terrain_hi(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["lo", "hi"]) || matches!(parts(l).as_slice(), ["work", "terrain-mid", t] if tile(t, 6))
}

/// A z10 tile's deep pack (terrain::terrain_deep), and the retired fine pack there it drops.
fn w_terrain_deep(l: &str, _: bool) -> bool {
    terrain::terrain_deep::pack_tile(l).is_some() || terrain::terrain_deep::mid_tile(l).is_some()
}

fn w_terrain_lo(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["lo"])
}

/// A z6 tile's depth, class and shading hi packs (removed where it makes none).
fn w_depth(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| DEPTH_LAYERS.contains(&ly), &["hi"])
}

/// A z6 tile's lake-depth packs (terrain::depth::lakes), a source's each.
fn w_lake_depth(l: &str, _: bool) -> bool {
    terrain::depth::lakes::tile_of(l).is_some()
}

/// A z3 tile's depth, class and shading lo packs.
fn w_depth_lo(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| DEPTH_LAYERS.contains(&ly), &["lo"])
}

fn w_depth_root(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| DEPTH_LAYERS.contains(&ly), &["root"])
}

/// The depth steps' layers: the depth, its classes and the floor's shading (terrain::depth).
const DEPTH_LAYERS: [&str; 3] = [terrain::depth::LAYER, terrain::depth::CLASS_LAYER, terrain::depth::SHADE_LAYER];

/// The water's index for the depth's repairs, by the content it's made of.
fn w_terrain_water(l: &str, _: bool) -> bool {
    l.strip_prefix(codecs::terrain_water::IDX_PREFIX).is_some_and(|k| !k.is_empty() && !k.contains('/'))
}

fn w_terrain_root(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["root"])
}

/// A z6 tile's hi pack and mid (a z6 tile); a z3 tile's whole run's lo pack and its z6 tiles' hi
/// packs.
fn w_slope_hi(l: &str, _: bool) -> bool {
    pack(l, "slope", &["lo", "hi"]) || matches!(parts(l).as_slice(), ["work", "slope-mid", t] if tile(t, 6))
}

/// A z10 tile's deep pack and deep mid (terrain::slope_pack::build_deep).
fn w_slope_deep(l: &str, _: bool) -> bool {
    terrain::slope_pack::deep_tile(l).is_some() || terrain::slope_pack::deep_mid_tile(l).is_some()
}

fn w_slope_lo(l: &str, _: bool) -> bool {
    pack(l, "slope", &["lo"])
}

fn w_slope_root(l: &str, _: bool) -> bool {
    pack(l, "slope", &["root"])
}

/// A polar layer's (`kind`) block packs (`layers/polar-<kind>-<pole>/polar/4-x-y`) or root.
fn polar_pack(l: &str, kind: &str, scope: &str) -> bool {
    terrain::polar::POLES.iter().any(|&p| match parts(l).as_slice() {
        ["layers", ly, sc, t] => {
            *ly == terrain::polar::layer(kind, p)
                && *sc == scope
                && (if scope == "root" { *t == "0-0-0" } else { tile(t, u32::from(terrain::polar::BLOCK_Z)) })
        }
        _ => false,
    })
}

fn w_polar_terrain(l: &str, _: bool) -> bool {
    polar_pack(l, "terrain", "polar")
}

fn w_polar_slope(l: &str, _: bool) -> bool {
    polar_pack(l, "slope", "polar")
}

fn w_polar_root(l: &str, _: bool) -> bool {
    polar_pack(l, "terrain", "root") || polar_pack(l, "slope", "root")
}

/// The tree layers' hi packs and a z6 tile's mid (a z6 tile); a z3 tile's whole run their lo packs
/// too.
fn w_trees(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| trees::treepacks::LAYERS.contains(&ly), &["lo", "hi"])
        || matches!(parts(l).as_slice(), ["work", "trees-mid", t] if tile(t, 6))
}

fn w_trees_lo(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| trees::treepacks::LAYERS.contains(&ly), &["lo"])
}

/// A z6 tile's files (roads::unit::saved_files), of a z6 tile.
fn w_unit(l: &str, _: bool) -> bool {
    l.rsplit_once('/').is_some_and(|(_, t)| tile(t, 6) && roads::unit::saved_files(t).iter().any(|f| f == l))
}

fn w_pois(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "pois", t] if tile(t, 6))
}

fn w_peaks(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "peaks", t] if tile(t, 6))
}

/// A z6 tile's map tiles: its roads' and rails' hi packs and its data.
fn w_pack(l: &str, _: bool) -> bool {
    pack(l, "roads", &["hi"]) || pack(l, "rails", &["hi"]) || matches!(parts(l).as_slice(), ["hidata", t] if tile(t, 6))
}

fn w_lo(l: &str, _: bool) -> bool {
    pack(l, "roads", &["lo"]) || pack(l, "rails", &["lo"])
}

/// The landmarks' Wikidata facts, pageviews and their meta, for the pass.
fn w_items(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "items", d, "facts" | "views" | "meta"] if date(d))
}

/// The heritage chain's files, at the top of the pass's heritage folder (those it didn't write
/// removed).
fn w_heritage(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "heritage", d, stem] if date(d) && !stem.is_empty())
}

/// The landmarks drawn: each kind's zoomed-out tiles as a bundle, their data by z6 tile as one
/// bundle (task #449; the packs and files per tile from before, removed), the heritage dots and the
/// summary (what it didn't write removed).
fn w_marks(l: &str, _: bool) -> bool {
    let kind = |ly: &str| ly.strip_prefix("marks-").is_some_and(|k| !k.is_empty());
    pack_of(l, &kind, &["root", "lo"])
        || matches!(parts(l).as_slice(), ["layers", ly, "all"] if kind(ly))
        || matches!(parts(l).as_slice(), ["markdata", t] if tile(t, 6))
        || l == codecs::marks::MARKDATA_BUNDLE
        || l == landmarks::markconv::HERITAGE_DOTS
        || l == "global/marks/summary"
}

/// The area overlays' tiles, every zoom, a bundle per layer (task #449; the packs from before,
/// removed), their data by z3 tile, and the heritage summaries (what it didn't write removed).
fn w_overlays(l: &str, _: bool) -> bool {
    let layer = |ly: &str| ["ov-heritage-areas", "ov-indigenous", "ov-special", "ov-whs"].contains(&ly);
    pack_of(l, &layer, &ALL)
        || matches!(parts(l).as_slice(), ["layers", ly, "all"] if layer(ly))
        || matches!(parts(l).as_slice(), ["ovdata", t] if tile(t, 3))
        || matches!(parts(l).as_slice(), ["global", "heritage", s] if !s.is_empty())
}

fn w_roadunits(l: &str, _: bool) -> bool {
    l == "global/roadunits"
}

fn w_stations(l: &str, _: bool) -> bool {
    pack(l, "stations", &ALL)
}

fn w_ferries(l: &str, _: bool) -> bool {
    pack(l, "ferries", &ALL) || l == inputs::inputs::gtfs::FERRY_LINES
}

/// The timetable feeds: each feed's zip, the lists of those checked, fetched and kept (the rail
/// feeds' and the ferry feeds'), and the catalogue when it was fetched.
fn w_rail_feeds(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "rail", "checked" | "fetched" | "feeds" | "ferry-feeds" | "catalogue"])
        || matches!(parts(l).as_slice(), ["sources", "rail", "gtfs", id] if !id.is_empty())
}

fn w_rail(l: &str, _: bool) -> bool {
    l == "work/rail/used" || l == "global/railfreq" || l == inputs::inputs::gtfs::RAIL_LINES
}

/// The timetables' report (its to-do list is written into the inputs folder's todo/, outside the
/// records).
fn w_timetables_todo(l: &str, _: bool) -> bool {
    l == inputs::inputs::gtfs::REPORT
}

fn w_bldprep(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "bld", t] if tile(t, 6))
}

/// The 3D buildings' hi packs, their field's and its mids.
fn w_bldtiles(l: &str, _: bool) -> bool {
    pack(l, bld::bld::LAYER, &["hi"])
        || pack(l, bld::bld::field::LAYER, &["hi"])
        || matches!(parts(l).as_slice(), ["work", "bldfield-mid", t] if tile(t, 6))
}

fn w_bldfield_lo(l: &str, _: bool) -> bool {
    pack(l, bld::bld::field::LAYER, &["lo"])
}

/// What the regions no longer cover, removed: a z6 tile's files, candidates and peaks, map tiles, the
/// 3D buildings'. Nothing written.
fn w_prune(l: &str, removal: bool) -> bool {
    removal
        && (w_unit(l, false)
            || w_pois(l, false)
            || w_peaks(l, false)
            || w_pack(l, false)
            || w_lo(l, false)
            || w_bldprep(l, false)
            || w_bldtiles(l, false)
            || w_bldfield_lo(l, false))
}

// (The plan's own tests: pipeline's alone, not where agentcore compiles the plan in, agentcore/build.rs.)
#[cfg(all(test, not(in_agentcore)))]
mod tests {
    /// A z6 tile waits for the terrain packs along its own long ways, which it stages (task #853,
    /// roads::stage::Area), not only for its box's: they're its `Read::Along`s, worked out from its
    /// reach into the view (the planner's `Left::along`, the forecast's input). Without them it
    /// would build, then build again once they're made.
    #[test]
    fn a_unit_waits_for_the_terrain_along_its_long_ways() {
        use super::{along_add, read_targets, readiness, Along, Left, ReadTarget, Ready};
        use roads::reach::{LongWay, Reach};
        // 6/31/19 (its tile reaches 55.8° N; its box, 35 km further) owns a way running 300 km
        // south from 55° N, a node every 2 km.
        let t = "6/31/19";
        let e7 = |lon: f64, lat: f64| [(lon * 1e7) as i32, (lat * 1e7) as i32];
        let verts: Vec<[i32; 2]> = (0..150).map(|k| e7(-1.6 + 0.004 * k as f64, 55.0 - 0.018 * k as f64)).collect();
        let reach = Reach { long: vec![LongWay { owned: true, ferry: false, verts }], ..Default::default() };
        let mut left = Left::default();
        along_add(&mut left.along, "unit", t, &reach);
        // Along it, past its box: the hi pack of 6/31/21 (52.5° N on) and the deep packs at its far end.
        let (hi, deep) = (left.along.get("unit", t, "terrain-hi"), left.along.get("unit", t, "terrain-deep"));
        let far = codecs::legacy::TileId::of_point(terrain::terrain_deep::PACK_Z, e7(-1.0, 52.4)).slash();
        assert_eq!(hi, ["6/31/21"]);
        assert!(deep.contains(&far), "{deep:?}");
        assert!(left.along.get("unit", t, "terrain-lo").is_empty(), "its box's z3 tile holds the rest");
        assert!(hi.iter().chain(deep).all(|p| !super::u_hi(t).contains(p) && !super::u_deep(t).contains(p)));
        let reads = read_targets("unit", t, Some(&left.along));
        assert!(reads.contains(&ReadTarget::One("terrain-deep", far.clone())));
        assert!(!read_targets("unit", t, None).contains(&ReadTarget::One("terrain-deep", far.clone())));
        // Left, they hold it; made, it's ready. A neighbour's long ways and a ferry aren't its.
        left.set("terrain-deep", [far.clone()]);
        left.set("terrain-hi", ["6/31/21".to_string()]);
        assert_eq!(
            readiness("unit", t, &left),
            Ready::Waits(vec![("terrain-deep", far.clone()), ("terrain-hi", "6/31/21".into())])
        );
        assert_eq!(readiness("unit", t, &Left { along: Along::default(), ..left.clone() }), Ready::Yes);
        let theirs = Reach {
            long: reach.long.iter().map(|w| LongWay { owned: false, ..w.clone() }).collect(),
            ..Default::default()
        };
        let ferry = Reach {
            long: reach.long.iter().map(|w| LongWay { ferry: true, ..w.clone() }).collect(),
            ..Default::default()
        };
        for r in [theirs, ferry] {
            let mut l = Left { along: Along::default(), ..left.clone() };
            along_add(&mut l.along, "unit", t, &r);
            assert!(l.along.is_empty());
            assert_eq!(readiness("unit", t, &l), Ready::Yes);
        }
    }

    /// A pole's root waits for its own pole's blocks only, both of their steps; a block's slope for
    /// its own and its neighbours' terrain (task #879).
    #[test]
    fn a_poles_root_waits_for_its_own_blocks() {
        use super::{read_targets, readiness, Left, ReadTarget, Ready};
        use roadcore::polar::Pole;
        let reads = read_targets("polar-root", "s", None);
        let south = terrain::polar::blocks(Pole::South);
        assert_eq!(reads.len(), 2 * south.len());
        assert!(reads.iter().all(|r| matches!(r, ReadTarget::One(_, t) if t.starts_with("s/"))), "{reads:?}");
        let mut left = Left::default();
        left.set("polar-terrain", [terrain::polar::target(Pole::North, terrain::polar::blocks(Pole::North)[0])]);
        assert_eq!(readiness("polar-root", "s", &left), Ready::Yes, "the other pole's block");
        let b = terrain::polar::target(Pole::South, south[0]);
        left.set("polar-slope", [b.clone()]);
        assert_eq!(readiness("polar-root", "s", &left), Ready::Waits(vec![("polar-slope", b)]));
    }

    /// Task #975: a z6 tile's terrain-hi and its z10 tiles' terrain-deep wait for that z6 tile's
    /// pyramids-fill (they read its tiles from the store, asking GSI for none), not another's; a
    /// terrain-hi through its terrain-deep too.
    #[test]
    fn the_terrain_waits_for_its_own_pyramids_fill() {
        use super::{readiness, Left, Ready};
        let mut left = Left::default();
        left.set("pyramids-fill", ["6/56/24".to_string()]);
        assert_eq!(readiness("terrain-hi", "6/56/25", &left), Ready::Yes, "another z6 tile's fill");
        // (And for its z10 tiles' terrain-deep, which wait for the same fill.)
        let Ready::Waits(w) = readiness("terrain-hi", "6/56/24", &left) else { panic!("it waits") };
        assert_eq!(w[0], ("pyramids-fill", "6/56/24".into()));
        assert!(
            w[1..].iter().all(|(s, t)| *s == "terrain-deep" && t.starts_with("10/")) && w.len() == 1 + 256,
            "{w:?}"
        );
        assert_eq!(
            readiness("terrain-deep", "10/901/399", &left),
            Ready::Waits(vec![("pyramids-fill", "6/56/24".into())])
        );
        assert_eq!(readiness("terrain-deep", "10/900/400", &left), Ready::Yes);
    }

    /// Task #701: no step makes a removed step's names (agentcore::agent::gc::RETIRED); the clean-up
    /// removes them, and nothing else.
    #[test]
    fn no_step_makes_a_retired_name() {
        for p in agentcore::agent::gc::RETIRED {
            for l in [format!("{p}v1"), format!("{p}v3-max")] {
                assert!(!made(&l), "{l}");
                assert_eq!(writes("gc", &l, true), Some(true));
            }
        }
        assert_eq!(writes("gc", "base/6-1-1", true), Some(false));
        // (Task #52: the legacy folder only, as removals.)
        assert_eq!(writes("gc", "global/legacy/ferries", true), Some(true));
        assert_eq!(writes("gc", "global/legacy/ferries", false), Some(false));
        for l in ["global/railfreq", "global/legacyx/ferries", "global/heritage/layer-summary"] {
            assert_eq!(writes("gc", l, true), Some(false), "{l}");
        }
    }

    /// Task #679: the clean-up removes the depth's packs (a z6 tile's hi, a z3 tile's lo), never
    /// writes one, and removes no other layer's.
    #[test]
    fn the_clean_up_removes_depth_packs_only() {
        for l in [
            "layers/depth/hi/6-1-33",
            "layers/depth-class/hi/6-1-33",
            "layers/depth-shade/hi/6-1-33",
            "layers/depth/lo/3-0-4",
            "layers/depth-shade/lo/3-0-4",
        ] {
            assert_eq!(writes("gc", l, true), Some(true), "{l}");
            assert_eq!(writes("gc", l, false), Some(false), "{l}");
        }
        for l in ["layers/terrain/hi/6-1-33", "layers/slope/lo/3-0-4", "hidata/6-1-33"] {
            assert_eq!(writes("gc", l, true), Some(false), "{l}");
        }
    }

    use super::*;
    use agentcore::pool::journal::Entry;
    use buildstore::handoff::Handoff;
    use jobrt::jobid::JobId;

    /// The build page's schedule colours each stage from STAGE_COLOURS (web/buildpage/dash.js), falling
    /// back to upkeep's grey: every stage has a colour there, and no two stages share one.
    #[test]
    fn every_stage_has_a_colour_of_its_own_on_the_worker_page() {
        // index.html sets each stage's --st-<id> once per theme: the first of each is the dark theme's, the second the light's.
        let html = include_str!("../../../../web/buildpage/index.html");
        let mut themes: [std::collections::BTreeMap<&str, String>; 2] = Default::default();
        for decl in html.split(';') {
            let Some(at) = decl.find("--st-") else { continue };
            let (k, v) = decl[at + 5..].split_once(':').expect("--st-<id>: colour");
            let theme = if themes[0].contains_key(k.trim()) { 1 } else { 0 };
            themes[theme].insert(k.trim(), v.trim().to_ascii_lowercase());
        }
        for colours in &themes {
            for s in &STAGES {
                assert!(colours.contains_key(s.id), "stage {} has no --st-{} in index.html", s.id, s.id);
            }
            let mut seen = std::collections::BTreeMap::new();
            for (k, v) in colours {
                if let Some(other) = seen.insert(v, k) {
                    panic!("stages {other} and {k} share the colour {v} in index.html");
                }
            }
        }
    }

    #[test]
    fn made_is_what_todays_steps_write() {
        // The worldwide files the build makes (each family the NAS's manifest holds today).
        for l in [
            "global/roads/6-28-16",
            "global/roadids/6-28-16",
            "global/roaden/6-28-16",
            "global/heritage/layer-summary",
            "global/heritage/heritage-sources",
            "global/marks/summary",
            "global/railfreq",
            "global/roadunits",
            "global/spoken",
        ] {
            assert!(made(l), "{l}");
        }
        // Names no step makes under global/: not today's outputs.
        for l in ["global/pois", "global/other/x"] {
            assert!(!made(l), "{l}");
        }
    }

    #[test]
    fn the_sets_are_todays() {
        // (As they were before the table: the agent's and the coordinator's rules unchanged; their
        // order is the table's, no preference, task #909.)
        let set = |v: &[&'static str]| v.iter().copied().collect::<std::collections::BTreeSet<&str>>();
        assert_eq!(
            set(&SHARED),
            set(&[
                "terrain-hi",
                "terrain-deep",
                "slope-deep",
                "slope-hi",
                "trees",
                "unit",
                "pois",
                "peaks",
                "bldprep",
                "bldtiles",
                "lake-depth",
                "depth"
            ])
        );
        assert_eq!(set(&ANSWERED), set(&["items", "heritage-sites", "heritage"]));
    }

    #[test]
    fn steps_read_together_are_known_from_the_reads() {
        // (Task #909: slope-deep reads terrain-deep's 3×3, slope-hi terrain-hi's and slope-deep's,
        // an assembly its z6 tiles.)
        for s in ["terrain-deep", "terrain-hi", "terrain-lo", "slope-deep", "slope-hi", "trees"] {
            assert!(read_together(s), "{s}");
        }
        assert!(!read_together("catalog"));
        // A run of four targets in block order is a 2×2 square.
        let mut ts: Vec<String> = (0..4).flat_map(|y| (0..4).map(move |x| format!("10/{x}/{y}"))).collect();
        ts.sort_by_key(|t| block_order(t));
        assert_eq!(ts[..4], ["10/0/0", "10/1/0", "10/0/1", "10/1/1"]);
        assert_eq!(block_order("s/x"), None);
    }

    #[test]
    fn readers_go_before_what_they_read() {
        // (Task #909: the owner's case, a ready slope-deep job before terrain-deep's; slope-hi
        // before both; steps the reads don't relate in the order given.)
        assert!(reads_from("slope-deep", "terrain-deep"));
        assert!(reads_from("slope-lo", "terrain-hi"), "through terrain-lo");
        assert!(!reads_from("terrain-deep", "slope-deep"));
        let v = vec!["terrain-deep", "trees", "slope-deep", "slope-hi"];
        assert_eq!(readers_first(v, |s| s), ["slope-hi", "slope-deep", "terrain-deep", "trees"]);
        let v = vec!["trees", "terrain-hi", "pois"];
        assert_eq!(readers_first(v.clone(), |s| s), v);
    }

    #[test]
    fn the_rows_are_todays() {
        // No row twice.
        for s in TABLE.iter().map(|s| s.name) {
            assert_eq!(TABLE.iter().filter(|r| r.name == s).count(), 1, "{s}");
        }
    }

    #[test]
    fn a_shared_steps_saves_are_in_its_write_set() {
        for (step, t) in [
            ("unit", "6/3/4"),
            ("pois", "6/3/4"),
            ("peaks", "6/3/4"),
            ("bldprep", "6/3/4"),
            ("bldtiles", "6/3/4"),
            ("trees", "6/3/4"),
            ("trees", "3/1/1"),
            ("trees-lo", "3/1/1"),
            ("terrain-hi", "6/3/4"),
            ("terrain-hi", "3/1/1"),
            ("slope-hi", "6/3/4"),
            ("slope-hi", "3/1/1"),
            ("terrain-lo", "3/1/1"),
            ("slope-lo", "3/1/1"),
            // (An older app's names, read as the new ones'.)
            ("terrain", "6/3/4"),
            ("slope", "3/1/1"),
            ("terrain-deep", "10/3/4"),
            ("slope-deep", "10/3/4"),
        ] {
            let dash = t.replace('/', "-");
            let mut names: Vec<String> = Vec::new();
            for l in [
                "base",
                "global/roads",
                "global/roaden",
                "layers/grid-class/hi",
                "layers/grid-canopy/hi",
                "layers/grid-cover/hi",
                "work/pois",
                "work/peaks",
                "work/bld",
                "layers/buildings/hi",
                "work/trees-mid",
                "work/terrain-mid",
                "work/slope-mid",
                "layers/terrain/hi",
                "layers/terrain/lo",
                "layers/slope/hi",
                "layers/slope/lo",
                "layers/trees-cover/hi",
                "layers/trees-leaf/lo",
                "layers/terrain/deep",
                "layers/terrain/fine",
                "layers/slope/deep",
                "work/slope-deep-mid",
            ] {
                names.push(format!("{l}/{dash}"));
                names.push(format!("{l}/6-8-9"));
            }
            let saved: Vec<&String> = names.iter().filter(|l| saves(step, t, l)).collect();
            assert!(!saved.is_empty(), "{step} {t} saves something");
            for l in saved {
                assert!((row(step).unwrap().writes)(l, false), "{step} {t}: {l}");
            }
        }
        assert!(!saves("pack", "6/3/4", "hidata/6-3-4"), "not a shared step");
    }

    #[test]
    fn a_targets_published_output_is_one_it_saves() {
        use std::collections::BTreeMap;
        // Each name output_named looks for is one `saves` counts as the target's (one mechanism),
        // and a target with none of them named made nothing to publish.
        for (step, t) in [
            ("unit", "6/3/4"),
            ("terrain-deep", "10/3/4"),
            ("slope-deep", "10/3/4"),
            ("terrain-hi", "6/3/4"),
            ("slope-hi", "6/3/4"),
            ("trees", "6/3/4"),
            ("terrain-lo", "3/3/4"),
            ("slope-lo", "3/3/4"),
            ("trees-lo", "3/3/4"),
            ("depth", "6/3/4"),
            ("depth-lo", "3/3/4"),
            ("bldtiles", "6/3/4"),
            ("bldfield-lo", "3/3/4"),
        ] {
            assert_eq!(output_named(step, t, &BTreeMap::new()), Some(false), "{step} {t}: nothing named");
            // (Every published name `saves` has for it, among the shapes the steps write.)
            let dash = t.replace('/', "-");
            let (x, y) = (3, 4);
            let mut cands: Vec<String> = roads::unit::saved_files(&dash).to_vec();
            for ly in ["terrain", "slope", "trees-cover", "trees-leaf", "trees-height", "buildings", "buildings-field"]
            {
                for sc in ["hi", "lo", "deep", "fine"] {
                    cands.push(format!("layers/{ly}/{sc}/{dash}"));
                }
            }
            cands.extend(DEPTH_LAYERS.iter().flat_map(|ly| {
                [terrain::depth::build::hi_logical(ly, x, y), terrain::depth::build::lo_logical(ly, x, y)]
            }));
            cands.extend([
                bld::bld::pack_logical(x, y),
                bld::bld::field::hi_logical(x, y),
                bld::bld::field::lo_logical(x, y),
            ]);
            let mut found = false;
            for l in cands.iter().filter(|l| saves(step, t, l) && (l.starts_with("layers/") || l.starts_with("base/")))
            {
                let m: BTreeMap<String, String> = [(l.clone(), "f".into())].into();
                assert_eq!(output_named(step, t, &m), Some(true), "{step} {t}: {l}");
                found = true;
            }
            assert!(found, "{step} {t}: a published name");
            let other: BTreeMap<String, String> = [("layers/terrain/deep/10-9-9".into(), "f".into())].into();
            assert_eq!(output_named(step, t, &other), Some(false), "{step} {t}: another tile's");
        }
        assert_eq!(output_named("pois", "6/3/4", &BTreeMap::new()), None, "a work step's: not told");
        assert_eq!(output_named("terrain-hi", "3/3/4", &BTreeMap::new()), None, "a z3 whole run: not told");
    }

    #[test]
    fn every_step_the_agent_runs_has_a_row() {
        // The steps the agent makes jobs of, as its source names them: the planner's work
        // (`Work { step: "…"`), the jobs it titles (`job(format!("…`, `id: "…"`; the pass's
        // worldwide jobs, build::pass_work's: `job("…`, `id: format!("…`), the steps the
        // checklist counts (build::COUNTED), and those whose names it puts together.
        // (Each source with its trimmed lines joined, as rustfmt breaks a call across lines.)
        let flat = |s: &str| s.lines().map(str::trim).collect::<String>();
        // (The agent's: its round's and its plan's, agent::plan.)
        let agent = flat(include_str!("../../../agentcore/src/agent/mod.rs"))
            + &flat(include_str!("../../../agentcore/src/agent/plan.rs"));
        let build = flat(include_str!("build.rs"));
        let (agent, build) = (agent.as_str(), build.as_str());
        let mut named = std::collections::BTreeSet::<String>::new();
        let mut take = |src: &str, pat: &str| {
            for (i, _) in src.match_indices(pat) {
                let name: String = src[i + pat.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                    .collect();
                if name.len() > 1 {
                    named.insert(name);
                }
            }
        };
        take(build, "Work { step: \"");
        take(build, "job(\"");
        take(build, "id: format!(\"");
        take(agent, "Work { step: \"");
        take(agent, "job(format!(\"");
        take(agent, "id: \"");
        take(agent, "id: format!(\"");
        named.extend(super::super::build::COUNTED.iter().map(|s| s.to_string()));
        named.extend(
            [
                "stations",
                "ferries",
                "terrain-lo",
                "slope-lo",
                "terrain-root",
                "slope-root",
                "polar-terrain",
                "polar-slope",
                "polar-root",
                "unit",
                "osm-pass",
                "regions-extract",
                // (The owner's redos: `redo::job_id`.)
                "redo",
            ]
            .map(String::from),
        );
        // (Not steps: a task's lease.)
        named.remove("task");
        assert!(named.len() >= 40, "{named:?}");
        let missing: Vec<&String> = named.iter().filter(|s| row(s).is_none()).collect();
        assert!(missing.is_empty(), "steps the agent runs with no row: {missing:?} (of {named:?})");
        // And no row for a step the agent never runs.
        let unused: Vec<&str> = TABLE.iter().map(|s| s.name).filter(|s| !named.contains(*s)).collect();
        assert!(unused.is_empty(), "rows of no step the agent runs: {unused:?}");
    }

    #[test]
    fn every_row_is_described_alike() {
        // A sentence each, ids unique, every stage known and used, every task's step a row; the
        // nouns by scope; a worldwide step is one run.
        let mut ids = std::collections::BTreeSet::new();
        for s in TABLE.iter() {
            assert!(s.does.ends_with('.') && s.does.len() > 20, "{}: {:?}", s.name, s.does);
            assert!(ids.insert(s.name), "{} twice", s.name);
            assert!(stage(s.stage).is_some(), "{}: stage {}", s.name, s.stage);
            if !agentcore::agent::every::names_target(s.name) {
                assert_eq!(s.scope, World, "{}: a step whose target isn't named is worldwide", s.name);
            }
        }
        for st in STAGES {
            assert!(TABLE.iter().any(|s| s.stage == st.id), "stage {} has no step", st.id);
        }
        for t in TASKS {
            assert!(row(t.step).is_some(), "{}: step {}", t.name, t.step);
            assert_eq!(stage_of(t.name).map(|g| g.id), Some(row(t.step).unwrap().stage));
        }
        assert_eq!(
            (noun("unit"), noun("terrain-lo"), noun("outlines"), noun("inputs"), noun("tail"), noun("a newer step")),
            ("z6 tile", "z3 tile", "run", "input", "task", "target")
        );
        assert_eq!(said("terrain", 8), "terrain-hi: 8 z6 tiles", "an old name said by its new");
        assert_eq!(said("terrain-lo", 1), "terrain-lo: 1 z3 tile");
        assert_eq!(said("outlines", 1), "outlines");
        assert_eq!(stage_of("publishing"), None, "no lane of its own: publishing goes per unit");
        // The place, from the pool's rules: the shared steps and the map tiles any member's, the
        // lead's own and the worldwide steps the lead's.
        for s in ["terrain-hi", "terrain-deep", "slope-deep", "slope-hi", "unit", "pack", "lo", "bldprep"] {
            assert_eq!(every::place(crate::agent::theplan::the_plan(), s), Place::Any, "{s}");
        }
        for s in ["catalog", "osm-pass", "items", "marks", "rail", "terrain-root", "gc", "inputs", "pyramids-fill"] {
            assert_eq!(every::place(crate::agent::theplan::the_plan(), s), Place::Lead, "{s}");
        }
        // The view: a row per step then per task, serializable, the task's step named.
        let v = table();
        assert_eq!(v.len(), TABLE.len() + TASKS.len());
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j[0]["id"], "osm-pass");
        assert_eq!(j[0]["scope"], "world");
        assert_eq!(j[0]["place"], "lead");
        let tail = v.iter().find(|i| i.id == "tail").unwrap();
        assert_eq!((tail.of.as_deref(), tail.stage.as_str(), tail.noun.as_str()), (Some("unit"), "roads", "task"));
    }

    #[test]
    fn the_ids_are_the_stored_names() {
        // The ids are what the records, journal, timings, leases and floors hold (docs/pipeline.md
        // §3): a change here would call every target stale, so none is made. The table's, in its
        // order, and the tasks'.
        let ids: Vec<&str> = TABLE.iter().map(|s| s.name).collect();
        assert_eq!(
            ids,
            [
                "osm-pass",
                "planet-fetch",
                "pass-sets",
                "outlines",
                "trailends",
                "reach",
                "buildings",
                "summits",
                "labels",
                "lakefill",
                "water",
                "spoken",
                "regions-extract",
                "regions-land",
                "heritage-sites",
                "terrain-water",
                "pyramids-fill",
                "terrain-hi",
                "terrain-deep",
                "terrain-lo",
                "terrain-root",
                "slope-deep",
                "slope-hi",
                "slope-lo",
                "polar-terrain",
                "polar-slope",
                "polar-root",
                "trees",
                "trees-lo",
                "lake-depth",
                "depth",
                "depth-lo",
                "depth-root",
                "unit",
                "pack",
                "lo",
                "prune",
                "roadunits",
                "stations",
                "ferries",
                "slope-root",
                "rail-feeds",
                "rail",
                "pois",
                "peaks",
                "items",
                "heritage",
                "marks",
                "overlays",
                "bld-fetch",
                "bldprep",
                "bldtiles",
                "bldfield-lo",
                "catalog",
                "catalog-held",
                "inputs",
                "names-todo",
                "timetables-todo",
                "gc",
                "redo",
                "backup"
            ]
        );
        assert_eq!(TASKS.map(|t| t.name), ["tail", "terrainsub", "treeblock", "bldtile"]);
    }

    fn entry(step: &str, changes: &[(&str, bool)]) -> Entry {
        let mut h = Handoff::default();
        for (l, removed) in changes {
            h.changes.insert(l.to_string(), (!removed).then(|| format!("{l}.0123456789abcdef.bin")));
        }
        Entry {
            member: "m-0000000000000001".into(),
            job: JobId { epoch: 1, n: 1 },
            step: step.into(),
            handoff: h,
            at: 1_791_500_000,
        }
    }

    #[test]
    fn entries_within_their_write_sets() {
        let ok: [(&str, &[(&str, bool)]); 29] = [
            ("lakefill", &[("work/lakefill/2026-09-28", false), ("work/lakefill/2026-03-01", true)]),
            (
                "inputs",
                &[
                    ("sources/inputs/_gate-test/@index", false),
                    ("sources/inputs/_gate-test/@held", true),
                    ("sources/inputs/_gate-test/@listed", false),
                    ("sources/inputs/_gate-nest/inner/@index", false),
                    ("sources/inputs/_gate-nest/inner/@listed", false),
                ],
            ),
            (
                "osm-pass",
                &[
                    ("sources/osm/2026-09-28/pieces/6-1-2", false),
                    ("sources/osm/2026-09-28/pass", false),
                    ("layers/basemap/world-2026-09-28", false),
                    ("sources/osm/2026-08-01/sets/hikes", true),
                    ("work/heritage/2026-08-01/base/heritage", true),
                    ("work/summits/2026-08-01", true),
                ],
            ),
            ("pass-sets", &[("sources/osm/2026-09-28/sets/summits-v2", false)]),
            ("outlines", &[("sources/osm/2026-09-28/outlines", false)]),
            (
                "water",
                &[
                    ("layers/water/root/0-0-0", false),
                    ("layers/water/lo/3-1-2", false),
                    ("layers/water/hi/6-1-2", false),
                    ("global/water-enclosed", false),
                    ("layers/smallwater/hi/6-1-2", true),
                ],
            ),
            ("labels", &[("layers/labels/hi/6-1-2", false), ("layers/labels/root/0-0-0", true)]),
            ("spoken", &[("global/spoken", false)]),
            ("regions-extract", &[("sources/osm/2026-09-28/regions-extract", false)]),
            ("regions-land", &[("sources/osm/2026-09-28/regions-land", false)]),
            (
                "heritage-sites",
                &[
                    ("work/heritage/2026-09-28/pos/6-1-2", false),
                    ("work/heritage/2026-09-28/base/area-shapes", false),
                    ("work/heritage/2026-09-28/areas/6-3-3", true),
                ],
            ),
            (
                "terrain",
                &[
                    ("layers/terrain/hi/6-10-20", false),
                    ("work/terrain-mid/6-10-20", false),
                    ("layers/terrain/lo/3-1-2", false),
                ],
            ),
            ("terrain-lo", &[("layers/terrain/lo/3-1-2", false)]),
            ("terrain-water", &[("work/water-idx/0f1a2b3c4d5e6f70", false)]),
            ("slope-root", &[("layers/slope/root/0-0-0", false)]),
            ("trees", &[("layers/trees-cover/hi/6-1-2", false), ("work/trees-mid/6-1-2", true)]),
            ("unit", &[("base/6-1-2", false), ("global/roaden/6-1-2", true), ("layers/grid-canopy/hi/6-1-2", false)]),
            ("pack", &[("hidata/6-1-2", false), ("layers/roads/hi/6-1-2", false), ("layers/rails/hi/6-1-2", true)]),
            ("lo", &[("layers/roads/lo/3-1-2", false)]),
            ("items", &[("sources/items/2026-09-28/facts", false)]),
            (
                "heritage",
                &[("work/heritage/2026-09-28/details-harea", false), ("work/heritage/2026-09-28/layer-whs", true)],
            ),
            (
                "marks",
                &[
                    ("layers/marks-peak/lo/3-1-2", true),
                    ("layers/marks-peak/all", false),
                    ("markdata/6-1-2", true),
                    ("markdata/all", false),
                    ("global/marks/summary", false),
                    ("work/marks/heritage-dots", false),
                ],
            ),
            (
                "overlays",
                &[
                    ("layers/ov-whs/hi/6-1-2", true),
                    ("layers/ov-whs/all", false),
                    ("ovdata/3-1-2", false),
                    ("global/heritage/layer-summary", false),
                ],
            ),
            (
                "rail-feeds",
                &[
                    ("sources/rail/gtfs/de-db", false),
                    ("sources/rail/feeds", false),
                    ("sources/rail/ferry-feeds", false),
                    ("sources/rail/catalogue", false),
                ],
            ),
            ("rail", &[("global/railfreq", false)]),
            ("bldprep", &[("work/bld/6-56-25", true)]),
            ("bldtiles", &[("layers/buildings/hi/6-56-25", false)]),
            ("prune", &[("base/6-1-2", true), ("layers/roads/lo/3-1-2", true)]),
            ("catalog", &[]),
        ];
        for (step, ch) in ok {
            assert_eq!(outside(&entry(step, ch)), None, "{step}");
        }
        // Outside: another step's files, a name of the wrong zoom, another layer, a pass's files
        // removed by a step that doesn't retire passes, a prune that writes, a removal the step
        // doesn't make.
        let bad: [(&str, &[(&str, bool)]); 14] = [
            // (Bundles of another step's layer, task #449.)
            ("marks", &[("layers/ov-whs/all", false)]),
            ("overlays", &[("layers/stations/all", false)]),
            ("lakefill", &[("work/lakefill", false), ("work/lakefill/x", false)]),
            (
                "inputs",
                &[
                    ("sources/inputs/_gate-test/a", false),
                    ("sources/inputs/_gate-test/index", false),
                    ("sources/inputs/x/y/@other", false),
                ],
            ),
            ("pack", &[("hidata/6-1-2", false), ("layers/terrain/hi/6-1-2", false)]),
            ("terrain", &[("work/terrain-mid/3-1-2", false)]),
            ("lo", &[("layers/roads/lo/6-1-2", false)]),
            ("labels", &[("layers/stations/hi/6-1-2", false)]),
            ("items", &[("sources/items/2026-08-01/facts", false), ("work/summits/2026-08-01", true)]),
            ("prune", &[("base/6-1-2", false)]),
            (
                "outlines",
                &[
                    ("sources/osm/2026-09-28/sets/outlines", false),
                    ("sources/osm/2026-09-28/pieces/6-1-2", false),
                    ("sources/osm/2026-08-01/outlines", true),
                ],
            ),
            ("water", &[("layers/smallwater/hi/6-1-2", false)]),
            ("catalog", &[("catalog/1", false)]),
            ("unit", &[("base/6-64-2", false)]),
        ];
        for (step, ch) in bad {
            assert!(outside(&entry(step, ch)).is_some(), "{step}: {ch:?}");
        }
        let why =
            outside(&entry("pack", &[("hidata/6-1-2", false), ("layers/terrain/hi/6-1-2", false), ("work/x", true)]))
                .unwrap();
        assert!(why.contains("layers/terrain/hi/6-1-2") && why.contains("and 1 more"), "{why}");
        // A step the table doesn't know: outside, if it changes anything.
        assert!(outside(&entry("newer-step", &[("x/y", false)])).unwrap().contains("isn't a step"));
        assert_eq!(outside(&entry("newer-step", &[])), None);
    }

    /// Every entry of a copy of the NAS's journal (`SCENIC_JOURNAL`: a folder of its day folders)
    /// within its step's write-set: `SCENIC_JOURNAL=… cargo test -p pipeline real_journal --
    /// --ignored`.
    #[test]
    #[ignore]
    fn real_journal_entries_are_within_their_write_sets() {
        let Some(dir) = std::env::var_os("SCENIC_JOURNAL") else { panic!("SCENIC_JOURNAL names no folder") };
        let (mut n, mut bad) = (0, Vec::new());
        let mut steps = std::collections::BTreeMap::<String, usize>::new();
        for day in std::fs::read_dir(&dir).unwrap().flatten() {
            for f in std::fs::read_dir(day.path()).into_iter().flatten().flatten() {
                let Ok(b) = std::fs::read(f.path()) else { continue };
                let Ok(e) = serde_json::from_slice::<Entry>(&b) else { continue };
                n += 1;
                *steps.entry(e.step.clone()).or_default() += 1;
                if let Some(why) = outside(&e) {
                    bad.push(format!("{}: {why}", f.path().display()));
                }
            }
        }
        eprintln!("{n} entries: {steps:?}");
        assert!(n > 0);
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
