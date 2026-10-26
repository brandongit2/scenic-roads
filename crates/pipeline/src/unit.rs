//! base(U) (docs/steps.md): one z6 tile's base pack, from its OSM extract, by the z6 tile's programs run on
//! a z6 tile folder (`extract`, `elev`, `areaflags`, `tile … elev`, `scenic-metrics`), then its arrays as
//! a base pack (`legacy::base_sections`) for the ways the z6 tile owns (first
//! vertex inside it) that touch the coverage, with the pass's worldwide road values.
//!
//! Before the expensive steps the folder is cut down to the ways that touch the coverage (whoever
//! owns them: a way just outside still gives the clean-up its junction context at shared nodes).
//! The global-source layers come from the catalogue's packs (`stage`), and the roads' heights from the
//! finished terrain staged with them (crate::road_terrain).

use crate::coverage::Coverage;
use crate::legacy::TileId;
use anyhow::{bail, Context, Result};
use roadcore::WayRec;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Moves the scenic results z6 tiles kept in this Mac's own cache (`scenic-units/`) to the shared one
/// (each Mac's z6 tiles read it); how many were moved.
pub fn move_kept_to_shared(cache: &Path, shared: &Path) -> Result<usize> {
    let gone = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    let mut moved = 0;
    let local = cache.join("scenic-units");
    for e in std::fs::read_dir(&local).into_iter().flatten().flatten() {
        let (from, to) = (e.path(), shared.join("scenic-units").join(e.file_name()));
        if !from.is_dir() || store::whole::is_tmp(&from) {
            continue;
        }
        if !to.exists() {
            // (Named by this process too: another on this Mac may be copying the same folder.)
            let tmp = to.with_extension(format!("{}-{}.tmp", store::sys::host(), std::process::id()));
            std::fs::remove_dir_all(&tmp).ok();
            std::fs::create_dir_all(&tmp)?;
            let files = match std::fs::read_dir(&from) {
                Ok(rd) => rd,
                Err(e) if gone(&e) => {
                    std::fs::remove_dir_all(&tmp).ok();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            for f in files.flatten() {
                store::whole::copy(&f.path(), &tmp.join(f.file_name()))?;
            }
            if let Err(e) = std::fs::rename(&tmp, &to) {
                std::fs::remove_dir_all(&tmp).ok();
                if !to.exists() {
                    return Err(e.into());
                }
            }
        }
        match std::fs::remove_dir_all(&from) {
            Err(e) if gone(&e) => continue,
            r => r?,
        }
        moved += 1;
    }
    Ok(moved)
}

/// Rewrites a build folder's `ways.bin` and `verts.bin` keeping the ways `keep` says (in order);
/// run right after `extract`, before any per-vertex array exists. Returns (ways, vertices) kept.
pub fn subset(dir: &Path, keep: impl Fn(&WayRec, &[[i32; 2]]) -> bool) -> Result<(usize, usize)> {
    let w = roadcore::Ways::open(dir)?;
    let (ways, verts) = (w.ways(), w.verts());
    let mut out_ways: Vec<WayRec> = Vec::new();
    let mut out_verts: Vec<[i32; 2]> = Vec::new();
    for r in ways {
        let vs = &verts[r.vstart as usize..(r.vstart + r.vcount as u64) as usize];
        if keep(r, vs) {
            let mut n = *r;
            n.vstart = out_verts.len() as u64;
            out_verts.extend_from_slice(vs);
            out_ways.push(n);
        }
    }
    let mut head = Vec::with_capacity(16);
    head.extend_from_slice(roadcore::WAYS_MAGIC);
    head.extend_from_slice(&(out_ways.len() as u64).to_le_bytes());
    let mut wb = head;
    wb.extend_from_slice(bytemuck::cast_slice(&out_ways));
    drop(w);
    std::fs::write(roadcore::tmp(dir, "ways.bin"), &wb)?;
    std::fs::write(roadcore::tmp(dir, "verts.bin"), bytemuck::cast_slice(&out_verts))?;
    roadcore::commit(dir, &["ways.bin", "verts.bin"])?;
    Ok((out_ways.len(), out_verts.len()))
}

/// Whether the z6 tile with bounds `tb` (w, s, e, n, E7) owns a way starting at `p`.
pub fn owns(tb: [i32; 4], p: [i32; 2]) -> bool {
    p[0] >= tb[0] && p[0] < tb[2] && p[1] >= tb[1] && p[1] < tb[3]
}

/// What a z6 tile's programs read of the declared sources (crate::inputs::sources), in its folder
/// (`UnitSources::FILE`), written as the folder is prepared (`prepare_folder`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnitSources {
    /// The land-cover source (`landcover`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land_cover: Option<crate::landcover::Cover>,
    /// Each canopy file's URL by its name (scenic-metrics' canopy step: crate::trees::canopy_urls).
    #[serde(default)]
    pub canopy: BTreeMap<String, String>,
}

impl UnitSources {
    pub const FILE: &'static str = "sources.json";

    /// What the z6 tile's programs read of `s`.
    pub fn of(s: &crate::inputs::sources::Sources) -> Result<UnitSources> {
        use crate::inputs::sources::{CANOPY, LAND_COVER};
        Ok(UnitSources {
            land_cover: Some(crate::landcover::Cover::of(s.role(LAND_COVER)?)?),
            canopy: crate::trees::canopy_urls(s.role(CANOPY)?)?,
        })
    }

    /// Folder `dir`'s.
    pub fn read(dir: &Path) -> Result<UnitSources> {
        let p = dir.join(Self::FILE);
        serde_json::from_slice(
            &std::fs::read(&p)
                .with_context(|| format!("{} (the unit job writes it from the declared sources)", p.display()))?,
        )
        .with_context(|| p.display().to_string())
    }
}

/// Where a z6 tile's steps find their programs and caches.
#[derive(Clone)]
pub struct Tools {
    /// `extract`, `tile`, `scenic-metrics`.
    pub bin: PathBuf,
    /// This Mac's caches: `chm10/` (canopy 10° files); and the z6 tiles' kept results when there's
    /// no `shared`.
    pub cache: PathBuf,
    /// Overture building boxes: a folder of `.f32` files (the z6 tile's tiles staged by
    /// `buildtiles::stage`), when there are any.
    pub buildings: Option<PathBuf>,
    /// The NAS's `sources/`, where downloads are kept, each downloaded once: Meta's canopy squares
    /// (`canopy/`); the local caches fill from it. None: local caches alone.
    pub sources: Option<PathBuf>,
    /// The NAS's `cache/`, where what a z6 tile keeps for its later runs is shared by every Mac: its
    /// scenic results (`scenic-units/`). None: the local cache.
    pub shared: Option<PathBuf>,
    /// The canopy squares read where they lie (the NAS's `sources/canopy/`), for a worker that
    /// keeps no copies of them (a task's); None: this Mac's copies (`cache/chm10/`), from the NAS.
    pub chm: Option<PathBuf>,
    /// The NAS's stores (`sources/`) only read: a task's worker reads them where they lie, and
    /// neither downloads into them nor removes from them (docs/workers.md §2); its programs are
    /// told so (`SCENIC_STORES_READ_ONLY`).
    pub stores_read_only: bool,
    /// The declared elevation datasets' accepted version (crate::inputs::elevation): a z6 tile's
    /// folder is prepared only with one (None: a task's worker, which runs the tail alone).
    pub elevation: Option<crate::inputs::Accepted>,
    /// The declared sources' accepted version (crate::inputs::sources): what a z6 tile's programs read
    /// of them is written into its folder (`UnitSources`); a folder is prepared only with them (None:
    /// a task's worker, which runs the tail alone).
    pub declared: Option<crate::inputs::sources::Sources>,
    /// Where to keep a copy of the folder before and after each of the steps' programs, with the
    /// command that ran it (`<n> <step> before|after` and `….cmd`): `scenic-build unit-snap`, for
    /// running the steps again elsewhere (natively at other thread counts, as WebAssembly) and
    /// comparing bytes (tools/check/same.py). Copies are APFS clones: no space until they differ.
    pub snap: Option<PathBuf>,
}

impl Tools {
    /// Where z6 tile `u`'s scenic results are kept between its runs (crate::scache::Carry).
    pub fn scenic_kept(&self, u: TileId) -> PathBuf {
        self.shared.as_ref().unwrap_or(&self.cache).join("scenic-units").join(u.dash())
    }
}

/// Where a z6 tile's build says how far it is (`on_stage`): each stage it finishes (a lap, a program)
/// with the time it took, and a program's own progress within its stage (its `progress:` lines),
/// as (stage, fraction of it done, its time when finished).
pub type StageHook = Box<dyn Fn(&str, f64, Option<std::time::Duration>) + Send + Sync>;

static HOOK: std::sync::RwLock<Option<StageHook>> = std::sync::RwLock::new(None);

/// Has a z6 tile's stages said to `h` from now on (None: to nothing): the `unit` job's progress
/// (scenic-build).
pub fn on_stage(h: Option<StageHook>) {
    if let Ok(mut w) = HOOK.write() {
        *w = h;
    }
}

/// Says how far a stage is (`on_stage`).
pub fn stage_said(stage: &str, frac: f64, took: Option<std::time::Duration>) {
    if let Ok(r) = HOOK.read() {
        if let Some(h) = r.as_ref() {
            h(stage, frac, took);
        }
    }
}

/// A z6 tile's stages in the order they run (`Laps::lap`'s and the programs' names), each with about
/// how long it takes (seconds: the M4's, over its z6 tiles of 2026-10): how far a z6 tile is, by
/// the stages it's through (`TileProgress`), until this Mac has timed its own.
pub const STAGES: [(&str, f64); 25] = [
    ("piece copied from the NAS", 10.0),
    ("buildings staged", 1.0),
    ("extract", 10.0),
    ("subset to the coverage", 1.0),
    ("DEM cache slice", 30.0),
    ("elevations (elev)", 150.0),
    ("DEM samples kept", 40.0),
    ("layers staged from the packs", 60.0),
    ("heritage inputs", 8.0),
    ("area flags (areaflags)", 5.0),
    ("land cover (landcover)", 4.0),
    ("scenic results restored", 20.0),
    ("clean-up and grade (tile elev)", 1.0),
    ("scenic prep", 1.0),
    ("scenic canopy", 25.0),
    ("scenic view", 12.0),
    ("scenic buildings", 19.0),
    ("scenic flags", 1.0),
    ("missing grids written", 2.0),
    ("scenic results kept", 8.0),
    ("base pack made", 1.0),
    ("base pack written", 1.0),
    ("road values made and written", 20.0),
    // (With the grids' and base pack's uploads, put off until here.)
    ("records saved", 60.0),
    ("its folders removed", 1.0),
];

/// A `unit` job's progress (scenic-build's `unit` step), z6 tile by z6 tile: those finished, and each under
/// way by how far through its stages it is (`STAGES`), each stage weighted by about how long it
/// takes here (learned as z6 tiles finish, and kept: `file`). Its line says `<done>/<n> z6 tiles`.
pub struct TileProgress {
    n: usize,
    /// The z6 tiles finished (a stage said after, such as its folders' removal, counts for nothing).
    finished: std::collections::BTreeSet<String>,
    /// The z6 tiles under way (prepared here, or their last steps out with other workers): how far each is.
    under_way: BTreeMap<String, f64>,
    /// The z6 tile whose stages are said now.
    current: Option<String>,
    /// About how long each stage takes here (seconds), and those this Mac has timed (kept or now).
    secs: BTreeMap<String, f64>,
    learned: std::collections::BTreeSet<String>,
    file: Option<PathBuf>,
    said_at: Option<std::time::Instant>,
}

impl TileProgress {
    /// `n` z6 tiles, their stages' times learned in `file` (when there is one).
    pub fn new(n: usize, file: Option<PathBuf>) -> TileProgress {
        let mut secs: BTreeMap<String, f64> = STAGES.iter().map(|(s, t)| (s.to_string(), *t)).collect();
        let mut learned = std::collections::BTreeSet::new();
        if let Some(kept) = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice::<BTreeMap<String, f64>>(&b).ok())
        {
            for (s, t) in
                kept.into_iter().filter(|(s, t)| STAGES.iter().any(|x| x.0 == s) && t.is_finite() && *t >= 0.0)
            {
                learned.insert(s.clone());
                secs.insert(s, t);
            }
        }
        TileProgress {
            n,
            finished: Default::default(),
            under_way: BTreeMap::new(),
            current: None,
            secs,
            learned,
            file,
            said_at: None,
        }
    }

    /// About how long `stages` take here together (seconds), when this Mac has timed each.
    pub fn here_s(&self, stages: &[&str]) -> Option<f64> {
        stages.iter().map(|s| self.learned.contains(*s).then(|| self.secs.get(*s).copied()).flatten()).sum()
    }

    /// Stages said from now on are z6 tile `u`'s.
    pub fn on(&mut self, u: &str) {
        self.current = Some(u.to_string());
    }

    /// The current z6 tile's `stage` is `frac` done (1: finished, in `took`): it counts that far, and
    /// the time teaches this Mac's weight for the stage.
    pub fn stage(&mut self, stage: &str, frac: f64, took: Option<std::time::Duration>) {
        let Some(i) = STAGES.iter().position(|(s, _)| *s == stage) else { return };
        let Some(u) = self.current.clone() else { return };
        if self.finished.contains(&u) {
            return;
        }
        if let (Some(t), true) = (took, frac >= 1.0) {
            let e = self.secs.entry(stage.to_string()).or_insert(t.as_secs_f64());
            *e = 0.75 * *e + 0.25 * t.as_secs_f64();
            self.learned.insert(stage.to_string());
        }
        let w = |s: &str| self.secs.get(s).copied().unwrap_or(1.0).max(0.1);
        let total: f64 = STAGES.iter().map(|(s, _)| w(s)).sum();
        let before: f64 = STAGES[..i].iter().map(|(s, _)| w(s)).sum();
        // (Never back: a stage said again, or one a z6 tile skipped, is no step back.)
        let f = ((before + frac.clamp(0.0, 1.0) * w(stage)) / total).min(0.999);
        let e = self.under_way.entry(u).or_insert(0.0);
        *e = e.max(f);
        self.say(frac >= 1.0);
    }

    /// z6 tile `u` is done (built and saved, or nothing to build).
    pub fn finished(&mut self, u: &str) {
        self.under_way.remove(u);
        self.finished.insert(u.to_string());
        self.say(true);
        self.keep();
    }

    /// The z6 tiles done, counting each under way by how far it is.
    pub fn done(&self) -> f64 {
        (self.finished.len() as f64 + self.under_way.values().sum::<f64>()).min(self.n as f64)
    }

    /// The progress line (`crate::agent::jobs::report_f`): at once at a stage's end, else at most
    /// once a second.
    pub fn say(&mut self, now: bool) {
        if !now && self.said_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1)) {
            return;
        }
        self.said_at = Some(std::time::Instant::now());
        crate::agent::jobs::report_f(self.done(), self.n as u64, "z6 tiles");
    }

    /// The stages' times learned, kept for this Mac's next job (a cache: not keeping them costs
    /// nothing but the weights).
    fn keep(&self) {
        if let Some(f) = &self.file {
            if let Ok(b) = serde_json::to_vec(&self.secs) {
                store::whole::write(f, &b).ok();
            }
        }
    }
}

