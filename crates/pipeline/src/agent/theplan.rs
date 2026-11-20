//! The plan's side of the seam (docs/crates.md §6.3, 6f2): the runtime's questions (seam::Plan)
//! answered from the steps table and build.rs. The binaries hand `the_plan()` to the runtime.

use super::build::{self, KeyCtx, KeyMemo};
use super::steps::{self, Kind, Read};
use super::tiles::TerrainTiles;
use agentcore::agent::forecast::ReadTarget;
use agentcore::agent::jobs::Work;
use agentcore::agent::nasread;
use agentcore::agent::progress::{Item, Line};
use agentcore::agent::seam::{
    Along, Left, NamesTodo, PassAsk, PassWork, Plan, PlanHeld, Planned, Publish, RegionCounts, Round, Shadow,
};
use agentcore::pool::records::Keys;
use roads::reach::Reaches;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use store::nasfs::{NasPath, NasRoot};

/// The plan, as the runtime holds it.
pub fn the_plan() -> &'static dyn Plan {
    &ThePlan
}

struct ThePlan;

impl Plan for ThePlan {
    fn step_names(&self) -> Vec<&'static str> {
        steps::TABLE.iter().map(|r| r.name).collect()
    }
    fn is_step(&self, step: &str) -> bool {
        steps::row(step).is_some()
    }
    fn shared(&self) -> &'static [&'static str] {
        &steps::SHARED
    }
    fn answered(&self, step: &str) -> bool {
        steps::ANSWERED.contains(&step)
    }
    fn alone(&self, step: &str) -> bool {
        steps::alone(step)
    }
    fn records_only(&self, step: &str) -> bool {
        steps::records_only(step)
    }
    fn fetches(&self, step: &str) -> bool {
        steps::row(step).is_some_and(|r| r.kind == Kind::Fetch)
    }
    fn cost_version(&self, step: &str) -> u32 {
        steps::row(step).map_or(0, |s| s.cost_v)
    }
    fn sources_naming(&self, step: &str, target: &str) -> Vec<&'static str> {
        let reads = steps::row(step).map_or(&[][..], |r| r.reads);
        reads
            .iter()
            .filter_map(|r| match r {
                Read::Targets { step: src, of, .. } if of(target).iter().any(|t| t == target) => Some(*src),
                _ => None,
            })
            .collect()
    }
    fn noun(&self, step: &str) -> &'static str {
        steps::noun(step)
    }
    fn said(&self, step: &str, n: usize) -> String {
        steps::said(step, n)
    }
    fn task_kinds(&self) -> Vec<&'static str> {
        steps::TASKS.iter().map(|t| t.name).collect()
    }
    fn task_disk(&self, kind: &str) -> Option<u64> {
        steps::task(kind).map(|t| t.disk)
    }
    fn is_none_key(&self, step: &str, t: &str, key: &str) -> bool {
        build::is_none_key(step, t, key)
    }
    fn output_named(&self, step: &str, target: &str, m: &BTreeMap<String, String>) -> Option<bool> {
        steps::output_named(step, target, m)
    }
    fn output_names(&self, step: &str, target: &str) -> Option<Vec<String>> {
        steps::output_names(step, target)
    }
    fn saves(&self, step: &str, target: &str, l: &str) -> bool {
        steps::saves(step, target, l)
    }
    fn writes(&self) -> buildstore::out::Writes {
        steps::writes
    }
    fn outside(&self, e: &agentcore::pool::journal::Entry) -> Option<String> {
        steps::outside(e)
    }
    fn outside_by(
        &self,
        e: &agentcore::pool::journal::Entry,
        writes: &dyn Fn(&str, bool) -> Option<bool>,
    ) -> Option<String> {
        steps::outside_by(e, writes)
    }
    fn read_steps(&self, step: &str) -> Vec<(&'static str, bool)> {
        steps::read_steps(step)
    }
    fn read_targets_passing(&self, step: &str, t: &str, along: Option<&Along>) -> Vec<(ReadTarget, bool)> {
        steps::read_targets_passing(step, t, along)
    }
    fn input_readers(&self, name: &str) -> Vec<&'static str> {
        steps::input_readers(name).collect()
    }
    fn summits_waits(&self, left: &Left) -> Option<String> {
        match steps::readiness("summits", "summits", left) {
            steps::Ready::Waits(w) => Some(steps::wait_note(&w)),
            _ => None,
        }
    }
    fn table_json(&self) -> serde_json::Value {
        serde_json::json!({ "steps": steps::table(), "stages": steps::STAGES })
    }
    fn reads_sources(&self, step: &str) -> bool {
        build::reads_sources(step)
    }
    fn reads_lakefill(&self, step: &str) -> bool {
        build::reads_lakefill(step)
    }
    fn catalogue_waits_for(&self, step: &str) -> bool {
        build::CATALOGUE_GATE.contains(&step)
    }
    fn job_of(&self, w: Work) -> Work {
        build::job_of(w)
    }
    fn worldwide_steps(&self) -> &'static [&'static str] {
        &build::WORLDWIDE_STEPS
    }
    fn fold(&self, items: &[Item], next: &BTreeMap<&str, Vec<String>>, notes: &BTreeMap<&str, String>) -> Vec<Line> {
        build::fold(items, next, notes)
    }
    fn checklist_to_come(&self) -> Vec<Item> {
        build::checklist_to_come()
    }
    fn depth_layers(&self) -> [&'static str; 3] {
        [terrain::depth::LAYER, terrain::depth::CLASS_LAYER, terrain::depth::SHADE_LAYER]
    }
    fn terrain_peak(&self, z6: usize) -> u64 {
        terrain::terrain_pack::piece_peak_mb()
            + (z6 as u64).saturating_sub(1) * 20
            + z6 as u64 * terrain::terrain_pack::DEEP_MIDS_MB
            + buildstore::out::UPLOAD_HELD.div_ceil(1 << 20)
    }
    fn task_what(&self, kind: &str, task: &serde_json::Value, unit: &str) -> String {
        match kind {
            bld::bld::task::KIND => format!("{kind}: z8 tile {unit}"),
            trees::trees::task::KIND => format!("{kind}: blocks {}", task["blocks"].as_str().unwrap_or(unit)),
            terrain::terrain_task::KIND => format!("{kind}: subtrees {}", task["subtrees"].as_str().unwrap_or(unit)),
            _ => format!("tail {unit}"),
        }
    }
    fn tree_task_kind(&self) -> &'static str {
        trees::trees::task::KIND
    }
    /// By its first step: "tail" (a z6 tile's last steps), or the 3D buildings', tree cover's or
    /// terrain's (bld::bld::task::KIND, trees::trees::task::KIND, terrain::terrain_task::KIND).
    fn task_kind(&self, spec: &serde_json::Value) -> String {
        let first = &spec["runs"][0];
        if first["prog"] == bld::bld::task::KIND {
            bld::bld::task::KIND.to_string()
        } else if first["what"] == trees::trees::task::KIND {
            trees::trees::task::KIND.to_string()
        } else if first["what"] == terrain::terrain_task::KIND {
            terrain::terrain_task::KIND.to_string()
        } else {
            "tail".to_string()
        }
    }
    fn names_todo_inputs(&self) -> &'static [&'static str] {
        &landmarks::namestodo::INPUTS
    }
    fn names_todo_key(
        &self,
        marks: &BTreeMap<String, store::map::Mark>,
        map: Option<&store::map::Map>,
    ) -> Option<String> {
        landmarks::namestodo::inputs_seq(marks, map)
    }
    fn canopy_files(&self, b: [f64; 4]) -> Vec<String> {
        trees::trees::squares_of(b)
            .into_iter()
            .flat_map(|(top, left)| ["median", "p95", "cover5m"].map(|kind| trees::trees::chm_name(top, left, kind)))
            .collect()
    }
    fn unit_margin_km(&self) -> f64 {
        roads::staging::MARGIN_KM
    }
    fn held(&self, index_dir: PathBuf) -> Box<dyn PlanHeld> {
        Box::new(Held {
            tiles: RefCell::new(TerrainTiles::new(Some(index_dir))),
            reach: RefCell::default(),
            reach_now: RefCell::default(),
            kept: RefCell::default(),
            memo: RefCell::default(),
            shadow: RefCell::default(),
            shadowing: Cell::new(true),
        })
    }
    fn key_inputs(&self, root: &NasRoot, manifest: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        key_inputs(root, manifest)
    }
}

