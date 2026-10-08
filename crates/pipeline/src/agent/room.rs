//! Room on the disk (docs/plan.md §8, Room on the disk): the agent's local copies of what the NAS
//! keeps, emptied before a job (`make_room`), after the build (`trim`) and on the owner's ask
//! (`clear`).
//!
//! Before a job starts, when the disk's free space is under what the job needs (`RESERVE`; a
//! terrain run more; the OSM pass, its own), the local copies of what the NAS
//! keeps lose files until it has that and a margin (`margin`: a sixth more, none for the OSM pass),
//! so the next jobs start without deleting again: Meta's canopy squares (`chm10/`, ~2 GB a 10°
//! square; scenic-metrics marks a square used when it reads it), AWS's raw terrain tiles
//! (`aws-terrarium/`, read once per terrain run), the copies of the records' files staging
//! reads (`blobs/`, store::blobs), and the pageview months' indexes (`items/months/`, ~600 MB each,
//! read once per items or heritage run: dem/pageviews.py's copies, which it marks used as it reads
//! them). They fill again from the NAS (`sources/canopy/`, `sources/aws-terrarium/`, the store,
//! `sources/pageviews/`), never from the internet.
//! - Canopy squares, copies of the records' files and pageview months not read in the last hour go
//!   first, each by its own use, the least recently used first. A copy of a recorded file goes
//!   without asking the NAS (the records name only files it has). One listing of the NAS's canopy
//!   folder answers for every square's files (hundreds of MB each), while each raw tile folder
//!   takes a listing of its own for ~14 MB: seconds each when the NAS is busy, hours for tens of GB.
//! - Then raw tiles a folder at a time and the squares read since, together, the least recently
//!   used first (a folder by its newest tile, and in it the oldest first, so a folder's tiles go
//!   together): the squares of the area being built, which the next jobs read again, outlast idle
//!   tiles.
//! - A file goes once the NAS's folder, listed once (sixteen at a time: a listing mostly waits on
//!   the NAS; a folder that can't be listed now, or whose listing is cut short, keeps its raw tiles
//!   here this run, and has each canopy square asked about alone), has it at the same size (asked
//!   about once more when the listing lacks it). A canopy square the NAS lacks, or has at another
//!   size (downloaded before it kept them, or a copy cut short), is copied there first (whole and
//!   flushed: crate::whole: one large file), and kept here when that fails; a raw tile it lacks
//!   stays here until it reaches the NAS in bulk (copied a tile at a time, with a flush each, small
//!   files stall the NAS: ~23 a second, and both Macs' processes wait on it meanwhile). One that
//!   isn't whole itself (cut short, or a temporary file) is deleted without being kept anywhere.
//! - Raw tiles the NAS lacks are packed onto it first (crate::rawpack: an archive of their own for
//!   each area, one large write each, none kept here), then go; the copies of its archives here
//!   (`aws-terrarium/packs/`) go as the copies of the records' files do, each by its own use (a
//!   job marks one used when it opens it), without asking it.
//! - A pageview month's index goes once the NAS has it at its size; one it lacks stays (pageviews.py
//!   puts it there when it next reads it), as do the counts from before the indexes: none is copied
//!   there from here.
//!
//! - A file the jobs queued and running read (`Hints`: a terrain run's own area's archive copies;
//!   the pageview months, for an items or heritage job; a unit's canopy squares and packs) goes
//!   last.
//!
//! After the build (`trim`): once the build has nothing left to build (the build Mac's forecast:
//! no work, no round under way), jobs running here or not, the agent empties those caches by the same
//! rules, once (again only after a job has run here since): a helper all of them, the build Mac all
//! but the canopy squares, which every pass's areas read again and which never change. (Not down
//! to the reserve: room-making makes that much room before each job, so the build ends with about
//! that free, and a trim to it would free little or nothing, while that Mac's mirror copies nothing
//! until 150 GB are free.)
//!
//! On the owner's ask (`clear`: the menu bar's Clear the Build's Caches, `scenic clean`, a
//! `ClearRequest`), once the build is done, jobs running here or not: those caches, the canopy squares
//! too, and the others a later job fills again from the NAS or makes again from what's there
//! (`kind`): the pack cache, the DEM seed (only while the NAS has it whole), the local copies of the
//! NAS's files and the heritage jobs' clip of the planet. Kept: the Wikidata and Wikipedia answers
//! the items and heritage jobs keep (crate::answers: the NAS has them too, but they free little, the
//! items' 13 MB and the heritage copy's own ~0.4 GB, which is whole or nothing), the heritage
//! scripts' old Python environment (`heritage-venv/`: their next run removes it), what frees next to
//! nothing (the registers' snapshot, which the pass's heritage folder is an APFS clone of; the
//! trains' stop pairs, under a MB), the agent's own timings (`unit-stages.json`) and what units kept
//! that isn't on the NAS yet (`dem-units/`, `scenic-units/`).
//!
//! The owner's room target (`Target`: `scenic room <GB>`, the menu bar's Disk Room; `toward`): the
//! free space this Mac's agent keeps, set at any moment, in its folder (each Mac's disk is its own).
//! A job starts only with the target free past its own room, so nothing a job copies back crosses
//! it; the jobs wait, saying why, until it's lowered or off (the daily backup and GC keep none).
//! Whenever the disk is short of the target, or of the room a job waiting for it needs past it,
//! the agent frees its caches toward that on a thread of its own as a trim's, jobs running or not,
//! whether or not the build has work left: the cheap caches by room-making's rules and order (the
//! canopy squares too), then a clear's others, the cheapest to fill again first (`TOWARD_ORDER`:
//! copies of the NAS's files, the base packs a file at a time, the least recently used first, the
//! DEM seed whole, the heritage clip last, and only while it can be made again:
//! `heritage_remade`), each only as far as needed, the goal read again as it goes (lowered or
//! off: it stops).
//!
//! Mid-job (docs/plan.md §8): room-making, a trim, a clear and a freeing toward the target act
//! while jobs run. They never delete what a job uses: a job holds each cache file it uses
//! (store::cachefile's shared flock), and a file goes only under an exclusive one taken without
//! waiting (`store::cachefile::try_remove`; the DEM seed's three files together), so one in use is
//! passed over, counted as kept. A wrongly deleted file costs a fetch: every read of the caches
//! fills what's missing. What the queued jobs read goes last (`Hints`). Not while a job an earlier
//! agent left runs (the agent's to see to: its programs may not hold what they use).
//!
//! Nothing goes through a link: a folder or file of the cache that's a link, at any depth (`walk`;
//! crate::rawpack's packer passes them over too), or a folder in the NAS's project folder by its
//! real path (`ours`), is left as it is and not counted, so the NAS's own files never go. (The agent
//! runs a trim or a clear on a thread of its own, jobs starting meanwhile.)
//!
//! Each ends early when the agent is asked to stop. Nothing else of the cache is deleted here.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The free space a job starts with, at least, when the caches can make it.
pub const RESERVE: u64 = 30 << 30;

/// What's freed past a job's `need` once the disk is short of it: a sixth more (5 GB past the
/// build Mac's 30), so the jobs after it start without deleting again. (Not for the OSM pass, whose
/// need is what its conditions admitted it with.)
pub fn margin(need: u64) -> u64 {
    need / 6
}

/// NAS folders listed at once.
const LIST_AHEAD: usize = 16;

/// Canopy squares read this lately (the area being built's, which the next jobs read again) wait
/// with the raw tiles, in least recently used order; the others go before any tile.
const RECENT: std::time::Duration = std::time::Duration::from_secs(3600);

/// The caches' folders whose files may be deleted, under the agent's cache, each with the NAS's
/// store of them, under its `sources/`; and `blobs/`, this Mac's copies of files the records name
/// (store::blobs), which the NAS has by construction: they go without asking it.
const CHEAP: [(&str, &str); 4] = [("chm10", "canopy"), ("aws-terrarium", "aws-terrarium"), ("blobs", ""), (MONTHS, "pageviews")];

/// The pageview months' indexes here (dem/pageviews.py's copies of the NAS's `sources/pageviews/`).
pub const MONTHS: &str = "items/months";

/// Bytes the cheap caches hold (what `make_room` can free on the build Mac: a helper's,
/// `helper_cheap_bytes`): of the pageview months, the indexes the NAS's `sources` has at their size
/// (`months_bytes`), none without it.
pub fn cheap_bytes(cache: &Path, sources: Option<&Path>) -> u64 {
    let mut files = Vec::new();
    for (d, _) in CHEAP.iter().filter(|(d, _)| *d != MONTHS) {
        walk_cheap(cache, d, None, &mut files);
    }
    files.iter().map(|f| f.1).sum::<u64>() + sources.map_or(0, |s| months_bytes(cache, s))
}

/// Whether `p`, a file of the cheap caches, is one they may lose: of the pageview months, an index
/// only (the counts from before them, and a stream's temporary file, stay: `fate`).
fn cheap_file(cache: &Path, p: &Path) -> bool {
    !p.starts_with(cache.join(MONTHS)) || p.to_string_lossy().ends_with(".tsv.zst")
}

/// The bytes of the pageview months' indexes here that the NAS's `sources` has at their size: what
/// room-making, a trim or a clear can free of them (one it lacks stays, for pageviews.py to put
/// there). Nothing in the NAS's project folder (`ours`).
fn months_bytes(cache: &Path, sources: &Path) -> u64 {
    let mut files = Vec::new();
    walk_cheap(cache, MONTHS, sources.parent(), &mut files);
    let there = |p: &Path, len: u64| p.file_name().is_some_and(|n| std::fs::metadata(sources.join("pageviews").join(n)).is_ok_and(|m| m.is_file() && m.len() == len));
    files.iter().filter(|f| cheap_file(cache, &f.2) && there(&f.2, f.1)).map(|f| f.1).sum()
}

/// What `make_room` can free on a helper: the cheap caches but the loose raw tiles (`aws-terrarium/
/// <z>/…`), which it keeps until a job of its own packs them onto the NAS (it can't: only a job
/// handing off packs; the NAS lacks them). The copies of the NAS's archives (`aws-terrarium/packs/`)
/// count.
pub fn helper_cheap_bytes(cache: &Path) -> u64 {
    let mut files = Vec::new();
    for d in ["chm10", "blobs", "aws-terrarium/packs"] {
        walk_cheap(cache, d, None, &mut files);
    }
    files.iter().map(|f| f.1).sum()
}

/// Whether `p`, in the agent's cache, is this Mac's own to count and delete from: not a link (a
/// cache folder linked to the NAS's own store would have the NAS's files deleted through it), nor,
/// by its real path, in the NAS's project folder `root` (None: unknown, the link alone tells).
fn ours(p: &Path, root: Option<&Path>) -> bool {
    if !std::fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink()) {
        return false;
    }
    let Some(root) = root else { return true };
    match (p.canonicalize(), root.canonicalize()) {
        (Ok(p), Ok(r)) => !p.starts_with(r),
        // (No NAS folder there at all: nothing is in it.)
        (Ok(_), Err(e)) if e.kind() == std::io::ErrorKind::NotFound => true,
        _ => false,
    }
}

/// Walks cheap cache folder `d` of `cache` (`chm10`, `aws-terrarium/packs` …) into `out`, when it
/// and each folder above it in the cache are this Mac's own (`ours`).
fn walk_cheap(cache: &Path, d: &str, root: Option<&Path>, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
    let mut p = cache.to_path_buf();
    for part in d.split('/') {
        p.push(part);
        if !ours(&p, root) {
            return;
        }
    }
    walk(&p, out);
}