/// A part of a z6 tile's build done in-process: a phase of the job's (crate::timings), logged as its
/// programs' times are, "  <what>: <time>", and said as a stage finished (`on_stage`) when it
/// `end`s. (Ended by an error instead, its time is the phase's alone.)
pub struct Stage {
    what: String,
    t: std::time::Instant,
    _p: crate::timings::Phase,
}

/// Starts stage `what` of class `class`.
pub fn stage(what: &str, class: crate::timings::Class) -> Stage {
    Stage { what: what.to_string(), t: std::time::Instant::now(), _p: crate::timings::phase(what, class) }
}

impl Stage {
    pub fn end(self) {
        let took = self.t.elapsed();
        eprintln!("  {}: {took:.0?}", self.what);
        stage_said(&self.what, 1.0, Some(took));
    }
}

/// What a z6 tile's step program mostly waits on, for its phase (crate::timings): its elevations read
/// the DEM servers' files (and say their own phases), the land cover downloads ESA's, the canopy
/// reads the canopy squares; the rest compute over the z6 tile's folder.
pub fn program_class(what: &str) -> crate::timings::Class {
    use crate::timings::Class;
    match what {
        "elevations (elev)" => Class::Mixed,
        "land cover (landcover)" => Class::Net,
        _ => Class::Compute,
    }
}

