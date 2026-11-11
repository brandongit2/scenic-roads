//! Removing what nothing holds from the NAS (docs/storage.md, GC; docs/maplog.md §7, step 8b): the
//! `gc` job, the lead's own, run whenever GC's queue has a chunk or a sweep's pass is due and the
//! NAS budget leaves it room (crate::agent::plan).
//!
//! - **By the queue** (`drain`, first each run): the content names GC's queue on the lead frees
//!   (crate::agent::gcqueue: no map a client holds, nor the records, name them, past its grace and
//!   every older lease), taken from the lead's coordinator `CHUNK_NAMES` at a time (`Lead::chunk`):
//!   the lead checks each against its holders as of then and marks it deleted before it answers,
//!   so the merge refuses a hand-off reusing one; the job removes them one by one over SMB, then
//!   says them gone (`Lead::gone`). Killed mid-chunk, the next run's first chunk is the same one,
//!   told again (a name not there counts as gone); yielding mid-chunk, it says what went and lets
//!   the rest go back to the queue, which the next chunk takes up first. Nothing is listed.
//! - **Sweeps, each a pass with its own cursor** (`Sweeps`, in the lead's map log folder, the
//!   folder each walk reached), run after the queue when due: the duties (`duties`: the names to
//!   drop, the archives, the old catalogues, the uploads' checks, once a day); the gate's checked
//!   copies (`inputs_walk`, by their own holders: the accepted indexes); and the orphan sweep
//!   (`orphan_walk`, §7.5), which lists the folders the map and the records name, the removed
//!   steps' and the retired passes', a folder at a time, and queues on the lead what's old enough
//!   that no job still holds it by its upload (`ORPHAN_AGE_S`), with the stray `.tmp` files and a
//!   retired pass's planet removed as it meets them.
//!
//! Deletions go through SMB: on this share they're permanent (no Recycle Bin entry; tested
//! 2026-10-02), and the M4 can't use SSH unattended (1Password asks each session). The newest
//! pass, a planet waiting for its pass, the rest of `sources/` (the packed tile stores,
//! `aws-terrarium/` and `pyramids/`, kept by their own indexes, buildstore::rawpack), the user's folders
//! (the inputs folders), state and the app are never touched. The lead not reached, nothing is
//! removed.

use super::seam::Plan;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};
use store::nasfs::NasPath;

/// The lead's coordinator's routes the `gc` job asks GC's queue by (crate::coord).
pub const CHUNK: &str = "/gc/chunk";
pub const GONE: &str = "/gc/gone";
pub const ORPHANS: &str = "/gc/orphans";
pub const REMOVING: &str = "/gc/removing";

/// How many names a chunk takes (the design's, docs/maplog.md §7.3). What bounds it: a chunk's
/// names are refused to every hand-off while they're being removed, and at the share's measured
/// deletion rates (`state/pool/nas-probes.json`: 8.4 a second at the median, 2.5 at the tenth
/// percentile) 200 are some 24 s of deletions, 80 s at the slow end; its two asks of the lead and
/// one `fsync` there are noise beside 200 SMB calls; and 200 names make a request of some 15 kB.
/// A yield doesn't wait for a chunk's end (the job looks at each name), so it bounds nothing there.
pub const CHUNK_NAMES: usize = 200;

/// How GC's queue on the lead is asked (`TheLead`; a test's queue in process).
pub trait Lead {
    /// The next chunk (`ask`), marked deleted on the lead before it answers; a dry run's, every
    /// name due, nothing marked.
    fn chunk(&self, ask: &ChunkAsk) -> Result<Chunk>;
    /// What a chunk removed (or found gone), and what it was told and didn't remove (it yielded).
    fn gone(&self, gone: &[String], released: &[String]) -> Result<()>;
    /// What the orphan sweep found: how many the lead queued.
    fn orphans(&self, names: &[String]) -> Result<usize>;
    /// A sweep's names about to be removed by its own rules (the gate's copies): those held now,
    /// to keep; the rest marked as a chunk is.
    fn removing(&self, names: &[String]) -> Result<Vec<String>>;
}

/// A chunk asked of the lead.
#[derive(Clone, Copy, Debug)]
pub struct ChunkAsk<'a> {
    pub max: usize,
    /// The newest complete OSM pass (`by_the_queue`).
    pub passes_before: Option<&'a str>,
    pub dry: bool,
}

/// The lead's answer: the chunk's names (those of a chunk told before first), and the top folders
/// the map and the records name (the orphan sweep's).
#[derive(Debug, Default, Deserialize)]
pub struct Chunk {
    pub names: Vec<String>,
    #[serde(default)]
    pub tops: BTreeSet<String>,
}

/// `Lead` by the lead's coordinator, reached as the NAS says (`state/coordinator.json`): the `gc`
/// job's. No coordinator, or one that doesn't answer (not the lead, an older app), and nothing is
/// removed.
pub struct TheLead(Result<crate::coord::client::Client, String>);

impl TheLead {
    pub fn from_nas(root: &store::nasfs::NasRoot) -> TheLead {
        TheLead(match crate::coord::client::Client::from_nas(root, &store::sys::host_name()) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => Err("no lead's coordinator (state/coordinator.json)".into()),
            Err(e) => Err(format!("{e:#}")),
        })
    }

    /// The coordinator `c`.
    pub fn at(c: crate::coord::client::Client) -> TheLead {
        TheLead(Ok(c))
    }

    fn ask(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let c = self.0.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        let (code, v) = c.post_json(path, body)?;
        anyhow::ensure!(code == 200, "the lead's coordinator answered {code} to {path}: {}", v["error"]);
        Ok(v)
    }
}

impl Lead for TheLead {
    fn chunk(&self, a: &ChunkAsk) -> Result<Chunk> {
        let v =
            self.ask(CHUNK, &serde_json::json!({ "max": a.max, "passes_before": a.passes_before, "dry": a.dry }))?;
        serde_json::from_value(v).context("the lead's chunk")
    }

    fn gone(&self, gone: &[String], released: &[String]) -> Result<()> {
        self.ask(GONE, &serde_json::json!({ "gone": gone, "released": released })).map(drop)
    }

    fn orphans(&self, names: &[String]) -> Result<usize> {
        let v = self.ask(ORPHANS, &serde_json::json!({ "names": names }))?;
        v["queued"].as_u64().map(|n| n as usize).context("the lead's answer names no count")
    }

    fn removing(&self, names: &[String]) -> Result<Vec<String>> {
        let v = self.ask(REMOVING, &serde_json::json!({ "names": names }))?;
        serde_json::from_value(v["held"].clone()).context("the lead's answer names no held names")
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub dry_run: bool,
    /// It yielded at a safe point (the build pausing, or a job held back for the NAS): what it did
    /// stands, the rest goes on at its next run.
    pub paused: bool,
    /// By the queue: the chunks taken, the files removed and those not there already. A dry run:
    /// the names due now (`due`).
    pub chunks: usize,
    pub removed: usize,
    pub gone_already: usize,
    pub due: usize,
    /// The sweeps run this time (their passes' names), and the folders they listed.
    pub swept: Vec<String>,
    pub folders: usize,
    /// The orphan sweep: content-named files old enough found, and of them those the lead queued
    /// (nothing held them).
    pub orphans_found: usize,
    pub orphans_queued: usize,
    pub tmp_removed: usize,
    /// A retired pass's other files (the planet download) removed.
    pub retired_removed: usize,
    /// The gate's checked copies removed, and those the lead held as they were told; why none were
    /// swept, when they weren't.
    pub inputs_removed: usize,
    pub inputs_held: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs_skipped: Option<String>,
    /// The names removed steps made (`RETIRED`) this run dropped from the manifest.
    pub retired_dropped: usize,
    /// The depth packs no current depth target names (`DepthKeep`) this run dropped from the
    /// manifest (a dry run: would drop).
    pub depth_dropped: usize,
    /// Unverified uploads the build doesn't read (`read_by`), dropped from the records' pending
    /// (`drop_names`; a dry run: would drop).
    pub pending_dropped: usize,
    /// Those it names checked on the NAS, there at their size (`verify_pending`; a dry run: would
    /// check), and those that weren't (left pending, named in the warnings).
    pub pending_verified: usize,
    pub pending_failed: usize,
    /// Days of the history packed into `state/coord/history-archive/<day>.tar` this run.
    pub history_days_archived: usize,
    /// Days of the forecasts' archive packed into `state/coord/forecasts-archive/<YYYY-MM>.tar`.
    pub forecast_days_archived: usize,
    /// Days of the forecast inputs' archive removed (crate::agent::fcinputs::KEEP_S), or counted.
    pub forecast_input_days_removed: usize,
    /// What the NAS kept of old catalogues removed this run (`OLD_CATALOGUES`; a dry run: there to
    /// remove), asked (`Asked::old_catalogues`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub old_catalogues: Vec<String>,
    /// What it passed over and why (the agent's status says them as the job's note).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Top folders no chunk or walk takes a file of, whatever the map names (`sources/` but its
/// retired passes, the removed steps' names and the gate's checked copies, each on its own rules).
const NEVER: [&str; 7] = ["translations", "descriptions", "inputs", "state", "app", "nas", "sources"];

/// The logical-name prefixes of what removed steps made (removed means deleted): GC's duties drop
/// every name the manifest has under one, in one save of the records (`drop_names`: a hand-off of
/// the `gc` job's, merged by the lead as any job's, its write-set these removals alone,
/// crate::agent::steps); the records then no longer hold their files, which GC's queue frees as it
/// does any (`by_the_queue` takes them under `sources/` too), and the orphan sweep lists their
/// folders for those named before the queue was. A step whose code is deleted adds its outputs'
/// prefix here; no step of the table may make a name under one. A prefix ends in `/` for a folder.
pub const RETIRED: [&str; 3] = [
    // (The worldwide z8 terrain and its maxima, `-v<n>` and `-v<n>-max`: its code deleted by task #670.)
    "sources/terrain-z8-",
    // (The old pipeline's data, converted once and read by nothing since; task #52.)
    "global/legacy/",
    // (The terrain-deep targets' lakes' levels, the terrain-lakes step's: its code deleted by task
    // #942, lakes keeping the source's heights.)
    "work/terrain-lakes/",
];

/// Whether logical name `l` is one a removed step made (`RETIRED`).
pub fn retired(l: &str) -> bool {
    RETIRED.iter().any(|p| l.starts_with(p))
}

/// Whether a chunk takes content name `n` when GC's queue frees it: one in a folder the map
/// indexes (outside `NEVER`), a removed step's (`RETIRED`), or a source of a pass older than the
/// newest complete one (`passes_before`; `sources/osm/<date>/`, `sources/items/<date>/`). The rest
/// of `sources/` is never GC's; the gate's copies (`sources/inputs/`) are `inputs_walk`'s, by the
/// indexes that hold them, which the queue doesn't see.
pub fn by_the_queue(n: &str, passes_before: Option<&str>) -> bool {
    let mut parts = n.split('/');
    let top = parts.next().unwrap_or_default();
    if !NEVER.contains(&top) || retired(n) {
        return true;
    }
    match (top, parts.next(), parts.next(), parts.next()) {
        ("sources", Some("osm" | "items"), Some(date), Some(_)) => {
            inputs::osmpass::is_date(date) && passes_before.is_some_and(|newest| date < newest)
        }
        _ => false,
    }
}

/// The depth's current targets as the agent's last plan found them (`build::depth_kept`: the z3 and
/// z6 tiles the coverage and the declared sources make targets, and those leased now), written to
/// the agent's home as the `gc` job is planned (`DEPTH_KEEP_FILE`) and passed by `--depth-keep`. A
/// depth pack is dropped only when no tile of these names it (`orphan`): an assembly's lo packs by
/// its z3 tile, a z6 tile's hi packs by it. None read (no file, one
/// not fresh, one unreadable), no depth name is dropped.
#[derive(Debug, Default, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct DepthKeep {
    /// When the plan found them (unix seconds).
    pub at: u64,
    /// The z3 tiles: the assemblies' and the floor's shading's targets.
    pub lo: BTreeSet<(u32, u32)>,
    /// The z6 tiles: the `depth` step's targets.
    pub hi: BTreeSet<(u32, u32)>,
}

/// The depth's steps, whose leased targets `DepthKeep` keeps (`with_leased`).
pub const DEPTH_STEPS: [&str; 2] = ["depth", "depth-lo"];

/// The file in the agent's home `DepthKeep` is written to.
pub const DEPTH_KEEP_FILE: &str = "gc-depth-keep.json";
/// How old a `DepthKeep` may be when GC reads it: an hour. The agent writes it at each plan while
/// GC is due, so a job started from that plan reads one minutes old; one older (a job restarted
/// after a sleep, a plan that stopped) drops no depth name that run.
pub const DEPTH_KEEP_FRESH_S: u64 = 3600;

impl DepthKeep {
    /// The one at `p` if fresh at `now`; why not otherwise (none there: Ok(None)).
    pub fn read(p: &std::path::Path, now: u64) -> Result<Option<DepthKeep>> {
        let b = match std::fs::read(p) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
        };
        let k: DepthKeep = serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?;
        anyhow::ensure!(
            now.saturating_sub(k.at) <= DEPTH_KEEP_FRESH_S && k.at <= now + 60,
            "the depth's targets in {} are from {} s ago, not within {DEPTH_KEEP_FRESH_S} s",
            p.display(),
            now as i64 - k.at as i64
        );
        Ok(Some(k))
    }

    /// With the targets of the depth's steps leased now (`leased`: step, target): a job running
    /// for a tile the coverage left since it was planned keeps its tile's packs this run.
    pub fn with_leased(mut self, leased: &[(String, String)]) -> DepthKeep {
        for (step, t) in leased {
            if let Some(u) = codecs::legacy::TileId::parse(t) {
                match (step.as_str(), u.z) {
                    ("depth", 6) => self.hi.insert((u.x, u.y)),
                    ("depth-lo", 3) => self.lo.insert((u.x, u.y)),
                    _ => false,
                };
            }
        }
        self
    }

    /// Whether logical name `l` is a depth pack (the depth's layers, `Plan::depth_layers`, a z6
    /// tile's hi or a z3 tile's lo) of a tile no current target names.
    pub fn orphan(&self, the_plan: &dyn Plan, l: &str) -> bool {
        let Some(r) = l.strip_prefix("layers/") else { return false };
        let Some((ly, r)) = r.split_once('/') else { return false };
        if !the_plan.depth_layers().contains(&ly) {
            return false;
        }
        let tile = |r: &str, z: &str| -> Option<(u32, u32)> {
            let (x, y) = r.strip_prefix(z)?.strip_prefix('-')?.split_once('-')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        };
        if let Some(t) = r.strip_prefix("hi/").and_then(|r| tile(r, "6")) {
            return !self.hi.contains(&t);
        }
        if let Some(t) = r.strip_prefix("lo/").and_then(|r| tile(r, "3")) {
            return !self.lo.contains(&t);
        }
        false
    }
}