/// Which of the caches `name`, an entry of the agent's cache folder, is part of, when a clear
/// empties it (`Freed`'s keys); None for what stays (the module's doc says what, and why).
fn kind(name: &str) -> Option<&'static str> {
    Some(match name {
        "chm10" => "canopy",
        "aws-terrarium" => "terrain",
        "blobs" => "blobs",
        // (Of it, the pageview months' copies: the items job's answers beside them stay.)
        "items" => "months",
        // (pack(T)'s and lo's base packs, copied from the NAS where the mirror lacks them.)
        "base" => "base",
        _ if name.starts_with("dem-cache.") => "dem",
        // (scenic-build's local copies, by their logical names: the z8 terrain, the summits.)
        _ if name.starts_with("sources-") || name.starts_with("work-") => "copies",
        _ if name.starts_with("heritage-merged-") => "heritage",
        _ => return None,
    })
}

/// Whether cache `kind` is one of the cheap ones, which room-making and a trim empty too.
fn cheap(kind: &str) -> bool {
    matches!(kind, "canopy" | "terrain" | "blobs" | "months")
}

/// A cache (`kind`) in words.
pub fn words(kind: &str) -> &str {
    match kind {
        "canopy" => "canopy squares",
        "terrain" => "raw terrain tiles",
        "blobs" => "copies of the records' files",
        "months" => "pageview months",
        "base" => "base packs",
        "dem" => "the DEM seed",
        "copies" => "copies of the NAS's files",
        "heritage" => "the heritage jobs' planet clip",
        k => k,
    }
}

/// A size in words: "51.2 GB", or under a GB, "350 MB".
pub fn size(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", b >> 20)
    }
}

/// What a trim or a clear did, for the agent's status (`Caches`) and the history.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freed {
    /// When it was done (seconds since the epoch).
    pub at: u64,
    /// A clear's ask: when it was asked (its `at`), and by whom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The bytes freed, by cache (`kind`).
    #[serde(default)]
    pub freed: BTreeMap<String, u64>,
    /// What stays of what it went through: files the NAS hasn't (a helper's loose raw tiles, which
    /// only its own jobs pack; a canopy square it couldn't take now; a pageview month's index, which
    /// pageviews.py puts there when it next reads it; the DEM seed, for a clear).
    #[serde(default)]
    pub left: u64,
    /// Why it wasn't done, when it wasn't (a clear asked for while a job ran here, or while the
    /// build had work left).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
    /// A freeing toward the owner's disk room target (`toward`): the target it was for (bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<u64>,
    /// And the room it freed toward, when more than the target: what a job waiting for the target
    /// needs past it (bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<u64>,
}

impl Freed {
    /// The bytes freed.
    pub fn bytes(&self) -> u64 {
        self.freed.values().sum()
    }

    /// In words: "39.2 GB freed (raw terrain tiles 24.1 GB, …)", and what stays.
    pub fn say(&self) -> String {
        let mut each: Vec<(u64, &str)> = self.freed.iter().filter(|(_, &b)| b > 0).map(|(k, &b)| (b, k.as_str())).collect();
        each.sort_by_key(|e| std::cmp::Reverse(e.0));
        let mut s = format!("{} freed", size(self.bytes()));
        if !each.is_empty() {
            s += &format!(" ({})", each.iter().map(|(b, k)| format!("{} {}", words(k), size(*b))).collect::<Vec<_>>().join(", "));
        }
        if self.left > 0 {
            s += &format!("; {} kept (the NAS hasn't it yet)", size(self.left));
        }
        s
    }
}

/// This Mac's build caches, as its agent's status says (`Status::caches`): what a clear would free,
/// why they can't be cleared now, and the last trim and clear.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caches {
    /// What a clear would free (bytes), as last counted: every ten minutes on a thread of its own,
    /// and after a trim or a clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearable: Option<u64>,
    /// Why they can't be cleared (or trimmed) now: a job an earlier agent left runs here, the build has work left …;
    /// None: they can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
    /// What a clear would free, cache by cache, with how each comes back (`gone`): for the owner's
    /// confirmation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub each: Vec<Gone>,
    /// The last trim after the build, the last clear done, and the last ask declined (why in it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trimmed: Option<Freed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<Freed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declined: Option<Freed>,
    /// The owner's disk room target and the free space now (`RoomView`); None on an agent from
    /// before targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<RoomView>,
}

/// A cache a clear would empty: its name (`kind`) and in words, its bytes, and how it comes back.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gone {
    pub cache: String,
    pub what: String,
    pub bytes: u64,
    pub back: String,
}

/// What a clear would free (`Sizes::clear`), the most first, with how each cache comes back and
/// about how long that takes, the NAS read at `mb_s` (MB/s; measured: 60 on the LAN, 12 through
/// Tailscale, plan §12).
pub fn gone(clear: &BTreeMap<String, u64>, mb_s: f64) -> Vec<Gone> {
    let time = |b: u64| match b as f64 / (mb_s * (1u64 << 20) as f64) {
        s if s < 5400.0 => format!("about {} min", (s / 60.0).ceil().max(1.0)),
        s => format!("about {:.1} h", s / 3600.0),
    };
    let mut v: Vec<Gone> = clear
        .iter()
        .filter(|(_, &b)| b > 0)
        .map(|(k, &b)| {
            let back = match k.as_str() {
                "canopy" => format!("copied back from the NAS as the areas that read them are built ({} in all)", time(b)),
                "terrain" => format!("copied back from the NAS's archives as terrain and peaks read them ({} in all)", time(b)),
                "blobs" => format!("copied back from the NAS as the areas are built again ({} in all)", time(b)),
                "months" => format!("copied back from the NAS when the items and heritage jobs next run ({})", time(b)),
                "base" => format!("copied back by the next round's map tiles, from the mirror where it has them, else the NAS ({})", time(b)),
                "dem" => format!("copied back from the NAS by the next unit job ({})", time(b)),
                "copies" => format!("copied back from the NAS when the summits and peaks next run ({})", time(b)),
                "heritage" => "made again from the NAS's filtered planet when the heritage chain next runs (about an hour of osmium)".to_string(),
                _ => "filled again from the NAS".to_string(),
            };
            Gone { cache: k.clone(), what: words(k).to_string(), bytes: b, back }
        })
        .collect();
    v.sort_by_key(|g| std::cmp::Reverse(g.bytes));
    v
}

/// The owner's ask to clear this Mac's build caches (the menu bar's Clear the Build's Caches,
/// `scenic clean`): a file in its agent's folder (`CLEAR_REQUEST`) the agent takes up within
/// seconds, as it does a pause's (crate::control): it clears them or says why it can't now, in its
/// status (`Caches::cleared` or `declined`, with when this was asked), and the ask goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearRequest {
    /// Who asked, in words: "the menu bar on …", "scenic clean on …".
    pub by: String,
    /// When it was asked (unix seconds).
    pub at: u64,
}

/// The ask's file, in the agent's folder; and the one it's renamed to as it's taken up.
pub const CLEAR_REQUEST: &str = "clear-request.json";
pub const CLEAR_TAKEN: &str = "clear-request.json.taken";

/// Asks this Mac's agent (its folder `home`) to clear its build caches.
pub fn request_clear(home: &Path, by: &str) -> Result<ClearRequest> {
    std::fs::create_dir_all(home)?;
    let r = ClearRequest { by: by.to_string(), at: super::jobs::now_s() };
    crate::whole::write(&home.join(CLEAR_REQUEST), &serde_json::to_vec(&r)?)?;
    Ok(r)
}

/// The ask waiting in `home`, taken up: renamed aside (`CLEAR_TAKEN`) before it's read, so one
/// written meanwhile waits its turn; or one an agent took up and stopped before it answered. None
/// when there's none (one that doesn't parse goes).
pub fn take_clear(home: &Path) -> Option<ClearRequest> {
    let taken = home.join(CLEAR_TAKEN);
    if !taken.exists() && std::fs::rename(home.join(CLEAR_REQUEST), &taken).is_err() {
        return None;
    }
    let r = std::fs::read(&taken).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if r.is_none() {
        std::fs::remove_file(&taken).ok();
    }
    r
}

/// The ask taken up answered: it goes.
pub fn clear_answered(home: &Path) {
    std::fs::remove_file(home.join(CLEAR_TAKEN)).ok();
}

/// Empties the cheap caches at `cache` once the build is done (the module's doc, After the build):
/// every file that may go by room-making's rules, each once the NAS's `sources` has it, but those
/// `spare` keeps (the build Mac's canopy squares). What it freed, and what stays but the spared.
pub fn trim(cache: &Path, sources: &Path, spare: &dyn Fn(&Path) -> bool) -> Result<Freed> {
    // (None yet: no job has run here.)
    if !cache.is_dir() {
        return Ok(Freed::default());
    }
    let freed = free_cheap(cache, sources, u64::MAX, u64::MAX, &disk_free, spare, &|_| false, None)?;
    Ok(Freed { freed, left: cheap_left(cache, sources.parent(), spare), ..Default::default() })
}

/// Clears this Mac's build caches at `cache` on the owner's ask (the module's doc, On the owner's
/// ask): the cheap caches as a trim empties them, the canopy squares too, then the others whole
/// (nothing through a link, nor in the NAS's folder: `ours`), the DEM seed only while the NAS's
/// `sources` has it whole (else every unit would sample its vertices anew). What it freed, and
/// what stays of them.
pub fn clear(cache: &Path, sources: &Path) -> Result<Freed> {
    if !cache.is_dir() {
        return Ok(Freed::default());
    }
    let mut freed = free_cheap(cache, sources, u64::MAX, u64::MAX, &disk_free, &|_| false, &|_| false, None)?;
    let mut left = cheap_left(cache, sources.parent(), &|_| false);
    let seed_whole = seed_whole(sources);
    let mut others: Vec<(PathBuf, &str)> = std::fs::read_dir(cache).into_iter().flatten().flatten().filter_map(|e| Some((e.path(), kind(&e.file_name().to_string_lossy()).filter(|k| !cheap(k))?))).collect();
    others.sort();
    for (p, k) in others {
        if super::stopping() {
            break;
        }
        if !ours(&p, sources.parent()) {
            continue;
        }
        let bytes = bytes_under(&p);
        if (k == "dem" && !seed_whole && !crate::whole::is_tmp(&p)) || (k == "heritage" && !heritage_remade(sources.parent(), &p)) {
            left += bytes;
            continue;
        }
        // (File by file, none a job holds: store::cachefile. The DEM seed's files together.)
        let t = if k == "dem" && !crate::whole::is_tmp(&p) { seed_gone(cache, &p) } else { store::cachefile::remove_tree(&p) };
        *freed.entry(k.to_string()).or_default() += t.freed;
        left += t.left;
    }
    freed.retain(|_, b| *b > 0);
    Ok(Freed { freed, left, ..Default::default() })
}

/// Whether the NAS's `sources` has the DEM seed whole, as unit::dem_seed takes it: its three files,
/// of one count. (Then every local file of it may go, whole or cut short; else only a temporary one.)
fn seed_whole(sources: &Path) -> bool {
    let len = |n: &str| std::fs::metadata(sources.join("dem-cache").join(n)).ok().filter(|m| m.is_file()).map(|m| m.len());
    matches!((len("dem-cache.keys.u64"), len("dem-cache.elev.f32"), len("dem-cache.src.u8")), (Some(k), Some(e), Some(s)) if k == 8 * s && e == 4 * s)
}

/// The DEM seed here deleted together (`p` one of its files: its three, each once), none of them
/// while a job holds one (store::cachefile::try_remove_all). Once the first of the three is seen,
/// the others are gone too: what they freed counts there.
fn seed_gone(cache: &Path, p: &Path) -> store::cachefile::Tree {
    let all: Vec<PathBuf> = ["keys.u64", "elev.f32", "src.u8"].iter().map(|n| cache.join(format!("dem-cache.{n}"))).collect();
    if !all.iter().any(|a| a == p) {
        return store::cachefile::remove_tree(p);
    }
    let bytes: u64 = all.iter().map(|a| bytes_under(a)).sum();
    match store::cachefile::try_remove_all(&all) {
        Some(n) => store::cachefile::Tree { freed: n, left: 0 },
        None => store::cachefile::Tree { freed: 0, left: bytes_under(p).min(bytes) },
    }
}