/// The most memory (resident, bytes) one of the steps' programs took since `take_peak` last read it.
static PEAK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The most memory one of the steps' programs took since the last call (scenic-build's log line per
/// z6 tile), and starts again.
pub fn take_peak() -> u64 {
    PEAK.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// The folder copied (cloned) to `<snap>/<n> <what> <when>`, with the command in `….cmd` (program,
/// then `arg` and `env` lines).
fn snapshot(snap: &Path, n: usize, c: &Command, dir: &Path, what: &str, when: &str) -> Result<()> {
    let to = snap.join(format!("{n:02} {what} {when}"));
    std::fs::create_dir_all(snap)?;
    let st = store::spawn::status(store::spawn::command("cp").arg("-c").arg("-R").arg(dir).arg(&to))?;
    anyhow::ensure!(st.success(), "clone {} to {}", dir.display(), to.display());
    let mut cmd = format!("prog {}\n", c.get_program().to_string_lossy());
    for a in c.get_args() {
        cmd.push_str(&format!("arg {}\n", a.to_string_lossy()));
    }
    for (k, v) in c.get_envs() {
        cmd.push_str(&format!(
            "env {}={}\n",
            k.to_string_lossy(),
            v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default()
        ));
    }
    std::fs::write(snap.join(format!("{n:02} {what} {when}.cmd")), cmd)?;
    Ok(())
}

/// A step's program run in `dir` (logged to `log`), its folder snapshotted around it when
/// `tools.snap` asks.
fn run_in(c: Command, what: &str, log: &Path, dir: &Path, tools: &Tools) -> Result<()> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let Some(snap) = &tools.snap else { return run(c, what, log) };
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    snapshot(snap, n, &c, dir, what, "before")?;
    // (A copy of the command for the snapshot after: `run` takes it.)
    let mut shown = store::spawn::command(c.get_program());
    shown.args(c.get_args());
    for (k, v) in c.get_envs() {
        if let Some(v) = v {
            shown.env(k, v);
        }
    }
    run(c, what, log)?;
    snapshot(snap, n, &shown, dir, what, "after")
}

