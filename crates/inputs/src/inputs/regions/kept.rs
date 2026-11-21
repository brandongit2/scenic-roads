//! The regions' check's resolutions kept as pieces (task #897; docs/pool.md §7.2): each region
//! without water resolved (`resolve::resolve_one`, by its memo key), and each region with water's
//! land, the folders' remainders and its water (`tiles`, each by what it reads: `tiles::Reads`),
//! and each cell's part of the cover check (`tiles::CoverReads`, all the cell's check is given),
//! kept as it's made in the job's scratch (jobrt::resume_log::Pieces), each a safe point. A check
//! resumed after a pause takes what its last kept, by the same keys (a digest of everything a
//! resolution reads), so it resolves to the same geometry and holds the same files.
//!
//! A scratch check (`scenic-build regions-check --keep <dir>`, task #1548) and the pool's (its Mac's
//! cache, `POOL_DIR`, task #1566) keep them in a lasting folder too (`Lasting`), a file each, by the same keys and the code that made them (`RESOLUTION`
//! and the regions' code digest, `inputs::code_of`): a check after an edit takes every piece whose
//! reads are as they were and makes only those that read the edit. The folder's size is held to
//! its cap (`--keep-mb`) once the check is done, the least recently used pieces deleted first
//! (`Lasting::sweep`).
//!
//! A pause comes while regions are resolved side by side: the piece kept, the rest stop making
//! (`get_or_make` hands back None, the caller gives up on that region) and the check returns the
//! pause once its resolution returns (`stopped`), keeping nothing of what was given up.

use super::geom::Area;
use super::resolve::{Problem, Resolved};
use anyhow::{Context, Result};
use jobrt::resume_log::{BlockId, Get, Pieces, Put};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The pieces' format, in their log's identity (`Regions::keep_in`) and every lasting piece's key:
/// a log or piece of another format is another identity's, never read. 2: a problem's level
/// (`warning`) kept with it. 3: a region's land kept with its outline where a remainder reaches
/// (#1448). 4: the land, remainder and water pieces keyed by what each reads, and kept without
/// what the extract lacks (told as their parts are read, before the piece: #1548).
pub const FORMAT: u32 = 4;

/// The stages a piece is of (its block's first number).
pub const RESOLVED: u8 = 1;
pub const LAND: u8 = 2;
pub const WATER: u8 = 3;
/// A folder's remainder (`tiles::remainders`).
pub const REMAINDER: u8 = 4;
/// A region with water's stored shapes and drawing (`tiles::stored`).
pub const STORED: u8 = 5;
/// A hand-out's moves (the hand-outs in `tiles::resolve`).
pub const HAND_OUT: u8 = 6;
/// The cells a folder's remainder reaches (`tiles::remainder_supports`).
pub const SUPPORT: u8 = 7;
/// A cell's part of the cover check (`tiles::cover_cell`).
pub const COVER: u8 = 8;

/// What each stage is called (`Kept::counts`).
pub fn stage_name(stage: u8) -> &'static str {
    match stage {
        RESOLVED => "resolved",
        LAND => "land",
        WATER => "water",
        REMAINDER => "remainder",
        STORED => "stored",
        HAND_OUT => "hand-out",
        SUPPORT => "remainder's cells",
        COVER => "cover check's cell",
        _ => "?",
    }
}

/// The pieces, and what this run made of them (a check resolves several versions, each region's
/// key the same where it reads the same).
#[derive(Default)]
pub struct Kept {
    pieces: Mutex<Option<Pieces>>,
    lasting: std::sync::OnceLock<Lasting>,
    /// The pieces this run made, for the versions it resolves after (none with a lasting folder,
    /// which holds them).
    made: Mutex<HashMap<BlockId, Arc<Vec<u8>>>>,
    /// By stage: the pieces taken (kept before, by this run or an earlier one) and made.
    counts: Mutex<BTreeMap<u8, (usize, usize)>>,
    stop: Mutex<Option<anyhow::Error>>,
    /// Tests: jobrt::resume_log::STOP_AFTER as it was at `open`, counted here (the pieces are kept
    /// on the resolution's own threads, where the test's thread-local isn't).
    #[cfg(test)]
    stop_after: Mutex<Option<usize>>,
    /// Tests: each piece made, its stage and key.
    #[cfg(test)]
    made_keys: Mutex<Vec<(u8, String)>>,
}

impl Kept {
    /// Pieces kept in `scratch` for identity `id` from now on (`keep`: resuming on).
    pub fn open(&self, scratch: &std::path::Path, id: &str, keep: bool) {
        *self.pieces.lock().unwrap() = Some(Pieces::open(scratch, (0, 0, 0), "inputs", id, keep));
        #[cfg(test)]
        {
            *self.stop_after.lock().unwrap() = jobrt::resume_log::STOP_AFTER.take();
        }
    }