/// Whether the clean-up drops logical name `l`: a removed step's (`RETIRED`), or a depth pack no
/// current target names (`depth`).
fn dropped(the_plan: &dyn Plan, l: &str, depth: Option<&DepthKeep>) -> bool {
    retired(l) || depth.is_some_and(|k| k.orphan(the_plan, l))
}

/// The content names the build reads by `manifest`: its own, and the files each accepted inputs'
/// index it names lists (inputs::inputs: a unit's checked copies, uploaded by its check, named by no
/// manifest entry of their own). An error when such an index can't be read now.
fn read_by(root: &store::nasfs::NasRoot, manifest: &BTreeMap<String, String>) -> Result<BTreeSet<String>> {
    let mut named: BTreeSet<String> = manifest.values().cloned().collect();
    for (l, c) in manifest {
        if inputs::inputs::record_of(l).is_some_and(|(_, r)| r == "@index") {
            named.extend(
                inputs::inputs::read_index(root, c)?
                    .files
                    .values()
                    .flat_map(|f| f.stored().into_iter().map(str::to_string)),
            );
        }
    }
    Ok(named)
}

/// What `drop_names` dropped (a dry run: would drop).
#[derive(Debug, Default, PartialEq)]
pub struct Dropped {
    /// Names removed steps made (`RETIRED`), and depth packs no current target names.
    pub retired: usize,
    pub depth: usize,
    /// Unverified uploads the build doesn't read (`read_by`, once this run's names are dropped): replaced
    /// or dropped since, never checked; their files then kept only as any unnamed file is.
    pub unpended: usize,
}

/// Drops from the build's manifest at `root` the names removed steps made (`RETIRED`) and the depth
/// packs no current target names (`depth`), and from its unverified uploads every one the build
/// then doesn't read (`read_by`: the manifest's names and its inputs' indexes' files), as the `gc` step's job (`Out`, its scratch `scratch`): in the pool a hand-off
/// the lead merges (its `checked`), else saved under the build lock. None of either, nothing
/// written, so a run after is a no-op. A dry run counts them, writing nothing.
pub fn drop_names(
    the_plan: &dyn Plan,
    root: &store::nasfs::NasRoot,
    scratch: &std::path::Path,
    depth: Option<&DepthKeep>,
    dry_run: bool,
) -> Result<Dropped> {
    let crate::pool::records::Records { pending, manifest, .. } = crate::pool::records::Records::now(root)?;
    let names: Vec<&String> = manifest.keys().filter(|l| dropped(the_plan, l, depth)).collect();
    let kept: BTreeMap<String, String> =
        manifest.iter().filter(|(l, _)| !dropped(the_plan, l, depth)).map(|(l, c)| (l.clone(), c.clone())).collect();
    let named = read_by(root, &kept)?;
    let unpend: Vec<&String> = pending.keys().filter(|c| !named.contains(*c)).collect();
    let retired = names.iter().filter(|l| retired(l)).count();
    let r = Dropped { retired, depth: names.len() - retired, unpended: unpend.len() };
    if dry_run || (names.is_empty() && unpend.is_empty()) {
        return Ok(r);
    }
    let mut out = buildstore::out::Out::open(root, scratch, STEP, the_plan.writes())?;
    for l in names {
        out.remove(l);
    }
    for c in unpend {
        out.unpend(c);
    }
    out.save()?;
    Ok(r)
}

/// What `verify_pending` checked (a dry run: would check).
#[derive(Debug, Default, PartialEq)]
pub struct Verified {
    /// Uploads the build reads (`read_by`), there at their size and out of the pending.
    pub ok: usize,
    /// Those of another size, or not there: left pending, nothing removed (their targets are done
    /// under their keys, so only `scenic redo` of them replaces them).
    pub bad: Vec<String>,
    /// Why the run ended before the last batch: a call that stalled past its deadline and was
    /// stopped (the batches before it are saved; the next duties' pass takes up the rest).
    pub stalled: Option<String>,
}

/// How many uploads one call on the NAS checks at most (buildstore::out::verify; a safe point between
/// calls): each a stat and a block read there, so a call takes seconds. Its command line names
/// each, so a batch also ends before its quoted names pass `out::NAS_NAMES_MAX` (`verify_batches`).
const VERIFY_CHUNK: usize = 500;

/// `todo` in batches of at most `VERIFY_CHUNK` names, each within `buildstore::out::NAS_NAMES_MAX`
/// bytes quoted and joined (one name longer than that alone in its batch, which `verify` refuses).
fn verify_batches(todo: &[(String, Option<u64>)]) -> Vec<&[(String, Option<u64>)]> {
    let mut batches = Vec::new();
    let (mut start, mut len) = (0, 0);
    for (i, (c, _)) in todo.iter().enumerate() {
        // (Its quoted name and the space before the next.)
        let n = buildstore::out::sh_quoted(c).len() + 1;
        if i > start && (i - start == VERIFY_CHUNK || len + n > buildstore::out::NAS_NAMES_MAX + 1) {
            batches.push(&todo[start..i]);
            (start, len) = (i, 0);
        }
        len += n;
    }
    if start < todo.len() {
        batches.push(&todo[start..]);
    }
    batches
}

/// Checks the records' unverified uploads the build reads (`read_by`) on the NAS itself, there at
/// the records' size (buildstore::out::verify: `ssh`, `nas_root` the project folder as the NAS sees it),
/// as the `gc` step's job (`scratch` its `Out`'s): those that are leave the pending (in the pool
/// through its hand-off's `checked`); those that aren't stay, said. Each batch (`VERIFY_CHUNK`) saved
/// as it ends, then a safe point (`yield_point`). A dry run counts them, checking nothing.
pub fn verify_pending(
    the_plan: &dyn Plan,
    root: &store::nasfs::NasRoot,
    scratch: &std::path::Path,
    ssh: &[&str],
    nas_root: &str,
    dry_run: bool,
) -> Result<Verified> {
    let crate::pool::records::Records { pending, manifest, sizes, .. } = crate::pool::records::Records::now(root)?;
    let named = read_by(root, &manifest)?;
    // (Each with its size by the records: none for a file named before they kept sizes, 17,166 of
    // the pending on 7 Nov, checked as there.)
    let todo: Vec<(String, Option<u64>)> =
        pending.keys().filter(|c| named.contains(*c)).map(|c| (c.clone(), sizes.get(c).copied())).collect();
    let mut v = Verified::default();
    if dry_run {
        v.ok = todo.len();
        return Ok(v);
    }
    if todo.is_empty() {
        return Ok(v);
    }
    let mut out = buildstore::out::Out::open(root, scratch, STEP, the_plan.writes())?;
    for chunk in verify_batches(&todo) {
        let batch: BTreeMap<String, Option<u64>> = chunk.iter().cloned().collect();
        let (ok, bad) = match buildstore::out::verify(ssh, nas_root, &batch) {
            Ok(r) => r,
            // (Stopped past its deadline: what the batches before it checked is saved already; the
            // rest stays pending for the next run.)
            Err(e)
                if e.chain().any(|c| {
                    c.downcast_ref::<std::io::Error>().is_some_and(|i| i.kind() == std::io::ErrorKind::TimedOut)
                }) =>
            {
                v.stalled = Some(format!("{e:#}"));
                break;
            }
            Err(e) => return Err(e),
        };
        // (The NAS looked each up: counted as stats, in its cost line and its live counts, which
        // its weight goes by, jobrt::budget::nas.)
        meta(0, batch.len() as u64, 0);
        store::nasio::add(store::nasio::Count::Stats, batch.len() as u64);
        v.ok += ok.len();
        for c in &ok {
            out.unpend(c);
        }
        v.bad.extend(bad);
        // (Each batch's saved before the next: in the pool a save of the job's hand-off, kept
        // whatever way the job ends, so a run stopped, paused or failed loses none of it.)
        out.save()?;
        yield_point();
    }
    Ok(v)
}

/// The command line `verify_pending` reaches the NAS by: `ssh` in batch mode (no prompts), through
/// the job's proxy when it has one (jobrt::budget::SANDBOX, as inputs::osmpass's), its host last.
fn nas_ssh() -> Vec<String> {
    let mut a: Vec<String> = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=20"].map(String::from).into();
    if let Some(p) = std::env::var(jobrt::budget::PROXY_ENV).ok().and_then(|u| buildstore::nasssh::proxy_addr(&u)) {
        a.extend(["-o".into(), format!("ProxyCommand=/usr/bin/nc -X connect -x {p} %h %p")]);
    }
    a.push(store::nas::SSH.into());
    a
}

/// What GC keeps of `sources/inputs/` (docs/inputs.md §4.9): what the records name (`manifest`, the
/// newest records' brief's, today's file before the pool: the units' accepted indexes, listings and
/// held reports), the files each such index lists, the files listed by every index whose time is
/// within `within` of `now` (one made lately, or replaced lately: the check touches the index it
/// replaces), and the index each check's state made within `within` names as replaced, with its
/// files (whatever that index's own time: a touch that failed loses nothing). So a version replaced
/// stays restorable for `within` and a step reading the version it planned with finds its files.
/// Units' names may be nested (`timetables/gtfs`): every folder under `sources/inputs/` is walked.
/// An error when any of them can't be read now: nothing there is swept then. (A hand-off not merged
/// yet isn't read: the lead's queue holds what a merged one brings, and the merge refuses one
/// bringing a name GC removed, its job's work done again: crate::agent::gcqueue.)
pub fn inputs_kept(
    root: &store::nasfs::NasRoot,
    manifest: &std::collections::BTreeMap<String, String>,
    within: Duration,
    now: SystemTime,
) -> Result<BTreeSet<String>> {
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let ours = |(l, c): &(String, String)| c.starts_with("sources/inputs/").then(|| (l.clone(), c.clone()));
    // The records' manifest (read once for the whole sweep).
    let named: Vec<(String, String)> =
        manifest.iter().map(|(l, c)| (l.clone(), c.clone())).filter_map(|p| ours(&p)).collect();
    let index_files = |c: &str, keep: &mut BTreeSet<String>| -> Result<()> {
        keep.extend(
            inputs::inputs::read_index(root, c)?
                .files
                .values()
                .flat_map(|f| f.stored().into_iter().map(str::to_string))
                .collect::<Vec<_>>(),
        );
        keep.insert(c.to_string());
        Ok(())
    };
    for (l, c) in named {
        if inputs::inputs::record_of(&l).is_some_and(|(_, r)| r == "@index") {
            index_files(&c, &mut keep)?;
        }
        keep.insert(c);
    }
    // Every unit's records made within `within` (by walking every folder: units' names nest): an
    // index's files, and a check's state's replaced index with its files.
    let mut stack = vec![root.join("sources/inputs")];
    while let Some(dir) = stack.pop() {
        let rd = match dir.read_dir() {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
        };
        for e in rd {
            meta(1, 0, 0);
            if e.file_type()?.is_dir() {
                stack.push(e.path);
                continue;
            }
            let rel = e.path.rel().to_string();
            let Some(record) = store::naming::parse_content_name(&rel)
                .filter(|n| n.ext == "json")
                .and_then(|n| inputs::inputs::record_of(n.logical))
                .map(|(_, r)| r)
            else {
                continue;
            };
            meta(0, 1, 0);
            let young = e
                .symlink_metadata()?
                .modified()
                .ok()
                .is_some_and(|t| now.duration_since(t).map_or(true, |d| d <= within));
            if !young {
                continue;
            }
            match record {
                "@index" => index_files(&rel, &mut keep)?,
                "@listed" => {
                    if let Some(old) = inputs::inputs::read_listed_file(root, &rel)?.replaced {
                        index_files(&old, &mut keep)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(keep)
}

/// The clean-up's step and target, as its costs file names them (`note_cost`; crate::coord::cost_key:
/// `gc gc`, as the lead offers its own job of no targets, the id's rest its target).
pub const STEP: &str = "gc";

/// When this process's run began (`run`), for its cost line's time.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// The way this run's step runs now (crate::agent::seam::Plan::cost_version), as the program said
/// (`set_way`), for its cost's line.
static WAY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Says `WAY`, before a run.
pub fn set_way(v: u32) {
    WAY.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Notes what this run cost so far in the job's costs file (`SCENIC_COSTS`, as scenic-build's steps
/// do): its NAS work (buildstore::costin: bytes read, and the folder entries listed, files statted and
/// deleted, `meta`) over its time, its memory: the clean-up's NAS weight (jobrt::budget::nas) as the
/// coordinator weighs it from then on, measured, never guessed (docs/pool.md §7.7, task #451).
pub fn note_cost() {
    crate::agent::jobs::note_cost(
        WAY.load(std::sync::atomic::Ordering::Relaxed),
        STEP,
        STEP,
        STARTED.get().map_or(0, |t| t.elapsed().as_secs()),
    );
}

/// A safe point between folders: asked to stop (the build pausing, or the agent yielding the NAS
/// to jobs long held back for it, docs/pool.md §7.7), it notes its cost and ends as paused; what it
/// removed is gone for good and the rest waits for its next run (each removal stands alone).
fn yield_point() {
    #[cfg(test)]
    YIELD_CHECKS.set(YIELD_CHECKS.get() + 1);
    jobrt::control::reached();
    if jobrt::control::draining() {
        note_cost();
        jobrt::control::stop_paused(STEP);
    }
}

#[cfg(test)]
thread_local! {
    /// The safe points a run on this thread passed (`yield_point`).
    static YIELD_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// Whether this run notes its NAS work (`SCENIC_COSTS` set as `run` began: a job's). Its walks
    /// run on the thread that called `run`.
    static COUNTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Counts NAS metadata work (buildstore::costin::meta) while this process notes it (`COUNTING`).
fn meta(listed: u64, stats: u64, deletes: u64) {
    if COUNTING.get() {
        buildstore::costin::meta(listed, stats, deletes);
    }
}

/// How old an unheld content-named file must be before the orphan sweep queues it (docs/maplog.md
/// §7.5): no job runs that long between its upload and its hand-off.
pub const ORPHAN_AGE_S: u64 = 14 * 86400;
/// How old a `.tmp` file must be before a walk removes it: a write that never finished (no write
/// takes that long).
pub const TMP_AGE_S: u64 = 2 * 86400;
/// How long the gate's checked copies of a version replaced stay restorable (docs/inputs.md
/// §4.9: an index made or replaced since, or a check's state of since naming it replaced, keeps
/// its files), and the age a copy nothing keeps must have.
pub const INPUTS_KEEP_S: u64 = 14 * 86400;
/// How long a retired pass's other files (the planet download) stay once the newer pass is
/// complete: a fresh pass can still be compared with the one before.
pub const PLANET_KEEP_S: u64 = 14 * 86400;
/// How often the duties' pass begins (`duties`): the history packs a day at a time, and the
/// uploads' checks and the names to drop keep up with a day's jobs.
pub const DUTIES_EVERY_S: u64 = 86400;

/// The sweeps in the order a run takes them, and how long after one pass ended the next begins:
/// the walks list folders of the whole share, which GC's queue spares it doing daily, so a pass
/// begins once an orphan of the pass before has had its age (an orphan goes within twice
/// `ORPHAN_AGE_S`; a replaced copy of the gate's within twice `INPUTS_KEEP_S`).
pub const SWEEPS: [(&str, u64); 3] = [("duties", DUTIES_EVERY_S), ("copies", INPUTS_KEEP_S), ("orphans", ORPHAN_AGE_S)];

/// The file in the lead's map log folder its sweeps' passes are kept in (`Sweeps`).
pub const SWEEPS_FILE: &str = "gc-sweeps.json";

/// Each sweep's pass (`SWEEPS`): when it began, when it ended, and the folder a walk reached, so a
/// run stopped (killed, yielding) goes on after it without listing again what it had.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sweeps {
    #[serde(default)]
    pub each: BTreeMap<String, Pass>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pass {
    pub begun: u64,
    /// None: under way.
    #[serde(default)]
    pub ended: Option<u64>,
    /// The last folder its walk finished (a path from the project folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached: Option<String>,
}

impl Sweeps {
    /// The passes kept at `p`: none there (or unreadable, said), none begun.
    pub fn read(p: &std::path::Path) -> Sweeps {
        match std::fs::read(p) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                eprintln!("gc: {} doesn't read ({e}): every sweep's pass begins again", p.display());
                Sweeps::default()
            }),
            Err(_) => Sweeps::default(),
        }
    }

    fn write(&self, p: &std::path::Path) -> Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        store::whole::write(p, &serde_json::to_vec(self)?).with_context(|| format!("write {}", p.display()))
    }

    /// Whether sweep `name`'s pass is under way, or its next is due at `now`.
    pub fn due(&self, name: &str, now: u64) -> bool {
        let every = SWEEPS.iter().find(|(n, _)| *n == name).map_or(0, |s| s.1);
        self.each.get(name).is_none_or(|p| p.ended.is_none_or(|e| now.saturating_sub(e) >= every))
    }

    /// Whether any is.
    pub fn any_due(&self, now: u64) -> bool {
        SWEEPS.iter().any(|(n, _)| self.due(n, now))
    }
}

/// What a run is asked (the `gc` job's flags, `scenic gc`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Asked<'a> {
    /// Says what it would do, and does none of it.
    pub dry_run: bool,
    /// The depth's current targets (`DepthKeep`; None: no depth name dropped).
    pub depth: Option<&'a DepthKeep>,
    /// Where its sweeps' passes are kept (`SWEEPS_FILE`, the agent's): each sweep runs when due and
    /// goes on where it reached. None (a run by hand): every sweep runs whole, nothing kept.
    pub sweeps: Option<&'a std::path::Path>,
    /// What the NAS kept of old catalogues removed (`remove_old_catalogues`): the lead asks it once
    /// every member's app checks the NAS by `map/` (crate::pool::beat::Beat::checks_map).
    pub old_catalogues: bool,
}

/// What a run acts through: the plan (its write sets, the depth's layers), the lead, its safe
/// points, a file's removal.
pub struct Ways<'a> {
    pub the_plan: &'a dyn Plan,
    pub lead: &'a dyn Lead,
    /// Whether to stop at this safe point (a job's: the agent asking it to, jobrt::control).
    pub yielding: &'a dyn Fn() -> bool,
    /// Removes a file from the NAS (one SMB call; a test's to fail or stop).
    pub remove: &'a dyn Fn(&NasPath) -> std::io::Result<()>,
}

/// A job's safe point between two removals or two folders: whether the agent asks it to stop (the
/// build pausing, or a job held back long for the NAS it holds: docs/pool.md §7.7). Read at most
/// once a second.
pub fn job_yielding() -> bool {
    jobrt::control::reached();
    jobrt::control::draining_lately()
}

/// A file's removal over SMB, counted as a delete (`meta`).
pub fn unlink(p: &NasPath) -> std::io::Result<()> {
    meta(0, 0, 1);
    p.remove_file()
}

/// Whether a run went on to its end or stopped at a safe point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Done,
    Paused,
}