/// A z6 tile's step ended by a server it reads giving no answer (its program's exit status
/// crate::fetch::UNREACHABLE_EXIT): the network's or the server's, not the z6 tile's. The `unit` job sets
/// the z6 tile aside and goes on with its others (scenic-build's `unit` step).
#[derive(Debug)]
pub struct SourceDown(pub String);

impl std::fmt::Display for SourceDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceDown {}

/// Whether `e` is a server that gave no answer: a step's (`SourceDown`), or a request of this
/// process's (crate::fetch::is_unreachable).
pub fn source_down(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<SourceDown>()) || crate::fetch::is_unreachable(e)
}

fn run(mut c: Command, what: &str, log: &Path) -> Result<()> {
    let f = std::fs::File::options().create(true).append(true).open(log)?;
    // (Its phase, the program's own phases under it where it says them.)
    let _p = crate::timings::phase(what, program_class(what));
    crate::timings::child(&mut c);
    // (What it and its programs read and open, counted in a folder of their own inside this
    // process's, seen as it runs: added to this process's as it ends, store::nasio.)
    let counts = store::nasio::program_dir(Path::new(&format!("{}.counts", log.display())));
    c.env(store::nasio::ENV, &counts);
    let t = std::time::Instant::now();
    let mut child = store::spawn::spawn(c.stdout(f.try_clone()?).stderr(std::process::Stdio::piped()))
        .with_context(|| format!("start {what}"))?;
    // Its errors into its log as they come, and how far it says it is passed on as its stage's
    // (a bar's redraws only that).
    let err = child.stderr.take();
    let name = what.to_string();
    let read = std::thread::spawn(move || {
        let mut f = f;
        if let Some(err) = err {
            crate::agent::jobs::each_line(err, |l| {
                let frac = crate::agent::jobs::fraction_of(l);
                if let Some(x) = frac {
                    stage_said(&name, x, None);
                }
                if frac.is_none() || !l.trim_start().starts_with('[') {
                    std::io::Write::write_all(&mut f, format!("{l}\n").as_bytes()).ok();
                }
            });
        }
    });
    // Waited for with what it used: its (and its programs') peak memory.
    let waited = crate::sys::wait_with_peak(child).with_context(|| format!("wait for {what}"));
    read.join().ok();
    store::nasio::take_program(&counts);
    let (st, peak) = waited?;
    PEAK.fetch_max(peak, std::sync::atomic::Ordering::Relaxed);
    if !st.success() {
        // The end of its log into the job's: the z6 tile's folder, its log with it, goes when the next
        // job starts.
        eprintln!("{what}, the end of its log:\n{}", log_tail(log, 40));
        if st.code() == Some(crate::fetch::UNREACHABLE_EXIT as i32) {
            return Err(
                SourceDown(format!("{what}: a server it reads gave no answer ({st}); see {}", log.display())).into()
            );
        }
        bail!("{what} failed ({st}); see {}", log.display());
    }
    eprintln!("  {what}: {:.0?}, {} MB at most", t.elapsed(), peak >> 20);
    stage_said(what, 1.0, Some(t.elapsed()));
    Ok(())
}

/// The last `n` lines of a log (at most its last 64 KB).
fn log_tail(log: &Path, n: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut b = Vec::new();
    if let Ok(mut f) = std::fs::File::open(log) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if f.seek(SeekFrom::Start(len.saturating_sub(64 << 10))).is_ok() {
            f.read_to_end(&mut b).ok();
        }
    }
    let s = String::from_utf8_lossy(&b);
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub unit: String,
    pub piece_ways: usize,
    pub kept_ways: usize,
    pub kept_verts: usize,
    pub owned: usize,
    pub staged: crate::stage::Staged,
    /// Heritage sites and designated-area polygons around the z6 tile.
    pub heritage: usize,
    pub areas: usize,
}

/// Writes a z6 tile's heritage inputs for its box (degrees) into its folder: `heritage.json` and
/// `area-shapes.geojsonseq` (crate::heritage::unit_inputs); the sites and polygons written.
pub type HeritageInputs<'a> = &'a dyn Fn([f64; 4], &Path) -> Result<(usize, usize)>;

/// Runs the z6 tile's programs for z6 tile `u` in `dir` from `piece`, with the coverage and the global-source
/// layers on the NAS (`src`), and its scenic results from its last run (`carry`): `prepare_folder`,
/// then its `tail`. Leaves the z6 tile folder ready for its base pack.
#[allow(clippy::too_many_arguments)]
pub fn build_folder(
    u: TileId,
    piece: &Path,
    dir: &Path,
    cov: &Coverage,
    src: &crate::stage::Source,
    tools: &Tools,
    heritage: HeritageInputs,
    carry: Option<&crate::scache::Carry>,
) -> Result<Report> {
    let rep = prepare_folder(u, piece, dir, cov, src, tools, heritage, carry)?;
    if rep.kept_ways > 0 {
        run_tail(&tail(u, tools.buildings.is_some(), tools.sources.is_some()), dir, tools)?;
    }
    Ok(rep)
}