/// The planner's inputs the steps' keys read (build::KeyCtx's `inputs`): `input_digests`, the
/// timetable feeds' declarations as rail-feeds' key names them ("feed-keys": the key names they
/// fetch with; "feeds": the digest of what of them it reads; "?" while their index can't be read,
/// and rail-feeds waits) and the declared elevation datasets' spacing, which the rail and
/// candidates keys name ("?" while it can't be read, and they wait: inputs::dem::spacing). Every
/// reader keying targets reads these (the plan, the checklist, the regions' counts).
pub(super) fn key_inputs(
    root: &store::nasfs::NasRoot,
    manifest: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut inputs = input_digests(root);
    if let Some(ix) = manifest.get(&inputs::inputs::logical(inputs::inputs::feeds::UNIT)) {
        let d = inputs::inputs::feeds::declared(root, ix);
        inputs.insert("feed-keys".into(), d.as_ref().map(|d| d.key_names.join(",")).unwrap_or_else(|_| "?".into()));
        inputs.insert("feeds".into(), d.map(|d| d.keyed).unwrap_or_else(|_| "?".into()));
    }
    // The source extents' pieces as the data-available layers' keys name them ("extents:<theme>":
    // each `.geojson`'s path and keyed digest, a line each; never `score.toml`, the server's), "?"
    // while their index can't be read (and the step waits).
    if let Some(ix) = manifest.get(&inputs::inputs::logical(inputs::inputs::extents::UNIT)) {
        let index = inputs::inputs::read_index(root, ix);
        for theme in inputs::inputs::extents::THEMES {
            let v = index.as_ref().map_or_else(|_| "?".to_string(), |i| extents_lines(i, theme));
            inputs.insert(format!("extents:{theme}"), v);
        }
    }
    // The regions' stored geometry as the region layer's key names it ("regions:geometry": each
    // region file's path and its geometry's content name, a line each), "?" while their index can't
    // be read (and the step waits).
    if let Some(ix) = manifest.get(&inputs::inputs::logical(inputs::inputs::regions::UNIT)) {
        let v = inputs::inputs::read_index(root, ix).map_or_else(|_| "?".to_string(), |i| geometry_lines(&i));
        inputs.insert("regions:geometry".into(), v);
    }
    inputs.insert(
        inputs::dem::spacing::INPUT.into(),
        inputs::dem::spacing::digest_read(root, manifest).unwrap_or_else(|_| "?".into()),
    );
    inputs
}