/// A run: the queue's chunks (`drain`), then each sweep whose pass is due (`SWEEPS`), its NAS
/// work counted from its start and noted as it ends (`note_cost`) in a job (`SCENIC_COSTS` set).
/// Stopped at a safe point, it says so (`Report::paused`), what it did kept.
pub fn run(root: &store::nasfs::NasRoot, asked: &Asked, ways: &Ways) -> Result<Report> {
    STARTED.get_or_init(std::time::Instant::now);
    if std::env::var_os("SCENIC_COSTS").is_some() {
        COUNTING.set(true);
        buildstore::costin::begin();
    }
    let r = run_counted(root, asked, ways);
    note_cost();
    r
}

fn run_counted(root: &store::nasfs::NasRoot, asked: &Asked, ways: &Ways) -> Result<Report> {
    let dry_run = asked.dry_run;
    let mut rep = Report { dry_run, ..Default::default() };
    let latest = inputs::osmpass::latest_pass(root);
    crate::agent::jobs::stage(0, 2, "steps (removing what GC's queue frees)");
    let (flow, tops) = {
        let _p = jobrt::timings::phase("removing what GC's queue frees");
        drain(root, ways, dry_run, latest.as_deref(), &mut rep)?
    };
    if flow == Flow::Paused {
        rep.paused = true;
        return Ok(rep);
    }
    crate::agent::jobs::stage(1, 2, "steps (sweeping)");
    let now = crate::agent::jobs::now_s();
    let mut sw = asked.sweeps.map(Sweeps::read).unwrap_or_default();
    for (name, _) in SWEEPS {
        if asked.sweeps.is_some() && !sw.due(name, now) {
            continue;
        }
        let pass = sw.each.entry(name.to_string()).or_default();
        if pass.ended.is_some() || pass.begun == 0 {
            *pass = Pass { begun: now, ..Default::default() };
        }
        rep.swept.push(name.to_string());
        let reached = pass.reached.clone();
        let save = |sw: &Sweeps| asked.sweeps.filter(|_| !dry_run).map_or(Ok(()), |p| sw.write(p));
        save(&sw)?;
        let mut at = |rel: &str| -> Result<()> {
            sw.each.entry(name.to_string()).or_default().reached = Some(rel.to_string());
            save(&sw)
        };
        let flow = match name {
            "duties" => duties(root, asked, ways.the_plan, &mut rep),
            "copies" => inputs_walk(root, ways, dry_run, reached.as_deref(), &mut at, &mut rep),
            _ => orphan_walk(root, ways, dry_run, &tops, latest.as_deref(), reached.as_deref(), &mut at, &mut rep),
        }?;
        if flow == Flow::Paused {
            rep.paused = true;
            return Ok(rep);
        }
        let pass = sw.each.entry(name.to_string()).or_default();
        (pass.ended, pass.reached) = (Some(crate::agent::jobs::now_s()), None);
        save(&sw)?;
    }
    Ok(rep)
}

/// The queue's chunks, one after another until the lead has none (a dry run: the names due now,
/// counted), each removed as it was told (`remove_told`). The orphan sweep's top folders, as the
/// lead's last answer named them.
fn drain(
    root: &store::nasfs::NasRoot,
    ways: &Ways,
    dry_run: bool,
    passes_before: Option<&str>,
    rep: &mut Report,
) -> Result<(Flow, BTreeSet<String>)> {
    let ask = ChunkAsk { max: if dry_run { usize::MAX } else { CHUNK_NAMES }, passes_before, dry: dry_run };
    loop {
        let c = ways.lead.chunk(&ask).context("nothing removed: GC's queue on the lead not asked")?;
        if dry_run {
            rep.due = c.names.len();
            return Ok((Flow::Done, c.tops));
        }
        if c.names.is_empty() {
            return Ok((Flow::Done, c.tops));
        }
        rep.chunks += 1;
        let (mut removed, mut already) = (0, 0);
        let flow = remove_told(root, &c.names, ways, &mut removed, &mut already);
        (rep.removed, rep.gone_already) = (rep.removed + removed, rep.gone_already + already);
        if flow? == Flow::Paused {
            return Ok((Flow::Paused, c.tops));
        }
    }
}

/// Names the lead has marked deleted (a chunk, or a sweep's told), removed one by one in their
/// order, a safe point before each; one not there is gone already (while the share answers:
/// `share_answers`, else it fails as a removal does). Then said gone to the lead. At
/// a safe point asked to stop: what went said gone and the rest let go (back to the queue,
/// unmarked). A removal that fails stops it: what went said, the rest let go but that one, whose
/// outcome isn't known (the next chunk takes it again), and the run fails.
fn remove_told(
    root: &store::nasfs::NasRoot,
    names: &[String],
    ways: &Ways,
    removed: &mut usize,
    already: &mut usize,
) -> Result<Flow> {
    let mut gone = Vec::new();
    for (i, n) in names.iter().enumerate() {
        if (ways.yielding)() {
            ways.lead.gone(&gone, &names[i..]).context("what went not said to GC's queue on the lead")?;
            return Ok(Flow::Paused);
        }
        match (ways.remove)(&root.join(n)) {
            Ok(()) => *removed += 1,
            // (Gone already only while the share answers: a mount lost or stale finds no file, and
            // what it didn't remove would leave the queue, kept for ever.)
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && share_answers(root) => *already += 1,
            Err(e) => {
                let e = if e.kind() == std::io::ErrorKind::NotFound {
                    std::io::Error::other(format!(
                        "not there, and the share doesn't answer (no {}/)",
                        crate::agent::NAS_CHECK
                    ))
                } else {
                    e
                };
                if let Err(g) = ways.lead.gone(&gone, &names[i + 1..]) {
                    eprintln!(
                        "gc: what went not said to GC's queue on the lead: {g:#} (the next chunk takes it again)"
                    );
                }
                return Err(e).with_context(|| format!("remove {n}"));
            }
        }
        gone.push(n.clone());
    }
    ways.lead
        .gone(&gone, &[])
        .context("what went not said to GC's queue on the lead (the next chunk takes it again)")?;
    Ok(Flow::Done)
}

/// Whether the share answers (its `NAS_CHECK` folder there): a file not found counts as gone only
/// then.
fn share_answers(root: &store::nasfs::NasRoot) -> bool {
    meta(0, 1, 0);
    root.join(crate::agent::NAS_CHECK).metadata().is_ok_and(|m| m.is_dir())
}

/// A folder a walk starts at: listed whole, or only its own files whose path starts with `only`
/// (a removed step's prefix in `sources/`); `pass`: a retired pass's (its other files removed once
/// the newer pass has settled).
#[derive(Clone, Debug, PartialEq)]
struct Root {
    dir: String,
    only: Option<&'static str>,
    pass: bool,
}

/// Where a folder stands against the folder a walk reached (`reached`), in the walk's order (each
/// folder before what's in it, names in order): done before it, it or one it's in (listed again
/// for what's in it, its own files done), or not yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    Done,
    Above,
    New,
}