/// A z6 tile's build up to its tail (docs/workers.md: what needs the NAS): its ways from the extract,
/// those touching the coverage, the global-source layers staged from the packs with the deep terrain
/// under its vertices and the datasets' codes (what its elevations read: crate::road_terrain), the
/// heritage inputs, area flags and land cover; then its scenic results from its last run restored
/// (`carry`), as the tail's canopy and view steps read them.
#[allow(clippy::too_many_arguments)]
pub fn prepare_folder(
    u: TileId,
    piece: &Path,
    dir: &Path,
    cov: &Coverage,
    src: &crate::stage::Source,
    tools: &Tools,
    heritage: HeritageInputs,
    carry: Option<&crate::scache::Carry>,
) -> Result<Report> {
    std::fs::create_dir_all(dir)?;
    let log = dir.join("steps.log");
    let mut rep = Report { unit: u.slash(), ..Default::default() };
    // 1. Every way of the extract, densified by the declared elevation datasets (crate::dem::spacing).
    let a = tools.elevation.as_ref().context("the unit's elevations: the declared elevation datasets' accepted version isn't given (the build reads them once their switch is recorded: docs/inputs.md §6.8)")?;
    let spacing = dir.join(crate::dem::spacing::FILE);
    let accepted = crate::dem::declared::accepted(a)?;
    let fine = crate::dem::spacing::Fine::of_accepted(&accepted);
    fine.write(&spacing)?;
    let mut c = store::spawn::command(tools.bin.join("extract"));
    c.arg(dir).arg(&spacing).arg(piece);
    run_in(c, "extract", &log, dir, tools)?;
    use crate::timings::Class;
    let s = stage("subset to the coverage", Class::Compute);
    rep.piece_ways = roadcore::Ways::open(dir)?.ways().len();
    // 2. Only what touches the coverage goes on.
    let (kw, kv) = subset(dir, |_, vs| cov.touches(vs))?;
    (rep.kept_ways, rep.kept_verts) = (kw, kv);
    if kw == 0 {
        s.end();
        return Ok(rep);
    }
    let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
    {
        let w = roadcore::Ways::open(dir)?;
        let verts = w.verts();
        rep.owned = w.ways().iter().filter(|r| owns(tb, verts[r.vstart as usize])).count();
    }
    s.end();
    let declared = tools
        .declared
        .as_ref()
        .context("the unit's sources: the declared sources' accepted version isn't given (docs/inputs.md §6.9)")?;
    std::fs::write(dir.join(UnitSources::FILE), serde_json::to_vec(&UnitSources::of(declared)?)?)?;
    // 4. The global-source layers the steps read, from the packs.
    let s = stage("layers staged from the packs", Class::NasRead);
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    // (With the terrain the roads' heights read: the deep tiles under the vertices; and the
    // datasets' codes.)
    let deep = crate::road_terrain::deep_tiles(roadcore::Ways::open(dir)?.verts(), &fine);
    rep.staged = crate::stage::stage(src, b, &deep, dir)?;
    crate::road_terrain::Codes::of(&accepted).write(&dir.join(crate::road_terrain::CODES_FILE))?;
    s.end();
    // The heritage sites around the z6 tile (the flags step's `heritage.json`), and the designated
    // areas rasterised onto its grid (`grid.areas.u8`), from the heritage-sites job's slices.
    let s = stage("heritage inputs", Class::NasRead);
    (rep.heritage, rep.areas) = heritage(b, dir)?;
    s.end();
    let mut c = store::spawn::command(tools.bin.join("areaflags"));
    c.arg(dir).arg(dir.join("area-shapes.geojsonseq"));
    run_in(c, "area flags (areaflags)", &log, dir, tools)?;
    // Land cover the packs lack (new coverage): the declared land-cover source for those grid tiles
    // only; the rest stays as staged.
    if rep.staged.missing.get("class").copied().unwrap_or(0) > 0 {
        let mut c = store::spawn::command(tools.bin.join("landcover"));
        c.arg(dir).arg("--only").arg(dir.join("grid.class.missing.u32"));
        run_in(c, "land cover (landcover)", &log, dir, tools)?;
    }
    // Its last run's scenic results (pipeline::scache: the canopy and view steps copy what's
    // unchanged), restored over the staged grids they were made from.
    if let Some(c) = carry {
        let s = stage("scenic results restored", Class::NasRead);
        // (From the copy made ahead of it, while the z6 tile before it built: `ahead_carry`.)
        let ahead = ahead_carry(dir);
        match c.restore_ahead(dir, ahead.is_dir().then_some(ahead.as_path())) {
            Ok(Some(n)) => eprintln!("unit {}: {n} samples' scenic results from its last run", u.slash()),
            Ok(None) => {}
            // (Without the cache's record the steps start afresh, whatever was copied.)
            Err(e) => {
                eprintln!("unit {}: its last run's scenic results not used: {e:#}", u.slash());
                std::fs::remove_dir_all(crate::scache::unit_dir(dir)).ok();
            }
        }
        s.end();
    }
    Ok(rep)
}

/// Where the `unit` job copies the scenic results kept for the z6 tile whose folder is `dir`, ahead of
/// it (scache::Carry::fetch): beside its folder.
pub fn ahead_carry(dir: &Path) -> PathBuf {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dir.with_file_name(format!("{name}-carry"))
}

/// One program a task runs (docs/workers.md §3): its name (the build's `bin/<prog>`, or
/// `<prog>.wasm` in a web worker), its arguments and environment, in which `{dir}` is the z6 tile's
/// folder, `{cache}` the canopy cache, `{scache}` the folder's scenic cache, `{buildings}` its
/// roadside buildings, `{store}` the NAS's canopy store to download into, and what's read where it
/// lies: `{sources}` the NAS's sources, `{chm}` its canopy squares, `{net}`
/// the DEM servers' files (crate::offload::places; an environment variable whose value names a
/// place the worker doesn't have is left out).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    pub what: String,
    pub prog: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The files it reads of the z6 tile's own (`{dir}/<name>`, `{buildings}/<name>`; a trailing `*`
    /// for any name so begun), earlier steps' outputs too: a task sends those there when it
    /// starts, so it may run on any worker (crate::offload). None: it stays here (`keep_here`).
    #[serde(default)]
    pub reads: Vec<String>,
}

/// The files a `unit` job saves for a z6 tile (its tile's dash name: `6-31-20`): its base pack, road
/// values, roads' English and the grids its packs lacked (scenic-build's `commit_unit`). All a
/// helper's hand-off may change (crate::coord).
pub fn saved_files(dash: &str) -> [String; 7] {
    [
        "base",
        "global/roads",
        "global/roaden",
        "global/roadids",
        "layers/grid-class/hi",
        "layers/grid-canopy/hi",
        "layers/grid-cover/hi",
    ]
    .map(|p| format!("{p}/{dash}"))
}

/// A tail split: the runs that stay here, then those any worker may run (what they read of the
/// z6 tile's folder listed: it's sent them), the longest such end.
pub fn split(runs: &[Run]) -> (&[Run], &[Run]) {
    let i = runs.iter().rposition(|r| r.reads.is_empty()).map_or(0, |i| i + 1);
    runs.split_at(i)
}

/// The steps through `what` kept here: neither it nor those before go to another worker.
pub fn keep_here(runs: &mut [Run], what: &str) {
    if let Some(i) = runs.iter().position(|r| r.what == what) {
        for r in &mut runs[..=i] {
            r.reads.clear();
        }
    }
}