/// The regions' accepted index's stored geometry: each region file's path and its geometry's content
/// name (`facts.geometry`), a line each; a region that resolved to nothing has none.
pub(super) fn geometry_lines(index: &inputs::inputs::Index) -> String {
    let mut v = String::new();
    for (p, f) in &index.files {
        if let Some(g) = f.facts.get("geometry").and_then(|g| g.as_str()) {
            v.push_str(&format!("{p} {g}\n"));
        }
    }
    v
}

/// A theme's pieces in the extents' accepted index: each `<theme>/<source>.geojson`'s path and keyed
/// digest, a line each.
pub(super) fn extents_lines(index: &inputs::inputs::Index, theme: &str) -> String {
    use inputs::inputs::extents::{kind, Kind};
    let mut v = String::new();
    for (p, f) in &index.files {
        if matches!(kind(p), Kind::Pieces(t, _) if t == theme) {
            v.push_str(&format!("{p} {}\n", f.keyed));
        }
    }
    v
}

/// What jobs read from inputs/ beside the manifest, by digest: which keys inputs/keys.env holds
/// ("keys", `key_names`), and the 3D buildings' downloaded sources ("bld-release", "bld-key", "bldprep 6/x/y",
/// "bldprep-rows 6/x/y": bld::bld::sources::digests).
pub(super) fn input_digests(root: &store::nasfs::NasRoot) -> BTreeMap<String, String> {
    let mut inputs: BTreeMap<String, String> = BTreeMap::new();
    // The heritage registers as the heritage sites' key names them (landmarks::heritage::registers_line);
    // "?" when they can't be read now (the heritage sites then wait), none without an accepted version.
    match landmarks::heritage::registers_now(root) {
        Ok(Some(line)) => {
            inputs.insert("heritage".into(), line);
        }
        Ok(None) => {}
        Err(_) => {
            inputs.insert("heritage".into(), "?".into());
        }
    }
    // The regions as a catalogue records them; "?" when they can't be read now (build::catalogue_work
    // then waits).
    inputs.insert(
        "regions".into(),
        inputs::inputs::regions::read::source_now(root).map(|a| a.version).unwrap_or_else(|_| "?".into()),
    );
    inputs.insert("keys".into(), key_names(&root.join("inputs/keys.env")).unwrap_or_else(|| "?".into()));
    // The 3D buildings' sources, of the declared releases (inputs::inputs::sources): what each z6
    // tile's bldprep reads, and its rows, and what the keys name of the declarations ("bld-key");
    // none before they're accepted (no 3D buildings' work), "?" while they can't be read now.
    match inputs::inputs::sources::open_now(root).and_then(|s| s.map(|s| bld::bld::sources::declared(&s)).transpose()) {
        Ok(Some((release, ghsl, key))) => {
            inputs.extend(bld::bld::sources::digests(root, &release, &ghsl));
            inputs.insert("bld-key".into(), key);
        }
        Ok(None) => {}
        Err(_) => {
            inputs.insert("bld-release".into(), "?".into());
        }
    }
    inputs
}

/// The names of the keys a `KEY=value` file holds a value for, sorted and comma-separated ("" when
/// there's no file; None when it can't be read). Never their values: the names go in job keys.
pub(super) fn key_names(path: &NasPath) -> Option<String> {
    let text = match path.read_to_string() {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(String::new()),
        Err(_) => return None,
    };
    let mut names: Vec<&str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, v)| !k.trim().starts_with('#') && !v.trim().is_empty())
        .map(|(k, _)| k.trim())
        .collect();
    names.sort_unstable();
    names.dedup();
    Some(names.join(","))
}

/// What the planner carries for the plan (seam::PlanHeld).
struct Held {
    /// The terrain packs' indexes, which the z6 tiles' keys read (`tiles`, kept in `pack-idx/`): those
    /// the manifest names, each read once.
    tiles: RefCell<TerrainTiles>,
    /// The pass's reaches as last read, by content name (large: read again only when they change).
    reach: RefCell<Option<(String, Arc<Reaches>)>>,
    /// The reaches this round reads (`begin_round`), by what they were read from (`reach`'s name).
    reach_now: RefCell<Option<(String, Arc<Reaches>)>>,
    /// The last `build::plan` result as it came: what "its jobs" filters again on a lease change,
    /// instead of planning (docs/incremental-planning.md step 2). Dropped as a plan starts.
    kept: RefCell<Option<Planned>>,
    /// The plan's keys, handed on to what derives keys of the same context this round, and kept with
    /// the plan. It's taken again only while its context reads the same (`KeyMemo::serves`): a pass
    /// newly on the NAS, or an index read that couldn't be before, and `build::plan` is made again.
    memo: RefCell<Option<KeyMemo>>,
    /// The plans' shadow (docs/incremental-planning.md step 4): its worker, started with the first
    /// plan.
    shadow: RefCell<Option<Shadowing>>,
    /// Whether each plan has its shadow made (off only in a test of what it costs).
    shadowing: Cell<bool>,
}