fn seen(rel: &str, reached: Option<&str>) -> Seen {
    let Some(r) = reached else { return Seen::New };
    let parts = |p: &str| p.split('/').filter(|c| !c.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (f, r) = (parts(rel), parts(r));
    if r.starts_with(&f) {
        Seen::Above
    } else if f < r {
        Seen::Done
    } else {
        Seen::New
    }
}

/// The folders under `roots` in the walk's order, after `reached`: each listed once and handed to
/// `visit` with its entries (the folders it holds walked after it, by name; never through a link),
/// which notes it reached and may stop the walk.
fn walk(
    root: &store::nasfs::NasRoot,
    roots: &[Root],
    reached: Option<&str>,
    visit: &mut dyn FnMut(&Root, &NasPath, Vec<store::nasfs::NasEntry>) -> Result<Flow>,
) -> Result<Flow> {
    let mut roots = roots.to_vec();
    roots.sort_by(|a, b| seen_key(&a.dir).cmp(&seen_key(&b.dir)));
    for r in &roots {
        let mut stack = vec![root.join(&r.dir)];
        while let Some(dir) = stack.pop() {
            let s = seen(dir.rel(), reached);
            if s == Seen::Done || (s == Seen::Above && r.only.is_some()) {
                continue;
            }
            let Ok(entries) = dir.read_dir() else { continue };
            if s == Seen::Above {
                meta(entries.len() as u64, 0, 0);
            }
            if r.only.is_none() {
                let mut subs: Vec<NasPath> = entries
                    .iter()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
                    .map(|e| e.path.clone())
                    .collect();
                subs.sort_by(|a, b| b.rel().cmp(a.rel()));
                stack.extend(subs);
            }
            if s == Seen::New && visit(r, &dir, entries)? == Flow::Paused {
                return Ok(Flow::Paused);
            }
        }
    }
    Ok(Flow::Done)
}

fn seen_key(p: &str) -> Vec<String> {
    p.split('/').filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// The orphan sweep (docs/maplog.md §7.5): the folders the map and the records name (`tops`, the
/// lead's), the removed steps' (`RETIRED`) and the retired passes' (older than `latest`), a folder
/// at a time from the one it reached: each content-named file at least `ORPHAN_AGE_S` old sent to
/// the lead, which queues those nothing holds (GC's queue frees them as any); each `.tmp` file
/// `TMP_AGE_S` old removed; a retired pass's other files removed once the newer pass has been
/// complete `PLANET_KEEP_S`, and its folders left empty. A dry run counts, sending and removing
/// nothing.
#[allow(clippy::too_many_arguments)]
fn orphan_walk(
    root: &store::nasfs::NasRoot,
    ways: &Ways,
    dry_run: bool,
    tops: &BTreeSet<String>,
    latest: Option<&str>,
    reached: Option<&str>,
    at: &mut dyn FnMut(&str) -> Result<()>,
    rep: &mut Report,
) -> Result<Flow> {
    let _p = jobrt::timings::phase("sweeping for orphans");
    let mut roots: Vec<Root> = tops
        .iter()
        .filter(|t| !NEVER.contains(&t.as_str()))
        .map(|t| Root { dir: t.clone(), only: None, pass: false })
        .collect();
    for p in RETIRED {
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
        let top = dir.split('/').next().unwrap_or_default();
        if NEVER.contains(&top) {
            roots.push(Root { dir: dir.into(), only: Some(p), pass: false });
        } else if !tops.contains(top) {
            roots.push(Root { dir: dir.into(), only: None, pass: false });
        }
    }
    // (A retired pass's planet goes once the newest pass completed `PLANET_KEEP_S` ago: its
    // summary's time.)
    let mut planet_due = false;
    if let Some(latest) = latest {
        meta(1, 0, 0);
        planet_due = root
            .join("sources/osm")
            .join(latest)
            .read_dir()
            .ok()
            .and_then(|rd| rd.into_iter().find(|e| e.name.starts_with("pass.") && e.name.ends_with(".json")))
            .and_then(|e| e.symlink_metadata().ok()?.modified().ok())
            .is_some_and(|t| age_s(t) > PLANET_KEEP_S);
        for kind in ["osm", "items"] {
            meta(1, 0, 0);
            let Ok(rd) = root.join("sources").join(kind).read_dir() else { continue };
            for e in rd.into_iter().filter(|e| inputs::osmpass::is_date(&e.name) && e.name.as_str() < latest) {
                roots.push(Root { dir: format!("sources/{kind}/{}", e.name), only: None, pass: true });
            }
        }
    }
    walk(root, &roots, reached, &mut |r, dir, entries| {
        let mut found = Vec::new();
        let mut left = false;
        for e in entries {
            meta(1, 1, 0);
            let Some(md) = stat(&e) else {
                left = true;
                continue;
            };
            if md.is_dir() {
                left = true;
                continue;
            }
            let rel = e.path.rel().to_string();
            if r.only.is_some_and(|p| !rel.starts_with(p)) {
                continue;
            }
            let age = md.modified().map_or(0, age_s);
            if e.name.ends_with(".tmp") {
                if age > TMP_AGE_S {
                    remove_plain(&e.path, ways, dry_run)?;
                    rep.tmp_removed += 1;
                } else {
                    left = true;
                }
            } else if store::naming::parse_content_name(&rel).is_some() {
                if age >= ORPHAN_AGE_S {
                    found.push(rel);
                }
                left = true;
            } else if r.pass && planet_due {
                remove_plain(&e.path, ways, dry_run)?;
                rep.retired_removed += 1;
            } else {
                left = true;
            }
        }
        rep.folders += 1;
        rep.orphans_found += found.len();
        if !dry_run {
            if !found.is_empty() {
                rep.orphans_queued += ways.lead.orphans(&found).context("the orphans found not queued on the lead")?;
            }
            if r.pass && !left {
                meta(0, 0, 1);
                dir.remove_dir().ok();
            }
        }
        at(dir.rel())?;
        Ok(if (ways.yielding)() { Flow::Paused } else { Flow::Done })
    })
}

/// The gate's checked copies (`sources/inputs/`, docs/inputs.md §4.9), a folder at a time from the
/// one it reached: a content-named file nothing keeps (`inputs_kept`, read as the walk starts and
/// again before a chunk is told once `KEEP_FRESH_S` old: the lead's holders as each is told cover
/// what was accepted since) and `INPUTS_KEEP_S` old told the
/// lead (`Lead::removing`), `CHUNK_NAMES` at a time, those it doesn't hold removed (`remove_told`);
/// a `.tmp` file `TMP_AGE_S` old removed. None swept while what keeps them can't be read (said).
fn inputs_walk(
    root: &store::nasfs::NasRoot,
    ways: &Ways,
    dry_run: bool,
    reached: Option<&str>,
    at: &mut dyn FnMut(&str) -> Result<()>,
    rep: &mut Report,
) -> Result<Flow> {
    let _p = jobrt::timings::phase("sweeping the inputs' copies");
    let read_keep = || -> Result<BTreeSet<String>> {
        let m = buildstore::records::read_manifest(root).context("the build manifest")?;
        inputs_kept(root, &m, Duration::from_secs(INPUTS_KEEP_S), SystemTime::now())
    };
    let mut keep = match read_keep() {
        Ok(k) => (k, std::time::Instant::now()),
        Err(e) => {
            rep.inputs_skipped = Some(format!("{e:#}"));
            return Ok(Flow::Done);
        }
    };
    let roots = [Root { dir: "sources/inputs".into(), only: None, pass: false }];
    walk(root, &roots, reached, &mut |_, dir, entries| {
        let mut due = Vec::new();
        for e in entries {
            meta(1, 1, 0);
            let Some(md) = stat(&e) else { continue };
            if md.is_dir() {
                continue;
            }
            let rel = e.path.rel().to_string();
            let age = md.modified().map_or(0, age_s);
            if e.name.ends_with(".tmp") {
                if age > TMP_AGE_S {
                    remove_plain(&e.path, ways, dry_run)?;
                    rep.tmp_removed += 1;
                }
            } else if store::naming::parse_content_name(&rel).is_some() && !keep.0.contains(&rel) && age > INPUTS_KEEP_S
            {
                due.push(rel);
            }
        }
        rep.folders += 1;
        if dry_run {
            rep.inputs_removed += due.len();
        } else {
            for chunk in due.chunks(CHUNK_NAMES) {
                // (What keeps them read again once `KEEP_FRESH_S` old, before each chunk, not each
                // folder: a copy a hand-off merged since brings is held by the lead only the
                // queue's grace once the records naming its index are seen, and nothing touches a
                // copy reused, so a folder's chunks mustn't outlast the other half.)
                if keep.1.elapsed() >= Duration::from_secs(KEEP_FRESH_S) {
                    let k =
                        read_keep().context("nothing more removed: what keeps the inputs' copies not read again")?;
                    keep = (k, std::time::Instant::now());
                }
                let chunk: Vec<String> = chunk.iter().filter(|n| !keep.0.contains(*n)).cloned().collect();
                if chunk.is_empty() {
                    continue;
                }
                let held: BTreeSet<String> = ways
                    .lead
                    .removing(&chunk)
                    .context("nothing more removed: GC's queue on the lead wasn't told")?
                    .into_iter()
                    .collect();
                rep.inputs_held += held.len();
                let told: Vec<String> = chunk.iter().filter(|n| !held.contains(*n)).cloned().collect();
                let (mut removed, mut already) = (0, 0);
                let flow = remove_told(root, &told, ways, &mut removed, &mut already);
                rep.inputs_removed += removed;
                if flow? == Flow::Paused {
                    return Ok(Flow::Paused);
                }
            }
        }
        at(dir.rel())?;
        Ok(if (ways.yielding)() { Flow::Paused } else { Flow::Done })
    })
}

/// How old what keeps the gate's copies (`inputs_kept`) may be as `inputs_walk` tells the lead a
/// chunk: half GC's grace (store::map::gc::GRACE_S, so 30 minutes). The lead holds a copy a
/// merge took for the grace from that merge (`GcQueue::bringing`, then queued from its time at
/// the records after it), so one the lead no longer holds as a chunk is told was merged at least
/// half the grace before what keeps them was read; its index, uploaded before its hand-off, was
/// young on the NAS by then, and the records naming it on the NAS within that half. Once told, a
/// name is marked, so the chunk's removals after need no margin. A re-read lists `sources/inputs/`,
/// reads its young indexes and the records' brief, once per 30 minutes at most.
pub const KEEP_FRESH_S: u64 = store::map::gc::GRACE_S / 2;

/// A file no content name is (a stray `.tmp`, a retired pass's planet) removed (a dry run: said);
/// one gone already is.
fn remove_plain(p: &NasPath, ways: &Ways, dry_run: bool) -> Result<()> {
    if dry_run {
        eprintln!("gc: would remove {}", p.display());
        return Ok(());
    }
    match (ways.remove)(p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(|| format!("remove {}", p.display())),
        _ => Ok(()),
    }
}

/// Seconds since `t` (0 for a time ahead).
fn age_s(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).map_or(0, |d| d.as_secs())
}

/// The duties, once a day (`DUTIES_EVERY_S`): the names to drop (removed steps', the depth's
/// orphans, the pending uploads the build doesn't read), the history's and the forecasts' archives,
/// the forecast inputs' old days, the old catalogues once asked, and the uploads still pending
/// checked on the NAS, last. Each a failure said, never failing GC; each goes on where it stopped
/// (what it did saved as it went), so a pass cut short at a safe point is taken again whole.
fn duties(root: &store::nasfs::NasRoot, asked: &Asked, the_plan: &dyn Plan, rep: &mut Report) -> Result<Flow> {
    let dry_run = asked.dry_run;
    // (Its scratch only a folder: a removal writes nothing.)
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!("scenic-gc-{}-{n}", std::process::id()));
    match drop_names(the_plan, root, &scratch, asked.depth, dry_run) {
        Ok(d) => (rep.retired_dropped, rep.depth_dropped, rep.pending_dropped) = (d.retired, d.depth, d.unpended),
        Err(e) => rep.warnings.push(format!("names not dropped: {e:#}")),
    }
    std::fs::remove_dir_all(&scratch).ok();
    // The history's old days, one tar a day (task #531).
    match super::histarchive::run(root, crate::agent::jobs::now_s(), dry_run, &yield_point) {
        Ok(h) => {
            meta(h.listed, h.stats, h.files_removed as u64 + h.tmp_removed as u64 + h.days_archived as u64);
            rep.history_days_archived = h.days_archived;
            rep.warnings.extend(h.warnings);
        }
        Err(e) => rep.warnings.push(format!("history not archived: {e:#}")),
    }
    // The forecasts' archive's old days, one tar a month, kept for good (task #621).
    match super::fcarchive::pack(root, crate::agent::jobs::now_s(), dry_run, &yield_point) {
        Ok(h) => {
            meta(h.listed, h.stats, h.files_removed as u64 + h.tmp_removed as u64);
            rep.forecast_days_archived = h.days_archived;
            rep.warnings.extend(h.warnings);
        }
        Err(e) => rep.warnings.push(format!("forecasts not archived: {e:#}")),
    }
    // The forecast inputs' archive's days before the backtest's span (task #933).
    match super::fcinputs::prune(root, crate::agent::jobs::now_s(), dry_run) {
        Ok(h) => {
            meta(h.listed, h.stats, h.files_removed as u64);
            rep.forecast_input_days_removed = h.files_removed;
        }
        Err(e) => rep.warnings.push(format!("forecast inputs not pruned: {e:#}")),
    }
    // The old catalogues' folders, once asked (docs/maplog.md §8, step 9).
    let nas_root = store::nas::on_nas(&root.unchecked_path());
    let ssh = nas_ssh();
    let ssh: Vec<&str> = ssh.iter().map(String::as_str).collect();
    if asked.old_catalogues {
        match remove_old_catalogues(root, nas_root.as_deref().map(|n| (&ssh[..], n)), dry_run) {
            Ok(o) => {
                rep.warnings.extend(o.warnings);
                rep.old_catalogues = o.removed;
            }
            Err(e) => rep.warnings.push(format!("old catalogues not removed: {e:#}")),
        }
    }
    // The uploads still pending checked on the NAS itself, last. Not a project folder on the NAS's
    // share (a test's): none checked.
    if let Some(nas_root) = &nas_root {
        match verify_pending(the_plan, root, &scratch, &ssh, nas_root, dry_run) {
            Ok(v) => {
                (rep.pending_verified, rep.pending_failed) = (v.ok, v.bad.len());
                if let Some(why) = &v.stalled {
                    rep.warnings.push(format!(
                        "uploads' verification stopped: {why} (the batches before it are saved; the next pass takes up the rest)"
                    ));
                }
                if let Some(first) = v.bad.first() {
                    rep.warnings.push(format!(
                        "{} uploads aren't on the NAS at their size (left pending; `scenic redo` their targets): {first}{}",
                        v.bad.len(),
                        if v.bad.len() > 1 { " …" } else { "" }
                    ));
                }
            }
            Err(e) => rep.warnings.push(format!("uploads not verified: {e:#}")),
        }
        std::fs::remove_dir_all(&scratch).ok();
    }
    Ok(Flow::Done)
}

/// What the NAS kept of the catalogues before docs/maplog.md §8 step 6 (step 9): their folders and
/// the switches of steps 3 and 6 (`delta-catalog`, `map-serve`), read by nothing since, and removed
/// once every member's app checks the NAS by `map/` (crate::agent::NAS_CHECK): an older app's check
/// looks at `catalog/`, and sees the NAS away without it (every Mac's build would stop).
pub const OLD_CATALOGUES: [&str; 4] = ["catalog", "catalog-held", "state/pool/delta-catalog", "state/pool/map-serve"];

/// What `remove_old_catalogues` did.
#[derive(Debug, Default)]
pub struct OldCatalogues {
    /// Those of `OLD_CATALOGUES` removed (a dry run: there to remove).
    pub removed: Vec<String>,
    /// Why it went file by file (its call on the NAS failing), said.
    pub warnings: Vec<String>,
}

/// Step 9's pass: `OLD_CATALOGUES` removed from the NAS. By one call on the NAS itself when it can
/// (`ssh`: the command and the project folder as the NAS sees it; buildstore::out::remove_on_nas), since
/// the share takes some tens of SMB deletions a second (catalog/ held 1,041 files on 7 Nov); else,
/// or that failing, file by file over SMB, a safe point between folders (a run paused there goes on
/// at the next). Nothing else is touched.
pub fn remove_old_catalogues(
    root: &store::nasfs::NasRoot,
    ssh: Option<(&[&str], &str)>,
    dry_run: bool,
) -> Result<OldCatalogues> {
    let mut o = OldCatalogues::default();
    // (Whether it's there, and a folder.)
    let there = |name: &str| -> Result<Option<bool>> {
        meta(0, 1, 0);
        match root.join(name).metadata() {
            Ok(m) => Ok(Some(m.is_dir())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("look at {name}")),
        }
    };
    let mut left = Vec::new();
    for name in OLD_CATALOGUES {
        if there(name)?.is_some() {
            left.push(name);
        }
    }
    if dry_run || left.is_empty() {
        o.removed = left.iter().map(|n| n.to_string()).collect();
        return Ok(o);
    }
    if let Some((ssh, nas_root)) = ssh {
        meta(0, 0, 1);
        if let Err(e) = buildstore::out::remove_on_nas(ssh, nas_root, &left) {
            o.warnings.push(format!("old catalogues removed over SMB, file by file: {e:#}"));
        }
    }
    for name in left {
        match there(name)? {
            Some(true) => remove_tree(&root.join(name))?,
            Some(false) => {
                meta(0, 0, 1);
                unless_gone(root.join(name).remove_file()).with_context(|| format!("remove {name}"))?;
            }
            None => {}
        }
        o.removed.push(name.to_string());
    }
    Ok(o)
}

/// Folder `dir` and all it holds removed over SMB: its files one by one, a safe point between
/// folders, then the folders deepest first.
fn remove_tree(dir: &NasPath) -> Result<()> {
    let mut dirs = vec![dir.clone()];
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        yield_point();
        meta(1, 0, 0);
        let rd = d.read_dir().with_context(|| format!("list {}", d.display()))?;
        for e in rd {
            meta(1, 1, 0);
            // (A link goes itself, what it points at stays: its stat is its own.)
            if e.symlink_metadata().with_context(|| format!("look at {}", e.path.display()))?.is_dir() {
                stack.push(e.path.clone());
                dirs.push(e.path);
            } else {
                meta(0, 0, 1);
                unless_gone(e.path.remove_file()).with_context(|| format!("remove {}", e.path.display()))?;
            }
        }
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.rel().split('/').count()));
    for d in dirs {
        meta(0, 0, 1);
        unless_gone(d.remove_dir()).with_context(|| format!("remove {}", d.display()))?;
    }
    Ok(())
}