/// The owner's disk room target on this Mac (`scenic room <GB>`, the menu bar's Disk Room): the
/// free space its agent keeps (the module's doc, The room target). In the agent's folder
/// (`TARGET`): each Mac's disk is its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The free space to keep (bytes).
    pub bytes: u64,
    /// Who set it, in words ("scenic room on m4", "the menu bar on m4"), and when (unix seconds).
    pub by: String,
    pub at: u64,
}

/// The target's file, in the agent's folder.
pub const TARGET: &str = "room-target.json";

/// Sets this Mac's disk room target (its agent's folder `home`) to `bytes`, or clears it (None).
pub fn set_target(home: &Path, bytes: Option<u64>, by: &str) -> Result<Option<Target>> {
    let p = home.join(TARGET);
    let Some(bytes) = bytes else {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        return Ok(None);
    };
    std::fs::create_dir_all(home)?;
    let t = Target { bytes, by: by.to_string(), at: super::jobs::now_s() };
    crate::whole::write(&p, &serde_json::to_vec(&t)?)?;
    Ok(Some(t))
}

/// This Mac's disk room target, when one is set (one that doesn't parse, or of 0 bytes, is none;
/// one past `MAX_TARGET` is that).
pub fn target(home: &Path) -> Option<Target> {
    std::fs::read(home.join(TARGET)).ok().and_then(|b| serde_json::from_slice::<Target>(&b).ok()).filter(|t| t.bytes > 0).map(|t| Target { bytes: t.bytes.min(MAX_TARGET), ..t })
}

/// The largest target (1 PB: a hand-edited file's past any disk), so sums with it can't overflow.
pub const MAX_TARGET: u64 = 1 << 50;

/// The disk room target as the agent's status says it (`Caches::room`): the target, the disk's
/// free space when the status was written, and the last time the agent freed its caches toward it
/// (`toward`: what it freed, and when it couldn't reach it, what stays and why). Its jobs and their
/// refills wait meanwhile, each saying why in the status's `waiting`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    pub free: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toward: Option<Freed>,
    /// Why the disk is short of the target and stays so, when it is (None: it has the target, or
    /// the agent is making it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
}

/// The caches a freeing toward the target empties after the cheap ones, in this order: the
/// cheapest to fill again first (copies of the NAS's files and the base packs, minutes from the
/// NAS; the DEM seed, 9 GB; the heritage jobs' planet clip, an hour of osmium, last).
const TOWARD_ORDER: [&str; 4] = ["copies", "base", "dem", "heritage"];

/// Whether the heritage jobs' clip of the planet at `p` (`heritage-merged-<date>-<cover>.osm.pbf`)
/// can be made again: the NAS's build manifest (under `root`) names the pass's filtered planet of
/// its date (`sources/osm/<date>/filtered`, what scenic-build's merged_over_cover clips), and it's
/// there. A temporary one is no use either way.
fn heritage_remade(root: Option<&Path>, p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.contains(".tmp") {
        return true;
    }
    let Some(date) = name.strip_prefix("heritage-merged-").and_then(|r| r.get(..10)) else { return false };
    let Some(root) = root else { return false };
    let m: BTreeMap<String, String> = std::fs::read(root.join("state/build/manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    m.get(&format!("sources/osm/{date}/filtered")).is_some_and(|c| root.join(c).is_file())
}

/// Frees this Mac's build caches at `cache` until the disk has `goal()` free (the owner's room
/// target, or what a job held by it needs past it; read again as it goes, so a target lowered or
/// cleared midway (0) stops it): the cheap caches first, by room-making's rules and in its order
/// (each file once the NAS's `sources` has it; the canopy squares too), then the others a clear
/// empties (`TOWARD_ORDER`), only as far as needed: the base packs a file at a time, the least
/// recently used first; the others whole (the DEM seed only while the NAS has it whole, its files
/// together; the heritage clip only while the pass's filtered planet it's clipped from is there).
/// Jobs running or not: what they hold stays (store::cachefile); what the queued ones read goes
/// last (`hint`). What it freed, and when it falls short, what stays of what it may free.
pub fn toward(cache: &Path, sources: &Path, goal: std::sync::Arc<std::sync::atomic::AtomicU64>, hint: &Hints) -> Result<Freed> {
    toward_with(cache, sources, &|| goal.load(std::sync::atomic::Ordering::Relaxed), &disk_free, &|p| hint.has(cache, p))
}

fn toward_with(cache: &Path, sources: &Path, goal: &dyn Fn() -> u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>, hint: &dyn Fn(&Path) -> bool) -> Result<Freed> {
    let first = goal();
    if !cache.is_dir() || free_space(cache)? >= first {
        return Ok(Freed::default());
    }
    let mut freed = free_cheap(cache, sources, first, first, free_space, &|_| false, hint, Some(goal))?;
    let root = sources.parent();
    let enough = || free_space(cache).map(|f| f >= goal());
    let whole_seed = seed_whole(sources);
    let mut left = 0;
    let mut short = !enough()?;
    for k in TOWARD_ORDER {
        if !short || super::stopping() {
            break;
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(cache).into_iter().flatten().flatten().filter(|e| kind(&e.file_name().to_string_lossy()) == Some(k)).map(|e| e.path()).filter(|p| ours(p, root)).collect();
        entries.sort();
        // The base packs a file at a time, the least recently used first (each file of the pack
        // cache is one a round reads, or copies again); the free space measured again once what
        // was short is deleted, every 2 GB (as room-making's `Room`), or when the goal changes.
        if k == "base" {
            let mut files = Vec::new();
            for e in &entries {
                if std::fs::symlink_metadata(e).is_ok_and(|m| m.is_dir()) {
                    walk(e, &mut files);
                } else if let Ok(m) = std::fs::metadata(e) {
                    files.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), e.clone()));
                }
            }
            // (Those the queued jobs read last: `Hints`.)
            files.sort_by_key(|f| (hint(&f.2), f.0, f.2.clone()));
            let mut aim = goal();
            let (mut since, mut lack) = (0, aim.saturating_sub(free_space(cache)?));
            for (_, len, p) in files {
                if super::stopping() {
                    break;
                }
                // (Not one a job holds: store::cachefile.)
                if let store::cachefile::Removed::Freed(_) = store::cachefile::try_remove(&p) {
                    *freed.entry(k.to_string()).or_default() += len;
                    since += len;
                } else if std::fs::symlink_metadata(&p).is_ok() {
                    left += len;
                }
                if since >= lack.min(2 << 30) || goal() != aim {
                    aim = goal();
                    let free = free_space(cache)?;
                    if free >= aim {
                        short = false;
                        break;
                    }
                    (since, lack) = (0, aim - free);
                }
            }
            short = short && !enough()?;
            continue;
        }
        // (The DEM seed's files together, whole or none: one of them alone is no seed.)
        if k == "dem" && !whole_seed {
            for p in entries {
                let b = bytes_under(&p);
                if crate::whole::is_tmp(&p) {
                    let t = store::cachefile::remove_tree(&p);
                    *freed.entry(k.to_string()).or_default() += t.freed;
                    left += t.left;
                } else {
                    left += b;
                }
            }
            short = !enough()?;
            continue;
        }
        let together = k == "dem";
        for p in entries {
            if super::stopping() || (!together && enough()?) {
                short = short && !enough()?;
                break;
            }
            let bytes = bytes_under(&p);
            // (A heritage clip that can't be made again stays.)
            if k == "heritage" && !heritage_remade(root, &p) {
                left += bytes;
                continue;
            }
            // (File by file, none a job holds: store::cachefile. The DEM seed's files together.)
            let t = if together && !crate::whole::is_tmp(&p) { seed_gone(cache, &p) } else { store::cachefile::remove_tree(&p) };
            *freed.entry(k.to_string()).or_default() += t.freed;
            left += t.left;
        }
        short = short && !enough()?;
    }
    if short {
        left += cheap_left(cache, root, &|_| false);
    }
    freed.retain(|_, b| *b > 0);
    Ok(Freed { freed, left: if short { left } else { 0 }, target: Some(first), ..Default::default() })
}

/// What the jobs queued here (and running) read of the caches, as far as the agent knows it: a hint
/// that has those files go last when room is made, so they aren't fetched twice. Only a hint: what
/// a running job uses it holds (store::cachefile), and a stale hint costs only a fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    /// Terrain jobs' targets ("terrain 3/x/y", an area's whole run; "terrain 6/x/y", a piece;
    /// "terrain-lo 3/x/y", an assembly): the archive copies they read (`terrain_reads`).
    pub terrain: Vec<String>,
    /// An items or heritage job: the pageview months.
    pub months: bool,
    /// Canopy squares' files (`chm10/<name>`) the units and tree cover read.
    pub squares: BTreeSet<String>,
    /// Units ("6-x-y"): the copies of their packs (`blobs/…/<unit>.<hash>.…`).
    pub units: BTreeSet<String>,
}

impl Hints {
    /// The hints of the work of `step` on `targets` ("z/x/y"): a unit's canopy squares (its box
    /// with staging's margin) and packs; a tree cover tile's squares; a terrain run's archives; an
    /// items or heritage job's pageview months.
    pub fn add(&mut self, step: &str, targets: &[String]) {
        let tile = |t: &str| -> Option<(u8, u32, u32)> {
            let mut v = t.split(['/', '-']).map(|s| s.parse::<u32>().ok());
            Some((u8::try_from(v.next()??).ok()?, v.next()??, v.next()??))
        };
        let mut squares = |b: [f64; 4]| {
            for (top, left) in crate::trees::squares_of(b) {
                for kind in ["median", "p95", "cover5m"] {
                    self.squares.insert(crate::trees::chm_name(top, left, kind));
                }
            }
        };
        match step {
            "unit" => {
                for (z, x, y) in targets.iter().filter_map(|t| tile(t)) {
                    squares(crate::stage::tile_box_grown(z, x, y, crate::stage::MARGIN_KM));
                    self.units.insert(format!("{z}-{x}-{y}"));
                }
            }
            "trees" => {
                for (z, x, y) in targets.iter().filter_map(|t| tile(t)) {
                    squares(crate::trees::tile_bounds(z, x, y));
                }
            }
            "terrain" => self.terrain.extend(targets.iter().map(|t| format!("terrain {t}"))),
            "terrain-lo" => self.terrain.extend(targets.iter().map(|t| format!("terrain-lo {t}"))),
            "items" | "heritage" => self.months = true,
            _ => {}
        }
    }

    /// Whether file `p` of the caches at `cache` is one the queued jobs read.
    pub fn has(&self, cache: &Path, p: &Path) -> bool {
        let name = p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        (self.months && p.starts_with(cache.join(MONTHS)))
            || self.terrain.iter().any(|id| super::terrain_reads(id, p))
            || (p.starts_with(cache.join("chm10")) && self.squares.contains(name.as_ref()))
            || (p.starts_with(cache.join("blobs")) && name.split('.').next().is_some_and(|u| self.units.contains(u)))
    }
}

/// What this Mac's caches hold (`sizes`): what room-making can free, and what a clear would, by
/// cache (`kind`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sizes {
    pub cheap: u64,
    pub clear: BTreeMap<String, u64>,
}

/// What this Mac's caches hold, for the status: what room-making can free (`cheap_bytes`; a
/// helper's, `helper_cheap_bytes`, its loose raw tiles left out; the pageview months the NAS has, only
/// when `root` is given), and what a clear would, by cache (those, and the others: the DEM seed
/// counted, as the NAS has it, whichever Mac copied it from there). Nothing through a link, nor in
/// the NAS's project folder `root` (`ours`).
pub fn sizes(cache: &Path, helper: bool, root: Option<&Path>) -> Sizes {
    let mut clear: BTreeMap<String, u64> = BTreeMap::new();
    let tiles = if helper { "aws-terrarium/packs" } else { "aws-terrarium" };
    for (d, k) in [("chm10", "canopy"), (tiles, "terrain"), ("blobs", "blobs")] {
        let mut files = Vec::new();
        walk_cheap(cache, d, root, &mut files);
        clear.insert(k.to_string(), files.iter().map(|f| f.1).sum());
    }
    // (The pageview months the NAS has at their size, when the NAS can be asked: the others stay.)
    if let Some(r) = root {
        clear.insert("months".to_string(), months_bytes(cache, &r.join("sources")));
    }
    let cheap_now = clear.values().sum();
    for e in std::fs::read_dir(cache).into_iter().flatten().flatten() {
        if let Some(k) = kind(&e.file_name().to_string_lossy()).filter(|k| !cheap(k) && ours(&e.path(), root)) {
            *clear.entry(k.to_string()).or_default() += bytes_under(&e.path());
        }
    }
    clear.retain(|_, b| *b > 0);
    Sizes { cheap: cheap_now, clear }
}