    /// Pieces kept in the lasting folder `l` too from now on.
    pub fn open_lasting(&self, l: Lasting) -> Result<()> {
        anyhow::ensure!(self.lasting.set(l).is_ok(), "a lasting folder opened twice");
        Ok(())
    }

    /// The piece of stage `stage` and key `key`: kept (by an earlier run, or this one), else
    /// `make`'s, kept (a safe point). None once a pause came (nothing made then).
    pub fn get_or_make<T>(
        &self,
        stage: u8,
        key: &str,
        make: impl FnOnce() -> T,
        enc: impl Fn(&T, &mut Put),
        dec: impl Fn(&mut Get) -> Result<T>,
    ) -> Option<T> {
        if self.stop.lock().unwrap().is_some() {
            return None;
        }
        if let Some(v) = self.take(stage, key, dec) {
            return Some(v);
        }
        let v = make();
        self.keep(stage, key, &v, enc);
        Some(v)
    }

    /// Whether a pause came (`stopped`): a piece not taken is given up, not made.
    pub fn stopping(&self) -> bool {
        self.stop.lock().unwrap().is_some()
    }

    fn block(stage: u8, key: &str) -> BlockId {
        let h = blake3::hash(format!("{stage} {key}").as_bytes());
        (
            stage,
            u32::from_le_bytes(h.as_bytes()[..4].try_into().unwrap()),
            u32::from_le_bytes(h.as_bytes()[4..8].try_into().unwrap()),
        )
    }

    /// The piece of stage `stage` and key `key`, when kept (by an earlier run, or this one) and
    /// read back whole.
    pub fn take<T>(&self, stage: u8, key: &str, dec: impl Fn(&mut Get) -> Result<T>) -> Option<T> {
        let b = Self::block(stage, key);
        let p = self
            .made
            .lock()
            .unwrap()
            .get(&b)
            .cloned()
            .or_else(|| self.pieces.lock().unwrap().as_ref().and_then(|p| p.read_kept(b)).map(Arc::new))
            .or_else(|| self.lasting.get().and_then(|l| l.read(stage, key)).map(Arc::new))?;
        let mut g = Get(&p);
        let read = (|| -> Result<T> {
            anyhow::ensure!(g.bytes()? == key.as_bytes(), "another key's");
            let v = dec(&mut g)?;
            g.end()?;
            Ok(v)
        })();
        match read {
            Ok(v) => {
                self.counts.lock().unwrap().entry(stage).or_default().0 += 1;
                Some(v)
            }
            Err(e) => {
                eprintln!("regions: a kept piece not taken ({e:#}): made again");
                None
            }
        }
    }

    /// Piece `v` of stage `stage` and key `key`, made: kept (a safe point; a pause met here is the
    /// check's, `stopped`).
    pub fn keep<T>(&self, stage: u8, key: &str, v: &T, enc: impl Fn(&T, &mut Put)) {
        let b = Self::block(stage, key);
        let mut p = Put::default();
        p.bytes(key.as_bytes());
        enc(v, &mut p);
        let p = Arc::new(p.0);
        self.counts.lock().unwrap().entry(stage).or_default().1 += 1;
        #[cfg(test)]
        self.made_keys.lock().unwrap().push((stage, key.to_string()));
        if let Some(l) = self.lasting.get() {
            // (Not kept: made again next time, which is only slower.)
            if let Err(e) = l.write(stage, key, &p) {
                eprintln!("regions: a piece not kept in {}: {e:#}", l.dir.display());
            }
        }
        if let Some(pieces) = self.pieces.lock().unwrap().as_mut() {
            if let Err(e) = pieces.keep(b, &p) {
                self.stop.lock().unwrap().get_or_insert(e);
            }
            #[cfg(test)]
            {
                let mut n = self.stop_after.lock().unwrap();
                match *n {
                    Some(k) if k <= 1 => {
                        *n = None;
                        self.stop.lock().unwrap().get_or_insert(anyhow::Error::new(jobrt::control::Paused));
                    }
                    Some(k) => *n = Some(k - 1),
                    None => {}
                }
            }
        }
        // (Held for the versions resolved after, unless the lasting folder holds it: read back from
        // there, not held twice.)
        if self.lasting.get().is_none() {
            self.made.lock().unwrap().insert(b, p);
        }
    }

    /// Tests: the pieces made since the last call (not those taken), each its stage and key.
    #[cfg(test)]
    pub fn made_keys(&self) -> Vec<(u8, String)> {
        std::mem::take(&mut *self.made_keys.lock().unwrap())
    }