/// A removal's outcome, its file or folder gone already counted as done.
fn unless_gone(r: std::io::Result<()>) -> std::io::Result<()> {
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// This Mac's copies of old catalogues (docs/maplog.md §8, step 9), read by nothing since step 6:
/// the map server's `catalog/` in the app's folder (`app_home`) and `Library/Caches/scenic/catalogue/`
/// in the user's (`user_home`). Removed at the agent's start (every Mac runs one; the map server
/// only on the Mac mini, a laptop's on demand). What was removed.
pub fn remove_this_macs_catalogues(app_home: &std::path::Path, user_home: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut gone = Vec::new();
    for d in [app_home.join("catalog"), user_home.join("Library/Caches/scenic/catalogue")] {
        match std::fs::remove_dir_all(&d) {
            Ok(()) => gone.push(d),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("agent: {} not removed: {e}", d.display()),
        }
    }
    gone
}

/// A sweep's stat of entry `e`: None when it can't be read, or it's a link. A sweep never goes through
/// a link: a file reached by one has a path no root names, so it would go however much its own name
/// is kept (`sources/inputs/regions/@geometry/@geometry`, a link to its own folder, 27 Oct, took the
/// walk 32 levels down it).
fn stat(e: &store::nasfs::NasEntry) -> Option<std::fs::Metadata> {
    e.symlink_metadata().ok().filter(|m| !m.file_type().is_symlink())
}

/// A lead with nothing queued (a run's duties alone, in tests): no chunk, nothing held.
#[cfg(test)]
pub struct Idle;

#[cfg(test)]
impl Lead for Idle {
    fn chunk(&self, _: &ChunkAsk) -> Result<Chunk> {
        Ok(Chunk::default())
    }
    fn gone(&self, _: &[String], _: &[String]) -> Result<()> {
        Ok(())
    }
    fn orphans(&self, _: &[String]) -> Result<usize> {
        Ok(0)
    }
    fn removing(&self, _: &[String]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// A run's ways with `lead`, never yielding, removing over the share.
#[cfg(test)]
pub fn ways_for_tests(lead: &dyn Lead) -> Ways<'_> {
    fn never() -> bool {
        false
    }
    Ways { the_plan: super::seam::for_tests(), lead, yielding: &never, remove: &unlink }
}

/// Every sweep's pass ended now in the agent's home `home` (its map log folder's `SWEEPS_FILE`):
/// an agent's test whose plan has no GC to do (its queue empty).
#[cfg(test)]
pub fn sweeps_done_for_tests(home: &std::path::Path) {
    let now = crate::agent::jobs::now_s();
    let each =
        SWEEPS.iter().map(|(n, _)| (n.to_string(), Pass { begun: now, ended: Some(now), reached: None })).collect();
    Sweeps { each }.write(&home.join(super::publish::MAPLOG).join(SWEEPS_FILE)).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::fs::{self, FileTimes};
    use std::path::{Path, PathBuf};
    use store::map::gc::{GcQueue, Holders, Lease, GRACE_S};
    use store::map::Map;

    fn age(p: &Path, days: u64) {
        let t = SystemTime::now() - Duration::from_secs(days * 86400);
        fs::File::options().write(true).open(p).unwrap().set_times(FileTimes::new().set_modified(t)).unwrap();
    }

    fn put(root: &Path, rel: &str, days: u64) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        age(&p, days);
    }

    /// The newest lease the in-process lead marks with.
    const NEWEST: Lease = Lease { epoch: 2, n: 9 };
    /// A lease granted before every mark.
    const OLDER: Lease = Lease { epoch: 1, n: 1 };

    /// GC's queue `q` asked in process as the lead's coordinator answers it (no client, no live
    /// lease), its clock `now` (unix s).
    struct InProcess<'a> {
        q: &'a GcQueue,
        now: Cell<u64>,
    }

    impl InProcess<'_> {
        fn holders<'c>(&self, none: &'c BTreeMap<String, store::map::Cursor>) -> Holders<'c> {
            Holders { cursors: none, oldest_lease: None, now: self.now.get() }
        }
    }

    impl Lead for InProcess<'_> {
        fn chunk(&self, a: &ChunkAsk) -> Result<Chunk> {
            let none = BTreeMap::new();
            let take = |n: &str| by_the_queue(n, a.passes_before);
            let names = if a.dry {
                self.q.due_by(&self.holders(&none), &take)
            } else {
                self.q.chunk(&self.holders(&none), NEWEST, a.max, &take)?
            };
            Ok(Chunk { names, tops: self.q.tops() })
        }
        fn gone(&self, gone: &[String], released: &[String]) -> Result<()> {
            self.q.gone(gone, released, self.now.get(), NEWEST)
        }
        fn orphans(&self, names: &[String]) -> Result<usize> {
            self.q.orphans(names, self.now.get())
        }
        fn removing(&self, names: &[String]) -> Result<Vec<String>> {
            let none = BTreeMap::new();
            self.q.removing(names, &self.holders(&none), NEWEST)
        }
    }

    fn now_s() -> u64 {
        crate::agent::jobs::now_s()
    }

    /// The map naming `files` (content names) at `seq`.
    fn map_of(seq: u64, files: &[&str]) -> Map {
        let e = files
            .iter()
            .map(|c| {
                (
                    format!("f/{}", c.split_once('.').unwrap().0),
                    serde_json::json!({ "file": c, "size": 1, "fmt": 1 }).into(),
                )
            })
            .collect();
        Map::new(seq, format!("t{seq}"), e)
    }

    /// GC's queue in `dir` as a lead keeps it: its map naming `kept`, the records seen (naming
    /// nothing), and `freed` freed by a batch at `at`.
    fn queue_of(dir: &Path, freed: &[String], kept: &[&str], at: u64) -> (GcQueue, Map) {
        let all: Vec<&str> = freed.iter().map(String::as_str).chain(kept.iter().copied()).collect();
        let m1 = map_of(1, &all);
        let q = GcQueue::default();
        q.open(dir, &m1).unwrap();
        q.records((2, 1), std::iter::empty(), at);
        let m2 = map_of(2, kept);
        q.batch(&m1.batch_to(m2.entries(), 2, "t2", "", ""), at);
        (q, m2)
    }

    /// A sweeps file with every pass ended `now` (a run of the queue's chunks alone), or but `due`.
    fn sweeps_ended(dir: &Path, now: u64, due: &[&str]) -> PathBuf {
        let p = dir.join(SWEEPS_FILE);
        let each = SWEEPS
            .iter()
            .filter(|(n, _)| !due.contains(n))
            .map(|(n, _)| (n.to_string(), Pass { begun: now, ended: Some(now), reached: None }))
            .collect();
        fs::write(&p, serde_json::to_vec(&Sweeps { each }).unwrap()).unwrap();
        p
    }

    /// `n` due files, `layers/a/hi/<i % 3>/6-1-<i>.<hash>.pack`, sorted as the queue orders them,
    /// on a root that answers as the share does (its `NAS_CHECK` folder).
    fn due_files(root: &Path, n: usize) -> Vec<String> {
        fs::create_dir_all(root.join(crate::agent::NAS_CHECK)).unwrap();
        let mut v: Vec<String> =
            (0..n).map(|i| format!("layers/a/hi/{}/6-1-{i}.{:016x}.pack", i % 3, i + 0x100)).collect();
        for f in &v {
            put(root, f, 60);
        }
        v.sort();
        v
    }

    /// docs/maplog.md §7.3 (step 8b): GC removes by the queue, a chunk of `CHUNK_NAMES` at a time,
    /// each name marked deleted on the lead (so the merge refuses a hand-off reusing it) before it
    /// goes, then said gone; what the map names stays. A dry run counts what's due, marking and
    /// removing nothing. The lead not asked: nothing removed, the run fails; what went not said
    /// gone: the names stay told, for the next chunk.
    #[test]
    fn every_removal_is_marked_deleted_before_it_goes() {
        let (d, q_dir, home) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let kept = "layers/a/hi/6-9-9.9999999999999999.pack";
        put(root, kept, 60);
        let due = due_files(root, CHUNK_NAMES + 5);
        let t0 = now_s() - GRACE_S - 10;
        let (q, _) = queue_of(q_dir.path(), &due, &[kept], t0);
        let lead = InProcess { q: &q, now: Cell::new(now_s()) };
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let sweeps = sweeps_ended(home.path(), now_s(), &[]);
        let asked = Asked { sweeps: Some(&sweeps), ..Default::default() };
        let r = run(&nas, &Asked { dry_run: true, ..asked }, &ways_for_tests(&lead)).unwrap();
        assert_eq!((r.due, r.removed), (due.len(), 0));
        assert!(q.removing_now().is_empty() && q.deleted_under(due.iter(), OLDER).is_none(), "a dry run marks nothing");
        let order: RefCell<Vec<String>> = Default::default();
        let remove = |p: &NasPath| {
            assert!(q.deleted_under([p.rel().to_string()].iter(), OLDER).is_some(), "{} removed unmarked", p.rel());
            order.borrow_mut().push(p.rel().to_string());
            unlink(p)
        };
        let ways = Ways { remove: &remove, ..ways_for_tests(&lead) };
        let r = run(&nas, &asked, &ways).unwrap();
        assert_eq!((r.chunks, r.removed, r.gone_already, r.paused), (2, due.len(), 0, false), "{r:?}");
        assert_eq!(*order.borrow(), due, "in the queue's order");
        assert!(due.iter().all(|f| !root.join(f).exists()) && root.join(kept).exists());
        assert!(q.is_empty() && q.removing_now().is_empty());
        assert!(q.deleted_under(due.iter(), OLDER).is_some(), "the marks kept for the merge");
        // The lead not asked: nothing removed, the run failed.
        struct Away;
        impl Lead for Away {
            fn chunk(&self, _: &ChunkAsk) -> Result<Chunk> {
                anyhow::bail!("the lead's coordinator doesn't answer")
            }
            fn gone(&self, _: &[String], _: &[String]) -> Result<()> {
                anyhow::bail!("the lead's coordinator doesn't answer")
            }
            fn orphans(&self, _: &[String]) -> Result<usize> {
                anyhow::bail!("the lead's coordinator doesn't answer")
            }
            fn removing(&self, _: &[String]) -> Result<Vec<String>> {
                anyhow::bail!("the lead's coordinator doesn't answer")
            }
        }
        let more = due_files(root, 3);
        let e = format!("{:#}", run(&nas, &asked, &ways_for_tests(&Away)).unwrap_err());
        assert!(e.contains("nothing removed") && e.contains("doesn't answer"), "{e}");
        assert!(more.iter().all(|f| root.join(f).exists()));
        // What went not said gone (the lead gone between): the run fails, the names still told.
        let (q, _) = queue_of(tempfile::tempdir().unwrap().path(), &more, &[], t0);
        struct Deaf<'a>(InProcess<'a>);
        impl Lead for Deaf<'_> {
            fn chunk(&self, a: &ChunkAsk) -> Result<Chunk> {
                self.0.chunk(a)
            }
            fn gone(&self, _: &[String], _: &[String]) -> Result<()> {
                anyhow::bail!("the lead's coordinator doesn't answer")
            }
            fn orphans(&self, n: &[String]) -> Result<usize> {
                self.0.orphans(n)
            }
            fn removing(&self, n: &[String]) -> Result<Vec<String>> {
                self.0.removing(n)
            }
        }
        let deaf = Deaf(InProcess { q: &q, now: Cell::new(now_s()) });
        assert!(run(&nas, &asked, &ways_for_tests(&deaf)).is_err());
        assert_eq!(q.removing_now(), more, "told, for the next chunk to take again");
    }

    /// docs/maplog.md §8 step 8b: the job killed between two deletions mid-chunk: the next run's
    /// first chunk is the one it was in, told again; the next deletion is of the first file not
    /// gone (those gone count as gone already, but not while the share doesn't answer: a lost mount
    /// finds no file), nothing is deleted twice, nothing kept for ever, and the queue, back from its
    /// file (a lead restart), ends empty.
    #[test]
    fn gc_resumes_mid_chunk_after_a_kill() {
        let (d, q_dir, home) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let due = due_files(root, CHUNK_NAMES + 50);
        let t0 = now_s() - GRACE_S - 10;
        let (q, m2) = queue_of(q_dir.path(), &due, &[], t0);
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let sweeps = sweeps_ended(home.path(), now_s(), &[]);
        let asked = Asked { sweeps: Some(&sweeps), ..Default::default() };
        // (Killed after 230 deletions: 30 into the second chunk.)
        let kill_at = CHUNK_NAMES + 30;
        let deleted: RefCell<Vec<String>> = Default::default();
        let killing = |p: &NasPath| {
            if deleted.borrow().len() == kill_at {
                panic!("killed");
            }
            unlink(p)?;
            deleted.borrow_mut().push(p.rel().to_string());
            Ok(())
        };
        {
            let lead = InProcess { q: &q, now: Cell::new(now_s()) };
            let ways = Ways { remove: &killing, ..ways_for_tests(&lead) };
            let killed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&nas, &asked, &ways)));
            assert!(killed.is_err());
        }
        assert_eq!(*deleted.borrow(), due[..kill_at]);
        assert!(due[kill_at..].iter().all(|f| root.join(f).exists()));
        // The lead back from its disk: the second chunk still told, the first gone.
        let back = GcQueue::default();
        back.open(q_dir.path(), &m2).unwrap();
        back.records((2, 1), std::iter::empty(), now_s());
        assert_eq!(back.removing_now(), due[CHUNK_NAMES..], "the chunk it was in");
        assert_eq!(back.len(), 50);
        let tried: RefCell<Vec<(String, bool)>> = Default::default();
        let counting = |p: &NasPath| {
            let r = unlink(p);
            tried.borrow_mut().push((p.rel().to_string(), r.is_ok()));
            r
        };
        let lead = InProcess { q: &back, now: Cell::new(now_s()) };
        // (The share lost first, its mount gone: no file is found, and none counts as gone.)
        let map_dir = root.join(crate::agent::NAS_CHECK);
        fs::rename(&map_dir, root.join("map-away")).unwrap();
        let e = format!("{:#}", run(&nas, &asked, &Ways { remove: &counting, ..ways_for_tests(&lead) }).unwrap_err());
        assert!(e.contains("doesn't answer"), "{e}");
        assert_eq!(back.removing_now(), [due[CHUNK_NAMES].clone()], "nothing said gone");
        assert!(
            due[CHUNK_NAMES..].iter().all(|f| back.deleted_under([f].into_iter(), OLDER).is_some()),
            "the rest let go but still marked: the killed run may have removed them"
        );
        tried.take();
        fs::rename(root.join("map-away"), &map_dir).unwrap();
        let r = run(&nas, &asked, &Ways { remove: &counting, ..ways_for_tests(&lead) }).unwrap();
        assert_eq!((r.chunks, r.removed, r.gone_already), (1, 20, 30), "{r:?}");
        let tried = tried.take();
        let removed: Vec<&String> = tried.iter().filter(|(_, ok)| *ok).map(|(n, _)| n).collect();
        assert_eq!(removed, due[kill_at..].iter().collect::<Vec<_>>(), "from the first not gone, in order");
        let mut all: Vec<String> = deleted.take().into_iter().chain(removed.into_iter().cloned()).collect();
        all.sort();
        assert_eq!(all, due, "each deleted once");
        assert!(due.iter().all(|f| !root.join(f).exists()));
        assert!(back.is_empty() && back.removing_now().is_empty(), "nothing kept for ever");
    }

    /// A run yields at a safe point mid-chunk (a job held back for the NAS it holds): what went is
    /// said gone, the rest of the chunk let go (queued, unmarked: a hand-off may bring one meanwhile);
    /// the next run starts where it left off, at the first file not gone.
    #[test]
    fn gc_yields_mid_chunk_and_resumes_where_it_left_off() {
        let (d, q_dir, home) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let due = due_files(root, 50);
        let (q, _) = queue_of(q_dir.path(), &due, &[], now_s() - GRACE_S - 10);
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let sweeps = sweeps_ended(home.path(), now_s(), &[]);
        let asked = Asked { sweeps: Some(&sweeps), ..Default::default() };
        let lead = InProcess { q: &q, now: Cell::new(now_s()) };
        let removed: RefCell<Vec<String>> = Default::default();
        let remove = |p: &NasPath| {
            removed.borrow_mut().push(p.rel().to_string());
            unlink(p)
        };
        let yielding = || removed.borrow().len() >= 20;
        let r = run(
            &nas,
            &asked,
            &Ways { yielding: &yielding, remove: &remove, lead: &lead, the_plan: super::super::seam::for_tests() },
        )
        .unwrap();
        assert!(r.paused && r.removed == 20 && r.swept.is_empty(), "{r:?}");
        assert!(due[..20].iter().all(|f| !root.join(f).exists()) && due[20..].iter().all(|f| root.join(f).exists()));
        assert!(q.removing_now().is_empty(), "the rest let go");
        assert!(due[20..].iter().all(|f| q.queued(f).is_some()) && q.deleted_under(due[20..].iter(), OLDER).is_none());
        removed.take();
        let r = run(&nas, &asked, &Ways { remove: &remove, ..ways_for_tests(&lead) }).unwrap();
        assert_eq!((r.removed, r.gone_already, r.paused), (30, 0, false), "{r:?}");
        assert_eq!(*removed.borrow(), due[20..], "where it left off");
        assert!(q.is_empty());
    }

    /// The orphan sweep (docs/maplog.md §7.5) queues a content-named file nothing holds only once
    /// it's `ORPHAN_AGE_S` old, freed when found (so it waits its grace from then, and a lease older
    /// than that holds it); a held one never; a removed step's files and a retired pass's too; it
    /// removes `.tmp` files `TMP_AGE_S` old and a retired pass's planet once the newer pass has
    /// settled; it never goes through a link, nor lists the newest pass or the rest of `sources/`.
    #[test]
    fn an_orphan_is_queued_only_after_its_age() {
        use std::os::unix::fs::symlink;
        let (d, q_dir, home) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let days = ORPHAN_AGE_S / 86400;
        let (kept, old, young) = (
            "layers/a/hi/6-1-1.1111111111111111.pack",
            "layers/a/hi/6-1-2.2222222222222222.pack",
            "layers/a/hi/6-1-3.3333333333333333.pack",
        );
        let (tmp_old, tmp_young, plain) = ("layers/a/hi/x.pack.tmp", "layers/a/hi/y.pack.tmp", "layers/a/README");
        let (z8, other) = ("sources/terrain-z8-v1.4444444444444444.pack", "sources/registers/r.5555555555555555.tar");
        let (old_pass, planet, newest, newest_done) = (
            "sources/osm/2026-03-01/pieces/6-1-1.6666666666666666.osm.pbf",
            "sources/osm/2026-03-01/planet.osm.pbf",
            "sources/osm/2026-09-28/pieces/6-1-1.7777777777777777.osm.pbf",
            "sources/osm/2026-09-28/pass.8888888888888888.json",
        );
        let outside = "state/x.9999999999999999.json";
        for f in [kept, old, plain, z8, other, old_pass, planet, newest, outside] {
            put(root, f, days + 1);
        }
        put(root, young, days - 1);
        put(root, tmp_old, TMP_AGE_S / 86400 + 1);
        put(root, tmp_young, 1);
        put(root, newest_done, PLANET_KEEP_S / 86400 + 1);
        symlink(root.join("layers/a/hi"), root.join("layers/a/hi/loop")).unwrap();
        symlink(root.join("state"), root.join("layers/a/state")).unwrap();
        let (q, _) = queue_of(q_dir.path(), &[], &[kept], now_s());
        let lead = InProcess { q: &q, now: Cell::new(now_s()) };
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let sweeps = sweeps_ended(home.path(), now_s(), &["orphans"]);
        let asked = Asked { sweeps: Some(&sweeps), ..Default::default() };
        let r = run(&nas, &Asked { dry_run: true, ..asked }, &ways_for_tests(&lead)).unwrap();
        assert_eq!((r.orphans_found, r.orphans_queued, r.tmp_removed), (4, 0, 1), "{r:?}");
        assert!(q.is_empty() && root.join(tmp_old).exists() && root.join(planet).exists(), "a dry run does nothing");
        let r = run(&nas, &asked, &ways_for_tests(&lead)).unwrap();
        assert_eq!(r.swept, ["orphans"]);
        assert_eq!((r.orphans_found, r.orphans_queued, r.tmp_removed, r.retired_removed), (4, 3, 1, 1), "{r:?}");
        for f in [old, z8, old_pass] {
            assert_eq!(q.queued(f).map(|x| x.at), Some(lead.now.get()), "{f}");
        }
        for f in [kept, young, other, newest, outside] {
            assert!(q.queued(f).is_none(), "{f}");
        }
        assert!(!root.join(tmp_old).exists() && root.join(tmp_young).exists() && !root.join(planet).exists());
        assert!(root.join(plain).exists() && root.join(old_pass).exists());
        // Found now: due once its grace has passed (and the lease rule from now), not before.
        let none = BTreeMap::new();
        let at = |now: u64, lease: Option<u64>| q.due(&Holders { cursors: &none, oldest_lease: lease, now });
        assert!(at(lead.now.get() + GRACE_S - 1, None).is_empty());
        assert!(at(lead.now.get() + GRACE_S, Some(lead.now.get() - 1)).is_empty(), "a lease older than the find");
        assert_eq!(at(lead.now.get() + GRACE_S, None).len(), 3);
        // The pass ended: not due again until `ORPHAN_AGE_S` after.
        let s = Sweeps::read(&sweeps);
        assert!(!s.due("orphans", now_s()) && s.due("orphans", now_s() + ORPHAN_AGE_S));
    }

    /// docs/maplog.md §4.5, §7.5: a lead whose disk was lost starts its queue empty; the orphan
    /// sweep finds what the lost queue held (old, named by nothing), and GC's chunks then remove it
    /// once its grace from the find has passed; what the map names stays.
    #[test]
    fn the_queue_rebuilds_after_a_lead_disk_restore() {
        let (d, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let kept = "base/6-1-1.1111111111111111.sect";
        let lost = ["base/6-1-2.2222222222222222.sect", "layers/a/hi/6-1-2.3333333333333333.pack"];
        for f in lost.iter().chain([&kept]) {
            put(root, f, ORPHAN_AGE_S / 86400 + 10);
        }
        // (The restored lead: its map from the NAS's snapshot, a queue file of nothing.)
        let fresh = tempfile::tempdir().unwrap();
        let (q, _) = queue_of(fresh.path(), &[], &[kept, "layers/a/hi/6-1-1.4444444444444444.pack"], now_s());
        let lead = InProcess { q: &q, now: Cell::new(now_s()) };
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let sweeps = sweeps_ended(home.path(), now_s(), &["orphans"]);
        let r = run(&nas, &Asked { sweeps: Some(&sweeps), ..Default::default() }, &ways_for_tests(&lead)).unwrap();
        assert_eq!((r.orphans_queued, r.removed), (2, 0), "{r:?}");
        lead.now.set(lead.now.get() + GRACE_S);
        let sweeps = sweeps_ended(home.path(), now_s(), &[]);
        let r = run(&nas, &Asked { sweeps: Some(&sweeps), ..Default::default() }, &ways_for_tests(&lead)).unwrap();
        assert_eq!(r.removed, 2, "{r:?}");
        assert!(lost.iter().all(|f| !root.join(f).exists()) && root.join(kept).exists());
    }

    /// The orphan sweep stopped at a safe point (between folders) goes on after the folder it
    /// reached at its next run: each folder listed once in all, every orphan found.
    #[test]
    fn the_orphan_sweep_goes_on_from_the_folder_it_reached() {
        let (d, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let orphans: Vec<String> = (0..6).map(|i| format!("layers/a/d{i}/e/6-1-{i}.{:016x}.pack", i + 0x10)).collect();
        for f in &orphans {
            put(root, f, ORPHAN_AGE_S / 86400 + 1);
        }
        let q_dir = tempfile::tempdir().unwrap();
        let (q, _) = queue_of(q_dir.path(), &[], &["layers/a/6-9-9.9999999999999999.pack"], now_s());
        let lead = InProcess { q: &q, now: Cell::new(now_s()) };
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let sweeps = sweeps_ended(home.path(), now_s(), &["orphans"]);
        let asked = Asked { sweeps: Some(&sweeps), ..Default::default() };
        let listed = Cell::new(0);
        let yielding = || {
            listed.set(listed.get() + 1);
            listed.get() == 5
        };
        let r = run(&nas, &asked, &Ways { yielding: &yielding, ..ways_for_tests(&lead) }).unwrap();
        assert!(r.paused && r.folders == 5, "{r:?}");
        let reached = Sweeps::read(&sweeps).each["orphans"].reached.clone();
        // (layers, layers/a, its d0 and d0/e, d1: the fifth.)
        assert_eq!(reached.as_deref(), Some("layers/a/d1"), "{reached:?}");
        let r = run(&nas, &asked, &ways_for_tests(&lead)).unwrap();
        // (layers, a, six d's and their e's: 14 in all.)
        assert_eq!(r.folders, 14 - 5, "{r:?}");
        assert!(orphans.iter().all(|f| q.queued(f).is_some()));
        let s = Sweeps::read(&sweeps);
        assert!(s.each["orphans"].ended.is_some() && s.each["orphans"].reached.is_none());
    }

    /// Task #451: a run in a job counts its NAS metadata work (buildstore::costin: `listed`, `stats`,
    /// `deletes`), its weight beside its bytes; outside one (no `SCENIC_COSTS`) nothing. A chunk
    /// costs a delete a file and nothing else on the share.
    #[test]
    fn a_run_counts_its_listings_stats_and_deletes() {
        let _l = buildstore::costin::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (d, q_dir, home) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = d.path();
        let due = due_files(root, 2);
        let (q, _) = queue_of(q_dir.path(), &due, &["layers/a/6-9-9.9999999999999999.pack"], now_s() - GRACE_S - 10);
        let lead = InProcess { q: &q, now: Cell::new(now_s()) };
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let sweeps = sweeps_ended(home.path(), now_s(), &[]);
        buildstore::costin::begin();
        COUNTING.set(true);
        let r =
            run_counted(&nas, &Asked { sweeps: Some(&sweeps), ..Default::default() }, &ways_for_tests(&lead)).unwrap();
        let m = buildstore::costin::take();
        assert_eq!(r.removed, 2);
        assert_eq!((m.get("deletes"), m.get("listed"), m.get("stats")), (Some(&2), None, None), "{m:?}");
        // The orphan sweep lists and looks at each entry.
        let sweeps = sweeps_ended(home.path(), now_s(), &["orphans"]);
        buildstore::costin::begin();
        run_counted(&nas, &Asked { sweeps: Some(&sweeps), ..Default::default() }, &ways_for_tests(&lead)).unwrap();
        COUNTING.set(false);
        let m = buildstore::costin::take();
        assert!(m["listed"] >= 1 && m["stats"] >= 1, "{m:?}");
        buildstore::costin::begin();
        meta(5, 5, 5);
        assert!(!buildstore::costin::take().contains_key("listed"), "not in a job: not counted");
    }

    /// The gate's copies (docs/inputs.md §4.9): kept while the manifest's index or report names
    /// them, or a version replaced lately; the rest, old,
    /// told the lead and removed, but those it holds as they're told; the inputs folders and the
    /// rest of `sources/` never swept; none swept while an index can't be read. A link isn't gone
    /// through.
    #[test]
    fn the_inputs_copies_go_once_nothing_names_them() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, body: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, body).unwrap();
            age(&p, days);
        };
        let u = "sources/inputs/_gate-test";
        let kept = format!("{u}/a.1111111111111111.jsonl");
        let index = |unit: &str, files: &[&str]| {
            let files: serde_json::Map<String, serde_json::Value> = files
                .iter()
                .enumerate()
                .map(|(i, f)| (format!("f{i}.jsonl"), serde_json::json!({"file": f, "size": 1, "keyed": "-"})))
                .collect();
            serde_json::json!({"fmt": 1, "unit": unit, "checks": format!("{unit} 1"), "files": files}).to_string()
        };
        put(&kept, "x", 60);
        put(&format!("{u}/a.2222222222222222.jsonl"), "x", 60); // an old version: goes
        put(&format!("{u}/a.3333333333333333.jsonl"), "x", 3); // young: stays
        put(&format!("{u}/@index.5555555555555555.json"), &index("_gate-test", &[&kept]), 60);
        put(&format!("{u}/@held.7777777777777777.json"), "{}", 60);
        put(&format!("{u}/@held.8888888888888888.json"), "{}", 60); // an old report: goes
        put(&format!("{u}/c.9999999999999990.jsonl"), "x", 60); // old, but the lead holds it as told
        put("inputs/_gate-test/a.jsonl", "x", 300); // the inputs folder: never
        put("sources/registers/legacy.9999999999999999.tar.zst", "x", 300); // the rest of sources/: never
        symlink(root.join(u), root.join(format!("{u}/loop"))).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        // A nested unit (`timetables/gtfs`'s kind): its index named, its copy kept; an old copy of
        // it gone.
        let n = "sources/inputs/_gate-nest/inner";
        put(&format!("{n}/c.aaaaaaaaaaaaaaab.zip"), "x", 60);
        put(&format!("{n}/c.aaaaaaaaaaaaaaac.zip"), "x", 60); // an old version: goes
        put(
            &format!("{n}/@index.aaaaaaaaaaaaaaad.json"),
            &index("_gate-nest/inner", &[&format!("{n}/c.aaaaaaaaaaaaaaab.zip")]),
            60,
        );
        fs::write(root.join("state/build/manifest.json"), serde_json::json!({format!("{u}/@index"): format!("{u}/@index.5555555555555555.json"), format!("{u}/@held"): format!("{u}/@held.7777777777777777.json"), format!("{n}/@index"): format!("{n}/@index.aaaaaaaaaaaaaaad.json")}).to_string()).unwrap();
        // (The lead's queue: its records hold the copy a hand-off just brought, `c`.)
        let q_dir = tempfile::tempdir().unwrap();
        let q = GcQueue::default();
        q.open(q_dir.path(), &map_of(1, &[])).unwrap();
        let held = format!("{u}/c.9999999999999990.jsonl");
        q.records((2, 1), [held.clone()].iter(), now_s());
        let lead = InProcess { q: &q, now: Cell::new(now_s()) };
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let home = tempfile::tempdir().unwrap();
        let sweeps = home.path().join(SWEEPS_FILE);
        let pass = || {
            sweeps_ended(home.path(), now_s(), &["copies"]);
            run(&nas, &Asked { sweeps: Some(&sweeps), ..Default::default() }, &ways_for_tests(&lead)).unwrap()
        };
        let r = pass();
        assert_eq!((r.inputs_skipped.as_deref(), r.inputs_held), (None, 1), "{r:?}");
        assert_eq!(r.inputs_removed, 3, "{r:?}");
        assert!(
            !root.join(format!("{u}/a.2222222222222222.jsonl")).exists()
                && !root.join(format!("{u}/@held.8888888888888888.json")).exists()
                && !root.join(format!("{n}/c.aaaaaaaaaaaaaaac.zip")).exists()
        );
        for k in [
            kept.clone(),
            held.clone(),
            format!("{u}/a.3333333333333333.jsonl"),
            format!("{u}/@index.5555555555555555.json"),
            format!("{u}/@held.7777777777777777.json"),
            format!("{n}/c.aaaaaaaaaaaaaaab.zip"),
            "inputs/_gate-test/a.jsonl".into(),
            "sources/registers/legacy.9999999999999999.tar.zst".into(),
        ] {
            assert!(root.join(&k).exists(), "{k}");
        }
        // A version replaced: its index touched as the check replaced it (yesterday), the copy it
        // lists accepted 30 days ago and named by no record any more: kept, restorable.
        put(&format!("{u}/a.cccccccccccccccc.jsonl"), "x", 30);
        put(
            &format!("{u}/@index.dddddddddddddddd.json"),
            &index("_gate-test", &[&format!("{u}/a.cccccccccccccccc.jsonl")]),
            1,
        );
        let r = pass();
        assert_eq!(r.inputs_removed, 0, "{r:?}");
        assert!(root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists());
        // Its touch failed (its time 30 days old), but the check's state made yesterday names it
        // replaced: kept all the same, the nested unit's alike.
        age(&root.join(format!("{u}/@index.dddddddddddddddd.json")), 30);
        put(&format!("{u}/@listed.eeeeeeeeeeeeeeee.json"), &serde_json::json!({"fmt": 1, "unit": "_gate-test", "listed": {}, "replaced": format!("{u}/@index.dddddddddddddddd.json")}).to_string(), 1);
        put(&format!("{n}/c.aaaaaaaaaaaaaaae.zip"), "x", 40);
        put(
            &format!("{n}/@index.aaaaaaaaaaaaaaaf.json"),
            &index("_gate-nest/inner", &[&format!("{n}/c.aaaaaaaaaaaaaaae.zip")]),
            40,
        );
        put(&format!("{n}/@listed.bbbbbbbbbbbbbbbb.json"), &serde_json::json!({"fmt": 1, "unit": "_gate-nest/inner", "listed": {}, "replaced": format!("{n}/@index.aaaaaaaaaaaaaaaf.json")}).to_string(), 2);
        let r = pass();
        assert_eq!(r.inputs_removed, 0, "{r:?}");
        assert!(
            root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists()
                && root.join(format!("{n}/c.aaaaaaaaaaaaaaae.zip")).exists()
        );
        // 15 days after it was replaced (the state that named it that old too): gone, with its index.
        age(&root.join(format!("{u}/@listed.eeeeeeeeeeeeeeee.json")), 15);
        let r = pass();
        assert_eq!(r.inputs_removed, 3, "the copy, its index, the old state: {r:?}");
        assert!(!root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists());
        // An index named that can't be read: nothing there swept.
        put(&format!("{u}/a.aaaaaaaaaaaaaaaa.jsonl"), "x", 60);
        fs::write(root.join(format!("{u}/@index.5555555555555555.json")), b"{not json").unwrap();
        let r = pass();
        assert!(r.inputs_skipped.is_some() && r.inputs_removed == 0);
        assert!(root.join(format!("{u}/a.aaaaaaaaaaaaaaaa.jsonl")).exists());
    }

    /// Task #701: a removed step's names (`RETIRED`) leave the manifest in one save, the rest kept
    /// (their files then go as GC's queue frees them: `by_the_queue` takes them under `sources/`);
    /// a run after changes nothing; a dry run counts, writing nothing.
    #[test]
    fn removed_steps_names_leave_the_manifest_and_the_queue_takes_their_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let (v1, v3, other) = (
            "sources/terrain-z8-v1.1111111111111111.pack",
            "sources/terrain-z8-v3.3333333333333333.pack",
            "base/6-1-1.4444444444444444.sect",
        );
        fs::create_dir_all(root.join("state/build")).unwrap();
        let manifest =
            serde_json::json!({ "sources/terrain-z8-v1": v1, "sources/terrain-z8-v3": v3, "base/6-1-1": other });
        let mpath = root.join("state/build/manifest.json");
        fs::write(&mpath, manifest.to_string()).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let duties = |dry_run: bool| {
            let home = tempfile::tempdir().unwrap();
            let sweeps = sweeps_ended(home.path(), now_s(), &["duties"]);
            run(&nas, &Asked { dry_run, sweeps: Some(&sweeps), ..Default::default() }, &ways_for_tests(&Idle)).unwrap()
        };
        assert_eq!(duties(true).retired_dropped, 2, "a dry run counts what it would drop");
        assert_eq!(fs::read(&mpath).unwrap(), manifest.to_string().as_bytes());
        let r = duties(false);
        assert_eq!(r.retired_dropped, 2, "{:?}", r.warnings);
        let m: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&mpath).unwrap()).unwrap();
        assert_eq!(m.keys().collect::<Vec<_>>(), ["base/6-1-1"]);
        let before = fs::metadata(&mpath).unwrap().modified().unwrap();
        assert_eq!(duties(false).retired_dropped, 0);
        assert_eq!(fs::metadata(&mpath).unwrap().modified().unwrap(), before, "nothing written");
        for f in
            [v1, v3, "global/legacy/ferries.1111111111111111.json", "work/terrain-lakes/6-1-1.1111111111111111.bin"]
        {
            assert!(by_the_queue(f, None), "{f}");
        }
        for f in [
            "sources/registers/r.1111111111111111.tar",
            "sources/inputs/u/a.1111111111111111.jsonl",
            "state/x.1111111111111111.json",
        ] {
            assert!(!by_the_queue(f, Some("2026-09-28")), "{f}");
        }
        assert!(by_the_queue("sources/osm/2026-03-01/pieces/6-1-1.1111111111111111.osm.pbf", Some("2026-09-28")));
        assert!(!by_the_queue("sources/osm/2026-09-28/pieces/6-1-1.1111111111111111.osm.pbf", Some("2026-09-28")));
        assert!(!by_the_queue("sources/osm/2026-03-01/pieces/6-1-1.1111111111111111.osm.pbf", None));
    }

    /// Task #52: the old pipeline's converted data (`global/legacy/`) is retired: its names leave
    /// the manifest (their uploads never verified leave the pending); nothing else under `global/`
    /// is dropped, a look-alike folder included.
    #[test]
    fn the_legacy_folder_is_retired_and_nothing_else_under_global() {
        for l in ["global/legacy/ferries", "global/legacy/details-poi", "global/legacy/dem-stats"] {
            assert!(retired(l), "{l}");
        }
        for l in ["global/legacy", "global/legacyx/ferries", "global/railfreq", "global/heritage/layer-summary"] {
            assert!(!retired(l), "{l}");
        }
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let (ferries, poi, rail, near) = (
            "global/legacy/ferries.1111111111111111.json",
            "global/legacy/details-poi.2222222222222222.jsonl",
            "global/railfreq.4444444444444444.bin",
            "global/legacyx/ferries.5555555555555555.json",
        );
        fs::create_dir_all(root.join("state/build")).unwrap();
        let manifest = serde_json::json!({
            "global/legacy/ferries": ferries,
            "global/legacy/details-poi": poi,
            "global/railfreq": rail,
            "global/legacyx/ferries": near,
        });
        let mpath = root.join("state/build/manifest.json");
        fs::write(&mpath, manifest.to_string()).unwrap();
        let ppath = root.join("state/build/pending.json");
        fs::write(&ppath, serde_json::json!({ferries: "a", poi: "b", rail: "c"}).to_string()).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let dr = drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), None, false).unwrap();
        assert_eq!(dr.retired, 2);
        let p: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&ppath).unwrap()).unwrap();
        assert_eq!(p.keys().collect::<Vec<_>>(), [rail]);
        let m: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&mpath).unwrap()).unwrap();
        assert_eq!(m.keys().collect::<Vec<_>>(), ["global/legacyx/ferries", "global/railfreq"]);
    }

    /// Task #679: the depth packs no current target names leave the manifest in one save, with the
    /// removed steps' names; every other name stays (a target's packs, other layers' of the same
    /// tile). Without the targets, no depth name goes; a dry run counts them, writing nothing; a
    /// run after drops nothing.
    #[test]
    fn depth_packs_no_target_names_leave_the_manifest() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let kept_hi = "layers/depth/hi/6-1-33.1111111111111111.pack";
        let kept_lo = "layers/depth-shade/lo/3-0-4.2222222222222222.pack";
        let gone_hi = "layers/depth/hi/6-2-33.3333333333333333.pack";
        let gone_class = "layers/depth-class/hi/6-2-33.4444444444444444.pack";
        let gone_lo = "layers/depth/lo/3-5-5.5555555555555555.pack";
        let terrain = "layers/terrain/hi/6-2-33.6666666666666666.pack";
        let z8 = "sources/terrain-z8-v1.7777777777777777.pack";
        let logical = |c: &str| c.split_once('.').unwrap().0.to_string();
        let manifest: BTreeMap<String, String> = [kept_hi, kept_lo, gone_hi, gone_class, gone_lo, terrain, z8]
            .iter()
            .map(|c| (logical(c), c.to_string()))
            .collect();
        fs::create_dir_all(root.join("state/build")).unwrap();
        let mpath = root.join("state/build/manifest.json");
        let text = serde_json::to_string(&manifest).unwrap();
        fs::write(&mpath, &text).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let k = keep();
        let dr = drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), None, true).unwrap();
        assert_eq!((dr.retired, dr.depth), (1, 0), "without the targets, no depth name");
        let dr = drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), Some(&k), true).unwrap();
        assert_eq!((dr.retired, dr.depth), (1, 3));
        assert_eq!(fs::read_to_string(&mpath).unwrap(), text, "a dry run writes nothing");
        let dr = drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), Some(&k), false).unwrap();
        assert_eq!((dr.retired, dr.depth), (1, 3));
        let m: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&mpath).unwrap()).unwrap();
        assert_eq!(
            m.keys().collect::<Vec<_>>(),
            ["layers/depth-shade/lo/3-0-4", "layers/depth/hi/6-1-33", "layers/terrain/hi/6-2-33"]
        );
        let before = fs::metadata(&mpath).unwrap().modified().unwrap();
        assert_eq!(
            drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), Some(&k), false).unwrap(),
            Dropped::default()
        );
        assert_eq!(fs::metadata(&mpath).unwrap().modified().unwrap(), before);
    }

    /// Step 9's pass (docs/maplog.md §8): the old catalogues' folders go whole, by one call on the
    /// NAS (here a shell on this Mac in its place), else, or that failing, over SMB file by file;
    /// nothing else goes, and a dry run removes nothing. (Its gate: agent's
    /// the_old_catalogues_go_only_once_every_app_checks_by_the_map.)
    #[test]
    fn the_old_catalogues_folders_go_whole() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let files = [
            "catalog/1066.delta.json.zst",
            "catalog/a/1.json.zst",
            "catalog-held/1060.json.zst",
            "state/pool/delta-catalog",
            "state/pool/map-serve",
        ];
        let fill = || {
            for f in files.iter().chain(&["map/1689.json.zst", "state/pool/nas-budget"]) {
                fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
                fs::write(root.join(f), b"x").unwrap();
            }
        };
        let gone = || {
            OLD_CATALOGUES.iter().all(|n| !root.join(n).exists())
                && root.join("map/1689.json.zst").exists()
                && root.join("state/pool/nas-budget").exists()
        };
        fill();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let o = remove_old_catalogues(&nas, None, true).unwrap();
        assert_eq!(o.removed, OLD_CATALOGUES);
        assert!(files.iter().all(|f| root.join(f).exists()), "a dry run removes nothing");
        let o = remove_old_catalogues(&nas, None, false).unwrap();
        assert_eq!((o.removed, o.warnings.len()), (OLD_CATALOGUES.map(String::from).to_vec(), 0));
        assert!(gone());
        assert!(remove_old_catalogues(&nas, None, false).unwrap().removed.is_empty(), "none left");
        let at = root.to_str().unwrap();
        fill();
        let o = remove_old_catalogues(&nas, Some((&["sh", "-c"], at)), false).unwrap();
        assert_eq!((o.removed.len(), o.warnings), (4, Vec::<String>::new()));
        assert!(gone());
        fill();
        let o = remove_old_catalogues(&nas, Some((&["false"], at)), false).unwrap();
        assert_eq!(o.removed.len(), 4);
        assert!(o.warnings[0].contains("file by file"), "{:?}", o.warnings);
        assert!(gone());
    }

    /// This Mac's copies of old catalogues (the map server's `catalog/`, the cache's `catalogue/`)
    /// go at the agent's start; the rest of the app's folder and the cache stay.
    #[test]
    fn this_macs_old_catalogues_go() {
        let (app, user) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for p in [
            app.path().join("catalog/1066.json.zst"),
            app.path().join("mirror/x"),
            user.path().join("Library/Caches/scenic/catalogue/1.json.zst"),
            user.path().join("Library/Caches/scenic/map/current.json.zst"),
        ] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        assert_eq!(remove_this_macs_catalogues(app.path(), user.path()).len(), 2);
        assert!(!app.path().join("catalog").exists() && !user.path().join("Library/Caches/scenic/catalogue").exists());
        assert!(
            app.path().join("mirror/x").exists()
                && user.path().join("Library/Caches/scenic/map/current.json.zst").exists()
        );
        assert!(remove_this_macs_catalogues(app.path(), user.path()).is_empty());
    }

    /// Task #701: in the pool the drop is the gc job's hand-off (merged by the lead), never a
    /// save of the records; a name outside `RETIRED` it can't remove.
    #[test]
    fn in_the_pool_the_drop_is_a_hand_off() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        fs::create_dir_all(root.join("state/build")).unwrap();
        let manifest = r#"{"sources/terrain-z8-v3": "sources/terrain-z8-v3.3333333333333333.pack", "base/6-1-1": "base/6-1-1.4444444444444444.sect"}"#;
        fs::write(root.join("state/build/manifest.json"), manifest).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let hand = d.path().join("handoff");
        let writes = super::super::seam::for_tests().writes();
        let mut out = buildstore::out::Out::open(&nas, &d.path().join("s"), STEP, writes).unwrap().handing_off(&hand);
        out.remove("sources/terrain-z8-v3");
        out.unpend("sources/terrain-z8-v3.3333333333333333.pack");
        out.save().unwrap();
        let hs = buildstore::handoff::written_in(&hand).unwrap().unwrap();
        assert_eq!(hs.len(), 1);
        // (Its upload, never verified, leaves the records' pending as the lead merges it.)
        assert_eq!(hs[0].checked, ["sources/terrain-z8-v3.3333333333333333.pack".to_string()]);
        assert_eq!(
            hs[0].changes,
            [("sources/terrain-z8-v3".to_string(), None)].into_iter().collect::<std::collections::BTreeMap<_, _>>()
        );
        assert_eq!(fs::read_to_string(root.join("state/build/manifest.json")).unwrap(), manifest);
        let mut out = buildstore::out::Out::open(&nas, &d.path().join("s"), STEP, writes).unwrap().handing_off(&hand);
        out.remove("base/6-1-1");
        assert!(out.save().is_err());
    }

    /// Task #679: the depth's targets as the agent's plan finds them (z3 tile 0/4, z6 tile 1/33).
    fn keep() -> DepthKeep {
        DepthKeep { at: crate::agent::jobs::now_s(), lo: [(0, 4)].into(), hi: [(1, 33)].into() }
    }

    /// Task #679: a depth pack goes only when no current target names its tile: a z6 tile's hi
    /// packs (depth, classes, shading) by it, a z3 tile's lo packs (the assembly's and the floor's
    /// shading's) by it; nothing else, whatever its tile.
    #[test]
    fn a_depth_pack_is_an_orphan_only_when_no_target_names_its_tile() {
        let k = keep();
        for l in ["layers/depth/hi/6-1-33", "layers/depth-class/hi/6-1-33", "layers/depth-shade/hi/6-1-33"] {
            assert!(!k.orphan(super::super::seam::for_tests(), l), "{l}");
        }
        for l in ["layers/depth/lo/3-0-4", "layers/depth-class/lo/3-0-4", "layers/depth-shade/lo/3-0-4"] {
            assert!(!k.orphan(super::super::seam::for_tests(), l), "{l}");
        }
        for l in [
            "layers/depth/hi/6-2-33",
            "layers/depth-class/hi/6-1-34",
            "layers/depth-shade/lo/3-1-4",
            "layers/depth/lo/3-0-5",
        ] {
            assert!(k.orphan(super::super::seam::for_tests(), l), "{l}");
        }
        // (Not a depth pack, or not a tile as the depth names them: never.)
        for l in [
            "layers/terrain/hi/6-2-33",
            "layers/depthx/hi/6-2-33",
            "layers/depth/hi/7-2-33",
            "layers/depth/lo/6-2-33",
            "layers/depth/hi/6-2",
            "layers/depth/root",
            "sources/depth/6-2-33",
        ] {
            assert!(!k.orphan(super::super::seam::for_tests(), l), "{l}");
        }
    }

    /// Task #679: the depth's targets are read only fresh; none there, none read (no depth name
    /// dropped); one stale or damaged, an error (said, no depth name dropped).
    #[test]
    fn the_depth_targets_are_read_only_fresh() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(DEPTH_KEEP_FILE);
        let now = crate::agent::jobs::now_s();
        assert!(DepthKeep::read(&p, now).unwrap().is_none());
        let k = keep();
        fs::write(&p, serde_json::to_vec(&k).unwrap()).unwrap();
        assert_eq!(DepthKeep::read(&p, now).unwrap(), Some(k.clone()));
        assert!(DepthKeep::read(&p, k.at + DEPTH_KEEP_FRESH_S + 1).is_err());
        fs::write(&p, b"{\"at\": ").unwrap();
        assert!(DepthKeep::read(&p, now).is_err());
    }

    /// Task #679: a depth job leased for a tile the coverage has left since keeps its packs; other
    /// steps' leases keep nothing.
    #[test]
    fn a_running_depth_job_keeps_its_tiles_packs() {
        let leased = [
            ("depth".to_string(), "6/2/33".to_string()),
            ("depth-lo".to_string(), "3/1/4".to_string()),
            ("depth-lo".to_string(), "3/5/5".to_string()),
            ("terrain-hi".to_string(), "6/9/9".to_string()),
            ("depth".to_string(), "3/7/7".to_string()),
        ];
        let k = keep().with_leased(&leased);
        for l in [
            "layers/depth/hi/6-2-33",
            "layers/depth-shade/lo/3-1-4",
            "layers/depth-class/lo/3-5-5",
            "layers/depth/hi/6-1-33",
        ] {
            assert!(!k.orphan(super::super::seam::for_tests(), l), "{l}");
        }
        for l in ["layers/depth/hi/6-9-9", "layers/depth/lo/3-7-7"] {
            assert!(k.orphan(super::super::seam::for_tests(), l), "{l}");
        }
    }

    /// Task #781: an upload the manifest no longer names leaves the records' pending unchecked (a
    /// replaced or dropped file: kept only by the pending, never verified); one it names, or an
    /// accepted inputs' index it names lists, stays for the check.
    #[test]
    fn pending_uploads_no_name_keeps_are_dropped() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        fs::create_dir_all(root.join("state/build")).unwrap();
        let (now, was) = ("base/6-1-1.2222222222222222.sect", "base/6-1-1.1111111111111111.sect");
        // (An accepted inputs' copy: named by no manifest entry, but by the index the manifest names.)
        let (index, copy) =
            ("sources/inputs/heritage/@index.3333333333333333.json", "sources/inputs/heritage/a.4444444444444444.txt");
        fs::create_dir_all(root.join("sources/inputs/heritage")).unwrap();
        let ix = serde_json::json!({"fmt": 1, "unit": "heritage", "checks": "heritage 1",
            "files": {"a.txt": {"file": copy, "size": 1, "keyed": "-", "facts": {}}}});
        fs::write(root.join(index), ix.to_string()).unwrap();
        let manifest = serde_json::json!({"base/6-1-1": now, "sources/inputs/heritage/@index": index});
        fs::write(root.join("state/build/manifest.json"), manifest.to_string()).unwrap();
        let ppath = root.join("state/build/pending.json");
        let pending = serde_json::json!({now: "a", was: "b", copy: "c"});
        fs::write(&ppath, pending.to_string()).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let dr = drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), None, true).unwrap();
        assert_eq!(dr, Dropped { retired: 0, depth: 0, unpended: 1 });
        assert_eq!(fs::read_to_string(&ppath).unwrap(), pending.to_string(), "a dry run writes nothing");
        assert_eq!(
            drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), None, false).unwrap().unpended,
            1
        );
        let p: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&ppath).unwrap()).unwrap();
        assert_eq!(p.keys().collect::<Vec<_>>(), [now, copy], "the copy the accepted index lists stays for the check");
        assert_eq!(
            drop_names(super::super::seam::for_tests(), &nas, &d.path().join("s"), None, false).unwrap(),
            Dropped::default(),
            "a no-op after"
        );
    }

    /// Task #781: the named uploads are checked on the NAS (here a shell on this Mac in its place):
    /// one there at the records' size leaves the pending, as does one there they hold no size of;
    /// one of another size, or not there, stays, said, nothing removed. Task #1001: by their sizes
    /// alone, nothing read but their metadata (the NAS had hashed 207 GB, its disks the whole budget).
    /// A dry run counts them, checking nothing.
    #[test]
    fn named_uploads_are_checked_and_only_those_there_whole_leave() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::create_dir_all(root.join("base")).unwrap();
        // (Task #1001 review: an odd name, a space and a quote in it, read back from `wc`'s lines.)
        let (good, no_size, short, gone) = (
            "base/6-1-1 it's.1111111111111111.sect",
            "base/6-1-4.4444444444444444.sect",
            "base/6-1-2.2222222222222222.sect",
            "base/6-1-3.3333333333333333.sect",
        );
        fs::write(root.join(good), b"good").unwrap();
        fs::write(root.join(no_size), b"named before sizes").unwrap();
        fs::write(root.join(short), b"cut").unwrap();
        let manifest =
            serde_json::json!({"base/6-1-1": good, "base/6-1-2": short, "base/6-1-3": gone, "base/6-1-4": no_size});
        fs::write(root.join("state/build/manifest.json"), manifest.to_string()).unwrap();
        let ppath = root.join("state/build/pending.json");
        let pending = serde_json::json!({good: "a", no_size: "b", short: "c", gone: "d"});
        fs::write(&ppath, pending.to_string()).unwrap();
        fs::write(root.join(buildstore::out::SIZES), serde_json::json!({good: 4, short: 11, gone: 1}).to_string())
            .unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let at = root.to_str().unwrap();
        let s = d.path().join("s");
        let v = verify_pending(super::super::seam::for_tests(), &nas, &s, &["sh", "-c"], at, true).unwrap();
        assert_eq!((v.ok, v.bad.len()), (4, 0));
        assert_eq!(fs::read_to_string(&ppath).unwrap(), pending.to_string(), "a dry run writes nothing");
        let v = verify_pending(super::super::seam::for_tests(), &nas, &s, &["sh", "-c"], at, false).unwrap();
        assert_eq!((v.ok, v.bad), (2, vec![short.to_string(), gone.to_string()]));
        let p: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&ppath).unwrap()).unwrap();
        assert_eq!(p.keys().collect::<Vec<_>>(), [short, gone]);
        assert!(root.join(good).exists() && root.join(short).exists());
        let m: BTreeMap<String, String> =
            serde_json::from_slice(&fs::read(root.join("state/build/manifest.json")).unwrap()).unwrap();
        assert_eq!(m.len(), 4, "nothing unnamed");
    }

    /// Task #1001: the check goes in batches of `VERIFY_CHUNK`, each saved and a safe point after it
    /// (on 7 Nov one call hashed 200 packs, tens of GB, with no safe point inside: the clean-up
    /// couldn't yield to the jobs it held back).
    #[test]
    fn verify_pending_goes_in_batches_each_a_safe_point() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::create_dir_all(root.join("base")).unwrap();
        let n = VERIFY_CHUNK + 3;
        let names: Vec<String> = (0..n).map(|i| format!("base/6-1-{i}.{i:016x}.sect")).collect();
        let manifest: BTreeMap<String, &String> =
            names.iter().enumerate().map(|(i, c)| (format!("base/6-1-{i}"), c)).collect();
        for c in &names {
            fs::write(root.join(c), b"x").unwrap();
        }
        fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let pending: BTreeMap<&String, &str> = names.iter().map(|c| (c, "-")).collect();
        let ppath = root.join("state/build/pending.json");
        fs::write(&ppath, serde_json::to_vec(&pending).unwrap()).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let before = YIELD_CHECKS.get();
        let v = verify_pending(
            super::super::seam::for_tests(),
            &nas,
            &d.path().join("s"),
            &["sh", "-c"],
            root.to_str().unwrap(),
            false,
        )
        .unwrap();
        assert_eq!((v.ok, v.bad.len()), (n, 0));
        assert_eq!(YIELD_CHECKS.get() - before, 2, "a safe point after each of its two batches");
        let p: BTreeMap<String, String> = serde_json::from_slice(&fs::read(&ppath).unwrap()).unwrap();
        assert!(p.is_empty(), "{p:?}");
    }

    /// Task #1001 review: a batch ends at `VERIFY_CHUNK` names or before its quoted names pass what
    /// one command takes on the NAS, whichever comes first; none is empty, and all are covered.
    #[test]
    fn verify_batches_end_at_their_count_or_their_bytes() {
        let short: Vec<(String, Option<u64>)> = (0..VERIFY_CHUNK * 2 + 1).map(|i| (format!("a/{i}"), None)).collect();
        let b = verify_batches(&short);
        assert_eq!(b.iter().map(|b| b.len()).collect::<Vec<_>>(), [VERIFY_CHUNK, VERIFY_CHUNK, 1]);
        // (Names of quotes, each four times its length quoted: the bytes end the batch first.)
        let long: Vec<(String, Option<u64>)> =
            (0..VERIFY_CHUNK).map(|i| (format!("{i:03}{}", "'".repeat(200)), None)).collect();
        let b = verify_batches(&long);
        assert!(b.len() > 1 && b.iter().all(|b| !b.is_empty()));
        assert_eq!(b.iter().map(|b| b.len()).sum::<usize>(), long.len());
        for b in &b {
            let joined = b.iter().map(|(c, _)| buildstore::out::sh_quoted(c)).collect::<Vec<_>>().join(" ");
            assert!(joined.len() <= buildstore::out::NAS_NAMES_MAX, "{}", joined.len());
        }
        assert!(verify_batches(&[]).is_empty());
    }

    /// A call on the NAS that hangs is stopped at its deadline (that child alone), said in `stalled`;
    /// the rest stays pending.
    #[test]
    fn verify_pending_stops_a_hung_call_and_keeps_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::create_dir_all(root.join("base")).unwrap();
        let name = "base/6-1-1.1111111111111111.sect";
        fs::write(root.join("state/build/manifest.json"), serde_json::json!({"base/6-1-1": name}).to_string()).unwrap();
        let ppath = root.join("state/build/pending.json");
        let pending = serde_json::json!({name: "00"});
        fs::write(&ppath, pending.to_string()).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        // (The probe, `true`, answers; the check's call sleeps: exec, so the PID signalled is the sleeper's.)
        let hung = ["sh", "-c", "[ \"$1\" = true ] || exec sleep 300", "sh"];
        let t = std::time::Instant::now();
        let v = verify_pending(
            super::super::seam::for_tests(),
            &nas,
            &d.path().join("s"),
            &hung,
            root.to_str().unwrap(),
            false,
        )
        .unwrap();
        store::testkit::ended_before(t, Duration::from_secs(300), "the hung call ends at its deadline");
        assert_eq!(v.ok, 0);
        assert!(v.stalled.as_deref().is_some_and(|s| s.contains("over 1 s")), "{:?}", v.stalled);
        assert_eq!(fs::read_to_string(&ppath).unwrap(), pending.to_string(), "still pending");
    }
}