/// The bytes the cheap caches at `cache` hold in files (not empty markers) `spare` doesn't keep and
/// they may lose (`cheap_file`: of the pageview months, the indexes), of this Mac's own (not in the
/// NAS's project folder `root`).
fn cheap_left(cache: &Path, root: Option<&Path>, spare: &dyn Fn(&Path) -> bool) -> u64 {
    let mut files = Vec::new();
    for (d, _) in CHEAP {
        walk_cheap(cache, d, root, &mut files);
    }
    files.iter().filter(|f| !spare(&f.2) && cheap_file(cache, &f.2)).map(|f| f.1).sum()
}

/// The bytes of the loose raw tiles in `tiles` (`<z>/<x>/<y>.png`: not the archives' copies).
fn loose_bytes(tiles: &Path) -> u64 {
    let mut files = Vec::new();
    for e in std::fs::read_dir(tiles).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().parse::<u8>().is_ok() {
            walk(&e.path(), &mut files);
        }
    }
    files.iter().map(|f| f.1).sum()
}

/// The bytes of file `p`, or of the files under folder `p` (0 when it isn't there).
fn bytes_under(p: &Path) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let mut files = Vec::new();
            walk(p, &mut files);
            files.iter().map(|f| f.1).sum()
        }
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// When the disk has less than `need` free, deletes from the caches at `cache` until it has `need`
/// and `margin` more (or they're empty), each cheap file only once the NAS's `sources` has it; the
/// bytes deleted.
pub fn make_room(cache: &Path, sources: &Path, need: u64, margin: u64, hint: &Hints) -> Result<u64> {
    make_room_with(cache, sources, need, need + margin, &disk_free, &|p| hint.has(cache, p))
}

/// The NAS folders' files and their sizes, each folder listed once (a folder not there: none; one
/// that can't be listed now: None, and its files stay this run).
type Listed = HashMap<PathBuf, Option<HashMap<OsString, u64>>>;

/// What becomes of a local cache file.
#[derive(Debug, PartialEq)]
enum Fate {
    /// It may go: the NAS's store has it at the same size, or it isn't whole (cut short, or a
    /// temporary file), so it's no use anywhere.
    Go,
    /// It may go once it's copied to `.0` in the NAS's store.
    Copy(PathBuf),
    /// It stays (not a cheap file, or gone already).
    Stay,
}

/// Where local cache file `p` (under `cache/<dir>`) is kept in the NAS's store.
fn nas_path(cache: &Path, sources: &Path, p: &Path) -> Option<PathBuf> {
    // (Copies of the records' files, and of the raw tiles' archives, have no NAS folder to list:
    // they go as they are.)
    if p.starts_with(cache.join("aws-terrarium/packs")) {
        return None;
    }
    let (dir, store) = CHEAP.iter().find(|(d, s)| !s.is_empty() && p.starts_with(cache.join(d)))?;
    Some(sources.join(store).join(p.strip_prefix(cache.join(dir)).ok()?))
}

/// A NAS folder's files and their sizes (none when it isn't there; None when it can't be read now,
/// or its listing was cut short: a busy NAS's timeout midway, which would make the files it didn't
/// get to look missing there).
fn list(folder: &Path) -> Option<HashMap<OsString, u64>> {
    let rd = match std::fs::read_dir(folder) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(HashMap::new()),
        Err(_) => return None,
    };
    let mut names = HashMap::new();
    for e in rd {
        let e = e.ok()?;
        match e.metadata() {
            Ok(m) if m.is_file() => {
                names.insert(e.file_name(), m.len());
            }
            Ok(_) => {}
            // (Gone since it was listed: a temporary file renamed.)
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    Some(names)
}

/// What becomes of local cache file `p` (under `cache/<dir>`), its NAS folder listed once.
fn fate(cache: &Path, sources: &Path, p: &Path, listed: &mut Listed) -> Fate {
    // (Copies of files the NAS has by construction: the records', and the raw tiles' archives.)
    if p.starts_with(cache.join("blobs")) || p.starts_with(cache.join("aws-terrarium/packs")) {
        return Fate::Go;
    }
    let Some(dest) = nas_path(cache, sources, p) else { return Fate::Stay };
    // (Of the pageview months, only an index may go: the counts from before them, and a stream's
    // temporary file, stay.)
    let month = p.starts_with(cache.join(MONTHS));
    if month && !p.to_string_lossy().ends_with(".tsv.zst") {
        return Fate::Stay;
    }
    let (Some(folder), Some(name)) = (dest.parent(), dest.file_name()) else { return Fate::Stay };
    let Ok(len) = std::fs::metadata(p).map(|m| m.len()) else { return Fate::Stay };
    let there = || std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == len);
    let Some(names) = listed.entry(folder.to_path_buf()).or_insert_with(|| list(folder)) else {
        // (Its folder can't be listed now: a canopy square, hundreds of MB, is asked about alone;
        // a raw tile stays this run, a question each costing about what the listing would.)
        return if !p.starts_with(cache.join("aws-terrarium")) && there() { Fate::Go } else { Fate::Stay };
    };
    // Missing from the listing: asked about once more before it's copied (a short listing, a
    // busy NAS's, without an error).
    if names.get(name) == Some(&len) || there() {
        return Fate::Go;
    }
    // (An index the NAS lacks: pageviews.py puts it there when it next reads it.)
    if month {
        return Fate::Stay;
    }
    if crate::whole::is_tmp(p) || !crate::whole::file_whole(p) {
        eprintln!("room: {} isn't whole: deleted, not kept", p.display());
        return Fate::Go;
    }
    // (A raw tile the NAS lacks waits here to reach it in bulk.)
    if p.starts_with(cache.join("aws-terrarium")) {
        return Fate::Stay;
    }
    Fate::Copy(dest)
}

/// Copies local cache file `p` to `dest` in the NAS's store, whole and flushed (the local copy goes
/// next: unflushed, a power cut on the NAS could lose both); whether it's there now. Each NAS folder
/// is made once a run (`made`).
fn copy_there(p: &Path, dest: &Path, made: &mut HashSet<PathBuf>) -> bool {
    let Some(folder) = dest.parent() else { return false };
    if !made.contains(folder) {
        if std::fs::create_dir_all(folder).is_err() {
            return false;
        }
        made.insert(folder.to_path_buf());
    }
    crate::whole::copy(p, dest).is_ok()
}

/// `make_room` with the disk's free space from `free_space`: nothing when it has `need`, else
/// deleting until it has `target`.
fn make_room_with(cache: &Path, sources: &Path, need: u64, target: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>, hint: &dyn Fn(&Path) -> bool) -> Result<u64> {
    Ok(free_cheap(cache, sources, need, target, free_space, &|_| false, hint, None)?.values().sum())
}

/// `make_room_with`'s work: the bytes deleted, by cache (`kind`). `spare` keeps a file for good
/// (a trim's rule); `hint` has it go last (`Hints`: what the queued jobs read).
/// (`goal`: the target as it is now, when it may change midway: room::toward's, the owner's to lower.)
#[allow(clippy::too_many_arguments)]
fn free_cheap(cache: &Path, sources: &Path, need: u64, target: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>, spare: &dyn Fn(&Path) -> bool, hint: &dyn Fn(&Path) -> bool, goal: Option<&dyn Fn() -> u64>) -> Result<BTreeMap<String, u64>> {
    let free = free_space(cache)?;
    if free >= need {
        return Ok(BTreeMap::new());
    }
    // Raw tiles the NAS lacks, packed onto it first (an archive an area: large writes), and gone
    // here once they're there. (Not from a linked folder: what's packed is deleted where it lies;
    // the packer passes links over inside it.)
    let tiles = cache.join("aws-terrarium");
    let packs = tiles.join("packs");
    let root = sources.parent();
    let mut packed = 0;
    if let Some(root) = root.filter(|r| ours(&tiles, Some(r))) {
        let before = loose_bytes(&tiles);
        if let Err(e) = crate::rawpack::pack_local(&tiles, &sources.join("aws-terrarium"), root, false) {
            eprintln!("room: raw tiles not packed now ({e:#}); they stay");
        }
        packed = before.saturating_sub(loose_bytes(&tiles));
    }
    // (Only this Mac's own folders: `ours`.)
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for (d, _) in CHEAP {
        walk_cheap(cache, d, root, &mut files);
    }
    // (Empty files are markers, "none there", that free nothing; what a trim keeps stays.)
    files.retain(|f| f.1 > 0 && !spare(&f.2));
    // Raw tiles by folder, canopy squares each alone. The squares not read lately first, then the
    // tiles and the squares read since, together: each the least recently used group (by its
    // newest file) first, in each the oldest first.
    let lately = SystemTime::now().checked_sub(RECENT).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut groups: BTreeMap<PathBuf, Vec<(SystemTime, u64, PathBuf)>> = BTreeMap::new();
    for f in files {
        // (An area's archive, each by its own use, as a canopy square.)
        let key = if f.2.starts_with(&tiles) && !f.2.starts_with(&packs) { f.2.parent().map(Path::to_path_buf).unwrap_or_default() } else { f.2.clone() };
        groups.entry(key).or_default().push(f);
    }
    let mut groups: Vec<Vec<(SystemTime, u64, PathBuf)>> = groups.into_values().collect();
    for g in &mut groups {
        g.sort();
    }
    // (What the queued jobs read, last of all: `Hints`.)
    groups.sort_by_cached_key(|g| {
        let newest = g.last().map(|f| f.0);
        let idle_square = !g.first().is_some_and(|f| f.2.starts_with(&tiles) && !f.2.starts_with(&packs)) && newest.is_some_and(|t| t < lately);
        (g.iter().any(|f| hint(&f.2)), !idle_square, newest)
    });
    let by = if packed > 0 { BTreeMap::from([("terrain".to_string(), packed)]) } else { BTreeMap::new() };
    let mut room = Room { cache, free_space, target, goal, short: target.saturating_sub(free), since: packed, by };
    let (mut listed, mut made) = (Listed::new(), HashSet::new());
    for ahead in groups.chunks(LIST_AHEAD) {
        // (Asked to stop: the room made so far does.)
        if room.enough()? || super::stopping() {
            break;
        }
        // Their NAS folders listed at once.
        let folders: BTreeSet<PathBuf> = ahead.iter().filter_map(|g| nas_path(cache, sources, &g.first()?.2)?.parent().map(Path::to_path_buf)).filter(|f| !listed.contains_key(f)).collect();
        // (A thread that can't start, or fails, leaves its folder to be listed when it's reached.)
        std::thread::scope(|s| {
            let lists: Vec<_> = folders.iter().filter_map(|f| std::thread::Builder::new().spawn_scoped(s, move || (f.clone(), list(f))).ok()).collect();
            listed.extend(lists.into_iter().filter_map(|h| h.join().ok()));
        });
        for (_, len, p) in ahead.iter().flatten() {
            if room.enough()? || super::stopping() {
                return Ok(room.by);
            }
            match fate(cache, sources, p, &mut listed) {
                Fate::Go => room.delete(p, *len),
                Fate::Copy(dest) if copy_there(p, &dest, &mut made) => room.delete(p, *len),
                Fate::Copy(_) | Fate::Stay => {}
            }
        }
    }
    Ok(room.by)
}