    /// By stage, the pieces this run took (kept before) and made.
    pub fn counts(&self) -> BTreeMap<u8, (usize, usize)> {
        self.counts.lock().unwrap().clone()
    }

    /// The check done with a lasting folder: what this run took of it and made, by stage, and the
    /// folder held to its cap (`Lasting::sweep`), each said on stderr after `who`; nothing without
    /// one. A folder not held to its cap is said, and the check goes on: its outcome is as good.
    pub fn finish_lasting(&self, who: &str) {
        let Some(l) = self.lasting.get() else { return };
        for (stage, (taken, made)) in self.counts() {
            eprintln!("{who}: pieces of {}: {taken} taken as kept, {made} made", stage_name(stage));
        }
        match l.sweep() {
            Err(e) => eprintln!("{who}: {} not held to its cap: {e:#}", l.dir.display()),
            Ok(s) => eprintln!(
                "{who}: {} holds {} pieces, {:.0} MB (its cap {} MB); {} deleted, {:.0} MB, the least recently used",
                l.dir.display(),
                s.files,
                s.bytes as f64 / 1e6,
                l.cap_mb,
                s.deleted,
                s.deleted_bytes as f64 / 1e6
            ),
        }
    }

    /// The check can't go on (`e`): it stops with it as with a pause (`stopped`), and no piece is
    /// made from now on.
    pub fn fail(&self, e: anyhow::Error) {
        self.stop.lock().unwrap().get_or_insert(e);
    }

    /// The pause a piece kept met, or the error the check failed with (`fail`), if one did: the
    /// check stops with it.
    pub fn stopped(&self) -> Result<()> {
        match self.stop.lock().unwrap().take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The check done: the pieces deleted.
    pub fn finish(&self) {
        if let Some(p) = self.pieces.lock().unwrap().take() {
            p.finish();
        }
    }
}

/// A lasting folder of pieces (`scenic-build regions-check --keep <dir>`): each piece a file,
/// `<blake3 of its whole key>.piece`, its whole key being `prefix` (the pieces' `FORMAT`,
/// `RESOLUTION`, the regions' code digest and what the check reads besides its folder), the
/// piece's stage and its own key (what it reads). So a folder is safe across versions: a piece of
/// other code, another format or other reads has another name, and is never read. A file holds
/// `MAGIC`, the blake3 of the rest, then the whole key and the piece: one cut short or changed fails
/// its hash, one of another key (a digest's collision) isn't taken, and either is made again.
/// Written beside its name and renamed into place, so a check stopped midway leaves no part of a
/// piece under a piece's name; checks may share a folder.
pub struct Lasting {
    pub dir: PathBuf,
    prefix: String,
    /// The most it holds once a check is done (`sweep`), MB.
    cap_mb: u64,
}

/// The code a lasting piece is made by, in its key: `RESOLUTION` and the regions' code digest
/// (`inputs::code_of`, every item of the module and all it names, as compiled).
pub fn code_key(resolution: u32, code: &str) -> String {
    format!("resolution {resolution} code {code}")
}

/// A lasting folder's cap (MB) when `--keep-mb` names none, and the pool's when its Mac's
/// setting does (agentcore's agent::settings, the build page's "Regions check's pieces").
pub const KEEP_MB: u64 = 10_000;

/// The pool's check's lasting folder (task #1566), in this Mac's cache (the job's
/// `buildstore::coverage::CACHE_ENV`): on its local disk, never the NAS (a whole check writes
/// about 21,100 small files, 16,200 of them the cover check's cells). Room-making frees it as it
/// frees the other cheap caches (agentcore's agent::room), the least recently used pieces first.
pub const POOL_DIR: &str = "regions-pieces";

/// The pool's check's lasting folder's cap (MB), as its agent gives it to the job from its Mac's
/// setting. Not set (a job run by hand), or no cache: no lasting folder.
pub const POOL_MB_ENV: &str = "SCENIC_REGIONS_PIECES_MB";

/// The pool's check's lasting folder and cap, as its job is given them (`POOL_DIR`, `POOL_MB_ENV`).
pub fn pool_lasting() -> Option<(PathBuf, u64)> {
    pool_lasting_of(std::env::var_os(buildstore::coverage::CACHE_ENV), std::env::var(POOL_MB_ENV).ok())
}

/// `pool_lasting` of the cache and the cap as given.
pub fn pool_lasting_of(cache: Option<std::ffi::OsString>, mb: Option<String>) -> Option<(PathBuf, u64)> {
    let cache = cache.filter(|v| !v.is_empty())?;
    Some((Path::new(&cache).join(POOL_DIR), mb?.trim().parse().ok()?))
}

/// A lasting piece's first bytes.
const MAGIC: &[u8; 16] = b"regions-piece 1\n";

/// A lasting piece's file name's ending, and a piece being written's (`<name>.tmp-<pid>-<n>`).
const EXT: &str = "piece";
const TMP: &str = ".tmp-";

/// What `Lasting::sweep` found and deleted.
#[derive(Debug, Default, PartialEq)]
pub struct Swept {
    pub files: usize,
    pub bytes: u64,
    pub deleted: usize,
    pub deleted_bytes: u64,
}

impl Lasting {
    /// The folder `dir` (made if missing), for a check reading `context` besides its folder.
    pub fn open(dir: &Path, context: &str, cap_mb: u64) -> Result<Lasting> {
        Lasting::open_as(dir, context, cap_mb, &code_key(super::RESOLUTION, crate::inputs::code_of(module_path!())))
    }