/// What a shadow plan's keys read their context as, for the next one's diff: the manifest and the
/// planner's inputs as its round read them, the rest of its context as one digest
/// (`build::context_rest` of the reaches the plan's keys read: what changing moves every key) and
/// what of the terrain's state the entries don't show (`TerrainTiles::load_state`).
struct Context {
    manifest: BTreeMap<String, String>,
    inputs: BTreeMap<String, String>,
    rest: u64,
    load: u64,
}

impl Context {
    /// The bytes it holds, about: each entry's two strings and its slot.
    fn bytes(&self) -> usize {
        [&self.manifest, &self.inputs]
            .iter()
            .flat_map(|m| m.iter())
            .map(|(k, v)| 48 + k.capacity() + v.capacity())
            .sum()
    }
}

/// What a plan's shadow is made from (docs/incremental-planning.md §6, step 4): a copy of the
/// plan's round as the plan read it, and what the plan made of it. Copied on the plan's thread
/// (its manifest, inputs and done keys, its result and keys: tens of milliseconds); the terrain's
/// indexes and memos, the reaches and the coverages are shared, not copied.
struct Ask {
    /// The plan's number among those shadowed (`Shadowing::asked`).
    n: u64,
    covs: Arc<agentcore::agent::Coverages>,
    date: String,
    manifest: BTreeMap<String, String>,
    inputs: BTreeMap<String, String>,
    done: Keys,
    /// The reaches the plan's keys read, by what they were read from.
    reach: Option<(String, Arc<Reaches>)>,
    tiles: TerrainTiles,
    on_map: BTreeMap<String, bool>,
    served: BTreeMap<String, String>,
    since_catalogue: Option<u64>,
    held_for: Option<u64>,
    held: bool,
    way: agentcore::agent::publish::Publishing,
    /// The plan: its result, its keys, and its thread's CPU time and the wall time it took.
    planned: Planned,
    keys: HashMap<(String, String), Option<String>>,
    cpu: Duration,
    wall: Duration,
}

impl Ask {
    /// The publishing it plans by, as the plan's (`Publish::each` is the coverages' own).
    fn publish(&self) -> Publish<'_> {
        Publish {
            each: &self.covs.each,
            on_map: &self.on_map,
            served: &self.served,
            since_catalogue: self.since_catalogue,
            held_for: self.held_for,
            held: self.held,
            way: self.way,
        }
    }
}

/// The plans' shadow (docs/incremental-planning.md §6, step 4): a worker, a thread of the plan
/// part's own at the utility QoS, taking each plan's `Ask` (the latest: those it had no time for
/// are skipped, the diff then spanning them) and holding what its shadow plans kept (`Kept`),
/// none of it shared with the plan. Its line and any difference are logged.
struct Shadowing {
    asks: Sender<Ask>,
    /// How many plans it was asked to shadow.
    asked: u64,
    #[cfg(test)]
    said: std::sync::mpsc::Receiver<(u64, Shadow)>,
    #[cfg(test)]
    last: (u64, Option<Shadow>),
}

/// What a shadow plan keeps for the next (its worker's): the context its keys read and the keys
/// with what each read.
struct Kept {
    at: Context,
    sets: build::ReadSets,
}

impl Shadowing {
    /// Its worker started; None, said, when no thread can be.
    fn start() -> Option<Shadowing> {
        let (asks, taken) = channel::<Ask>();
        #[cfg(test)]
        let (say, said) = channel();
        let run = move || {
            nasread::unphased();
            agentcore::agent::forecast::low_priority();
            let mut kept: Option<Kept> = None;
            while let Ok(mut ask) = taken.recv() {
                let mut skipped = 0;
                while let Ok(next) = taken.try_recv() {
                    (ask, skipped) = (next, skipped + 1);
                }
                let n = ask.n;
                let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shadow(&mut kept, ask, skipped)));
                let s = match made {
                    Ok(s) => s,
                    Err(e) => {
                        kept = None;
                        let why = e
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
                        let said = format!(
                            "plan shadow {n}: panicked ({}); the next derives every key",
                            why.unwrap_or_default()
                        );
                        Shadow::new(said.clone(), vec![said], 0, 0)
                    }
                };
                eprintln!("agent: {}", s.said);
                #[cfg(test)]
                let _ = say.send((n, s));
            }
        };
        match std::thread::Builder::new().name("plan shadow".into()).spawn(run) {
            Ok(_) => Some(Shadowing {
                asks,
                asked: 0,
                #[cfg(test)]
                said,
                #[cfg(test)]
                last: (0, None),
            }),
            Err(e) => {
                eprintln!("agent: the plan's shadow has no thread now ({e}); tried again with the next plan");
                None
            }
        }
    }
}