/// make_room_with's count of what it has deleted, against the free space it's after.
struct Room<'a> {
    cache: &'a Path,
    free_space: &'a dyn Fn(&Path) -> std::io::Result<u64>,
    target: u64,
    /// The target as it is now, when it may change midway (`free_cheap`'s `goal`): a change has the
    /// free space measured again at once.
    goal: Option<&'a dyn Fn() -> u64>,
    /// What was short of `target` when the free space was last measured, and the bytes deleted
    /// since.
    short: u64,
    since: u64,
    /// The bytes deleted, by cache (`kind`).
    by: BTreeMap<String, u64>,
}

impl Room<'_> {
    /// Whether the disk has `target` free: measured again once what was short is deleted, or
    /// every 2 GB (what a file held isn't always what deleting it frees: snapshots keep it).
    fn enough(&mut self) -> Result<bool> {
        let changed = match self.goal.map(|g| g()) {
            Some(t) if t != self.target => {
                self.target = t;
                true
            }
            _ => false,
        };
        if !changed && self.since < self.short.min(2 << 30) {
            return Ok(false);
        }
        let free = (self.free_space)(self.cache)?;
        if free >= self.target {
            return Ok(true);
        }
        (self.short, self.since) = (self.target - free, 0);
        Ok(false)
    }

    /// Deletes `p` unless a job holds it (store::cachefile).
    fn delete(&mut self, p: &Path, len: u64) {
        if let store::cachefile::Removed::Freed(_) = store::cachefile::try_remove(p) {
            let top = p.strip_prefix(self.cache).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned());
            *self.by.entry(top.as_deref().and_then(kind).unwrap_or("other").to_string()).or_default() += len;
            self.since += len;
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        // (Not a link: what it points to isn't the cache's.)
        let (Ok(t), Ok(md)) = (e.file_type(), e.metadata()) else { continue };
        if t.is_symlink() {
            continue;
        }
        if md.is_dir() {
            walk(&e.path(), out);
        } else {
            out.push((md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len(), e.path()));
        }
    }
}

/// `walk`'s files' paths, for other modules' tests.
#[cfg(test)]
pub(crate) fn walk_for_test(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut v = Vec::new();
    walk(dir, &mut v);
    out.extend(v.into_iter().map(|f| f.2));
}

/// The free space on the disk holding `path` (a local disk).
pub fn disk_free(path: &Path) -> std::io::Result<u64> {
    #[cfg(test)]
    if let Some(f) = TEST_FREE.with(|c| c.get()) {
        return Ok(f);
    }
    super::cond::free_bytes(path).ok_or_else(std::io::Error::last_os_error)
}