    /// `open`, its pieces those of code `code` (`code_key`'s: tests, of another code).
    pub fn open_as(dir: &Path, context: &str, cap_mb: u64, code: &str) -> Result<Lasting> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Lasting { dir: dir.to_path_buf(), prefix: format!("regions pieces {FORMAT} {code}\n{context}"), cap_mb })
    }

    fn whole_key(&self, stage: u8, key: &str) -> String {
        format!("{}\n{stage} {key}", self.prefix)
    }

    fn path(&self, whole: &str) -> PathBuf {
        self.dir.join(format!("{}.{EXT}", blake3::hash(whole.as_bytes()).to_hex()))
    }

    /// The piece of `stage` and `key`, when kept whole (its time set to now: used, `sweep`).
    pub fn read(&self, stage: u8, key: &str) -> Option<Vec<u8>> {
        let whole = self.whole_key(stage, key);
        let path = self.path(&whole);
        let bytes = std::fs::read(&path).ok()?;
        match piece_in(&bytes, &whole) {
            Ok(p) => {
                if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                    f.set_modified(std::time::SystemTime::now()).ok();
                }
                Some(p)
            }
            Err(e) => {
                eprintln!("regions: {} not taken ({e:#}): made again", path.display());
                None
            }
        }
    }

    /// Piece `p` of `stage` and `key` kept.
    pub fn write(&self, stage: u8, key: &str, p: &[u8]) -> Result<()> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let whole = self.whole_key(stage, key);
        let path = self.path(&whole);
        let mut body = Put::default();
        body.bytes(whole.as_bytes());
        body.bytes(p);
        let mut out = MAGIC.to_vec();
        out.extend(blake3::hash(&body.0).as_bytes());
        out.extend(body.0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut tmp = path.clone().into_os_string();
        tmp.push(format!("{TMP}{}-{n}", std::process::id()));
        std::fs::write(&tmp, &out).with_context(|| format!("write {}", path.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))
    }

    /// The folder held to its cap (`sweep_dir`).
    pub fn sweep(&self) -> Result<Swept> {
        sweep_dir(&self.dir, self.cap_mb)
    }
}

/// A lasting folder `dir` held to `cap_mb`: its pieces (and any piece's write a stopped check
/// left), the most recently used first, kept while they fit; the first that doesn't, and every one
/// used before it, deleted. (By a check once it's done, and by the build page's setting of the
/// pool's cap as it's set.)
pub fn sweep_dir(dir: &Path, cap_mb: u64) -> Result<Swept> {
    let mut v: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(dir).with_context(|| format!("list {}", dir.display()))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.ends_with(&format!(".{EXT}")) || name.contains(TMP)) {
            continue;
        }
        // (A file another check's sweep deleted, or a write it renamed, since the listing: passed over.)
        let Ok(m) = e.metadata() else { continue };
        v.push((m.modified()?, m.len(), e.path()));
    }
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
    let cap = cap_mb.saturating_mul(1_000_000);
    let mut s = Swept::default();
    let mut full = false;
    for (_, len, path) in v {
        full |= s.bytes + len > cap;
        if full {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Ok(()) => {
                    s.deleted += 1;
                    s.deleted_bytes += len;
                }
                // (Left, said, and counted as held: the next check's sweep tries it again.)
                Err(e) => {
                    eprintln!("regions: {} not deleted: {e}", path.display());
                    s.files += 1;
                    s.bytes += len;
                }
            }
        } else {
            s.files += 1;
            s.bytes += len;
        }
    }
    Ok(s)
}