/// How many of the differing keys a shadow's lines say what changed in (`key_lines_apart`); the rest
/// are named.
const SAID_KEYS: usize = 5;

/// The plan of `ask` made again incrementally: the keys of `kept` (the shadow before's, taken) that
/// the diff since doesn't move kept, the rest derived (`KeyMemo::after`); compared with the plan's,
/// key by key and `Planned` whole. Its own keys left in `kept` for the next, but after a
/// difference: then none, and the next derives every key (the state rebuilt, as the plan's own).
/// Its line (`Shadow::said`), then a line for each key that differs: by step and target, its key
/// and the plan's, kept or derived, and for the first few the lines of what it reads that changed
/// since it was kept.
fn shadow(kept: &mut Option<Kept>, ask: Ask, skipped: u64) -> Shadow {
    use buildstore::reads::{Changes, Diff};
    let (t, cpu) = (Instant::now(), jobrt::sys::thread_cpu_ns());
    let reach = ask.reach.as_ref().map(|(_, r)| &**r);
    let rest = build::context_rest(&ask.covs.all, &ask.date, ask.reach.as_ref().map(|(c, _)| c.as_str()));
    let load = ask.tiles.load_state();
    let (at, sets) = kept.take().map(|k| (k.at, k.sets)).unzip();
    let d = at.as_ref().map_or_else(Diff::default, |at| Diff {
        manifest: Changes::of(&at.manifest, &ask.manifest),
        inputs: Changes::of(&at.inputs, &ask.inputs),
        load: at.load != load,
    });
    let all = at.as_ref().is_some_and(|at| at.rest != rest);
    let had = sets.as_ref().map(|s| s.keys.len());
    let (memo, let_go) = KeyMemo::after(sets, &d, all);
    let planned = build::plan_with(
        &memo,
        &ask.covs.all,
        &ask.date,
        &ask.manifest,
        &ask.done,
        &ask.inputs,
        reach,
        &ask.tiles,
        ask.publish(),
    );
    let (wall, cpu) = (t.elapsed(), Duration::from_nanos(jobrt::sys::thread_cpu_ns().saturating_sub(cpu)));
    // (Each key against the plan's: kept, or derived here.)
    let (mut kept_n, mut derived_n) = (0, 0);
    let mut apart: Vec<(String, String, Option<String>, Option<String>, bool)> = Vec::new();
    memo.each(|(step, t), k, was_kept| {
        if was_kept {
            kept_n += 1;
        } else {
            derived_n += 1;
        }
        match ask.keys.get(&(step.clone(), t.clone())) {
            Some(theirs) if theirs != k => apart.push((step.clone(), t.clone(), k.clone(), theirs.clone(), was_kept)),
            _ => {}
        }
    });
    apart.sort_unstable();
    // (The keys the diff let go that the plan has as they were: a read moved under a key not naming
    // it so, or a target's own coverage record put, which its key reads: buildstore::asked.)
    let changed: std::collections::HashSet<&str> = d.manifest.names.iter().map(|(l, _)| l.as_str()).collect();
    let (mut moved_same, mut own_record) = (Vec::new(), Vec::new());
    for ((step, t), was) in &let_go {
        if was.is_some() && ask.keys.get(&(step.clone(), t.clone())) == Some(was) {
            if changed.contains(buildstore::asked::target_logical(step, t).as_str()) {
                own_record.push(format!("{step} {t}"));
            } else {
                moved_same.push(format!("{step} {t}"));
            }
        }
    }
    moved_same.sort_unstable();
    own_record.sort_unstable();
    let plan_apart = (planned != ask.planned).then(|| first_difference(&planned, &ask.planned));
    let mut lines: Vec<String> = Vec::new();
    for (i, (step, t, ours, theirs, was_kept)) in apart.iter().enumerate() {
        let how = if *was_kept { "kept" } else { "derived" };
        let mut l = format!("plan shadow {}: {step} {t} {how} as {ours:?}, the plan's {theirs:?}", ask.n);
        if *was_kept && i < SAID_KEYS {
            if let Some(at) = &at {
                l.push_str(&key_lines_apart(step, t, at, &ask));
            }
        }
        lines.push(l);
    }
    if let Some(p) = &plan_apart {
        lines.push(format!("plan shadow {}: the plan {p}", ask.n));
    }
    let clean = lines.is_empty();
    let by_step = |v: &[String]| -> String {
        let mut n: BTreeMap<&str, usize> = BTreeMap::new();
        for k in v {
            *n.entry(k.split(' ').next().unwrap_or_default()).or_default() += 1;
        }
        let steps: Vec<String> = n.iter().map(|(s, c)| format!("{s} {c}")).collect();
        let named: Vec<&str> = v.iter().take(SAID_KEYS).map(String::as_str).collect();
        if v.is_empty() {
            "0".into()
        } else {
            format!("{} ({}: {})", v.len(), steps.join(", "), named.join(", "))
        }
    };
    let before = match (had, all) {
        (None, _) => "no shadow before it: every key derived".to_string(),
        (Some(h), true) => format!("the coverage, pass or reaches changed: none of the shadow before's {h} keys kept"),
        (Some(h), false) => format!(
            "of the shadow before's {h} keys, the diff ({} entries, {} inputs{}) kept {kept_n}",
            d.manifest.names.len(),
            d.inputs.names.len(),
            if d.load { ", the load" } else { "" },
        ),
    };
    let summary = if clean {
        "the same as the plan, key by key and whole".to_string()
    } else {
        let keys = apart.iter().filter(|a| a.4).count();
        format!(
            "DIFFERS from the plan: {keys} kept keys, {} derived{}",
            apart.len() - keys,
            if plan_apart.is_some() { ", the plan" } else { "" }
        )
    };
    let (n, plan_cpu, plan_wall) = (ask.n, ask.cpu, ask.wall);
    // (Kept for the next as the round read it: its manifest and inputs not copied again.)
    let Ask { manifest, inputs, .. } = ask;
    *kept = if clean {
        memo.read_sets().map(|sets| Kept { at: Context { manifest, inputs, rest, load }, sets })
    } else {
        None
    };
    let holds = match kept.as_ref() {
        Some(k) => {
            let size = k.sets.graph.size();
            format!(
                "it keeps {} keys and {} reads, {:.1} MB (the keys {:.1}, their reverse index {:.1}, at most {:.1} as it was made, their context {:.1})",
                k.sets.keys.len(),
                size.reads,
                (k.sets.bytes() + size.bytes + k.at.bytes()) as f64 / 1e6,
                k.sets.bytes() as f64 / 1e6,
                size.bytes as f64 / 1e6,
                size.peak as f64 / 1e6,
                k.at.bytes() as f64 / 1e6,
            )
        }
        None => "it keeps nothing: the next derives every key".to_string(),
    };
    let said = format!(
        concat!(
            "plan shadow {} (docs/incremental-planning.md step 4): {}; {}, {} derived, moved but the same {}, ",
            "and by their own coverage record put {}; {:.1} s CPU in {:.1} s against the plan's {:.1} s in {:.1} s{}; {}"
        ),
        n,
        summary,
        before,
        derived_n,
        by_step(&moved_same),
        by_step(&own_record),
        cpu.as_secs_f64(),
        wall.as_secs_f64(),
        plan_cpu.as_secs_f64(),
        plan_wall.as_secs_f64(),
        if skipped > 0 { format!(", {skipped} plans before it skipped") } else { String::new() },
        holds,
    );
    for l in &lines {
        eprintln!("agent: {l}");
    }
    Shadow::new(said, lines, kept_n, derived_n)
}