#[cfg(test)]
thread_local! {
    /// A test's disk, so what it asserts doesn't hang on this Mac's: its free bytes.
    pub static TEST_FREE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_room_spared(cache: &Path, sources: &Path, need: u64, target: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>) -> Result<u64> {
        make_room_with(cache, sources, need, target, free_space, &|_| false)
    }
    use std::cell::Cell;
    use std::time::Duration;

    fn file(p: &Path, len: usize, age_s: u64) {
        bytes(p, &vec![0u8; len], age_s);
    }

    fn bytes(p: &Path, b: &[u8], age_s: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
        let f = std::fs::File::options().append(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
    }

    /// A whole file of `p`'s kind (crate::whole), `age_s` old; its length.
    fn whole(p: &Path, age_s: u64) -> u64 {
        let b = if p.extension().is_some_and(|x| x == "png") { crate::whole::testfiles::png() } else { crate::whole::testfiles::tiff(false) };
        bytes(p, &b, age_s);
        b.len() as u64
    }

    /// The bytes under the caches' folders.
    fn used(c: &Path) -> u64 {
        let mut fs = Vec::new();
        for d in ["chm10", "aws-terrarium", "blobs"] {
            walk(&c.join(d), &mut fs);
        }
        fs.iter().map(|f| f.1).sum()
    }

    #[test]
    fn what_the_queued_jobs_read_goes_last() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        // The job's own area's archive, idle longest, and another area's.
        let own = whole(&c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles"), 9000);
        let other = whole(&c.join("aws-terrarium/packs/6-9-9.0000000000000002.tiles"), 7200);
        let all = used(c);
        let disk = |base: u64| move |p: &Path| Ok(base + all - used(p));
        let hint = |p: &Path| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("6-1-1."));
        // Short of one archive: the other area's goes, though the job's own was idle longer.
        assert_eq!(make_room_with(c, nas, 1000 + own, 1000 + own, &disk(1000), &hint).unwrap(), other);
        assert!(c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles").exists() && !c.join("aws-terrarium/packs/6-9-9.0000000000000002.tiles").exists());
        // Far short: it goes too, last (a hint, not a rule: it costs a copy, never a failure).
        assert_eq!(make_room_with(c, nas, 1 << 40, 1 << 40, &disk(0), &hint).unwrap(), own);
        // As the agent hints a terrain run's own area's archives (`terrain_reads`).
        let mut h = Hints::default();
        h.add("terrain", &["3/0/0".to_string()]);
        assert!(h.has(c, &c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles")) && !h.has(c, &c.join("aws-terrarium/packs/6-9-9.0000000000000002.tiles")));
        // A unit's canopy squares and the copies of its packs; an items job's pageview months.
        h.add("unit", &["6/33/22".to_string()]);
        h.add("items", &[]);
        assert!(h.has(c, &c.join("blobs/base/6-33-22.0123456789abcdef.base")) && !h.has(c, &c.join("blobs/base/6-33-23.0123456789abcdef.base")));
        assert!(h.squares.iter().all(|n| n.starts_with("meta_chm_lat=")) && !h.squares.is_empty());
        assert!(h.has(c, &c.join(MONTHS).join("2026-08.tsv.zst")));
    }

    #[test]
    fn a_file_a_job_holds_stays_and_counts_as_kept() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        file(&c.join("blobs/a/6-1-2.0000000000000001.pack"), 1000, 9000);
        file(&c.join("blobs/a/6-1-3.0000000000000002.pack"), 500, 7200);
        // A job holds the older (store::cachefile): room-making passes it over, whatever its age.
        let held = c.join("blobs/a/6-1-2.0000000000000001.pack");
        store::cachefile::hold_existing(&held).unwrap();
        let t = trim(c, nas, &|_| false).unwrap();
        assert_eq!((t.freed, t.left), (BTreeMap::from([("blobs".to_string(), 500)]), 1000));
        assert!(held.exists());
        // Let go: it goes.
        store::cachefile::release(&held);
        assert_eq!(trim(c, nas, &|_| false).unwrap().bytes(), 1000);
    }

    #[test]
    fn idle_copies_go_first_then_those_read_lately() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        // An area's archive idle longest, a canopy square idle two hours, another read a minute ago.
        let arch = whole(&c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles"), 9000);
        let idle = whole(&c.join("chm10/idle.tif"), 7200);
        file(&c.join("chm10/none.tif"), 0, 8000);
        let read = whole(&c.join("chm10/read.tif"), 60);
        file(&c.join("dem-cache.keys.u64"), 100, 9000);
        assert_eq!(cheap_bytes(c, None), arch + idle + read);
        // A disk with 850 free plus what's deleted.
        let all = used(c);
        let disk = |base: u64| move |p: &Path| Ok(base + all - used(p));
        // Short of the archive and all but a byte of the idle square: both go, the archive (idle
        // longer, and the NAS has it: nothing asked) first; the marker and the DEM seed stay.
        assert_eq!(make_room_spared(c, nas, 850 + arch + idle - 1, 850 + arch + idle - 1, &disk(850)).unwrap(), arch + idle);
        assert!(!c.join("chm10/idle.tif").exists() && !c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles").exists());
        assert!(c.join("chm10/read.tif").exists() && c.join("chm10/none.tif").exists());
        // What went is on the NAS (the square copied there first: it wasn't).
        assert!(nas.join("canopy/idle.tif").exists());
        // Room enough: nothing goes.
        assert_eq!(make_room_spared(c, nas, 1000, 1000, &|_| Ok(1 << 20)).unwrap(), 0);
        // Far short: every cheap file; never the DEM seed.
        make_room_spared(c, nas, 1 << 40, 1 << 40, &disk(0)).unwrap();
        assert!(!c.join("chm10/read.tif").exists() && c.join("dem-cache.keys.u64").exists());
        assert!(disk_free(c).unwrap() > 0);
    }

    #[test]
    fn raw_tiles_are_packed_onto_the_nas_before_they_go() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let root = &d.path().join("nas");
        let nas = &root.join("sources");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // Tiles AWS gave, the NAS without them; one too new to pack.
        let a = whole(&c.join("aws-terrarium/12/2048/1365.png"), 9000);
        whole(&c.join("aws-terrarium/12/2048/1366.png"), 9000);
        whole(&c.join("aws-terrarium/12/2049/1365.png"), 1);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        make_room_spared(c, nas, a, a, &disk).unwrap();
        // Packed onto the NAS (an archive for their area, named in the index), gone here.
        let index = crate::rawpack::Index::load(&nas.join("aws-terrarium")).unwrap();
        let name = &index.of("6-32-21")[0].name;
        assert!(nas.join("aws-terrarium/packs").join(name).exists());
        assert!(!c.join("aws-terrarium/packs").join(name).exists(), "the disk is short: not kept here");
        assert!(!c.join("aws-terrarium/12/2048/1365.png").exists() && !c.join("aws-terrarium/12/2048/1366.png").exists());
        // The newest waits here: the NAS hasn't it (packed later).
        assert!(c.join("aws-terrarium/12/2049/1365.png").exists());
        assert!(!nas.join("aws-terrarium/12/2049/1365.png").exists(), "never copied alone");
    }

    #[test]
    fn copies_of_the_records_files_go_without_asking_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        // An idle copy of a recorded pack, and a square read since: no NAS folder to ask exists.
        file(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), 1000, 7200);
        let square = whole(&c.join("chm10/read.tif"), 60);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        assert_eq!(make_room_spared(c, nas, 1000, 1000, &disk).unwrap(), 1000);
        assert!(!c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack").exists() && c.join("chm10/read.tif").exists());
        assert!(!nas.exists(), "nothing was listed or copied there");
        assert_eq!(used(c), square);
    }

    #[test]
    fn what_can_go_counts_only_the_months_the_nas_has() {
        // (The review's case, 2026-10-06.) An index the NAS lacks and the counts from before the
        // indexes stay: not counted as room (the OSM pass's admission adds it to the free space).
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        let months = c.join(MONTHS);
        file(&months.join("2026-05.tsv.zst"), 1000, 7200);
        file(&months.join("2026-08.json"), 300, 7200);
        file(&months.join("2026-08.counted.json"), 200, 7200);
        assert_eq!(cheap_bytes(c, Some(nas)), 0);
        assert_eq!(make_room_spared(c, nas, 1 << 40, 1 << 40, &|_| Ok(0)).unwrap(), 0, "none of it freed");
        // The NAS has it now: counted, and freed.
        file(&nas.join("pageviews/2026-05.tsv.zst"), 1000, 0);
        assert_eq!(cheap_bytes(c, Some(nas)), 1000);
        assert_eq!(make_room_spared(c, nas, 1 << 40, 1 << 40, &|_| Ok(0)).unwrap(), 1000);
    }

    #[test]
    fn a_pageview_months_index_goes_once_the_nas_has_it() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        // Two indexes the NAS has (one at another size), one it lacks, the counts from before the
        // indexes, a stream's temporary file, and the items job's answers beside them.
        let months = c.join(MONTHS);
        for (m, len) in [("2025-11", 1000), ("2026-02", 900), ("2026-05", 800)] {
            file(&months.join(format!("{m}.tsv.zst")), len, 7200);
        }
        file(&nas.join("pageviews/2025-11.tsv.zst"), 1000, 0);
        file(&nas.join("pageviews/2026-02.tsv.zst"), 10, 0);
        for f in ["2026-08.json", "2026-08.counted.json", "2026-08.tsv.zst.123.tmp"] {
            file(&months.join(f), 100, 7200);
        }
        file(&c.join("items/facts-2026-09-28.jsonl"), 100, 7200);
        // What room-making can free of them: the index the NAS has at its size, not the counts from
        // before the indexes nor one it lacks (the review's case, 2026-10-06: the OSM pass counts it
        // as room); none without the NAS to ask.
        assert_eq!((cheap_bytes(c, Some(nas)), cheap_bytes(c, None)), (1000, 0));
        let all = used_with(c, &["items"]);
        let disk = move |p: &Path| Ok(all - used_with(p, &["items"]));
        assert_eq!(make_room_spared(c, nas, 1 << 40, 1 << 40, &disk).unwrap(), 1000);
        assert!(!months.join("2025-11.tsv.zst").exists());
        for f in ["2026-02.tsv.zst", "2026-05.tsv.zst", "2026-08.json", "2026-08.counted.json", "2026-08.tsv.zst.123.tmp"] {
            assert!(months.join(f).exists(), "{f} stays");
        }
        assert!(c.join("items/facts-2026-09-28.jsonl").exists());
        assert!(!nas.join("pageviews/2026-05.tsv.zst").exists(), "never copied there from here");
        file(&nas.join("pageviews/2026-02.tsv.zst"), 900, 0);
        // A trim: the one the NAS has now goes; the one it lacks stays, said as kept (the counts
        // from before the indexes stay unsaid: nothing would take them).
        let t = trim(c, nas, &|_| false).unwrap();
        assert_eq!((t.freed, t.left), (BTreeMap::from([("months".to_string(), 900)]), 800));
        assert!(months.join("2026-05.tsv.zst").exists() && months.join("2026-08.json").exists());
    }

    /// The bytes under the caches' folders and `more` of the cache's.
    fn used_with(c: &Path, more: &[&str]) -> u64 {
        let mut fs = Vec::new();
        for d in more {
            walk(&c.join(d), &mut fs);
        }
        used(c) + fs.iter().map(|f| f.1).sum::<u64>()
    }

    #[test]
    fn it_stops_once_the_disk_has_room() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        for i in 0..4 {
            file(&c.join(format!("chm10/{i}.tif")), 1 << 20, 100 - i);
        }
        // 2 MB short: two files, the free space measured again, and it stops.
        let calls = Cell::new(0);
        let all = used(c);
        let freed = make_room_spared(c, nas, 10 << 20, 10 << 20, &|p| {
            calls.set(calls.get() + 1);
            Ok((8 << 20) + all - used(p))
        })
        .unwrap();
        assert_eq!(freed, 2 << 20);
        assert_eq!(calls.get(), 2);
        assert!(!c.join("chm10/0.tif").exists() && !c.join("chm10/1.tif").exists() && c.join("chm10/3.tif").exists());
    }

    #[test]
    fn a_folder_goes_whole_the_least_recently_used_first() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        // (The NAS takes no archive here: tiles it lacks stay, and those it has loose follow the
        // rules for loose files.)
        std::fs::create_dir_all(nas.join("aws-terrarium")).unwrap();
        std::fs::write(nas.join("aws-terrarium/packs"), b"").unwrap();
        // Column 1 holds the oldest tile, but was used since; column 2's are all older than that.
        // (The NAS has them all.)
        for (t, age) in [("1/1", 5000), ("1/2", 100), ("2/1", 4000), ("2/2", 3000), ("3/1", 9500), ("3/2", 9000)] {
            whole(&nas.join(format!("aws-terrarium/12/{t}.png")), 0);
            if !t.starts_with('3') {
                whole(&c.join(format!("aws-terrarium/12/{t}.png")), age);
            }
        }
        let (a, b) = (std::fs::metadata(c.join("aws-terrarium/12/2/1.png")).unwrap().len(), std::fs::metadata(c.join("aws-terrarium/12/2/2.png")).unwrap().len());
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        // Short of column 2: it goes, column 1 stays.
        assert_eq!(make_room_spared(c, nas, a + b, a + b, &disk).unwrap(), a + b);
        assert!(c.join("aws-terrarium/12/1/1.png").exists() && c.join("aws-terrarium/12/1/2.png").exists());
        assert!(!c.join("aws-terrarium/12/2/1.png").exists() && !c.join("aws-terrarium/12/2/2.png").exists());
        // Canopy squares idle an hour go before any tile, each by its own use, not its folder's: of
        // two in chm10/, the one read longer ago goes, before column 3, idle longer still; the
        // other stays.
        whole(&c.join("aws-terrarium/12/3/1.png"), 9500);
        whole(&c.join("aws-terrarium/12/3/2.png"), 9000);
        let square = whole(&c.join("chm10/old.tif"), 4500);
        whole(&c.join("chm10/new.tif"), 4000);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        assert_eq!(make_room_spared(c, nas, square, square, &disk).unwrap(), square);
        assert!(!c.join("chm10/old.tif").exists() && c.join("chm10/new.tif").exists());
        assert!(c.join("aws-terrarium/12/3/1.png").exists() && c.join("aws-terrarium/12/1/1.png").exists());
    }

    #[test]
    fn a_helpers_trim_empties_the_cheap_caches_but_what_the_nas_lacks() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let root = &d.path().join("nas");
        let nas = &root.join("sources");
        // (Another Mac writes the records: this one packs no raw tiles itself.)
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/writer"), "the-build-mac").unwrap();
        // A canopy square the NAS lacks, an empty marker, a copy of a recorded file and of an
        // archive, a raw tile the NAS has loose and one it lacks; and caches only a clear empties.
        let square = whole(&c.join("chm10/a.tif"), 60);
        file(&c.join("chm10/none.tif"), 0, 60);
        file(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), 1000, 60);
        let arch = whole(&c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles"), 60);
        let has = whole(&c.join("aws-terrarium/12/2048/1366.png"), 9000);
        whole(&nas.join("aws-terrarium/12/2048/1366.png"), 0);
        let lacks = whole(&c.join("aws-terrarium/12/2048/1365.png"), 9000);
        for f in ["base/base/6-1-1.0000000000000001.base", "dem-cache.keys.u64", "sources-terrain-z8-v1/terrain-z8-v1.0000000000000001.pack", "items/facts-2026-09-28.jsonl"] {
            file(&c.join(f), 100, 60);
        }
        let f = trim(c, nas, &|_| false).unwrap();
        assert_eq!(f.freed, BTreeMap::from([("blobs".to_string(), 1000), ("canopy".to_string(), square), ("terrain".to_string(), arch + has)]));
        assert_eq!(f.left, lacks, "the raw tile the NAS lacks stays: only a job of this Mac's packs it");
        assert!(c.join("aws-terrarium/12/2048/1365.png").exists() && c.join("chm10/none.tif").exists());
        // The square copied onto the NAS before it went.
        assert_eq!(std::fs::metadata(nas.join("canopy/a.tif")).unwrap().len(), square);
        // The others stay: a trim empties the cheap caches alone.
        for f in ["base/base/6-1-1.0000000000000001.base", "dem-cache.keys.u64", "sources-terrain-z8-v1/terrain-z8-v1.0000000000000001.pack", "items/facts-2026-09-28.jsonl"] {
            assert!(c.join(f).exists(), "{f}");
        }
        let said = f.say();
        assert!(said.starts_with("0 MB freed (") && said.contains("canopy squares 0 MB") && said.ends_with("0 MB kept (the NAS hasn't it yet)"), "{said}");
        assert_eq!((size(350 << 20), size((51 << 30) + (1 << 29))), ("350 MB".to_string(), "51.5 GB".to_string()));
    }

    #[test]
    fn the_build_macs_trim_keeps_the_canopy_squares_and_packs_raw_tiles_first() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let root = &d.path().join("nas");
        let nas = &root.join("sources");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let squares = c.join("chm10");
        let spare = |p: &Path| p.starts_with(&squares);
        whole(&c.join("chm10/a.tif"), 60);
        file(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), 1000, 60);
        let tiles = whole(&c.join("aws-terrarium/12/2048/1365.png"), 9000) + whole(&c.join("aws-terrarium/12/2048/1366.png"), 9000);
        let f = trim(c, nas, &spare).unwrap();
        // The raw tiles packed onto the NAS (an archive for their area, named in the index), gone
        // here with the copy; the square kept, and not counted as left.
        let index = crate::rawpack::Index::load(&nas.join("aws-terrarium")).unwrap();
        assert!(nas.join("aws-terrarium/packs").join(&index.of("6-32-21")[0].name).exists());
        assert!(!c.join("aws-terrarium/12/2048/1365.png").exists() && !c.join("aws-terrarium/12/2048/1366.png").exists());
        assert!(!c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack").exists() && c.join("chm10/a.tif").exists());
        assert_eq!(f.freed, BTreeMap::from([("blobs".to_string(), 1000), ("terrain".to_string(), tiles)]));
        assert_eq!(f.left, 0);
        assert!(!nas.join("canopy").exists(), "the square wasn't asked about");
        // Again: nothing more to free.
        assert_eq!(trim(c, nas, &spare).unwrap().bytes(), 0);
    }

    #[test]
    fn a_clear_names_the_pageview_months_it_frees() {
        // (The review's case, 2026-10-06.) The months the NAS has: in what a clear would free (the
        // owner's confirmation), freed as themselves by a clear and by the build Mac's trim; one it
        // lacks stays, said as kept; the counts from before the indexes neither.
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("app/agent/cache");
        let nas = &d.path().join("nas/sources");
        std::fs::create_dir_all(d.path().join("nas/state/build")).unwrap();
        file(&c.join(MONTHS).join("2026-05.tsv.zst"), 600, 7200);
        file(&nas.join("pageviews/2026-05.tsv.zst"), 600, 0);
        file(&c.join(MONTHS).join("2026-08.tsv.zst"), 50, 7200);
        file(&c.join(MONTHS).join("2026-08.json"), 30, 7200);
        let said = sizes(c, false, nas.parent());
        assert_eq!((said.clear.get("months"), said.cheap), (Some(&600), 600));
        assert_eq!(sizes(c, false, None).clear.get("months"), None, "not without the NAS to ask");
        let g = gone(&said.clear, 60.0);
        assert_eq!((g[0].what.as_str(), g[0].back.starts_with("copied back from the NAS when the items and heritage jobs next run")), ("pageview months", true));
        let f = clear(c, nas).unwrap();
        assert!(!c.join(MONTHS).join("2026-05.tsv.zst").exists());
        assert_eq!((f.freed.clone(), f.left), (BTreeMap::from([("months".to_string(), 600)]), 50));
        assert!(f.say().contains("pageview months 0 MB"), "{}", f.say());
        // The build Mac's trim (the canopy squares spared) takes them too.
        file(&c.join(MONTHS).join("2026-05.tsv.zst"), 600, 7200);
        let t = trim(c, nas, &|p| p.starts_with(c.join("chm10"))).unwrap();
        assert_eq!((t.freed.get("months"), t.left), (Some(&600), 50));
        assert!(c.join(MONTHS).join("2026-08.tsv.zst").exists() && c.join(MONTHS).join("2026-08.json").exists());
    }

    #[test]
    fn a_clear_empties_what_the_nas_fills_again_and_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let home = &d.path().join("app/agent");
        let c = &home.join("cache");
        let nas = &d.path().join("nas/sources");
        std::fs::create_dir_all(d.path().join("nas/state/build")).unwrap();
        // The cheap caches; the others a later job fills again from the NAS, or makes again from
        // what's there (the NAS has the DEM seed at its sizes here).
        let square = whole(&c.join("chm10/a.tif"), 60);
        file(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), 1000, 60);
        let others = ["base/base/6-1-1.0000000000000001.base", "base/global/roads/6-1-1.0000000000000002.roads", "sources-terrain-z8-v1/terrain-z8-v1.0000000000000001.pack", "work-summits-2026-09-28/2026-09-28.0000000000000001.jsonl.zst", "heritage-merged-2026-09-28-0123456789ab.osm.pbf"];
        for f in others {
            file(&c.join(f), 100, 60);
        }
        for (n, len) in [("keys.u64", 80), ("elev.f32", 40), ("src.u8", 10)] {
            file(&c.join(format!("dem-cache.{n}")), len, 60);
            file(&nas.join(format!("dem-cache/dem-cache.{n}")), len, 60);
        }
        // What stays: the answers the NAS keeps too (they free little), a pageview month the NAS
        // lacks (said as kept: pageviews.py puts it there when it next reads it), what frees next to
        // nothing, the agent's own timings, what units kept that isn't on the NAS yet; and
        // everything outside the caches (the agent's state, logs, work and outbox, the map's
        // mirror).
        let kept = ["items/facts-2026-09-28.jsonl", "items/months/2026-05.tsv.zst", "heritage-2026-09-28-0123456789ab/.done", "registers-0123456789ab/.done", "rail/pairs-0123456789abcdef.bin", "unit-stages.json", "dem-units/6-1-1.dem", "scenic-units/6-1-1/canopy.keys"];
        for f in kept {
            file(&c.join(f), 100, 60);
        }
        let outside = [home.join("state.json"), home.join("logs/unit-6-1-1.log"), home.join("scratch/unit/verts.bin"), home.join("outbox/7/result.json"), home.join("clear-request.json"), d.path().join("app/mirror/base/6-1-1.0000000000000001.base")];
        for p in &outside {
            file(p, 100, 60);
        }
        filtered_planet(&d.path().join("nas"));
        let f = clear(c, nas).unwrap();
        assert_eq!(f.freed, BTreeMap::from([("base".to_string(), 200), ("blobs".to_string(), 1000), ("canopy".to_string(), square), ("copies".to_string(), 200), ("dem".to_string(), 130), ("heritage".to_string(), 100)]));
        assert_eq!(f.left, 100, "the month the NAS lacks");
        assert!(nas.join("canopy/a.tif").exists(), "the square the NAS lacked copied there first");
        for e in std::fs::read_dir(c).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            assert!(kept.iter().any(|k| k.starts_with(&format!("{n}/")) || *k == n) || ["chm10", "blobs", "aws-terrarium"].contains(&n.as_str()), "{n} left");
        }
        for k in kept {
            assert!(c.join(k).exists(), "{k} kept");
        }
        for p in &outside {
            assert!(p.exists(), "{} untouched", p.display());
        }
        // A seed here of another count than the NAS's (the review's case), or cut short (a copy that
        // stopped), the NAS's whole: it goes too.
        for (n, len) in [("keys.u64", 160), ("elev.f32", 80), ("src.u8", 20)] {
            file(&c.join(format!("dem-cache.{n}")), len, 60);
        }
        assert_eq!(clear(c, nas).unwrap().freed.get("dem"), Some(&260));
        file(&c.join("dem-cache.keys.u64"), 80, 60);
        file(&c.join("dem-cache.elev.f32"), 12, 60);
        assert_eq!(clear(c, nas).unwrap().freed.get("dem"), Some(&92));
        // The NAS's not whole (its files not of one count): the seed here stays, but a temporary
        // file of it.
        for (n, len) in [("keys.u64", 80), ("elev.f32", 40), ("src.u8", 10)] {
            file(&c.join(format!("dem-cache.{n}")), len, 60);
        }
        file(&c.join("dem-cache.keys.u64.m1.123.tmp"), 30, 60);
        std::fs::write(nas.join("dem-cache/dem-cache.src.u8"), b"x").unwrap();
        let f = clear(c, nas).unwrap();
        assert_eq!((f.bytes(), f.left), (30, 130 + 100));
        assert!(c.join("dem-cache.keys.u64").exists() && !c.join("dem-cache.keys.u64.m1.123.tmp").exists());
    }

    #[test]
    fn toward_the_target_frees_only_as_far_as_needed_the_cheapest_to_refill_first() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("app/agent/cache");
        let nas = &d.path().join("nas/sources");
        std::fs::create_dir_all(d.path().join("nas/state/build")).unwrap();
        // The cheap caches (copies of the records' files), then the others: the copies of the
        // NAS's files, three base packs (the oldest used longest ago), the DEM seed (the NAS has it
        // whole) and the heritage clip.
        file(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), 1000, 60);
        file(&c.join("sources-terrain-z8-v1/terrain-z8-v1.0000000000000001.pack"), 500, 60);
        for (i, age) in [(1, 300), (2, 200), (3, 100)] {
            file(&c.join(format!("base/base/6-1-{i}.0000000000000001.base")), 2000, age);
        }
        for (n, len) in [("keys.u64", 80), ("elev.f32", 40), ("src.u8", 10)] {
            file(&c.join(format!("dem-cache.{n}")), len, 60);
            file(&nas.join(format!("dem-cache/dem-cache.{n}")), len, 60);
        }
        file(&c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf"), 700, 60);
        // (The pass's filtered planet it's clipped from, on the NAS.)
        filtered_planet(&d.path().join("nas"));
        let all = |p: &Path| {
            let mut fs = Vec::new();
            walk(p, &mut fs);
            fs.iter().map(|f| f.1).sum::<u64>()
        };
        let held = all(c);
        // A disk of `held` bytes, with what's deleted free: a target of 3500 takes the blobs (1000),
        // the copies (500), then base packs a file at a time, the oldest first, as far as needed:
        // one.
        let disk = move |p: &Path| Ok(held - all(p));
        let f = toward_with(c, nas, &|| 3500, &disk, &|_| false).unwrap();
        assert_eq!(f.freed, BTreeMap::from([("base".to_string(), 2000), ("blobs".to_string(), 1000), ("copies".to_string(), 500)]));
        assert_eq!((f.target, f.left), (Some(3500), 0));
        assert!(!c.join("base/base/6-1-1.0000000000000001.base").exists() && c.join("base/base/6-1-2.0000000000000001.base").exists());
        assert!(c.join("dem-cache.keys.u64").exists() && c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf").exists());
        // A little more: the next base pack.
        assert_eq!(toward_with(c, nas, &|| 4000, &disk, &|_| false).unwrap().freed, BTreeMap::from([("base".to_string(), 2000)]));
        // Already there: nothing goes.
        assert_eq!(toward_with(c, nas, &|| 3500, &disk, &|_| false).unwrap(), Freed::default());
        // A target past everything: the DEM seed whole, then the heritage clip; short, and what
        // stays said (nothing here).
        let f = toward_with(c, nas, &|| u64::MAX, &disk, &|_| false).unwrap();
        assert_eq!(f.freed, BTreeMap::from([("base".to_string(), 2000), ("dem".to_string(), 130), ("heritage".to_string(), 700)]));
        assert_eq!(f.left, 0);
        assert!(!c.join("dem-cache.src.u8").exists());
        // A seed the NAS hasn't whole stays, counted as left.
        for (n, len) in [("keys.u64", 80), ("elev.f32", 40), ("src.u8", 10)] {
            file(&c.join(format!("dem-cache.{n}")), len, 60);
        }
        std::fs::write(nas.join("dem-cache/dem-cache.src.u8"), b"x").unwrap();
        let f = toward_with(c, nas, &|| u64::MAX, &disk, &|_| false).unwrap();
        assert_eq!((f.bytes(), f.left), (0, 130));
    }

    /// The pass's filtered planet of 2026-09-28 in the NAS's manifest under `root`, and there.
    fn filtered_planet(root: &Path) {
        file(&root.join("sources/osm/2026-09-28/filtered.0000000000000001.osm.pbf"), 10, 60);
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), br#"{"sources/osm/2026-09-28/filtered":"sources/osm/2026-09-28/filtered.0000000000000001.osm.pbf"}"#).unwrap();
    }

    #[test]
    fn the_heritage_clip_stays_while_it_cant_be_made_again() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        std::fs::create_dir_all(nas).unwrap();
        let clip = c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf");
        file(&clip, 700, 60);
        let all = |p: &Path| {
            let mut fs = Vec::new();
            walk(p, &mut fs);
            fs.iter().map(|f| f.1).sum::<u64>()
        };
        let disk = move |p: &Path| Ok(700 - all(p));
        // No manifest naming the planet it's clipped from: kept, by a clear too.
        let f = toward_with(c, nas, &|| u64::MAX, &disk, &|_| false).unwrap();
        assert_eq!((f.bytes(), f.left), (0, 700));
        assert_eq!((clear(c, nas).unwrap().bytes(), clip.exists()), (0, true));
        // Named, but not there: kept.
        std::fs::create_dir_all(d.path().join("nas/state/build")).unwrap();
        std::fs::write(d.path().join("nas/state/build/manifest.json"), br#"{"sources/osm/2026-09-28/filtered":"sources/osm/2026-09-28/filtered.0000000000000001.osm.pbf"}"#).unwrap();
        assert!(toward_with(c, nas, &|| u64::MAX, &disk, &|_| false).unwrap().freed.is_empty() && clip.exists());
        // There: it goes.
        filtered_planet(&d.path().join("nas"));
        assert_eq!(toward_with(c, nas, &|| u64::MAX, &disk, &|_| false).unwrap().freed.get("heritage"), Some(&700));
    }

    #[test]
    fn a_target_lowered_midway_stops_the_freeing() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        std::fs::create_dir_all(nas).unwrap();
        filtered_planet(&d.path().join("nas"));
        file(&c.join("sources-a/x.pack"), 500, 60);
        for (i, age) in [(1, 300), (2, 200), (3, 100)] {
            file(&c.join(format!("base/base/6-1-{i}.0000000000000001.base")), 2000, age);
        }
        file(&c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf"), 700, 60);
        let all = |p: &Path| {
            let mut fs = Vec::new();
            walk(p, &mut fs);
            fs.iter().map(|f| f.1).sum::<u64>()
        };
        let held = all(c);
        // A target past everything, lowered to nothing (off) once the first base pack has gone.
        let first = c.join("base/base/6-1-1.0000000000000001.base");
        let goal = || if first.exists() { u64::MAX } else { 0 };
        let disk = move |p: &Path| Ok(held - all(p));
        let f = toward_with(c, nas, &goal, &disk, &|_| false).unwrap();
        assert_eq!(f.freed, BTreeMap::from([("base".to_string(), 2000), ("copies".to_string(), 500)]));
        assert!(c.join("base/base/6-1-2.0000000000000001.base").exists() && c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf").exists());
    }

    #[test]
    fn toward_the_target_stops_once_its_there() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        std::fs::create_dir_all(nas).unwrap();
        file(&c.join("sources-a/x.pack"), 500, 60);
        file(&c.join("sources-b/x.pack"), 500, 60);
        file(&c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf"), 700, 60);
        let held = 1700;
        let all = |p: &Path| {
            let mut fs = Vec::new();
            walk(p, &mut fs);
            fs.iter().map(|f| f.1).sum::<u64>()
        };
        let disk = move |p: &Path| Ok(held - all(p));
        let f = toward_with(c, nas, &|| 400, &disk, &|_| false).unwrap();
        assert_eq!(f.freed, BTreeMap::from([("copies".to_string(), 500)]));
        assert!(!c.join("sources-a").exists() && c.join("sources-b/x.pack").exists() && c.join("heritage-merged-2026-09-28-0123456789ab.osm.pbf").exists());
    }

    #[test]
    fn the_room_target_is_set_read_and_cleared_in_the_agents_folder() {
        let d = tempfile::tempdir().unwrap();
        let home = &d.path().join("agent");
        assert!(target(home).is_none());
        let t = set_target(home, Some(100 << 30), "scenic room on m4").unwrap().unwrap();
        assert_eq!(target(home), Some(t));
        // The menu bar's, as it writes it.
        std::fs::write(home.join(TARGET), br#"{"bytes":53687091200,"by":"the menu bar on m4","at":1791300000}"#).unwrap();
        assert_eq!(target(home).map(|t| (t.bytes, t.by)), Some((50 << 30, "the menu bar on m4".to_string())));
        // None of 0 bytes, nor a damaged one; off, it goes (twice, no error).
        std::fs::write(home.join(TARGET), br#"{"bytes":0,"by":"x","at":1}"#).unwrap();
        assert!(target(home).is_none());
        std::fs::write(home.join(TARGET), b"{").unwrap();
        assert!(target(home).is_none());
        assert_eq!(set_target(home, None, "x").unwrap(), None);
        assert_eq!(set_target(home, None, "x").unwrap(), None);
        assert!(!home.join(TARGET).exists());
    }

    #[test]
    fn a_clear_ask_waits_in_the_agents_folder_until_taken_up() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        assert!(take_clear(home).is_none());
        let r = request_clear(home, "scenic clean on m1").unwrap();
        assert_eq!(take_clear(home), Some(r.clone()));
        // Taken up, it's set aside: one written meanwhile (the menu bar's, as it writes it) waits
        // its turn, and is taken up once the first is answered. (An agent that stopped before it
        // answered: the next takes the first up again.)
        std::fs::write(home.join(CLEAR_REQUEST), br#"{"by":"the menu bar on m1","at":1791300000}"#).unwrap();
        assert_eq!(take_clear(home), Some(r));
        clear_answered(home);
        let menu = take_clear(home).unwrap();
        assert_eq!((menu.by.as_str(), menu.at), ("the menu bar on m1", 1791300000));
        clear_answered(home);
        assert!(take_clear(home).is_none());
        // A damaged one goes.
        std::fs::write(home.join(CLEAR_REQUEST), b"{").unwrap();
        assert!(take_clear(home).is_none() && !home.join(CLEAR_REQUEST).exists() && !home.join(CLEAR_TAKEN).exists());
    }

    /// A helper's NAS (the records' writer named elsewhere): its project folder and `sources/`.
    fn helpers_nas(d: &Path) -> (PathBuf, PathBuf) {
        let root = d.join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/writer"), "the-build-mac").unwrap();
        (root.clone(), root.join("sources"))
    }

    #[test]
    fn a_trim_leaves_a_linked_squares_folders_target() {
        // (The review's case.) The squares read where they lie: chm10 a link to the NAS's canopy
        // folder. Its files are the NAS's only copies: neither deleted nor counted.
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let (root, nas) = helpers_nas(d.path());
        whole(&nas.join("canopy/a.tif"), 60);
        std::fs::create_dir_all(c).unwrap();
        std::os::unix::fs::symlink(nas.join("canopy"), c.join("chm10")).unwrap();
        let f = trim(c, &nas, &|_| false).unwrap();
        let g = clear(c, &nas).unwrap();
        assert!(nas.join("canopy/a.tif").exists(), "the NAS's copy deleted through the link");
        assert_eq!((f.bytes(), f.left, g.bytes(), cheap_bytes(c, Some(&nas))), (0, 0, 0, 0));
        assert_eq!((sizes(c, false, Some(root.as_path())), sizes(c, true, Some(root.as_path()))), (Sizes::default(), Sizes::default()));
    }

    #[test]
    fn a_link_inside_the_squares_folder_keeps_the_nas_files() {
        // (The review's case.) A folder of squares and a square in chm10/, links to the NAS's.
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let (_, nas) = helpers_nas(d.path());
        whole(&nas.join("canopy/a.tif"), 60);
        let own = whole(&c.join("chm10/b.tif"), 60);
        std::os::unix::fs::symlink(nas.join("canopy"), c.join("chm10/sub")).unwrap();
        std::os::unix::fs::symlink(nas.join("canopy/a.tif"), c.join("chm10/a.tif")).unwrap();
        let f = trim(c, &nas, &|_| false).unwrap();
        assert!(nas.join("canopy/a.tif").exists() && c.join("chm10/a.tif").exists());
        // Its own square, copied there first, gone here.
        assert_eq!((f.freed.get("canopy"), std::fs::metadata(nas.join("canopy/b.tif")).unwrap().len()), (Some(&own), own));
    }

    #[test]
    fn a_cache_linked_into_the_nas_folder_loses_nothing_there() {
        // (The review's case.) The cache itself a link into the NAS's project folder.
        let d = tempfile::tempdir().unwrap();
        let (root, nas) = helpers_nas(d.path());
        let there = root.join("cache-of-m1");
        whole(&there.join("chm10/a.tif"), 60);
        file(&there.join("blobs/layers/x.0000000000000001.pack"), 100, 60);
        file(&there.join("base/base/6-1-1.0000000000000001.base"), 100, 60);
        let c = &d.path().join("cache");
        std::os::unix::fs::symlink(&there, c).unwrap();
        assert_eq!((trim(c, &nas, &|_| false).unwrap().bytes(), clear(c, &nas).unwrap().bytes()), (0, 0));
        assert!(there.join("chm10/a.tif").exists() && there.join("blobs/layers/x.0000000000000001.pack").exists() && there.join("base/base/6-1-1.0000000000000001.base").exists());
    }

    #[test]
    fn a_linked_tile_folder_is_neither_packed_nor_deleted() {
        // (The review's case.) The build Mac's aws-terrarium/12/2048 a link to a folder elsewhere:
        // its tiles aren't this cache's to pack, nor to delete where they lie.
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let root = &d.path().join("nas");
        let nas = &root.join("sources");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let elsewhere = d.path().join("elsewhere/12/2048");
        whole(&elsewhere.join("1365.png"), 9000);
        std::fs::create_dir_all(c.join("aws-terrarium/12")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, c.join("aws-terrarium/12/2048")).unwrap();
        // (And a tile of its own beside it, packed as ever.)
        let own = whole(&c.join("aws-terrarium/12/2049/1365.png"), 9000);
        let f = trim(c, nas, &|_| false).unwrap();
        assert!(elsewhere.join("1365.png").exists(), "deleted through the link");
        assert_eq!(f.freed.get("terrain"), Some(&own));
        assert!(!c.join("aws-terrarium/12/2049/1365.png").exists());
    }

    #[test]
    fn a_clear_leaves_a_linked_folders_target() {
        // (The review's case.) The pack cache a link to another folder: its target stays.
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        std::fs::create_dir_all(nas).unwrap();
        let elsewhere = d.path().join("mirror");
        file(&elsewhere.join("base/6-1-1.0000000000000001.base"), 100, 60);
        std::fs::create_dir_all(c).unwrap();
        std::os::unix::fs::symlink(&elsewhere, c.join("base")).unwrap();
        let f = clear(c, nas).unwrap();
        assert!(elsewhere.join("base/6-1-1.0000000000000001.base").exists() && c.join("base").exists());
        assert_eq!((f.bytes(), f.left), (0, 0));
    }

    #[test]
    fn once_short_it_frees_the_margin_too() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas/sources");
        for i in 0..6 {
            file(&c.join(format!("chm10/{i}.tif")), 1 << 20, 100 - i);
        }
        let all = used(c);
        let disk = move |p: &Path| Ok((1 << 20) + all - used(p));
        // 1 MB free: not short of 1 MB, so nothing goes, though it's under 3 MB.
        assert_eq!(make_room_spared(c, nas, 1 << 20, 3 << 20, &disk).unwrap(), 0);
        // Short of 2 MB: freed to 4 MB (three files), not just to 2 MB.
        assert_eq!(make_room_spared(c, nas, 2 << 20, 4 << 20, &disk).unwrap(), 3 << 20);
        assert_eq!(margin(30 << 30), 5 << 30);
    }
}

#[cfg(test)]
mod nas_tests {
    use super::*;

    #[test]
    fn a_file_a_short_listing_lacks_is_asked_about_before_a_copy() {
        let d = tempfile::tempdir().unwrap();
        let (c, nas) = (d.path().join("cache"), d.path().join("nas"));
        let png = crate::whole::testfiles::png();
        for p in [c.join("aws-terrarium/9/1/2.png"), nas.join("aws-terrarium/9/1/2.png")] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &png).unwrap();
        }
        // The folder's listing came back without it (cut short): it's on the NAS, so it may go.
        let mut listed = Listed::new();
        listed.insert(nas.join("aws-terrarium/9/1"), Some(HashMap::new()));
        assert_eq!(fate(&c, &nas, &c.join("aws-terrarium/9/1/2.png"), &mut listed), Fate::Go);
        // A folder that couldn't be listed keeps its raw tiles.
        listed.insert(nas.join("aws-terrarium/9/1"), None);
        assert_eq!(fate(&c, &nas, &c.join("aws-terrarium/9/1/2.png"), &mut listed), Fate::Stay);
        // Its canopy squares are asked about alone: one the NAS has at its size may go, one it
        // lacks stays.
        let tif = crate::whole::testfiles::tiff(false);
        for p in [c.join("chm10/a.tif"), nas.join("canopy/a.tif"), c.join("chm10/b.tif")] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &tif).unwrap();
        }
        listed.insert(nas.join("canopy"), None);
        assert_eq!(fate(&c, &nas, &c.join("chm10/a.tif"), &mut listed), Fate::Go);
        assert_eq!(fate(&c, &nas, &c.join("chm10/b.tif"), &mut listed), Fate::Stay);
    }

    #[test]
    fn a_file_the_nas_cant_take_stays() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("cache");
        std::fs::create_dir_all(c.join("chm10")).unwrap();
        std::fs::write(c.join("chm10/a.tif"), crate::whole::testfiles::tiff(false)).unwrap();
        // The NAS's store is a file, not a folder: nothing can be copied there.
        let nas = d.path().join("nas/sources");
        std::fs::create_dir_all(d.path().join("nas")).unwrap();
        std::fs::write(&nas, b"").unwrap();
        assert_eq!(make_room_with(&c, &nas, 1000, 1000, &|_| Ok(0), &|_| false).unwrap(), 0);
        assert!(c.join("chm10/a.tif").exists());
    }

    #[test]
    fn only_whole_files_are_kept_on_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let (c, nas) = (d.path().join("cache"), d.path().join("nas/sources"));
        let png = crate::whole::testfiles::png();
        let put = |p: &Path, b: &[u8]| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b).unwrap();
        };
        // Cut short, or a temporary file: deleted here, not kept there.
        put(&c.join("aws-terrarium/9/1/1.png"), &png[..png.len() - 5]);
        put(&c.join("chm10/a.tif.m4.12.tmp"), &crate::whole::testfiles::tiff(false));
        // The NAS has it at the same size: deleted here, the NAS's copy left as it is.
        put(&c.join("aws-terrarium/9/1/2.png"), &png);
        put(&nas.join("aws-terrarium/9/1/2.png"), &vec![7u8; png.len()]);
        // The NAS has it cut short: a canopy square's copy replaced with this whole one; a raw tile
        // kept here, to reach it in bulk.
        let tif = crate::whole::testfiles::tiff(false);
        put(&c.join("chm10/b.tif"), &tif);
        put(&nas.join("canopy/b.tif"), &tif[..10]);
        put(&c.join("aws-terrarium/9/1/3.png"), &png);
        put(&nas.join("aws-terrarium/9/1/3.png"), &png[..10]);
        make_room_with(&c, &nas, 1 << 40, 1 << 40, &|_| Ok(0), &|_| false).unwrap();
        for f in ["aws-terrarium/9/1/1.png", "chm10/a.tif.m4.12.tmp", "aws-terrarium/9/1/2.png", "chm10/b.tif"] {
            assert!(!c.join(f).exists(), "{f} deleted");
        }
        assert!(!nas.join("aws-terrarium/9/1/1.png").exists() && !nas.join("canopy/a.tif.m4.12.tmp").exists());
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/2.png")).unwrap(), vec![7u8; png.len()]);
        assert_eq!(std::fs::read(nas.join("canopy/b.tif")).unwrap(), tif);
        assert!(c.join("aws-terrarium/9/1/3.png").exists());
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/3.png")).unwrap(), png[..10].to_vec());
    }
}