/// The piece a lasting file `bytes` holds, when it's whole and of key `whole`.
fn piece_in(bytes: &[u8], whole: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(bytes.len() >= MAGIC.len() + 32 && bytes[..MAGIC.len()] == MAGIC[..], "not a piece");
    let (hash, body) = bytes[MAGIC.len()..].split_at(32);
    anyhow::ensure!(blake3::hash(body).as_bytes() == hash, "cut short or changed");
    let mut g = Get(body);
    anyhow::ensure!(g.bytes()? == whole.as_bytes(), "another key's");
    let p = g.bytes()?;
    g.end()?;
    Ok(p)
}

/// A check name as `Problem` has it (static): the names read back are few, each kept once.
pub(super) fn check_name(s: &str) -> &'static str {
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut n = NAMES.lock().unwrap();
    if let Some(x) = n.iter().find(|x| **x == s) {
        return x;
    }
    let x: &'static str = Box::leak(s.to_string().into_boxed_str());
    n.push(x);
    x
}

fn strs(p: &mut Put, v: &[String]) {
    p.u64(v.len() as u64);
    for s in v {
        p.bytes(s.as_bytes());
    }
}

fn get_strs(g: &mut Get) -> Result<Vec<String>> {
    (0..g.count(8)?).map(|_| Ok(String::from_utf8(g.bytes()?)?)).collect()
}

pub fn problems(p: &mut Put, v: &[Problem]) {
    p.u64(v.len() as u64);
    for x in v {
        p.u8(x.warning.into());
        p.bytes(x.check.as_bytes());
        strs(p, &x.about);
        strs(p, &x.found);
        strs(p, &x.files);
        p.bytes(x.message.as_bytes());
    }
}

pub fn get_problems(g: &mut Get) -> Result<Vec<Problem>> {
    (0..g.count(40)?)
        .map(|_| {
            let warning = match g.u8()? {
                0 => false,
                1 => true,
                t => anyhow::bail!("a problem's level tagged {t}"),
            };
            Ok(Problem {
                warning,
                check: check_name(&String::from_utf8(g.bytes()?)?),
                about: get_strs(g)?,
                found: get_strs(g)?,
                files: get_strs(g)?,
                message: String::from_utf8(g.bytes()?)?,
            })
        })
        .collect()
}

pub fn area(p: &mut Put, a: &Area) {
    p.u64(a.polys.len() as u64);
    for poly in &a.polys {
        p.u64(poly.len() as u64);
        for ring in poly {
            p.u64(ring.len() as u64);
            for pt in ring {
                p.u64(pt[0] as u64);
                p.u64(pt[1] as u64);
            }
        }
    }
}

pub fn get_area(g: &mut Get) -> Result<Area> {
    let mut polys = Vec::new();
    for _ in 0..g.count(8)? {
        let mut poly = Vec::new();
        for _ in 0..g.count(8)? {
            let n = g.count(16)?;
            poly.push((0..n).map(|_| Ok([g.u64()? as i64, g.u64()? as i64])).collect::<Result<Vec<_>>>()?);
        }
        polys.push(poly);
    }
    Ok(Area { polys })
}

/// What it needed of the extract, as ids.
pub fn ids(p: &mut Put, v: &[super::extract::Id]) {
    strs(p, &v.iter().map(ToString::to_string).collect::<Vec<_>>());
}

pub fn get_ids(g: &mut Get) -> Result<Vec<super::extract::Id>> {
    get_strs(g)?.iter().map(|s| s.parse()).collect()
}

/// A resolution or its problems.
pub fn result<T>(p: &mut Put, r: &Result<T, Vec<Problem>>, ok: impl Fn(&mut Put, &T)) {
    match r {
        Ok(v) => {
            p.u8(1);
            ok(p, v);
        }
        Err(ps) => {
            p.u8(0);
            problems(p, ps);
        }
    }
}

pub fn get_result<T>(g: &mut Get, ok: impl Fn(&mut Get) -> Result<T>) -> Result<Result<T, Vec<Problem>>> {
    Ok(match g.u8()? {
        1 => Ok(ok(g)?),
        0 => Err(get_problems(g)?),
        t => anyhow::bail!("a result tagged {t}"),
    })
}

pub fn resolved(p: &mut Put, r: &Resolved) {
    p.bytes(&r.geometry.encode());
    p.bytes(&serde_json::to_vec(&r.drawn).unwrap_or_default());
    area(p, &r.lakes);
}

pub fn get_resolved(g: &mut Get) -> Result<Resolved> {
    Ok(Resolved {
        geometry: super::stored::Geometry::decode(&g.bytes()?)?,
        drawn: serde_json::from_slice(&g.bytes()?)?,
        lakes: get_area(g)?,
    })
}