/// What target `t`'s key of `step` reads that changed since it was kept: its key lines now
/// against those of the context it was kept in (`at`'s manifest and inputs, the rest as now),
/// the lines of either side alone, a few of each.
fn key_lines_apart(step: &str, t: &str, at: &Context, ask: &Ask) -> String {
    let reach = ask.reach.as_ref().map(|(_, r)| &**r);
    let lines = |m, i| {
        let x = KeyCtx::new(&ask.covs.all, &ask.date, m, i, reach, &ask.tiles);
        super::steps::key_lines(step, t, &x).unwrap_or_default()
    };
    let (now, then) = (lines(&ask.manifest, &ask.inputs), lines(&at.manifest, &at.inputs));
    let (a, b): (std::collections::HashSet<&String>, std::collections::HashSet<&String>) =
        (now.iter().collect(), then.iter().collect());
    let only = |v: &[String], other: &std::collections::HashSet<&String>, sign: &str| -> (usize, Vec<String>) {
        let o: Vec<&String> = v.iter().filter(|l| !other.contains(l)).collect();
        (o.len(), o.iter().take(3).map(|l| format!("{sign} {l}")).collect())
    };
    let ((na, sa), (nb, sb)) = (only(&now, &b, "+"), only(&then, &a, "-"));
    format!(
        "; its key lines ({} now): {na} now alone, {nb} then alone{}",
        now.len(),
        sa.iter().chain(&sb).map(|l| format!("; {l}")).collect::<String>()
    )
}

/// Where plan `a` first differs from `b`: the field, and each around the first character apart.
fn first_difference(a: &Planned, b: &Planned) -> String {
    let Planned {
        work,
        ready,
        building,
        regions,
        drops,
        low,
        catalogue_held,
        catalogue_due,
        unknown,
        backfill,
        publishing,
        idle,
        left,
        held,
        checklist,
        built,
    } = a;
    let apart = |what: &str, a: String, b: String| -> String {
        let i =
            a.char_indices().zip(b.chars()).find(|((_, x), y)| x != y).map_or(a.len().min(b.len()), |((i, _), _)| i);
        let around = |s: &str| -> String {
            s.chars().skip(s[..i.min(s.len())].chars().count().saturating_sub(60)).take(160).collect()
        };
        format!("differs in its {what}: …{}… against the plan's …{}…", around(&a), around(&b))
    };
    macro_rules! field {
        ($($f:ident),*) => {
            $(if *$f != b.$f {
                return apart(stringify!($f), format!("{:?}", $f), format!("{:?}", b.$f));
            })*
        };
    }
    field!(
        work,
        ready,
        building,
        regions,
        drops,
        low,
        catalogue_held,
        catalogue_due,
        unknown,
        backfill,
        publishing,
        idle,
        left,
        held,
        checklist,
        built
    );
    "differs".to_string()
}