/// The 10° canopy squares the canopy step reads for grid tiles `tiles` (those of each tile's
/// corners), by (top latitude, left longitude).
pub fn canopy_squares(tiles: &[[u32; 2]]) -> std::collections::BTreeSet<(i32, i32)> {
    use det::Det;
    let mut need = std::collections::BTreeSet::new();
    for t in tiles {
        for (dx, dy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = ((t[0] + dx) as f64 * 256.0, (t[1] + dy) as f64 * 256.0);
            let lon = x / roadcore::grid::WORLD * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / roadcore::grid::WORLD)).dsinh().datan().to_degrees();
            need.insert(((lat / 10.0).ceil() as i32 * 10, (lon / 10.0).floor() as i32 * 10));
        }
    }
    need
}

/// A canopy square's files, its median, p95 and cover (named by its top and left).
pub fn canopy_square_files((top, left): (i32, i32)) -> [String; 3] {
    ["median", "p95", "cover5m"].map(|st| format!("meta_chm_lat={top}.0_lon={left}.0_{st}.tif"))
}

/// Whether the NAS's store (`<sources>/canopy/`) has every canopy square the canopy step of the
/// z6 tile in `dir` reads (a file, or Meta's "none there"): a worker reads them where they lie, and
/// can't download one.
pub fn canopy_stored(dir: &Path, sources: &Path) -> bool {
    let Ok(idx) = roadcore::grid::GridIndex::load(dir) else { return false };
    canopy_squares(&idx.tiles)
        .into_iter()
        .all(|sq| canopy_square_files(sq).iter().all(|n| sources.join("canopy").join(n).exists()))
}

/// The tail of z6 tile `u`'s build: its elevations, its clean-up and grade, then its road samples,
/// canopy, views, buildings (with `buildings`) and flags. Every program is Rust, and gives the same
/// bytes natively and as WebAssembly (tools/check/same.py, tail.mjs), so any worker can run it. The
/// data the elevations and canopy read is read where it lies: the DEM servers' files and the NAS's
/// (`{net}`, `{sources}`, `{chm}`: a browser's through the coordinator, a Mac's as this
/// Mac reads them).
pub fn tail(u: TileId, buildings: bool, store: bool) -> Vec<Run> {
    // What each step reads of the z6 tile's folder, earlier steps' outputs too: a task sends what's
    // there when it starts (traced in WebAssembly, the outputs compared: tools/check/tail.mjs).
    let mine = |names: &[&str]| {
        names
            .iter()
            .map(|n| if n.starts_with('{') { n.to_string() } else { format!("{{dir}}/{n}") })
            .collect::<Vec<_>>()
    };
    let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
    let own = format!("{},{},{},{}", tb[0], tb[1], tb[2], tb[3]);
    let scenic = |step: &str, with_own: bool| {
        let mut e = Vec::new();
        if with_own {
            e.push(("SCENIC_OWN".to_string(), own.clone()));
        }
        e.push(("SCENIC_CACHE".to_string(), "{cache}".to_string()));
        e.push(("SCENIC_SCACHE".to_string(), "{scache}".to_string()));
        if store {
            e.push(("SCENIC_CANOPY_STORE".to_string(), "{store}".to_string()));
        }
        // (The canopy squares read where they lie: the NAS's, for a worker that has no copies.)
        if step == "canopy" {
            e.push(("SCENIC_CHM".to_string(), "{chm}".to_string()));
        }
        let mut args = vec!["{dir}".to_string(), step.to_string()];
        if step == "buildings" {
            args.push("{buildings}".into());
        }
        let reads = match step {
            "prep" => mine(&["ways.bin", "verts.bin", "final.u16", "terrain.tiles"]),
            // (Its last run's results, restored: scache::Carry.)
            "canopy" => mine(&[
                "grid.idx",
                "grid.terrain.i16",
                "samples.bin",
                "scache/canopy*",
                "scache/changed.tiles",
                "near.i8",
                "roadside.u8",
                "grid.canopy.u8",
                "grid.cover.u8",
            ]),
            "view" => mine(&[
                "grid.idx",
                "grid.terrain.i16",
                "grid.canopy.u8",
                "grid.class.u8",
                "grid.areas.u8",
                "samples.bin",
                "near.i8",
                "roadside.u8",
                "pois.json",
                "heritage.json",
                "ways.bin",
                "verts.bin",
                "scache/view*",
                "scache/changed.tiles",
                "samples.metrics.u8",
            ]),
            "buildings" => mine(&["samples.bin", "ways.bin", "verts.bin", "{buildings}/*"]),
            "flags" => mine(&[
                "grid.idx",
                "grid.terrain.i16",
                "grid.areas.u8",
                "samples.bin",
                "samples.metrics.u8",
                "samples.bld.u8",
                "pois.json",
                "heritage.json",
                "ways.bin",
                "verts.bin",
            ]),
            _ => Vec::new(),
        };
        Run { what: format!("scenic {step}"), prog: "scenic-metrics".into(), args, env: e, reads }
    };
    let elev = Run {
        what: "elevations (elev)".into(),
        prog: "elev".into(),
        args: vec!["{dir}".into()],
        env: Vec::new(),
        reads: mine(&["verts.bin", "terrain.tiles", "fine.tiles", "codes.json"]),
    };
    let grade = Run {
        what: "clean-up and grade (tile elev)".into(),
        prog: "tile".into(),
        args: vec!["{dir}".into(), "elev".into()],
        env: Vec::new(),
        reads: mine(&[
            "ways.bin",
            "verts.bin",
            "elev.f32",
            "strings.txt",
            "causeways.i64",
            "grid.idx",
            "grid.class.u8",
        ]),
    };
    let mut runs = vec![elev, grade];
    for step in ["prep", "canopy", "view"] {
        runs.push(scenic(step, true));
    }
    if buildings {
        runs.push(scenic("buildings", false));
    }
    runs.push(scenic("flags", false));
    runs
}

