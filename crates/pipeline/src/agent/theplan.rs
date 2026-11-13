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
    Along, Left, NamesTodo, PassAsk, PassWork, Plan, PlanHeld, Planned, Publish, RegionCounts, Round,
};
use agentcore::pool::records::Keys;
use roads::reach::Reaches;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
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
        roads::stage::MARGIN_KM
    }
    fn held(&self, index_dir: PathBuf) -> Box<dyn PlanHeld> {
        Box::new(Held {
            tiles: RefCell::new(TerrainTiles::new(Some(index_dir))),
            reach: RefCell::default(),
            reach_now: RefCell::default(),
            kept: RefCell::default(),
            memo: RefCell::default(),
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
    inputs.insert(
        inputs::dem::spacing::INPUT.into(),
        inputs::dem::spacing::digest_read(root, manifest).unwrap_or_else(|_| "?".into()),
    );
    inputs
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
    /// The reaches this round reads (`begin_round`).
    reach_now: RefCell<Option<Arc<Reaches>>>,
    /// The last `build::plan` result as it came: what "its jobs" filters again on a lease change,
    /// instead of planning (docs/incremental-planning.md step 2). Dropped as a plan starts.
    kept: RefCell<Option<Planned>>,
    /// The plan's keys, handed on to what derives keys of the same context this round, and kept with
    /// the plan. It's taken again only while its context reads the same (`KeyMemo::serves`): a pass
    /// newly on the NAS, or an index read that couldn't be before, and `build::plan` is made again.
    memo: RefCell<Option<KeyMemo>>,
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
    ) -> Result<Option<Arc<Reaches>>, roads::reach::LoadError> {
        let cx = build::PassCtx::default();
        if build::reach_work(&cx.ctx(date, manifest), keys).is_some() {
            return Ok(None);
        }
        let Some(c) = manifest.get(&roads::reach::logical(date)) else { return Ok(None) };
        // (Read again when the base packs' extents kept with them change too: codecs::extents.)
        let c = &format!("{c} {}", codecs::extents::stamp(root));
        let mut cached = self.reach.borrow_mut();
        if let Some((have, r)) = cached.as_ref() {
            if have == c {
                return Ok(Some(r.clone()));
            }
        }
        let Some(r) = Reaches::load(root, manifest, date)? else { return Ok(None) };
        let r = Arc::new(r);
        *cached = Some((c.clone(), r.clone()));
        Ok(Some(r))
    }

    /// `f` of the round's keying context, with the plan's keys.
    fn with_keys<T>(&self, r: &Round, f: impl FnOnce(&KeyCtx) -> T) -> T {
        let reach = self.reach_now.borrow().clone();
        let tiles = self.terrain_tiles(r.root, r.manifest);
        let memo = self.memo.borrow();
        let x = KeyCtx::new(r.cov, r.date, r.manifest, r.inputs, reach.as_deref(), &tiles);
        match memo.as_ref() {
            Some(m) => f(&x.memo(m)),
            None => f(&x),
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
        let reach = self.reach_now.borrow().clone();
        let reused = if refilter { self.kept.borrow_mut().take().zip(self.memo.borrow_mut().take()) } else { None };
        let reused = reused.filter(|(_, memo)| {
            memo.serves(&KeyCtx::new(
                r.cov,
                r.date,
                r.manifest,
                r.inputs,
                reach.as_deref(),
                &self.terrain_tiles(r.root, r.manifest),
            ))
        });
        let depth =
            build::depth_kept(r.cov, r.manifest, &self.terrain_tiles(r.root, r.manifest), r.done, jobrt::jobs::now_s());
        let (planned, memo) = match reused {
            Some(k) => k,
            None => {
                let memo = KeyMemo::default();
                let tiles = self.terrain_tiles(r.root, r.manifest);
                nasread::PLAN.enter("build::plan");
                let p = build::plan_with(
                    &memo,
                    r.cov,
                    r.date,
                    r.manifest,
                    r.done,
                    r.inputs,
                    reach.as_deref(),
                    &tiles,
                    publish,
                );
                drop(tiles);
                (p, memo)
            }
        };
        *self.kept.borrow_mut() = Some(planned.clone());
        *self.memo.borrow_mut() = Some(memo);
        (planned, depth)
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
        build::restaled_by(manifest, self.reach_now.borrow().as_deref(), served, tiles)
    }
    fn region_counts(
        &self,
        root: &NasRoot,
        date: String,
        manifest: BTreeMap<String, String>,
        covs: Arc<agentcore::agent::Coverages>,
        keys: Keys,
    ) -> Box<dyn FnOnce() -> RegionCounts + Send> {
        let reach = self.current_reach(root, &manifest, &keys, &date).ok().flatten();
        let tiles = self.terrain_tiles(root, &manifest).clone();
        let root = root.clone();
        Box::new(move || {
            let inputs = key_inputs(&root, &manifest);
            let x = KeyCtx::new(&covs.all, &date, &manifest, &inputs, reach.as_deref(), &tiles);
            build::region_steps(&x, &covs.each, &keys)
        })
    }
    #[cfg(test)]
    fn keep_for_test(&self, p: Planned) {
        *self.kept.borrow_mut() = Some(p);
        *self.memo.borrow_mut() = Some(KeyMemo::default());
    }
}