impl Held {
    /// The terrain packs' indexes `manifest` names, those not yet held read (from `pack-idx/`, else
    /// the NAS: half a minute for the whole build's, once).
    fn terrain_tiles(&self, root: &NasRoot, manifest: &BTreeMap<String, String>) -> std::cell::Ref<'_, TerrainTiles> {
        let t = std::time::Instant::now();
        let n = self.tiles.borrow_mut().load(root, manifest);
        if n > 0 {
            eprintln!(
                "agent: read the indexes of {n} terrain pack{} in {:.1} s",
                if n == 1 { "" } else { "s" },
                t.elapsed().as_secs_f64()
            );
        }
        self.tiles.borrow()
    }

    /// The pass's reaches (roads::reach), once they're made for the current version (Ok(None)
    /// until then: no z6 tile is planned, and the reach job comes first among the pass's worldwide
    /// jobs), or why they can't be read.
    fn current_reach(
        &self,
        root: &NasRoot,
        manifest: &BTreeMap<String, String>,
        keys: &Keys,
        date: &str,
    ) -> Result<Option<(String, Arc<Reaches>)>, roads::reach::LoadError> {
        let cx = build::PassCtx::default();
        if build::reach_work(&cx.ctx(date, manifest), keys).is_some() {
            return Ok(None);
        }
        let Some(c) = manifest.get(&roads::reach::logical(date)) else { return Ok(None) };
        // (Read again when the base packs' extents kept with them change too: codecs::extents.)
        let c = &format!("{c} {}", codecs::extents::stamp(root));
        let mut cached = self.reach.borrow_mut();
        if let Some(have) = cached.as_ref().filter(|(have, _)| have == c) {
            return Ok(Some(have.clone()));
        }
        let Some(r) = Reaches::load(root, manifest, date)? else { return Ok(None) };
        *cached = Some((c.clone(), Arc::new(r)));
        Ok(cached.clone())
    }

    /// `f` of the round's keying context, with the plan's keys.
    fn with_keys<T>(&self, r: &Round, f: impl FnOnce(&KeyCtx) -> T) -> T {
        let reach = self.reach_now.borrow().clone();
        let tiles = self.terrain_tiles(r.root, r.manifest);
        let memo = self.memo.borrow();
        let x = KeyCtx::new(&r.covs.all, r.date, r.manifest, r.inputs, reach.as_ref().map(|(_, r)| &**r), &tiles);
        match memo.as_ref() {
            Some(m) => f(&x.memo(m)),
            None => f(&x),
        }
    }

    /// The plan just made (`planned`, with `memo`, on `tiles`, in `made`: its wall time and its
    /// thread's CPU time) handed to its shadow, with a copy of its round.
    #[allow(clippy::too_many_arguments)]
    fn ask_shadow(
        &self,
        r: &Round,
        publish: Publish,
        tiles: &TerrainTiles,
        planned: &Planned,
        memo: &KeyMemo,
        made: (Duration, Duration),
    ) {
        let mut s = self.shadow.borrow_mut();
        if s.is_none() {
            *s = Shadowing::start();
        }
        let Some(sh) = s.as_mut() else { return };
        debug_assert!(std::ptr::eq(publish.each, &r.covs.each[..]), "the plan publishes by the coverages' regions");
        sh.asked += 1;
        let ask = Ask {
            n: sh.asked,
            covs: r.covs.clone(),
            date: r.date.to_string(),
            manifest: r.manifest.clone(),
            inputs: r.inputs.clone(),
            done: r.done.clone(),
            reach: self.reach_now.borrow().clone(),
            tiles: tiles.clone(),
            on_map: publish.on_map.clone(),
            served: publish.served.clone(),
            since_catalogue: publish.since_catalogue,
            held_for: publish.held_for,
            held: publish.held,
            way: publish.way,
            planned: planned.clone(),
            keys: memo.keys(),
            wall: made.0,
            cpu: made.1,
        };
        if sh.asks.send(ask).is_err() {
            eprintln!("agent: the plan's shadow's worker is gone; started again with the next plan");
            *s = None;
        }
    }
}