/// Runs a task's programs here, natively, in `dir` (snapshotted around each when `tools.snap`
/// asks).
pub fn run_tail(runs: &[Run], dir: &Path, tools: &Tools) -> Result<()> {
    let log = dir.join("steps.log");
    let scache = crate::scache::unit_dir(dir);
    let store = tools.sources.as_ref().filter(|_| !tools.stores_read_only).map(|s| s.join("canopy"));
    // (A place this worker hasn't, or that no worker here fills, `{net}`: its variable left out.)
    let fill = |v: &str| -> Option<String> {
        let put = |s: &str, k: &str, p: Option<&Path>| -> Option<String> {
            if s.contains(k) {
                Some(s.replace(k, &p?.to_string_lossy()))
            } else {
                Some(s.to_string())
            }
        };
        let v = put(v, "{dir}", Some(dir))?;
        let v = put(&v, "{cache}", Some(&tools.cache))?;
        let v = put(&v, "{scache}", Some(&scache))?;
        let v = put(&v, "{buildings}", tools.buildings.as_deref())?;
        let v = put(&v, "{sources}", tools.sources.as_deref())?;
        let v = put(&v, "{chm}", tools.chm.as_deref())?;
        let v = put(&v, "{store}", store.as_deref())?;
        (!v.contains("{net}")).then_some(v)
    };
    for r in runs {
        let mut c = store::spawn::command(tools.bin.join(&r.prog));
        for a in &r.args {
            c.arg(fill(a).with_context(|| format!("{}: no place for {a}", r.what))?);
        }
        for (k, v) in &r.env {
            if let Some(v) = fill(v) {
                c.env(k, v);
            }
        }
        if tools.stores_read_only {
            c.env("SCENIC_STORES_READ_ONLY", "1");
        }
        run_in(c, &r.what, &log, dir, tools)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a z6 tile's spawned program reads (counted in its own folder, store::nasio) lands in the
    /// z6 tile's measures, as its tail's steps run here (task #235).
    #[test]
    fn a_spawned_steps_reads_land_on_its_unit() {
        let _l = crate::costin::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let mut c = store::spawn::command("sh");
        c.arg("-c").arg(counts_sh(5_000_000_000, 7));
        crate::costin::begin();
        crate::costin::set("km2", 1);
        run(c, "a step", &d.path().join("step.log")).unwrap();
        let m = crate::costin::take();
        assert!(m["bytes"] >= 5_000_000_000 && m["tiffs"] >= 7, "{m:?}");
        assert!(!d.path().join("step.log.counts").exists());
        // (A tail's, after its z6 tile's measures were taken.)
        let mut c = store::spawn::command("sh");
        c.arg("-c").arg(counts_sh(300, 2));
        let (r, t) = crate::costin::measured(|| run(c, "a tail step", &d.path().join("tail.log")));
        r.unwrap();
        assert!(t.bytes >= 300 && t.tiffs >= 2);
        // (Its programs' CPU as each is waited for: a busy one's counted, and added to the z6 tile's.)
        let mut c = store::spawn::command("sh");
        c.arg("-c").arg("i=0; while [ $i -lt 200000 ]; do i=$((i+1)); done");
        let (r, t) = crate::costin::measured(|| run(c, "a busy tail step", &d.path().join("busy.log")));
        r.unwrap();
        let ms = t.cpu_ms.unwrap();
        assert!(ms >= 50, "{t:?}");
        let mut m = BTreeMap::from([("cpu_ms".to_string(), 7u64)]);
        crate::costin::add_to(&mut m, t);
        assert_eq!(m["cpu_ms"], 7 + ms);
    }

    /// A shell line writing a counts file (store::nasio) of `read` NAS bytes and `tiffs` GeoTIFFs
    /// into its folder, as a program counting them would.
    fn counts_sh(read: u64, tiffs: u64) -> String {
        let mut c = [0u64; store::nasio::SLOTS];
        c[store::nasio::Count::ReadBytes as usize] = read;
        c[store::nasio::Count::Tiffs as usize] = tiffs;
        let esc: String = c.iter().flat_map(|v| v.to_le_bytes()).map(|b| format!("\\{b:03o}")).collect();
        format!("printf '{esc}' > \"$SCENIC_NAS_COUNTS/$$-1\"")
    }

    #[test]
    fn a_z6_tile_counts_by_the_stages_it_is_through() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("unit-stages.json");
        let mut a = TileProgress::new(4, Some(file.clone()));
        let total: f64 = STAGES.iter().map(|s| s.1).sum();
        a.on("6/1/1");
        a.stage("piece copied from the NAS", 1.0, None);
        assert!((a.done() - 10.0 / total).abs() < 1e-9);
        // Halfway through its elevations: the stages before, and half of that one.
        a.stage("elevations (elev)", 0.5, None);
        let before: f64 = STAGES[..5].iter().map(|s| s.1).sum();
        assert!((a.done() - (before + 75.0) / total).abs() < 1e-9);
        // A stage said again, or an earlier one, is no step back; one it doesn't know, nothing.
        a.stage("extract", 1.0, None);
        a.stage("something else", 1.0, None);
        assert!((a.done() - (before + 75.0) / total).abs() < 1e-9);
        // Its last steps out with another worker while the next z6 tile builds: both count.
        a.on("6/1/2");
        a.stage("extract", 1.0, None);
        let two = a.done();
        assert!(two > (before + 75.0) / total);
        a.on("6/1/1");
        a.finished("6/1/1");
        // (Its folders removed after: nothing more.)
        a.stage("its folders removed", 1.0, None);
        assert!((a.done() - (1.0 + (STAGES[..3].iter().map(|s| s.1).sum::<f64>()) / total)).abs() < 1e-9);
        // A stage's time teaches its weight, kept for the next job.
        a.on("6/1/2");
        a.stage("DEM cache slice", 1.0, Some(std::time::Duration::from_secs(70)));
        a.finished("6/1/2");
        let kept: BTreeMap<String, f64> = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(kept["DEM cache slice"], 0.75 * 30.0 + 0.25 * 70.0);
        assert_eq!(TileProgress::new(4, Some(file.clone())).secs["DEM cache slice"], 40.0);
        assert_eq!(a.done(), 2.0);
        // How long stages take here, once this Mac has timed each (kept or now): a tail's patience.
        assert_eq!(a.here_s(&["DEM cache slice"]), Some(40.0));
        assert_eq!(a.here_s(&["DEM cache slice", "scenic view"]), None);
        assert_eq!(TileProgress::new(4, Some(file)).here_s(&["DEM cache slice"]), Some(40.0));
    }

    #[test]
    fn a_logs_last_lines() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("steps.log");
        std::fs::write(&log, (1..=100).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        assert_eq!(log_tail(&log, 3), "line 98\nline 99\nline 100");
        assert_eq!(log_tail(&d.path().join("none.log"), 3), "");
        // A failing step: its log's end is kept, and the error names it.
        let mut c = store::spawn::command("sh");
        c.args(["-c", "echo why it failed; exit 3"]);
        let e = run(c, "a step", &log).unwrap_err().to_string();
        assert!(e.contains("a step failed") && log_tail(&log, 1) == "why it failed");
    }

    #[test]
    fn a_step_whose_server_gave_no_answer_is_told_from_a_failed_one() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("steps.log");
        // (Its program's own status for a server that gave no answer: crate::fetch::exit_with.)
        let mut c = store::spawn::command("sh");
        c.args(["-c", &format!("echo no answer; exit {}", crate::fetch::UNREACHABLE_EXIT)]);
        let e = run(c, "elevations (elev)", &log).unwrap_err();
        assert!(e.is::<SourceDown>() && source_down(&e), "{e:#}");
        // (Wrapped in context, too.)
        assert!(source_down(&e.context("unit 6/1/1")));
        // Any other failure is the job's.
        let mut c = store::spawn::command("sh");
        c.args(["-c", "exit 1"]);
        let e = run(c, "elevations (elev)", &log).unwrap_err();
        assert!(!source_down(&e), "{e:#}");
        // A request of the job's own process that gave no answer, as the fetcher says it.
        let e = anyhow::anyhow!("https://example.org/a.zip: no answer (connection reset); tried 11 times");
        assert!(source_down(&e));
    }

    #[test]
    fn a_whole_tail_may_run_anywhere_but_the_canopy_squares_a_worker_cant_have() {
        let u = TileId::parse("6/20/22").unwrap();
        let what = |runs: &[Run]| runs.iter().map(|r| r.what.clone()).collect::<Vec<_>>();
        let mut runs = tail(u, true, true);
        let (here, anywhere) = split(&runs);
        assert!(here.is_empty());
        assert_eq!(
            what(anywhere),
            [
                "elevations (elev)",
                "clean-up and grade (tile elev)",
                "scenic prep",
                "scenic canopy",
                "scenic view",
                "scenic buildings",
                "scenic flags"
            ]
        );
        assert!(anywhere
            .iter()
            .all(|r| r.reads.iter().all(|p| p.starts_with("{dir}/") || p.starts_with("{buildings}/"))));
        // A canopy square the NAS's store lacks: the steps through the canopy stay here.
        keep_here(&mut runs, "scenic canopy");
        let (here, anywhere) = split(&runs);
        assert_eq!(what(here), ["elevations (elev)", "clean-up and grade (tile elev)", "scenic prep", "scenic canopy"]);
        assert_eq!(what(anywhere), ["scenic view", "scenic buildings", "scenic flags"]);
        // Without the roadside buildings, flags still follows view.
        let runs = tail(u, false, false);
        assert_eq!(split(&runs).1.len(), 6);
    }

    #[test]
    fn the_canopy_squares_a_unit_reads_are_known_from_its_grid() {
        let d = tempfile::tempdir().unwrap();
        // Two grid tiles (z11) either side of 50°N at 7°E, and one at the antimeridian's west.
        let z11 = |lon: f64, lat: f64| {
            let (gx, gy) = roadcore::grid::cell_of(lon, lat);
            [(gx / 256.0) as u32, (gy / 256.0) as u32]
        };
        let tiles = vec![z11(7.0, 50.05), z11(7.0, 49.95), z11(-179.99, 0.5)];
        assert_eq!(
            canopy_squares(&tiles).into_iter().collect::<Vec<_>>(),
            [(10, -180), (60, 0), (50, 0)]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        roadcore::grid::GridIndex::new(tiles).save(&d.path().join("grid.idx")).unwrap();
        let sources = d.path().join("sources");
        std::fs::create_dir_all(sources.join("canopy")).unwrap();
        assert!(!canopy_stored(d.path(), &sources));
        for sq in canopy_squares(&roadcore::grid::GridIndex::load(d.path()).unwrap().tiles) {
            for n in canopy_square_files(sq) {
                assert!(!canopy_stored(d.path(), &sources));
                // (A file, or Meta's "none there", empty.)
                std::fs::write(sources.join("canopy").join(n), b"").unwrap();
            }
        }
        assert!(canopy_stored(d.path(), &sources));
    }

    #[test]
    fn kept_scenic_results_move_to_the_shared_cache() {
        let d = tempfile::tempdir().unwrap();
        let (local, shared) = (d.path().join("local"), d.path().join("shared"));
        std::fs::create_dir_all(local.join("scenic-units/6-9-9")).unwrap();
        std::fs::write(local.join("scenic-units/6-9-9/a.bin"), b"x").unwrap();
        assert_eq!(move_kept_to_shared(&local, &shared).unwrap(), 1);
        assert_eq!(std::fs::read(shared.join("scenic-units/6-9-9/a.bin")).unwrap(), b"x");
        assert!(!local.join("scenic-units/6-9-9").exists());
        assert_eq!(move_kept_to_shared(&local, &shared).unwrap(), 0);
    }

    #[test]
    fn subset_rewrites_ways_and_verts() {
        let d = tempfile::tempdir().unwrap();
        let mk = |id: i64, vstart: u64, vcount: u32| WayRec { id, vstart, vcount, ..bytemuck::Zeroable::zeroed() };
        let ways = [mk(1, 0, 2), mk(2, 2, 3), mk(3, 5, 2)];
        let verts: Vec<[i32; 2]> = (0..7).map(|i| [i, i]).collect();
        let mut wb = roadcore::WAYS_MAGIC.to_vec();
        wb.extend_from_slice(&3u64.to_le_bytes());
        wb.extend_from_slice(bytemuck::cast_slice(&ways));
        std::fs::write(d.path().join("ways.bin"), wb).unwrap();
        std::fs::write(d.path().join("verts.bin"), bytemuck::cast_slice(&verts)).unwrap();
        let (n, v) = subset(d.path(), |w, _| w.id != 2).unwrap();
        assert_eq!((n, v), (2, 4));
        let w = roadcore::Ways::open(d.path()).unwrap();
        assert_eq!(w.ways().iter().map(|r| (r.id, r.vstart, r.vcount)).collect::<Vec<_>>(), vec![(1, 0, 2), (3, 2, 2)]);
        assert_eq!(w.verts(), &[[0, 0], [1, 1], [5, 5], [6, 6]]);
    }
}