impl PlanHeld for Held {
    fn let_go(&self) {
        self.kept.borrow_mut().take();
        self.memo.borrow_mut().take();
    }
    fn has_kept(&self) -> bool {
        self.kept.borrow().is_some()
    }
    fn pass_work(
        &self,
        root: &NasRoot,
        date: Option<&str>,
        manifest: &BTreeMap<String, String>,
        keys: &Keys,
        ask: PassAsk,
        names: Option<&dyn Fn() -> NamesTodo>,
    ) -> PassWork {
        let planning = names.is_some();
        let reads = build::PassReads {
            release: date.and_then(|_| bld::buildtiles::release_of(root, manifest).ok()),
            lakefill: ask.lakefill,
            reach: &|| date.filter(|_| planning).and_then(|d| self.current_reach(root, manifest, keys, d).err()),
            failures: ask.failures,
        };
        build::pass_work(date, manifest, keys, reads, names)
    }
    fn begin_round(&self, root: &NasRoot, manifest: &BTreeMap<String, String>, done: &Keys, date: &str) {
        *self.reach_now.borrow_mut() = self.current_reach(root, manifest, done, date).ok().flatten();
    }
    fn plan(&self, r: &Round, publish: Publish, refilter: bool) -> (Planned, Option<agentcore::agent::gc::DepthKeep>) {
        let reach_now = self.reach_now.borrow().clone();
        let reach = reach_now.as_ref().map(|(_, r)| &**r);
        let cov = &r.covs.all;
        let reused = if refilter { self.kept.borrow_mut().take().zip(self.memo.borrow_mut().take()) } else { None };
        let reused = reused.filter(|(_, memo)| {
            memo.serves(&KeyCtx::new(cov, r.date, r.manifest, r.inputs, reach, &self.terrain_tiles(r.root, r.manifest)))
        });
        let depth =
            build::depth_kept(cov, r.manifest, &self.terrain_tiles(r.root, r.manifest), r.done, jobrt::jobs::now_s());
        let (planned, memo) = match reused {
            Some(k) => k,
            None => {
                let memo = KeyMemo::default();
                let tiles = self.terrain_tiles(r.root, r.manifest);
                nasread::PLAN.enter("build::plan");
                let (t, cpu) = (Instant::now(), jobrt::sys::thread_cpu_ns());
                let p = build::plan_with(&memo, cov, r.date, r.manifest, r.done, r.inputs, reach, &tiles, publish);
                if self.shadowing.get() {
                    let cpu = Duration::from_nanos(jobrt::sys::thread_cpu_ns().saturating_sub(cpu));
                    nasread::PLAN.enter("its shadow's copy");
                    self.ask_shadow(r, publish, &tiles, &p, &memo, (t.elapsed(), cpu));
                }
                drop(tiles);
                (p, memo)
            }
        };
        *self.kept.borrow_mut() = Some(planned.clone());
        *self.memo.borrow_mut() = Some(memo);
        (planned, depth)
    }
    #[cfg(test)]
    fn shadow(&self) -> Option<Shadow> {
        let mut s = self.shadow.borrow_mut();
        let s = s.as_mut()?;
        while s.last.0 < s.asked {
            match s.said.recv() {
                Ok((n, said)) => s.last = (n, Some(said)),
                Err(_) => break,
            }
        }
        s.last.1.clone()
    }
    fn heritage_waits(&self, r: &Round) -> Option<String> {
        self.with_keys(r, build::heritage_awaits_acceptance)
    }
    fn timetables_todo(&self, r: &Round) -> Option<Work> {
        self.with_keys(r, |x| build::timetables_todo_work(x, r.done))
    }
    fn chains_left(&self, r: &Round) -> [Vec<Work>; 5] {
        self.with_keys(r, |x| build::chains_left(x, r.done))
    }
    fn unknown_why(&self, manifest: &BTreeMap<String, String>, n: usize) -> String {
        match self.tiles.borrow().elevation(manifest) {
            Ok(None) => "the records don't say the build reads the declared elevation datasets yet (their switch, docs/inputs.md §6.8)".to_string(),
            Err(e) => e.0,
            Ok(Some(_)) => {
                let why = self.tiles.borrow().unread().next().map(|(c, e)| format!(": {c}, {e}")).unwrap_or_default();
                format!("the terrain's indexes can't be read now for {n} of them{why}")
            }
        }
    }
    fn restaled_by(
        &self,
        manifest: &BTreeMap<String, String>,
        served: &BTreeMap<String, String>,
        tiles: &[String],
    ) -> BTreeMap<String, Vec<String>> {
        build::restaled_by(manifest, self.reach_now.borrow().as_ref().map(|(_, r)| &**r), served, tiles)
    }
    fn region_counts(
        &self,
        root: &NasRoot,
        date: String,
        manifest: BTreeMap<String, String>,
        covs: Arc<agentcore::agent::Coverages>,
        keys: Keys,
    ) -> Box<dyn FnOnce() -> RegionCounts + Send> {
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten().map(|(_, r)| r);
        let tiles = self.terrain_tiles(root, &manifest).clone();
        let root = root.clone();
        Box::new(move || {
            let inputs = key_inputs(&root, &manifest);
            let x = KeyCtx::new(&covs.all, &date, &manifest, &inputs, reach.as_deref(), &tiles);
            build::region_steps(&x, &covs.each, &keys)
        })
    }
    #[cfg(test)]
    fn shadow_for_test(&self, on: bool) {
        self.shadowing.set(on);
    }
    #[cfg(test)]
    fn keep_for_test(&self, p: Planned) {
        *self.kept.borrow_mut() = Some(p);
        *self.memo.borrow_mut() = Some(KeyMemo::default());
    }
}
